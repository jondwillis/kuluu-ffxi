use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use ffxi_nav_recast::RecastNavMesh;
use kuluu_nav::{glam, GridNav, NavMesh};

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

impl LoadedNav {
    fn slide_along(&self, from: glam::Vec3, to: glam::Vec3) -> Option<glam::Vec3> {
        match self {
            LoadedNav::Recast(n) => n.slide_along(from, to),
            LoadedNav::Grid(_) => None,
        }
    }
}
use tokio::sync::{broadcast, mpsc};

use crate::fishing::{FishingMachine, FishingOut};
use crate::state::{
    ground_correction_matches, model_radius, ActionKind, AgentCommand, AgentEvent, ChatChannel,
    ChatLine, EntityKind, ReactorGoalSnapshot, SessionState, Vec3, CONTACT_GAP,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReactorProfile {
    #[default]
    Player,
    Agent,
}

impl ReactorProfile {
    fn automates_player_input(self) -> bool {
        matches!(self, Self::Agent)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ReactorConfig {
    pub profile: ReactorProfile,

    pub tick: Duration,

    pub low_hp_threshold: u8,

    pub max_step_per_tick: f32,
}

impl Default for ReactorConfig {
    fn default() -> Self {
        Self {
            profile: ReactorProfile::Player,
            tick: Duration::from_millis(33),
            low_hp_threshold: 25,

            max_step_per_tick: 0.165,
        }
    }
}

impl ReactorConfig {
    pub fn player() -> Self {
        Self::default()
    }

    pub fn agent() -> Self {
        Self {
            profile: ReactorProfile::Agent,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Goal {
    #[default]
    Idle,
    Following {
        target_id: u32,
        distance: f32,
    },

    Engaged {
        target_id: u32,
        attack_issued: bool,
    },

    Pathing {
        waypoints: Vec<Vec3>,
        idx: usize,
        clamp: bool,
    },

    Banking {
        threshold: u8,
        mog_house_zoneline: u32,
    },
}

fn debug_pathto_line(text: String) -> ChatLine {
    ChatLine {
        spans: Vec::new(),
        channel: ChatChannel::Debug,
        sender: "client".into(),
        text,
        server_ts: 0,
    }
}

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
        Goal::Pathing { waypoints, idx, .. } => {
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

const FIELD_BAG_CONTAINERS: [u8; 4] = [0, 5, 6, 7];

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

#[derive(Debug, Default)]
pub struct TickOutput {
    pub commands: Vec<AgentCommand>,
    pub derived_events: Vec<AgentEvent>,
}

pub struct Reactor {
    cfg: ReactorConfig,
    state: SessionState,
    goal: Goal,

    self_low_hp_latched: bool,
    party_low_hp_latched: HashMap<u32, bool>,

    nav_cache: Option<(u16, LoadedNav)>,

    dat_root: Option<std::sync::Arc<ffxi_dat::DatRoot>>,

    mh_rect_cache: Option<(u16, Vec<ffxi_dat::zone_interaction::ZoneInteraction>)>,

    zoneline_trigger_latched: Option<u32>,

    needs_zone_seed: bool,

    reactor_override: Option<ReactorOverride>,

    target_locked: bool,

    fishing: FishingMachine,
    fishing_phase_pub: Option<u8>,
    fishing_pending: Vec<AgentCommand>,
}

#[derive(Debug, Clone, Copy)]
pub struct ReactorOverride {
    pub target: Vec3,

    pub heading: u8,

    pub expiry: Instant,
}

impl Reactor {
    pub fn new(cfg: ReactorConfig) -> Self {
        let automates_player_input = cfg.profile.automates_player_input();
        Self {
            cfg,
            state: SessionState::default(),
            goal: Goal::Idle,
            self_low_hp_latched: false,
            party_low_hp_latched: HashMap::new(),
            nav_cache: None,
            dat_root: None,
            mh_rect_cache: None,
            zoneline_trigger_latched: None,
            needs_zone_seed: false,
            reactor_override: None,
            target_locked: automates_player_input,
            fishing: FishingMachine::new(automates_player_input),
            fishing_phase_pub: None,
            fishing_pending: Vec::new(),
        }
    }

    pub fn set_dat_root(&mut self, root: Option<std::sync::Arc<ffxi_dat::DatRoot>>) {
        self.dat_root = root;
    }

    fn in_mog_house(&self) -> bool {
        self.state.self_in_mog_house()
    }

    pub fn current_override(&self) -> Option<ReactorOverride> {
        self.reactor_override
    }

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

    #[cfg(test)]
    pub fn set_nav_for_test(&mut self, zone_id: u16, nav: GridNav) {
        self.nav_cache = Some((zone_id, LoadedNav::Grid(nav)));
    }

    fn ensure_nav_loaded(&mut self) -> Option<&LoadedNav> {
        // MH interior is origin-space; the shared-zone-id town navmesh would
        // path through the wrong geometry.
        if self.in_mog_house() {
            return None;
        }
        let zone_id = self.state.zone_id?;
        let cached = matches!(&self.nav_cache, Some((z, _)) if *z == zone_id);
        if !cached {
            self.nav_cache = default_load_navmesh(zone_id).map(|n| (zone_id, n));
        }
        self.nav_cache.as_ref().map(|(_, n)| n)
    }

    fn build_waypoints(&mut self, target: Vec3, force: bool) -> Vec<Vec3> {
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

                if waypoints
                    .first()
                    .is_some_and(|w| horizontal_distance(*w, cur) < self.cfg.max_step_per_tick)
                {
                    waypoints.remove(0);
                }

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
                force,
                "navmesh found but produced no path — {}",
                if force {
                    "force-straight-lining"
                } else {
                    "refusing"
                }
            );
        }

        if force {
            vec![target]
        } else {
            Vec::new()
        }
    }

    pub fn observe_event(&mut self, ev: &AgentEvent) -> Vec<AgentEvent> {
        let mut out = self.detect_aggro_edge(ev);

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

        // Feed the fishing machine its server-side inputs and publish any resulting phase
        // change / immediate progress. Commands (e.g. the hook check after a bite) queue
        // for the next tick.
        let fishing_outs = match ev {
            AgentEvent::FishingCast { hook_delay } => {
                self.fishing.on_cast(*hook_delay);
                Vec::new()
            }
            AgentEvent::FishHooked { params } => self.fishing.on_hooked(*params),
            AgentEvent::FishingServerPhase { phase } => {
                self.fishing.on_phase(*phase);
                Vec::new()
            }
            AgentEvent::FishingEnded => {
                self.fishing.abort();
                Vec::new()
            }
            _ => Vec::new(),
        };
        if matches!(
            ev,
            AgentEvent::FishingCast { .. }
                | AgentEvent::FishHooked { .. }
                | AgentEvent::FishingServerPhase { .. }
                | AgentEvent::FishingEnded
        ) {
            let (cmds, events) = self.translate_fishing(fishing_outs);
            self.fishing_pending.extend(cmds);
            out.extend(events);
        }

        if matches!(ev, AgentEvent::ZoneChanged { .. }) {
            self.needs_zone_seed = true;
        }

        // Emit (don't just set) the reset: a silent reset left the folded
        // current_goal stuck at Engaged across a death / home-point warp.
        let died = matches!(
            ev,
            AgentEvent::DeathTimerUpdated {
                seconds_until_homepoint: Some(_)
            }
        );
        if (died || matches!(ev, AgentEvent::ZoneChanged { .. }))
            && !matches!(self.goal, Goal::Idle)
        {
            self.goal = Goal::Idle;
            out.push(AgentEvent::ReactorGoalChanged {
                goal: snapshot_goal(&self.goal),
            });
        }
        out.extend(self.detect_threshold_events(ev));
        out
    }

    fn detect_aggro_edge(&self, ev: &AgentEvent) -> Vec<AgentEvent> {
        let Some(self_id) = self.state.char_id else {
            return Vec::new();
        };
        let AgentEvent::EntityUpserted { entity, .. } = ev else {
            return Vec::new();
        };
        if entity.id == self_id {
            return Vec::new();
        }

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
            AgentEvent::EntityUpserted { entity, .. } => {
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
            AgentCommand::SetTargetLock { locked } => {
                self.target_locked = locked;
                CommandRouting::default()
            }
            AgentCommand::PathTo { x, y, z, force } => {
                let target = Vec3 { x, y, z };
                let waypoints = self.build_waypoints(target, force);
                if waypoints.is_empty() {
                    return CommandRouting {
                        forward: None,
                        derived_events: vec![AgentEvent::ChatLine {
                            line: debug_pathto_line(format!(
                                "pathto: no walkable route to ({x:.0}, {y:.0}, {z:.0}) — use /pathtoforce or /warp"
                            )),
                        }],
                    };
                }
                let summary = debug_pathto_line(format!(
                    "pathto \u{2192} ({x:.0}, {y:.0}, {z:.0}): {} wp{}",
                    waypoints.len(),
                    if force { " [force]" } else { "" }
                ));
                self.goal = Goal::Pathing {
                    waypoints,
                    idx: 0,
                    clamp: !force,
                };
                CommandRouting {
                    forward: None,
                    derived_events: vec![
                        AgentEvent::ReactorGoalChanged {
                            goal: snapshot_goal(&self.goal),
                        },
                        AgentEvent::ChatLine { line: summary },
                    ],
                }
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
            AgentCommand::GroundCorrection {
                zone_id,
                self_id,
                x,
                y,
                z,
                ..
            } => {
                if self.state.zone_id != Some(zone_id) || self.state.char_id != Some(self_id) {
                    return CommandRouting::default();
                }
                if self.override_active() {
                    let ov = self.reactor_override.as_mut().unwrap();
                    if !ground_correction_matches(x, y, ov.target.x, ov.target.y) {
                        return CommandRouting::default();
                    }
                    ov.target.z = z;
                }
                CommandRouting::forward(cmd)
            }
            AgentCommand::Move { .. } => {
                if self.override_active() {
                    return CommandRouting::default();
                }

                if matches!(self.goal, Goal::Engaged { .. }) {
                    return CommandRouting::forward(cmd);
                }

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
            AgentCommand::Fish => {
                let outs = self.fishing.start();
                self.route_fishing(outs)
            }
            AgentCommand::FishingInput { input } => {
                let outs = self.fishing.input(input);
                self.route_fishing(outs)
            }
            other => CommandRouting::forward(other),
        }
    }

    pub fn tick(&mut self) -> TickOutput {
        let mut out = if let Some(out) = self.tick_override() {
            out
        } else {
            let mut out = self.tick_goal();
            if let Some(req) = self.check_zoneline_trigger() {
                out.commands.push(req);
            }
            out
        };
        self.tick_fishing(&mut out);
        out
    }

    /// Advance the fishing mini-game machine and fold its outputs into this tick.
    fn tick_fishing(&mut self, out: &mut TickOutput) {
        let dt = self.cfg.tick.as_secs_f32();
        let outs = self.fishing.tick(dt);
        let (cmds, events) = self.translate_fishing(outs);
        out.commands.append(&mut self.fishing_pending);
        out.commands.extend(cmds);
        out.derived_events.extend(events);
    }

    /// Convert the fishing machine's outputs into outgoing commands + published events,
    /// and emit a phase change whenever the machine's view phase moves.
    fn translate_fishing(&mut self, outs: Vec<FishingOut>) -> (Vec<AgentCommand>, Vec<AgentEvent>) {
        let mut cmds = Vec::new();
        let mut events = Vec::new();
        let self_id = self.state.char_id.unwrap_or(0);
        let self_idx = self
            .entity_target_info(self_id)
            .map(|(idx, _, _)| idx)
            .unwrap_or(0);
        for o in outs {
            match o {
                FishingOut::StartCast => cmds.push(AgentCommand::Action {
                    target_id: self_id,
                    target_index: self_idx,
                    kind: ActionKind::Fish,
                }),
                FishingOut::Request { mode, para, para2 } => {
                    cmds.push(AgentCommand::FishingRequest { mode, para, para2 })
                }
                FishingOut::Progress { fish_hp, arrow } => {
                    events.push(AgentEvent::FishingProgress { fish_hp, arrow })
                }
            }
        }
        let phase = self.fishing.phase();
        if phase != self.fishing_phase_pub {
            self.fishing_phase_pub = phase;
            events.push(AgentEvent::FishingPhaseChanged { phase });
        }
        (cmds, events)
    }

    /// Route a fishing machine output set through a single [`CommandRouting`] (forwarding
    /// the first command, queueing the rest for the next tick).
    fn route_fishing(&mut self, outs: Vec<FishingOut>) -> CommandRouting {
        let (mut cmds, derived_events) = self.translate_fishing(outs);
        let forward = (!cmds.is_empty()).then(|| cmds.remove(0));
        self.fishing_pending.extend(cmds);
        CommandRouting {
            forward,
            derived_events,
        }
    }

    fn tick_override(&mut self) -> Option<TickOutput> {
        if !self.override_active() {
            return None;
        }
        let ov = self.reactor_override?;
        let cur = self.self_pos();
        let dist = horizontal_distance(cur, ov.target);

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

    fn check_zoneline_trigger(&mut self) -> Option<AgentCommand> {
        let zone_id = self.state.zone_id?;
        // In the MH the zone id stays the city's: town zonelines would misfire
        // against origin-space coords; exit is menu-driven (zmrq) only.
        if self.in_mog_house() {
            self.zoneline_trigger_latched = None;
            return None;
        }
        let player = self.self_pos();
        let lines = kuluu_nav::zone_lines_for(zone_id);

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
        // Scraped MH rows carry the town-side to_scale (thin enough to step
        // over at run speed); prefer the DAT trigger OBB when available.
        let mh_rects: &[ffxi_dat::zone_interaction::ZoneInteraction] =
            if lines.iter().any(kuluu_nav::zonelines::is_mog_house_entry) {
                self.ensure_mh_rects_loaded(zone_id)
            } else {
                &[]
            };
        let inside = lines
            .iter()
            .find(|line| {
                if kuluu_nav::zonelines::is_mog_house_entry(line) {
                    if let Some(rect) = mh_rects.iter().find(|r| r.rect_id() == line.line_id) {
                        return is_inside_dat_obb(player, rect);
                    }
                }
                is_inside_trigger_box(player, line)
            })
            .map(|line| line.line_id);
        if self.needs_zone_seed {
            self.zoneline_trigger_latched = inside;
            self.needs_zone_seed = false;
            return None;
        }
        let was = self.zoneline_trigger_latched;
        self.zoneline_trigger_latched = inside;
        match (was, inside) {
            (None, Some(line_id)) => Some(AgentCommand::RequestZoneChange { line_id }),

            (Some(prev), Some(line_id)) if prev != line_id => {
                Some(AgentCommand::RequestZoneChange { line_id })
            }

            _ => None,
        }
    }

    /// Per-zone lazy cache of the DAT MH trigger rects, loaded the same way
    /// [`Self::ensure_nav_loaded`] loads nav data. Empty when no DAT root is
    /// configured (callers fall back to the LSB-scraped box).
    fn ensure_mh_rects_loaded(
        &mut self,
        zone_id: u16,
    ) -> &[ffxi_dat::zone_interaction::ZoneInteraction] {
        let cached = matches!(&self.mh_rect_cache, Some((z, _)) if *z == zone_id);
        if !cached {
            self.mh_rect_cache = Some((zone_id, load_mh_rects(self.dat_root.as_deref(), zone_id)));
        }
        self.mh_rect_cache
            .as_ref()
            .map(|(_, rects)| rects.as_slice())
            .unwrap_or(&[])
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
                let target_alive = self
                    .state
                    .entities
                    .iter()
                    .find(|e| e.id == target_id)
                    .is_some_and(|e| e.hp_pct != Some(0));
                if !target_alive {
                    self.goal = Goal::Idle;
                    return TickOutput {
                        commands: Vec::new(),
                        derived_events: vec![AgentEvent::ReactorGoalChanged {
                            goal: snapshot_goal(&self.goal),
                        }],
                    };
                }
                let mut commands = Vec::new();
                if !attack_issued {
                    if let Some((act_index, _, _)) = self.entity_target_info(target_id) {
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
                if self.target_locked {
                    if let Some(m) = self.face_entity(target_id) {
                        commands.push(m);
                    }
                }
                TickOutput {
                    commands,
                    derived_events: Vec::new(),
                }
            }
            Goal::Pathing {
                waypoints,
                idx,
                clamp,
            } => {
                if waypoints.get(idx).is_none() {
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
                    return TickOutput {
                        commands: Vec::new(),
                        derived_events: Vec::new(),
                    };
                }

                let start_pos = self.self_pos();
                let mut cur = start_pos;
                let mut budget = step;
                let mut idx_local = idx;

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

                let on_final_segment = path_done || idx_local + 1 >= waypoints.len();
                if clamp && !on_final_segment {
                    if let Some((_, nav)) = self.nav_cache.as_ref() {
                        let from = glam::Vec3::new(start_pos.x, start_pos.y, start_pos.z);
                        let to = glam::Vec3::new(cur.x, cur.y, cur.z);
                        if let Some(slid) = nav.slide_along(from, to) {
                            cur = Vec3 {
                                x: slid.x,
                                y: slid.y,
                                z: slid.z,
                            };
                        }
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
        self.state
            .self_position()
            .map(|p| p.pos)
            .unwrap_or_default()
    }

    fn entity_target_info(&self, target_id: u32) -> Option<(u16, Vec3, EntityKind)> {
        self.state
            .entities
            .iter()
            .find(|e| e.id == target_id)
            .map(|e| (e.act_index, e.pos, e.kind))
    }

    fn step_toward_entity(&self, target_id: u32, min_hold: f32) -> Option<AgentCommand> {
        let (_, target_pos, target_kind) = self.entity_target_info(target_id)?;
        let hold =
            min_hold.max(model_radius(EntityKind::Pc) + model_radius(target_kind) + CONTACT_GAP);
        let cur = self.self_pos();
        let dist = horizontal_distance(cur, target_pos);
        if dist <= hold {
            return None;
        }
        let step = self.effective_step_per_tick();
        if step <= 0.0 {
            return None;
        }
        let step_size = (dist - hold).min(step);
        let stepped = step_point(cur, target_pos, step_size);
        Some(mk_move(stepped, heading_toward(cur, target_pos)))
    }

    fn effective_step_per_tick(&self) -> f32 {
        let Some(pos) = self.state.self_position() else {
            return self.cfg.max_step_per_tick;
        };
        self.cfg.max_step_per_tick
            * crate::state::move_speed_ratio(pos.speed, self.state.self_mounted())
    }

    fn face_entity(&self, target_id: u32) -> Option<AgentCommand> {
        let (_, target_pos, _) = self.entity_target_info(target_id)?;
        let cur = self.self_pos();
        Some(mk_move(cur, heading_toward(cur, target_pos)))
    }
}

fn load_mh_rects(
    root: Option<&ffxi_dat::DatRoot>,
    zone_id: u16,
) -> Vec<ffxi_dat::zone_interaction::ZoneInteraction> {
    let Some(root) = root else {
        return Vec::new();
    };
    let Some(file_id) = ffxi_dat::zone_dat::zone_id_to_mzb_file_id(zone_id) else {
        return Vec::new();
    };
    let Ok(loc) = root.resolve(file_id) else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(loc.path_under(root)) else {
        return Vec::new();
    };
    match ffxi_dat::zone_interaction::from_dat(&bytes) {
        Ok(all) => {
            let rects: Vec<_> = all.into_iter().filter(|i| i.is_mog_house_line()).collect();
            for r in &rects {
                if r.orientation[0] != 0.0 || r.orientation[2] != 0.0 {
                    tracing::warn!(
                        zone_id,
                        rect_id = r.rect_id(),
                        orientation = ?r.orientation,
                        "MH rect has non-yaw Euler angles; is_inside_dat_obb tests the wrong volume"
                    );
                }
            }
            tracing::debug!(
                zone_id,
                count = rects.len(),
                "MH trigger rects loaded from DAT"
            );
            rects
        }
        Err(e) => {
            tracing::warn!(zone_id, error = %e, "RID parse failed; MH triggers fall back to the LSB scrape");
            Vec::new()
        }
    }
}

/// Faithful DAT trigger-box test in FFXI-native zone space. State `Vec3` axes are
/// (x, ground z, vertical y) — the GP_SERV_POS_HEAD wire order — while RID rects
/// are native (x, y, z) with y vertical and vertically centered. Applies yaw only
/// (`orientation[1]`): every observed zmr*/zms* rect has zero X/Z Euler, so this
/// is the X=Z=0 reduction of XIM's box-to-world ZYX matrix (column-major, local =
/// Rᵀ·(p − center); research/xim/src/jsMain/kotlin/xim/poc/CollisionShapes.kt:
/// 242-247 + xim/math/Matrix4f.kt:132-160) — a counterexample warns at rect-load.
/// Inside iff |local| ≤ size/2 per axis.
fn is_inside_dat_obb(player: Vec3, rect: &ffxi_dat::zone_interaction::ZoneInteraction) -> bool {
    let dx = player.x - rect.position[0];
    let dz = player.y - rect.position[2];
    let dv = player.z - rect.position[1];
    let (sin_y, cos_y) = rect.orientation[1].sin_cos();
    let local_x = dx * cos_y - dz * sin_y;
    let local_z = dx * sin_y + dz * cos_y;
    local_x.abs() <= rect.size[0] / 2.0
        && local_z.abs() <= rect.size[2] / 2.0
        && dv.abs() <= rect.size[1] / 2.0
}

fn is_inside_trigger_box(player: Vec3, line: &kuluu_nav::ZoneLine) -> bool {
    let dx = player.x - line.from_pos[0];
    let dy = player.y - line.from_pos[1];
    let cos_r = line.rotation.cos();
    let sin_r = line.rotation.sin();

    let local_x = dx * cos_r + dy * sin_r;
    let local_y = -dx * sin_r + dy * cos_r;
    local_x.abs() <= line.scale_x / 2.0 && local_y.abs() <= line.scale_z / 2.0
}

fn horizontal_distance(a: Vec3, b: Vec3) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    (dx * dx + dy * dy).sqrt()
}

fn step_point(from: Vec3, to: Vec3, step_size: f32) -> Vec3 {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let dist = (dx * dx + dy * dy).sqrt();
    if dist <= 1e-3 || step_size >= dist {
        return to;
    }
    let f = step_size / dist;
    // Height interpolates toward the destination rather than freezing at
    // `from.z`: holding it drags a stale height across every /follow, engage
    // approach and override step, which is the reactor half of the wire-z wedge
    // (kuluu-mo4q). The async runtime has no MZB collision (kuluu-render is
    // native-window only), so the destination's height is the best estimate
    // here; `recover_self_ground_system` is the authority whenever the viewer
    // has the zone's floor loaded.
    Vec3 {
        x: from.x + dx * f,
        y: from.y + dy * f,
        z: from.z + (to.z - from.z) * f,
    }
}

fn heading_toward(from: Vec3, to: Vec3) -> u8 {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    if dx.abs() < 1e-3 && dy.abs() < 1e-3 {
        return 0;
    }
    let radians = dy.atan2(dx);
    let raw = radians * -(128.0 / std::f32::consts::PI);

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

fn detour_navmesh_path(zone_id: u16) -> Option<PathBuf> {
    let base = if let Ok(custom) = std::env::var("FFXI_NAVMESH_DIR") {
        PathBuf::from(custom)
    } else {
        let cwd = std::env::current_dir().ok()?;
        find_navmesh_dir(&cwd)?
    };
    if let Some(name) = kuluu_nav::zone_name(zone_id) {
        let by_name = base.join(format!("{name}.nav"));
        if by_name.exists() {
            return Some(by_name);
        }
    }
    let by_id = base.join(format!("{zone_id}.nav"));
    if by_id.exists() {
        return Some(by_id);
    }

    Some(by_id)
}

fn find_navmesh_dir(start: &std::path::Path) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        for sib in ["vendor/server/navmeshes", "research/Phoenix/navmeshes"] {
            let candidate = ancestor.join(sib);
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    None
}

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
        base.join("kuluu-mcp")
            .join("heightmaps")
            .join(format!("{zone_id}.png")),
    )
}

fn default_load_navmesh(zone_id: u16) -> Option<LoadedNav> {
    if zone_id == 0 {
        return None;
    }

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

pub async fn run(
    cfg: crate::session::Config,
    mut external_cmd_rx: mpsc::Receiver<AgentCommand>,
    event_tx: broadcast::Sender<AgentEvent>,
    reactor_cfg: ReactorConfig,
) -> Result<()> {
    let (internal_cmd_tx, internal_cmd_rx) = mpsc::channel(64);
    let mut event_rx = event_tx.subscribe();
    let session_event_tx = event_tx.clone();
    let dat_root = cfg.dat_root.clone();
    let mut session_handle =
        tokio::spawn(
            async move { crate::session::run(cfg, internal_cmd_rx, session_event_tx).await },
        );

    let mut reactor = Reactor::new(reactor_cfg);
    reactor.set_dat_root(dat_root);
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
                Err(broadcast::error::RecvError::Lagged(_)) => {  }
                Err(broadcast::error::RecvError::Closed) => {
                    break (&mut session_handle).await
                        .map_err(|e| anyhow::anyhow!("session task: {e}"))
                        .and_then(|r| r);
                }
            },
            _ = tick.tick() => {

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
                    target: "kuluu_session::reactor",
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
mod tests;
