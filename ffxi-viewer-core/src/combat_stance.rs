use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex, OnceLock};

use bevy::prelude::*;
use ffxi_dat::anim::Mo2Animation;
use ffxi_dat::{walk, ChunkKind, DatRoot};

use crate::components::{IsSelf, WorldEntity};
use crate::snapshot::SceneState;
use ffxi_viewer_wire::EntityKind;

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

#[derive(Resource, Default, Debug, Clone, Copy, Eq, PartialEq)]
pub struct RestStance {
    pub kind: RestKind,
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

/// Whether the self character's movement keys are held this tick, written by
/// the client's movement dispatch. While keys are what move the player, the
/// self pose reads this instead of inferring motion from transform deltas:
/// prediction reconcile keeps nudging the rendered transform, so inferred speed
/// can hover above `MOVE_EXIT` and hold the run cycle after the keys are
/// released. Not authoritative while a reactor goal (follow/goto/engage) moves
/// the player with no keys held — the pose falls back to inference there.
#[derive(Resource, Default, Debug, Clone, Copy, Eq, PartialEq)]
pub struct SelfMoveIntent {
    pub moving: bool,
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
    let bytes = fs::read(loc.path_under(root.root())).ok()?;
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
) {
    let dt = time.delta_secs().max(1e-4);

    let heading_by_id: std::collections::HashMap<u32, u8> = state
        .snapshot
        .entities
        .iter()
        .map(|e| (e.id, e.heading))
        .collect();
    for (world, transform) in &q {
        let pos = transform.translation;

        let heading_u8 = heading_by_id.get(&world.id).copied().unwrap_or(0);
        let heading_rad = (heading_u8 as f32) * std::f32::consts::TAU / 256.0;

        let fwd_x = heading_rad.sin();
        let fwd_z = -heading_rad.cos();

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

    pub last_server_pos: Vec3,

    pub dr_velocity: Vec3,

    pub target_heading: u8,

    pub rendered_heading_rad: f32,

    pub secs_since_update: f32,

    pub sample_dirty: bool,

    pub initialized: bool,
}

impl PredictSample {
    fn seed(server_pos: Vec3, heading: u8) -> Self {
        PredictSample {
            rendered_pos: server_pos,
            server_pos,
            last_server_pos: server_pos,
            dr_velocity: Vec3::ZERO,
            target_heading: heading,
            rendered_heading_rad: heading_to_rad(heading),
            secs_since_update: 0.0,
            sample_dirty: false,
            initialized: true,
        }
    }
}

#[derive(Resource, Default)]
pub struct EntityPrediction {
    pub by_id: HashMap<u32, PredictSample>,
}

impl EntityPrediction {
    pub const SNAP_DIST_SQ: f32 = 4.0;

    pub const VEL_BLEND_TAU: f32 = 0.20;

    pub const CORRECT_TAU: f32 = 0.12;

    pub const Y_TAU: f32 = 0.15;

    pub const HEADING_TAU: f32 = 0.10;

    pub const DECEL_TAU: f32 = 0.25;

    pub const STALE_VEL_SECS: f32 = 0.6;

    pub const MAX_DR_SPEED: f32 = 7.0;

    pub const DT_SERVER_CEIL: f32 = 1.0;

    const SAMPLE_EPSILON_SQ: f32 = 1e-4;

    pub fn observe(&mut self, id: u32, server_pos: Vec3, heading: u8) {
        match self.by_id.get_mut(&id) {
            None => {
                self.by_id
                    .insert(id, PredictSample::seed(server_pos, heading));
            }
            Some(e) => {
                if e.server_pos.distance_squared(server_pos) > Self::SAMPLE_EPSILON_SQ {
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

#[inline]
fn exp_approach(from: f32, to: f32, tau: f32, dt: f32) -> f32 {
    let alpha = 1.0 - (-dt / tau.max(1e-4)).exp();
    from + alpha * (to - from)
}

fn advance_prediction(s: &mut PredictSample, dt: f32) -> (Vec3, f32) {
    use std::f32::consts::{PI, TAU};

    if s.sample_dirty {
        s.sample_dirty = false;
        let dt_server = s.secs_since_update;
        let discontinuity = dt_server > EntityPrediction::DT_SERVER_CEIL
            || s.server_pos.distance_squared(s.rendered_pos) >= EntityPrediction::SNAP_DIST_SQ;
        if discontinuity {
            s.rendered_pos = s.server_pos;
            s.dr_velocity = Vec3::ZERO;
        } else {
            let dt_eff = dt_server.max(1e-3);
            let mut v_meas = (s.server_pos - s.last_server_pos) / dt_eff;
            v_meas.y = 0.0;
            let alpha = 1.0 - (-dt_eff / EntityPrediction::VEL_BLEND_TAU).exp();
            s.dr_velocity += (v_meas - s.dr_velocity) * alpha;
            s.dr_velocity.y = 0.0;
            let speed = s.dr_velocity.length();
            if speed > EntityPrediction::MAX_DR_SPEED {
                s.dr_velocity *= EntityPrediction::MAX_DR_SPEED / speed;
            }
        }
        s.last_server_pos = s.server_pos;
        s.secs_since_update = 0.0;
    }

    let mut pos = s.rendered_pos + s.dr_velocity * dt;
    let server_track = s.server_pos + s.dr_velocity * s.secs_since_update;

    pos.x = exp_approach(pos.x, server_track.x, EntityPrediction::CORRECT_TAU, dt);
    pos.z = exp_approach(pos.z, server_track.z, EntityPrediction::CORRECT_TAU, dt);
    pos.y = exp_approach(
        s.rendered_pos.y,
        s.server_pos.y,
        EntityPrediction::Y_TAU,
        dt,
    );
    s.rendered_pos = pos;

    s.secs_since_update += dt;
    if s.secs_since_update > EntityPrediction::STALE_VEL_SECS {
        s.dr_velocity *= (-dt / EntityPrediction::DECEL_TAU).exp();
    }

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
            EntityKind::Mob | EntityKind::Pc | EntityKind::Pet
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
    let Ok(bytes) = fs::read(loc.path_under(root.root())) else {
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
        use crate::ffxi_actor_render::{infers_walk_gait, WALK_RUN_BOUNDARY};
        assert!(EntityMotion::MOVE_EXIT < WALK_RUN_BOUNDARY);
        assert!(
            WALK_RUN_BOUNDARY < 5.0,
            "a base-run actor must NOT be classed as walking"
        );
        assert!(!infers_walk_gait(0.0), "stationary is not walking");
        assert!(infers_walk_gait(1.5), "slow mover walks");
        assert!(!infers_walk_gait(6.0), "runner runs, not walks");
    }

    fn dirty_sample(server: Vec3, prev: Vec3, age: f32) -> PredictSample {
        PredictSample {
            rendered_pos: prev,
            server_pos: server,
            last_server_pos: prev,
            dr_velocity: Vec3::ZERO,
            target_heading: 0,
            rendered_heading_rad: 0.0,
            secs_since_update: age,
            sample_dirty: true,
            initialized: true,
        }
    }

    #[test]
    fn prediction_ingest_seeds_velocity_from_displacement() {
        let mut s = dirty_sample(Vec3::new(1.0, 0.0, 0.0), Vec3::ZERO, 0.2);
        advance_prediction(&mut s, 1.0 / 30.0);
        assert!(
            s.dr_velocity.x > 0.5,
            "x velocity seeded: {}",
            s.dr_velocity.x
        );
        assert!(s.dr_velocity.y.abs() < 1e-6 && s.dr_velocity.z.abs() < 1e-6);
        assert!(!s.sample_dirty, "sample consumed by ingest");
    }

    #[test]
    fn prediction_clamps_velocity_to_max() {
        let mut s = dirty_sample(Vec3::new(100.0, 0.0, 0.0), Vec3::ZERO, 0.1);
        let (pos, _) = advance_prediction(&mut s, 1.0 / 30.0);
        assert!(s.dr_velocity.length() < 1e-6, "snap zeroes velocity");
        assert!(
            (pos.x - 100.0).abs() < 0.5,
            "snapped onto server: {}",
            pos.x
        );
    }

    #[test]
    fn prediction_extrapolates_between_updates() {
        let mut s = PredictSample::seed(Vec3::ZERO, 0);
        s.dr_velocity = Vec3::new(5.0, 0.0, 0.0);
        let start = s.rendered_pos.x;
        for _ in 0..10 {
            advance_prediction(&mut s, 1.0 / 30.0);
        }
        assert!(
            s.rendered_pos.x > start + 0.5,
            "dead-reckons forward between updates: {} -> {}",
            start,
            s.rendered_pos.x
        );
    }

    #[test]
    fn prediction_velocity_decays_when_stale() {
        let mut s = PredictSample::seed(Vec3::ZERO, 0);
        s.dr_velocity = Vec3::new(5.0, 0.0, 0.0);
        for _ in 0..120 {
            advance_prediction(&mut s, 1.0 / 30.0);
        }
        assert!(
            s.dr_velocity.length() < 0.5,
            "stale velocity coasts to a stop: {}",
            s.dr_velocity.length()
        );
    }

    #[test]
    fn prediction_static_actor_does_not_drift() {
        let anchor = Vec3::new(3.0, 1.0, 2.0);
        let mut s = PredictSample::seed(anchor, 64);
        for _ in 0..60 {
            advance_prediction(&mut s, 1.0 / 30.0);
        }
        assert!(
            (s.rendered_pos - anchor).length() < 0.05,
            "stays put: {:?}",
            s.rendered_pos
        );
        assert!(s.dr_velocity.length() < 1e-3);
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
