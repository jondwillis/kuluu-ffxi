use ffxi_dat::datid::DatId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    None,
    Forward,
    Left,
    Right,
    Backward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngageAnimationState {
    NotEngaged,
    Engaged,
    Engaging,
    Disengaging,
}

impl EngageAnimationState {
    pub fn is_battle_idle(self) -> bool {
        matches!(self, Self::Engaged | Self::Disengaging)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestKind {
    None,

    Sit,

    Heal,

    Kneel,
}

pub const LOAD_ROUTINE: [u8; 4] = *b"init";

/// Retail's sub->routine table ("Sub-to-routine dispatch" in
/// .agents/skills/retail-observe/references/2026-09-08-worm-burrow-routines.md): the raw
/// animationsub byte indexes [init, ini1, ini2, ini3] with a
/// mod-4 wrap that absorbs LSB's spawn flag. The client plays the named routine from the model
/// DAT through its generic named-play slots; a model that does not ship it simply gets nothing.
/// No interpretation of what any particular model does with a sub value lives in the engine:
/// the DAT decides.
pub const SPECIAL_ROUTINE_TABLE: [[u8; 4]; 8] = [
    LOAD_ROUTINE,
    *b"ini1",
    *b"ini2",
    *b"ini3",
    LOAD_ROUTINE,
    *b"ini1",
    *b"ini2",
    *b"ini3",
];

const SPECIAL_ROUTINE_INDEX_MASK: u8 = SPECIAL_ROUTINE_TABLE.len() as u8 - 1;

/// The active special-pose routine for a raw animationsub byte. Sub 0 (and the spawn-flagged 4)
/// means no active special: 'init' is the load routine, not an override.
pub fn special_routine(animationsub: u8) -> Option<[u8; 4]> {
    let name = SPECIAL_ROUTINE_TABLE[(animationsub & SPECIAL_ROUTINE_INDEX_MASK) as usize];
    (name != LOAD_ROUTINE).then_some(name)
}

/// The wire state retail's special-pose mechanism consumes: the raw animationsub byte, whether
/// status hides the actor, and which routine was last triggered.
/// No per-mob interpretation: what a sub value does is defined by the model DAT's routine of
/// that name if it ships one at all (worms use ini1/init for their dig/pop special poses; other
/// models may point sp?? clips at entirely different things, or ship nothing).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SpecialPose {
    /// Raw animationsub byte as last seen on the wire. Retail indexes its table with the raw
    /// 3-bit value and lets the mod-4 wrap absorb the spawn flag, so no masking here.
    pub sub: u8,

    /// status == INVISIBLE(3): the actor is hidden. Retail destroys it; we keep one hidden
    /// actor instead of destroying/rebuilding, so the resurface's `init` runs on our actor.
    pub hidden: bool,

    /// The routine last triggered for this entity: table[sub] when a sub change landed while
    /// visible, 'init' when the entity resurfaced (retail's create path). The pose pass
    /// resolves it against the model DAT. It persists after the wire sub clears so the pose
    /// can stay held for the routine's AnimationLock before falling to idle; None = nothing
    /// ever triggered.
    pub active_routine: Option<[u8; 4]>,

    /// Whether the pose override is held by the wire slot (a sub change on a live actor) rather
    /// than by the routine's own AnimationLock. True while the wire sub is set by a change -
    /// the dig's buried pose, held until the sub clears or the resurface. False after a
    /// resurface, where retail's fresh actor has no slot: the pose is held only for the load
    /// routine's lock length, then falls to idle even if the server keeps the sub set.
    pub slot_held: bool,
}

/// LSB `STATUS_TYPE::INVISIBLE` (vendor/server/data/enums/status.yaml): the server hides
/// the model entirely while a burrowing mob is underground.
pub const INVISIBLE_STATUS: u8 = 3;

/// One frame's special-pose transition: the new state plus the routine retail would start on
/// this entity now, if any (None when nothing triggers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecialPoseStep {
    pub pose: SpecialPose,

    /// The routine to fire this frame. A sub change while visible plays table[sub] on the model;
    /// a hidden->visible transition is an actor create in retail and runs the load routine 'init'.
    pub triggered: Option<[u8; 4]>,
}

/// Advance one entity's special-pose state from last frame to this snapshot, mirroring retail's
/// two triggers:
///   * animationsub changing to a named routine while the actor is visible plays that routine;
///     the wire slot holds the pose until the sub clears.
///   * a hidden->visible transition runs 'init' (the DAT's load routine) on retail's fresh
///     actor, which has no slot: the pose is held only for the routine's AnimationLock length,
///     then falls to idle even if the server keeps the sub set.
///
/// A sub change to zero clears the slot but leaves the last-triggered routine selected, so the
/// pose pass can hold it for the routine's lock before settling. While hidden nothing triggers:
/// retail has no live actor to run it on; the resurface replays 'init'.
///
/// The server (vendor/server/src/map/ai/controllers/mob_controller.cpp) drives the worm cycle:
/// dig sets `animationsub = 1` while still visible, then flips `status -> INVISIBLE` ~3s later;
/// pop-up sends an explicit position update (which carries the still-INVISIBLE status byte) and
/// then flips `status` back to visible with `animationsub` still set for ~2s before it returns to
/// 0. A sub-clear arriving mid-routine does nothing in retail: the routine's lock runs its
/// natural length and only then does the pose settle - the server never has to stop the
/// animation.
///
/// Our port keeps one hidden actor instead of destroying/rebuilding it (retail destroys on
/// INVISIBLE and constructs a fresh model on resurface), so `hidden` stands in for "actor
/// destroyed" and the resurface's 'init' runs on our kept-hidden actor.
pub fn next_special_pose(prev: &SpecialPose, status: u8, animationsub: u8) -> SpecialPoseStep {
    let hidden = status == INVISIBLE_STATUS;
    let (active_routine, slot_held, triggered) = if hidden {
        (prev.active_routine, prev.slot_held, None)
    } else if prev.hidden {
        (Some(LOAD_ROUTINE), false, Some(LOAD_ROUTINE))
    } else if animationsub != prev.sub {
        match special_routine(animationsub) {
            Some(routine) => (Some(routine), true, Some(routine)),
            None => (prev.active_routine, false, None),
        }
    } else if special_routine(animationsub).is_none() {
        (prev.active_routine, false, None)
    } else {
        (prev.active_routine, prev.slot_held, None)
    };
    SpecialPoseStep {
        pose: SpecialPose {
            sub: animationsub,
            hidden,
            active_routine,
            slot_held,
        },
        triggered,
    }
}

/// Gait from the wire speed bytes. LSB's UpdateSpeed(run) multiplies `speed` only and never
/// touches animationSpeed (vendor/server/src/map/entities/battle_entity.cpp), so a speed byte
/// above the base means the server is running this entity; at or below it, walking. Applies to
/// server-paced kinds (Mob/Pet/Npc): a PC's walk toggle rides the 0x00D RunMode bit and its
/// speed bytes stay at base either way.
pub fn wire_walking(speed: u8, speed_base: u8) -> bool {
    speed <= speed_base
}

#[derive(Debug, Clone, Copy)]
pub struct ActorAnimInputs {
    pub moving: bool,
    pub walking: bool,

    /// Walk/run clip playback scale relative to the authored rate. Retail's AnimationSpeed
    /// (SpeedBase * 0.1) drives it; 1.0 plays the clip at its authored pace.
    pub playback_rate: f32,

    pub forward_vel: f32,

    pub strafe_vel: f32,
    pub heading_rate: f32,

    pub engage_state: EngageAnimationState,
    pub dead: bool,

    pub owner_is_none: bool,

    pub mount_pose_type: Option<u8>,

    pub has_dft_idle: bool,
    pub rest: RestKind,
    pub mount_or_chocobo: bool,
    pub static_npc: bool,

    pub idle_mode: u8,

    pub battle_mode: u8,

    pub walking_mode: u8,

    pub running_mode: u8,

    /// Fishing macro-state phase 0..=6 (see [`fishing_clip`]), or `None` when not fishing.
    /// Driven by the entity's server_status animation byte for observed players, and by
    /// the local mini-game state machine for self.
    pub fishing_phase: Option<u8>,

    /// Special-pose wire state (see [`next_special_pose`]): raw animationsub, hidden flag and
    /// the last-triggered routine name. The pose pass resolves that name against this model's
    /// DAT; models without the routine get plain locomotion.
    pub special: SpecialPose,
}

impl Default for ActorAnimInputs {
    fn default() -> Self {
        ActorAnimInputs {
            moving: false,
            walking: false,
            playback_rate: 1.0,
            forward_vel: 0.0,
            strafe_vel: 0.0,
            heading_rate: 0.0,
            engage_state: EngageAnimationState::NotEngaged,
            dead: false,
            owner_is_none: true,
            mount_pose_type: None,
            has_dft_idle: false,
            rest: RestKind::None,
            mount_or_chocobo: false,
            static_npc: false,
            idle_mode: 0,
            battle_mode: 0,
            walking_mode: 0,
            running_mode: 0,
            fishing_phase: None,
            special: SpecialPose::default(),
        }
    }
}

impl ActorAnimInputs {
    /// The bucket this frame's travel falls in, before any per-actor latch; `None` while still.
    pub fn travel(&self) -> Direction {
        if !self.moving {
            return Direction::None;
        }
        movement_direction(self.forward_vel, self.strafe_vel)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FishingClip {
    pub id: DatId,

    /// `true` for the looping waiting/fighting poses, `false` for the resolution poses
    /// that play once and hold the final frame.
    pub looping: bool,
}

/// Maps a fishing macro-state phase (0..=6) to its `fsh<n>` model clip. Phases:
/// 0=cast/wait, 1=fighting, 2=caught fish, 3=rod break, 4=line break, 5=caught monster,
/// 6=stop/cancel. research/xim poc/Actor.kt updateFishingState.
pub fn fishing_clip(phase: u8) -> Option<FishingClip> {
    if phase > 6 {
        return None;
    }
    let id = DatId::from_name(&[b'f', b's', b'h', b'0' + phase]);
    Some(FishingClip {
        id,
        looping: phase <= 1,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedAnimation {
    pub id: DatId,

    pub idle: bool,
}

pub fn animation_mode_variant(default_id: DatId, mode: u8, variant_template: &str) -> Vec<DatId> {
    if mode == 0 {
        return vec![default_id];
    }

    let variant = DatId::from_str(&format!("{}{}?", mode, variant_template));
    vec![variant, default_id]
}

pub fn idle_animation_id(inputs: &ActorAnimInputs) -> Vec<DatId> {
    if inputs.mount_or_chocobo {
        return vec![DatId::from_str("chi?")];
    }

    if let Some(pose_type) = inputs.mount_pose_type {
        return vec![DatId::from_str(&format!("{}un?", pose_type))];
    }

    if inputs.static_npc && inputs.has_dft_idle {
        return vec![DatId::from_str("dft?")];
    }

    if inputs.dead && inputs.owner_is_none {
        return animation_mode_variant(corpse_pose_id(), inputs.idle_mode, "cr");
    }

    if inputs.engage_state.is_battle_idle() {
        return animation_mode_variant(DatId::from_str("btl?"), inputs.battle_mode, "tl");
    }

    animation_mode_variant(DatId::from_str("idl?"), inputs.idle_mode, "dl")
}

// The travel speed below which xim stops classifying at all (Actor.kt getMovementDirection's
// `magnitudeSquare() <= 1e-5`), squared units.
const STILL_SPEED_SQ: f32 = 1e-5;

/// research/xim Actor.kt getMovementDirection's bucketing, applied to the two cosines a caller has
/// already resolved on its reference axes. xim's own comment: "Prefer to run forward > horizontal > backward".
fn classify_travel(cos_angle: f32, lateral_cos: f32) -> Direction {
    const FORWARD_COS_MIN: f32 = 0.25;
    const BACKWARD_COS_MAX: f32 = -0.75;

    if cos_angle >= FORWARD_COS_MIN {
        Direction::Forward
    } else if cos_angle >= BACKWARD_COS_MAX {
        if lateral_cos >= 0.0 {
            Direction::Right
        } else {
            Direction::Left
        }
    } else {
        Direction::Backward
    }
}

pub fn movement_direction(forward_vel: f32, strafe_vel: f32) -> Direction {
    let speed_sq = forward_vel * forward_vel + strafe_vel * strafe_vel;
    if speed_sq <= STILL_SPEED_SQ {
        return Direction::None;
    }

    let inv = 1.0 / speed_sq.sqrt();
    classify_travel(forward_vel * inv, strafe_vel * inv)
}

/// research/xim Actor.kt getMovementDirection measures travel against the direction to the actor's
/// target, not its own facing — that is the form the swing routines need (`moving_swing_routine`).
/// `bearing` is the flat vector from the mover to that target. A degenerate bearing has no axis to
/// classify on, so it answers like xim's own "no locked target and not strafing" branch: `None`.
pub fn movement_direction_toward(
    vel_x: f32,
    vel_z: f32,
    bearing_x: f32,
    bearing_z: f32,
) -> Direction {
    let bearing_sq = bearing_x * bearing_x + bearing_z * bearing_z;
    if bearing_sq <= STILL_SPEED_SQ || (vel_x * vel_x + vel_z * vel_z) <= STILL_SPEED_SQ {
        return Direction::None;
    }

    let inv_speed = 1.0 / (vel_x * vel_x + vel_z * vel_z).sqrt();
    let vx = vel_x * inv_speed;
    let vz = vel_z * inv_speed;
    // xim's lateral axis is bearing × UP, which in the flat XZ plane is (-bearing.z, bearing.x); a
    // positive dot on it is travel to the target's right — the same pair combat_stance resolves an
    // entity's own `right` from, so the Left/Right labels agree with the mvl?/mvr? clip choice.
    let inv_bearing = 1.0 / bearing_sq.sqrt();
    let bx = bearing_x * inv_bearing;
    let bz = bearing_z * inv_bearing;

    classify_travel(vx * bx + vz * bz, vx * -bz + vz * bx)
}

/// The motion-name chooser: `travel` is the bucket after [`SideStepFlipLatch`], which is what the
/// chooser reads. Travel back from the target names the back-step clip (`mvb?`), walking or
/// running; a run that buckets left or right of the line to the target names the side-step clips
/// (`mvl?`/`mvr?`); forward travel keeps the gait. The local player's free motion never reaches
/// those buckets (the walker turns the body onto its travel), so for it they show with the camera
/// locked on, weapon out or not.
///
/// Provenance (`FFXiMain.dll retail-2026-09`): the chooser at RVA 0xC8D36..0xC8DC5 names `mvl `
/// (RVA 0xC8DAF) for bucket 4, `mvb ` (RVA 0xC8DB7) for 3 and `mvr ` (RVA 0xC8DBF) for 2 whenever
/// auto-run is on or free-run is off; the lock handlers clear free-run (RVA 0xC5440, RVA 0xC54C0)
/// and the release sets it (RVA 0xC5410). Its other naming path (RVA 0xC8DC7) names `wlk ` for the
/// side buckets but still jumps to the `mvb ` store for bucket 3 (RVA 0xC8DD3). The buckets come
/// from the classifier at RVA 0xA80D0, whose 2/3/4 exits test x87 C0 (`fnstsw` then
/// `and eax, 0x100`), the less-than flag, so all four exits are live.
pub fn movement_animation(inputs: &ActorAnimInputs, travel: Direction) -> Vec<DatId> {
    match travel {
        Direction::Backward => vec![DatId::from_str("mvb?")],
        _ if inputs.walking => {
            animation_mode_variant(DatId::from_str("wlk?"), inputs.walking_mode, "lk")
        }
        Direction::Left => vec![DatId::from_str("mvl?")],
        Direction::Right => vec![DatId::from_str("mvr?")],
        Direction::None | Direction::Forward => {
            animation_mode_variant(DatId::from_str("run?"), inputs.running_mode, "un")
        }
    }
}

/// Rendered frames of straight gait a direct left/right flip shows before the new side step.
pub const SIDE_STEP_FLIP_STRAIGHT_FRAMES: f32 = 2.0;

/// A direct change of side step, left to right or right to left, shows the straight gait for
/// [`SIDE_STEP_FLIP_STRAIGHT_FRAMES`] rendered frames before the new side step plays, so the
/// chooser never goes from one side step straight to the other. Every other change passes through
/// untouched. A still frame clears the comparison (only consecutive moving frames can flip) but
/// leaves any straight frames still owed for the next moving frame.
///
/// Provenance (`FFXiMain.dll retail-2026-09`): RVA 0xA7EF6..0xA7F2D compares the stored bucket
/// with the classifier's new one; a 2→4 or 4→2 change arms a per-actor byte at 2, and while it is
/// positive every moving frame decrements it and stores bucket 1 for the chooser. A still frame
/// (travel length at most 0.001) stores bucket 0 instead and skips the byte (RVA 0xA6C05).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SideStepFlipLatch {
    /// The bucket the chooser read last frame.
    shown: Direction,
    /// Rendered frames of straight gait still owed after a flip.
    straight_owed: f32,
}

impl Default for SideStepFlipLatch {
    fn default() -> Self {
        Self {
            shown: Direction::None,
            straight_owed: 0.0,
        }
    }
}

impl SideStepFlipLatch {
    /// One frame of travel. `classified` is this frame's bucket ([`ActorAnimInputs::travel`]) and
    /// `rendered_frames` how far retail's rendered-frame clock advanced. Returns what the chooser reads.
    pub fn advance(&mut self, classified: Direction, rendered_frames: f32) -> Direction {
        if classified == Direction::None {
            self.shown = Direction::None;
            return Direction::None;
        }
        if matches!(
            (self.shown, classified),
            (Direction::Left, Direction::Right) | (Direction::Right, Direction::Left)
        ) {
            self.straight_owed = SIDE_STEP_FLIP_STRAIGHT_FRAMES;
        }
        self.shown = if self.straight_owed > 0.0 {
            self.straight_owed = (self.straight_owed - rendered_frames).max(0.0);
            Direction::Forward
        } else {
            classified
        };
        self.shown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestPhase {
    In,

    Loop,

    Out,
}

pub fn rest_animation_id_phase(rest: RestKind, phase: RestPhase) -> Option<DatId> {
    let prefix = match rest {
        RestKind::None => return None,
        RestKind::Sit => b"si",
        RestKind::Heal | RestKind::Kneel => b"rx",
    };
    let digit = match phase {
        RestPhase::In => b'0',
        RestPhase::Loop => b'1',
        RestPhase::Out => b'2',
    };
    Some(DatId::from_name(&[prefix[0], prefix[1], digit, b'?']))
}

pub fn rest_animation_id(rest: RestKind) -> Option<DatId> {
    rest_animation_id_phase(rest, RestPhase::In)
}

pub fn corpse_pose_id() -> DatId {
    DatId::from_str("cor?")
}

/// Mount, pose-type and static-NPC idles outrank `dead` in [`idle_animation_id`],
/// so the collapse asks that resolution rather than re-testing `dead` on its own.
pub fn corpse_pose_selected(inputs: &ActorAnimInputs) -> bool {
    inputs.dead && idle_animation_id(inputs).last() == Some(&corpse_pose_id())
}

pub fn corpse_routine_id() -> DatId {
    DatId::from_str("corp")
}

pub fn death_routine_id() -> DatId {
    DatId::from_str("dead")
}

/// Playback of retail's `dead` model routine: the `ded?` collapse played once,
/// then `cor?` held (PC skeleton DATs 7072 / 10248 / 13424 / 16600 / 19776 /
/// 23176 / 26352, via `ffxi-dat --example dat-routine-stages <id> dead`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeathPhase {
    /// No observation yet, so a first sighting cannot be attributed to a death
    /// that happened in view.
    Unobserved,

    Alive,

    Collapsing {
        remaining: f32,
    },

    Corpse,
}

pub fn next_death_phase(
    prev: DeathPhase,
    dead: bool,
    collapse_frames: f32,
    elapsed_frames: f32,
) -> DeathPhase {
    if !dead {
        return DeathPhase::Alive;
    }

    match prev {
        // Already dead on first sighting (zone-in on a corpse, a KO'd player
        // streaming into range). Inference rather than an observed capture, recorded
        // as such in .agents/skills/retail-observe/references/death-ko-behavior.md:
        // replaying `ded?` would pop the corpse upright to fall over again.
        DeathPhase::Unobserved => DeathPhase::Corpse,
        DeathPhase::Alive => {
            if collapse_frames > 0.0 {
                DeathPhase::Collapsing {
                    remaining: collapse_frames,
                }
            } else {
                DeathPhase::Corpse
            }
        }
        DeathPhase::Collapsing { remaining } => {
            let remaining = remaining - elapsed_frames;
            if remaining <= 0.0 {
                DeathPhase::Corpse
            } else {
                DeathPhase::Collapsing { remaining }
            }
        }
        DeathPhase::Corpse => DeathPhase::Corpse,
    }
}

/// `travel` is the chooser's bucket ([`movement_animation`]); a still actor ignores it.
pub fn selected_animation(inputs: &ActorAnimInputs, travel: Direction) -> SelectedAnimation {
    if inputs.moving {
        let id = movement_animation(inputs, travel)[0];
        SelectedAnimation { id, idle: false }
    } else {
        let id = idle_animation_id(inputs)[0];
        SelectedAnimation { id, idle: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idstr(id: DatId) -> String {
        id.as_str()
    }

    #[test]
    fn idle_states_map_to_base_ids() {
        let mut i = ActorAnimInputs::default();
        assert_eq!(idstr(idle_animation_id(&i)[0]), "idl?");

        i.engage_state = EngageAnimationState::Engaged;
        assert_eq!(idstr(idle_animation_id(&i)[0]), "btl?");
        i.engage_state = EngageAnimationState::NotEngaged;

        i.dead = true;
        assert_eq!(idstr(idle_animation_id(&i)[0]), "cor?");
        i.dead = false;

        i.mount_or_chocobo = true;
        assert_eq!(idstr(idle_animation_id(&i)[0]), "chi?");
        i.mount_or_chocobo = false;

        i.static_npc = true;
        i.has_dft_idle = true;
        assert_eq!(idstr(idle_animation_id(&i)[0]), "dft?");
    }

    #[test]
    fn static_npc_without_dft_idle_falls_through_to_idl() {
        let i = ActorAnimInputs {
            static_npc: true,
            has_dft_idle: false,
            ..Default::default()
        };
        assert_eq!(idstr(idle_animation_id(&i)[0]), "idl?");

        let i = ActorAnimInputs {
            static_npc: true,
            has_dft_idle: false,
            engage_state: EngageAnimationState::Engaged,
            ..Default::default()
        };
        assert_eq!(idstr(idle_animation_id(&i)[0]), "btl?");
    }

    #[test]
    fn rider_uses_pose_type_un_branch() {
        let i = ActorAnimInputs {
            mount_pose_type: Some(3),
            ..Default::default()
        };
        assert_eq!(idstr(idle_animation_id(&i)[0]), "3un?");

        let i = ActorAnimInputs {
            mount_or_chocobo: true,
            mount_pose_type: Some(3),
            ..Default::default()
        };
        assert_eq!(idstr(idle_animation_id(&i)[0]), "chi?");
    }

    #[test]
    fn dead_owned_corpse_falls_through() {
        let i = ActorAnimInputs {
            dead: true,
            owner_is_none: false,
            ..Default::default()
        };
        assert_eq!(idstr(idle_animation_id(&i)[0]), "idl?");

        let i = ActorAnimInputs {
            dead: true,
            owner_is_none: false,
            engage_state: EngageAnimationState::Engaged,
            ..Default::default()
        };
        assert_eq!(idstr(idle_animation_id(&i)[0]), "btl?");

        let i = ActorAnimInputs {
            dead: true,
            owner_is_none: true,
            ..Default::default()
        };
        assert_eq!(idstr(idle_animation_id(&i)[0]), "cor?");
    }

    #[test]
    fn engage_state_battle_idle_classification() {
        for state in [
            EngageAnimationState::Engaged,
            EngageAnimationState::Disengaging,
        ] {
            let i = ActorAnimInputs {
                engage_state: state,
                ..Default::default()
            };
            assert_eq!(idstr(idle_animation_id(&i)[0]), "btl?", "{state:?}");
        }
        for state in [
            EngageAnimationState::NotEngaged,
            EngageAnimationState::Engaging,
        ] {
            let i = ActorAnimInputs {
                engage_state: state,
                ..Default::default()
            };
            assert_eq!(idstr(idle_animation_id(&i)[0]), "idl?", "{state:?}");
        }
    }

    #[test]
    fn mount_takes_priority_over_dead_and_engaged() {
        let i = ActorAnimInputs {
            mount_or_chocobo: true,
            dead: true,
            engage_state: EngageAnimationState::Engaged,
            ..Default::default()
        };
        assert_eq!(idstr(idle_animation_id(&i)[0]), "chi?");
    }

    #[test]
    fn idle_mode_variant_produced() {
        let i = ActorAnimInputs {
            idle_mode: 2,
            ..Default::default()
        };
        let ids = idle_animation_id(&i);

        assert_eq!(idstr(ids[0]), "2dl?");
        assert_eq!(idstr(ids[1]), "idl?");
    }

    #[test]
    fn battle_mode_variant_produced() {
        let i = ActorAnimInputs {
            engage_state: EngageAnimationState::Engaged,
            battle_mode: 3,
            ..Default::default()
        };
        let ids = idle_animation_id(&i);
        assert_eq!(idstr(ids[0]), "3tl?");
        assert_eq!(idstr(ids[1]), "btl?");
    }

    #[test]
    fn dead_mode_variant_produced() {
        let i = ActorAnimInputs {
            dead: true,
            idle_mode: 1,
            ..Default::default()
        };
        let ids = idle_animation_id(&i);
        assert_eq!(idstr(ids[0]), "1cr?");
        assert_eq!(idstr(ids[1]), "cor?");
    }

    #[test]
    fn movement_ids_by_direction() {
        let run = |forward_vel: f32, strafe_vel: f32| {
            let inputs = ActorAnimInputs {
                moving: true,
                forward_vel,
                strafe_vel,
                ..Default::default()
            };
            idstr(movement_animation(&inputs, inputs.travel())[0])
        };

        let walk = |forward_vel: f32, strafe_vel: f32| {
            let inputs = ActorAnimInputs {
                moving: true,
                walking: true,
                forward_vel,
                strafe_vel,
                ..Default::default()
            };
            idstr(movement_animation(&inputs, inputs.travel())[0])
        };
        assert_eq!(walk(0.0, -1.0), "wlk?");
        assert_eq!(walk(1.0, 0.0), "wlk?");
        assert_eq!(
            walk(-1.0, 0.0),
            "mvb?",
            "a walking back step is the back step"
        );

        assert_eq!(run(1.0, 0.0), "run?");
        assert_eq!(run(0.0, 0.0), "run?");
        assert_eq!(run(-0.5, -1.0), "mvl?");
        assert_eq!(run(0.0, 1.0), "mvr?");
        assert_eq!(run(-1.0, 0.0), "mvb?", "backward names the back step");
    }

    #[test]
    fn a_still_actor_has_no_travel_bucket() {
        let still = ActorAnimInputs {
            forward_vel: 0.0,
            strafe_vel: 1.0,
            ..Default::default()
        };
        assert_eq!(still.travel(), Direction::None);
        assert_eq!(
            ActorAnimInputs {
                moving: true,
                ..still
            }
            .travel(),
            Direction::Right
        );
    }

    /// Steps `latch` through `buckets` one rendered frame each and returns what the chooser read.
    fn shown(latch: &mut SideStepFlipLatch, buckets: &[Direction]) -> Vec<Direction> {
        buckets.iter().map(|&b| latch.advance(b, 1.0)).collect()
    }

    #[test]
    fn a_direct_side_step_flip_runs_straight_for_two_frames() {
        use Direction::{Forward, Left, Right};
        let mut latch = SideStepFlipLatch::default();
        assert_eq!(
            shown(&mut latch, &[Left, Left, Right, Right, Right, Right]),
            [Left, Left, Forward, Forward, Right, Right]
        );
        assert_eq!(
            shown(&mut latch, &[Left, Left, Left]),
            [Forward, Forward, Left]
        );
    }

    #[test]
    fn only_a_direct_flip_arms_the_latch() {
        use Direction::{Backward, Forward, Left, Right};
        let mut latch = SideStepFlipLatch::default();
        assert_eq!(
            shown(&mut latch, &[Left, Forward, Right, Backward, Left]),
            [Left, Forward, Right, Backward, Left],
            "a bucket in between is not a flip"
        );
        let mut latch = SideStepFlipLatch::default();
        assert_eq!(
            shown(&mut latch, &[Right, Direction::None, Left, Left]),
            [Right, Direction::None, Left, Left],
            "a still frame in between clears the comparison"
        );
    }

    /// Flipping back while the straight frames run does not extend them; the frame after they end
    /// shows whatever the travel is by then.
    #[test]
    fn a_flip_back_inside_the_window_does_not_extend_it() {
        use Direction::{Forward, Left, Right};
        let mut latch = SideStepFlipLatch::default();
        assert_eq!(
            shown(&mut latch, &[Right, Left, Right, Right]),
            [Right, Forward, Forward, Right]
        );
    }

    /// The window is rendered-frame time, so a faster pose clock shows it for more, shorter frames.
    #[test]
    fn the_straight_window_is_frame_time_not_a_call_count() {
        use Direction::{Forward, Left, Right};
        const HALF_FRAME: f32 = 0.5;
        let mut latch = SideStepFlipLatch::default();
        latch.advance(Left, HALF_FRAME);
        let read: Vec<Direction> = (0..6).map(|_| latch.advance(Right, HALF_FRAME)).collect();
        assert_eq!(read, [Forward, Forward, Forward, Forward, Right, Right]);
    }

    /// A stop inside the window keeps what is still owed: the next moving frame pays it off.
    #[test]
    fn a_stop_inside_the_window_keeps_what_is_owed() {
        use Direction::{Forward, Left, Right};
        let mut latch = SideStepFlipLatch::default();
        assert_eq!(
            shown(&mut latch, &[Left, Right, Direction::None, Right, Right]),
            [Left, Forward, Direction::None, Forward, Right]
        );
    }

    #[test]
    fn movement_direction_thresholds() {
        assert_eq!(movement_direction(1.0, 0.0), Direction::Forward);

        assert_eq!(
            movement_direction(0.25, (1.0f32 - 0.0625).sqrt()),
            Direction::Forward
        );

        assert_eq!(movement_direction(-1.0, 0.0), Direction::Backward);

        assert_eq!(movement_direction(0.0, 1.0), Direction::Right);

        assert_eq!(movement_direction(0.0, -1.0), Direction::Left);

        assert_eq!(movement_direction(0.0, 0.0), Direction::None);
    }

    /// The same law measured against the vector to a target rather than the actor's own facing —
    /// the form a travelling attacker's swing routine is chosen from. Bearing here is +X throughout.
    #[test]
    fn movement_direction_toward_a_target() {
        assert_eq!(
            movement_direction_toward(1.0, 0.0, 1.0, 0.0),
            Direction::Forward
        );
        assert_eq!(
            movement_direction_toward(-1.0, 0.0, 1.0, 0.0),
            Direction::Backward
        );

        // xim takes the lateral axis as bearing × UP = (-b.z, b.x) in this plane: travel along +Z is
        // to the target's right, and that sign is shared with mvl?/mvr? (both read MotionSample).
        assert_eq!(
            movement_direction_toward(0.0, 1.0, 1.0, 0.0),
            Direction::Right
        );
        assert_eq!(
            movement_direction_toward(0.0, -1.0, 1.0, 0.0),
            Direction::Left
        );

        // xim's preference band: cos 0.5 is still Forward, while cos -0.9 is past both thresholds.
        assert_eq!(
            movement_direction_toward(0.5, (1.0f32 - 0.25).sqrt(), 1.0, 0.0),
            Direction::Forward
        );
        assert_eq!(
            movement_direction_toward(-0.9, (1.0f32 - 0.81).sqrt(), 1.0, 0.0),
            Direction::Backward
        );

        assert_eq!(
            movement_direction_toward(0.0, 0.0, 1.0, 0.0),
            Direction::None
        );
        assert_eq!(
            movement_direction_toward(1.0, 0.0, 0.0, 0.0),
            Direction::None
        );
    }

    #[test]
    fn wire_gait_table_is_speed_vs_speed_base() {
        // Both bytes ride the POS head of the 0x0E position update
        // (research/XIClient/src/XIClient/source/Game/Net/Packets/s2c/0x00E.cpp);
        // UpdateSpeed(run) lifts `speed` only, so speed-vs-base is the whole rule.
        let cases = [
            ((40u8, 40), true),
            ((39, 40), true),
            ((50, 40), false),
            ((255, 1), false),
            ((0, 0), true),
            ((1, 0), false),
        ];
        for ((speed, speed_base), walking) in cases {
            assert_eq!(
                wire_walking(speed, speed_base),
                walking,
                "gait for (speed={speed}, base={speed_base})"
            );
        }
    }

    #[test]
    fn running_mode_variant_for_run() {
        let i = ActorAnimInputs {
            moving: true,
            forward_vel: 1.0,
            running_mode: 4,
            ..Default::default()
        };
        let ids = movement_animation(&i, i.travel());
        assert_eq!(idstr(ids[0]), "4un?");
        assert_eq!(idstr(ids[1]), "run?");
    }

    #[test]
    fn walking_mode_variant() {
        let i = ActorAnimInputs {
            moving: true,
            walking: true,
            walking_mode: 5,
            ..Default::default()
        };
        let ids = movement_animation(&i, i.travel());
        assert_eq!(idstr(ids[0]), "5lk?");
        assert_eq!(idstr(ids[1]), "wlk?");
    }

    #[test]
    fn rest_ids() {
        assert!(rest_animation_id(RestKind::None).is_none());
        assert_eq!(idstr(rest_animation_id(RestKind::Sit).unwrap()), "si0?");
        assert_eq!(idstr(rest_animation_id(RestKind::Heal).unwrap()), "rx0?");
        assert_eq!(idstr(rest_animation_id(RestKind::Kneel).unwrap()), "rx0?");
        assert_eq!(idstr(corpse_routine_id()), "corp");
        assert_eq!(idstr(death_routine_id()), "dead");
    }

    #[test]
    fn rest_phase_ids() {
        use RestPhase::{In, Loop, Out};

        assert_eq!(
            idstr(rest_animation_id_phase(RestKind::Sit, In).unwrap()),
            "si0?"
        );
        assert_eq!(
            idstr(rest_animation_id_phase(RestKind::Sit, Loop).unwrap()),
            "si1?"
        );
        assert_eq!(
            idstr(rest_animation_id_phase(RestKind::Sit, Out).unwrap()),
            "si2?"
        );
        assert_eq!(
            idstr(rest_animation_id_phase(RestKind::Kneel, In).unwrap()),
            "rx0?"
        );
        assert_eq!(
            idstr(rest_animation_id_phase(RestKind::Heal, Loop).unwrap()),
            "rx1?"
        );
        assert_eq!(
            idstr(rest_animation_id_phase(RestKind::Kneel, Out).unwrap()),
            "rx2?"
        );
        assert!(rest_animation_id_phase(RestKind::None, In).is_none());
    }

    // The HumeM `dead` routine: `ded?` for 116 half-frames (58 real frames), then `cor?`;
    // timings from the retail PC skeleton DATs via ffxi-dat/examples/dat-routine-stages.rs.
    const HUME_M_COLLAPSE_FRAMES: f32 = 58.0;

    #[test]
    fn collapse_plays_once_then_holds_the_corpse_pose() {
        let mut phase = next_death_phase(DeathPhase::Unobserved, false, 0.0, 0.0);
        assert_eq!(phase, DeathPhase::Alive);

        phase = next_death_phase(phase, true, HUME_M_COLLAPSE_FRAMES, 1.0);
        assert_eq!(
            phase,
            DeathPhase::Collapsing {
                remaining: HUME_M_COLLAPSE_FRAMES
            }
        );

        let mut ticks = 0;
        while matches!(phase, DeathPhase::Collapsing { .. }) {
            phase = next_death_phase(phase, true, HUME_M_COLLAPSE_FRAMES, 1.0);
            ticks += 1;
            assert!(ticks <= HUME_M_COLLAPSE_FRAMES as u32 + 1);
        }
        assert_eq!(ticks, HUME_M_COLLAPSE_FRAMES as u32);
        assert_eq!(phase, DeathPhase::Corpse);

        for _ in 0..600 {
            phase = next_death_phase(phase, true, HUME_M_COLLAPSE_FRAMES, 1.0);
            assert_eq!(phase, DeathPhase::Corpse);
        }
    }

    #[test]
    fn corpse_pose_selected_matches_idle_resolution() {
        let mut i = ActorAnimInputs {
            dead: true,
            ..Default::default()
        };
        assert!(corpse_pose_selected(&i));

        i.idle_mode = 3;
        assert!(corpse_pose_selected(&i));
        i.idle_mode = 0;

        i.owner_is_none = false;
        assert!(!corpse_pose_selected(&i));
        i.owner_is_none = true;

        i.mount_or_chocobo = true;
        assert!(!corpse_pose_selected(&i));
        i.mount_or_chocobo = false;

        i.mount_pose_type = Some(2);
        assert!(!corpse_pose_selected(&i));
        i.mount_pose_type = None;

        i.static_npc = true;
        i.has_dft_idle = true;
        assert!(!corpse_pose_selected(&i));
        i.static_npc = false;
        i.has_dft_idle = false;

        i.dead = false;
        assert!(!corpse_pose_selected(&i));
    }

    #[test]
    fn first_sighting_of_a_corpse_skips_the_collapse() {
        let phase = next_death_phase(DeathPhase::Unobserved, true, HUME_M_COLLAPSE_FRAMES, 1.0);
        assert_eq!(phase, DeathPhase::Corpse);
    }

    #[test]
    fn raise_resets_so_a_later_death_collapses_again() {
        let phase = next_death_phase(DeathPhase::Corpse, false, HUME_M_COLLAPSE_FRAMES, 1.0);
        assert_eq!(phase, DeathPhase::Alive);

        let phase = next_death_phase(phase, true, HUME_M_COLLAPSE_FRAMES, 1.0);
        assert_eq!(
            phase,
            DeathPhase::Collapsing {
                remaining: HUME_M_COLLAPSE_FRAMES
            }
        );
    }

    #[test]
    fn missing_collapse_clip_falls_straight_to_the_corpse_pose() {
        let phase = next_death_phase(DeathPhase::Alive, true, 0.0, 1.0);
        assert_eq!(phase, DeathPhase::Corpse);
    }

    #[test]
    fn fishing_clips_by_phase() {
        assert_eq!(idstr(fishing_clip(0).unwrap().id), "fsh0");
        assert!(fishing_clip(0).unwrap().looping);
        assert!(fishing_clip(1).unwrap().looping);
        for phase in 2u8..=6 {
            let c = fishing_clip(phase).unwrap();
            assert_eq!(idstr(c.id), format!("fsh{phase}"));
            assert!(!c.looping, "resolution phase {phase} must not loop");
        }
        assert!(fishing_clip(7).is_none());
    }

    #[test]
    fn selected_animation_switches_on_moving() {
        let idle = ActorAnimInputs::default();
        let sel = selected_animation(&idle, idle.travel());
        assert_eq!(idstr(sel.id), "idl?");
        assert!(sel.idle);

        let moving = ActorAnimInputs {
            moving: true,
            forward_vel: 1.0,
            ..Default::default()
        };
        let sel = selected_animation(&moving, moving.travel());
        assert_eq!(idstr(sel.id), "run?");
        assert!(!sel.idle);
    }

    fn step(prev: &SpecialPose, status: u8, animationsub: u8) -> SpecialPoseStep {
        next_special_pose(prev, status, animationsub)
    }

    #[test]
    fn special_routine_table_matches_retail() {
        // Retail's table verbatim: sub 4..7 wraps mod-4, which is what absorbs LSB's spawn
        // flag without any client-side masking.
        assert_eq!(special_routine(0), None);
        assert_eq!(special_routine(1), Some(*b"ini1"));
        assert_eq!(special_routine(2), Some(*b"ini2"));
        assert_eq!(special_routine(3), Some(*b"ini3"));
        assert_eq!(special_routine(4), None, "spawn-flagged zero is plain");
        assert_eq!(
            special_routine(5),
            Some(*b"ini1"),
            "spawn flag wraps to ini1"
        );
        assert_eq!(special_routine(6), Some(*b"ini2"));
        assert_eq!(special_routine(7), Some(*b"ini3"));
        // LSB pools ship animationsub 8/16/20 as ordinary spawn values
        // (vendor/server/sql/mob_pools.sql): above the table they wrap to the same 3-bit index.
        for sub in [8u8, 12, 16, 20] {
            assert_eq!(special_routine(sub), None, "sub {sub}");
        }
        for sub in [9u8, 13, 17, 21] {
            assert_eq!(special_routine(sub), Some(*b"ini1"), "sub {sub}");
        }
    }

    #[test]
    fn special_pose_idle_stays_plain() {
        let s = step(&SpecialPose::default(), 0, 0);
        assert_eq!(s.pose.active_routine, None);
        assert_eq!(s.triggered, None);
    }

    #[test]
    fn special_pose_sub_change_triggers_the_named_routine() {
        for (sub, name) in [(1u8, *b"ini1"), (2, *b"ini2"), (3, *b"ini3")] {
            let s = step(&SpecialPose::default(), 0, sub);
            assert_eq!(s.pose.active_routine, Some(name));
            assert_eq!(s.triggered, Some(name));
        }
        let s = step(&SpecialPose::default(), 0, 5);
        assert_eq!(s.pose.active_routine, Some(*b"ini1"));
        assert_eq!(s.triggered, Some(*b"ini1"));
    }

    #[test]
    fn special_pose_spawn_flag_zero_is_plain() {
        // The spawn flag alone (LSB ORs it into animationsub) is a zero selector: no active
        // special, nothing triggered.
        let s = step(&SpecialPose::default(), 0, 0b100);
        assert_eq!(s.pose.active_routine, None);
        assert_eq!(s.triggered, None);
    }

    #[test]
    fn special_pose_hidden_never_triggers() {
        let s = step(&SpecialPose::default(), INVISIBLE_STATUS, 0);
        assert!(s.pose.hidden);
        assert_eq!(s.triggered, None);

        let buried = SpecialPose {
            sub: 1,
            hidden: true,
            active_routine: Some(*b"ini1"),
            slot_held: true,
        };
        let s = step(&buried, INVISIBLE_STATUS, 2);
        assert_eq!(s.triggered, None);
        assert!(s.pose.slot_held, "the buried dig keeps its slot");
    }

    #[test]
    fn special_pose_resurface_runs_init() {
        let buried = SpecialPose {
            sub: 1,
            hidden: true,
            active_routine: Some(*b"ini1"),
            slot_held: true,
        };
        let s = step(&buried, 0, 1);
        assert!(!s.pose.hidden);
        assert_eq!(s.pose.active_routine, Some(*b"init"));
        assert_eq!(s.triggered, Some(*b"init"));
        assert!(
            !s.pose.slot_held,
            "the resurface's fresh actor has no slot; the lock holds the pose"
        );

        let s2 = step(&s.pose, INVISIBLE_STATUS, 1);
        assert_eq!(s2.triggered, None);
        let s3 = step(&s2.pose, 0, 1);
        assert_eq!(s3.triggered, Some(*b"init"));
    }

    #[test]
    fn special_pose_settles_on_sub_clear() {
        let digging = SpecialPose {
            sub: 1,
            hidden: false,
            active_routine: Some(*b"ini1"),
            slot_held: true,
        };
        let s = step(&digging, 0, 0);
        assert_eq!(s.pose.active_routine, Some(*b"ini1"));
        assert!(!s.pose.slot_held);
        assert_eq!(s.triggered, None);

        let settled = SpecialPose {
            sub: 1,
            hidden: false,
            active_routine: Some(*b"init"),
            slot_held: false,
        };
        let held = step(&settled, 0, 1);
        assert_eq!(held.pose.active_routine, Some(*b"init"));
        assert!(!held.pose.slot_held);
        assert_eq!(
            held.triggered, None,
            "the settle window must not re-fire the special"
        );
        let cleared = step(&held.pose, 0, 0);
        assert_eq!(cleared.pose.active_routine, Some(*b"init"));
        assert!(!cleared.pose.slot_held);
    }

    #[test]
    fn special_pose_full_worm_cycle() {
        // The documented LSB worm cycle (mob_controller.cpp) through the generic rule:
        // dig = sub set while visible; buried ~3s; pop-up = explicit POS then status flip;
        // settle ~2s with the sub still set; sub clears.
        let mut pose = SpecialPose::default();

        let s = step(&pose, 0, 0);
        assert_eq!(s.triggered, None);
        pose = s.pose;

        let s = step(&pose, 0, 1);
        assert_eq!(s.triggered, Some(*b"ini1"));
        pose = s.pose;

        let s = step(&pose, 1, 1);
        assert_eq!(s.pose.active_routine, Some(*b"ini1"));
        assert_eq!(s.triggered, None);
        pose = s.pose;

        let s = step(&pose, INVISIBLE_STATUS, 1);
        assert!(s.pose.hidden);
        assert_eq!(s.triggered, None);
        pose = s.pose;

        let s = step(&pose, 0, 1);
        assert!(!s.pose.hidden);
        assert_eq!(s.triggered, Some(*b"init"));
        assert!(!s.pose.slot_held);
        pose = s.pose;

        let s = step(&pose, 0, 0);
        assert_eq!(s.pose.active_routine, Some(*b"init"));
        assert!(!s.pose.slot_held);
        assert_eq!(s.triggered, None);
    }
}
