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

use crate::components::{IsSelf, WorldEntity};
use crate::snapshot::SceneState;
use ffxi_viewer_wire::EntityKind;

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
        7072 => Some(9672),   // Hume M
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

/// Cache for the `run0` run animation in the skeleton DAT. Keyed by
/// skeleton DAT id; not all skeletons have a run anim (NPCs that never
/// relocate) so we cache the `None` result too — the existence check is
/// the load itself.
///
/// NOTE (verified by `zz-anim-cov 7072 9672 run0 run1`): `run0` is NOT a
/// low-LOD of `run1`. `run0` is the LEGS/FEET body-region LAYER (~12 joints
/// incl RightFoot=31/LeftFoot=37, in the skeleton DAT); `run1` is the
/// DISJOINT spine/arms/head layer (~40 joints, in the motion DAT). They
/// composite into ONE full-body run — neither is "casual" vs "combat". The
/// engaged/casual run distinction is NOT `run0` vs `run1`: it comes from
/// resolving the SAME `run?` id through a weapon-specific BATTLE animation
/// directory (XIM Actor.kt:430), which retail picks by weapon class. This
/// helper feeds the legacy `dat_vos2` CPU-bake path only; the live faithful
/// path (`ffxi_actor_render::advance_actor_pose`) composites both layers.
static RUN_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> = OnceLock::new();

/// Cache for the resting `sit` / `hea` animations. Keyed by skeleton
/// DAT id; both clips live in the skeleton DAT on PC races (sit/hea
/// are not weapon-class-specific, so they don't have motion-DAT
/// variants). `None` is cached for skeletons that don't ship the
/// clip — NPC skels routinely lack `hea` even when they have `sit`,
/// so the lookup must be independent per clip.
static SIT_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> = OnceLock::new();
static HEAL_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> = OnceLock::new();

/// Cache for the `run1` run animation in the motion DAT. PC-only; keyed
/// by motion DAT id like [`BATTLE_IDLE_ANIMS`].
///
/// MISNOMER WARNING: despite the historical name, `run1` is NOT a "combat
/// run". It is the spine/arms/head body-region LAYER of the run cycle (the
/// disjoint complement of `run0`'s legs layer — see [`RUN_ANIMS`]). The live
/// faithful path composites `run0`+`run1` for EVERY run, engaged or not. This
/// helper exists only for the legacy `dat_vos2` path, which (incorrectly)
/// treated `run1` as a standalone "combat" clip; do not propagate that
/// interpretation into new code.
static COMBAT_RUN_ANIMS: OnceLock<Mutex<HashMap<u32, Option<Arc<Mo2Animation>>>>> = OnceLock::new();

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
/// Hume M retail archive — the motion DAT carries `btl0` (legs/feet
/// layer) and `btl1` (spine/arms/head layer — they composite, NOT
/// LODs), but no `idl*` chunk; resting idle lives in the *skeleton*
/// DAT only. Lotus's
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

/// Load the `run0` (legs/feet layer) run animation from a *skeleton* DAT.
/// Lotus's classic-input `playAnimationLoop("run", speed)` resolves
/// against the skeleton DAT's animation map
/// (`actor_skeleton_static.cpp:86-108`), which is where `run0` lives
/// for PCs. NPC skeleton DATs also carry `run` when they're meant to
/// relocate; non-relocating NPCs return `None` and the caller stays on idle.
///
/// `run0` is one HALF of the run cycle (legs/feet only); a faithful run needs
/// the `run1` arms/spine layer composited on top — see [`RUN_ANIMS`]. The
/// legacy `dat_vos2` path that calls this only plays one layer at a time.
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

/// Load the `run1` (spine/arms/head layer) run animation from the PC race's
/// motion DAT.
///
/// MISNOMER WARNING: this is NOT a "combat run". `run1` is the disjoint
/// upper-body LAYER of the casual run cycle — it composites with `run0`
/// (legs, from the skeleton DAT) for EVERY run, engaged or not (verified by
/// `zz-anim-cov`: `run1` = ~40 spine/arms/head joints, no leg joints). A real
/// engaged/combat run would come from a weapon-specific BATTLE directory
/// resolved by weapon class (XIM Actor.kt:430), not from `run1`. Kept only for
/// the legacy `dat_vos2` path; the live faithful path composites both layers.
///
/// Returns `None` for non-PC skeletons (NPCs etc.).
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

/// Walk/run toggle (retail `Z`). When `walking` is true, the input
/// dispatcher scales the step magnitude by [`WalkMode::WALK_SCALE`]
/// (~50%), producing the slower retail walk gait. Flip via the
/// `Action::ToggleWalk` keybind.
///
/// Pure local affordance — there is no server-side walk packet; the
/// server only cares about the actual position deltas, which still
/// land within the reactor speed envelope when scaled.
#[derive(Resource, Default, Debug, Clone, Copy, Eq, PartialEq)]
pub struct WalkMode {
    pub walking: bool,
}

impl WalkMode {
    /// Step-magnitude multiplier when walking. Retail walk is roughly
    /// a quarter of run speed (slow, deliberate gait — easy to miss
    /// with /target while moving). 0.25 matches that feel.
    pub const WALK_SCALE: f32 = 0.25;
    /// Returns the per-tick speed multiplier (1.0 for run, 0.5 for walk).
    pub fn scale(self) -> f32 {
        if self.walking {
            Self::WALK_SCALE
        } else {
            1.0
        }
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
    /// `run0` (legs/feet layer) in the legacy single-layer `dat_vos2` path.
    Run,
    /// MISNOMER: `run1` (spine/arms/head layer), NOT a distinct combat run.
    /// The legacy `dat_vos2` path swaps `Run`->`CombatRun` (legs->upper body)
    /// when engaged, which is wrong — it shows only the upper-body layer with
    /// no legs. The live faithful path composites BOTH layers and has no such
    /// swap. A genuine engaged run would come from a weapon-class battle dir,
    /// not from `run1`. Retained only to keep the legacy path compiling.
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
    /// EMA-smoothed xz velocity. Server-driven entity positions update in
    /// steps, so the raw per-frame finite-difference velocity is 0 on most
    /// frames and a large spike on update frames — at high frame rates that
    /// flips the locomotion clip Idle↔Run every few frames. `speed` /
    /// `forward_component` / `strafe_component` are derived from this
    /// low-pass-filtered velocity instead, which a steadily-moving entity
    /// keeps above [`EntityMotion::MOVE_THRESHOLD`] regardless of frame rate.
    pub smooth_vx: f32,
    pub smooth_vz: f32,
    /// Latched "is moving" state with hysteresis (see
    /// [`EntityMotion::apply_move_hysteresis`]). Crossing a single threshold
    /// chatters idle↔run when `speed` hovers at the boundary; the latch only
    /// flips on the wider [`EntityMotion::MOVE_ENTER`] /
    /// [`EntityMotion::MOVE_EXIT`] bands. [`EntityMotion::is_moving`] reads
    /// this, NOT the raw `speed`.
    pub moving: bool,
}

#[derive(Resource, Default)]
pub struct EntityMotion {
    /// Per-wire-entity motion sample. See [`MotionSample`].
    pub by_id: HashMap<u32, MotionSample>,
}

impl EntityMotion {
    /// Is this entity currently moving (latched, hysteretic)? Reads the
    /// [`MotionSample::moving`] latch that `track_entity_motion_system`
    /// maintains via [`apply_move_hysteresis`](Self::apply_move_hysteresis) —
    /// NOT the raw `speed` — so a steady walker right at the boundary doesn't
    /// flip idle↔run every few frames.
    pub fn is_moving(&self, id: u32) -> bool {
        self.by_id.get(&id).is_some_and(|s| s.moving)
    }

    /// Lookup the full sample. Returns `None` for never-seen ids.
    pub fn sample(&self, id: u32) -> Option<MotionSample> {
        self.by_id.get(&id).copied()
    }

    /// Hysteresis for the [`MotionSample::moving`] latch: enter the moving
    /// state only above [`MOVE_ENTER`](Self::MOVE_ENTER), leave it only below
    /// [`MOVE_EXIT`](Self::MOVE_EXIT), and hold the previous state in between.
    /// The gap between the two thresholds is what kills idle↔run chatter near
    /// the boundary.
    pub fn apply_move_hysteresis(prev_moving: bool, speed: f32) -> bool {
        if speed >= Self::MOVE_ENTER {
            true
        } else if speed <= Self::MOVE_EXIT {
            false
        } else {
            prev_moving
        }
    }

    /// Minimum xz speed in yalms/sec to count as "moving" (legacy single
    /// threshold; superseded by the [`MOVE_ENTER`](Self::MOVE_ENTER) /
    /// [`MOVE_EXIT`](Self::MOVE_EXIT) hysteresis band but retained for the
    /// walk/run boundary lower bound and existing call sites).
    pub const MOVE_THRESHOLD: f32 = 0.5;
    /// Upper hysteresis band: speed must exceed this to flip idle→moving.
    pub const MOVE_ENTER: f32 = 0.8;
    /// Lower hysteresis band: speed must drop below this to flip moving→idle.
    pub const MOVE_EXIT: f32 = 0.35;
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
    // Was O(N²): each entity in the query did a linear `.find()` over
    // the full snapshot.entities Vec. With ~200 nearby entities that's
    // 40k scans/frame on the locomotion path. Build a heading index
    // once.
    let heading_by_id: std::collections::HashMap<u32, u8> = state
        .snapshot
        .entities
        .iter()
        .map(|e| (e.id, e.heading))
        .collect();
    for (world, transform) in &q {
        let pos = transform.translation;
        // Convert wire heading (u8, 0..256) to radians using LSB
        // convention echoed in `scene::heading_to_quat`: heading 0 =
        // +Y in FFXI = -Z in Bevy. We compute the forward vector
        // *directly in Bevy xz* so projection math stays consistent
        // with the Transform we read above.
        let heading_u8 = heading_by_id.get(&world.id).copied().unwrap_or(0);
        let heading_rad = (heading_u8 as f32) * std::f32::consts::TAU / 256.0;
        // Forward in Bevy xz: heading 0 → (-Z). `Quat::from_rotation_y(-θ)`
        // applied to default forward (0, 0, -1) gives
        // (-sin(-θ), 0, -cos(-θ)) = (sin θ, 0, -cos θ).
        let fwd_x = heading_rad.sin();
        let fwd_z = -heading_rad.cos();
        // Right vector = 90° CW from forward in xz-plane (looking
        // down +Y). CW rotation of (x, z) by 90° is (z, -x).
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
        // Low-pass the finite-difference velocity (see `MotionSample`).
        // `alpha = 1 - e^(-dt/τ)` makes the smoothing frame-rate-independent;
        // τ ≈ 0.25 s holds a steadily-moving entity above the move threshold
        // between stepped position updates without lagging start/stop noticeably.
        const VEL_TAU: f32 = 0.25;
        let alpha = 1.0 - (-dt / VEL_TAU).exp();
        let smooth_vx = prev.smooth_vx + alpha * (dx / dt - prev.smooth_vx);
        let smooth_vz = prev.smooth_vz + alpha * (dz / dt - prev.smooth_vz);
        let speed = (smooth_vx * smooth_vx + smooth_vz * smooth_vz).sqrt();
        let forward_component = smooth_vx * fwd_x + smooth_vz * fwd_z;
        let strafe_component = smooth_vx * right_x + smooth_vz * right_z;

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
                smooth_vx,
                smooth_vz,
                moving: EntityMotion::apply_move_hysteresis(prev.moving, speed),
            },
        );
    }
}

// =========================================================================
// Dead-reckoning ("object in motion stays in motion") for remote actors
// =========================================================================
//
// The wire snapshot for other actors (Mob/Pc/Pet) only changes on
// packet-arrival frames (`snapshot::ingest_system` flips `dirty` only when
// the session mutated), and `scene::sync_entities_system` early-returns on
// `!dirty`. So without prediction a remote actor's Bevy `Transform` freezes
// between server updates and then jumps — the classic stop-start stutter,
// which also flips the locomotion clip idle↔run every tick.
//
// This module makes the predictor the SOLE writer of the rendered XZ + facing
// for moving non-self kinds. `sync_entities_system` pushes each server sample
// in via [`EntityPrediction::observe`]; [`predict_entities_system`] runs every
// frame (NOT gated on `dirty`), extrapolating along the last-known velocity
// and rubber-banding the rendered position toward the server-authoritative
// track. Self is exempt (locally integrated + reconciled in the client crate);
// static Npc/Other keep their direct transform write in `sync_entities_system`.

/// Per-entity dead-reckoning state. See module note above.
#[derive(Clone, Copy, Debug)]
pub struct PredictSample {
    /// Position we actually write to `Transform` (XZ dead-reckoned, Y smoothed).
    pub rendered_pos: Vec3,
    /// Last server-authoritative position (Bevy space) — the rubber-band anchor.
    pub server_pos: Vec3,
    /// `server_pos` at the previous ingest — used to derive measured velocity.
    pub last_server_pos: Vec3,
    /// Smoothed dead-reckon velocity (Bevy XZ; y always 0).
    pub dr_velocity: Vec3,
    /// Last server heading (u8 LSB convention).
    pub target_heading: u8,
    /// Heading actually applied this frame (radians, "heading space" — the
    /// `Quat::from_rotation_y(-rendered_heading_rad)` sign matches
    /// `scene::heading_to_quat`). Slewed toward `target_heading`.
    pub rendered_heading_rad: f32,
    /// Seconds since the current `server_pos` arrived (drives Δt, the
    /// server-track extrapolation, and the stale-velocity decay).
    pub secs_since_update: f32,
    /// Set by `observe` when `server_pos` actually moved; consumed (cleared)
    /// by the predictor's ingest path. A HP-only tick that preserves position
    /// must NOT raise this (else it re-ingests a zero Δpos and decays velocity).
    pub sample_dirty: bool,
    /// False until the first server sample seeds the entry.
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

/// Per-wire-entity dead-reckoning table, keyed by `Entity::id`. Parallel to
/// [`EntityMotion`]; drained on the same stale-id set in
/// `scene::sync_entities_system` and cleared on `OnExit(AppPhase::InGame)`.
#[derive(Resource, Default)]
pub struct EntityPrediction {
    pub by_id: HashMap<u32, PredictSample>,
}

impl EntityPrediction {
    /// Discontinuity gate (yalm²): if the fresh server position is farther than
    /// this from where we predicted the actor to be, snap instead of
    /// dead-reckoning (teleport / zone / respawn). Reuses
    /// `scene::SNAP_DIST_SQ`'s 2-yalm gate. Compared against `rendered_pos`,
    /// NOT `last_server_pos`, so a fast mover whose prediction is tracking the
    /// server doesn't snap every tick.
    pub const SNAP_DIST_SQ: f32 = 4.0;
    /// Accel/decel time constant: how fast `dr_velocity` blends toward the
    /// freshly-measured velocity. Never hard-set, so direction/speed changes
    /// ease in.
    pub const VEL_BLEND_TAU: f32 = 0.20;
    /// Rubber-band time constant: how fast `rendered_pos` converges to the
    /// extrapolated server track. Kept tight so the rendered position never
    /// drifts far from the server-authoritative one (the server's 3D range
    /// check uses the true position, so the operator must be aiming near it).
    pub const CORRECT_TAU: f32 = 0.12;
    /// Vertical smoothing time constant. Y is server-authoritative and NOT
    /// dead-reckoned (extrapolating Y floats actors through terrain steps the
    /// server resolves discretely); just low-passed toward `server_pos.y`.
    pub const Y_TAU: f32 = 0.15;
    /// Heading slew time constant.
    pub const HEADING_TAU: f32 = 0.10;
    /// Once a sample is older than [`STALE_VEL_SECS`](Self::STALE_VEL_SECS),
    /// decay `dr_velocity` with this time constant so an actor that stops
    /// getting updates coasts to a stop instead of drifting forever.
    pub const DECEL_TAU: f32 = 0.25;
    /// Age (seconds) past which `dr_velocity` starts decaying (~2 missed ticks).
    pub const STALE_VEL_SECS: f32 = 0.6;
    /// Velocity clamp (yalms/sec): base run ≈5, leaving headroom for
    /// haste/mounts without letting one jumpy sample fling the prediction.
    pub const MAX_DR_SPEED: f32 = 7.0;
    /// Δt ceiling (seconds): a gap longer than this is a stall/respawn, treated
    /// as a discontinuity (snap) rather than a velocity estimate.
    pub const DT_SERVER_CEIL: f32 = 1.0;
    /// XZ-distance² (yalm²) a server sample must move before it counts as a new
    /// position. Filters HP-only ticks that re-send the preserved position.
    const SAMPLE_EPSILON_SQ: f32 = 1e-4;

    /// Push a server sample for a moving non-self entity. Seeds the entry on
    /// first sight (no lurch from origin); afterward records the new
    /// `server_pos` + heading and raises `sample_dirty` only when the position
    /// actually changed.
    pub fn observe(&mut self, id: u32, server_pos: Vec3, heading: u8) {
        match self.by_id.get_mut(&id) {
            None => {
                self.by_id.insert(id, PredictSample::seed(server_pos, heading));
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

/// FFXI heading byte → radians in "heading space" (0..τ). The applied
/// rotation is `Quat::from_rotation_y(-rad)`, matching `scene::heading_to_quat`.
#[inline]
fn heading_to_rad(heading: u8) -> f32 {
    (heading as f32) * std::f32::consts::TAU / 256.0
}

/// Frame-rate-independent exponential approach: move `from` a fraction of the
/// way to `to` such that the gap decays with time constant `tau`.
#[inline]
fn exp_approach(from: f32, to: f32, tau: f32, dt: f32) -> f32 {
    let alpha = 1.0 - (-dt / tau.max(1e-4)).exp();
    from + alpha * (to - from)
}

/// Advance one dead-reckoning sample by `dt`, consuming a fresh server sample
/// first when `sample_dirty`. Pure so it can be unit-tested without Bevy.
/// Returns the `(rendered_pos, rendered_heading_rad)` to apply to the transform.
fn advance_prediction(s: &mut PredictSample, dt: f32) -> (Vec3, f32) {
    use std::f32::consts::{PI, TAU};

    // --- Ingest a fresh server sample (if one arrived this frame) ---
    if s.sample_dirty {
        s.sample_dirty = false;
        let dt_server = s.secs_since_update;
        let discontinuity = dt_server > EntityPrediction::DT_SERVER_CEIL
            || s.server_pos.distance_squared(s.rendered_pos) >= EntityPrediction::SNAP_DIST_SQ;
        if discontinuity {
            // Teleport / zone / respawn: snap, never seed velocity.
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

    // --- Advance every frame: dead-reckon + rubber-band toward server track ---
    let mut pos = s.rendered_pos + s.dr_velocity * dt;
    let server_track = s.server_pos + s.dr_velocity * s.secs_since_update;
    // XZ converges to the extrapolated server track; Y just smooths to server.
    pos.x = exp_approach(pos.x, server_track.x, EntityPrediction::CORRECT_TAU, dt);
    pos.z = exp_approach(pos.z, server_track.z, EntityPrediction::CORRECT_TAU, dt);
    pos.y = exp_approach(s.rendered_pos.y, s.server_pos.y, EntityPrediction::Y_TAU, dt);
    s.rendered_pos = pos;

    s.secs_since_update += dt;
    if s.secs_since_update > EntityPrediction::STALE_VEL_SECS {
        s.dr_velocity *= (-dt / EntityPrediction::DECEL_TAU).exp();
    }

    // --- Heading slew (unwrap across the 0/τ seam) ---
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

/// Per-frame: dead-reckon the rendered transform for every moving non-self
/// actor from its [`EntityPrediction`] entry. Runs after
/// `scene::sync_entities_system` (which feeds server samples in) and before
/// [`track_entity_motion_system`] (which then reads the now-smooth transform).
/// NOT gated on `SceneState::dirty` — that's the whole point: it advances on
/// the frames between server packets.
pub fn predict_entities_system(
    time: Res<Time>,
    mut prediction: ResMut<EntityPrediction>,
    mut q: Query<(&WorldEntity, &mut Transform), Without<IsSelf>>,
) {
    let dt = time.delta_secs().max(1e-4);
    for (world, mut transform) in &mut q {
        if !matches!(world.kind, EntityKind::Mob | EntityKind::Pc | EntityKind::Pet) {
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

/// Forces every `tick_skinned_actors` actor onto a single named clip,
/// bypassing the engagement/motion/rest state machine. Only the
/// `--model-viewer` subcommand registers this — its absence keeps the
/// live-game path byte-identical.
///
/// `clip_name` is the 3-char MO2 prefix (`"idl"`, `"btl"`, `"run"`,
/// `"sit"`, …). The override resolves against the skeleton DAT first,
/// then the PC motion DAT (`motion_dat_for_skel`) so PC combat clips
/// (`btl0`, `run1`) are reachable.
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

/// Enumerate every animation chunk (clip) discoverable for a given
/// skeleton DAT. For PC race skeletons, also appends clips from the
/// race's motion DAT (`motion_dat_for_skel`) so battle/run1/etc. are
/// discoverable alongside skeleton-resident clips like `idl`/`sit`/`wlk`.
///
/// One-shot read — not a hot path. The model viewer calls this on mode
/// change to populate the clip cycler.
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

/// Resolve a 3-char-prefix clip override against the skeleton DAT and,
/// if no match, the PC motion DAT. Returns `None` for non-existent
/// prefixes (e.g. typing `xxx`).
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
            let anim = battle_idle_anim_for_skel(skel).expect("battle-idle MO2 missing for skel");
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

    /// `run1` (motion DAT) and `run0` (skeleton DAT) are DISJOINT body-region
    /// LAYERS, not LODs: `run0` = ~12 legs/feet joints, `run1` = ~40 spine/
    /// arms/head joints (verified by `zz-anim-cov`). Because `run1` covers the
    /// larger upper-body region it has the higher bone count — this test pins
    /// that both load and that the layer sizes don't silently swap (which would
    /// flag a DAT-routing regression), NOT that one is a higher LOD of the
    /// other. Neither is "combat" vs "casual"; see [`COMBAT_RUN_ANIMS`].
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
