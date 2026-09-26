use std::collections::{HashMap, VecDeque};
use std::fs;
use std::sync::{Arc, Mutex, OnceLock};

use bevy::prelude::*;
use ffxi_dat::anim::Mo2Animation;
use ffxi_dat::{walk, ChunkKind, DatRoot};

use crate::components::{IsSelf, WorldEntity};
use crate::snapshot::SceneState;
use kuluu_snapshot::EntityKind;

#[cfg(not(target_arch = "wasm32"))]
/// The race's battle-motion DAT (engaged idle, run, the per-weapon-type stance
/// files that follow it): the FFXiMain.dll battle-animation table
/// (`MainDll::base_battle_animation_index`), else the shipped fallback when the
/// dll is unreadable.
#[cfg(not(target_arch = "wasm32"))]
pub fn motion_dat_for_race(dll: Option<&ffxi_dat::main_dll::MainDll>, race: u8) -> Option<u32> {
    // Only the playable races have a battle-animation block; a child config
    // (look_resolver::equipment_table_row) reads past the table otherwise.
    if !crate::look_resolver::PC_LOOK_RACES.contains(&race) {
        return None;
    }
    dll.and_then(|dll| dll.base_battle_animation_index(race))
        .map(u32::from)
        .or_else(|| motion_dat_fallback(crate::dat_vos2::skeleton_file_id_fallback(race)?))
}

#[cfg(not(target_arch = "wasm32"))]
/// [`motion_dat_for_race`] keyed on the race's skeleton file id, which is what
/// the animation caches below and the legacy VOS2 path carry instead of a race.
/// The dll's race-config table inverts the id to its race; a non-PC id (an NPC
/// model DAT) matches no row and, as before, has no battle DAT here.
#[cfg(not(target_arch = "wasm32"))]
pub fn motion_dat_for_skel(skel_file_id: u32) -> Option<u32> {
    if let Some(dll) = crate::scheduler_runtime::main_dll_from_env() {
        let race = crate::look_resolver::PC_LOOK_RACES
            .into_iter()
            .find(|&race| dll.base_race_config_index(race).map(u32::from) == Some(skel_file_id))?;
        return motion_dat_for_race(Some(&dll), race);
    }
    motion_dat_fallback(skel_file_id)
}

/// The browser viewer renders from relayed snapshots and never resolves an
/// install root, so there is no FFXiMain.dll to invert a skeleton id through.
#[cfg(target_arch = "wasm32")]
pub fn motion_dat_for_skel(skel_file_id: u32) -> Option<u32> {
    motion_dat_fallback(skel_file_id)
}

/// Skeleton -> battle-motion DAT as measured on KNOWN_CLIENTS horizonxi-2023 and
/// retail-2026-09 (identical); the fallback for an install whose FFXiMain.dll
/// cannot be read, pinned against the dll by
/// `kuluu-render/tests/install_conformance.rs`.
pub fn motion_dat_fallback(skel_file_id: u32) -> Option<u32> {
    match skel_file_id {
        7072 => Some(9672),
        10248 => Some(12848),
        13424 => Some(16024),
        16600 => Some(19200),
        19776 => Some(22376),
        23176 => Some(25776),
        26352 => Some(28952),
        _ => None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
static BATTLE_IDLE_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> =
    OnceLock::new();

static RUN_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> = OnceLock::new();

static SIT_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> = OnceLock::new();
static HEAL_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> = OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
static COMBAT_RUN_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> = OnceLock::new();

static DIRECTIONAL_ANIMS: OnceLock<Mutex<HashMap<(u32, [u8; 3]), Option<Arc<Mo2Animation>>>>> =
    OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
const BATTLE_IDLE_PREFIX: &[u8; 3] = b"btl";

#[cfg(not(target_arch = "wasm32"))]
pub fn battle_idle_anim_for_skel(root: &DatRoot, skel_file_id: u32) -> Option<Arc<Mo2Animation>> {
    let motion_dat = motion_dat_for_skel(skel_file_id)?;
    let map = BATTLE_IDLE_ANIMS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().ok()?;
    if let Some(entry) = guard.get(&motion_dat) {
        return entry.clone();
    }
    let loaded = load_battle_idle(root, motion_dat).map(Arc::new);
    guard.insert(motion_dat, loaded.clone());
    loaded
}

#[cfg(not(target_arch = "wasm32"))]
fn load_battle_idle(root: &DatRoot, motion_dat_id: u32) -> Option<Mo2Animation> {
    load_anim_with_prefix(root, motion_dat_id, BATTLE_IDLE_PREFIX)
}

pub fn run_anim_for_skel(root: &DatRoot, skel_file_id: u32) -> Option<Arc<Mo2Animation>> {
    let map = RUN_ANIMS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().ok()?;
    if let Some(entry) = guard.get(&skel_file_id) {
        return entry.clone();
    }
    let loaded = load_anim_with_prefix(root, skel_file_id, b"run").map(Arc::new);
    guard.insert(skel_file_id, loaded.clone());
    loaded
}

#[cfg(not(target_arch = "wasm32"))]
pub fn combat_run_anim_for_skel(root: &DatRoot, skel_file_id: u32) -> Option<Arc<Mo2Animation>> {
    let motion_dat = motion_dat_for_skel(skel_file_id)?;
    let map = COMBAT_RUN_ANIMS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().ok()?;
    if let Some(entry) = guard.get(&motion_dat) {
        return entry.clone();
    }
    let loaded = load_anim_with_prefix(root, motion_dat, b"run").map(Arc::new);
    guard.insert(motion_dat, loaded.clone());
    loaded
}

pub fn sit_anim_for_skel(root: &DatRoot, skel_file_id: u32) -> Option<Arc<Mo2Animation>> {
    let map = SIT_ANIMS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().ok()?;
    if let Some(entry) = guard.get(&skel_file_id) {
        return entry.clone();
    }
    let loaded = load_anim_with_prefix(root, skel_file_id, b"sit").map(Arc::new);
    guard.insert(skel_file_id, loaded.clone());
    loaded
}

pub fn heal_anim_for_skel(root: &DatRoot, skel_file_id: u32) -> Option<Arc<Mo2Animation>> {
    let map = HEAL_ANIMS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().ok()?;
    if let Some(entry) = guard.get(&skel_file_id) {
        return entry.clone();
    }
    let loaded = load_anim_with_prefix(root, skel_file_id, b"hea").map(Arc::new);
    guard.insert(skel_file_id, loaded.clone());
    loaded
}

#[derive(Resource, Default, Debug, Clone, Copy, PartialEq)]
pub struct RestStance {
    pub kind: RestKind,
    pub exit: RestExit,
}

/// Retail charges you for standing up: cancelling a rest plays the stand-up
/// clip first, and the character only starts moving if the movement keys are
/// still held when it finishes. Movement stays suppressed for the whole phase
/// instead of sliding out from under the animation.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum RestExit {
    #[default]
    Idle,

    Pending {
        grace: f32,
    },

    Playing,
}

impl RestExit {
    /// A self actor with no rest clips — missing DATs, or the actor not spawned
    /// yet — never reaches `Playing`, so the bridge from the input cancel to the
    /// pose machine's Out phase has to expire on its own.
    pub const HANDOFF_SECS: f32 = 0.25;
}

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub enum RestKind {
    #[default]
    None,

    Sit,

    Heal,
}

impl RestStance {
    pub fn is_resting(&self) -> bool {
        !matches!(self.kind, RestKind::None)
    }

    pub fn begin_exit(&mut self) {
        self.kind = RestKind::None;
        self.exit = RestExit::Pending {
            grace: RestExit::HANDOFF_SECS,
        };
    }

    pub fn exit_blocks_movement(&mut self, dt: f32) -> bool {
        match &mut self.exit {
            RestExit::Idle => false,
            RestExit::Playing => true,
            RestExit::Pending { grace } => {
                *grace -= dt;
                if *grace <= 0.0 {
                    self.exit = RestExit::Idle;
                    false
                } else {
                    true
                }
            }
        }
    }

    pub fn observe_exit_clip(&mut self, playing: bool) {
        self.exit = match (self.exit, playing) {
            (_, true) => RestExit::Playing,
            (RestExit::Playing, false) => RestExit::Idle,
            (other, false) => other,
        };
    }
}

/// Retail's rest is server-owned: it starts and ends on the server's terms
/// (damage, status effects, zoning), and the answer comes back as the 0x037
/// animation byte. The local stance is an optimistic prediction, so it gets
/// this long to be confirmed — the 0x0E8 camp leaves on the client's 200 ms
/// datagram cadence and the server's reply rides the following update — before
/// the server byte wins.
pub const SELF_REST_ACK_SECS: f32 = 1.0;

pub fn reconcile_rest_kind(local: RestKind, server_status: u8) -> Option<RestKind> {
    let server_healing = server_status == ffxi_proto::decode::animation::HEALING;
    match (local, server_healing) {
        (RestKind::Heal, false) => Some(RestKind::None),
        // Sitting never appears here: /sit is a client-side pose that sends no
        // packet, so a 0 byte must not stand the player up.
        (k, true) if k != RestKind::Heal => Some(RestKind::Heal),
        _ => None,
    }
}

pub fn reconcile_self_rest_stance_system(
    time: Res<Time>,
    state: Res<SceneState>,
    mut rest: ResMut<RestStance>,
    mut prev_local: Local<RestKind>,
    mut ack_grace: Local<f32>,
) {
    if *prev_local != rest.kind {
        *prev_local = rest.kind;
        *ack_grace = SELF_REST_ACK_SECS;
        return;
    }
    if *ack_grace > 0.0 {
        *ack_grace -= time.delta_secs();
        return;
    }
    if !matches!(state.snapshot.stage, kuluu_snapshot::Stage::InZone) {
        return;
    }
    if let Some(next) = reconcile_rest_kind(rest.kind, state.snapshot.self_server_status) {
        rest.kind = next;
        *prev_local = next;
    }
}

#[derive(Resource, Default, Debug, Clone, Copy, Eq, PartialEq)]
pub struct WalkMode {
    pub walking: bool,
}

impl WalkMode {
    pub fn scale(self) -> f32 {
        if self.walking {
            kuluu_snapshot::speed::WALK_SPEED_SCALE
        } else {
            1.0
        }
    }
}

pub const WALK_RUN_BOUNDARY: f32 = 3.0;

#[inline]
pub fn infers_walk_gait(speed: f32) -> bool {
    speed > EntityMotion::MOVE_EXIT && speed < WALK_RUN_BOUNDARY
}

/// Explicit intent prevents reconciliation jitter from sustaining the self locomotion clip.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq)]
pub struct SelfMoveIntent {
    pub moving: bool,
    pub forward: f32,
    pub strafe: f32,
    pub scripted_speed: Option<f32>,
    /// The body heading the movement dispatch produced this tick, the one the
    /// visual yaw and the camera follow read. `None` on a tick that produced
    /// none (a snapshot-driven or muted tick), when the wire heading stands in.
    pub heading: Option<u8>,
}

impl SelfMoveIntent {
    pub fn walking(&self, manual_walk: bool) -> bool {
        self.scripted_speed
            .map(infers_walk_gait)
            .unwrap_or(manual_walk)
    }
}

pub fn directional_anim_for_skel(
    root: &DatRoot,
    skel_file_id: u32,
    prefix: &[u8; 3],
) -> Option<Arc<Mo2Animation>> {
    let map = DIRECTIONAL_ANIMS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().ok()?;
    let key = (skel_file_id, *prefix);
    if let Some(entry) = guard.get(&key) {
        return entry.clone();
    }
    let loaded = load_anim_with_prefix(root, skel_file_id, prefix).map(Arc::new);
    guard.insert(key, loaded.clone());
    loaded
}

pub fn load_anim_with_prefix(
    root: &DatRoot,
    file_id: u32,
    prefix: &[u8; 3],
) -> Option<Mo2Animation> {
    let loc = root.resolve(file_id).ok()?;
    let bytes = fs::read(loc.path_under(root)).ok()?;
    for chunk in walk(&bytes).filter_map(Result::ok) {
        if ChunkKind::from_u8(chunk.kind) != Some(ChunkKind::AnimMo2) {
            continue;
        }
        let name_prefix = &chunk.name[..3];
        if name_prefix.eq_ignore_ascii_case(prefix) {
            if let Ok(anim) = ffxi_dat::anim::parse_mo2(chunk.data, &chunk.name) {
                return Some(anim);
            }
        }
    }
    None
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ClipId {
    Idle,
    BattleIdle,

    Run,

    CombatRun,

    Backpedal,

    StrafeLeft,
    StrafeRight,

    TurnInPlace,

    Walk,
}

#[derive(Clone, Copy, Debug)]
pub struct AnimationBlend {
    pub from_clip: ClipId,
    pub to_clip: ClipId,

    pub t: f32,

    pub duration: f32,
}

#[derive(Resource, Default)]
pub struct AnimationBlends {
    pub by_id: HashMap<u32, AnimationBlend>,
}

impl AnimationBlends {
    pub const DEFAULT_DURATION: f32 = 0.15;

    pub fn update(&mut self, id: u32, current: ClipId, dt: f32) {
        match self.by_id.get_mut(&id) {
            None => {
                self.by_id.insert(
                    id,
                    AnimationBlend {
                        from_clip: current,
                        to_clip: current,
                        t: 1.0,
                        duration: Self::DEFAULT_DURATION,
                    },
                );
            }
            Some(blend) => {
                if blend.to_clip != current {
                    blend.from_clip = blend.to_clip;
                    blend.to_clip = current;
                    blend.t = 0.0;
                    blend.duration = Self::DEFAULT_DURATION;
                } else if blend.t < 1.0 {
                    blend.t = (blend.t + dt / blend.duration.max(1e-4)).min(1.0);
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MotionSample {
    pub last_pos: Vec3,

    pub speed: f32,

    pub forward_component: f32,

    pub strafe_component: f32,

    pub last_heading_rad: f32,

    pub heading_rate: f32,

    pub smooth_vx: f32,
    pub smooth_vz: f32,

    pub moving: bool,
}

#[derive(Resource, Default)]
pub struct EntityMotion {
    pub by_id: HashMap<u32, MotionSample>,
}

/// KULUU_MOTION_LOG=1 gated probe for the remote locomotion model.
///
/// Measures, per entity: server-update spacing (seconds between 0x0E position
/// updates, research/XiPackets world/server/0x000E), jump distance and which
/// snap band `advance_prediction` took,
/// remaining chase distance when a packet lands, idle frames between packets,
/// moving-toggle rate on the transform-delta fallback path (the chase model's
/// entities toggle without hysteresis: reached target or not), and
/// heading-vs-travel-direction mismatch events. Emits one tracing::debug! line
/// per event on target "motion" plus a rolling summary every few seconds.
/// Observation only - no constant changes.
#[derive(Resource)]
pub struct MotionProbe {
    enabled: bool,
    per_id: HashMap<u32, ProbeEntity>,
    /// Every server-update spacing seen, for the global median/p90 in summaries.
    all_intervals: VecDeque<f32>,
    last_summary_at: f32,
}

#[derive(Default)]
struct ProbeEntity {
    kind_name: &'static str,
    updates: u64,
    normals: u64,
    stretches: u64,
    pops: u64,
    toggles: u64,
    mismatch_events: u64,
    in_mismatch: bool,
}

fn entity_kind_name(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Mob => "Mob",
        EntityKind::Pc => "Pc",
        EntityKind::Pet => "Pet",
        EntityKind::Npc => "Npc",
        _ => "Other",
    }
}

impl MotionProbe {
    pub const SUMMARY_EVERY_SECS: f32 = 5.0;

    /// Travel direction this far from the heading counts as playing sideways.
    pub const MISMATCH_THRESHOLD_RAD: f32 = std::f32::consts::FRAC_PI_4;

    /// Below this dead-reckoned speed the travel direction is noise, not intent.
    const MIN_MEANINGFUL_SPEED_SQ: f32 = 0.01;

    /// OnExit(InGame) teardown, mirroring drain_entity_prediction / the entity_motion clear in
    /// despawn_ingame_entities: a long session's counters and interval history must not carry
    /// across the game boundary.
    pub fn drain(&mut self) {
        self.per_id.clear();
        self.all_intervals.clear();
    }

    /// Test-only: an enabled probe (init() reads KULUU_MOTION_LOG).
    #[cfg(test)]
    pub fn enabled_for_test() -> Self {
        let mut p = Self::init();
        p.enabled = true;
        p
    }

    pub fn init() -> Self {
        static ONCE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let enabled = crate::env_flags::env_flag(&ONCE, "KULUU_MOTION_LOG");
        Self {
            enabled,
            per_id: HashMap::new(),
            all_intervals: VecDeque::new(),
            last_summary_at: 0.0,
        }
    }

    fn entry(&mut self, id: u32, kind: EntityKind) -> &mut ProbeEntity {
        self.per_id.entry(id).or_insert_with(|| ProbeEntity {
            kind_name: entity_kind_name(kind),
            ..Default::default()
        })
    }

    /// One server update was consumed by `advance_prediction` this frame. The
    /// printed ratio is jump/step, the band's defining quantity; a zero step
    /// (a stationary speed byte) prints unbounded rather than NaN.
    pub fn record_update(&mut self, id: u32, kind: EntityKind, u: UpdateOutcome) {
        if !self.enabled {
            return;
        }
        self.all_intervals.push_back(u.dt_server);
        if self.all_intervals.len() > 65_536 {
            self.all_intervals.pop_front();
        }
        let e = self.entry(id, kind);
        e.updates += 1;
        match u.band {
            SnapBand::Normal => e.normals += 1,
            SnapBand::Stretch => e.stretches += 1,
            SnapBand::Pop => e.pops += 1,
        }
        let jump = u.jump_sq.sqrt();
        let ratio = if u.step_yalms > 0.0 {
            jump / u.step_yalms
        } else {
            f32::INFINITY
        };
        tracing::debug!(
            target: "motion",
            "MOTION_UPD id={id:#x} kind={} band={} dt_srv={:.3}s jump={:.2}y step={:.2} ratio={:.2} speed_pkt={} speed_base={} idle_fr={} rem={:.2} seg={:.3}s ring_max={:.3}s",
            e.kind_name,
            match u.band {
                SnapBand::Normal => "Normal",
                SnapBand::Stretch => "Stretch",
                SnapBand::Pop => "Pop",
            },
            u.dt_server,
            jump,
            u.step_yalms,
            ratio,
            u.speed,
            u.speed_base,
            u.idle_frames,
            u.rem_dist,
            u.segment_duration,
            u.ring_max
        );
    }

    /// The MOVE_ENTER/MOVE_EXIT hysteresis flipped for this entity.
    pub fn record_toggle(&mut self, id: u32, kind: EntityKind, now_moving: bool, speed: f32) {
        if !self.enabled {
            return;
        }
        let e = self.entry(id, kind);
        e.toggles += 1;
        tracing::debug!(
            target: "motion",
            "MOTION_TGL id={id:#x} kind={} moving={} speed={:.2}",
            e.kind_name, now_moving, speed
        );
    }

    /// Rising-edge detector: one event per sideways episode, not per frame.
    /// worldAngle basis (see heading_forward): forward = (cos h, sin h) in Bevy
    /// space, so a travel vector (vx, vz) corresponds to the angle atan2(vz, vx).
    pub fn record_heading_mismatch(
        &mut self,
        id: u32,
        kind: EntityKind,
        heading_rad: f32,
        vel: Vec3,
    ) {
        if !self.enabled || vel.length_squared() < Self::MIN_MEANINGFUL_SPEED_SQ {
            return;
        }
        let travel = vel.z.atan2(vel.x);
        let mut diff = (heading_rad - travel).rem_euclid(std::f32::consts::TAU);
        if diff > std::f32::consts::PI {
            diff -= std::f32::consts::TAU;
        }
        let sideways = diff.abs() >= Self::MISMATCH_THRESHOLD_RAD;
        let e = self.entry(id, kind);
        if sideways && !e.in_mismatch {
            e.mismatch_events += 1;
            tracing::debug!(
                target: "motion",
                "MOTION_MIS id={id:#x} heading={:.0}deg travel={:.0}deg diff={:.0}deg speed={:.2}",
                heading_rad.to_degrees(),
                travel.to_degrees(),
                diff.abs().to_degrees(),
                vel.length()
            );
        }
        e.in_mismatch = sideways;
    }

    /// Rolling summary; called once per frame from predict_entities_system.
    pub fn maybe_summary(&mut self, now_secs: f32) {
        if !self.enabled || now_secs - self.last_summary_at < Self::SUMMARY_EVERY_SECS {
            return;
        }
        self.last_summary_at = now_secs;
        let (mut upd, mut norm, mut stretch, mut pop, mut tgl, mut mis) =
            (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
        for e in self.per_id.values() {
            upd += e.updates;
            norm += e.normals;
            stretch += e.stretches;
            pop += e.pops;
            tgl += e.toggles;
            mis += e.mismatch_events;
        }
        let (median, p90) = quantiles(&self.all_intervals);
        tracing::debug!(
            target: "motion",
            "MOTION_SUM t={:.1}s ents={} upd={} normal={} stretch={} pop={} tgl={} mis={} med_dt_srv={} p90_dt_srv={}",
            now_secs,
            self.per_id.len(),
            upd,
            norm,
            stretch,
            pop,
            tgl,
            mis,
            fmt_opt(median),
            fmt_opt(p90)
        );
    }
}

fn quantiles(samples: &VecDeque<f32>) -> (Option<f32>, Option<f32>) {
    if samples.is_empty() {
        return (None, None);
    }
    let mut v: Vec<f32> = samples.iter().copied().collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pick = |q: f32| {
        let idx = (q * (v.len() - 1) as f32).round() as usize;
        Some(v[idx.min(v.len() - 1)])
    };
    (pick(0.5), pick(0.9))
}

fn fmt_opt(x: Option<f32>) -> String {
    match x {
        Some(v) => format!("{v:.3}s"),
        None => "-".to_string(),
    }
}

impl EntityMotion {
    pub fn is_moving(&self, id: u32) -> bool {
        self.by_id.get(&id).is_some_and(|s| s.moving)
    }

    pub fn sample(&self, id: u32) -> Option<MotionSample> {
        self.by_id.get(&id).copied()
    }

    pub fn apply_move_hysteresis(prev_moving: bool, speed: f32) -> bool {
        if speed >= Self::MOVE_ENTER {
            true
        } else if speed <= Self::MOVE_EXIT {
            false
        } else {
            prev_moving
        }
    }

    pub const MOVE_THRESHOLD: f32 = 0.5;

    pub const MOVE_ENTER: f32 = 0.8;

    pub const MOVE_EXIT: f32 = 0.35;

    pub const TURN_THRESHOLD_RAD_PER_SEC: f32 = 0.5;
}

pub fn track_entity_motion_system(
    time: Res<Time>,
    state: Res<SceneState>,
    prediction: Res<EntityPrediction>,
    mut motion: ResMut<EntityMotion>,
    mut probe: ResMut<MotionProbe>,
    q: Query<(&WorldEntity, &Transform)>,
    mut heading_by_id: Local<std::collections::HashMap<u32, u8>>,
) {
    let dt = time.delta_secs().max(1e-4);

    // Headings only change with a snapshot; rebuilding the map every frame was
    // pure per-frame churn in the crowd scene.
    if state.dirty {
        heading_by_id.clear();
        heading_by_id.extend(state.snapshot.entities.iter().map(|e| (e.id, e.heading)));
    }
    for (world, transform) in &q {
        let pos = transform.translation;

        // Entities the chase model owns get their motion sample from the chase state itself:
        // moving is "inside its arrival segment" (is_chasing), speed is the tween's constant
        // close-in rate (gap over remaining segment budget; 0 while holding on target), and the
        // direction components are where the target still is relative to the rendered heading.
        // No velocity smoothing or enter/exit hysteresis on this path (research/XiPackets
        // world/server/0x000E: walk toward the target, stop on arrival).
        if let Some(chase) = prediction.by_id.get(&world.id) {
            let to_target = Vec3::new(chase.server_pos.x - pos.x, 0.0, chase.server_pos.z - pos.z);
            let chasing = chase.is_chasing();
            let heading_rad = chase.rendered_heading_rad;
            // worldAngle basis (see forward_from_rad): forward = (cos h, sin h) in Bevy space,
            // and the entity's right is its forward rotated +90 deg of heading (the self walker
            // in input.rs strafes with heading.wrapping_add(64)).
            let fwd = forward_from_rad(heading_rad);
            let right = Vec3::new(-fwd.z, 0.0, fwd.x);
            let prev = motion
                .by_id
                .get(&world.id)
                .copied()
                .unwrap_or(MotionSample {
                    last_pos: pos,
                    last_heading_rad: heading_rad,
                    ..Default::default()
                });
            let mut dh = heading_rad - prev.last_heading_rad;
            if dh > std::f32::consts::PI {
                dh -= std::f32::consts::TAU;
            } else if dh < -std::f32::consts::PI {
                dh += std::f32::consts::TAU;
            }
            let heading_rate = dh / dt;
            motion.by_id.insert(
                world.id,
                MotionSample {
                    last_pos: pos,
                    speed: if chasing {
                        let remaining = (chase.segment_duration - chase.segment_elapsed).max(1e-4);
                        to_target.length() / remaining
                    } else {
                        0.0
                    },
                    forward_component: to_target.dot(fwd),
                    strafe_component: to_target.dot(right),
                    last_heading_rad: heading_rad,
                    heading_rate,
                    smooth_vx: 0.0,
                    smooth_vz: 0.0,
                    moving: chasing,
                },
            );
            continue;
        }

        let heading_u8 = heading_by_id.get(&world.id).copied().unwrap_or(0);
        let heading_rad = heading_to_rad(heading_u8);

        let fwd = heading_forward(heading_u8);
        let (fwd_x, fwd_z) = (fwd.x, fwd.z);

        let right_x = -fwd_z;
        let right_z = fwd_x;

        let prev = motion
            .by_id
            .get(&world.id)
            .copied()
            .unwrap_or(MotionSample {
                last_pos: pos,
                last_heading_rad: heading_rad,
                ..Default::default()
            });
        let dx = pos.x - prev.last_pos.x;
        let dz = pos.z - prev.last_pos.z;

        const VEL_TAU: f32 = 0.25;
        let alpha = 1.0 - (-dt / VEL_TAU).exp();
        let smooth_vx = prev.smooth_vx + alpha * (dx / dt - prev.smooth_vx);
        let smooth_vz = prev.smooth_vz + alpha * (dz / dt - prev.smooth_vz);
        let speed = (smooth_vx * smooth_vx + smooth_vz * smooth_vz).sqrt();
        let forward_component = smooth_vx * fwd_x + smooth_vz * fwd_z;
        let strafe_component = smooth_vx * right_x + smooth_vz * right_z;

        let mut dh = heading_rad - prev.last_heading_rad;
        if dh > std::f32::consts::PI {
            dh -= std::f32::consts::TAU;
        } else if dh < -std::f32::consts::PI {
            dh += std::f32::consts::TAU;
        }
        let heading_rate = dh / dt;

        let moving = EntityMotion::apply_move_hysteresis(prev.moving, speed);
        if probe.enabled && moving != prev.moving {
            probe.record_toggle(world.id, world.kind, moving, speed);
        }
        motion.by_id.insert(
            world.id,
            MotionSample {
                last_pos: pos,
                speed,
                forward_component,
                strafe_component,
                last_heading_rad: heading_rad,
                heading_rate,
                smooth_vx,
                smooth_vz,
                moving,
            },
        );
    }
}

/// Which snap band a POS update fell into (see `EntityPrediction::SNAP_*_RATIO`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapBand {
    /// jump <= 1.0 * step (+ the float-boundary epsilon): an ordinary tick; chase.
    Normal,

    /// 1.0*step < jump <= 2.0*step: plausible (path re-eval or a late tick); chase, no snap.
    Stretch,

    /// jump > 2.0 * step: pop rendered XZ onto the server. Distance-only: staleness is a timing
    /// event that resets the cadence ring in observe() instead of snapping; the position tweens.
    Pop,
}

/// What `advance_prediction` learned when it consumed a server update this
/// frame; read by the KULUU_MOTION_LOG probe in predict_entities_system.
#[derive(Clone, Copy, Debug)]
pub struct UpdateOutcome {
    /// Seconds since the previous server update for this entity (the LSB
    /// position-update cadence).
    pub dt_server: f32,

    /// Squared distance from the rendered position to the new server position.
    pub jump_sq: f32,

    /// The snap band this update fell into; Pop is the only band that snaps XZ onto the server.
    pub band: SnapBand,

    /// expected_step_yalms for this update's wire bytes: the LSB per-tick step distance the bands
    /// are measured against (the ratio jump/step is what the probe prints).
    pub step_yalms: f32,

    /// The arrival-segment budget in effect when this update was consumed: max of the 8-sample
    /// interval ring times INTERVAL_HEADROOM. The tween reaches the target exactly when this
    /// budget lapses.
    pub segment_duration: f32,

    /// Widest inter-update interval currently in the ring (before headroom): what set the budget.
    /// A late tick shows up here immediately and ages out as newer intervals replace it.
    pub ring_max: f32,

    /// The 0x0E speed byte this update carried (research/XiPackets
    /// world/server/0x000E; retail decodes it as yalms/sec * 10).
    pub speed: u8,

    /// The 0x0E animationSpeed byte (LSB `animationSpeed`, never multiplied by the run factor;
    /// vendor/server/src/map/entities/battle_entity.cpp CBattleEntity::UpdateSpeed writes the
    /// movement speed only). Feeds the gait rule and the clip playback-rate scale.
    pub speed_base: u8,

    /// Rendered frames that elapsed between this update and the last one:
    /// how long the client chased on its own before the wire caught up.
    pub idle_frames: u32,

    /// Distance from the rendered position to the new server target after this
    /// frame's advance: what is left of the chase when the packet lands.
    pub rem_dist: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct PredictSample {
    pub rendered_pos: Vec3,

    pub server_pos: Vec3,

    pub target_heading: u8,

    /// Last 0x0E speed byte observed for this entity (research/XiPackets
    /// world/server/0x000E; retail decodes it as yalms/sec * 10).
    pub packet_speed: u8,

    /// Last 0x0E animationSpeed byte observed for this entity (LSB `animationSpeed`; the gait
    /// rule compares it against `packet_speed`).
    pub packet_speed_base: u8,

    /// Rendered frames since the last consumed POS update. Reset on an update, incremented every
    /// frame without one: how long the client chased unaided before the wire caught up.
    pub idle_frames: u32,

    pub rendered_heading_rad: f32,

    /// Seconds since the last real position change; observe() resets it when a moved update lands.
    /// At the next moved observe() this is the measured inter-update interval. advance_prediction
    /// adds dt to it every frame.
    pub sample_age: f32,

    /// 8-sample ring of recent measured inter-update intervals (clamped to [MIN_INTERVAL,
    /// MAX_INTERVAL]), newest last. Seeded with one AI tick so the first segment budget is sane
    /// before any interval has been measured.
    sample_intervals: [f32; EntityPrediction::JITTER_HISTORY_SAMPLES],

    /// Seconds elapsed in the current arrival segment since its update was consumed. The tween
    /// reaches `server_pos` exactly when this hits `segment_duration`; the segment is also what
    /// holds the moving flag up on target across a late packet.
    pub segment_elapsed: f32,

    /// Budget of the current arrival segment in seconds: max(ring) * INTERVAL_HEADROOM. One long
    /// gap widens it immediately; a burst of fast arrivals cannot shrink it until that sample ages
    /// out of the ring (max-of-ring is asymmetric on purpose).
    pub segment_duration: f32,

    /// Measured inter-update interval (seconds) of the last consumed update, for the band's
    /// staleness check and the probe. 0.0 before the first real move after seeding.
    pub last_interval: f32,

    /// Squared XZ distance between this update's confirmed position and the previous one: what
    /// LSB actually moved on this tick. observe() stores it before overwriting server_pos; the
    /// band compares it to one step, never to where we are rendering (render lag is not a
    /// teleport).
    wire_jump_sq: f32,

    /// True once a moved update has started an arrival segment. Gates the on-target hold in
    /// is_chasing so a fresh sample that has not yet moved reports idle instead of holding its
    /// seeded budget up for 0.5 s after spawn.
    pub segment_started: bool,

    pub sample_dirty: bool,

    pub initialized: bool,

    /// Set by the most recent `advance_prediction(record_outcome = true)` call that consumed a
    /// server update; cleared on frames without one. Written only while the MotionProbe is
    /// enabled (its sole production reader); tests pass record_outcome = true.
    pub last_update: Option<UpdateOutcome>,
}

impl PredictSample {
    fn seed(server_pos: Vec3, heading: u8, speed: u8, speed_base: u8) -> Self {
        PredictSample {
            rendered_pos: server_pos,
            server_pos,
            target_heading: heading,
            packet_speed: speed,
            packet_speed_base: speed_base,
            idle_frames: 0,
            rendered_heading_rad: heading_to_rad(heading),
            sample_age: 0.0,
            // One AI tick per slot (vendor/server/src/map/map_constants.h kLogicUpdateRate):
            // the first segment budget is
            // a tick plus headroom before any real interval has been measured.
            sample_intervals: [EntityPrediction::TICK_SECS;
                EntityPrediction::JITTER_HISTORY_SAMPLES],
            segment_elapsed: 0.0,
            segment_duration: EntityPrediction::TICK_SECS * EntityPrediction::INTERVAL_HEADROOM,
            last_interval: 0.0,
            wire_jump_sq: 0.0,
            segment_started: false,
            sample_dirty: false,
            initialized: true,
            last_update: None,
        }
    }

    /// Whether the entity still counts as moving. This is the chase model's moving flag (retail
    /// stops on arrival, research/XiPackets world/server/0x000E): no speed hysteresis, no velocity
    /// smoothing. XZ only: Y is fully server-resolved (LSB StepTo grounds it and we assign it
    /// directly on each update), so it never holds "moving" up after the XZ chase has arrived.
    ///
    /// Once the rendered position reaches the wire target, the flag stays up for the rest of this
    /// update's arrival segment: `segment_elapsed < segment_duration`. The tween lands exactly at
    /// budget end by construction, so a late packet finds the entity holding on target inside its
    /// segment instead of dropping to idle between updates and restarting the walk clip from frame 0.
    /// A sample that has never consumed a moved update (segment_started false) is not chasing:
    /// it sits at its spawn position with nothing to close in on.
    pub fn is_chasing(&self) -> bool {
        let dx = self.server_pos.x - self.rendered_pos.x;
        let dz = self.server_pos.z - self.rendered_pos.z;
        if dx * dx + dz * dz > EntityPrediction::ARRIVAL_EPS_SQ {
            return true;
        }
        self.segment_started && self.segment_elapsed < self.segment_duration
    }
}

#[derive(Resource, Default)]
pub struct EntityPrediction {
    pub by_id: HashMap<u32, PredictSample>,
}

impl EntityPrediction {
    /// AI logic tick rate in Hz. vendor/server/src/map/map_constants.h kLogicUpdateRate = 2.5f,
    /// with kLogicUpdateInterval = 1000 / kLogicUpdateRate ms (one 400 ms tick). A moving mob's
    /// path step runs on that tick and ends in updatemask |= UPDATE_POS exactly once per step
    /// (vendor/server/src/map/ai/helpers/pathfind/pathfind.cpp CPathFind::StepTo), so one POS update lands
    /// per tick: the cadence engine below seeds and bounds from this period.
    const LSB_LOGIC_UPDATE_RATE_HZ: f32 = 2.5;

    /// One AI tick in seconds (1 / kLogicUpdateRate). Seeds the interval ring and is the base of
    /// MIN_INTERVAL / MAX_INTERVAL below.
    const TICK_SECS: f32 = 1.0 / Self::LSB_LOGIC_UPDATE_RATE_HZ;

    // Engineering bounds around the cited tick, not LSB-derived: a measured inter-update interval
    // shorter than half a tick is clamped up (arrival bursts must not shrink the budget), and one
    // longer than two and a half ticks is treated as idle/resume rather than movement cadence.
    const MIN_INTERVAL_TICKS: f32 = 0.5;

    const MAX_INTERVAL_TICKS: f32 = 2.5;

    /// Shortest interval the ring records (half an AI tick).
    const MIN_INTERVAL: f32 = Self::MIN_INTERVAL_TICKS * Self::TICK_SECS;

    /// Longest interval the ring records (two and a half AI ticks). Gaps beyond
    /// STALE_INTERVAL (= 2x this) are stale: observe() resets the ring to its seed instead of
    /// recording them; the band itself is distance-only.
    const MAX_INTERVAL: f32 = Self::MAX_INTERVAL_TICKS * Self::TICK_SECS;

    // Engineering bound, not LSB-derived: intervals up to this many times MAX_INTERVAL are still
    // recorded (clamped) so a long idle does not instantly erase the measured cadence; beyond it
    // the entity was stationary and the ring keeps its last budget.
    const IDLE_INTERVAL_MULTIPLIER: f32 = 2.0;

    /// Staleness horizon, in seconds: the ring's own recording horizon (five AI ticks). Gaps up
    /// to here are late ticks: they widen the budget and still chase. Beyond it the measured
    /// cadence is untrustworthy and observe() resets the ring to its kLogicUpdateRate seed; the
    /// band itself is distance-only, so a stale sample with a small jump still tweens.
    const STALE_INTERVAL: f32 = Self::MAX_INTERVAL * Self::IDLE_INTERVAL_MULTIPLIER;

    // Engineering bound, not LSB-derived: a small buffer on top of the widest recent interval so
    // packet-arrival jitter does not end each run segment early.
    const INTERVAL_HEADROOM: f32 = 1.25;

    /// Ring depth for the inter-update cadence estimate. Short arrival bursts must not immediately
    /// erase a recent long gap from the budget, so the widest of these samples sets it.
    const JITTER_HISTORY_SAMPLES: usize = 8;

    /// Per-AI-tick step distance a moving mob advances, in yalms, from the wire speed byte.
    /// vendor/server/src/map/ai/helpers/pathfind/pathfind.cpp CPathFind::StepTo (pinned vendor/server):
    /// `float stepDistance = speed / (run ? 50 : 40);` on every logic tick. The run flag is
    /// `m_pathFlags & PATHFLAG_RUN` at the StepTo call site, which
    /// vendor/server/src/map/ai/controllers/mob_controller.cpp passes for chase/follow/return-home
    /// and leaves clear while roaming: roam = walk (/40), engaged = run (/50).
    /// vendor/server/src/map/entities/battle_entity.cpp CBattleEntity::UpdateSpeed multiplies only the movement speed by
    /// the run factor, never animationSpeed, so the client's gait signal is already `run = speed >
    /// speed_base` (wire bytes) and the divisor is chosen from that same comparison. These two
    /// divisors are LSB constants; they are the only literals this step model may use.
    pub const LSB_RUN_STEP_DIVISOR: f32 = 50.0;

    pub const LSB_WALK_STEP_DIVISOR: f32 = 40.0;

    /// The speed StepTo substitutes for a burrowing mob whose base speed is 0:
    /// `if (baseSpeed == 0 && ((roamFlags_ & xi::RoamFlag::Worm) != xi::RoamFlag::None) && owner_->isMobEntity())`
    /// returns 20 (vendor/server/src/map/ai/helpers/pathfind/pathfind.cpp CPathFind::StepToInternal;
    /// xi::RoamFlag::Worm from vendor/server/data/enums/roam_flag.yaml). The
    /// substitute is a local that is never written back to the entity, so the wire speed byte stays
    /// 0 while the worm still advances 20 / divisor per tick. expected_step_yalms mirrors it: a 0
    /// byte on an update that actually moved means the server used this value.
    const LSB_WORM_SUBSTITUTE_SPEED: f32 = 20.0;

    /// Snap-band multipliers, as a RATIO TO THE EXPECTED STEP (not distances). A jump within one
    /// step is an ordinary tick; up to two steps is plausible (a path re-eval or a late tick) and
    /// still chases without snapping; beyond that the sample is stale/teleported and we pop.
    pub const SNAP_NORMAL_RATIO: f32 = 1.0;

    pub const SNAP_STRETCH_RATIO: f32 = 2.0;

    /// Float-boundary tolerance on the Normal/Stretch edge, in ratio space (jump/step): a
    /// one-step tick measured through wire + sqrt noise lands at 1.0 +/- ~1e-7, and without this
    /// healthy updates split across the boundary. Engineering bound for float noise, not
    /// LSB-derived; it moves the edge by a fraction of a milliyalm at most.
    pub const SNAP_NORMAL_EPS_RATIO: f32 = 1e-3;

    pub const HEADING_TAU: f32 = 0.10;

    /// Squared XZ distance at which the chase counts as arrived on target. The clamp below lands
    /// exactly on `server_pos`, so this only has to clear float noise, not a real gap.
    pub const ARRIVAL_EPS_SQ: f32 = 1e-6;

    const SAMPLE_EPSILON_SQ: f32 = 1e-4;

    /// Ingest one POS update. A move in any component (Y included, which StepTo
    /// resolves along the slope) is ingested, but the band's jump is XZ-only: Y
    /// is assigned directly and a floor-height change must not inflate into a
    /// Pop. On a real position change, sample_age holds the measured interval
    /// since the last one; it is recorded in the ring (clamped to [MIN_INTERVAL,
    /// MAX_INTERVAL]) and the segment budget re-derived from the widest recent
    /// interval. Max-of-ring is asymmetric on purpose: one long gap widens the
    /// budget immediately, while a burst of fast arrivals cannot shrink it back
    /// until that sample ages out. A stale gap (idle/resume) instead resets the
    /// ring to its kLogicUpdateRate seed and the budget to one tick plus
    /// headroom: staleness is a timing event, not a distance event, so the
    /// position still tweens.
    pub fn observe(&mut self, id: u32, server_pos: Vec3, heading: u8, speed: u8, speed_base: u8) {
        match self.by_id.get_mut(&id) {
            None => {
                self.by_id.insert(
                    id,
                    PredictSample::seed(server_pos, heading, speed, speed_base),
                );
            }
            Some(e) => {
                let moved_sq = e.server_pos.distance_squared(server_pos);
                if moved_sq > Self::SAMPLE_EPSILON_SQ {
                    if e.sample_age > 0.0 && e.sample_age <= Self::STALE_INTERVAL {
                        let interval = e.sample_age.clamp(Self::MIN_INTERVAL, Self::MAX_INTERVAL);
                        e.sample_intervals.rotate_left(1);
                        e.sample_intervals[Self::JITTER_HISTORY_SAMPLES - 1] = interval;
                        e.segment_duration = e.sample_intervals.iter().copied().fold(0.0, f32::max)
                            * Self::INTERVAL_HEADROOM;
                    } else if e.sample_age > Self::STALE_INTERVAL {
                        e.sample_intervals = [Self::TICK_SECS; Self::JITTER_HISTORY_SAMPLES];
                        e.segment_duration = Self::TICK_SECS * Self::INTERVAL_HEADROOM;
                    }
                    // What LSB actually moved on this tick in XZ: the distance between consecutive
                    // confirmed positions. Render lag is not a teleport, so the band measures this
                    // pair, never where we are rendering; its threshold is one StepTo step, not a
                    // fixed distance.
                    let dxw = server_pos.x - e.server_pos.x;
                    let dzw = server_pos.z - e.server_pos.z;
                    e.last_interval = e.sample_age;
                    e.sample_age = 0.0;
                    e.segment_elapsed = 0.0;
                    e.segment_started = true;
                    e.wire_jump_sq = dxw * dxw + dzw * dzw;
                    e.server_pos = server_pos;
                    e.sample_dirty = true;
                }
                e.target_heading = heading;
                e.packet_speed = speed;
                e.packet_speed_base = speed_base;
            }
        }
    }
}

/// Heading byte to radians in the worldAngle basis.
///
/// vendor/server/src/common/utils.cpp worldAngle (pinned vendor/server): LSB position_t.z is a
/// horizontal axis, not vertical; see [`heading_forward`] for the full wire/Bevy mapping.
#[inline]
fn heading_to_rad(heading: u8) -> f32 {
    (heading as f32) * std::f32::consts::TAU / 256.0
}

/// World-space direction an entity with this heading faces, in Bevy space.
///
/// vendor/server/src/common/utils.cpp worldAngle (pinned vendor/server) writes the rotation byte
/// as `atan2f(B.z - A.z, B.x - A.x) * -(128 / PI), mod 256`. LSB position_t.z is a horizontal axis
/// (kuluu's WireVec3 names it `y`; WireVec3.z is vertical). worldAngle measures from wire +X,
/// negated: theta = heading * TAU / 256 gives a wire horizontal forward of (x = cos theta,
/// z_lsb = -sin theta). ffxi_to_bevy maps wire x -> Bevy x and the horizontal wire axis -> Bevy
/// -z, so the Bevy forward is (cos theta, 0, sin theta). Heading 0 faces +X, not north.
#[inline]
pub fn heading_forward(heading: u8) -> Vec3 {
    forward_from_rad(heading_to_rad(heading))
}

/// The [`heading_forward`] basis for an already-unpacked radian value; the chase path carries
/// only `rendered_heading_rad`.
#[inline]
pub fn forward_from_rad(rad: f32) -> Vec3 {
    Vec3::new(rad.cos(), 0.0, rad.sin())
}

/// Per-AI-tick step distance in yalms for the incoming wire speed bytes.
///
/// vendor/server/src/map/ai/helpers/pathfind/pathfind.cpp CPathFind::StepTo (pinned vendor/server) advances
/// a moving mob by `speed / (run ? 50 : 40)` each tick;
/// vendor/server/src/map/entities/battle_entity.cpp CBattleEntity::UpdateSpeed
/// multiplies only the movement speed by the run factor, never animationSpeed, so the run/walk
/// split is read straight off the wire as `speed > speed_base` (the same comparison the gait rule
/// uses). No mount or retail-yps factor: this is the server's own per-tick budget.
///
/// StepTo also substitutes a local `speed = 20` for xi::RoamFlag::Worm mobs whose base speed is 0, and
/// never writes it back, so the wire byte stays 0 while the worm still moves. This function is only
/// ever called on an update that actually moved (observe gates on SAMPLE_EPSILON_SQ), so a 0 speed
/// byte here means the server used that substitute.
pub fn expected_step_yalms(speed: u8, speed_base: u8) -> f32 {
    let divisor = if speed > speed_base {
        EntityPrediction::LSB_RUN_STEP_DIVISOR
    } else {
        EntityPrediction::LSB_WALK_STEP_DIVISOR
    };
    let effective_speed = if speed == 0 {
        EntityPrediction::LSB_WORM_SUBSTITUTE_SPEED
    } else {
        speed as f32
    };
    effective_speed / divisor
}

/// `record_outcome` gates the write of [`PredictSample::last_update`]: it exists for the
/// MotionProbe, so a disabled probe pays nothing per entity per frame. sample_age
/// accumulates from the last observe() reset; at the next moved observe() it is
/// the measured inter-update interval. A Pop snaps XZ onto the server position,
/// leaving the tween nothing left to cover for this update. Frames without a
/// consumed update count as idle: the client is chasing on its own.
fn advance_prediction(s: &mut PredictSample, dt: f32, record_outcome: bool) -> (Vec3, f32) {
    use std::f32::consts::{PI, TAU};

    s.sample_age += dt;

    let mut outcome: Option<UpdateOutcome> = None;
    if s.sample_dirty {
        s.sample_dirty = false;
        let dt_server = s.last_interval;
        let step = expected_step_yalms(s.packet_speed, s.packet_speed_base);

        // Step-relative snap band, decided BEFORE the tween runs. jump is what LSB actually moved
        // on this tick: the XZ distance between consecutive confirmed server positions (observe()
        // stores it), never where we are rendering -- render lag is not a teleport. step is what
        // StepTo advanced this tick (vendor/server/src/map/ai/helpers/pathfind/pathfind.cpp CPathFind::StepTo).
        // The bands are a ratio to that step, never a flat distance, so a fast mob's legitimate
        // per-tick move stays in Normal. XZ only: Y is assigned directly below and
        // must not inflate the jump with a floor-height change. Distance-only: a stale sample
        // (idle past STALE_INTERVAL) resets the cadence ring in observe() instead of snapping --
        // staleness is a timing event, and the position still tweens. The Normal edge carries a
        // float-boundary epsilon so one-step ticks do not split across it.
        let jump = s.wire_jump_sq.sqrt();
        let band = if jump > EntityPrediction::SNAP_STRETCH_RATIO * step {
            SnapBand::Pop
        } else if jump
            > (EntityPrediction::SNAP_NORMAL_RATIO + EntityPrediction::SNAP_NORMAL_EPS_RATIO) * step
        {
            SnapBand::Stretch
        } else {
            SnapBand::Normal
        };

        if band == SnapBand::Pop {
            s.rendered_pos.x = s.server_pos.x;
            s.rendered_pos.z = s.server_pos.z;
        }
        // Y is fully server-resolved (LSB StepTo walks it along the slope and snaps it onto
        // target.y on arrival), so assign it directly on every update. No smoothing, no ground
        // probe, no gravity.
        s.rendered_pos.y = s.server_pos.y;

        outcome = Some(UpdateOutcome {
            dt_server,
            jump_sq: s.wire_jump_sq,
            band,
            step_yalms: step,
            segment_duration: s.segment_duration,
            ring_max: s.sample_intervals.iter().copied().fold(0.0, f32::max),
            speed: s.packet_speed,
            speed_base: s.packet_speed_base,
            idle_frames: s.idle_frames,
            rem_dist: 0.0,
        });
        s.idle_frames = 0;
    }

    // Arrival-segment tween (upstream main): the rendered position closes on server_pos by a
    // fraction of the remaining gap per frame, dt / remaining, so it reaches the target exactly at
    // budget end by construction: no early arrival, no overshoot. Retail stops on arrival
    // (research/XiPackets world/server/0x000E) and there is no velocity state, so a stop can never
    // slide back.
    let remaining = s.segment_duration - s.segment_elapsed;
    if remaining > dt {
        s.rendered_pos += (s.server_pos - s.rendered_pos) * (dt / remaining);
    } else {
        s.rendered_pos = s.server_pos;
    }
    s.segment_elapsed = (s.segment_elapsed + dt).min(s.segment_duration);

    if let Some(u) = &mut outcome {
        let rx = s.server_pos.x - s.rendered_pos.x;
        let rz = s.server_pos.z - s.rendered_pos.z;
        u.rem_dist = (rx * rx + rz * rz).sqrt();
    }

    if outcome.is_none() {
        s.idle_frames = s.idle_frames.saturating_add(1);
    }

    let target = heading_to_rad(s.target_heading);
    let mut dh = target - s.rendered_heading_rad;
    dh = dh.rem_euclid(TAU);
    if dh > PI {
        dh -= TAU;
    }
    let alpha_h = 1.0 - (-dt / EntityPrediction::HEADING_TAU).exp();
    s.rendered_heading_rad += dh * alpha_h;

    s.last_update = if record_outcome { outcome } else { None };

    (s.rendered_pos, s.rendered_heading_rad)
}

/// Advances every remote entity's prediction one frame and writes the result to
/// its transform. A running cutscene owns a touched entity's transform until
/// CutsceneEnded releases it, so those are skipped. The probe's travel direction
/// is toward the target at the tween's close-in rate (gap over remaining segment
/// budget) while chasing, still otherwise.
pub fn predict_entities_system(
    time: Res<Time>,
    mut prediction: ResMut<EntityPrediction>,
    mut probe: ResMut<MotionProbe>,
    cutscene: Res<crate::scheduler_runtime::CutsceneActorState>,
    mut q: Query<(&WorldEntity, &mut Transform), Without<IsSelf>>,
) {
    let dt = time.delta_secs().max(1e-4);
    for (world, mut transform) in &mut q {
        if !matches!(
            world.kind,
            EntityKind::Mob | EntityKind::Pc | EntityKind::Pet | EntityKind::Npc
        ) {
            continue;
        }
        if cutscene.is_touched(world.id) {
            continue;
        }
        let Some(sample) = prediction.by_id.get_mut(&world.id) else {
            continue;
        };
        if !sample.initialized {
            continue;
        }
        let (pos, heading_rad) = advance_prediction(sample, dt, probe.enabled);
        if probe.enabled {
            if let Some(u) = sample.last_update {
                probe.record_update(world.id, world.kind, u);
            }
            let to_target = Vec3::new(
                sample.server_pos.x - pos.x,
                0.0,
                sample.server_pos.z - pos.z,
            );
            let vel = if sample.is_chasing() {
                let remaining = (sample.segment_duration - sample.segment_elapsed).max(1e-4);
                to_target * (1.0 / remaining)
            } else {
                Vec3::ZERO
            };
            probe.record_heading_mismatch(world.id, world.kind, heading_rad, vel);
        }
        transform.translation = pos;
        transform.rotation = Quat::from_rotation_y(-heading_rad);
    }
    probe.maybe_summary(time.elapsed_secs());
}

// Once-per-entity dedupe for the off-mesh debug line (same pattern as CLIP_WARN_SEEN in
// ffxi_actor_render.rs): an entity that stays off-mesh would otherwise log every frame.
#[cfg(not(target_arch = "wasm32"))]
static GROUND_OFF_MESH_SEEN: OnceLock<Mutex<std::collections::HashSet<u32>>> = OnceLock::new();

/// Per-frame remote grounding, run after the prediction tween.
///
/// LSB grounds mobs to the Detour navmesh, not the render mesh: waypoints come from Detour
/// (vendor/server/src/map/navmesh/detour_navmesh.cpp DetourNavMesh::findPath / findRandomPosition over
/// DetourNavMeshQuery) and vendor/server/src/map/ai/helpers/pathfind/pathfind.cpp CPathFind::StepTo walks Y
/// to that waypoint. Detour poly heights differ from the MZB collision surface by up to a navmesh
/// cell height, so the POS packet Y is approximate: it picks the level, and this system places
/// remote ground movers on their own collision mesh. Kuluu inference from the LSB navmesh model,
/// not an observed retail client behavior.
///
/// Runs every frame on the RENDERED (x,z) so the model rides the slope during interpolation; no
/// history, no distance test, no snap constant: the SnapBand bands own the jump decision already.
/// Self is unchanged. The 0x45 Info movement byte from the loaded model gates it: Flying keeps
/// server Y while alive (no ground to stand on); Walking/Large/Sliding/Unset ground. A dead
/// entity grounds regardless of movement type (see the block in the body for why). A None answer
/// (off-mesh, unloaded interior) keeps server Y and logs once per entity at debug. A running
/// cutscene owns a touched entity's transform until CutsceneEnded releases it, so those are
/// skipped; only entities routed through the prediction model ground here, since mount actors
/// and Other kinds carry no sample (mounts are pinned to their rider by
/// pin_mount_actors_system).
#[cfg(not(target_arch = "wasm32"))]
pub fn ground_remote_movers_system(
    collision: Res<crate::dat_mzb::MzbCollisionGeometry>,
    prediction: Res<EntityPrediction>,
    cutscene: Res<crate::scheduler_runtime::CutsceneActorState>,
    mut q: Query<(Entity, &WorldEntity, &mut Transform), Without<IsSelf>>,
    q_children: Query<&Children>,
    q_render: Query<&crate::ffxi_actor_render::FfxiRenderActor>,
    q_dead: Query<(), With<crate::scheduler_runtime::DeadFromAction>>,
) {
    for (entity, world, mut transform) in &mut q {
        if !matches!(
            world.kind,
            EntityKind::Mob | EntityKind::Pc | EntityKind::Pet | EntityKind::Npc
        ) {
            continue;
        }
        if cutscene.is_touched(world.id) {
            continue;
        }
        let Some(sample) = prediction.by_id.get(&world.id) else {
            continue;
        };
        // 0x45 Info movement byte from the loaded model (Unset when the DAT carries no CIB, or no
        // render actor exists yet): Flying keeps server Y while alive. Death overrides the
        // exemption: LSB never writes Y on the KO transition (vendor/server entity_update.cpp sets
        // Y only under UPDATE_POS; the death path raises UPDATE_HP with Hpp = GetHPP() == 0 and
        // leaves loc.p untouched), so a dead flyer's last POS Y is wherever it was hovering and it
        // would sit in the air forever. Movement type describes locomotion, not corpses; a corpse
        // grounds like everything else. DeadFromAction is latched on the render child (cleared on
        // raise), so a raised flyer lifts back to server Y on its own.
        let mut flying = false;
        let mut dead = false;
        if let Ok(children) = q_children.get(entity) {
            for child in children.iter() {
                if let Ok(actor) = q_render.get(child) {
                    flying |= actor.movement_type() == ffxi_dat::cib::MovementType::Flying;
                }
                dead |= q_dead.get(child).is_ok();
            }
        }
        if flying && !dead {
            continue;
        }
        let xz = Vec2::new(transform.translation.x, transform.translation.z);
        match collision.ground_nearest(xz, sample.server_pos.y) {
            Some(ground_y) => transform.translation.y = ground_y,
            None => {
                if GROUND_OFF_MESH_SEEN
                    .get_or_init(Default::default)
                    .lock()
                    .ok()
                    .is_some_and(|mut seen| seen.insert(world.id))
                {
                    tracing::debug!(
                        target: "motion",
                        id = world.id,
                        ?xz,
                        ref_y = sample.server_pos.y,
                        "remote grounding off-mesh; keeping server Y"
                    );
                }
            }
        }
    }
}

#[derive(Resource, Debug, Clone)]
pub struct ModelViewerClipOverride {
    pub clip_name: String,
}

impl ModelViewerClipOverride {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            clip_name: name.into(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn enumerate_clips_for_skel(
    root: &DatRoot,
    skel_file_id: u32,
) -> Vec<(String, Arc<Mo2Animation>)> {
    let mut out = Vec::new();
    let mut sources: Vec<u32> = vec![skel_file_id];
    if let Some(motion) = motion_dat_for_skel(skel_file_id) {
        sources.push(motion);
    }
    let mut seen = std::collections::HashSet::<String>::new();
    for file_id in sources {
        for_each_anim_chunk_in_dat(root, file_id, |name, anim| {
            if seen.insert(name.clone()) {
                out.push((name, Arc::new(anim)));
            }
        });
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(not(target_arch = "wasm32"))]
pub fn override_anim_for_skel(
    root: &DatRoot,
    skel_file_id: u32,
    prefix: &[u8; 3],
) -> Option<Arc<Mo2Animation>> {
    if let Some(a) = load_anim_with_prefix(root, skel_file_id, prefix) {
        return Some(Arc::new(a));
    }
    let motion = motion_dat_for_skel(skel_file_id)?;
    load_anim_with_prefix(root, motion, prefix).map(Arc::new)
}

#[cfg(not(target_arch = "wasm32"))]
fn for_each_anim_chunk_in_dat(
    root: &DatRoot,
    file_id: u32,
    mut f: impl FnMut(String, Mo2Animation),
) {
    let Ok(loc) = root.resolve(file_id) else {
        return;
    };
    let Ok(bytes) = fs::read(loc.path_under(root)) else {
        return;
    };
    for chunk in walk(&bytes).filter_map(Result::ok) {
        if ChunkKind::from_u8(chunk.kind) != Some(ChunkKind::AnimMo2) {
            continue;
        }
        if let Ok(anim) = ffxi_dat::anim::parse_mo2(chunk.data, &chunk.name) {
            let name = String::from_utf8_lossy(&chunk.name)
                .trim_end_matches('\0')
                .trim_end()
                .to_string();
            f(name, anim);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_dat_fallback_resolves_for_each_pc_race() {
        let pairs = [
            (7072, 9672),
            (10248, 12848),
            (13424, 16024),
            (16600, 19200),
            (19776, 22376),
            (23176, 25776),
            (26352, 28952),
        ];
        for (skel, motion) in pairs {
            assert_eq!(
                motion_dat_fallback(skel),
                Some(motion),
                "skel {skel} should map to motion {motion}"
            );
            assert_eq!(motion_dat_for_skel(skel), Some(motion));
        }
    }

    #[test]
    fn motion_dat_returns_none_for_non_pc_skel() {
        for skel in [0u32, 7000, 50000] {
            assert_eq!(motion_dat_fallback(skel), None);
            assert_eq!(motion_dat_for_skel(skel), None);
        }
    }

    #[test]
    fn motion_dat_fallback_offset_is_consistent() {
        for skel in [7072u32, 10248, 13424, 16600, 19776, 23176, 26352] {
            let motion = motion_dat_fallback(skel).expect("PC race");
            assert_eq!(
                motion - skel,
                2600,
                "skel {skel} -> motion {motion}: offset must be +2600"
            );
        }
    }

    #[test]
    fn motion_dat_for_race_without_a_dll_is_the_fallback() {
        for race in 1u8..=8 {
            let skel = crate::dat_vos2::skeleton_file_id_fallback(race).expect("PC race");
            assert_eq!(motion_dat_for_race(None, race), motion_dat_fallback(skel));
        }
        assert_eq!(motion_dat_for_race(None, 0), None);
        assert_eq!(motion_dat_for_race(None, 9), None);
    }

    #[test]
    fn motion_dat_for_race_reads_the_installed_dll_battle_table() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let dll = ffxi_dat::main_dll::MainDll::load(root.root()).expect("FFXiMain.dll loads");
        for race in 1u8..=8 {
            let from_dll = dll.base_battle_animation_index(race).map(u32::from);
            assert!(
                from_dll.is_some(),
                "race {race} has a battle-animation base"
            );
            assert_eq!(motion_dat_for_race(Some(&dll), race), from_dll);
            let skel = u32::from(dll.base_race_config_index(race).expect("race config"));
            assert_eq!(
                motion_dat_for_skel(skel),
                from_dll,
                "race {race} via its skeleton id"
            );
        }
    }

    #[test]
    fn battle_idle_resolves_for_every_pc_race_when_dats_available() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        for skel in [7072u32, 10248, 13424, 16600, 19776, 23176, 26352] {
            let anim =
                battle_idle_anim_for_skel(&root, skel).expect("battle-idle MO2 missing for skel");
            assert!(
                anim.frames > 0,
                "skel {skel}: btl MO2 has zero frames — parse drift?"
            );
        }
    }

    #[test]
    fn run_anim_resolves_for_every_pc_race_when_dats_available() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        for skel in [7072u32, 10248, 13424, 16600, 19776, 23176, 26352] {
            let anim = run_anim_for_skel(&root, skel).expect("casual run MO2 missing for skel");
            assert!(anim.frames > 0, "skel {skel}: run MO2 has zero frames");
        }
    }

    #[test]
    fn is_moving_reads_latch_not_raw_speed() {
        let mut m = EntityMotion::default();
        m.by_id.insert(
            1,
            MotionSample {
                moving: true,
                speed: 0.0,
                ..Default::default()
            },
        );
        m.by_id.insert(
            2,
            MotionSample {
                moving: false,
                speed: 9.0,
                ..Default::default()
            },
        );
        assert!(
            m.is_moving(1),
            "latched-moving animates even at instant speed 0"
        );
        assert!(
            !m.is_moving(2),
            "latched-idle stays idle even at instant speed 9"
        );
        assert!(!m.is_moving(99), "unknown id should not animate");
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn move_hysteresis_enter_exit_and_hold() {
        assert!(
            EntityMotion::MOVE_EXIT < EntityMotion::MOVE_ENTER,
            "there must be a genuine hold band"
        );
        let mid = 0.5 * (EntityMotion::MOVE_EXIT + EntityMotion::MOVE_ENTER);

        assert!(!EntityMotion::apply_move_hysteresis(
            false,
            EntityMotion::MOVE_EXIT
        ));
        assert!(
            !EntityMotion::apply_move_hysteresis(false, mid),
            "idle holds in band"
        );
        assert!(EntityMotion::apply_move_hysteresis(
            false,
            EntityMotion::MOVE_ENTER + 0.1
        ));

        assert!(
            EntityMotion::apply_move_hysteresis(true, mid),
            "moving holds in band"
        );
        assert!(EntityMotion::apply_move_hysteresis(
            true,
            EntityMotion::MOVE_ENTER + 0.1
        ));
        assert!(!EntityMotion::apply_move_hysteresis(
            true,
            EntityMotion::MOVE_EXIT - 0.01
        ));
        assert!(!EntityMotion::apply_move_hysteresis(true, 0.0));
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn walk_run_boundary_is_sane() {
        assert!(EntityMotion::MOVE_EXIT < WALK_RUN_BOUNDARY);
        assert!(
            WALK_RUN_BOUNDARY < 5.0,
            "a base-run actor must NOT be classed as walking"
        );
        assert!(!infers_walk_gait(0.0), "stationary is not walking");
        assert!(infers_walk_gait(1.5), "slow mover walks");
        assert!(!infers_walk_gait(6.0), "runner runs, not walks");
    }

    /// A dirty chase sample: rendered at `rendered`, wire target at `server`, the measured
    /// inter-update interval `age` seconds, and a speed byte (speed_base set equal so the walk
    /// divisor applies). Mirrors what observe() leaves behind after consuming a moved update:
    /// last_interval holds the measured gap, sample_age is back to zero, the arrival segment is
    /// running (clock already zeroed by seed), and wire_jump_sq holds what this update moved in
    /// XZ (Y excluded: it must not inflate the band's jump).
    fn chase_sample(server: Vec3, rendered: Vec3, age: f32, speed_byte: u8) -> PredictSample {
        let mut s = PredictSample::seed(rendered, 0, speed_byte, speed_byte);
        s.segment_started = true;
        let dxw = server.x - rendered.x;
        let dzw = server.z - rendered.z;
        s.wire_jump_sq = dxw * dxw + dzw * dzw;
        s.server_pos = server;
        s.last_interval = age;
        s.sample_dirty = true;
        s
    }

    /// A dirty sample with distinct speed/speed_base bytes so the run/walk divisor is chosen by
    /// `speed > speed_base` (the gait rule), not by a fixed walk assumption.
    fn chase_sample_gait(
        server: Vec3,
        rendered: Vec3,
        age: f32,
        speed_byte: u8,
        base_byte: u8,
    ) -> PredictSample {
        let mut s = PredictSample::seed(rendered, 0, speed_byte, base_byte);
        s.segment_started = true;
        let dxw = server.x - rendered.x;
        let dzw = server.z - rendered.z;
        s.wire_jump_sq = dxw * dxw + dzw * dzw;
        s.server_pos = server;
        s.last_interval = age;
        s.sample_dirty = true;
        s
    }

    #[test]
    fn expected_step_uses_the_lsb_divisors() {
        // walk (speed <= speed_base): /40; run (speed > speed_base): /50, per
        // CPathFind::StepTo in vendor/server/src/map/ai/helpers/pathfind/pathfind.cpp.
        assert!(
            (expected_step_yalms(40, 40) - 1.0).abs() < 1e-6,
            "walk step = 40/40"
        );
        assert!(
            (expected_step_yalms(40, 39) - 0.8).abs() < 1e-6,
            "run step = 40/50"
        );
        assert!(
            expected_step_yalms(120, 120) > expected_step_yalms(120, 119),
            "a faster byte steps further per tick"
        );
    }

    #[test]
    fn prediction_band_normal_within_one_step() {
        let mut s = chase_sample(Vec3::new(1.0, 0.0, 0.0), Vec3::ZERO, 0.4, 40);
        advance_prediction(&mut s, 1.0 / 60.0, true);
        assert_eq!(s.last_update.unwrap().band, SnapBand::Normal);
        assert!(
            (s.rendered_pos.x - 1.0).abs() > 1e-3,
            "a Normal band chases rather than snapping: {}",
            s.rendered_pos.x
        );
    }

    #[test]
    fn prediction_band_stretch_between_one_and_two_steps() {
        let mut s = chase_sample(Vec3::new(1.5, 0.0, 0.0), Vec3::ZERO, 0.4, 40);
        advance_prediction(&mut s, 1.0 / 60.0, true);
        assert_eq!(s.last_update.unwrap().band, SnapBand::Stretch);
        assert!(
            (s.rendered_pos.x - 1.5).abs() > 1e-3,
            "a Stretch band chases rather than snapping: {}",
            s.rendered_pos.x
        );
    }

    #[test]
    fn prediction_band_pop_beyond_two_steps_snaps_xz() {
        let mut s = chase_sample(Vec3::new(3.0, 0.0, 0.0), Vec3::ZERO, 0.4, 40);
        let (pos, _) = advance_prediction(&mut s, 1.0 / 60.0, true);
        assert_eq!(s.last_update.unwrap().band, SnapBand::Pop);
        assert_eq!(pos.x, 3.0, "a Pop band snaps XZ onto the server position");
    }

    #[test]
    fn prediction_stale_gap_resets_the_ring_and_tweens() {
        let mut s = chase_sample(Vec3::new(0.5, 0.0, 0.0), Vec3::ZERO, 10.0, 40);
        advance_prediction(&mut s, 1.0 / 60.0, true);
        assert_eq!(
            s.last_update.unwrap().band,
            SnapBand::Normal,
            "a one-step move after a long idle is Normal"
        );
        assert!(
            s.rendered_pos.x > 1e-3,
            "a stale one-step move chases rather than snapping: {}",
            s.rendered_pos.x
        );
        let ring_max = s.sample_intervals.iter().copied().fold(0.0f32, f32::max);
        assert!(
            (ring_max - EntityPrediction::TICK_SECS).abs() < 1e-6,
            "the stale gap reset the ring to its seed: {ring_max}"
        );
        assert!(
            (s.segment_duration
                - EntityPrediction::TICK_SECS * EntityPrediction::INTERVAL_HEADROOM)
                .abs()
                < 1e-6,
            "the budget is one tick plus headroom again: {}",
            s.segment_duration
        );
    }

    #[test]
    fn prediction_normal_band_tolerates_the_float_boundary() {
        let mut s = chase_sample(Vec3::new(1.0004, 0.0, 0.0), Vec3::ZERO, 0.4, 40);
        advance_prediction(&mut s, 1.0 / 60.0, true);
        assert_eq!(
            s.last_update.unwrap().band,
            SnapBand::Normal,
            "ratio 1.0004 is inside the float-boundary epsilon"
        );

        let mut s = chase_sample(Vec3::new(1.002, 0.0, 0.0), Vec3::ZERO, 0.4, 40);
        advance_prediction(&mut s, 1.0 / 60.0, true);
        assert_eq!(
            s.last_update.unwrap().band,
            SnapBand::Stretch,
            "ratio 1.002 is past the epsilon"
        );
    }

    #[test]
    fn prediction_y_assigns_server_directly() {
        // Y is fully server-resolved (vendor/server/src/map/ai/helpers/pathfind/pathfind.cpp
        // CPathFind::StepTo walks it along the slope and
        // snaps it onto target.y on arrival): it lands on server.y in one update with no exp
        // smoothing. A floor-height change must not inflate the XZ jump into a Pop either.
        let mut s = chase_sample(Vec3::new(1.0, 5.0, 0.0), Vec3::new(0.0, 0.0, 0.0), 0.4, 40);
        advance_prediction(&mut s, 1.0 / 60.0, true);
        assert_eq!(
            s.rendered_pos.y, 5.0,
            "Y is assigned directly from the server"
        );
        assert_eq!(s.last_update.unwrap().band, SnapBand::Normal);
    }

    /// A minimal app running the prediction tween and the per-frame remote grounding against a
    /// single slab floor at `floor_y` (x/z in -4..4).
    fn grounding_app(floor_y: f32) -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<EntityPrediction>()
            .init_resource::<crate::scheduler_runtime::CutsceneActorState>()
            .insert_resource(MotionProbe::init())
            .insert_resource(crate::dat_mzb::MzbCollisionGeometry::from_block(
                crate::dat_mzb::ground_tests::slab_block(&[(floor_y, Vec3::Y)]),
            ))
            .add_systems(
                Update,
                (predict_entities_system, ground_remote_movers_system).chain(),
            );
        app
    }

    fn spawn_remote_mob(app: &mut App, id: u32) -> Entity {
        app.world_mut()
            .spawn((
                WorldEntity {
                    id,
                    act_index: 1,
                    kind: EntityKind::Mob,
                },
                Transform::default(),
            ))
            .id()
    }

    /// 60 fps render frames spanning the settle gap the grounding tests leave between
    /// server confirms: one and a quarter AI ticks, a measured inter-update interval that
    /// keeps the segment budget ahead of the mid-tween probes that follow it.
    const AI_TICK_FRAMES: usize = 30;

    fn tick_frames(app: &mut App, frames: usize) {
        for _ in 0..frames {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
            app.update();
        }
    }

    #[test]
    fn grounded_remote_mover_rides_the_collision_mesh() {
        let mut app = grounding_app(2.0);
        let mob = spawn_remote_mob(&mut app, 900);
        app.world_mut().resource_mut::<EntityPrediction>().observe(
            900,
            Vec3::new(0.5, 1.5, 0.0),
            0,
            40,
            40,
        );
        tick_frames(&mut app, 5);
        let t = app.world().get::<Transform>(mob).unwrap();
        assert!(
            (t.translation.y - 2.0).abs() < 1e-4,
            "mesh Y, not the wire Y"
        );
        assert_ne!(t.translation.y, 1.5);
        assert!((t.translation.x - 0.5).abs() < 1e-4);
    }

    #[test]
    fn grounding_runs_on_the_tweened_intermediate_position() {
        let mut app = grounding_app(2.0);
        let mob = spawn_remote_mob(&mut app, 901);
        app.world_mut().resource_mut::<EntityPrediction>().observe(
            901,
            Vec3::new(0.0, 1.5, 0.0),
            0,
            40,
            40,
        );
        tick_frames(&mut app, AI_TICK_FRAMES);
        app.world_mut().resource_mut::<EntityPrediction>().observe(
            901,
            Vec3::new(1.5, 1.5, 0.0),
            0,
            40,
            40,
        );
        let mut next = 0;
        for offset in [2usize, 8, 16] {
            tick_frames(&mut app, offset - next);
            next = offset;
            let t = app.world().get::<Transform>(mob).unwrap();
            assert!(
                (0.0..1.5).contains(&t.translation.x),
                "frame +{offset}: mid-tween, x={}",
                t.translation.x
            );
            assert!(
                (t.translation.y - 2.0).abs() < 1e-4,
                "frame +{offset}: grounded on the interpolated XZ"
            );
        }
        tick_frames(&mut app, 60);
        let t = app.world().get::<Transform>(mob).unwrap();
        assert!(
            (t.translation.x - 1.5).abs() < 1e-4,
            "arrived at the confirmed endpoint"
        );
        assert!((t.translation.y - 2.0).abs() < 1e-4);
    }

    #[test]
    fn off_mesh_remote_mover_keeps_server_y() {
        let mut app = grounding_app(2.0);
        let mob = spawn_remote_mob(&mut app, 902);
        app.world_mut().resource_mut::<EntityPrediction>().observe(
            902,
            Vec3::new(100.0, 1.5, 0.0),
            0,
            40,
            40,
        );
        tick_frames(&mut app, 5);
        let t = app.world().get::<Transform>(mob).unwrap();
        assert!(
            (t.translation.y - 1.5).abs() < 1e-6,
            "server Y kept off-mesh"
        );
        assert!((t.translation.x - 100.0).abs() < 1e-4);
    }

    #[test]
    fn flying_remote_mover_keeps_server_y() {
        // The 0x45 Info movement byte (ffxi-dat/src/cib.rs MovementType) gates grounding:
        // a Flying model has no ground to stand on, so it keeps the wire Y even inside the slab
        // footprint.
        let mut app = grounding_app(2.0);
        let mob = spawn_remote_mob(&mut app, 903);
        let skeleton = ffxi_dat::skel::Skeleton {
            id: ffxi_dat::datid::DatId::from_name(b"skel"),
            joints: Vec::new(),
            references: Vec::new(),
            bounding_boxes: Vec::new(),
        };
        let child = app
            .world_mut()
            .spawn(
                crate::ffxi_actor_render::render_actor_with_movement_for_test(
                    skeleton,
                    Vec::new(),
                    ffxi_dat::cib::MovementType::Flying,
                ),
            )
            .id();
        app.world_mut().entity_mut(mob).add_child(child);
        app.world_mut().resource_mut::<EntityPrediction>().observe(
            903,
            Vec3::new(0.5, 1.5, 0.0),
            0,
            40,
            40,
        );
        tick_frames(&mut app, 5);
        let t = app.world().get::<Transform>(mob).unwrap();
        assert!(
            (t.translation.y - 1.5).abs() < 1e-6,
            "Flying keeps the wire Y: {}",
            t.translation.y
        );
        assert!((t.translation.x - 0.5).abs() < 1e-4);
    }

    #[test]
    fn dead_flying_remote_mover_grounds() {
        // Death overrides the Flying exemption: LSB never writes Y on the KO transition, so a
        // dead flyer's last POS Y is its hover height; the corpse grounds like everything else.
        let mut app = grounding_app(2.0);
        let mob = spawn_remote_mob(&mut app, 904);
        let skeleton = ffxi_dat::skel::Skeleton {
            id: ffxi_dat::datid::DatId::from_name(b"skel"),
            joints: Vec::new(),
            references: Vec::new(),
            bounding_boxes: Vec::new(),
        };
        let child = app
            .world_mut()
            .spawn(
                crate::ffxi_actor_render::render_actor_with_movement_for_test(
                    skeleton,
                    Vec::new(),
                    ffxi_dat::cib::MovementType::Flying,
                ),
            )
            .id();
        app.world_mut().entity_mut(mob).add_child(child);
        app.world_mut()
            .entity_mut(child)
            .insert(crate::scheduler_runtime::DeadFromAction::default());
        app.world_mut().resource_mut::<EntityPrediction>().observe(
            904,
            Vec3::new(0.5, 1.5, 0.0),
            0,
            40,
            40,
        );
        tick_frames(&mut app, 1);
        let t = app.world().get::<Transform>(mob).unwrap();
        assert!(
            (t.translation.y - 2.0).abs() < 1e-6,
            "a dead flyer grounds on the slab, not its hover Y: {}",
            t.translation.y
        );
    }

    #[test]
    fn raised_flying_remote_mover_returns_to_server_y() {
        let mut app = grounding_app(2.0);
        let mob = spawn_remote_mob(&mut app, 905);
        let skeleton = ffxi_dat::skel::Skeleton {
            id: ffxi_dat::datid::DatId::from_name(b"skel"),
            joints: Vec::new(),
            references: Vec::new(),
            bounding_boxes: Vec::new(),
        };
        let child = app
            .world_mut()
            .spawn(
                crate::ffxi_actor_render::render_actor_with_movement_for_test(
                    skeleton,
                    Vec::new(),
                    ffxi_dat::cib::MovementType::Flying,
                ),
            )
            .id();
        app.world_mut().entity_mut(mob).add_child(child);
        app.world_mut()
            .entity_mut(child)
            .insert(crate::scheduler_runtime::DeadFromAction::default());
        app.world_mut().resource_mut::<EntityPrediction>().observe(
            905,
            Vec3::new(0.5, 1.5, 0.0),
            0,
            40,
            40,
        );
        tick_frames(&mut app, 60);
        let t = app.world().get::<Transform>(mob).unwrap();
        assert!(
            (t.translation.y - 2.0).abs() < 1e-6,
            "dead flyer stays grounded: {}",
            t.translation.y
        );
        app.world_mut()
            .entity_mut(child)
            .remove::<crate::scheduler_runtime::DeadFromAction>();
        tick_frames(&mut app, 1);
        let t = app.world().get::<Transform>(mob).unwrap();
        assert!(
            (t.translation.y - 1.5).abs() < 1e-6,
            "a raised flyer lifts back to server Y: {}",
            t.translation.y
        );
    }

    #[test]
    fn prediction_chase_paces_to_packet_cadence() {
        let mut p = EntityPrediction::default();
        p.observe(1, Vec3::ZERO, 0, 40, 40);
        for _ in 0..24 {
            advance_prediction(p.by_id.get_mut(&1).unwrap(), 1.0 / 60.0, true);
        }
        p.observe(1, Vec3::new(1.5, 0.0, 0.0), 0, 40, 40);
        let s = p.by_id.get_mut(&1).unwrap();
        assert!(s.sample_dirty, "the moved update is pending consumption");
        let ring_max = s.sample_intervals.iter().copied().fold(0.0f32, f32::max);
        assert!(
            (ring_max - 0.4).abs() < 1e-3,
            "the measured interval is in the ring: {ring_max}"
        );
        assert!(
            (s.segment_duration - ring_max * EntityPrediction::INTERVAL_HEADROOM).abs() < 1e-6,
            "budget = widest recent interval plus headroom: {}",
            s.segment_duration
        );
        let dt = 1.0 / 60.0;
        advance_prediction(s, dt, true);
        assert_eq!(
            s.last_update.unwrap().band,
            SnapBand::Stretch,
            "a 1.5 yalms jump is between one and two walk steps"
        );
        let gap_after = (s.server_pos.x - s.rendered_pos.x).abs();
        let expected_gap = 1.5 * (1.0 - dt / s.segment_duration);
        assert!(
            (gap_after - expected_gap).abs() < 1e-4,
            "closes at jump/budget per frame: {} vs {expected_gap}",
            gap_after
        );
    }

    #[test]
    fn prediction_run_gait_bands_off_the_run_step() {
        let mut walker = chase_sample(Vec3::new(1.7, 0.0, 0.0), Vec3::ZERO, 0.4, 40);
        advance_prediction(&mut walker, 1.0 / 60.0, true);
        assert_eq!(
            walker.last_update.unwrap().band,
            SnapBand::Stretch,
            "a 1.7 yalms jump is between one and two walk steps"
        );

        let mut runner = chase_sample_gait(Vec3::new(1.7, 0.0, 0.0), Vec3::ZERO, 0.4, 40, 39);
        advance_prediction(&mut runner, 1.0 / 60.0, true);
        let run_step = expected_step_yalms(40, 39);
        assert!((run_step - 0.8).abs() < 1e-6);
        assert_eq!(
            runner.last_update.unwrap().band,
            SnapBand::Pop,
            "the same jump is beyond two /50 run steps"
        );
    }

    #[test]
    fn prediction_chase_never_overshoots_the_target() {
        let mut s = chase_sample(Vec3::new(1.5, 0.0, 0.0), Vec3::ZERO, 0.4, 40);
        for _ in 0..600 {
            advance_prediction(&mut s, 1.0 / 60.0, true);
            assert!(
                s.rendered_pos.x <= 1.5 + 1e-6,
                "chase must not run past the target: {}",
                s.rendered_pos.x
            );
        }
    }

    #[test]
    fn prediction_chase_arrives_exactly_and_reports_idle() {
        let mut s = chase_sample(Vec3::new(1.5, 0.0, 0.0), Vec3::ZERO, 0.4, 40);
        let dt = 1.0 / 60.0;
        for _ in 0..600 {
            if !s.is_chasing() {
                break;
            }
            advance_prediction(&mut s, dt, true);
        }
        assert!(
            !s.is_chasing(),
            "the chase reports idle once it reaches the target"
        );
        let arrived = s.rendered_pos.x;
        assert!(
            (arrived - 1.5).abs() < 1e-3,
            "idle means on target: {arrived}"
        );
        for _ in 0..60 {
            advance_prediction(&mut s, dt, true);
        }
        assert!(!s.is_chasing());
        assert!(
            (s.rendered_pos.x - arrived).abs() < 1e-3,
            "stays put: {arrived} -> {}",
            s.rendered_pos.x
        );
    }

    #[test]
    fn prediction_keeps_chasing_between_updates() {
        let mut s = chase_sample(Vec3::new(1.5, 0.0, 0.0), Vec3::ZERO, 0.4, 40);
        advance_prediction(&mut s, 1.0 / 60.0, true);
        let dt = 1.0 / 60.0;
        for _ in 0..14 {
            advance_prediction(&mut s, dt, true);
        }
        assert_eq!(s.idle_frames, 14, "frames without an update count");
        let expected_x = 1.5 * (0.25 / s.segment_duration);
        assert!(
            (s.rendered_pos.x - expected_x).abs() < 1e-3,
            "closes at jump/budget between updates: {} vs {expected_x}",
            s.rendered_pos.x
        );
        for _ in 0..45 {
            advance_prediction(&mut s, dt, true);
        }
        assert_eq!(s.idle_frames, 59);
        assert!(
            (s.rendered_pos.x - 1.5).abs() < 1e-4,
            "arrived on target: {}",
            s.rendered_pos.x
        );
    }

    #[test]
    fn is_chasing_holds_across_the_arrival_segment() {
        let mut s = chase_sample(Vec3::new(0.02, 0.0, 0.0), Vec3::ZERO, 0.4, 40);
        let dt = 1.0 / 60.0;
        advance_prediction(&mut s, dt, true);
        assert!(
            s.segment_elapsed < s.segment_duration,
            "the segment extends past this frame"
        );
        let mut held = false;
        for _ in 0..240 {
            advance_prediction(&mut s, dt, true);
            let on_target = (s.rendered_pos.x - 0.02).abs() < 1e-3;
            if on_target && s.segment_elapsed < s.segment_duration {
                held = true;
                break;
            }
        }
        assert!(held, "the entity sat on target inside its arrival segment");
        assert!(s.is_chasing(), "the moving flag holds across a late packet");
        for _ in 0..240 {
            advance_prediction(&mut s, dt, true);
            if !s.is_chasing() {
                break;
            }
        }
        assert!(!s.is_chasing(), "idle once the segment lapses");
    }

    #[test]
    fn prediction_heading_eases_toward_the_target() {
        let mut s = PredictSample::seed(Vec3::ZERO, 0, 40, 40);
        let start = s.rendered_heading_rad;
        s.target_heading = 16;
        let target = heading_to_rad(16);
        assert!(
            (start - target).abs() > f32::EPSILON,
            "the test needs a real heading change"
        );
        let dist = |h: f32| {
            let mut d = (target - h).rem_euclid(std::f32::consts::TAU);
            if d > std::f32::consts::PI {
                d -= std::f32::consts::TAU;
            }
            d.abs()
        };
        advance_prediction(&mut s, 1.0 / 60.0, true);
        assert_ne!(
            s.rendered_heading_rad, target,
            "the first frame must not snap the heading"
        );
        assert!(
            dist(s.rendered_heading_rad) < dist(start),
            "heading moves toward the target"
        );
        for _ in 0..600 {
            advance_prediction(&mut s, 1.0 / 60.0, true);
        }
        assert!(
            dist(s.rendered_heading_rad) < 1e-3,
            "converges on the target heading"
        );
    }

    /// Port of vendor/server/src/common/utils.cpp worldAngle (pinned vendor/server), byte for
    /// byte: f32 math, truncating i16 cast, double-mod into [0, 256). The 0.1 yalms gate is the
    /// XZ form of utils.h isWithinDistance(A, B, 0.1f, true); A.rotation is assumed 0 here.
    /// Inputs are Bevy-space positions: ffxi_to_bevy maps (lsb.x, lsb.z) to (x, -z), so the LSB
    /// horizontal delta this formula needs is (dx, -dz). Feeding a raw Bevy delta would compute
    /// the mirrored byte.
    fn world_angle_byte(a: Vec3, b: Vec3) -> u8 {
        let dx = b.x - a.x;
        let dz_lsb = -(b.z - a.z);
        if dx * dx + dz_lsb * dz_lsb <= 0.1 * 0.1 {
            return 0;
        }
        let radians = dz_lsb.atan2(dx);
        let raw = (radians * -(128.0 / std::f32::consts::PI)) as i16;
        ((raw % 256 + 256) % 256) as u8
    }

    /// Bevy forward to wire ground plane: ffxi_to_bevy maps (wx, wy) to (x, -z), so the inverse
    /// of a Bevy direction (f.x, f.z) is the wire direction (f.x, -f.z).
    fn bevy_forward_to_wire(f: Vec3) -> Vec2 {
        Vec2::new(f.x, -f.z)
    }

    #[test]
    fn heading_forward_points_along_the_world_angle_direction() {
        // For a grid of Bevy-space displacements, the byte LSB would write for a -> b must make
        // kuluu's forward point from a toward b in wire space. One quantization step is 1.4 deg,
        // so even an off-by-one byte passes dot > 0.99; a convention error (a quarter turn) fails
        // by ~90. The want vector is the true wire delta: ffxi_to_bevy maps (lsb.x, lsb.z) to
        // (x, -z), so a Bevy step (dx, dz) is the wire step (dx, -dz).
        let origin = Vec3::ZERO;
        for dx in [-4.0f32, -1.5, 0.7, 2.0, 4.0] {
            for dz in [-4.0f32, -2.0, -0.5, 1.5, 3.0] {
                if dx.abs() < 1e-6 && dz.abs() < 1e-6 {
                    continue;
                }
                let byte = world_angle_byte(origin, Vec3::new(dx, 0.0, dz));
                let wire_fwd = bevy_forward_to_wire(heading_forward(byte));
                let want = Vec2::new(dx, -dz).normalize();
                assert!(
                    wire_fwd.dot(want) > 0.99,
                    "a->b ({dx}, {dz}): byte {byte} points the wrong way"
                );
            }
        }
    }

    #[test]
    fn heading_byte_round_trips_within_one_step() {
        // Encoding kuluu's forward for byte b through the ported worldAngle must land on b or a
        // neighbor: LSB's own truncating i16 cast quantizes to 256 steps, so exact identity is
        // not what the pinned formula gives; a convention error would show up as a constant
        // offset (a quarter turn is 64 bytes). heading_forward(b) is Bevy space, which is what
        // world_angle_byte expects.
        for b in 0u8..=255 {
            let got = world_angle_byte(Vec3::ZERO, heading_forward(b));
            let diff = ((got as i32 - b as i32 + 128) % 256 + 256) % 256 - 128;
            assert!(diff.abs() <= 1, "byte {b} encodes back to {got}");
        }
    }

    #[test]
    fn heading_forward_matches_step_to_travel_direction() {
        // CPathFind::StepTo (pinned vendor/server) moves a mob by (cosf(radians), sinf(radians))
        // with radians = (1 - rotation / 256) * 2 * PI: kuluu's wire-space forward for the same
        // byte must be that exact direction. This is LSB's own decode of its own byte, so it
        // proves the basis without going through worldAngle's quantization.
        for b in 0u8..=255 {
            let radians = (1.0 - b as f32 / 256.0) * std::f32::consts::TAU;
            let step_dir = Vec2::new(radians.cos(), radians.sin()).normalize();
            let wire_fwd = bevy_forward_to_wire(heading_forward(b));
            assert!(
                wire_fwd.dot(step_dir) > 1.0 - 1e-6,
                "byte {b}: kuluu forward is not StepTo's travel direction"
            );
        }
    }

    #[test]
    fn pos_heading_applies_on_the_same_snapshot_as_position() {
        // LSB writes the rotation byte and the position in one POS block: CPathFind::LookAt sets
        // loc.p.rotation, then updatemask |= UPDATE_POS. A remote actor receiving a POS update
        // with the byte for its travel direction must start facing that direction on the same
        // frame: observe() stores target_heading together with the position and
        // advance_prediction eases toward it immediately (no one-snapshot lag). The assertions
        // read the actor's actual Transform, not just the prediction resource.
        let mut app = grounding_app(0.0);
        let mob = spawn_remote_mob(&mut app, 950);
        app.world_mut()
            .resource_mut::<EntityPrediction>()
            .observe(950, Vec3::ZERO, 128, 40, 40);
        tick_frames(&mut app, 60);

        let a = Vec3::new(1.0, 0.0, 2.0);
        let b = Vec3::new(5.0, 0.0, 9.0);
        let byte = world_angle_byte(a, b);
        let target = heading_to_rad(byte);
        let dist = |h: f32| {
            let mut d = (target - h).rem_euclid(std::f32::consts::TAU);
            if d > std::f32::consts::PI {
                d -= std::f32::consts::TAU;
            }
            d.abs()
        };

        let before_h = app.world().resource::<EntityPrediction>().by_id[&950].rendered_heading_rad;
        let t_before = *app.world().get::<Transform>(mob).unwrap();

        app.world_mut()
            .resource_mut::<EntityPrediction>()
            .observe(950, a, byte, 40, 40);
        tick_frames(&mut app, 1);

        let t_after = *app.world().get::<Transform>(mob).unwrap();
        assert!(
            t_after.translation.distance(t_before.translation) > 1e-6,
            "the position update must move the actor on the same frame"
        );
        assert!(
            t_after.translation.distance(a) < t_before.translation.distance(a),
            "the tween closes the gap toward the new server position on the first frame"
        );
        let after_h = app.world().resource::<EntityPrediction>().by_id[&950].rendered_heading_rad;
        assert!(
            dist(after_h) < dist(before_h),
            "the first frame after the POS update already turns toward the travel direction"
        );

        tick_frames(&mut app, 240);
        let settled = *app.world().get::<Transform>(mob).unwrap();
        let yaw = 2.0 * settled.rotation.y.atan2(settled.rotation.w);
        assert!(
            dist(-yaw) < 1e-3,
            "the actor ends facing its travel direction (rotation is from_rotation_y(-heading))"
        );
    }

    #[test]
    fn prediction_static_actor_does_not_drift() {
        let anchor = Vec3::new(3.0, 1.0, 2.0);
        let mut s = PredictSample::seed(anchor, 64, 0, 0);
        for _ in 0..60 {
            advance_prediction(&mut s, 1.0 / 30.0, true);
        }
        assert!(
            (s.rendered_pos - anchor).length() < 0.05,
            "stays put: {:?}",
            s.rendered_pos
        );
    }

    #[test]
    fn observe_seeds_then_flags_only_on_real_move() {
        let mut p = EntityPrediction::default();
        p.observe(7, Vec3::new(1.0, 0.0, 0.0), 10, 25, 40);
        let s = p.by_id[&7];
        assert!(
            s.initialized && !s.sample_dirty,
            "first sight seeds, not dirty"
        );
        assert_eq!(s.rendered_pos, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(s.packet_speed, 25);
        assert_eq!(s.packet_speed_base, 40);
        assert!(!s.is_chasing(), "a fresh sample is already on target");

        p.observe(7, Vec3::new(1.0, 0.0, 0.0), 10, 25, 40);
        assert!(
            !p.by_id[&7].sample_dirty,
            "unchanged position must not re-ingest"
        );

        p.observe(7, Vec3::new(2.0, 0.0, 0.0), 20, 40, 50);
        assert!(p.by_id[&7].sample_dirty, "moved position raises dirty");
        assert_eq!(p.by_id[&7].target_heading, 20);
        assert_eq!(p.by_id[&7].packet_speed, 40, "speed byte tracks the update");
        assert_eq!(
            p.by_id[&7].packet_speed_base, 50,
            "animationSpeed byte tracks the update"
        );

        let seeded_budget = EntityPrediction::TICK_SECS * EntityPrediction::INTERVAL_HEADROOM;
        p.observe(7, Vec3::new(2.0, 0.0, 0.0), 20, 40, 50);
        assert!(
            (p.by_id[&7].segment_duration - seeded_budget).abs() < 1e-6,
            "observe does not re-budget the segment: {} vs seeded {}",
            p.by_id[&7].segment_duration,
            seeded_budget
        );
    }

    #[test]
    fn remote_running_keeps_gait_across_captured_lsb_arrival_jitter() {
        const FRAME_SECS: f32 = 1.0 / 60.0;
        const RUN_SPEED: f32 = 4.8;
        const MAX_SPEED_MULTIPLIER: f32 = 2.0;
        const WARMUP_FRAMES: usize = 180;
        const STOP_FRAMES: usize = 180;
        const ARRIVAL_FRAMES: [usize; 10] = [24, 48, 66, 114, 138, 162, 192, 234, 264, 288];
        const SPEED_BYTE: u8 = 120;
        const BASE_BYTE: u8 = 48;
        let packet_step = expected_step_yalms(SPEED_BYTE, BASE_BYTE);
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<SceneState>()
            .init_resource::<EntityPrediction>()
            .init_resource::<crate::scheduler_runtime::CutsceneActorState>()
            .init_resource::<EntityMotion>();
        app.world_mut().insert_resource(MotionProbe::init());
        app.add_systems(
            Update,
            (predict_entities_system, track_entity_motion_system).chain(),
        );
        let actor = app
            .world_mut()
            .spawn((
                WorldEntity {
                    id: 7,
                    act_index: 1,
                    kind: EntityKind::Pc,
                },
                Transform::default(),
            ))
            .id();
        app.world_mut().resource_mut::<EntityPrediction>().observe(
            7,
            Vec3::ZERO,
            0,
            SPEED_BYTE,
            BASE_BYTE,
        );
        let mut packet = 0;
        let mut confirmed = 0.0;
        let mut previous = 0.0;
        let last_arrival = *ARRIVAL_FRAMES.last().unwrap();
        for frame in 0..last_arrival + STOP_FRAMES {
            if ARRIVAL_FRAMES.get(packet) == Some(&frame) {
                packet += 1;
                confirmed = packet as f32 * packet_step;
                app.world_mut().resource_mut::<EntityPrediction>().observe(
                    7,
                    Vec3::X * confirmed,
                    0,
                    SPEED_BYTE,
                    BASE_BYTE,
                );
            }
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(FRAME_SECS));
            app.update();
            let x = app.world().get::<Transform>(actor).unwrap().translation.x;
            assert!(x >= previous && x <= confirmed);
            assert!(x - previous <= RUN_SPEED * FRAME_SECS * MAX_SPEED_MULTIPLIER);
            previous = x;
            if (WARMUP_FRAMES..last_arrival).contains(&frame) {
                let sample = app.world().resource::<EntityMotion>().sample(7).unwrap();
                assert!(sample.moving);
                assert!(
                    !infers_walk_gait(sample.speed),
                    "run restarted at frame {frame}"
                );
            }
        }
        assert_eq!(previous, confirmed);
        assert!(!app.world().resource::<EntityMotion>().is_moving(7));
    }

    #[test]
    fn remote_motion_jitter_and_missing_updates_stay_between_confirmed_endpoints() {
        const FRAME_SECS: f32 = 1.0 / 60.0;
        const RUN_SPEED: f32 = 4.8;
        let mut prediction = EntityPrediction::default();
        prediction.observe(7, Vec3::ZERO, 0, 80, 80);
        let mut elapsed = 0.0;
        for frames in [12, 9, 15, 24, 6, 18] {
            let target = Vec3::X * elapsed * RUN_SPEED;
            prediction.observe(7, target, 0, 80, 80);
            let sample = prediction.by_id.get_mut(&7).unwrap();
            let mut previous = sample.rendered_pos.x;
            for _ in 0..frames {
                let (position, _) = advance_prediction(sample, FRAME_SECS, true);
                assert!(position.x >= previous && position.x <= target.x);
                previous = position.x;
            }
            elapsed += frames as f32 * FRAME_SECS;
        }
    }

    #[test]
    fn worm_speed_zero_mirrors_the_server_substitute_step() {
        // vendor/server/src/map/ai/helpers/pathfind/pathfind.cpp CPathFind::StepTo substitutes speed = 20
        // for a xi::RoamFlag::Worm mob whose
        // base speed is 0 and never writes it back, so the wire byte stays 0 while the server still
        // advances 20 / divisor per tick. A 0 byte on an update that actually moved must band off
        // that substitute step: successive 0.5 yalms hops are exactly one walk step each -> Normal,
        // chase, never Pop.
        assert!(
            (expected_step_yalms(0, 0) - 0.5).abs() < 1e-6,
            "the worm substitute step is 20/40"
        );
        let mut p = EntityPrediction::default();
        p.observe(9, Vec3::ZERO, 0, 0, 0);
        for hop in 1..=6 {
            for _ in 0..24 {
                advance_prediction(p.by_id.get_mut(&9).unwrap(), 1.0 / 60.0, true);
            }
            p.observe(9, Vec3::new(hop as f32 * 0.5, 0.0, 0.0), 0, 0, 0);
            let s = p.by_id.get_mut(&9).unwrap();
            advance_prediction(s, 1.0 / 60.0, true);
            let u = s.last_update.unwrap();
            assert_eq!(
                u.band,
                SnapBand::Normal,
                "hop {hop}: a worm tick is one substitute step"
            );
            assert!(
                (s.rendered_pos.x - hop as f32 * 0.5).abs() > 1e-4,
                "hop {hop}: Normal chases rather than snapping: {}",
                s.rendered_pos.x
            );
        }
        let s = p.by_id.get_mut(&9).unwrap();
        for _ in 0..39 {
            advance_prediction(s, 1.0 / 60.0, true);
        }
        assert_eq!(
            s.rendered_pos.x, 3.0,
            "arrives exactly at segment end: {}",
            s.rendered_pos.x
        );
    }

    #[test]
    fn worm_speed_zero_idle_produces_no_chase_and_no_band_churn() {
        let anchor = Vec3::new(12.0, -4.0, 7.5);
        let mut p = EntityPrediction::default();
        p.observe(9, anchor, 0, 0, 0);
        for _ in 0..600 {
            advance_prediction(p.by_id.get_mut(&9).unwrap(), 1.0 / 60.0, true);
        }
        for _ in 0..5 {
            p.observe(9, anchor, 0, 0, 0);
            advance_prediction(p.by_id.get_mut(&9).unwrap(), 1.0 / 60.0, true);
        }
        let s = &p.by_id[&9];
        assert!(!s.is_chasing(), "an idle worm has no segment to hold");
        assert!(
            s.last_update.is_none(),
            "no update was ever consumed: no band churn"
        );
        assert_eq!(s.rendered_pos, anchor, "the rendered pose does not drift");
    }

    #[test]
    fn late_tick_widens_the_ring_budget_without_popping() {
        const FRAME_SECS: f32 = 1.0 / 60.0;
        const SPEED_BYTE: u8 = 120;
        const BASE_BYTE: u8 = 48;
        let packet_step = expected_step_yalms(SPEED_BYTE, BASE_BYTE);
        let mut p = EntityPrediction::default();
        p.observe(7, Vec3::ZERO, 0, SPEED_BYTE, BASE_BYTE);
        let arrivals: [usize; 6] = [24, 48, 120, 144, 168, 192];
        let mut arrived = 0;
        let mut confirmed = 0.0;
        let mut previous = 0.0;
        let mut last_update: Option<UpdateOutcome> = None;
        for frame in 0..240 {
            if arrived < arrivals.len() && arrivals[arrived] == frame {
                arrived += 1;
                confirmed += packet_step;
                p.observe(7, Vec3::X * confirmed, 0, SPEED_BYTE, BASE_BYTE);
            }
            let s = p.by_id.get_mut(&7).unwrap();
            advance_prediction(s, FRAME_SECS, true);
            if let Some(u) = s.last_update {
                last_update = Some(u);
                assert_eq!(
                    u.band,
                    SnapBand::Normal,
                    "frame {frame}: a late tick is not a Pop"
                );
            }
            let x = s.rendered_pos.x;
            assert!(x >= previous && x <= confirmed + 1e-6, "frame {frame}");
            previous = x;
        }
        let widened = last_update.expect("updates were consumed").segment_duration;
        assert!(
            (widened - EntityPrediction::MAX_INTERVAL * EntityPrediction::INTERVAL_HEADROOM).abs()
                < 1e-6,
            "the ring kept the widest interval in the budget: {widened}"
        );
    }

    #[test]
    fn rest_stance_is_resting_matches_kind() {
        let mut s = RestStance::default();
        assert!(!s.is_resting());
        s.kind = RestKind::Sit;
        assert!(s.is_resting());
        s.kind = RestKind::Heal;
        assert!(s.is_resting());
        s.kind = RestKind::None;
        assert!(!s.is_resting());
    }

    #[test]
    fn reconcile_adopts_the_server_rest_state() {
        use ffxi_proto::decode::animation;

        assert_eq!(
            reconcile_rest_kind(RestKind::Heal, animation::NONE),
            Some(RestKind::None),
            "the server ended the rest (damage, effect wearing) — stand up"
        );
        assert_eq!(
            reconcile_rest_kind(RestKind::None, animation::HEALING),
            Some(RestKind::Heal),
            "the server has us resting — adopt it"
        );
        assert_eq!(
            reconcile_rest_kind(RestKind::Heal, animation::HEALING),
            None,
            "agreement needs no correction"
        );
        assert_eq!(reconcile_rest_kind(RestKind::None, animation::NONE), None);
    }

    #[test]
    fn reconcile_never_stands_up_a_client_side_sit() {
        use ffxi_proto::decode::animation;

        assert_eq!(
            reconcile_rest_kind(RestKind::Sit, animation::NONE),
            None,
            "/sit sends no packet, so a 0 byte must not cancel it"
        );
        assert_eq!(
            reconcile_rest_kind(RestKind::Sit, animation::HEALING),
            Some(RestKind::Heal),
            "a server-side rest still wins over the local sit"
        );
    }

    #[test]
    fn rest_exit_blocks_movement_until_the_stand_up_clip_ends() {
        let mut s = RestStance {
            kind: RestKind::Heal,
            ..Default::default()
        };
        s.begin_exit();
        assert_eq!(s.kind, RestKind::None, "the stance is released immediately");

        let dt = 1.0 / 30.0;
        assert!(s.exit_blocks_movement(dt), "pending bridge holds movement");

        s.observe_exit_clip(true);
        for _ in 0..60 {
            assert!(
                s.exit_blocks_movement(dt),
                "the Out clip outlasts the pending grace"
            );
        }

        s.observe_exit_clip(false);
        assert!(!s.exit_blocks_movement(dt), "movement resumes once it ends");
    }

    #[test]
    fn rest_exit_pending_expires_without_a_stand_up_clip() {
        let mut s = RestStance::default();
        s.begin_exit();
        let dt = 1.0 / 30.0;
        let mut ticks = 0;
        while s.exit_blocks_movement(dt) {
            ticks += 1;
            assert!(ticks < 1000, "pending must not block movement forever");
        }
        assert!(
            (ticks as f32) * dt >= RestExit::HANDOFF_SECS - dt,
            "the grace should run out roughly at HANDOFF_SECS, not instantly"
        );
        assert_eq!(s.exit, RestExit::Idle);
    }

    #[test]
    fn resting_again_clears_a_finished_exit() {
        let mut s = RestStance::default();
        s.begin_exit();
        s.observe_exit_clip(true);
        s.kind = RestKind::Heal;
        s.observe_exit_clip(false);
        assert!(!s.exit_blocks_movement(1.0 / 30.0));
    }

    /// KULUU_MOTION_LOG probe: snap-band counting, toggle counting, and rising-edge mismatch
    /// detection.
    #[test]
    fn motion_probe_counts_snap_bands() {
        let mut p = MotionProbe::enabled_for_test();
        for band in [SnapBand::Normal, SnapBand::Stretch, SnapBand::Pop] {
            p.record_update(
                1,
                EntityKind::Mob,
                UpdateOutcome {
                    dt_server: 0.4,
                    jump_sq: 1.0,
                    band,
                    step_yalms: 1.0,
                    segment_duration: 0.5,
                    ring_max: 0.4,
                    speed: 40,
                    speed_base: 40,
                    idle_frames: 12,
                    rem_dist: 0.0,
                },
            );
        }
        let e = &p.per_id[&1];
        assert_eq!((e.updates, e.normals, e.stretches, e.pops), (3, 1, 1, 1));
        assert_eq!(p.all_intervals.len(), 3);
    }

    #[test]
    fn motion_probe_counts_moving_toggles() {
        let mut p = MotionProbe::enabled_for_test();
        p.record_toggle(9, EntityKind::Mob, true, 0.9);
        p.record_toggle(9, EntityKind::Mob, false, 0.3);
        assert_eq!(p.per_id[&9].toggles, 2);
    }

    #[test]
    fn motion_probe_mismatch_is_rising_edge() {
        let mut p = MotionProbe::enabled_for_test();
        p.record_heading_mismatch(3, EntityKind::Mob, 0.0, Vec3::new(0.0, 0.0, 5.0));
        assert_eq!(p.per_id[&3].mismatch_events, 1);
        p.record_heading_mismatch(3, EntityKind::Mob, 0.05, Vec3::new(0.6, 0.0, 4.9));
        assert_eq!(p.per_id[&3].mismatch_events, 1);
        p.record_heading_mismatch(3, EntityKind::Mob, 0.0, Vec3::new(5.0, 0.0, 0.0));
        assert_eq!(p.per_id[&3].mismatch_events, 1);
        p.record_heading_mismatch(3, EntityKind::Mob, 0.0, Vec3::new(0.0, 0.0, 5.0));
        assert_eq!(p.per_id[&3].mismatch_events, 2);
    }

    #[test]
    fn motion_probe_ignores_crawl_speed_for_mismatches() {
        let mut p = MotionProbe::enabled_for_test();
        p.record_heading_mismatch(4, EntityKind::Mob, 0.0, Vec3::new(0.05, 0.0, 0.0));
        assert!(!p.per_id.contains_key(&4), "crawl speed records nothing");
    }

    #[test]
    fn motion_probe_quantiles() {
        let mut samples = std::collections::VecDeque::new();
        for v in [1.0f32, 0.5, 0.75, 0.25, 2.0] {
            samples.push_back(v);
        }
        let (med, p90) = quantiles(&samples);
        assert_eq!(med, Some(0.75));
        assert_eq!(p90, Some(2.0), "five samples: p90 rounds to the top");
    }

    #[test]
    fn combat_run_resolves_with_higher_bone_count_than_casual() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        for skel in [7072u32, 10248, 13424, 16600, 19776, 23176, 26352] {
            let casual = run_anim_for_skel(&root, skel).expect("casual run");
            let combat = combat_run_anim_for_skel(&root, skel).expect("combat run");
            assert!(
                combat.per_bone.len() >= casual.per_bone.len(),
                "skel {skel}: combat run ({}) should have ≥ bones than casual ({})",
                combat.per_bone.len(),
                casual.per_bone.len()
            );
        }
    }
}
