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
#[derive(Resource, Default)]
pub struct EntityMotion {
    /// `(last_translation, current_speed_yalms_per_sec)` per wire
    /// entity id. Speed is the magnitude of last frame's xz delta
    /// divided by `Time::delta_secs()`.
    pub by_id: HashMap<u32, (Vec3, f32)>,
}

impl EntityMotion {
    /// Pure decision: is this entity currently moving fast enough
    /// to warrant a run animation? Threshold tuned to filter out
    /// floor-snap jitter (`scene::apply_visual_smoothing`) without
    /// missing actual locomotion. FFXI base run speed is ~5
    /// yalms/sec, so 0.5 is well below the genuine-motion floor.
    pub fn is_moving(&self, id: u32) -> bool {
        self.by_id.get(&id).is_some_and(|(_, v)| *v > Self::MOVE_THRESHOLD)
    }

    /// Minimum xz speed in yalms/sec to count as "moving". Above
    /// the smoothing noise floor (`SNAP_DIST_SQ.sqrt() * VISUAL_SMOOTH
    /// / dt` for a 60 Hz tick is ~0.6 yalms/sec at the extreme; 0.5
    /// catches everything but a stop-and-go jitter sequence).
    const MOVE_THRESHOLD: f32 = 0.5;
}

/// Per-frame: write each `WorldEntity`'s current xz speed into
/// [`EntityMotion`] from its Bevy `Transform` delta. Runs *before*
/// `tick_skinned_actors` so the locomotion animation decision sees
/// the same-frame motion state.
pub fn track_entity_motion_system(
    time: Res<Time>,
    mut motion: ResMut<EntityMotion>,
    q: Query<(&WorldEntity, &Transform)>,
) {
    let dt = time.delta_secs().max(1e-4);
    for (world, transform) in &q {
        let pos = transform.translation;
        let entry = motion.by_id.entry(world.id).or_insert((pos, 0.0));
        let dx = pos.x - entry.0.x;
        let dz = pos.z - entry.0.z;
        let xz_dist = (dx * dx + dz * dz).sqrt();
        // Y is ignored deliberately: terrain/floor snap can shift
        // y per-frame without the entity actually moving.
        entry.0 = pos;
        entry.1 = xz_dist / dt;
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
        m.by_id.insert(1, (Vec3::ZERO, 0.0));
        m.by_id.insert(2, (Vec3::ZERO, 0.49));
        m.by_id.insert(3, (Vec3::ZERO, 0.51));
        m.by_id.insert(4, (Vec3::ZERO, 6.0));
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
