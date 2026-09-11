use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex, OnceLock};

use bevy::prelude::*;
use ffxi_dat::anim::Mo2Animation;
use ffxi_dat::{walk, ChunkKind, DatRoot};

use crate::components::{IsSelf, WorldEntity};
use crate::snapshot::SceneState;
use kuluu_snapshot::EntityKind;

pub fn motion_dat_for_skel(skel_file_id: u32) -> Option<u32> {
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

static BATTLE_IDLE_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> =
    OnceLock::new();

static RUN_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> = OnceLock::new();

static SIT_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> = OnceLock::new();
static HEAL_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> = OnceLock::new();

static COMBAT_RUN_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> = OnceLock::new();

static DIRECTIONAL_ANIMS: OnceLock<Mutex<HashMap<(u32, [u8; 3]), Option<Arc<Mo2Animation>>>>> =
    OnceLock::new();

const BATTLE_IDLE_PREFIX: &[u8; 3] = b"btl";

pub fn battle_idle_anim_for_skel(skel_file_id: u32) -> Option<Arc<Mo2Animation>> {
    let motion_dat = motion_dat_for_skel(skel_file_id)?;
    let map = BATTLE_IDLE_ANIMS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().ok()?;
    if let Some(entry) = guard.get(&motion_dat) {
        return entry.clone();
    }
    let loaded = load_battle_idle(motion_dat).map(Arc::new);
    guard.insert(motion_dat, loaded.clone());
    loaded
}

fn load_battle_idle(motion_dat_id: u32) -> Option<Mo2Animation> {
    load_anim_with_prefix(motion_dat_id, BATTLE_IDLE_PREFIX)
}

pub fn run_anim_for_skel(skel_file_id: u32) -> Option<Arc<Mo2Animation>> {
    let map = RUN_ANIMS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().ok()?;
    if let Some(entry) = guard.get(&skel_file_id) {
        return entry.clone();
    }
    let loaded = load_anim_with_prefix(skel_file_id, b"run").map(Arc::new);
    guard.insert(skel_file_id, loaded.clone());
    loaded
}

pub fn combat_run_anim_for_skel(skel_file_id: u32) -> Option<Arc<Mo2Animation>> {
    let motion_dat = motion_dat_for_skel(skel_file_id)?;
    let map = COMBAT_RUN_ANIMS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().ok()?;
    if let Some(entry) = guard.get(&motion_dat) {
        return entry.clone();
    }
    let loaded = load_anim_with_prefix(motion_dat, b"run").map(Arc::new);
    guard.insert(motion_dat, loaded.clone());
    loaded
}

pub fn sit_anim_for_skel(skel_file_id: u32) -> Option<Arc<Mo2Animation>> {
    let map = SIT_ANIMS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().ok()?;
    if let Some(entry) = guard.get(&skel_file_id) {
        return entry.clone();
    }
    let loaded = load_anim_with_prefix(skel_file_id, b"sit").map(Arc::new);
    guard.insert(skel_file_id, loaded.clone());
    loaded
}

pub fn heal_anim_for_skel(skel_file_id: u32) -> Option<Arc<Mo2Animation>> {
    let map = HEAL_ANIMS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().ok()?;
    if let Some(entry) = guard.get(&skel_file_id) {
        return entry.clone();
    }
    let loaded = load_anim_with_prefix(skel_file_id, b"hea").map(Arc::new);
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
    pub const WALK_SCALE: f32 = 0.25;

    pub fn scale(self) -> f32 {
        if self.walking {
            Self::WALK_SCALE
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
}

impl SelfMoveIntent {
    pub fn walking(&self, manual_walk: bool) -> bool {
        self.scripted_speed
            .map(infers_walk_gait)
            .unwrap_or(manual_walk)
    }
}

pub fn directional_anim_for_skel(skel_file_id: u32, prefix: &[u8; 3]) -> Option<Arc<Mo2Animation>> {
    let map = DIRECTIONAL_ANIMS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().ok()?;
    let key = (skel_file_id, *prefix);
    if let Some(entry) = guard.get(&key) {
        return entry.clone();
    }
    let loaded = load_anim_with_prefix(skel_file_id, prefix).map(Arc::new);
    guard.insert(key, loaded.clone());
    loaded
}

pub fn load_anim_with_prefix(file_id: u32, prefix: &[u8; 3]) -> Option<Mo2Animation> {
    let root = DatRoot::from_env_or_default().ok()?;
    let loc = root.resolve(file_id).ok()?;
    let bytes = fs::read(loc.path_under(&root)).ok()?;
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
    mut motion: ResMut<EntityMotion>,
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

        let heading_u8 = heading_by_id.get(&world.id).copied().unwrap_or(0);
        let heading_rad = heading_to_rad(heading_u8);

        let fwd = heading_forward(heading_u8);
        let (fwd_x, fwd_z) = (fwd.x, fwd.z);

        let right_x = fwd_z;
        let right_z = -fwd_x;

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
                moving: EntityMotion::apply_move_hysteresis(prev.moving, speed),
            },
        );
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PredictSample {
    pub rendered_pos: Vec3,

    pub server_pos: Vec3,

    pub target_heading: u8,

    pub rendered_heading_rad: f32,

    pub sample_dirty: bool,

    pub initialized: bool,

    sample_age: f32,
    sample_intervals: [f32; EntityPrediction::JITTER_HISTORY_SAMPLES],
    segment_elapsed: f32,
    segment_duration: f32,
    snap_pending: bool,
}

impl PredictSample {
    fn seed(server_pos: Vec3, heading: u8) -> Self {
        PredictSample {
            rendered_pos: server_pos,
            server_pos,
            target_heading: heading,
            rendered_heading_rad: heading_to_rad(heading),
            sample_dirty: false,
            initialized: true,
            sample_age: 0.0,
            sample_intervals: [EntityPrediction::DEFAULT_INTERVAL;
                EntityPrediction::JITTER_HISTORY_SAMPLES],
            segment_elapsed: 0.0,
            segment_duration: EntityPrediction::DEFAULT_INTERVAL
                * EntityPrediction::INTERVAL_HEADROOM,
            snap_pending: false,
        }
    }
}

#[derive(Resource, Default)]
pub struct EntityPrediction {
    pub by_id: HashMap<u32, PredictSample>,
}

impl EntityPrediction {
    pub const SNAP_DIST_SQ: f32 = 4.0;

    // Engineering bounds: cover sparse movement packets without replaying a long idle as movement.
    const DEFAULT_INTERVAL: f32 = 0.2;
    const MIN_INTERVAL: f32 = 0.1;
    const MAX_INTERVAL: f32 = 1.0;
    const IDLE_INTERVAL_MULTIPLIER: f32 = 2.0;

    // A small buffer keeps packet-arrival jitter from ending each run segment early.
    const INTERVAL_HEADROOM: f32 = 1.25;

    // Short arrival bursts must not immediately erase a recent long gap from the buffer budget.
    const JITTER_HISTORY_SAMPLES: usize = 8;

    // Render lag is not a teleport; only large changes between confirmed positions bypass tweening.
    const TELEPORT_DIST_SQ: f32 = 20.0 * 20.0;

    pub const HEADING_TAU: f32 = 0.10;

    const SAMPLE_EPSILON_SQ: f32 = 1e-4;

    pub fn observe(&mut self, id: u32, server_pos: Vec3, heading: u8) {
        match self.by_id.get_mut(&id) {
            None => {
                self.by_id
                    .insert(id, PredictSample::seed(server_pos, heading));
            }
            Some(e) => {
                if e.server_pos.distance_squared(server_pos) > Self::SAMPLE_EPSILON_SQ {
                    e.snap_pending |=
                        e.server_pos.distance_squared(server_pos) >= Self::TELEPORT_DIST_SQ;
                    let idle_interval = Self::MAX_INTERVAL * Self::IDLE_INTERVAL_MULTIPLIER;
                    if e.sample_age > 0.0 && e.sample_age <= idle_interval {
                        let interval = e.sample_age.clamp(Self::MIN_INTERVAL, Self::MAX_INTERVAL);
                        e.sample_intervals.rotate_left(1);
                        e.sample_intervals[Self::JITTER_HISTORY_SAMPLES - 1] = interval;
                        e.segment_duration = e.sample_intervals.iter().copied().fold(0.0, f32::max)
                            * Self::INTERVAL_HEADROOM;
                    }
                    e.sample_age = 0.0;
                    e.segment_elapsed = 0.0;
                    e.server_pos = server_pos;
                    e.sample_dirty = true;
                }
                e.target_heading = heading;
            }
        }
    }
}

#[inline]
fn heading_to_rad(heading: u8) -> f32 {
    (heading as f32) * std::f32::consts::TAU / 256.0
}

/// World-space direction an entity with this heading faces. The one place the
/// `(sin, -cos)` pairing lives — the motion basis below and the fishing water
/// probe both come through here rather than re-deriving it.
#[inline]
pub fn heading_forward(heading: u8) -> Vec3 {
    let rad = heading_to_rad(heading);
    Vec3::new(rad.sin(), 0.0, -rad.cos())
}

#[inline]
fn advance_prediction(s: &mut PredictSample, dt: f32) -> (Vec3, f32) {
    use std::f32::consts::{PI, TAU};

    s.sample_age += dt;
    s.sample_dirty = false;
    if s.snap_pending {
        s.snap_pending = false;
        s.rendered_pos = s.server_pos;
    }
    let remaining = s.segment_duration - s.segment_elapsed;
    if remaining > dt {
        s.rendered_pos += (s.server_pos - s.rendered_pos) * (dt / remaining);
    } else {
        s.rendered_pos = s.server_pos;
    }
    s.segment_elapsed = (s.segment_elapsed + dt).min(s.segment_duration);

    let target = heading_to_rad(s.target_heading);
    let mut dh = target - s.rendered_heading_rad;
    dh = dh.rem_euclid(TAU);
    if dh > PI {
        dh -= TAU;
    }
    let alpha_h = 1.0 - (-dt / EntityPrediction::HEADING_TAU).exp();
    s.rendered_heading_rad += dh * alpha_h;

    (s.rendered_pos, s.rendered_heading_rad)
}

pub fn predict_entities_system(
    time: Res<Time>,
    mut prediction: ResMut<EntityPrediction>,
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
        let Some(sample) = prediction.by_id.get_mut(&world.id) else {
            continue;
        };
        if !sample.initialized {
            continue;
        }
        let (pos, heading_rad) = advance_prediction(sample, dt);
        transform.translation = pos;
        transform.rotation = Quat::from_rotation_y(-heading_rad);
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

pub fn enumerate_clips_for_skel(skel_file_id: u32) -> Vec<(String, Arc<Mo2Animation>)> {
    let mut out = Vec::new();
    let mut sources: Vec<u32> = vec![skel_file_id];
    if let Some(motion) = motion_dat_for_skel(skel_file_id) {
        sources.push(motion);
    }
    let mut seen = std::collections::HashSet::<String>::new();
    for file_id in sources {
        for_each_anim_chunk_in_dat(file_id, |name, anim| {
            if seen.insert(name.clone()) {
                out.push((name, Arc::new(anim)));
            }
        });
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub fn override_anim_for_skel(skel_file_id: u32, prefix: &[u8; 3]) -> Option<Arc<Mo2Animation>> {
    if let Some(a) = load_anim_with_prefix(skel_file_id, prefix) {
        return Some(Arc::new(a));
    }
    let motion = motion_dat_for_skel(skel_file_id)?;
    load_anim_with_prefix(motion, prefix).map(Arc::new)
}

fn for_each_anim_chunk_in_dat(file_id: u32, mut f: impl FnMut(String, Mo2Animation)) {
    let Ok(root) = DatRoot::from_env_or_default() else {
        return;
    };
    let Ok(loc) = root.resolve(file_id) else {
        return;
    };
    let Ok(bytes) = fs::read(loc.path_under(&root)) else {
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
    fn motion_dat_resolves_for_each_pc_race() {
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
                motion_dat_for_skel(skel),
                Some(motion),
                "skel {skel} should map to motion {motion}"
            );
        }
    }

    #[test]
    fn motion_dat_returns_none_for_non_pc_skel() {
        assert_eq!(motion_dat_for_skel(0), None);
        assert_eq!(motion_dat_for_skel(7000), None);
        assert_eq!(motion_dat_for_skel(50000), None);
    }

    #[test]
    fn motion_dat_offset_is_consistent() {
        for skel in [7072u32, 10248, 13424, 16600, 19776, 23176, 26352] {
            let motion = motion_dat_for_skel(skel).expect("PC race");
            assert_eq!(
                motion - skel,
                2600,
                "skel {skel} → motion {motion}: offset must be +2600"
            );
        }
    }

    #[test]
    fn battle_idle_resolves_for_every_pc_race_when_dats_available() {
        if DatRoot::from_env_or_default().is_err() {
            eprintln!("skipping: no retail DAT root");
            return;
        }
        for skel in [7072u32, 10248, 13424, 16600, 19776, 23176, 26352] {
            let anim = battle_idle_anim_for_skel(skel).expect("battle-idle MO2 missing for skel");
            assert!(
                anim.frames > 0,
                "skel {skel}: btl MO2 has zero frames — parse drift?"
            );
        }
    }

    #[test]
    fn run_anim_resolves_for_every_pc_race_when_dats_available() {
        if DatRoot::from_env_or_default().is_err() {
            eprintln!("skipping: no retail DAT root");
            return;
        }
        for skel in [7072u32, 10248, 13424, 16600, 19776, 23176, 26352] {
            let anim = run_anim_for_skel(skel).expect("casual run MO2 missing for skel");
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

    #[test]
    fn remote_motion_at_five_hz_stops_without_overshoot_and_returns_to_idle() {
        const FRAME_SECS: f32 = 1.0 / 60.0;
        const FRAMES_PER_UPDATE: usize = 12;
        const RUN_SPEED: f32 = 4.8;
        const RUN_FRAMES: usize = 120;
        const STOP_FRAMES: usize = 180;
        const ENTITY_ID: u32 = 7;
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<SceneState>()
            .init_resource::<EntityPrediction>()
            .init_resource::<EntityMotion>()
            .add_systems(
                Update,
                (predict_entities_system, track_entity_motion_system).chain(),
            );
        let actor = app
            .world_mut()
            .spawn((
                WorldEntity {
                    id: ENTITY_ID,
                    act_index: 1,
                    kind: EntityKind::Pc,
                },
                Transform::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<EntityPrediction>()
            .observe(ENTITY_ID, Vec3::ZERO, 0);
        let mut last_confirmed_x = 0.0;
        let mut previous_x = 0.0;
        for frame in 0..(RUN_FRAMES + STOP_FRAMES) {
            if frame < RUN_FRAMES && frame % FRAMES_PER_UPDATE == 0 {
                last_confirmed_x = frame as f32 * FRAME_SECS * RUN_SPEED;
                app.world_mut().resource_mut::<EntityPrediction>().observe(
                    ENTITY_ID,
                    Vec3::X * last_confirmed_x,
                    0,
                );
            }
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(FRAME_SECS));
            app.update();
            let x = app.world().get::<Transform>(actor).unwrap().translation.x;
            assert!(x >= previous_x && x <= last_confirmed_x);
            previous_x = x;
            if frame == RUN_FRAMES - 1 {
                assert!(app.world().resource::<EntityMotion>().is_moving(ENTITY_ID));
            }
        }
        assert!((previous_x - last_confirmed_x).abs() < 1e-5);
        assert!(!app.world().resource::<EntityMotion>().is_moving(ENTITY_ID));
    }

    #[test]
    fn remote_running_tweens_sparse_updates_without_changing_gait() {
        use ffxi_actor::actor_state::{selected_animation, ActorAnimInputs};
        const FRAME_SECS: f32 = 1.0 / 60.0;
        const UPDATE_COUNT: usize = 8;
        const WARMUP_UPDATES: usize = 3;
        const MAX_SPEED_MULTIPLIER: f32 = 1.5;
        for (update_frames, jitter_frames) in
            [(12, 0), (30, 0), (60, 0), (12, 2), (30, 6), (60, 12)]
        {
            for speed in [1.5, 4.8, 6.0] {
                let mut app = App::new();
                app.init_resource::<Time>()
                    .init_resource::<SceneState>()
                    .init_resource::<EntityPrediction>()
                    .init_resource::<EntityMotion>()
                    .add_systems(
                        Update,
                        (predict_entities_system, track_entity_motion_system).chain(),
                    );
                let entity = app
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
                app.world_mut()
                    .resource_mut::<EntityPrediction>()
                    .observe(7, Vec3::ZERO, 0);
                let expected_clip = selected_animation(&ActorAnimInputs {
                    moving: true,
                    walking: infers_walk_gait(speed),
                    ..Default::default()
                })
                .id;
                let mut previous = 0.0;
                let mut confirmed = 0.0;
                let mut next_update = 0;
                let mut update_count = 0;
                for frame in 0..update_frames * UPDATE_COUNT {
                    if frame == next_update {
                        next_update += if update_count % 2 == 0 {
                            update_frames + jitter_frames
                        } else {
                            update_frames - jitter_frames
                        };
                        update_count += 1;
                        confirmed = frame as f32 * FRAME_SECS * speed;
                        app.world_mut().resource_mut::<EntityPrediction>().observe(
                            7,
                            Vec3::X * confirmed,
                            0,
                        );
                    }
                    app.world_mut()
                        .resource_mut::<Time>()
                        .advance_by(std::time::Duration::from_secs_f32(FRAME_SECS));
                    app.update();
                    let x = app.world().get::<Transform>(entity).unwrap().translation.x;
                    assert!(x >= previous && x <= confirmed);
                    assert!(
                        x - previous <= speed * FRAME_SECS * MAX_SPEED_MULTIPLIER,
                        "cadence={update_frames} speed={speed} frame={frame}: jumped {}",
                        x - previous
                    );
                    previous = x;
                    if frame >= update_frames * WARMUP_UPDATES {
                        let motion = app.world().resource::<EntityMotion>().sample(7).unwrap();
                        let clip = selected_animation(&ActorAnimInputs {
                            moving: motion.moving,
                            walking: infers_walk_gait(motion.speed),
                            ..Default::default()
                        })
                        .id;
                        assert_eq!(
                            clip, expected_clip,
                            "cadence={update_frames} speed={speed} frame={frame}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn remote_running_keeps_gait_across_captured_lsb_arrival_jitter() {
        const FRAME_SECS: f32 = 1.0 / 60.0;
        const PACKET_STEP: f32 = 2.4;
        const RUN_SPEED: f32 = 4.8;
        const MAX_SPEED_MULTIPLIER: f32 = 2.0;
        const WARMUP_FRAMES: usize = 180;
        const STOP_FRAMES: usize = 180;
        // Arrival jitter changes timing without changing the sender's half-second position steps.
        const ARRIVAL_FRAMES: [usize; 10] = [24, 48, 66, 114, 138, 162, 192, 234, 264, 288];
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<SceneState>()
            .init_resource::<EntityPrediction>()
            .init_resource::<EntityMotion>()
            .add_systems(
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
        app.world_mut()
            .resource_mut::<EntityPrediction>()
            .observe(7, Vec3::ZERO, 0);
        let mut packet = 0;
        let mut confirmed = 0.0;
        let mut previous = 0.0;
        let last_arrival = *ARRIVAL_FRAMES.last().unwrap();
        for frame in 0..last_arrival + STOP_FRAMES {
            if ARRIVAL_FRAMES.get(packet) == Some(&frame) {
                packet += 1;
                confirmed = packet as f32 * PACKET_STEP;
                app.world_mut().resource_mut::<EntityPrediction>().observe(
                    7,
                    Vec3::X * confirmed,
                    0,
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
    fn remote_motion_stops_at_confirmed_position_during_packet_silence() {
        let mut prediction = EntityPrediction::default();
        prediction.observe(7, Vec3::ZERO, 0);
        prediction.observe(7, Vec3::X, 0);
        let sample = prediction.by_id.get_mut(&7).unwrap();
        let mut previous = sample.rendered_pos.x;
        for _ in 0..240 {
            let (position, _) = advance_prediction(sample, 1.0 / 60.0);
            assert!(position.x >= previous && position.x <= 1.0);
            previous = position.x;
        }
        assert!((sample.rendered_pos - Vec3::X).length() < 1e-5);
    }

    #[test]
    fn remote_motion_repeated_snapshots_do_not_restart_smoothing() {
        let mut prediction = EntityPrediction::default();
        prediction.observe(7, Vec3::ZERO, 0);
        prediction.observe(7, Vec3::X, 0);
        let mut silence = prediction.by_id[&7];
        for _ in 0..120 {
            prediction.observe(7, Vec3::X, 0);
            advance_prediction(prediction.by_id.get_mut(&7).unwrap(), 1.0 / 60.0);
            advance_prediction(&mut silence, 1.0 / 60.0);
            assert_eq!(prediction.by_id[&7].rendered_pos, silence.rendered_pos);
        }
    }

    #[test]
    fn remote_motion_turn_does_not_carry_old_velocity_past_the_corner() {
        let mut prediction = EntityPrediction::default();
        prediction.observe(7, Vec3::ZERO, 0);
        prediction.observe(7, Vec3::X, 0);
        for _ in 0..12 {
            advance_prediction(prediction.by_id.get_mut(&7).unwrap(), 1.0 / 60.0);
        }
        let corner = Vec3::new(1.0, 0.0, 1.0);
        prediction.observe(7, corner, 64);
        for _ in 0..120 {
            let (position, _) =
                advance_prediction(prediction.by_id.get_mut(&7).unwrap(), 1.0 / 60.0);
            assert!(position.x <= corner.x);
            assert!((0.0..=corner.z).contains(&position.z));
        }
        assert!((prediction.by_id[&7].rendered_pos - corner).length() < 1e-5);
    }

    #[test]
    fn remote_motion_jitter_and_missing_updates_stay_between_confirmed_endpoints() {
        const FRAME_SECS: f32 = 1.0 / 60.0;
        const RUN_SPEED: f32 = 4.8;
        let mut prediction = EntityPrediction::default();
        prediction.observe(7, Vec3::ZERO, 0);
        let mut elapsed = 0.0;
        for frames in [12, 9, 15, 24, 6, 18] {
            let target = Vec3::X * elapsed * RUN_SPEED;
            prediction.observe(7, target, 0);
            let sample = prediction.by_id.get_mut(&7).unwrap();
            let mut previous = sample.rendered_pos.x;
            for _ in 0..frames {
                let (position, _) = advance_prediction(sample, FRAME_SECS);
                assert!(position.x >= previous && position.x <= target.x);
                previous = position.x;
            }
            elapsed += frames as f32 * FRAME_SECS;
        }
    }

    #[test]
    fn remote_motion_reverses_toward_the_confirmed_position_without_coasting() {
        let mut prediction = EntityPrediction::default();
        prediction.observe(7, Vec3::ZERO, 0);
        prediction.observe(7, Vec3::X, 0);
        for _ in 0..12 {
            advance_prediction(prediction.by_id.get_mut(&7).unwrap(), 1.0 / 60.0);
        }
        let before_turn = prediction.by_id[&7].rendered_pos.x;
        prediction.observe(7, Vec3::ZERO, 128);
        let (position, _) = advance_prediction(prediction.by_id.get_mut(&7).unwrap(), 1.0 / 60.0);
        assert!(position.x < before_turn && position.x >= 0.0);
    }

    #[test]
    fn remote_motion_resumes_smoothly_after_a_long_stationary_period() {
        let mut prediction = EntityPrediction::default();
        prediction.observe(7, Vec3::ZERO, 0);
        for _ in 0..240 {
            advance_prediction(prediction.by_id.get_mut(&7).unwrap(), 1.0 / 60.0);
        }
        prediction.observe(7, Vec3::X, 0);
        let (position, _) = advance_prediction(prediction.by_id.get_mut(&7).unwrap(), 1.0 / 60.0);
        assert!(position.x > 0.0 && position.x < 1.0);
    }

    #[test]
    fn remote_motion_teleport_snaps_and_does_not_drift() {
        let mut prediction = EntityPrediction::default();
        prediction.observe(7, Vec3::ZERO, 0);
        let destination = Vec3::splat(100.0);
        prediction.observe(7, destination, 0);
        for _ in 0..120 {
            let (position, _) =
                advance_prediction(prediction.by_id.get_mut(&7).unwrap(), 1.0 / 60.0);
            assert_eq!(position, destination);
        }
    }

    #[test]
    fn remote_motion_smoothing_is_frame_rate_independent() {
        let mut prediction = EntityPrediction::default();
        prediction.observe(7, Vec3::ZERO, 0);
        prediction.observe(7, Vec3::ONE, 64);
        let mut slow = prediction.by_id[&7];
        let mut fast = slow;
        for _ in 0..30 {
            advance_prediction(&mut slow, 1.0 / 30.0);
        }
        for _ in 0..144 {
            advance_prediction(&mut fast, 1.0 / 144.0);
        }
        assert!((slow.rendered_pos - fast.rendered_pos).length() < 1e-5);
        assert!((slow.rendered_heading_rad - fast.rendered_heading_rad).abs() < 1e-5);
    }

    #[test]
    fn remote_motion_stationary_heading_change_does_not_translate() {
        let mut prediction = EntityPrediction::default();
        let anchor = Vec3::new(3.0, 1.0, 2.0);
        prediction.observe(7, anchor, 0);
        prediction.observe(7, anchor, 64);
        let sample = prediction.by_id.get_mut(&7).unwrap();
        for _ in 0..60 {
            advance_prediction(sample, 1.0 / 30.0);
            assert_eq!(sample.rendered_pos, anchor);
        }
        assert!((sample.rendered_heading_rad - heading_to_rad(64)).abs() < 1e-5);
    }

    #[test]
    fn observe_seeds_then_flags_only_on_real_move() {
        let mut p = EntityPrediction::default();
        p.observe(7, Vec3::new(1.0, 0.0, 0.0), 10);
        let s = p.by_id[&7];
        assert!(
            s.initialized && !s.sample_dirty,
            "first sight seeds, not dirty"
        );
        assert_eq!(s.rendered_pos, Vec3::new(1.0, 0.0, 0.0));

        p.observe(7, Vec3::new(1.0, 0.0, 0.0), 10);
        assert!(
            !p.by_id[&7].sample_dirty,
            "unchanged position must not re-ingest"
        );

        p.observe(7, Vec3::new(2.0, 0.0, 0.0), 20);
        assert!(p.by_id[&7].sample_dirty, "moved position raises dirty");
        assert_eq!(p.by_id[&7].target_heading, 20);
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

    #[test]
    fn combat_run_resolves_with_higher_bone_count_than_casual() {
        if DatRoot::from_env_or_default().is_err() {
            eprintln!("skipping: no retail DAT root");
            return;
        }
        for skel in [7072u32, 10248, 13424, 16600, 19776, 23176, 26352] {
            let casual = run_anim_for_skel(skel).expect("casual run");
            let combat = combat_run_anim_for_skel(skel).expect("combat run");
            assert!(
                combat.per_bone.len() >= casual.per_bone.len(),
                "skel {skel}: combat run ({}) should have ≥ bones than casual ({})",
                combat.per_bone.len(),
                casual.per_bone.len()
            );
        }
    }
}
