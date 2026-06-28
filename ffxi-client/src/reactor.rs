use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use ffxi_nav::{glam, GridNav, NavMesh};
use ffxi_nav_recast::RecastNavMesh;

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

use crate::state::{
    model_radius, ActionKind, AgentCommand, AgentEvent, ChatChannel, ChatLine, EntityKind,
    ReactorGoalSnapshot, SessionState, Vec3, CONTACT_GAP,
};

#[derive(Debug, Clone, Copy)]
pub struct ReactorConfig {
    pub tick: Duration,

    pub low_hp_threshold: u8,

    pub max_step_per_tick: f32,
}

impl Default for ReactorConfig {
    fn default() -> Self {
        Self {
            tick: Duration::from_millis(33),
            low_hp_threshold: 25,

            max_step_per_tick: 0.165,
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

    zoneline_trigger_latched: Option<u32>,

    needs_zone_seed: bool,

    reactor_override: Option<ReactorOverride>,
}

#[derive(Debug, Clone, Copy)]
pub struct ReactorOverride {
    pub target: Vec3,

    pub heading: u8,

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

        if matches!(ev, AgentEvent::ZoneChanged { .. }) {
            self.needs_zone_seed = true;
        }

        // Emit (don't just set) the reset: a silent reset left the folded
        // current_goal stuck at Engaged across a death / home-point warp.
        if matches!(
            ev,
            AgentEvent::ZoneChanged { .. } | AgentEvent::DeathTimerUpdated { .. }
        ) && !matches!(self.goal, Goal::Idle)
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
            other => CommandRouting::forward(other),
        }
    }

    pub fn tick(&mut self) -> TickOutput {
        if let Some(out) = self.tick_override() {
            return out;
        }

        let mut out = self.tick_goal();
        if let Some(req) = self.check_zoneline_trigger() {
            out.commands.push(req);
        }
        out
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
        let player = self.self_pos();
        let lines = ffxi_nav::zone_lines_for(zone_id);

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
            (None, Some(line_id)) => Some(AgentCommand::RequestZoneChange { line_id }),

            (Some(prev), Some(line_id)) if prev != line_id => {
                Some(AgentCommand::RequestZoneChange { line_id })
            }

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
                if let Some(m) = self.face_entity(target_id) {
                    commands.push(m);
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
        if pos.speed == 0 {
            return 0.0;
        }
        let base = pos.speed_base.max(1) as f32;
        let ratio = (pos.speed as f32 / base).min(2.0);
        self.cfg.max_step_per_tick * ratio
    }

    fn face_entity(&self, target_id: u32) -> Option<AgentCommand> {
        let (_, target_pos, _) = self.entity_target_info(target_id)?;
        let cur = self.self_pos();
        Some(mk_move(cur, heading_toward(cur, target_pos)))
    }
}

fn is_inside_trigger_box(player: Vec3, line: &ffxi_nav::ZoneLine) -> bool {
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
    Vec3 {
        x: from.x + dx * f,
        y: from.y + dy * f,
        z: from.z,
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

    Some(by_id)
}

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
                npc_state: None,
                status: 0,
            },
            pos_present: true,
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
                assert!(
                    (x - 1.0).abs() < 1e-3,
                    "step toward target capped at max_step: got {x}"
                );
            }
            other => panic!("expected Move, got {other:?}"),
        }

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
    fn follow_holds_at_pc_model_radius() {
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(1, Vec3::default(), 100, EntityKind::Pc, 1));
        r.observe_event(&upsert(
            2,
            Vec3 {
                x: 0.8,
                y: 0.0,
                z: 0.0,
            },
            100,
            EntityKind::Pc,
            2,
        ));
        r.handle_command(AgentCommand::Follow {
            target_id: 2,
            distance: 0.0,
        });
        assert_eq!(
            r.tick().commands.len(),
            1,
            "outside PC contact radius (0.8 > ~0.70): step"
        );

        r.observe_event(&upsert(
            2,
            Vec3 {
                x: 0.6,
                y: 0.0,
                z: 0.0,
            },
            100,
            EntityKind::Pc,
            2,
        ));
        assert!(
            r.tick().commands.is_empty(),
            "inside PC contact radius (0.6 < ~0.70): hold"
        );
    }

    #[test]
    fn follow_hold_scales_with_target_kind() {
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(1, Vec3::default(), 100, EntityKind::Pc, 1));
        r.observe_event(&upsert(
            2,
            Vec3 {
                x: 0.8,
                y: 0.0,
                z: 0.0,
            },
            100,
            EntityKind::Mob,
            2,
        ));
        r.handle_command(AgentCommand::Follow {
            target_id: 2,
            distance: 0.0,
        });
        assert!(
            r.tick().commands.is_empty(),
            "inside Mob contact radius (0.8 < ~0.90): hold"
        );

        r.observe_event(&upsert(
            2,
            Vec3 {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            },
            100,
            EntityKind::Mob,
            2,
        ));
        assert_eq!(
            r.tick().commands.len(),
            1,
            "outside Mob contact radius (1.0 > ~0.90): step"
        );
    }

    #[test]
    fn follow_distance_floor_still_honored() {
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        r.observe_event(&upsert(1, Vec3::default(), 100, EntityKind::Pc, 1));
        r.observe_event(&upsert(
            2,
            Vec3 {
                x: 4.0,
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
        assert!(
            r.tick().commands.is_empty(),
            "within explicit floor distance (4.0 < 5.0): hold"
        );
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

    fn emits_idle_goal(events: &[AgentEvent]) -> bool {
        events.iter().any(|e| {
            matches!(
                e,
                AgentEvent::ReactorGoalChanged {
                    goal: ReactorGoalSnapshot::Idle
                }
            )
        })
    }

    #[test]
    fn death_timer_disengages_and_emits_goal_change() {
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        r.handle_command(AgentCommand::Engage { target_id: 99 });
        assert!(matches!(r.current_goal(), Goal::Engaged { .. }));

        let derived = r.observe_event(&AgentEvent::DeathTimerUpdated {
            seconds_until_homepoint: 60,
        });
        assert!(
            matches!(r.current_goal(), Goal::Idle),
            "death must force disengage"
        );
        assert!(
            emits_idle_goal(&derived),
            "the reset must be emitted so the folded current_goal updates"
        );
    }

    #[test]
    fn zone_change_while_engaged_emits_idle_goal_change() {
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        r.handle_command(AgentCommand::Engage { target_id: 99 });
        assert!(matches!(r.current_goal(), Goal::Engaged { .. }));

        let derived = r.observe_event(&AgentEvent::ZoneChanged {
            from: Some(116),
            to: 240,
        });
        assert!(matches!(r.current_goal(), Goal::Idle));
        assert!(
            emits_idle_goal(&derived),
            "home-point warp must propagate disengage to current_goal"
        );
    }

    #[test]
    fn death_after_disengage_is_idempotent_no_event() {
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        assert!(matches!(r.current_goal(), Goal::Idle));

        let derived = r.observe_event(&AgentEvent::DeathTimerUpdated {
            seconds_until_homepoint: 60,
        });
        assert!(
            !emits_idle_goal(&derived),
            "already Idle: no spurious goal-change event while dead"
        );
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
                force: false,
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
            force: true,
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
            }, AgentEvent::ChatLine { line }] => {
                assert!((*x - 1.0).abs() < 1e-3);
                assert!((*y - 2.0).abs() < 1e-3);
                assert!((*z - 3.0).abs() < 1e-3);

                assert_eq!(*waypoints_remaining, 1);
                assert_eq!(line.channel, ChatChannel::Debug);
                assert!(line.text.contains("pathto"));
            }
            other => panic!("expected [ReactorGoalChanged(Pathing), ChatLine], got {other:?}"),
        }
    }

    #[test]
    fn pathto_without_route_refuses_and_reports() {
        let mut r = Reactor::new(ReactorConfig::default());
        let routing = r.handle_command(AgentCommand::PathTo {
            x: 5.0,
            y: 0.0,
            z: 0.0,
            force: false,
        });
        assert!(routing.forward.is_none());
        assert!(
            matches!(r.current_goal(), Goal::Idle),
            "refused pathto must leave the goal Idle, got {:?}",
            r.current_goal()
        );
        match routing.derived_events.as_slice() {
            [AgentEvent::ChatLine { line }] => {
                assert_eq!(line.channel, ChatChannel::Debug);
                assert!(
                    line.text.contains("no walkable route"),
                    "got {:?}",
                    line.text
                );
            }
            other => panic!("expected a single refusal ChatLine, got {other:?}"),
        }
    }

    #[test]
    fn pathto_force_beelines_without_route() {
        let mut r = Reactor::new(ReactorConfig::default());
        let routing = r.handle_command(AgentCommand::PathTo {
            x: 5.0,
            y: 6.0,
            z: 7.0,
            force: true,
        });
        assert!(routing.forward.is_none());
        match r.current_goal() {
            Goal::Pathing {
                waypoints,
                idx,
                clamp,
            } => {
                assert_eq!(*idx, 0);
                assert!(!*clamp, "forced pathing must not wall-slide");
                assert_eq!(waypoints.len(), 1);
                let wp = waypoints[0];
                assert!((wp.x - 5.0).abs() < 1e-3 && (wp.y - 6.0).abs() < 1e-3);
            }
            other => panic!("expected forced Pathing goal, got {other:?}"),
        }

        assert!(routing.derived_events.iter().any(|e| matches!(
            e,
            AgentEvent::ChatLine { line } if line.text.contains("[force]")
        )));
    }

    #[test]
    fn pathing_uses_navmesh_when_available() {
        let mut walkable = vec![true; 100];
        for row in 0..7u32 {
            walkable[(row * 10 + 5) as usize] = false;
        }
        let nav =
            ffxi_nav::GridNav::from_walkable(10, 10, walkable, ffxi_nav::glam::Vec2::ZERO, 1.0);

        let mut r = Reactor::new(ReactorConfig::default());

        r.state.zone_id = Some(123);
        r.set_nav_for_test(123, nav);

        let routing = r.handle_command(AgentCommand::PathTo {
            x: 9.0,
            y: 0.0,
            z: 0.0,
            force: false,
        });
        assert!(routing.forward.is_none());

        let goal = r.current_goal().clone();
        let Goal::Pathing { waypoints, idx, .. } = &goal else {
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

        let _ = r.handle_command(AgentCommand::Engage { target_id: 1 });

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

        let derived = r.observe_event(&upsert(1, Vec3::default(), 80, EntityKind::Pc, 1));
        assert!(derived.is_empty());

        let derived = r.observe_event(&upsert(1, Vec3::default(), 20, EntityKind::Pc, 1));
        assert!(matches!(
            derived.as_slice(),
            [AgentEvent::LowHp { pct: 20 }]
        ));

        let derived = r.observe_event(&upsert(1, Vec3::default(), 15, EntityKind::Pc, 1));
        assert!(derived.is_empty(), "latched: no repeat");

        let derived = r.observe_event(&upsert(1, Vec3::default(), 80, EntityKind::Pc, 1));
        assert!(derived.is_empty());

        let derived = r.observe_event(&upsert(1, Vec3::default(), 10, EntityKind::Pc, 1));
        assert!(matches!(
            derived.as_slice(),
            [AgentEvent::LowHp { pct: 10 }]
        ));
    }

    #[test]
    fn party_member_low_hp_latches_per_member() {
        let mut r = Reactor::new(ReactorConfig::default());

        assert!(r.observe_event(&party_update(10, 80)).is_empty());
        assert!(r.observe_event(&party_update(11, 90)).is_empty());

        let derived = r.observe_event(&party_update(10, 20));
        assert!(matches!(
            derived.as_slice(),
            [AgentEvent::PartyMemberLowHp { id: 10, pct: 20 }]
        ));

        assert!(r.observe_event(&party_update(11, 30)).is_empty());

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

        r.handle_command(AgentCommand::PathTo {
            x: 0.5,
            y: 0.0,
            z: 0.0,
            force: true,
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
            x: 0.5,
            y: 0.0,
            z: 0.0,
            force: true,
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
            force: true,
        });

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
            clamp: false,
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
            clamp: false,
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

        let Goal::Pathing { idx, .. } = r.current_goal() else {
            panic!("expected still Pathing");
        };
        assert_eq!(*idx, 3);

        assert!(matches!(
            out.derived_events.as_slice(),
            [AgentEvent::ReactorGoalChanged { .. }]
        ));
    }

    #[test]
    fn heading_toward_pins_cardinal_quarters() {
        let origin = Vec3::default();

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

        let p = step_point(from, to, 100.0);
        assert!((p.x - 1.0).abs() < 1e-3);
    }

    #[test]
    fn engaged_by_emits_on_mob_targeting_self() {
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));

        let derived = r.observe_event(&upsert_with_bt(
            99,
            Vec3::default(),
            100,
            EntityKind::Other,
            7,
            0,
        ));
        assert!(derived.is_empty(), "no aggro on initial sighting");

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

        r.observe_event(&upsert_with_bt(
            99,
            Vec3::default(),
            100,
            EntityKind::Other,
            7,
            0,
        ));

        let d1 = r.observe_event(&upsert_with_bt(
            99,
            Vec3::default(),
            100,
            EntityKind::Other,
            7,
            1,
        ));
        assert_eq!(d1.len(), 1);

        let d2 = r.observe_event(&upsert_with_bt(
            99,
            Vec3::default(),
            100,
            EntityKind::Other,
            7,
            1,
        ));
        assert!(d2.is_empty(), "no repeat while target unchanged");

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
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));
        r.state.zone_id = Some(230);

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
        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&connected(1));

        r.zoneline_trigger_latched = Some(812855930);

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

        let d = r.observe_event(&upsert_with_bt(
            50,
            Vec3::default(),
            100,
            EntityKind::Pc,
            2,
            1,
        ));
        assert!(d.is_empty(), "PCs aren't aggro");

        let d = r.observe_event(&upsert_with_bt(
            60,
            Vec3::default(),
            100,
            EntityKind::Npc,
            3,
            1,
        ));
        assert!(d.is_empty(), "NPCs aren't aggro");

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

        r.handle_command(AgentCommand::Follow {
            target_id: 1,
            distance: 5.0,
        });

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
            AgentCommand::Move { x, y, z, heading } => {
                assert!((x - 0.5).abs() < 1e-3, "lerp reached target.x");
                assert!(y.abs() < 1e-3);
                assert!(z.abs() < 1e-3);
                assert_eq!(*heading, 64, "heading from override carries through");
            }
            other => panic!("expected Move, got {other:?}"),
        }
    }

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
        let _ = r.tick();
        assert!(
            r.current_override().is_none(),
            "override clears once expiry passes"
        );

        let out = r.tick();
        assert!(out.commands.is_empty());
        assert!(out.derived_events.is_empty());
    }

    #[test]
    fn hp_threshold_at_exact_value_is_above() {
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

    #[test]
    fn pathing_suppresses_move_when_server_speed_is_zero() {
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));

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
            force: true,
        });
        let out = r.tick();
        assert!(
            out.commands.is_empty(),
            "speed=0 must suppress Move emission, got {:?}",
            out.commands
        );

        assert!(matches!(r.goal, Goal::Pathing { .. }));
    }

    #[test]
    fn pathing_scales_step_by_server_speed_ratio() {
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));

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
            force: true,
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
        let mut r = Reactor::new(step_test_cfg());
        r.observe_event(&connected(1));
        r.observe_event(&upsert_with_speed(
            1,
            Vec3::default(),
            100,
            EntityKind::Pc,
            1,
            0,
            200,
            40,
        ));
        r.handle_command(AgentCommand::PathTo {
            x: 10.0,
            y: 0.0,
            z: 0.0,
            force: true,
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
