//! Combat-idle MO2 lookup for engaged PCs.
//!
//! When a PC has `bt_target_id != 0` on the wire (auto-attacking
//! something) the avatar should switch from the resting idle MO2 to a
//! battle-idle MO2 so the operator can read "in combat" from the
//! avatar alone — same signal the red engaged-ring uses, but on the
//! model itself.
//!
//! ## Where the battle MO2s live
//!
//! Each PC race ships with a **separate motion DAT** that sits
//! `+2600` ids past its skeleton DAT, holding the combat-stance
//! animation set. Citation: `vendor/lotus-ffxi/ffxi/entity/actor_data.cppm:23-30`:
//!
//! ```text
//! PCSkeletonIDs{ .skel =  7072, .motion =  9672, ... }  // Hume M
//! PCSkeletonIDs{ .skel = 10248, .motion = 12848, ... }  // Hume F
//! PCSkeletonIDs{ .skel = 13424, .motion = 16024, ... }  // Elv  M
//! PCSkeletonIDs{ .skel = 16600, .motion = 19200, ... }  // Elv  F
//! PCSkeletonIDs{ .skel = 19776, .motion = 22376, ... }  // Taru M
//! PCSkeletonIDs{ .skel = 19776, .motion = 22376, ... }  // Taru F (shares Taru M)
//! PCSkeletonIDs{ .skel = 23176, .motion = 25776, ... }  // Mithra
//! PCSkeletonIDs{ .skel = 26352, .motion = 28952, ... }  // Galka
//! ```
//!
//! Lotus then loads `battle_animation_size = 8` consecutive motion
//! DATs starting at `.motion` (one per weapon class). We don't have
//! weapon class on the wire today, so we use index 0 — the unarmed /
//! base battle stance — as the engaged-state animation.
//!
//! ## Why this is in its own module
//!
//! Per [[pc_gpu_skinning_blockers]] the skinned-actor surface has
//! historically been fragile. Keeping the combat-stance code in a
//! separate file with a narrow public surface (one function call from
//! `dat_vos2::tick_skinned_actors`) makes the experimental commit
//! cleanly revertable if it regresses PC rendering.

use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex, OnceLock};

use bevy::prelude::*;
use ffxi_dat::anim::Mo2Animation;
use ffxi_dat::{walk, ChunkKind, DatRoot};

use crate::components::WorldEntity;
use crate::snapshot::SceneState;

/// Reverse-map a PC skeleton DAT id to its motion DAT id (battle
/// animation set #0 — unarmed).
///
/// Returns `None` for non-PC skeletons (NPCs, mobs, beastman races
/// outside the PC table); those keep the existing idle-only path.
///
/// The mapping is **identity + 2600** for every PC race in lotus's
/// `PCSkeletonIDs` table; we still keep an explicit `match` rather
/// than computing it because the +2600 invariant is incidental to
/// data layout, not guaranteed by SE, and citing each row makes
/// drift easy to spot if a future retail patch reorganizes the
/// archive.
pub fn motion_dat_for_skel(skel_file_id: u32) -> Option<u32> {
    match skel_file_id {
        7072 => Some(9672),  // Hume M
        10248 => Some(12848), // Hume F
        13424 => Some(16024), // Elvaan M
        16600 => Some(19200), // Elvaan F
        19776 => Some(22376), // Taru M & Taru F share file 19776 → same motion DAT
        23176 => Some(25776), // Mithra
        26352 => Some(28952), // Galka
        _ => None,
    }
}

/// Same shape as `dat_vos2::IDLE_ANIMS` — keyed by motion DAT id so
/// the two Taru variants and any future shared-skel races land on the
/// same cache slot.
static BATTLE_IDLE_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> =
    OnceLock::new();

/// Cache for the casual run animation in the skeleton DAT (`run0`,
/// 16-bone LOD). Keyed by skeleton DAT id; not all skeletons have a
/// run anim (NPCs that never relocate) so we cache the `None` result
/// too — the existence check is the load itself.
static RUN_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> = OnceLock::new();

/// Cache for the resting `sit` / `hea` animations. Keyed by skeleton
/// DAT id; both clips live in the skeleton DAT on PC races (sit/hea
/// are not weapon-class-specific, so they don't have motion-DAT
/// variants). `None` is cached for skeletons that don't ship the
/// clip — NPC skels routinely lack `hea` even when they have `sit`,
/// so the lookup must be independent per clip.
static SIT_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> = OnceLock::new();
static HEAL_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> = OnceLock::new();

/// Cache for the combat run animation in the motion DAT (`run1`,
/// 68-bone full rig). PC-only; keyed by motion DAT id like
/// [`BATTLE_IDLE_ANIMS`].
static COMBAT_RUN_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> =
    OnceLock::new();

/// Cache for directional locomotion variants resolved by 3-char prefix
/// against the skeleton DAT. Key is `(skel_file_id, prefix)`.
///
/// Probed prefixes (see [`directional_anim_for_skel`]):
///   - `bck` — backpedal. Absent from every PC skeleton DAT we probed;
///     callers fall back to `run` at negative time-scale.
///   - `stl` / `str` — strafe-left / strafe-right. Absent from PC
///     skeletons; retail does *not* ship strafe clips and the engine
///     just plays `run` while the camera-relative input picks the
///     direction. We probe them anyway in case beastman/NPC skels
///     carry them; if they don't, the cached `None` makes lookups O(1).
///   - `trn` — turn-in-place. Absent from PC skeletons; retail uses
///     the idle pose with a yaw-only delta. Probed for NPC skeletons
///     that may carry it (some ranger NPCs do).
///   - `wlk` — walk. Present on most PC skeletons as `wlk0`; used
///     for sub-base-run speeds (e.g. shadow walk, /walk toggle).
///
/// Cache entries are keyed by `(file_id, [u8; 3])` rather than a
/// string so we don't pay a `String` allocation per query.
static DIRECTIONAL_ANIMS: OnceLock<Mutex<HashMap<(u32, [u8; 3]), Option<Arc<Mo2Animation>>>>> =
    OnceLock::new();

/// 3-char MO2 name prefix for the battle-idle pose inside a motion
/// DAT. Discovered by running `bin/dump-motion-dat 9672` against the
/// Hume M retail archive — the motion DAT carries `btl0` (16-bone
/// LOD) and `btl1` (68-bone LOD), but no `idl*` chunk; resting idle
/// lives in the *skeleton* DAT only. Lotus's
/// `actor_skeleton_static.cpp` loads the entire motion DAT into a
/// name-keyed map and picks the right pose by string, so the
/// 4-char name (`btl0`/`btl1`) acts as the protocol-level handle.
const BATTLE_IDLE_PREFIX: &[u8; 3] = b"btl";

/// Load and cache the battle-idle MO2 for a PC skeleton.
///
/// Returns `None` when the skeleton isn't a PC race (no motion DAT
/// in lotus's table), the DAT file can't be opened (DAT root unset,
/// retail archive missing), or no `btl`-named MO2 chunk exists in
/// the motion DAT.
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

/// Scan a motion DAT for the first MO2 chunk whose 3-char name prefix
/// is `btl`. Mirrors `dat_vos2::load_idle_animation_for_file` shape
/// but lives here so the combat-stance commit can be reverted
/// independently.
fn load_battle_idle(motion_dat_id: u32) -> Option<Mo2Animation> {
    load_anim_with_prefix(motion_dat_id, BATTLE_IDLE_PREFIX)
}

/// Load the casual (non-combat) run animation from a *skeleton* DAT.
/// Lotus's classic-input `playAnimationLoop("run", speed)` resolves
/// against the skeleton DAT's animation map
/// (`actor_skeleton_static.cpp:86-108`), which is where `run0` lives
/// for PCs (16-bone LOD). NPC skeleton DATs also carry `run` when
/// they're meant to relocate; non-relocating NPCs return `None` and
/// the caller stays on idle.
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

/// Load the combat (battle-aware) run animation from the PC race's
/// motion DAT. This is `run1` (68-bone full rig) — lotus picks it
/// via `battle_animations[index]` in
/// `actor_skeleton_static.cpp:205-208` when the actor is engaged.
///
/// Returns `None` for non-PC skeletons (NPCs etc.); caller should
/// fall back to [`run_anim_for_skel`] or to idle.
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

/// Load the `sit` rest-pose MO2 from the skeleton DAT. Lotus's
/// `actor_skeleton_static.cpp` resolves `/sit` against the same
/// name-keyed animation map as `idle` / `run`, so the clip lives in
/// the skeleton DAT (not the motion DAT). Some NPC skels ship it
/// (chairs etc. that play "sit" on idle); most don't, so `None` is
/// expected for the majority of skel ids.
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

/// Load the `hea` (heal / CAMP rest) MO2 from the skeleton DAT. PC-only
/// in practice — the rest crouch animation is the same one the server
/// gates `EFFECT_HEALING` on, and only the seven playable race skels
/// ship it.
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

/// Which rest stance the local self is currently in. Set by the
/// `/sit` / `/heal` / `/kneel` slash commands and by the bound
/// `Action::Sit` / `Action::Heal` keypresses; cleared by any
/// movement-input press (`MoveForward` / `Backward` / `Strafe*` /
/// `Turn*` / `Rotate*`) per retail FFXI behavior — stand-on-first-
/// input.
///
/// While `kind != None`:
///   - `dispatch_movement_system` discards translation and rotation
///     deltas (and the *press* of any movement Action clears the
///     stance back to `None`);
///   - `tick_skinned_actors` selects the matching `sit` / `hea` MO2
///     on the self avatar and treats it as uninterruptible
///     (no cross-fade out until cleared);
///   - the keepalive thread for `Heal` still arms server-side
///     `EFFECT_HEALING` via the `AgentCommand::Heal` send path —
///     this resource is the *visual / local-input* surface, not
///     the wire protocol surface.
///
/// `Sit` is a pure client-side affordance — there is no `0x0Eb sit`
/// packet in retail. The server has no opinion on a player sitting.
#[derive(Resource, Default, Debug, Clone, Copy, Eq, PartialEq)]
pub struct RestStance {
    pub kind: RestKind,
}

/// Which rest-pose is active.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub enum RestKind {
    /// Not resting — normal locomotion / engagement animation rules apply.
    #[default]
    None,
    /// `/sit` or `/kneel` — pure visual.
    Sit,
    /// `/heal` — server-armed CAMP plus visual.
    Heal,
}

impl RestStance {
    /// True if any rest pose is held.
    pub fn is_resting(&self) -> bool {
        !matches!(self.kind, RestKind::None)
    }
}

/// Resolve a directional locomotion variant (`bck`/`stl`/`str`/`trn`/`wlk`)
/// from a skeleton DAT. Cached; returns `None` and caches that `None`
/// if the DAT carries no chunk with that 3-char prefix.
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

/// Shared loader: open a DAT, walk its chunks, return the first MO2
/// whose 3-char name prefix matches `prefix`. Used for `btl`, `run`,
/// and any future prefix we wire up (`wlk`, `mvb`, …).
fn load_anim_with_prefix(file_id: u32, prefix: &[u8; 3]) -> Option<Mo2Animation> {
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

/// A logical animation slot picked by `tick_skinned_actors` for a
/// given (engaged, motion-pattern) tuple. Used both as a key for
/// detecting "the clip changed since last frame" and as the
/// `from`/`to` ends of a [`AnimationBlend`].
///
/// We deliberately do NOT use `Arc<Mo2Animation>` pointer identity
/// — the per-skel caches hand back the same `Arc` for repeated
/// queries of the same slot, but multiple distinct
/// `(skel, prefix)` combos may resolve to the same underlying clip
/// when a race lacks a dedicated variant and falls back to e.g.
/// `run`. Comparing by `ClipId` matches the *intent* (what the
/// selection rule chose) rather than the resolved bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ClipId {
    Idle,
    BattleIdle,
    Run,
    CombatRun,
    /// Backpedal — currently always resolves to `Run` played at
    /// negative time-scale (no `bck` clip exists on PC skeletons we
    /// probed). Carried as a distinct ClipId so the cross-fade trips
    /// when transitioning between run and backpedal.
    Backpedal,
    /// Strafe left / right — falls back to `Run` when the
    /// per-skeleton `stl`/`str` probe returns `None`.
    StrafeLeft,
    StrafeRight,
    /// Turn-in-place — falls back to `Idle` when no `trn` clip.
    TurnInPlace,
    /// Walk speed (sub-base-run). Falls back to `Run` when no `wlk`.
    Walk,
}

/// Per-actor cross-fade state. Stored in a `Resource` keyed by wire
/// entity id (parallel to [`EntityMotion`]) rather than as an ECS
/// `Component` so the lifetime is dictated by gameplay (clear when
/// the entity vanishes from the snapshot) rather than ECS spawn /
/// despawn churn.
#[derive(Clone, Copy, Debug)]
pub struct AnimationBlend {
    pub from_clip: ClipId,
    pub to_clip: ClipId,
    /// Blend progress in [0, 1]. 0 = fully on `from`, 1 = fully
    /// settled on `to`.
    pub t: f32,
    /// Total duration in seconds for this cross-fade. 0.15 is the
    /// retail-feel default — long enough to hide popping, short
    /// enough that fast direction changes still feel responsive.
    pub duration: f32,
}

/// Per-actor cross-fade table. See [`AnimationBlend`].
#[derive(Resource, Default)]
pub struct AnimationBlends {
    pub by_id: HashMap<u32, AnimationBlend>,
}

impl AnimationBlends {
    pub const DEFAULT_DURATION: f32 = 0.15;

    /// Start a cross-fade from the entity's last-selected clip to
    /// `to`. If there is no last clip (first frame the entity is
    /// seen) or the entity is already on `to`, sets the blend
    /// directly to t=1.0 / no-op respectively. Call once per frame
    /// per actor.
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
                    // Mid-blend retarget: pin the visual position by
                    // treating the current interpolated state as the
                    // new `from`. We approximate by setting from =
                    // previous to and resetting t — the visual jump
                    // is sub-frame because the blend was already
                    // most of the way through the previous transition
                    // in nearly every realistic case.
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

/// Per-entity motion state: last seen Bevy translation and the
/// computed velocity magnitude from the previous frame.
///
/// Why this exists: the wire `Entity.speed` field is the player's
/// movement *capability* (40 = base run speed, 0 = bound/stunned —
/// per `vendor/server/src/map/packets/char_update.cpp:262`), NOT
/// whether they are currently moving. To pick the right
/// locomotion animation we have to derive "is moving" ourselves
/// from per-frame transform deltas.
///
/// Self in particular gets a `speed = 40` value on LOGIN and never
/// updates (CHAR_PC for self only refreshes pos/heading after
/// zone-in — `session.rs:660-671`). So a `speed > 0` check would
/// have animated self as running the entire session — but in
/// practice the wrong-sign bug went the *other* way: the engaged
/// path was never reached because the snapshot doesn't echo
/// post-LOGIN speed changes consistently.
/// Per-entity directional motion sample.
#[derive(Clone, Copy, Debug, Default)]
pub struct MotionSample {
    /// Last observed Bevy translation. Used to derive next-frame
    /// velocity by finite difference.
    pub last_pos: Vec3,
    /// xz speed magnitude in yalms/sec.
    pub speed: f32,
    /// Velocity projected onto the entity's facing direction (xz). +
    /// = forward, − = backpedal. Magnitude is yalms/sec.
    pub forward_component: f32,
    /// Velocity projected perpendicular to facing. + = right (strafe-R),
    /// − = left (strafe-L). Right is defined as a 90° CW rotation of
    /// forward in the xz-plane (FFXI heading convention).
    pub strafe_component: f32,
    /// Last observed heading (u8 LSB convention). Stored as the
    /// unwrapped float in radians so cross-zero wraps don't generate
    /// spurious "instant 360° spin" rate spikes.
    pub last_heading_rad: f32,
    /// Heading rotation rate in rad/sec, unwrapped across the
    /// 0/2π seam. Sign matches FFXI heading: + = clockwise from
    /// above (per scene::heading_to_quat docstring).
    pub heading_rate: f32,
}

#[derive(Resource, Default)]
pub struct EntityMotion {
    /// Per-wire-entity motion sample. See [`MotionSample`].
    pub by_id: HashMap<u32, MotionSample>,
}

impl EntityMotion {
    /// Pure decision: is this entity currently moving fast enough
    /// to warrant a run animation? Threshold tuned to filter out
    /// floor-snap jitter (`scene::apply_visual_smoothing`) without
    /// missing actual locomotion. FFXI base run speed is ~5
    /// yalms/sec, so 0.5 is well below the genuine-motion floor.
    pub fn is_moving(&self, id: u32) -> bool {
        self.by_id
            .get(&id)
            .is_some_and(|s| s.speed > Self::MOVE_THRESHOLD)
    }

    /// Lookup the full sample. Returns `None` for never-seen ids.
    pub fn sample(&self, id: u32) -> Option<MotionSample> {
        self.by_id.get(&id).copied()
    }

    /// Minimum xz speed in yalms/sec to count as "moving".
    pub const MOVE_THRESHOLD: f32 = 0.5;
    /// Minimum |heading_rate| in rad/sec to count as "turning in place".
    /// ~28°/sec — above sample noise, well below combat-cam yaw flicks.
    pub const TURN_THRESHOLD_RAD_PER_SEC: f32 = 0.5;
}

/// Per-frame: write each `WorldEntity`'s current xz speed into
/// [`EntityMotion`] from its Bevy `Transform` delta. Runs *before*
/// `tick_skinned_actors` so the locomotion animation decision sees
/// the same-frame motion state.
pub fn track_entity_motion_system(
    time: Res<Time>,
    state: Res<SceneState>,
    mut motion: ResMut<EntityMotion>,
    q: Query<(&WorldEntity, &Transform)>,
) {
    let dt = time.delta_secs().max(1e-4);
    for (world, transform) in &q {
        let pos = transform.translation;
        // Convert wire heading (u8, 0..256) to radians using LSB
        // convention echoed in `scene::heading_to_quat`: heading 0 =
        // +Y in FFXI = -Z in Bevy. We compute the forward vector
        // *directly in Bevy xz* so projection math stays consistent
        // with the Transform we read above.
        let heading_u8 = state
            .snapshot
            .entities
            .iter()
            .find(|e| e.id == world.id)
            .map(|e| e.heading)
            .unwrap_or(0);
        let heading_rad =
            (heading_u8 as f32) * std::f32::consts::TAU / 256.0;
        // Forward in Bevy xz: heading 0 → (-Z). `Quat::from_rotation_y(-θ)`
        // applied to default forward (0, 0, -1) gives
        // (-sin(-θ), 0, -cos(-θ)) = (sin θ, 0, -cos θ).
        let fwd_x = heading_rad.sin();
        let fwd_z = -heading_rad.cos();
        // Right vector = 90° CW from forward in xz-plane (looking
        // down +Y). CW rotation of (x, z) by 90° is (z, -x).
        let right_x = fwd_z;
        let right_z = -fwd_x;

        let prev = motion.by_id.get(&world.id).copied().unwrap_or(MotionSample {
            last_pos: pos,
            last_heading_rad: heading_rad,
            ..Default::default()
        });
        let dx = pos.x - prev.last_pos.x;
        let dz = pos.z - prev.last_pos.z;
        let vx = dx / dt;
        let vz = dz / dt;
        let speed = (vx * vx + vz * vz).sqrt();
        let forward_component = vx * fwd_x + vz * fwd_z;
        let strafe_component = vx * right_x + vz * right_z;

        // Heading rate: unwrap across the 0/2π seam so wrapping from
        // 255 → 0 doesn't read as a ~2π/dt instant spin spike.
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
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every PC race (1..=8) must resolve to a distinct motion DAT
    /// (Taru M and F share, which is the only collision). NPCs +
    /// monstrosity skeletons return None so the caller can fall
    /// through to the idle-only path without panic.
    #[test]
    fn motion_dat_resolves_for_each_pc_race() {
        let pairs = [
            (7072, 9672),
            (10248, 12848),
            (13424, 16024),
            (16600, 19200),
            (19776, 22376), // Taru M and Taru F share both skel and motion
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

    /// Skel id that isn't in the PC table (e.g. an NPC or a
    /// monstrosity skeleton) yields `None`. The motion-DAT table is
    /// PC-only by design — see module docs.
    #[test]
    fn motion_dat_returns_none_for_non_pc_skel() {
        assert_eq!(motion_dat_for_skel(0), None);
        assert_eq!(motion_dat_for_skel(7000), None);
        assert_eq!(motion_dat_for_skel(50000), None);
    }

    /// The motion DAT id is always `+2600` past the skeleton id. The
    /// explicit match in `motion_dat_for_skel` is the
    /// source-of-truth, but if a future edit accidentally widens that
    /// offset for one race (e.g. typo in a hand-edited row), this
    /// invariant test will catch it. NOT a refactor toward the
    /// closed form — see module docs.
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

    /// Integration: when retail DATs are reachable, every PC race
    /// should resolve a battle-idle MO2 from its motion DAT. Caught
    /// `btl` vs `idl` naming during development (the motion DAT has
    /// no `idl*` chunk — resting idle lives in the skeleton DAT
    /// only). Skipped silently when DATs aren't available so CI
    /// without DATs still runs.
    #[test]
    fn battle_idle_resolves_for_every_pc_race_when_dats_available() {
        if DatRoot::from_env_or_default().is_err() {
            eprintln!("skipping: no retail DAT root");
            return;
        }
        for skel in [7072u32, 10248, 13424, 16600, 19776, 23176, 26352] {
            let anim =
                battle_idle_anim_for_skel(skel).expect("battle-idle MO2 missing for skel");
            assert!(
                anim.frames > 0,
                "skel {skel}: btl MO2 has zero frames — parse drift?"
            );
        }
    }

    /// Casual `run` lives in the skeleton DAT as `run0`. Same
    /// availability check as the battle-idle test — confirms the
    /// 3-char prefix matcher picks it up and the cache stays
    /// `Some(anim)` for every PC race.
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

    /// `is_moving` threshold: 0 → never moving; below threshold →
    /// not moving (smoothing jitter); above → moving. Pin the
    /// threshold value so a future tweak that bumps the constant
    /// silently disabling run animation on slow movement gets
    /// caught.
    #[test]
    fn is_moving_threshold_behaviour() {
        let mut m = EntityMotion::default();
        let make = |speed: f32| MotionSample {
            last_pos: Vec3::ZERO,
            speed,
            ..Default::default()
        };
        m.by_id.insert(1, make(0.0));
        m.by_id.insert(2, make(0.49));
        m.by_id.insert(3, make(0.51));
        m.by_id.insert(4, make(6.0));
        assert!(!m.is_moving(1), "0 speed should not animate");
        assert!(!m.is_moving(2), "below 0.5 should not animate");
        assert!(m.is_moving(3), "just above 0.5 should animate");
        assert!(m.is_moving(4), "full run speed should animate");
        assert!(!m.is_moving(99), "unknown id should not animate");
    }

    /// `RestStance::is_resting` reflects the kind variant.
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

    /// Combat run lives in the motion DAT as `run1` (68-bone LOD).
    /// Distinct from casual `run0` — verify both load and that the
    /// motion-DAT version has the higher bone count so a future
    /// "wait, am I getting the right LOD" bug fails loudly.
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
