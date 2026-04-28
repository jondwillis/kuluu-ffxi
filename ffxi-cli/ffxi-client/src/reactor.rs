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
use std::time::Duration;

use anyhow::Result;
use tokio::sync::{broadcast, mpsc};

use crate::state::{
    ActionKind, AgentCommand, AgentEvent, EntityKind, ReactorGoalSnapshot, SessionState, Vec3,
};

/// Reactor parameters. Defaults match the plan's "tactical loop" target.
#[derive(Debug, Clone, Copy)]
pub struct ReactorConfig {
    /// Tick interval. The plan specifies 200 ms; less frequent risks
    /// missing sub-second combat windows.
    pub tick: Duration,
    /// HP percent below which `LowHp` / `PartyMemberLowHp` fires.
    pub low_hp_threshold: u8,
    /// Per-tick movement step in yalms when chasing a follow/path target.
    /// Capped so the reactor never warps the player faster than legitimate
    /// movement speed (FFXI base is ~5 yalms/sec).
    pub max_step_per_tick: f32,
}

impl Default for ReactorConfig {
    fn default() -> Self {
        Self {
            tick: Duration::from_millis(200),
            low_hp_threshold: 25,
            max_step_per_tick: 5.0,
        }
    }
}

/// Active high-level intent. `Idle` produces no per-tick output. Each
/// non-idle variant is what the agent / LLM committed to last.
#[derive(Debug, Clone, PartialEq)]
pub enum Goal {
    Idle,
    Following { target_id: u32, distance: f32 },
    /// `attack_issued` flips true once the reactor has emitted the
    /// initial `Action::Attack` packet on transition; subsequent ticks
    /// only re-face the target.
    Engaged { target_id: u32, attack_issued: bool },
    /// Single-segment path. Multi-waypoint paths are a future extension —
    /// FFXI's collision is server-validated, so we step in straight lines
    /// and rely on the server to reject illegal moves.
    Pathing { target: Vec3 },
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
        Goal::Following { target_id, distance } => ReactorGoalSnapshot::Following {
            target_id: *target_id,
            distance: *distance,
        },
        Goal::Engaged { target_id, attack_issued } => ReactorGoalSnapshot::Engaged {
            target_id: *target_id,
            attack_issued: *attack_issued,
        },
        Goal::Pathing { target } => ReactorGoalSnapshot::Pathing {
            x: target.x,
            y: target.y,
            z: target.z,
        },
    }
}

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
        Self { forward: Some(cmd), derived_events: Vec::new() }
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
}

impl Reactor {
    pub fn new(cfg: ReactorConfig) -> Self {
        Self {
            cfg,
            state: SessionState::default(),
            goal: Goal::Idle,
            self_low_hp_latched: false,
            party_low_hp_latched: HashMap::new(),
        }
    }

    pub fn current_goal(&self) -> &Goal {
        &self.goal
    }

    /// Fold an event into the mirror, then derive any threshold-crossing
    /// events the reactor wants to broadcast.
    pub fn observe_event(&mut self, ev: &AgentEvent) -> Vec<AgentEvent> {
        // Aggro-edge detection runs *before* apply_event because we need
        // the old `bt_target_id` to detect a transition into us.
        let mut out = self.detect_aggro_edge(ev);
        self.state.apply_event(ev);
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
        // We can't reliably distinguish mobs from trusts/pets via our
        // current `EntityKind` (both come through 0x067/0x068 as Other).
        // We DO want to skip Pcs and Npcs — friendly entities targeting
        // us aren't aggro. False positives for trusts/pets are the
        // tolerable tradeoff for catching real mob aggro.
        if matches!(entity.kind, EntityKind::Pc | EntityKind::Npc) {
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
            AgentCommand::Follow { target_id, distance } => {
                self.goal = Goal::Following { target_id, distance };
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
                self.goal = Goal::Pathing {
                    target: Vec3 { x, y, z },
                };
                CommandRouting::absorbed_with_goal(snapshot_goal(&self.goal))
            }
            AgentCommand::Cancel => {
                self.goal = Goal::Idle;
                CommandRouting::absorbed_with_goal(snapshot_goal(&self.goal))
            }
            AgentCommand::BankWhenFull { .. } => {
                // Stage-9 wires the actual Goal::Banking variant + tick branch.
                // For now: forward as a no-op pass-through so the command
                // round-trips (the supervisor still persists it via
                // goal_store::is_persistable_goal once that's extended).
                CommandRouting::forward(cmd)
            }
            AgentCommand::Move { .. } => {
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
        match self.goal.clone() {
            Goal::Idle => TickOutput::default(),
            Goal::Following { target_id, distance } => TickOutput {
                commands: self
                    .step_toward_entity(target_id, distance)
                    .map(|m| vec![m])
                    .unwrap_or_default(),
                derived_events: Vec::new(),
            },
            Goal::Engaged { target_id, attack_issued } => {
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
            Goal::Pathing { target } => {
                let cur = self.self_pos();
                let dist = horizontal_distance(cur, target);
                if dist <= self.cfg.max_step_per_tick {
                    // Path complete — clear to Idle and notify renderers.
                    self.goal = Goal::Idle;
                    return TickOutput {
                        commands: vec![mk_move(target, heading_toward(cur, target))],
                        derived_events: vec![AgentEvent::ReactorGoalChanged {
                            goal: snapshot_goal(&self.goal),
                        }],
                    };
                }
                let stepped = step_point(cur, target, self.cfg.max_step_per_tick);
                TickOutput {
                    commands: vec![mk_move(stepped, heading_toward(cur, target))],
                    derived_events: Vec::new(),
                }
            }
        }
    }

    fn self_pos(&self) -> Vec3 {
        // Prefer the server's authoritative entity position when known
        // (came in via CHAR_PC for self during the zone-in flood); fall
        // back to whatever Move set self_pos to.
        if let Some(id) = self.state.char_id {
            if let Some(e) = self.state.entities.iter().find(|e| e.id == id) {
                return e.pos;
            }
        }
        self.state.self_pos.pos
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
        let step_size = (dist - hold_distance).min(self.cfg.max_step_per_tick);
        let stepped = step_point(cur, target_pos, step_size);
        Some(mk_move(stepped, heading_toward(cur, target_pos)))
    }

    fn face_entity(&self, target_id: u32) -> Option<AgentCommand> {
        let (_, target_pos) = self.entity_target_info(target_id)?;
        let cur = self.self_pos();
        Some(mk_move(cur, heading_toward(cur, target_pos)))
    }
}

fn horizontal_distance(a: Vec3, b: Vec3) -> f32 {
    let dx = b.x - a.x;
    let dz = b.z - a.z;
    (dx * dx + dz * dz).sqrt()
}

/// Step from `from` toward `to` by `step_size` yalms (in the x/z plane;
/// y is preserved). If `step_size` >= the actual distance, returns `to`.
fn step_point(from: Vec3, to: Vec3, step_size: f32) -> Vec3 {
    let dx = to.x - from.x;
    let dz = to.z - from.z;
    let dist = (dx * dx + dz * dz).sqrt();
    if dist <= 1e-3 || step_size >= dist {
        return to;
    }
    let f = step_size / dist;
    Vec3 {
        x: from.x + dx * f,
        y: from.y,
        z: from.z + dz * f,
    }
}

/// FFXI heading: u8 spans 0..256 ↔ 0..2π. Mapping pinned by tests:
/// north (+z) = 0, east (+x) = 64, south (-z) = 128, west (-x) = 192.
/// Live-server calibration is a Stage 7 task — this formula is the
/// *internally consistent* shape; if the server disagrees we'll rotate
/// by a constant offset here.
fn heading_toward(from: Vec3, to: Vec3) -> u8 {
    let dx = to.x - from.x;
    let dz = to.z - from.z;
    if dx.abs() < 1e-3 && dz.abs() < 1e-3 {
        return 0;
    }
    let theta = dx.atan2(dz);
    let normalized = if theta < 0.0 {
        theta + 2.0 * std::f32::consts::PI
    } else {
        theta
    };
    let heading = normalized * 256.0 / (2.0 * std::f32::consts::PI);
    // round() handles the case where `normalized` lands a tick below the
    // intended quarter mark (atan2's exact π returns 127.999… not 128).
    (heading.round() as i32).rem_euclid(256) as u8
}

fn mk_move(pos: Vec3, heading: u8) -> AgentCommand {
    AgentCommand::Move {
        x: pos.x,
        y: pos.y,
        z: pos.z,
        heading,
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
    let mut session_handle = tokio::spawn(async move {
        crate::session::run(cfg, internal_cmd_rx, session_event_tx).await
    });

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
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(1, Vec3::default(), 100, EntityKind::Pc, 1));
        r.observe_event(&upsert(2, Vec3 { x: 20.0, y: 0.0, z: 0.0 }, 100, EntityKind::Pc, 2));
        r.handle_command(AgentCommand::Follow { target_id: 2, distance: 5.0 });

        let cmds = r.tick().commands;
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            AgentCommand::Move { x, .. } => {
                // step capped at max_step_per_tick=5.0 → land at x=5.
                assert!((x - 5.0).abs() < 1e-3, "step toward target capped at max_step: got {x}");
            }
            other => panic!("expected Move, got {other:?}"),
        }

        // Self moves into the hold distance — reactor stops.
        r.observe_event(&upsert(1, Vec3 { x: 17.0, y: 0.0, z: 0.0 }, 100, EntityKind::Pc, 1));
        assert!(r.tick().commands.is_empty(), "within distance: hold");
    }

    #[test]
    fn follow_against_unknown_target_emits_nothing() {
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(1, Vec3::default(), 100, EntityKind::Pc, 1));
        r.handle_command(AgentCommand::Follow { target_id: 999, distance: 5.0 });
        assert!(r.tick().commands.is_empty(), "no entity → no movement");
    }

    #[test]
    fn engage_emits_attack_once_then_only_face() {
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(1, Vec3::default(), 100, EntityKind::Pc, 1));
        r.observe_event(&upsert(99, Vec3 { x: 5.0, y: 0.0, z: 0.0 }, 100, EntityKind::Mob, 7));
        r.handle_command(AgentCommand::Engage { target_id: 99 });

        let t1 = r.tick().commands;
        let attacks_t1 = t1
            .iter()
            .filter(|c| matches!(c, AgentCommand::Action { kind: ActionKind::Attack, .. }))
            .count();
        assert_eq!(attacks_t1, 1, "tick 1 emits exactly one Attack");

        let t2 = r.tick().commands;
        let attacks_t2 = t2
            .iter()
            .filter(|c| matches!(c, AgentCommand::Action { kind: ActionKind::Attack, .. }))
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
        r.handle_command(AgentCommand::Follow { target_id: 2, distance: 5.0 });
        assert!(matches!(r.current_goal(), Goal::Following { .. }));
        let m = AgentCommand::Move { x: 1.0, y: 2.0, z: 3.0, heading: 64 };
        let routing = r.handle_command(m);
        assert!(matches!(routing.forward, Some(AgentCommand::Move { .. })), "Move passes through");
        // Move-with-active-goal transitions to Idle; renderers learn via the
        // goal-changed event.
        assert!(matches!(
            routing.derived_events.as_slice(),
            [AgentEvent::ReactorGoalChanged { goal: ReactorGoalSnapshot::Idle }]
        ));
        assert!(matches!(r.current_goal(), Goal::Idle));
    }

    #[test]
    fn explicit_move_while_idle_emits_no_goal_event() {
        let mut r = Reactor::new(ReactorConfig::default());
        let m = AgentCommand::Move { x: 0.0, y: 0.0, z: 0.0, heading: 0 };
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
        let chat = AgentCommand::Chat { kind: 0, text: "hello".into() };
        let routing = r.handle_command(chat);
        assert!(matches!(routing.forward, Some(AgentCommand::Chat { .. })));
        assert!(routing.derived_events.is_empty());
    }

    #[test]
    fn snapshot_emits_scene_summary_and_forwards() {
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        let routing = r.handle_command(AgentCommand::Snapshot);
        assert!(matches!(routing.forward, Some(AgentCommand::Snapshot)),
                "Snapshot still forwards to session for Diagnostics");
        assert_eq!(routing.derived_events.len(), 1);
        assert!(matches!(&routing.derived_events[0], AgentEvent::SceneSummary { .. }));
    }

    #[test]
    fn goal_commands_are_absorbed_no_forward() {
        // Goal-mutating commands stay in the reactor (no session forward).
        // The contract that's *changed* from the original test:
        // they now emit ReactorGoalChanged so renderers see live intent.
        let mut r = Reactor::new(ReactorConfig::default());
        for cmd in [
            AgentCommand::Follow { target_id: 1, distance: 5.0 },
            AgentCommand::Engage { target_id: 1 },
            AgentCommand::PathTo { x: 1.0, y: 0.0, z: 0.0 },
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
                goal: ReactorGoalSnapshot::Following { target_id, distance },
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
                goal: ReactorGoalSnapshot::Pathing { x, y, z },
            }] => {
                assert!((*x - 1.0).abs() < 1e-3);
                assert!((*y - 2.0).abs() < 1e-3);
                assert!((*z - 3.0).abs() < 1e-3);
            }
            other => panic!("expected ReactorGoalChanged(Pathing), got {other:?}"),
        }
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
        assert!(matches!(derived.as_slice(), [AgentEvent::LowHp { pct: 20 }]));

        // Stay below — latched, no repeat.
        let derived = r.observe_event(&upsert(1, Vec3::default(), 15, EntityKind::Pc, 1));
        assert!(derived.is_empty(), "latched: no repeat");

        // Cross back up — reset latch.
        let derived = r.observe_event(&upsert(1, Vec3::default(), 80, EntityKind::Pc, 1));
        assert!(derived.is_empty());

        // Cross down again — re-emits.
        let derived = r.observe_event(&upsert(1, Vec3::default(), 10, EntityKind::Pc, 1));
        assert!(matches!(derived.as_slice(), [AgentEvent::LowHp { pct: 10 }]));
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
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(1, Vec3 { x: 0.0, y: 0.0, z: 0.0 }, 100, EntityKind::Pc, 1));
        // Target 3 yalms away — within max_step (5.0); reaches in one tick.
        r.handle_command(AgentCommand::PathTo { x: 3.0, y: 0.0, z: 0.0 });
        let out = r.tick();
        assert_eq!(out.commands.len(), 1);
        match &out.commands[0] {
            AgentCommand::Move { x, z, .. } => {
                assert!((x - 3.0).abs() < 1e-3);
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
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(1, Vec3 { x: 0.0, y: 0.0, z: 0.0 }, 100, EntityKind::Pc, 1));
        r.handle_command(AgentCommand::PathTo { x: 3.0, y: 0.0, z: 0.0 });
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
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(1, Vec3 { x: 0.0, y: 0.0, z: 0.0 }, 100, EntityKind::Pc, 1));
        r.handle_command(AgentCommand::PathTo { x: 12.0, y: 0.0, z: 0.0 });
        // Tick 1: step 5 yalms, still pathing.
        let out = r.tick();
        match &out.commands[0] {
            AgentCommand::Move { x, .. } => assert!((x - 5.0).abs() < 1e-3),
            other => panic!("got {other:?}"),
        }
        assert!(matches!(r.current_goal(), Goal::Pathing { .. }));
        assert!(
            out.derived_events.is_empty(),
            "mid-path tick should not emit goal-changed"
        );
    }

    #[test]
    fn heading_toward_pins_cardinal_quarters() {
        let origin = Vec3::default();
        // North (+z): atan2(0, +) = 0 → heading 0.
        assert_eq!(heading_toward(origin, Vec3 { x: 0.0, y: 0.0, z: 10.0 }), 0);
        // East (+x): atan2(+, 0) = π/2 → heading 64.
        assert_eq!(heading_toward(origin, Vec3 { x: 10.0, y: 0.0, z: 0.0 }), 64);
        // South (-z): atan2(0, -) = π → heading 128.
        assert_eq!(heading_toward(origin, Vec3 { x: 0.0, y: 0.0, z: -10.0 }), 128);
        // West (-x): atan2(-, 0) = -π/2 → 3π/2 normalized → heading 192.
        assert_eq!(heading_toward(origin, Vec3 { x: -10.0, y: 0.0, z: 0.0 }), 192);
    }

    #[test]
    fn step_point_caps_at_target() {
        let from = Vec3::default();
        let to = Vec3 { x: 1.0, y: 0.0, z: 0.0 };
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
            99, Vec3::default(), 100, EntityKind::Other, 7, 0,
        ));
        assert!(derived.is_empty(), "no aggro on initial sighting");

        // Mob now targets self → emit EngagedBy.
        let derived = r.observe_event(&upsert_with_bt(
            99, Vec3::default(), 100, EntityKind::Other, 7, 1,
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
        r.observe_event(&upsert_with_bt(99, Vec3::default(), 100, EntityKind::Other, 7, 0));
        // Aggro!
        let d1 = r.observe_event(&upsert_with_bt(99, Vec3::default(), 100, EntityKind::Other, 7, 1));
        assert_eq!(d1.len(), 1);
        // Same target across the next tick — not a new edge.
        let d2 = r.observe_event(&upsert_with_bt(99, Vec3::default(), 100, EntityKind::Other, 7, 1));
        assert!(d2.is_empty(), "no repeat while target unchanged");
        // Mob disengages and re-engages → emits again.
        r.observe_event(&upsert_with_bt(99, Vec3::default(), 100, EntityKind::Other, 7, 0));
        let d3 = r.observe_event(&upsert_with_bt(99, Vec3::default(), 100, EntityKind::Other, 7, 1));
        assert_eq!(d3.len(), 1, "re-engage after release fires again");
    }

    #[test]
    fn engaged_by_skips_friendly_entities_and_self() {
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        // PC targeting us — skipped.
        let d = r.observe_event(&upsert_with_bt(50, Vec3::default(), 100, EntityKind::Pc, 2, 1));
        assert!(d.is_empty(), "PCs aren't aggro");
        // NPC targeting us — skipped.
        let d = r.observe_event(&upsert_with_bt(60, Vec3::default(), 100, EntityKind::Npc, 3, 1));
        assert!(d.is_empty(), "NPCs aren't aggro");
        // Self entity (id == char_id) — skipped.
        let d = r.observe_event(&upsert_with_bt(1, Vec3::default(), 100, EntityKind::Pc, 1, 1));
        assert!(d.is_empty(), "self isn't aggroing self");
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
}
