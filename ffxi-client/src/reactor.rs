//! Tactical reactor — the deterministic 200 ms loop that translates
//! high-level intent (Follow / Engage / PathTo) into per-tick `Move` /
//! `Action` packets, and emits high-signal threshold events (LowHp,
//! PartyMemberLowHp) so the LLM only wakes when something actually changed.
//!
//! Architecturally, the reactor is middleware that sits between external
//! clients (TUI, agent_io, ffxi-mcp) and `session::run`:
//!
//! ```text
//!     clients → external_cmd_rx → reactor → internal_cmd_tx → session
//!                                    ↑
//!                              event_tx ← session
//! ```
//!
//! Responsibilities:
//!  - **Forward** passthrough commands (Chat, Action, RequestZoneChange, …)
//!    unchanged.
//!  - **Absorb** goal commands (Follow, Engage, PathTo, Cancel) and drive
//!    them on a 200 ms tick by emitting Move / Action.
//!  - **Mirror** `SessionState` via `apply_event` so the reactor can read
//!    entity positions, party HP, etc. without coordinating with the session.
//!  - **Emit** `LowHp` / `PartyMemberLowHp` when crossings are detected
//!    (latched: one event per downward crossing, reset on rise).
//!
//! This file is split into a *pure* `Reactor` struct (tested in-module
//! without I/O) and an async `run` that wires it to channels.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use ffxi_nav::{glam, GridNav, NavMesh};
use ffxi_nav_recast::RecastNavMesh;

/// One-of-N nav implementation. Enum dispatch (vs `Box<dyn NavMesh>`)
/// avoids the trait-object `Send`-bound question — `RecastNavMesh`
/// holds raw FFI pointers from `recastnavigation-rs` whose `Send`-ness
/// isn't guaranteed by the binding crate. Adding new variants is the
/// natural place to hang Stage-3 zone collision or future tile-cache
/// formats.
enum LoadedNav {
    Recast(RecastNavMesh),
    Grid(GridNav),
}

impl NavMesh for LoadedNav {
    fn path(&self, from: glam::Vec3, to: glam::Vec3) -> Option<Vec<glam::Vec3>> {
        match self {
            LoadedNav::Recast(n) => n.path(from, to),
            LoadedNav::Grid(n) => n.path(from, to),
        }
    }
}
use tokio::sync::{broadcast, mpsc};

use crate::state::{
    ActionKind, AgentCommand, AgentEvent, EntityKind, ReactorGoalSnapshot, SessionState, Vec3,
};

/// Reactor parameters. Defaults match the plan's "tactical loop" target.
#[derive(Debug, Clone, Copy)]
pub struct ReactorConfig {
    /// Tick interval. At 30 Hz the reactor acts as a frame-rate movement
    /// integrator (matching how the OG FFXI client advances local
    /// position) — each tick emits a sub-yalm step rather than a coarse
    /// 1-yalm jump. Combat decisions (attack, trigger checks) also run
    /// at this rate; all of them are per-tick constant-time so the cost
    /// is negligible.
    pub tick: Duration,
    /// HP percent below which `LowHp` / `PartyMemberLowHp` fires.
    pub low_hp_threshold: u8,
    /// Per-tick movement step in yalms when chasing a follow/path target.
    /// Sized to keep velocity at FFXI's base run speed (~5 yalms/sec)
    /// regardless of tick rate: `tick_seconds * 5.0`. Going above this
    /// trips server-side speedhack checks and makes `/pathto` look
    /// teleport-y to the operator.
    pub max_step_per_tick: f32,
}

impl Default for ReactorConfig {
    fn default() -> Self {
        Self {
            // 30 Hz: matches the OG client's local-integration cadence
            // and is fine-grained enough that 1-frame jumps at 60 Hz
            // render look continuous.
            tick: Duration::from_millis(33),
            low_hp_threshold: 25,
            // 5 yalms/sec base × 33 ms tick ≈ 0.165 yalm/tick.
            max_step_per_tick: 0.165,
        }
    }
}

/// Active high-level intent. `Idle` produces no per-tick output. Each
/// non-idle variant is what the agent / LLM committed to last.
#[derive(Debug, Clone, PartialEq)]
pub enum Goal {
    Idle,
    Following {
        target_id: u32,
        distance: f32,
    },
    /// `attack_issued` flips true once the reactor has emitted the
    /// initial `Action::Attack` packet on transition; subsequent ticks
    /// only re-face the target.
    Engaged {
        target_id: u32,
        attack_issued: bool,
    },
    /// Pathing toward a destination, possibly via intermediate
    /// waypoints from `ffxi-nav`. `idx` indexes into `waypoints`; the
    /// final element is the requested destination. A straight-line
    /// path (no navmesh available) holds a single-element `waypoints`.
    /// Each tick steps toward `waypoints[idx]`; arrival advances idx.
    Pathing {
        waypoints: Vec<Vec3>,
        idx: usize,
    },
    /// Stage-9 banking goal: monitor inventory; when any field bag
    /// (Inventory / Mog Satchel / Mog Sack / Mog Case) reaches
    /// `threshold` slots filled, emit a `RequestZoneChange` to the
    /// configured mog-house zoneline and clear the goal (one-shot per
    /// banking cycle). Persists across reconnects.
    Banking {
        threshold: u8,
        mog_house_zoneline: u32,
    },
}

impl Default for Goal {
    fn default() -> Self {
        Goal::Idle
    }
}

/// Project the reactor's internal `Goal` into the serializable
/// `ReactorGoalSnapshot` mirror in `state.rs`. Exhaustive on the current
/// `Goal` enum on purpose — when Stage 9 adds `Goal::Banking` the merge
/// will surface a non-exhaustive-match compile error here, which is the
/// right outcome (forces the merger to wire `Banking → ReactorGoalSnapshot::Banking`).
fn snapshot_goal(goal: &Goal) -> ReactorGoalSnapshot {
    match goal {
        Goal::Idle => ReactorGoalSnapshot::Idle,
        Goal::Following {
            target_id,
            distance,
        } => ReactorGoalSnapshot::Following {
            target_id: *target_id,
            distance: *distance,
        },
        Goal::Engaged {
            target_id,
            attack_issued,
        } => ReactorGoalSnapshot::Engaged {
            target_id: *target_id,
            attack_issued: *attack_issued,
        },
        Goal::Pathing { waypoints, idx } => {
            // Surface the *final* destination so renderers stay simple
            // ("path → (x,y,z)"); attach the count of waypoints still
            // ahead (including the destination) so a multi-waypoint
            // route can show a "[3 wp]" badge.
            let dest = waypoints.last().copied().unwrap_or(Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            });
            let remaining = waypoints.len().saturating_sub(*idx).max(1) as u32;
            ReactorGoalSnapshot::Pathing {
                x: dest.x,
                y: dest.y,
                z: dest.z,
                waypoints_remaining: remaining,
            }
        }
        Goal::Banking {
            threshold,
            mog_house_zoneline,
        } => ReactorGoalSnapshot::Banking {
            threshold: *threshold,
            mog_house_zoneline: *mog_house_zoneline,
        },
    }
}

/// CONTAINER_IDs the banking goal monitors. Mirrors
/// `Phoenix/src/map/item_container.h::CONTAINER_ID`. We watch the four
/// "field bags" — the containers that fill during play. Wardrobes are
/// equipment slots (don't fill in normal play); safes / lockers are
/// where banking _puts_ items, not where it triggers.
const FIELD_BAG_CONTAINERS: [u8; 4] = [
    0, // LOC_INVENTORY
    5, // LOC_MOGSATCHEL
    6, // LOC_MOGSACK
    7, // LOC_MOGCASE
];

/// What `handle_command` decided to do with a client command:
/// optionally forward something to the session, optionally broadcast
/// some derived events. Most paths produce one or the other; `Snapshot`
/// produces both. Goal-mutating commands (Follow/Engage/PathTo/Cancel
/// and Move's goal-clear) emit a `ReactorGoalChanged` so renderers can
/// reflect the live intent.
#[derive(Debug, Default)]
pub struct CommandRouting {
    pub forward: Option<AgentCommand>,
    pub derived_events: Vec<AgentEvent>,
}

impl CommandRouting {
    fn absorbed_with_goal(goal: ReactorGoalSnapshot) -> Self {
        Self {
            forward: None,
            derived_events: vec![AgentEvent::ReactorGoalChanged { goal }],
        }
    }
    fn forward(cmd: AgentCommand) -> Self {
        Self {
            forward: Some(cmd),
            derived_events: Vec::new(),
        }
    }
    fn forward_with_goal(cmd: AgentCommand, goal: ReactorGoalSnapshot) -> Self {
        Self {
            forward: Some(cmd),
            derived_events: vec![AgentEvent::ReactorGoalChanged { goal }],
        }
    }
}

/// One reactor-tick's output. Most ticks produce zero commands and zero
/// derived events; `Pathing → Idle` self-clearing surfaces a
/// `ReactorGoalChanged` so the operator HUD reflects it without
/// requiring the agent to issue an explicit `Cancel`.
#[derive(Debug, Default)]
pub struct TickOutput {
    pub commands: Vec<AgentCommand>,
    pub derived_events: Vec<AgentEvent>,
}

/// Pure reactor — no I/O. Drives a state machine off observed events and
/// emits commands on tick. The async `run` wraps this with channels.
pub struct Reactor {
    cfg: ReactorConfig,
    state: SessionState,
    goal: Goal,
    /// Latched: `LowHp` fires when self HP crosses *below* the threshold;
    /// stays latched until HP rises back above. Same pattern per-party-member.
    self_low_hp_latched: bool,
    party_low_hp_latched: HashMap<u32, bool>,
    /// Single-zone navmesh cache. When the agent zones, the next
    /// `PathTo` reloads. `None` means we either haven't tried to load
    /// for the current zone yet or there's no heightmap available
    /// (Stage 10b ships the wiring; per-zone PNG acquisition is a
    /// later operator task).
    nav_cache: Option<(u16, LoadedNav)>,
    /// `line_id` of the zone-line trigger box the player is currently
    /// inside. Used for edge-triggered `RequestZoneChange` emission:
    /// each tick we recompute "am I inside a trigger?", and only fire
    /// the 0x05E packet when the answer flips from `None` to `Some(L)`.
    /// Without the latch we'd spam the server 5×/sec while the player
    /// stands on the trigger.
    zoneline_trigger_latched: Option<u32>,
    /// Set on `ZoneChanged`; the next `check_zoneline_trigger` adopts
    /// whatever trigger the player spawns inside as the baseline latch
    /// without firing. Prevents an immediate zone-back when the server
    /// drops us on top of the return zoneline (the destination side of
    /// the doorway sits inside its own AABB).
    needs_zone_seed: bool,
    /// Active forced-position override, set by `AgentEvent::ForcedMove`.
    /// During its lifetime, input-driven `Move` emission is suppressed
    /// and the reactor emits per-tick `Move`s lerping the self position
    /// toward `target`. When `expiry` passes, the override clears and
    /// normal goal-driven behavior resumes.
    ///
    /// This covers the LSB WPOS / WPOS2 family (cutscene end, zone-line
    /// re-anchor, homepoint, GM warp). LSB does **not** ship combat
    /// knockback over the wire — it's a BATTLE2 animation hint the
    /// retail client integrates locally — so synthetic knockback packets
    /// in tests use the same override path to exercise the contract.
    reactor_override: Option<ReactorOverride>,
}

/// Active forced-position override window. Mirrors the design in
/// `ffxi-proto::decode::ForcedMove` (server-driven re-anchor).
#[derive(Debug, Clone, Copy)]
pub struct ReactorOverride {
    /// Server-authoritative destination in our z-up frame.
    pub target: Vec3,
    /// Server-supplied heading byte for the destination.
    pub heading: u8,
    /// When the override window ends; after this `Instant`, the reactor
    /// resumes normal input-driven movement.
    pub expiry: Instant,
}

impl Reactor {
    pub fn new(cfg: ReactorConfig) -> Self {
        Self {
            cfg,
            state: SessionState::default(),
            goal: Goal::Idle,
            self_low_hp_latched: false,
            party_low_hp_latched: HashMap::new(),
            nav_cache: None,
            zoneline_trigger_latched: None,
            needs_zone_seed: false,
            reactor_override: None,
        }
    }

    /// Currently-active forced-position override, if any. The reactor
    /// suppresses input-driven `Move` and lerps toward `override.target`
    /// while this is `Some` and `Instant::now() < override.expiry`.
    pub fn current_override(&self) -> Option<ReactorOverride> {
        self.reactor_override
    }

    /// True iff a forced-position override window is currently active.
    /// Clears the stored override lazily when its expiry has passed so
    /// callers don't re-check the timestamp.
    fn override_active(&mut self) -> bool {
        match self.reactor_override {
            Some(ov) if Instant::now() < ov.expiry => true,
            Some(_) => {
                self.reactor_override = None;
                false
            }
            None => false,
        }
    }

    /// Test escape hatch: install a forced-position override directly
    /// without depending on wall-clock `Instant` arithmetic. Tests can
    /// expire the window by stepping `expiry` back into the past.
    #[cfg(test)]
    pub fn set_override_for_test(&mut self, target: Vec3, heading: u8, ttl: Duration) {
        self.reactor_override = Some(ReactorOverride {
            target,
            heading,
            expiry: Instant::now() + ttl,
        });
    }

    pub fn current_goal(&self) -> &Goal {
        &self.goal
    }

    /// Test escape hatch: pre-populate the navmesh cache with a
    /// fixture so unit tests can exercise the multi-waypoint pathing
    /// branch without writing a PNG to disk.
    #[cfg(test)]
    pub fn set_nav_for_test(&mut self, zone_id: u16, nav: GridNav) {
        self.nav_cache = Some((zone_id, LoadedNav::Grid(nav)));
    }

    /// Resolve the current zone's navmesh, lazy-loading it from
    /// upstream xiNavmeshes (Recast/Detour) or the
    /// `~/.config/ffxi-mcp/heightmaps/<zone_id>.png` fallback on the
    /// first `PathTo` after a zone change. Returns `None` (and leaves
    /// the cache unset) when neither is available — the caller falls
    /// back to a single-segment straight-line path.
    fn ensure_nav_loaded(&mut self) -> Option<&LoadedNav> {
        let zone_id = self.state.zone_id?;
        let cached = matches!(&self.nav_cache, Some((z, _)) if *z == zone_id);
        if !cached {
            self.nav_cache = default_load_navmesh(zone_id).map(|n| (zone_id, n));
        }
        self.nav_cache.as_ref().map(|(_, n)| n)
    }

    /// Build the waypoint list for a `PathTo`. Uses navmesh A* when
    /// available; otherwise a single-segment straight line. Either
    /// way, the final element is always the requested destination.
    fn build_waypoints(&mut self, target: Vec3) -> Vec<Vec3> {
        let cur = self.self_pos();
        let nav = self.ensure_nav_loaded();
        if let Some(nav) = nav {
            let from = glam::Vec3::new(cur.x, cur.y, cur.z);
            let to = glam::Vec3::new(target.x, target.y, target.z);
            if let Some(path) = nav.path(from, to) {
                let mut waypoints: Vec<Vec3> = path
                    .into_iter()
                    .map(|v| Vec3 {
                        x: v.x,
                        y: v.y,
                        z: v.z,
                    })
                    .collect();
                // The first waypoint coincides with `from`; skip it so
                // the agent starts moving toward the next corner.
                if waypoints.first().map_or(false, |w| {
                    horizontal_distance(*w, cur) < self.cfg.max_step_per_tick
                }) {
                    waypoints.remove(0);
                }
                // Off-mesh last-mile: Detour's `find_straight_path`
                // ends at the *snapped* destination poly, not the
                // requested point. Zone-line origins (and many operator
                // path targets) sit a few yalms off the walkable mesh
                // — at the doorframe threshold, at the edge of a
                // platform, etc. If we stop at the snapped end, the
                // agent never enters the trigger box. Append the real
                // target as a final straight-line waypoint when the
                // gap is more than two step-sizes (small overshoots
                // are already inside the path's tolerance).
                let snapped_end_is_target = waypoints.last().is_some_and(|w| {
                    horizontal_distance(*w, target) <= self.cfg.max_step_per_tick * 2.0
                });
                if !snapped_end_is_target {
                    waypoints.push(target);
                }
                if waypoints.is_empty() {
                    waypoints.push(target);
                }
                return waypoints;
            }
            tracing::warn!(
                zone = self.state.zone_id,
                "navmesh found but produced no path — straight-lining"
            );
        }
        vec![target]
    }

    /// Fold an event into the mirror, then derive any threshold-crossing
    /// events the reactor wants to broadcast.
    pub fn observe_event(&mut self, ev: &AgentEvent) -> Vec<AgentEvent> {
        // Aggro-edge detection runs *before* apply_event because we need
        // the old `bt_target_id` to detect a transition into us.
        let mut out = self.detect_aggro_edge(ev);
        // Install a forced-position override for the lifetime of the
        // server's re-anchor window. Suppresses outbound Move emission
        // during the window; tick_override lerps toward the target.
        if let AgentEvent::ForcedMove {
            target,
            duration_ms,
            ..
        } = ev
        {
            self.reactor_override = Some(ReactorOverride {
                target: target.pos,
                heading: target.heading,
                expiry: Instant::now() + Duration::from_millis(*duration_ms as u64),
            });
        }
        self.state.apply_event(ev);
        // Zone-change invalidates any in-flight `Goal::Pathing` — the
        // waypoints were computed in the old zone's coord frame and
        // would cause `Move` spam toward nonsense locations in the new
        // zone (lands the player "in the sky" or off-map). Drop them.
        if matches!(ev, AgentEvent::ZoneChanged { .. }) {
            self.goal = Goal::Idle;
            self.needs_zone_seed = true;
        }
        out.extend(self.detect_threshold_events(ev));
        out
    }

    /// Detect a `BtTargetID` transition where some non-self entity goes
    /// from "not targeting me" to "targeting me". Emits exactly one
    /// `EngagedBy` per such transition.
    fn detect_aggro_edge(&self, ev: &AgentEvent) -> Vec<AgentEvent> {
        let Some(self_id) = self.state.char_id else {
            return Vec::new();
        };
        let AgentEvent::EntityUpserted { entity } = ev else {
            return Vec::new();
        };
        if entity.id == self_id {
            return Vec::new();
        }
        // Pets are now reliably classified (via `0x068 CPetSyncPacket`
        // enrichment in `session::handle_sub_packet`), so we can drop
        // them from aggro detection alongside friendly PCs/NPCs. Mobs
        // (and trusts mid-spawn, before their EntitySetName arrives)
        // remain `Other` here — those are the kinds we want to flag.
        if matches!(
            entity.kind,
            EntityKind::Pc | EntityKind::Npc | EntityKind::Pet
        ) {
            return Vec::new();
        }
        let now_targeting_self = entity.bt_target_id == self_id;
        if !now_targeting_self {
            return Vec::new();
        }
        let was_targeting_self = self
            .state
            .entities
            .iter()
            .find(|e| e.id == entity.id)
            .map(|prev| prev.bt_target_id == self_id)
            .unwrap_or(false);
        if was_targeting_self {
            return Vec::new();
        }
        vec![AgentEvent::EngagedBy {
            entity_id: entity.id,
        }]
    }

    fn detect_threshold_events(&mut self, ev: &AgentEvent) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        match ev {
            AgentEvent::EntityUpserted { entity } => {
                if Some(entity.id) == self.state.char_id {
                    if let Some(pct) = entity.hp_pct {
                        let now_low = pct < self.cfg.low_hp_threshold;
                        if now_low && !self.self_low_hp_latched {
                            out.push(AgentEvent::LowHp { pct });
                            self.self_low_hp_latched = true;
                        } else if !now_low {
                            self.self_low_hp_latched = false;
                        }
                    }
                }
            }
            AgentEvent::PartyMemberUpdated { member } => {
                let now_low = member.hp_pct < self.cfg.low_hp_threshold;
                let latched = self
                    .party_low_hp_latched
                    .get(&member.id)
                    .copied()
                    .unwrap_or(false);
                if now_low && !latched {
                    out.push(AgentEvent::PartyMemberLowHp {
                        id: member.id,
                        pct: member.hp_pct,
                    });
                    self.party_low_hp_latched.insert(member.id, true);
                } else if !now_low && latched {
                    self.party_low_hp_latched.insert(member.id, false);
                }
            }
            _ => {}
        }
        out
    }

    /// Process a command from a client. Goal commands are absorbed and
    /// return `None`; everything else passes through. An explicit `Move`
    /// also clears the goal — the agent has overridden tactical control.
    /// `Snapshot` is forwarded to the session (for `Diagnostics`) but the
    /// reactor *also* renders a `SceneSummary` from its mirror — that's
    /// the one place the SessionState mirror surfaces to clients.
    pub fn handle_command(&mut self, cmd: AgentCommand) -> CommandRouting {
        match cmd {
            AgentCommand::Follow {
                target_id,
                distance,
            } => {
                self.goal = Goal::Following {
                    target_id,
                    distance,
                };
                CommandRouting::absorbed_with_goal(snapshot_goal(&self.goal))
            }
            AgentCommand::Engage { target_id } => {
                self.goal = Goal::Engaged {
                    target_id,
                    attack_issued: false,
                };
                CommandRouting::absorbed_with_goal(snapshot_goal(&self.goal))
            }
            AgentCommand::PathTo { x, y, z } => {
                let target = Vec3 { x, y, z };
                let waypoints = self.build_waypoints(target);
                self.goal = Goal::Pathing { waypoints, idx: 0 };
                CommandRouting::absorbed_with_goal(snapshot_goal(&self.goal))
            }
            AgentCommand::Cancel => {
                self.goal = Goal::Idle;
                CommandRouting::absorbed_with_goal(snapshot_goal(&self.goal))
            }
            AgentCommand::BankWhenFull {
                threshold,
                mog_house_zoneline,
            } => {
                self.goal = Goal::Banking {
                    threshold,
                    mog_house_zoneline,
                };
                CommandRouting::absorbed_with_goal(snapshot_goal(&self.goal))
            }
            AgentCommand::Move { .. } => {
                // Drop input-driven Move while a server-initiated forced
                // position is in flight. The reactor's per-tick lerp owns
                // movement until the override expires; forwarding a
                // user Move now would race the server's re-anchor and
                // either land us back at the pre-warp spot or trip the
                // anti-speedhack heuristic in session.
                if self.override_active() {
                    return CommandRouting::default();
                }
                // Manual `Move` overrides whatever goal was active. Only
                // emit ReactorGoalChanged if we actually transitioned —
                // otherwise spurious Idle→Idle events flood the log.
                let was_active = !matches!(self.goal, Goal::Idle);
                self.goal = Goal::Idle;
                if was_active {
                    CommandRouting::forward_with_goal(cmd, snapshot_goal(&self.goal))
                } else {
                    CommandRouting::forward(cmd)
                }
            }
            AgentCommand::Snapshot => {
                let summary = crate::scene::SceneSummary::from_state(&self.state);
                CommandRouting {
                    forward: Some(AgentCommand::Snapshot),
                    derived_events: vec![AgentEvent::SceneSummary { text: summary.text }],
                }
            }
            other => CommandRouting::forward(other),
        }
    }

    /// One tick. Returns commands to issue plus any derived events
    /// (e.g. `ReactorGoalChanged` when `Pathing` self-clears to `Idle`).
    /// Most ticks: zero or one command, zero events.
    pub fn tick(&mut self) -> TickOutput {
        // Forced-position override owns this tick if active — it
        // suppresses goal-driven output and emits a lerp Move toward
        // the server-authoritative target. Zone-line auto-triggers stay
        // disabled during the window so a re-anchor that puts the
        // player on a trigger doesn't immediately re-zone them.
        if let Some(out) = self.tick_override() {
            return out;
        }
        // Goal-driven output first; the zone-line auto-trigger runs
        // after so we can append a `RequestZoneChange` command if the
        // player just walked onto a trigger box this tick.
        let mut out = self.tick_goal();
        if let Some(req) = self.check_zoneline_trigger() {
            out.commands.push(req);
        }
        out
    }

    /// Per-tick lerp toward the server-authoritative `target` of the
    /// active forced-position override. Returns `None` when no override
    /// is active (caller falls through to normal goal-driven output).
    fn tick_override(&mut self) -> Option<TickOutput> {
        if !self.override_active() {
            return None;
        }
        let ov = self.reactor_override?;
        let cur = self.self_pos();
        let dist = horizontal_distance(cur, ov.target);
        // When we're close enough, snap to the target and clear early —
        // future ticks no-op until the window naturally expires; the
        // suppression flag in handle_command stays armed until then so
        // queued user input doesn't fight the server's re-anchor.
        let stepped = if dist <= self.cfg.max_step_per_tick {
            ov.target
        } else {
            step_point(cur, ov.target, self.cfg.max_step_per_tick)
        };
        Some(TickOutput {
            commands: vec![mk_move(stepped, ov.heading)],
            derived_events: Vec::new(),
        })
    }

    /// Edge-triggered zone-line detection. If the player's current
    /// position falls inside a trigger box for this zone *and* it's
    /// a different trigger than the one we last latched, emit a
    /// `RequestZoneChange` so the session sends `0x05E MAPRECT` and
    /// the server fires the transition.
    ///
    /// FFXI's zone-change protocol is **client-initiated**: standing
    /// on a trigger does nothing if we never ask. The retail client
    /// computes this same check tick-by-tick; we mirror that here.
    fn check_zoneline_trigger(&mut self) -> Option<AgentCommand> {
        let zone_id = self.state.zone_id?;
        let player = self.self_pos();
        let lines = ffxi_nav::zone_lines_for(zone_id);
        // Diagnostic: log any trigger we're within 5y of so we can tell
        // "client never entered the box" from "client entered, server
        // rejected" without instrumenting the network path. Logs at most
        // a handful of lines per second while loitering at a marker;
        // silent otherwise.
        for line in lines {
            let dx = player.x - line.from_pos[0];
            let dy = player.y - line.from_pos[1];
            let ground_dist = (dx * dx + dy * dy).sqrt();
            if ground_dist <= 5.0 {
                tracing::debug!(
                    line_id = line.line_id,
                    to_zone = line.to_zone,
                    player_xy = format!("({:.2},{:.2})", player.x, player.y),
                    trigger_xy = format!("({:.2},{:.2})", line.from_pos[0], line.from_pos[1]),
                    scale_x = line.scale_x,
                    scale_z = line.scale_z,
                    rotation = format!("{:.3}", line.rotation),
                    ground_dist = format!("{:.2}", ground_dist),
                    inside = is_inside_trigger_box(player, line),
                    "near zoneline trigger",
                );
            }
        }
        let inside = lines
            .iter()
            .find(|line| is_inside_trigger_box(player, line))
            .map(|line| line.line_id);
        if self.needs_zone_seed {
            self.zoneline_trigger_latched = inside;
            self.needs_zone_seed = false;
            return None;
        }
        let was = self.zoneline_trigger_latched;
        self.zoneline_trigger_latched = inside;
        match (was, inside) {
            // Just entered a trigger (edge: outside → inside).
            (None, Some(line_id)) => Some(AgentCommand::RequestZoneChange { line_id }),
            // Crossed directly from one trigger box to another (rare
            // but possible at a junction). Fire for the new one.
            (Some(prev), Some(line_id)) if prev != line_id => {
                Some(AgentCommand::RequestZoneChange { line_id })
            }
            // Inside the same trigger as last tick, or outside both
            // ticks. No edge — say nothing.
            _ => None,
        }
    }

    fn tick_goal(&mut self) -> TickOutput {
        match self.goal.clone() {
            Goal::Idle => TickOutput::default(),
            Goal::Following {
                target_id,
                distance,
            } => TickOutput {
                commands: self
                    .step_toward_entity(target_id, distance)
                    .map(|m| vec![m])
                    .unwrap_or_default(),
                derived_events: Vec::new(),
            },
            Goal::Engaged {
                target_id,
                attack_issued,
            } => {
                let mut commands = Vec::new();
                if !attack_issued {
                    if let Some((act_index, _)) = self.entity_target_info(target_id) {
                        commands.push(AgentCommand::Action {
                            target_id,
                            target_index: act_index,
                            kind: ActionKind::Attack,
                        });
                        if let Goal::Engaged { attack_issued, .. } = &mut self.goal {
                            *attack_issued = true;
                        }
                    }
                }
                if let Some(m) = self.face_entity(target_id) {
                    commands.push(m);
                }
                TickOutput {
                    commands,
                    derived_events: Vec::new(),
                }
            }
            Goal::Pathing { waypoints, idx } => {
                if waypoints.get(idx).is_none() {
                    // Empty or exhausted — clear to Idle defensively.
                    self.goal = Goal::Idle;
                    return TickOutput {
                        commands: Vec::new(),
                        derived_events: vec![AgentEvent::ReactorGoalChanged {
                            goal: snapshot_goal(&self.goal),
                        }],
                    };
                }
                let step = self.effective_step_per_tick();
                if step <= 0.0 {
                    // Server-set speed is zero (Bind / Stun / Sleep /
                    // zoning) — emit no Move and don't claim arrival.
                    // We'll resume pathing once the server restores
                    // speed and the next tick sees step > 0.
                    return TickOutput {
                        commands: Vec::new(),
                        derived_events: Vec::new(),
                    };
                }

                // Consume `step` across as many waypoints as fit in one
                // tick. Detour-produced navmesh paths frequently have
                // sub-step waypoint spacing through tight corridors and
                // around corners; the naive "snap to wp, advance idx,
                // wait for next tick" pattern would advance only
                // `dist_to_next_wp` per tick on those segments — easily
                // a fraction of a full step — dropping /pathto's
                // effective speed well below free-walk speed.
                //
                // We snap through every waypoint within the remaining
                // budget, then step partway toward the next. Heading
                // tracks whichever segment we're currently traversing
                // so the player faces the right way as they round
                // corners.
                let mut cur = self.self_pos();
                let mut budget = step;
                let mut idx_local = idx;
                // First iteration always sets `heading` (the pre-check
                // above guarantees `waypoints.get(idx)` is Some).
                let mut heading = 0u8;
                let mut path_done = false;
                loop {
                    let Some(wp) = waypoints.get(idx_local).copied() else {
                        path_done = true;
                        break;
                    };
                    heading = heading_toward(cur, wp);
                    let dist = horizontal_distance(cur, wp);
                    if dist <= budget {
                        cur = wp;
                        budget -= dist;
                        idx_local += 1;
                        if budget <= 0.0 {
                            break;
                        }
                    } else {
                        cur = step_point(cur, wp, budget);
                        break;
                    }
                }

                let mut derived_events = Vec::new();
                if path_done {
                    self.goal = Goal::Idle;
                    derived_events.push(AgentEvent::ReactorGoalChanged {
                        goal: snapshot_goal(&self.goal),
                    });
                } else if idx_local != idx {
                    if let Goal::Pathing { idx: ref mut i, .. } = self.goal {
                        *i = idx_local;
                    }
                    // Surface the waypoint advance(s) so the HUD's
                    // "[N wp]" count decreases visibly per corner —
                    // one event per tick even if we snapped through
                    // multiple waypoints, since the count reflects
                    // post-tick state.
                    derived_events.push(AgentEvent::ReactorGoalChanged {
                        goal: snapshot_goal(&self.goal),
                    });
                }

                TickOutput {
                    commands: vec![mk_move(cur, heading)],
                    derived_events,
                }
            }
            Goal::Banking {
                threshold,
                mog_house_zoneline,
            } => {
                // Wait for the inventory mirror to be authoritative —
                // pre-flood, slot counts are unreliable and we'd
                // false-trigger on an empty bag.
                if !self.state.inventory.all_loaded {
                    return TickOutput::default();
                }
                let any_full = FIELD_BAG_CONTAINERS.iter().any(|id| {
                    self.state
                        .inventory
                        .containers
                        .get(id)
                        .map(|c| c.slots.len() as u8 >= threshold)
                        .unwrap_or(false)
                });
                if !any_full {
                    return TickOutput::default();
                }
                // One-shot: clear the goal so we don't re-emit on every
                // tick while the zone change is in flight. The agent
                // can re-issue `bank_when_full` after dropping items.
                self.goal = Goal::Idle;
                TickOutput {
                    commands: vec![AgentCommand::RequestZoneChange {
                        line_id: mog_house_zoneline,
                    }],
                    derived_events: vec![AgentEvent::ReactorGoalChanged {
                        goal: snapshot_goal(&self.goal),
                    }],
                }
            }
        }
    }

    fn self_pos(&self) -> Vec3 {
        // Single source of truth: the self entity in the entity list,
        // seeded from `0x00A LOGIN`'s `PosHead` on zone-in and updated
        // by CHAR_PC + `AgentCommand::Move`. Returns origin only during
        // the brief pre-LOGIN window — agents that rely on `self_pos()`
        // for path-finding accept this as "we don't know yet" (idle
        // ticks no-op when no waypoint computation is in flight).
        self.state
            .self_position()
            .map(|p| p.pos)
            .unwrap_or_default()
    }

    fn entity_target_info(&self, target_id: u32) -> Option<(u16, Vec3)> {
        self.state
            .entities
            .iter()
            .find(|e| e.id == target_id)
            .map(|e| (e.act_index, e.pos))
    }

    fn step_toward_entity(&self, target_id: u32, hold_distance: f32) -> Option<AgentCommand> {
        let (_, target_pos) = self.entity_target_info(target_id)?;
        let cur = self.self_pos();
        let dist = horizontal_distance(cur, target_pos);
        if dist <= hold_distance {
            return None;
        }
        let step = self.effective_step_per_tick();
        if step <= 0.0 {
            // Server speed=0 — let the keepalive hold position rather
            // than emit a Move that would advance our local self_pos
            // and then get broadcast as a speedhack-shaped delta.
            return None;
        }
        let step_size = (dist - hold_distance).min(step);
        let stepped = step_point(cur, target_pos, step_size);
        Some(mk_move(stepped, heading_toward(cur, target_pos)))
    }

    /// Effective per-tick step size in yalms, honoring server-set speed.
    ///
    /// FFXI is authoritative on movement speed: the server publishes
    /// the current effective speed in every `0x00D` packet (PosHead).
    /// Modifiers — Bind / Stun / Sleep / Slow / encumbrance / mounts /
    /// movement-buff gear — all land here as `PosHead::speed`. Sending
    /// position deltas that exceed `expected_speed * elapsed` triggers
    /// the server-side speedhack heuristic (`MAX_DISTANCE_WARP` /
    /// per-tick velocity check in LSB's `data_session.cpp`), which is
    /// what gets accounts auto-flagged on private servers.
    ///
    /// Policy:
    /// - **speed == 0** (bound/stunned/zoning): return 0 so callers
    ///   suppress Move emission entirely. The keepalive still
    ///   re-broadcasts our last position, which is what the server
    ///   expects for an immobilized character.
    /// - **speed > 0**: scale `max_step_per_tick` by `speed/speed_base`,
    ///   capped at 2× as a paranoia bound (real values stay close to 1,
    ///   ~2 for mounts; anything wilder is likely a decoder bug).
    /// - **no PosHead yet** (pre-LOGIN): return base unmodified — the
    ///   reactor doesn't emit movement before `Stage::InZone` anyway.
    fn effective_step_per_tick(&self) -> f32 {
        let Some(pos) = self.state.self_position() else {
            return self.cfg.max_step_per_tick;
        };
        if pos.speed == 0 {
            return 0.0;
        }
        let base = pos.speed_base.max(1) as f32;
        let ratio = (pos.speed as f32 / base).min(2.0);
        self.cfg.max_step_per_tick * ratio
    }

    fn face_entity(&self, target_id: u32) -> Option<AgentCommand> {
        let (_, target_pos) = self.entity_target_info(target_id)?;
        let cur = self.self_pos();
        Some(mk_move(cur, heading_toward(cur, target_pos)))
    }
}

/// True if `player` (z-up: `.x` east, `.y` north, `.z` height) is
/// inside the zone-line's 2D ground trigger box. Box is centered at
/// `from_pos`, sized `scale_x` × `scale_z`, rotated by `rotation`
/// radians around its center in the (x, y) ground plane.
///
/// Height isn't bounded here — the server's `0x05E MAPRECT` handler
/// does its own ~40-yalm distance check (per session.rs:891), which
/// covers the vertical axis. Adding our own height bound would just
/// false-negative when the navmesh and trigger sit at slightly
/// different heights.
fn is_inside_trigger_box(player: Vec3, line: &ffxi_nav::ZoneLine) -> bool {
    let dx = player.x - line.from_pos[0];
    let dy = player.y - line.from_pos[1];
    let cos_r = line.rotation.cos();
    let sin_r = line.rotation.sin();
    // Inverse-rotate (dx, dy) into box-local axes.
    let local_x = dx * cos_r + dy * sin_r;
    let local_y = -dx * sin_r + dy * cos_r;
    local_x.abs() <= line.scale_x / 2.0 && local_y.abs() <= line.scale_z / 2.0
}

fn horizontal_distance(a: Vec3, b: Vec3) -> f32 {
    // FFXI wire convention (per `ffxi-proto::decode::PosHead` byte-order
    // remap and `session::build_subpacket_pos` symmetry): codebase
    // `Position` is z-up — `pos.y` is north, `pos.z` is height. Ground
    // distance is therefore measured in the (x, y) plane.
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    (dx * dx + dy * dy).sqrt()
}

/// Step from `from` toward `to` by `step_size` yalms (in the x/y
/// ground plane; z = height is preserved). Codebase `Position` is
/// z-up; height in `.z` and the ground plane is `(x, y)`. If
/// `step_size` >= the actual distance, returns `to`.
fn step_point(from: Vec3, to: Vec3, step_size: f32) -> Vec3 {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let dist = (dx * dx + dy * dy).sqrt();
    if dist <= 1e-3 || step_size >= dist {
        return to;
    }
    let f = step_size / dist;
    Vec3 {
        x: from.x + dx * f,
        y: from.y + dy * f,
        z: from.z,
    }
}

/// FFXI heading: u8 spans 0..256 ↔ 0..2π. Mapping pinned by tests:
/// north (+y) = 0, east (+x) = 64, south (-y) = 128, west (-x) = 192.
/// Heading byte (0..=255) the server will read as "A is facing toward B."
///
/// Matches LSB's `worldAngle` in `vendor/server/src/common/utils.cpp:130-140`:
///
/// ```text
/// radians  = atan2f(B.z - A.z, B.x - A.x);
/// rawAngle = radians * -(128.0 / PI);
/// return   = (rawAngle mod 256 + 256) mod 256;
/// ```
///
/// LSB convention: heading 0 = +x (east), 64 = south, 128 = west, 192 = north,
/// clockwise viewed from above. The negation flips standard math-CCW into
/// FFXI's CW rotation. Our wire-side `Position` swaps the y/z axes
/// relative to LSB (our.y = LSB.z = north-south, our.z = LSB.y = vertical),
/// so the `atan2` arguments are `(our.y, our.x)` — same horizontal pair LSB
/// uses, just labelled differently. The vertical (`.z` in our coords) does
/// not influence facing.
///
/// Live-server `MsgBasic::UnableToSeeTarget` (`charentity.cpp::CanAttack`)
/// fires when the player's stored heading is outside a ±32-unit cone of
/// `worldAngle(player, target)` — so the byte we send here has to match
/// LSB's formula exactly.
fn heading_toward(from: Vec3, to: Vec3) -> u8 {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    if dx.abs() < 1e-3 && dy.abs() < 1e-3 {
        return 0;
    }
    let radians = dy.atan2(dx);
    let raw = radians * -(128.0 / std::f32::consts::PI);
    // round() handles atan2's quarter-mark rounding (exact π → 127.999…).
    (raw.round() as i32).rem_euclid(256) as u8
}

fn mk_move(pos: Vec3, heading: u8) -> AgentCommand {
    AgentCommand::Move {
        x: pos.x,
        y: pos.y,
        z: pos.z,
        heading,
    }
}

/// Resolve the LSB Detour navmesh path: `<dir>/<zone_id>.nav` where
/// `<dir>` is `$FFXI_NAVMESH_DIR` if set, otherwise the
/// `server/navmeshes` submodule walking up from the current working
/// directory. The submodule ships zone-id-numbered `.nav` files for
/// some zones and zone-name-named files for others — id-only lookup
/// here means we only resolve the numbered subset, which is fine for
/// the zones we care about right now (and `FFXI_NAVMESH_DIR` lets an
/// operator point elsewhere if they have a name-keyed mirror).
/// Find the on-disk Detour `.nav` for a zone. LSB ships the bulk of
/// these as zone-name-keyed (`Rabao.nav`, `West_Sarutabaruta.nav`); a
/// handful of legacy zones use numeric (`133.nav`). We try the name
/// first when known (covers most of the table), then fall back to
/// `<zone_id>.nav` (covers the legacy three). Unknown zone ids only
/// get the numeric attempt — we don't synthesize a name from `unknown`.
fn detour_navmesh_path(zone_id: u16) -> Option<PathBuf> {
    let base = if let Ok(custom) = std::env::var("FFXI_NAVMESH_DIR") {
        PathBuf::from(custom)
    } else {
        let cwd = std::env::current_dir().ok()?;
        find_navmesh_dir(&cwd)?
    };
    if let Some(name) = ffxi_nav::zone_name(zone_id) {
        let by_name = base.join(format!("{name}.nav"));
        if by_name.exists() {
            return Some(by_name);
        }
    }
    let by_id = base.join(format!("{zone_id}.nav"));
    if by_id.exists() {
        return Some(by_id);
    }
    // Return the name-or-id-shaped path the caller would have built
    // anyway, so the warn-log surfaces the right candidate even when
    // the file isn't there yet (operators may be hand-installing).
    Some(by_id)
}

/// Walk up from `start` looking for either `vendor/server/navmeshes/` or
/// `vendor/Phoenix/navmeshes/` — the LSB / Phoenix vendored submodule layout.
/// Returns the first directory that exists.
fn find_navmesh_dir(start: &std::path::Path) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        for sib in ["vendor/server/navmeshes", "vendor/Phoenix/navmeshes"] {
            let candidate = ancestor.join(sib);
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    None
}

/// PNG heightmap fallback: `~/.config/ffxi-mcp/heightmaps/<zone_id>.png`.
/// Used when no Detour `.nav` is available (or the loader for that
/// format isn't implemented yet — Stage 10c).
fn heightmap_png_path(zone_id: u16) -> Option<PathBuf> {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })?;
    Some(
        base.join("ffxi-mcp")
            .join("heightmaps")
            .join(format!("{zone_id}.png")),
    )
}

/// Try to load a navmesh for `zone_id`. Priority:
///
///   1. xiNavmeshes Recast/Detour `.nav` (auto-fetched + cached by
///      `ffxi-nav-recast::fetch`). Covers all 299 zones LSB ships.
///   2. Locally-installed Detour `.nav` under `$FFXI_NAVMESH_DIR` or
///      a sibling `vendor/server/navmeshes/` dir. Same file format,
///      different source — useful when running offline.
///   3. PNG occupancy heightmap from `~/.config/ffxi-mcp/heightmaps/`.
///   4. None → caller straight-lines, accepting that the path may
///      clip walls. The server validates moves so an illegal path
///      ends in a server-side reject, not undefined behavior.
fn default_load_navmesh(zone_id: u16) -> Option<LoadedNav> {
    // Zone 0 is the "unknown" sentinel (`ffxi_nav::zone_name(0)` →
    // `"unknown"`). No real play session uses it; a unit-test or
    // packet-decode glitch can report it spuriously. Short-circuit
    // before any disk / network side effect so tests stay hermetic.
    if zone_id == 0 {
        return None;
    }

    // 1. Local override path takes precedence over the auto-fetch
    //    cache, so operators with a hand-managed copy aren't forced
    //    onto the GitHub-hosted version.
    if let Some(detour_path) = detour_navmesh_path(zone_id) {
        if detour_path.exists() {
            match RecastNavMesh::from_path(&detour_path) {
                Ok(nav) => {
                    tracing::info!(
                        zone_id,
                        path = %detour_path.display(),
                        "navmesh loaded (local Detour)"
                    );
                    return Some(LoadedNav::Recast(nav));
                }
                Err(e) => {
                    tracing::warn!(
                        zone_id,
                        path = %detour_path.display(),
                        error = %e,
                        "local Detour .nav rejected; trying upstream"
                    );
                }
            }
        }
    }

    // 2. Upstream xiNavmeshes (download-on-first-use, cached after).
    match RecastNavMesh::for_zone(zone_id) {
        Ok(nav) => {
            tracing::info!(zone_id, "navmesh loaded (xiNavmeshes upstream)");
            return Some(LoadedNav::Recast(nav));
        }
        Err(ffxi_nav_recast::LoadError::NotAvailable(_)) => {
            tracing::debug!(zone_id, "no xiNavmeshes navmesh upstream; trying PNG");
        }
        Err(e) => {
            tracing::warn!(
                zone_id,
                error = %e,
                "xiNavmeshes load failed; trying PNG fallback"
            );
        }
    }

    // 3. PNG heightmap fallback.
    let png = heightmap_png_path(zone_id)?;
    if !png.exists() {
        return None;
    }
    match GridNav::from_png(&png, 128, glam::Vec2::ZERO, 1.0) {
        Ok(nav) => {
            tracing::info!(
                zone_id,
                path = %png.display(),
                "navmesh loaded (PNG fallback)"
            );
            Some(LoadedNav::Grid(nav))
        }
        Err(e) => {
            tracing::warn!(
                zone_id,
                path = %png.display(),
                error = %e,
                "navmesh PNG load failed — straight-lining"
            );
            None
        }
    }
}

/// Run the reactor as middleware in front of `session::run`. Spawns the
/// session as a child task; returns when either side exits. The caller
/// keeps the same `(cmd_rx, event_tx)` shape as `session::run`, so this
/// is a drop-in replacement when the agent wants the tactical loop.
pub async fn run(
    cfg: crate::session::Config,
    mut external_cmd_rx: mpsc::Receiver<AgentCommand>,
    event_tx: broadcast::Sender<AgentEvent>,
    reactor_cfg: ReactorConfig,
) -> Result<()> {
    let (internal_cmd_tx, internal_cmd_rx) = mpsc::channel(64);
    let mut event_rx = event_tx.subscribe();
    let session_event_tx = event_tx.clone();
    let mut session_handle =
        tokio::spawn(
            async move { crate::session::run(cfg, internal_cmd_rx, session_event_tx).await },
        );

    let mut reactor = Reactor::new(reactor_cfg);
    let mut tick = tokio::time::interval(reactor_cfg.tick);
    tick.tick().await;

    let result = loop {
        tokio::select! {
            biased;
            res = &mut session_handle => {
                break res.map_err(|e| anyhow::anyhow!("session task: {e}")).and_then(|r| r);
            }
            cmd = external_cmd_rx.recv() => match cmd {
                None => {
                    drop(internal_cmd_tx);
                    break (&mut session_handle).await
                        .map_err(|e| anyhow::anyhow!("session task: {e}"))
                        .and_then(|r| r);
                }
                Some(cmd) => {
                    let routing = reactor.handle_command(cmd);
                    for ev in routing.derived_events {
                        let _ = event_tx.send(ev);
                    }
                    if let Some(forward) = routing.forward {
                        if internal_cmd_tx.send(forward).await.is_err() {
                            // Session-side closed — wait for join.
                            break (&mut session_handle).await
                                .map_err(|e| anyhow::anyhow!("session task: {e}"))
                                .and_then(|r| r);
                        }
                    }
                }
            },
            ev = event_rx.recv() => match ev {
                Ok(ev) => {
                    for derived in reactor.observe_event(&ev) {
                        let _ = event_tx.send(derived);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => { /* best-effort */ }
                Err(broadcast::error::RecvError::Closed) => {
                    break (&mut session_handle).await
                        .map_err(|e| anyhow::anyhow!("session task: {e}"))
                        .and_then(|r| r);
                }
            },
            _ = tick.tick() => {
                // Trace-level: 5 Hz at the default 200 ms tick. Opt in via
                // `RUST_LOG=ffxi_client::reactor=trace` to profile the loop.
                // Plain event (not span) because EnteredSpan is `!Send` and
                // we await inside the loop.
                let tick_started = std::time::Instant::now();
                let TickOutput { commands, derived_events } = reactor.tick();
                let cmds_emitted = commands.len();
                for ev in derived_events {
                    let _ = event_tx.send(ev);
                }
                for cmd in commands {
                    if internal_cmd_tx.send(cmd).await.is_err() { break; }
                }
                tracing::trace!(
                    target: "ffxi_client::reactor",
                    elapsed_us = tick_started.elapsed().as_micros() as u64,
                    cmds_emitted,
                    "reactor.tick"
                );
            }
        }
    };

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Entity, EntityKind, PartyMember};

    /// Config pinned to a 1.0-yalm step so the per-tick movement
    /// assertions in this module stay readable. The production default
    /// is a much smaller per-tick step (frame-rate integration) — these
    /// tests verify the *logic* of stepping, not the tuning constant.
    fn step_test_cfg() -> ReactorConfig {
        ReactorConfig {
            max_step_per_tick: 1.0,
            ..ReactorConfig::default()
        }
    }

    fn upsert(id: u32, pos: Vec3, hp_pct: u8, kind: EntityKind, act_index: u16) -> AgentEvent {
        upsert_with_bt(id, pos, hp_pct, kind, act_index, 0)
    }

    fn upsert_with_bt(
        id: u32,
        pos: Vec3,
        hp_pct: u8,
        kind: EntityKind,
        act_index: u16,
        bt_target_id: u32,
    ) -> AgentEvent {
        upsert_with_speed(id, pos, hp_pct, kind, act_index, bt_target_id, 40, 40)
    }

    /// Variant for tests that need to exercise the server-speed safety
    /// branches (`Bind`/`Slow`/`Stun`). Defaults to `40/40` (normal PC
    /// walking) which matches what LSB sends in `PosHead`.
    fn upsert_with_speed(
        id: u32,
        pos: Vec3,
        hp_pct: u8,
        kind: EntityKind,
        act_index: u16,
        bt_target_id: u32,
        speed: u8,
        speed_base: u8,
    ) -> AgentEvent {
        AgentEvent::EntityUpserted {
            entity: Entity {
                id,
                act_index,
                kind,
                name: None,
                pos,
                heading: 0,
                hp_pct: Some(hp_pct),
                bt_target_id,
                claim_id: 0,
                speed,
                speed_base,
                look: None,
            },
        }
    }

    fn connected(char_id: u32) -> AgentEvent {
        AgentEvent::Connected {
            account_id: 0,
            char_id,
            character: "Tester".into(),
            zone_id: 0,
        }
    }

    fn party_update(id: u32, pct: u8) -> AgentEvent {
        AgentEvent::PartyMemberUpdated {
            member: PartyMember {
                id,
                act_index: 1,
                name: Some("M".into()),
                hp: 100,
                mp: 100,
                tp: 0,
                hp_pct: pct,
                mp_pct: 100,
                zone_no: 230,
                main_job: 1,
                main_job_lv: 75,
                sub_job: 6,
                sub_job_lv: 37,
                is_party_leader: false,
                is_alliance_leader: false,
                in_mog_house: false,
            },
        }
    }

    #[test]
    fn idle_tick_produces_nothing() {
        let mut r = Reactor::new(ReactorConfig::default());
        let out = r.tick();
        assert!(out.commands.is_empty());
        assert!(out.derived_events.is_empty());
    }

    #[test]
    fn follow_steps_toward_target_then_holds() {
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(1, Vec3::default(), 100, EntityKind::Pc, 1));
        r.observe_event(&upsert(
            2,
            Vec3 {
                x: 20.0,
                y: 0.0,
                z: 0.0,
            },
            100,
            EntityKind::Pc,
            2,
        ));
        r.handle_command(AgentCommand::Follow {
            target_id: 2,
            distance: 5.0,
        });

        let cmds = r.tick().commands;
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            AgentCommand::Move { x, .. } => {
                // step capped at step_test_cfg's max_step_per_tick (=1.0) → land at x=1.
                assert!(
                    (x - 1.0).abs() < 1e-3,
                    "step toward target capped at max_step: got {x}"
                );
            }
            other => panic!("expected Move, got {other:?}"),
        }

        // Self moves into the hold distance — reactor stops.
        r.observe_event(&upsert(
            1,
            Vec3 {
                x: 17.0,
                y: 0.0,
                z: 0.0,
            },
            100,
            EntityKind::Pc,
            1,
        ));
        assert!(r.tick().commands.is_empty(), "within distance: hold");
    }

    #[test]
    fn follow_against_unknown_target_emits_nothing() {
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(1, Vec3::default(), 100, EntityKind::Pc, 1));
        r.handle_command(AgentCommand::Follow {
            target_id: 999,
            distance: 5.0,
        });
        assert!(r.tick().commands.is_empty(), "no entity → no movement");
    }

    #[test]
    fn engage_emits_attack_once_then_only_face() {
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(1, Vec3::default(), 100, EntityKind::Pc, 1));
        r.observe_event(&upsert(
            99,
            Vec3 {
                x: 5.0,
                y: 0.0,
                z: 0.0,
            },
            100,
            EntityKind::Mob,
            7,
        ));
        r.handle_command(AgentCommand::Engage { target_id: 99 });

        let t1 = r.tick().commands;
        let attacks_t1 = t1
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    AgentCommand::Action {
                        kind: ActionKind::Attack,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(attacks_t1, 1, "tick 1 emits exactly one Attack");

        let t2 = r.tick().commands;
        let attacks_t2 = t2
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    AgentCommand::Action {
                        kind: ActionKind::Attack,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(attacks_t2, 0, "tick 2 does not re-issue Attack");
        // Still face the target (Move with same pos, heading toward target).
        assert!(t2.iter().any(|c| matches!(c, AgentCommand::Move { .. })));
    }

    #[test]
    fn cancel_clears_goal() {
        let mut r = Reactor::new(ReactorConfig::default());
        r.handle_command(AgentCommand::Engage { target_id: 99 });
        assert!(matches!(r.current_goal(), Goal::Engaged { .. }));
        r.handle_command(AgentCommand::Cancel);
        assert!(matches!(r.current_goal(), Goal::Idle));
        assert!(r.tick().commands.is_empty());
    }

    #[test]
    fn explicit_move_clears_goal_and_passes_through() {
        let mut r = Reactor::new(ReactorConfig::default());
        r.handle_command(AgentCommand::Follow {
            target_id: 2,
            distance: 5.0,
        });
        assert!(matches!(r.current_goal(), Goal::Following { .. }));
        let m = AgentCommand::Move {
            x: 1.0,
            y: 2.0,
            z: 3.0,
            heading: 64,
        };
        let routing = r.handle_command(m);
        assert!(
            matches!(routing.forward, Some(AgentCommand::Move { .. })),
            "Move passes through"
        );
        // Move-with-active-goal transitions to Idle; renderers learn via the
        // goal-changed event.
        assert!(matches!(
            routing.derived_events.as_slice(),
            [AgentEvent::ReactorGoalChanged {
                goal: ReactorGoalSnapshot::Idle
            }]
        ));
        assert!(matches!(r.current_goal(), Goal::Idle));
    }

    #[test]
    fn explicit_move_while_idle_emits_no_goal_event() {
        let mut r = Reactor::new(ReactorConfig::default());
        let m = AgentCommand::Move {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            heading: 0,
        };
        let routing = r.handle_command(m);
        assert!(matches!(routing.forward, Some(AgentCommand::Move { .. })));
        assert!(
            routing.derived_events.is_empty(),
            "no transition → no goal event (avoids Idle→Idle log spam)"
        );
    }

    #[test]
    fn passthrough_chat_unchanged() {
        let mut r = Reactor::new(ReactorConfig::default());
        let chat = AgentCommand::Chat {
            kind: 0,
            text: "hello".into(),
        };
        let routing = r.handle_command(chat);
        assert!(matches!(routing.forward, Some(AgentCommand::Chat { .. })));
        assert!(routing.derived_events.is_empty());
    }

    #[test]
    fn snapshot_emits_scene_summary_and_forwards() {
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        let routing = r.handle_command(AgentCommand::Snapshot);
        assert!(
            matches!(routing.forward, Some(AgentCommand::Snapshot)),
            "Snapshot still forwards to session for Diagnostics"
        );
        assert_eq!(routing.derived_events.len(), 1);
        assert!(matches!(
            &routing.derived_events[0],
            AgentEvent::SceneSummary { .. }
        ));
    }

    #[test]
    fn goal_commands_are_absorbed_no_forward() {
        // Goal-mutating commands stay in the reactor (no session forward).
        // The contract that's *changed* from the original test:
        // they now emit ReactorGoalChanged so renderers see live intent.
        let mut r = Reactor::new(ReactorConfig::default());
        for cmd in [
            AgentCommand::Follow {
                target_id: 1,
                distance: 5.0,
            },
            AgentCommand::Engage { target_id: 1 },
            AgentCommand::PathTo {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            },
            AgentCommand::Cancel,
        ] {
            let routing = r.handle_command(cmd);
            assert!(routing.forward.is_none());
        }
    }

    #[test]
    fn follow_emits_reactor_goal_changed() {
        let mut r = Reactor::new(ReactorConfig::default());
        let routing = r.handle_command(AgentCommand::Follow {
            target_id: 42,
            distance: 3.0,
        });
        assert!(routing.forward.is_none());
        match routing.derived_events.as_slice() {
            [AgentEvent::ReactorGoalChanged {
                goal:
                    ReactorGoalSnapshot::Following {
                        target_id,
                        distance,
                    },
            }] => {
                assert_eq!(*target_id, 42);
                assert!((*distance - 3.0).abs() < 1e-3);
            }
            other => panic!("expected single ReactorGoalChanged(Following), got {other:?}"),
        }
    }

    #[test]
    fn engage_emits_reactor_goal_changed() {
        let mut r = Reactor::new(ReactorConfig::default());
        let routing = r.handle_command(AgentCommand::Engage { target_id: 99 });
        assert!(routing.forward.is_none());
        match routing.derived_events.as_slice() {
            [AgentEvent::ReactorGoalChanged {
                goal:
                    ReactorGoalSnapshot::Engaged {
                        target_id,
                        attack_issued,
                    },
            }] => {
                assert_eq!(*target_id, 99);
                // The reactor sets attack_issued=false until the first tick;
                // the snapshot must reflect that.
                assert!(!*attack_issued, "attack_issued is false until first tick");
            }
            other => panic!("expected ReactorGoalChanged(Engaged), got {other:?}"),
        }
    }

    #[test]
    fn path_to_emits_reactor_goal_changed() {
        let mut r = Reactor::new(ReactorConfig::default());
        let routing = r.handle_command(AgentCommand::PathTo {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        });
        assert!(routing.forward.is_none());
        match routing.derived_events.as_slice() {
            [AgentEvent::ReactorGoalChanged {
                goal:
                    ReactorGoalSnapshot::Pathing {
                        x,
                        y,
                        z,
                        waypoints_remaining,
                    },
            }] => {
                assert!((*x - 1.0).abs() < 1e-3);
                assert!((*y - 2.0).abs() < 1e-3);
                assert!((*z - 3.0).abs() < 1e-3);
                // No navmesh in this test → straight-line single waypoint.
                assert_eq!(*waypoints_remaining, 1);
            }
            other => panic!("expected ReactorGoalChanged(Pathing), got {other:?}"),
        }
    }

    #[test]
    fn pathing_uses_navmesh_when_available() {
        // 10×10 grid, all walkable except a wall at column 5 (rows 0..7
        // blocked, rows 7..10 walkable). Start at (0, 0), goal at (9,
        // 0). The only route goes through some row >= 7. Without a
        // navmesh, the straight-line path would clip the wall.
        let mut walkable = vec![true; 100];
        for row in 0..7u32 {
            walkable[(row * 10 + 5) as usize] = false;
        }
        let nav =
            ffxi_nav::GridNav::from_walkable(10, 10, walkable, ffxi_nav::glam::Vec2::ZERO, 1.0);

        let mut r = Reactor::new(ReactorConfig::default());
        // Establish a zone so ensure_nav_loaded considers the cache key.
        r.state.zone_id = Some(123);
        r.set_nav_for_test(123, nav);

        let routing = r.handle_command(AgentCommand::PathTo {
            x: 9.0,
            y: 0.0,
            z: 0.0,
        });
        assert!(routing.forward.is_none());
        // The navmesh path must include at least one waypoint that
        // routes around the wall (row >= 7 in grid coords; since the
        // grid maps `x` → col and `z` → row at cell_size=1, that's
        // any waypoint with z >= 7).
        let goal = r.current_goal().clone();
        let Goal::Pathing { waypoints, idx } = &goal else {
            panic!("expected Pathing goal, got {goal:?}");
        };
        assert_eq!(*idx, 0);
        assert!(
            waypoints.iter().any(|w| w.z >= 7.0),
            "navmesh path should route around the wall, got {waypoints:?}"
        );
        assert!(
            waypoints.last().map(|w| w.x as i32 == 9).unwrap_or(false),
            "last waypoint should be the destination"
        );
    }

    #[test]
    fn cancel_clears_goal_emits_idle_event() {
        let mut r = Reactor::new(ReactorConfig::default());
        // Seed a non-idle goal first.
        let _ = r.handle_command(AgentCommand::Engage { target_id: 1 });
        // Now Cancel should emit Idle.
        let routing = r.handle_command(AgentCommand::Cancel);
        assert!(routing.forward.is_none());
        assert!(matches!(
            routing.derived_events.as_slice(),
            [AgentEvent::ReactorGoalChanged {
                goal: ReactorGoalSnapshot::Idle,
            }]
        ));
    }

    #[test]
    fn low_hp_emits_once_per_downward_crossing() {
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));

        // Above threshold — no event.
        let derived = r.observe_event(&upsert(1, Vec3::default(), 80, EntityKind::Pc, 1));
        assert!(derived.is_empty());

        // Cross down — emits.
        let derived = r.observe_event(&upsert(1, Vec3::default(), 20, EntityKind::Pc, 1));
        assert!(matches!(
            derived.as_slice(),
            [AgentEvent::LowHp { pct: 20 }]
        ));

        // Stay below — latched, no repeat.
        let derived = r.observe_event(&upsert(1, Vec3::default(), 15, EntityKind::Pc, 1));
        assert!(derived.is_empty(), "latched: no repeat");

        // Cross back up — reset latch.
        let derived = r.observe_event(&upsert(1, Vec3::default(), 80, EntityKind::Pc, 1));
        assert!(derived.is_empty());

        // Cross down again — re-emits.
        let derived = r.observe_event(&upsert(1, Vec3::default(), 10, EntityKind::Pc, 1));
        assert!(matches!(
            derived.as_slice(),
            [AgentEvent::LowHp { pct: 10 }]
        ));
    }

    #[test]
    fn party_member_low_hp_latches_per_member() {
        let mut r = Reactor::new(ReactorConfig::default());
        // Both above threshold.
        assert!(r.observe_event(&party_update(10, 80)).is_empty());
        assert!(r.observe_event(&party_update(11, 90)).is_empty());

        // 10 drops below — emits.
        let derived = r.observe_event(&party_update(10, 20));
        assert!(matches!(
            derived.as_slice(),
            [AgentEvent::PartyMemberLowHp { id: 10, pct: 20 }]
        ));

        // 11 still above; 10's latch doesn't gate 11.
        assert!(r.observe_event(&party_update(11, 30)).is_empty());

        // 11 drops below — emits independently.
        let derived = r.observe_event(&party_update(11, 10));
        assert!(matches!(
            derived.as_slice(),
            [AgentEvent::PartyMemberLowHp { id: 11, pct: 10 }]
        ));
    }

    #[test]
    fn pathing_walks_to_target_and_returns_to_idle() {
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(
            1,
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            100,
            EntityKind::Pc,
            1,
        ));
        // Target 0.5 yalms away — within step_test_cfg's max_step (1.0); reaches
        // in one tick.
        r.handle_command(AgentCommand::PathTo {
            x: 0.5,
            y: 0.0,
            z: 0.0,
        });
        let out = r.tick();
        assert_eq!(out.commands.len(), 1);
        match &out.commands[0] {
            AgentCommand::Move { x, z, .. } => {
                assert!((x - 0.5).abs() < 1e-3);
                assert!(z.abs() < 1e-3);
            }
            other => panic!("expected Move, got {other:?}"),
        }
        assert!(matches!(r.current_goal(), Goal::Idle));
    }

    #[test]
    fn pathing_self_clear_emits_idle_event() {
        // Tick-side emission: a pathing tick that completes the segment
        // must surface ReactorGoalChanged(Idle) so the operator HUD sees
        // the transition without needing the agent to issue Cancel.
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(
            1,
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            100,
            EntityKind::Pc,
            1,
        ));
        // Same single-tick distance as above.
        r.handle_command(AgentCommand::PathTo {
            x: 0.5,
            y: 0.0,
            z: 0.0,
        });
        let out = r.tick();
        assert!(matches!(r.current_goal(), Goal::Idle));
        assert!(matches!(
            out.derived_events.as_slice(),
            [AgentEvent::ReactorGoalChanged {
                goal: ReactorGoalSnapshot::Idle,
            }]
        ));
    }

    #[test]
    fn pathing_takes_multiple_ticks_for_distant_target() {
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(
            1,
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            100,
            EntityKind::Pc,
            1,
        ));
        r.handle_command(AgentCommand::PathTo {
            x: 12.0,
            y: 0.0,
            z: 0.0,
        });
        // Tick 1: step max_step_per_tick (=1.0 from step_test_cfg) yalms, still pathing.
        let out = r.tick();
        match &out.commands[0] {
            AgentCommand::Move { x, .. } => assert!((x - 1.0).abs() < 1e-3),
            other => panic!("got {other:?}"),
        }
        assert!(matches!(r.current_goal(), Goal::Pathing { .. }));
        assert!(
            out.derived_events.is_empty(),
            "mid-path tick should not emit goal-changed"
        );
    }

    #[test]
    fn pathing_consumes_step_across_multiple_waypoints() {
        // Regression: before the step-remainder loop, a tick that snapped
        // to a sub-step waypoint would advance idx and stop, dropping
        // effective speed proportional to waypoint density. With the loop,
        // one tick should consume as many waypoints as fit in `step` and
        // emit a single Move at the cumulative position.
        //
        // step_test_cfg.max_step_per_tick = 1.0. Construct a path with
        // four waypoints 0.2 yalms apart (total 0.8 yalms) — comfortably
        // inside one tick's budget.
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(
            1,
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            100,
            EntityKind::Pc,
            1,
        ));
        r.goal = Goal::Pathing {
            waypoints: vec![
                Vec3 {
                    x: 0.2,
                    y: 0.0,
                    z: 0.0,
                },
                Vec3 {
                    x: 0.4,
                    y: 0.0,
                    z: 0.0,
                },
                Vec3 {
                    x: 0.6,
                    y: 0.0,
                    z: 0.0,
                },
                Vec3 {
                    x: 0.8,
                    y: 0.0,
                    z: 0.0,
                },
            ],
            idx: 0,
        };

        let out = r.tick();
        assert_eq!(out.commands.len(), 1);
        match &out.commands[0] {
            AgentCommand::Move { x, y, .. } => {
                assert!(
                    (x - 0.8).abs() < 1e-3,
                    "tick should consume all four 0.2-yalm waypoints in one budget of 1.0, got x={x}"
                );
                assert!(y.abs() < 1e-3);
            }
            other => panic!("expected Move, got {other:?}"),
        }
        // Path complete → Idle + one event.
        assert!(matches!(r.current_goal(), Goal::Idle));
        assert!(matches!(
            out.derived_events.as_slice(),
            [AgentEvent::ReactorGoalChanged {
                goal: ReactorGoalSnapshot::Idle,
            }]
        ));
    }

    #[test]
    fn pathing_partial_consume_carries_remainder_into_next_segment() {
        // Six waypoints 0.3 yalms apart (total 1.5 yalms). One tick at
        // step=1.0 should snap through three waypoints (using 0.9 budget)
        // and step 0.1 into the fourth — landing at x=1.0 exactly.
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(
            1,
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            100,
            EntityKind::Pc,
            1,
        ));
        r.goal = Goal::Pathing {
            waypoints: (1..=6)
                .map(|i| Vec3 {
                    x: 0.3 * i as f32,
                    y: 0.0,
                    z: 0.0,
                })
                .collect(),
            idx: 0,
        };

        let out = r.tick();
        match &out.commands[0] {
            AgentCommand::Move { x, .. } => {
                assert!(
                    (x - 1.0).abs() < 1e-3,
                    "expected x=1.0 (0.3+0.3+0.3+0.1), got {x}"
                );
            }
            other => panic!("expected Move, got {other:?}"),
        }
        // idx should have advanced past three full waypoints; still pathing.
        let Goal::Pathing { idx, .. } = r.current_goal() else {
            panic!("expected still Pathing");
        };
        assert_eq!(*idx, 3);
        // Multiple waypoint advances surface as one ReactorGoalChanged.
        assert!(matches!(
            out.derived_events.as_slice(),
            [AgentEvent::ReactorGoalChanged { .. }]
        ));
    }

    #[test]
    fn heading_toward_pins_cardinal_quarters() {
        // LSB convention (server authoritative, see
        // vendor/server/src/common/utils.cpp:130-140 `worldAngle`): heading
        // 0 = +x (east), 64 = south, 128 = west, 192 = north — clockwise
        // viewed from above. Our `Position` swaps y/z relative to LSB so
        // cardinal directions live in the (x, y) horizontal plane, with
        // `+y` = north (LSB.z) and `+x` = east (LSB.x).
        let origin = Vec3::default();
        // East (+x): dy.atan2(dx) = atan2(0, +) = 0 → raw 0 → heading 0.
        assert_eq!(
            heading_toward(
                origin,
                Vec3 {
                    x: 10.0,
                    y: 0.0,
                    z: 0.0
                }
            ),
            0
        );
        // South (-y): atan2(-, 0) = -π/2 → raw +64 → heading 64.
        assert_eq!(
            heading_toward(
                origin,
                Vec3 {
                    x: 0.0,
                    y: -10.0,
                    z: 0.0
                }
            ),
            64
        );
        // West (-x): atan2(0, -) = π → raw -128 → heading 128.
        assert_eq!(
            heading_toward(
                origin,
                Vec3 {
                    x: -10.0,
                    y: 0.0,
                    z: 0.0
                }
            ),
            128
        );
        // North (+y): atan2(+, 0) = π/2 → raw -64 → heading 192.
        assert_eq!(
            heading_toward(
                origin,
                Vec3 {
                    x: 0.0,
                    y: 10.0,
                    z: 0.0
                }
            ),
            192
        );
    }

    #[test]
    fn step_point_caps_at_target() {
        let from = Vec3::default();
        let to = Vec3 {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        };
        // step_size > distance: clamp at target.
        let p = step_point(from, to, 100.0);
        assert!((p.x - 1.0).abs() < 1e-3);
    }

    #[test]
    fn engaged_by_emits_on_mob_targeting_self() {
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        // First sighting: mob isn't targeting us.
        let derived = r.observe_event(&upsert_with_bt(
            99,
            Vec3::default(),
            100,
            EntityKind::Other,
            7,
            0,
        ));
        assert!(derived.is_empty(), "no aggro on initial sighting");

        // Mob now targets self → emit EngagedBy.
        let derived = r.observe_event(&upsert_with_bt(
            99,
            Vec3::default(),
            100,
            EntityKind::Other,
            7,
            1,
        ));
        assert!(matches!(
            derived.as_slice(),
            [AgentEvent::EngagedBy { entity_id: 99 }]
        ));
    }

    #[test]
    fn engaged_by_does_not_repeat_while_target_held() {
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        // Initial state: not targeting.
        r.observe_event(&upsert_with_bt(
            99,
            Vec3::default(),
            100,
            EntityKind::Other,
            7,
            0,
        ));
        // Aggro!
        let d1 = r.observe_event(&upsert_with_bt(
            99,
            Vec3::default(),
            100,
            EntityKind::Other,
            7,
            1,
        ));
        assert_eq!(d1.len(), 1);
        // Same target across the next tick — not a new edge.
        let d2 = r.observe_event(&upsert_with_bt(
            99,
            Vec3::default(),
            100,
            EntityKind::Other,
            7,
            1,
        ));
        assert!(d2.is_empty(), "no repeat while target unchanged");
        // Mob disengages and re-engages → emits again.
        r.observe_event(&upsert_with_bt(
            99,
            Vec3::default(),
            100,
            EntityKind::Other,
            7,
            0,
        ));
        let d3 = r.observe_event(&upsert_with_bt(
            99,
            Vec3::default(),
            100,
            EntityKind::Other,
            7,
            1,
        ));
        assert_eq!(d3.len(), 1, "re-engage after release fires again");
    }

    #[test]
    fn zoneline_trigger_fires_once_on_entry_and_latches() {
        // Northern San d'Oria's west exit (line 845493882, → zone 100).
        // Post-swap from_pos: (-113.372, -57.418, -4.075) in z-up.
        // 3 yalm scale_x × 10 yalm scale_z, rotation 2.356 rad (135°).
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        r.state.zone_id = Some(230);

        // 1) Place self OUTSIDE all triggers — no fire.
        r.observe_event(&upsert(
            1,
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: -5.0,
            },
            100,
            EntityKind::Pc,
            1,
        ));
        let out1 = r.tick();
        assert!(
            !out1
                .commands
                .iter()
                .any(|c| matches!(c, AgentCommand::RequestZoneChange { .. })),
            "must not fire while outside any trigger"
        );

        // 2) Snap self ONTO the west-exit trigger center — should fire once.
        r.observe_event(&upsert(
            1,
            Vec3 {
                x: -113.372,
                y: -57.418,
                z: -4.075,
            },
            100,
            EntityKind::Pc,
            1,
        ));
        let out2 = r.tick();
        let req = out2
            .commands
            .iter()
            .find_map(|c| match c {
                AgentCommand::RequestZoneChange { line_id } => Some(*line_id),
                _ => None,
            })
            .expect("expected RequestZoneChange on entry");
        assert_eq!(req, 845493882, "should match the west-exit line_id");

        // 3) Tick again with self still on the trigger — must NOT re-fire
        //    (latched). Same upsert keeps position stable.
        r.observe_event(&upsert(
            1,
            Vec3 {
                x: -113.372,
                y: -57.418,
                z: -4.075,
            },
            100,
            EntityKind::Pc,
            1,
        ));
        let out3 = r.tick();
        assert!(
            !out3
                .commands
                .iter()
                .any(|c| matches!(c, AgentCommand::RequestZoneChange { .. })),
            "must not re-fire while still inside same trigger"
        );

        // 4) Walk OFF the trigger — no fire (still latched as None now).
        r.observe_event(&upsert(
            1,
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: -5.0,
            },
            100,
            EntityKind::Pc,
            1,
        ));
        let out4 = r.tick();
        assert!(
            !out4
                .commands
                .iter()
                .any(|c| matches!(c, AgentCommand::RequestZoneChange { .. })),
            "must not fire on leave"
        );

        // 5) Walk back ON — must fire again (re-entry).
        r.observe_event(&upsert(
            1,
            Vec3 {
                x: -113.372,
                y: -57.418,
                z: -4.075,
            },
            100,
            EntityKind::Pc,
            1,
        ));
        let out5 = r.tick();
        assert!(
            out5.commands
                .iter()
                .any(|c| matches!(c, AgentCommand::RequestZoneChange { .. })),
            "re-entry must fire fresh RequestZoneChange"
        );
    }

    #[test]
    fn zoneline_trigger_seeds_on_zone_change_no_immediate_refire() {
        // Regression: after zoning into Southern San d'Oria (zone 230),
        // the server drops the player on top of the *return* zoneline
        // back to West Ronfaure (line 845493882). Without seeding, the
        // latch left over from the prior zone's exit trigger doesn't
        // match this new line_id, so check_zoneline_trigger fires a
        // RequestZoneChange on the very first tick in the new zone —
        // bouncing the player straight back out.
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));

        // Simulate having just fired an exit trigger in the prior zone:
        // pre-load the latch with a stale line_id from "the old zone".
        r.zoneline_trigger_latched = Some(812855930);

        // Now zone in to 230, and have the server's first position
        // upsert place us inside the return trigger.
        r.observe_event(&AgentEvent::ZoneChanged {
            from: Some(100),
            to: 230,
        });
        r.state.zone_id = Some(230);
        r.observe_event(&upsert(
            1,
            Vec3 {
                x: -113.372,
                y: -57.418,
                z: -4.075,
            },
            100,
            EntityKind::Pc,
            1,
        ));

        // First post-zone tick: must NOT fire, even though we're inside
        // a trigger and the stale latch doesn't match it.
        let out1 = r.tick();
        assert!(
            !out1
                .commands
                .iter()
                .any(|c| matches!(c, AgentCommand::RequestZoneChange { .. })),
            "must not fire on first tick after ZoneChanged (spawn-inside grace)"
        );
        assert_eq!(
            r.zoneline_trigger_latched,
            Some(845493882),
            "seed should adopt the spawn-inside trigger as the baseline latch"
        );

        // Walk OFF the trigger — no fire.
        r.observe_event(&upsert(
            1,
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: -5.0,
            },
            100,
            EntityKind::Pc,
            1,
        ));
        let out2 = r.tick();
        assert!(
            !out2
                .commands
                .iter()
                .any(|c| matches!(c, AgentCommand::RequestZoneChange { .. })),
            "walking off the seeded trigger must not fire"
        );

        // Walk back ON — this is a real edge, must fire.
        r.observe_event(&upsert(
            1,
            Vec3 {
                x: -113.372,
                y: -57.418,
                z: -4.075,
            },
            100,
            EntityKind::Pc,
            1,
        ));
        let out3 = r.tick();
        assert!(
            out3.commands
                .iter()
                .any(|c| matches!(c, AgentCommand::RequestZoneChange { .. })),
            "deliberate re-entry after seeding must fire"
        );
    }

    #[test]
    fn engaged_by_skips_friendly_entities_and_self() {
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        // PC targeting us — skipped.
        let d = r.observe_event(&upsert_with_bt(
            50,
            Vec3::default(),
            100,
            EntityKind::Pc,
            2,
            1,
        ));
        assert!(d.is_empty(), "PCs aren't aggro");
        // NPC targeting us — skipped.
        let d = r.observe_event(&upsert_with_bt(
            60,
            Vec3::default(),
            100,
            EntityKind::Npc,
            3,
            1,
        ));
        assert!(d.is_empty(), "NPCs aren't aggro");
        // Self entity (id == char_id) — skipped.
        let d = r.observe_event(&upsert_with_bt(
            1,
            Vec3::default(),
            100,
            EntityKind::Pc,
            1,
            1,
        ));
        assert!(d.is_empty(), "self isn't aggroing self");
    }

    fn inv_capacities(caps: [u16; 18]) -> AgentEvent {
        AgentEvent::InventoryUpdated {
            container: 0,
            update: crate::state::InventoryUpdate::Capacities {
                capacities: caps.to_vec(),
            },
        }
    }

    fn inv_slot(container: u8, index: u8, item_no: u16) -> AgentEvent {
        AgentEvent::InventoryUpdated {
            container,
            update: crate::state::InventoryUpdate::SlotChanged {
                slot: crate::state::ItemSlot {
                    index,
                    item_no,
                    quantity: 1,
                    locked: false,
                    price: 0,
                },
            },
        }
    }

    #[test]
    fn bank_when_full_holds_until_all_loaded() {
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        // Configure capacities & seed a full bag, but don't signal AllLoaded.
        let mut caps = [0u16; 18];
        caps[0] = 80;
        r.observe_event(&inv_capacities(caps));
        for i in 0..30u8 {
            r.observe_event(&inv_slot(0, i, 4112));
        }
        r.handle_command(AgentCommand::BankWhenFull {
            threshold: 30,
            mog_house_zoneline: 12345,
        });
        let out = r.tick();
        assert!(
            out.commands.is_empty(),
            "must wait for InventoryReady before triggering"
        );
        assert!(matches!(r.current_goal(), Goal::Banking { .. }));
    }

    #[test]
    fn bank_when_full_emits_zoneline_when_threshold_crossed() {
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        let mut caps = [0u16; 18];
        caps[0] = 80;
        r.observe_event(&inv_capacities(caps));
        for i in 0..30u8 {
            r.observe_event(&inv_slot(0, i, 4112));
        }
        r.observe_event(&AgentEvent::InventoryReady);
        r.handle_command(AgentCommand::BankWhenFull {
            threshold: 30,
            mog_house_zoneline: 12345,
        });
        let out = r.tick();
        assert!(matches!(
            out.commands.as_slice(),
            [AgentCommand::RequestZoneChange { line_id: 12345 }]
        ));
        assert!(
            matches!(r.current_goal(), Goal::Idle),
            "one-shot — goal clears after firing"
        );
        assert!(matches!(
            out.derived_events.as_slice(),
            [AgentEvent::ReactorGoalChanged {
                goal: ReactorGoalSnapshot::Idle
            }]
        ));
    }

    #[test]
    fn bank_when_full_holds_when_under_threshold() {
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        let mut caps = [0u16; 18];
        caps[0] = 80;
        r.observe_event(&inv_capacities(caps));
        // Only 5 slots in inventory; threshold = 30.
        for i in 0..5u8 {
            r.observe_event(&inv_slot(0, i, 4112));
        }
        r.observe_event(&AgentEvent::InventoryReady);
        r.handle_command(AgentCommand::BankWhenFull {
            threshold: 30,
            mog_house_zoneline: 12345,
        });
        let out = r.tick();
        assert!(out.commands.is_empty());
        assert!(matches!(r.current_goal(), Goal::Banking { .. }));
    }

    #[test]
    fn bank_when_full_triggers_on_any_field_bag() {
        // A full Mog Satchel (LOC_MOGSATCHEL = 5) should trigger even
        // if Inventory itself isn't full.
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        let mut caps = [0u16; 18];
        caps[5] = 30;
        r.observe_event(&inv_capacities(caps));
        for i in 0..30u8 {
            r.observe_event(&inv_slot(5, i, 4112));
        }
        r.observe_event(&AgentEvent::InventoryReady);
        r.handle_command(AgentCommand::BankWhenFull {
            threshold: 30,
            mog_house_zoneline: 7777,
        });
        let out = r.tick();
        assert!(matches!(
            out.commands.as_slice(),
            [AgentCommand::RequestZoneChange { line_id: 7777 }]
        ));
    }

    #[test]
    fn bank_when_full_ignores_safe_and_storage() {
        // Filling LOC_MOGSAFE (1) or LOC_STORAGE (2) — bank containers,
        // not field bags — must NOT trigger banking. Banking is for
        // overflowing field bags, the safes are the destination.
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        let mut caps = [0u16; 18];
        caps[1] = 80;
        caps[2] = 80;
        r.observe_event(&inv_capacities(caps));
        for i in 0..40u8 {
            r.observe_event(&inv_slot(1, i, 4112));
            r.observe_event(&inv_slot(2, i, 4112));
        }
        r.observe_event(&AgentEvent::InventoryReady);
        r.handle_command(AgentCommand::BankWhenFull {
            threshold: 30,
            mog_house_zoneline: 12345,
        });
        let out = r.tick();
        assert!(
            out.commands.is_empty(),
            "safe/storage are bank dest, not field bag"
        );
    }

    /// Synthetic-knockback / forced-move contract: when the reactor sees
    /// `AgentEvent::ForcedMove`, the next tick must drive a Move toward
    /// `target` (suppressing whatever goal was in flight). After the
    /// window expires, normal goal-driven output resumes.
    ///
    /// LSB doesn't ship combat knockback over the wire (it's a BATTLE2
    /// animation hint; see ffxi-proto::decode::ForcedMove docs); this
    /// test exercises the override path with a synthetic packet that
    /// could just as easily come from WPOS / WPOS2 (cutscene end,
    /// homepoint, zone-line re-anchor).
    #[test]
    fn forced_move_event_installs_override_and_lerps() {
        use crate::state::Position;
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(
            1,
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            100,
            EntityKind::Pc,
            1,
        ));
        // Drive a goal too — the override must override it.
        r.handle_command(AgentCommand::Follow {
            target_id: 1,
            distance: 5.0,
        });

        // Synthetic forced-move 0.5 yalms east of origin. With step_test_cfg's
        // max_step=1.0 the reactor reaches it in a single tick.
        let target = Vec3 {
            x: 0.5,
            y: 0.0,
            z: 0.0,
        };
        r.observe_event(&AgentEvent::ForcedMove {
            mode: 0x00,
            target: Position {
                pos: target,
                heading: 64,
                speed: 0,
                speed_base: 0,
            },
            duration_ms: 5_000,
        });
        assert!(
            r.current_override().is_some(),
            "ForcedMove event installs an override"
        );

        let out = r.tick();
        assert_eq!(out.commands.len(), 1, "exactly one Move emitted per tick");
        match &out.commands[0] {
            AgentCommand::Move {
                x,
                y,
                z,
                heading,
            } => {
                assert!((x - 0.5).abs() < 1e-3, "lerp reached target.x");
                assert!(y.abs() < 1e-3);
                assert!(z.abs() < 1e-3);
                assert_eq!(*heading, 64, "heading from override carries through");
            }
            other => panic!("expected Move, got {other:?}"),
        }
    }

    /// While the override is active, an explicit `Move` from the agent
    /// is dropped so it doesn't race the server's re-anchor. Same
    /// rationale as suppressing goal-driven output.
    #[test]
    fn forced_move_suppresses_explicit_move_command() {
        let mut r = Reactor::new(step_test_cfg());
        r.set_override_for_test(
            Vec3 {
                x: 10.0,
                y: 0.0,
                z: 0.0,
            },
            0,
            Duration::from_secs(5),
        );
        let routing = r.handle_command(AgentCommand::Move {
            x: 99.0,
            y: 99.0,
            z: 99.0,
            heading: 192,
        });
        assert!(
            routing.forward.is_none(),
            "explicit Move dropped while override active"
        );
        assert!(routing.derived_events.is_empty());
    }

    /// After the override window expires, normal goal-driven output
    /// resumes — and the override field clears lazily.
    #[test]
    fn forced_move_expires_and_resumes_normal_flow() {
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(
            1,
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            100,
            EntityKind::Pc,
            1,
        ));
        // Already-expired override (ttl in the past via 0-length window
        // then a sleep would race; instead set a tiny ttl and assert
        // via a second tick after expiry).
        r.set_override_for_test(
            Vec3 {
                x: 100.0,
                y: 0.0,
                z: 0.0,
            },
            0,
            Duration::from_millis(1),
        );
        std::thread::sleep(Duration::from_millis(5));
        let _ = r.tick(); // triggers lazy expiry inside override_active
        assert!(
            r.current_override().is_none(),
            "override clears once expiry passes"
        );
        // Normal Idle tick: no commands.
        let out = r.tick();
        assert!(out.commands.is_empty());
        assert!(out.derived_events.is_empty());
    }

    #[test]
    fn hp_threshold_at_exact_value_is_above() {
        // pct < threshold (strict). pct == threshold → not low.
        let cfg = ReactorConfig {
            low_hp_threshold: 25,
            ..ReactorConfig::default()
        };
        let mut r = Reactor::new(cfg);
        r.observe_event(&connected(1));
        let d = r.observe_event(&upsert(1, Vec3::default(), 25, EntityKind::Pc, 1));
        assert!(d.is_empty(), "exactly threshold should not fire");
        let d = r.observe_event(&upsert(1, Vec3::default(), 24, EntityKind::Pc, 1));
        assert!(matches!(d.as_slice(), [AgentEvent::LowHp { pct: 24 }]));
    }

    // -- Server-set speed safety ------------------------------------------
    //
    // Anti-speedhack protection: outbound Move emissions must scale with
    // the server's authoritative `PosHead::speed`, and must STOP entirely
    // when speed==0 (Bind / Stun / Sleep / zoning). The reactor reads the
    // self-entity's speed/speed_base from the state mirror, which is
    // populated from inbound 0x00D PosHead by session.rs.

    #[test]
    fn pathing_suppresses_move_when_server_speed_is_zero() {
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        // Self: bound — server says speed=0 but speed_base=40 (normal).
        r.observe_event(&upsert_with_speed(
            1,
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            100,
            EntityKind::Pc,
            1,
            0,
            0,
            40,
        ));
        r.handle_command(AgentCommand::PathTo {
            x: 10.0,
            y: 0.0,
            z: 0.0,
        });
        let out = r.tick();
        assert!(
            out.commands.is_empty(),
            "speed=0 must suppress Move emission, got {:?}",
            out.commands
        );
        // Goal should NOT have flipped to Idle — we resume once speed > 0.
        assert!(matches!(r.goal, Goal::Pathing { .. }));
    }

    #[test]
    fn pathing_scales_step_by_server_speed_ratio() {
        let mut r = Reactor::new(step_test_cfg()); // max_step_per_tick = 1.0
        r.observe_event(&connected(1));
        // Slowed: speed = half of base → step should be 0.5 yalm.
        r.observe_event(&upsert_with_speed(
            1,
            Vec3::default(),
            100,
            EntityKind::Pc,
            1,
            0,
            20,
            40,
        ));
        r.handle_command(AgentCommand::PathTo {
            x: 10.0,
            y: 0.0,
            z: 0.0,
        });
        let out = r.tick();
        match out.commands.as_slice() {
            [AgentCommand::Move { x, .. }] => {
                assert!(
                    (x - 0.5).abs() < 1e-4,
                    "expected step x=0.5 (half of base 1.0), got {x}"
                );
            }
            other => panic!("expected single scaled Move, got {other:?}"),
        }
    }

    #[test]
    fn pathing_step_caps_at_2x_base_against_weird_server_values() {
        // If the decoder ever feeds us speed >> speed_base (a bug or a
        // weird server custom), the cap keeps us from speedhacking by
        // accident. Cap is 2× per the doc-comment policy.
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        r.observe_event(&upsert_with_speed(
            1,
            Vec3::default(),
            100,
            EntityKind::Pc,
            1,
            0,
            200, // wildly elevated
            40,
        ));
        r.handle_command(AgentCommand::PathTo {
            x: 10.0,
            y: 0.0,
            z: 0.0,
        });
        let out = r.tick();
        match out.commands.as_slice() {
            [AgentCommand::Move { x, .. }] => {
                assert!(
                    *x <= 2.0 + 1e-4,
                    "step must be capped at 2× base (=2.0), got {x}"
                );
            }
            other => panic!("expected capped Move, got {other:?}"),
        }
    }

    #[test]
    fn follow_suppresses_step_when_server_speed_is_zero() {
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        // Self: bound.
        r.observe_event(&upsert_with_speed(
            1,
            Vec3::default(),
            100,
            EntityKind::Pc,
            1,
            0,
            0,
            40,
        ));
        // Follow target somewhere far away.
        r.observe_event(&upsert(
            2,
            Vec3 {
                x: 20.0,
                y: 0.0,
                z: 0.0,
            },
            100,
            EntityKind::Pc,
            2,
        ));
        r.handle_command(AgentCommand::Follow {
            target_id: 2,
            distance: 3.0,
        });
        let out = r.tick();
        // Reactor's `face_entity` (heading-only) is still allowed during
        // following — we suppress the *step*, not the face. Check that
        // no Move with non-self-position came back.
        let cur = Vec3::default();
        for cmd in &out.commands {
            if let AgentCommand::Move { x, y, z, .. } = cmd {
                assert!(
                    (*x - cur.x).abs() < 1e-3
                        && (*y - cur.y).abs() < 1e-3
                        && (*z - cur.z).abs() < 1e-3,
                    "speed=0 follow must not step (only face); got Move to ({x},{y},{z})"
                );
            }
        }
    }
}
