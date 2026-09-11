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
    /// visible, 'init' when the entity resurfaced (retail's create path). Drives the pose
    /// override until settled or re-triggered; None = plain locomotion.
    pub active_routine: Option<[u8; 4]>,
}

/// LSB `STATUS_TYPE::INVISIBLE` (vendor/server/src/map/entities/baseentity.h): the server hides
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
///   * animationsub changing while the actor is visible plays table[sub] on that model;
///   * a hidden->visible transition runs 'init' (the DAT's load routine).
///     A sub change to zero settles back to locomotion. While hidden nothing triggers: retail has no
///     live actor to run it on, and the resurface replays 'init' instead of re-firing the old special.
///
/// The server (vendor/server/src/map/ai/controllers/mob_controller.cpp) drives the worm cycle:
/// dig sets `animationsub = 1` while still visible, then flips `status -> INVISIBLE` ~3s later;
/// pop-up sends an explicit position update (which carries the still-INVISIBLE status byte) and
/// then flips `status` back to visible with `animationsub` still set for ~2s before it returns to
/// 0. A sub-clear arriving mid-routine does nothing in retail: the routine finishes, there is no
/// early-cancel path; only the pose override settles.
///
/// Our port keeps one hidden actor instead of destroying/rebuilding it (retail destroys on
/// INVISIBLE and constructs a fresh model on resurface), so `hidden` stands in for "actor
/// destroyed" and the resurface's 'init' runs on our kept-hidden actor.
pub fn next_special_pose(prev: &SpecialPose, status: u8, animationsub: u8) -> SpecialPoseStep {
    let hidden = status == INVISIBLE_STATUS;
    let (active_routine, triggered) = if hidden {
        (prev.active_routine, None)
    } else if prev.hidden {
        // Resurfaced: retail constructs a fresh actor and runs its load routine. This wins over
        // any sub change in the same frame: the create path is what retail runs, and the worm's
        // settle window keeps the sub set without re-triggering its special.
        (Some(LOAD_ROUTINE), Some(LOAD_ROUTINE))
    } else if animationsub != prev.sub {
        // Visible and the sub byte changed: retail's change detector plays table[sub]. A zero
        // (or spawn-flagged zero) settles; anything else triggers that model's special routine.
        let routine = special_routine(animationsub);
        (routine, routine)
    } else if special_routine(animationsub).is_none() {
        // Visible with the sub settled to zero: back to locomotion.
        (None, None)
    } else {
        (prev.active_routine, None)
    };
    SpecialPoseStep {
        pose: SpecialPose {
            sub: animationsub,
            hidden,
            active_routine,
        },
        triggered,
    }
}

/// Gait from the wire speed bytes. LSB's UpdateSpeed(run) multiplies `speed` only and never
/// touches animationSpeed (vendor/server/src/map/entities/battleentity.cpp), so a speed byte
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FishingClip {
    pub id: DatId,

    /// `true` for the looping waiting/fighting poses, `false` for the resolution poses
    /// that play once and hold the final frame.
    pub looping: bool,
}

/// Maps a fishing macro-state phase (0..=6) to its `fsh<n>` model clip. Phases:
/// 0=cast/wait, 1=fighting, 2=caught fish, 3=rod break, 4=line break, 5=caught monster,
/// 6=stop/cancel. research/xim Actor.kt (`updateFishingState`).
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

pub fn movement_direction(forward_vel: f32, strafe_vel: f32) -> Direction {
    let speed_sq = forward_vel * forward_vel + strafe_vel * strafe_vel;
    if speed_sq <= 1e-5 {
        return Direction::None;
    }

    let inv = 1.0 / speed_sq.sqrt();

    let cos_angle = forward_vel * inv;

    if cos_angle >= 0.25 {
        Direction::Forward
    } else if cos_angle >= -0.75 {
        let horizontal_cos = strafe_vel * inv;
        if horizontal_cos >= 0.0 {
            Direction::Right
        } else {
            Direction::Left
        }
    } else {
        Direction::Backward
    }
}

pub fn movement_animation(inputs: &ActorAnimInputs) -> Vec<DatId> {
    if inputs.walking {
        return animation_mode_variant(DatId::from_str("wlk?"), inputs.walking_mode, "lk");
    }

    match movement_direction(inputs.forward_vel, inputs.strafe_vel) {
        Direction::None | Direction::Forward => {
            animation_mode_variant(DatId::from_str("run?"), inputs.running_mode, "un")
        }
        Direction::Left => vec![DatId::from_str("mvl?")],
        Direction::Right => vec![DatId::from_str("mvr?")],
        Direction::Backward => vec![DatId::from_str("mvb?")],
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

pub fn selected_animation(inputs: &ActorAnimInputs) -> SelectedAnimation {
    if inputs.moving {
        let id = movement_animation(inputs)[0];
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
        let walk = ActorAnimInputs {
            walking: true,
            ..Default::default()
        };
        assert_eq!(idstr(movement_animation(&walk)[0]), "wlk?");

        let fwd = ActorAnimInputs {
            forward_vel: 1.0,
            ..Default::default()
        };
        assert_eq!(idstr(movement_animation(&fwd)[0]), "run?");

        let none = ActorAnimInputs::default();
        assert_eq!(idstr(movement_animation(&none)[0]), "run?");

        let left = ActorAnimInputs {
            forward_vel: -0.5,
            strafe_vel: -1.0,
            ..Default::default()
        };
        assert_eq!(idstr(movement_animation(&left)[0]), "mvl?");

        let right = ActorAnimInputs {
            forward_vel: 0.0,
            strafe_vel: 1.0,
            ..Default::default()
        };
        assert_eq!(idstr(movement_animation(&right)[0]), "mvr?");

        let back = ActorAnimInputs {
            forward_vel: -1.0,
            strafe_vel: 0.0,
            ..Default::default()
        };
        assert_eq!(idstr(movement_animation(&back)[0]), "mvb?");
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

    #[test]
    fn wire_gait_table_is_speed_vs_speed_base() {
        // (speed, animationSpeed) pairs straight off the 0x0E block. UpdateSpeed(run) writes a
        // higher `speed` and leaves animationSpeed alone, so the comparison is the whole rule:
        // no kind, no base-speed constant.
        let cases = [
            ((40u8, 40), true), // roam: speed at base walks
            ((39, 40), true),   // slower than base still walks
            ((50, 40), false),  // chase: the run factor lifted speed above base
            ((255, 1), false),  // extreme bytes keep the ordering sane
            ((0, 0), true),     // idle bytes: not running
            ((1, 0), false),    // any lift of speed over base is a run
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
            forward_vel: 1.0,
            running_mode: 4,
            ..Default::default()
        };
        let ids = movement_animation(&i);
        assert_eq!(idstr(ids[0]), "4un?");
        assert_eq!(idstr(ids[1]), "run?");
    }

    #[test]
    fn walking_mode_variant() {
        let i = ActorAnimInputs {
            walking: true,
            walking_mode: 5,
            ..Default::default()
        };
        let ids = movement_animation(&i);
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

    // Routine timings dumped from the retail PC skeleton DATs
    // (`dat-routine-stages 7072 dead`): `ded?` for 116 half-frames = 58 real
    // frames, then `cor?`.
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
        let sel = selected_animation(&idle);
        assert_eq!(idstr(sel.id), "idl?");
        assert!(sel.idle);

        let moving = ActorAnimInputs {
            moving: true,
            forward_vel: 1.0,
            ..Default::default()
        };
        let sel = selected_animation(&moving);
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
        // Idle on the surface: no sub, visible -> nothing active, nothing triggered.
        let s = step(&SpecialPose::default(), 0, 0);
        assert_eq!(s.pose.active_routine, None);
        assert_eq!(s.triggered, None);
    }

    #[test]
    fn special_pose_sub_change_triggers_the_named_routine() {
        // A sub change while visible plays table[sub] on the model (change detector).
        for (sub, name) in [(1u8, *b"ini1"), (2, *b"ini2"), (3, *b"ini3")] {
            let s = step(&SpecialPose::default(), 0, sub);
            assert_eq!(s.pose.active_routine, Some(name));
            assert_eq!(s.triggered, Some(name));
        }
        // Spawn-flagged selector: the raw byte indexes the table; no masking.
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
        // First observed already hidden: no live actor in retail, nothing runs.
        let s = step(&SpecialPose::default(), INVISIBLE_STATUS, 0);
        assert!(s.pose.hidden);
        assert_eq!(s.triggered, None);

        // A sub change while hidden also triggers nothing; the resurface decides what plays.
        let buried = SpecialPose {
            sub: 1,
            hidden: true,
            active_routine: Some(*b"ini1"),
        };
        let s = step(&buried, INVISIBLE_STATUS, 2);
        assert_eq!(s.triggered, None);
    }

    #[test]
    fn special_pose_resurface_runs_init() {
        // Hidden -> visible is an actor create in retail: the load routine 'init' runs,
        // whether or not the sub byte changed in the same frame.
        let buried = SpecialPose {
            sub: 1,
            hidden: true,
            active_routine: Some(*b"ini1"),
        };
        let s = step(&buried, 0, 1);
        assert!(!s.pose.hidden);
        assert_eq!(s.pose.active_routine, Some(*b"init"));
        assert_eq!(s.triggered, Some(*b"init"));

        // Re-hidden mid-settle and resurfaced again: 'init' replays (retail rebuilds the
        // actor every time).
        let s2 = step(&s.pose, INVISIBLE_STATUS, 1);
        assert_eq!(s2.triggered, None);
        let s3 = step(&s2.pose, 0, 1);
        assert_eq!(s3.triggered, Some(*b"init"));
    }

    #[test]
    fn special_pose_settles_on_sub_clear() {
        // The server cleared animationsub while visible (engaged mid-dig): the override
        // settles back to locomotion; no routine fires on a clear.
        let digging = SpecialPose {
            sub: 1,
            hidden: false,
            active_routine: Some(*b"ini1"),
        };
        let s = step(&digging, 0, 0);
        assert_eq!(s.pose.active_routine, None);
        assert_eq!(s.triggered, None);

        // Same settle after a resurface's 'init': the settle window holds while the sub is
        // still set, then releases when it clears.
        let settled = SpecialPose {
            sub: 1,
            hidden: false,
            active_routine: Some(*b"init"),
        };
        let held = step(&settled, 0, 1);
        assert_eq!(held.pose.active_routine, Some(*b"init"));
        assert_eq!(
            held.triggered, None,
            "the settle window must not re-fire the special"
        );
        let cleared = step(&held.pose, 0, 0);
        assert_eq!(cleared.pose.active_routine, None);
    }

    #[test]
    fn special_pose_full_worm_cycle() {
        // The documented LSB worm cycle (mob_controller.cpp) through the generic rule:
        // dig = sub set while visible; buried ~3s; pop-up = explicit POS then status flip;
        // settle ~2s with the sub still set; sub clears.
        let mut pose = SpecialPose::default();

        // Idle on the surface.
        let s = step(&pose, 0, 0);
        assert_eq!(s.triggered, None);
        pose = s.pose;

        // Server starts the dig: sub set while still visible -> ini1 fires once.
        let s = step(&pose, 0, 1);
        assert_eq!(s.triggered, Some(*b"ini1"));
        pose = s.pose;

        // Visible dig window: hold the override, no re-fire.
        let s = step(&pose, 1, 1);
        assert_eq!(s.pose.active_routine, Some(*b"ini1"));
        assert_eq!(s.triggered, None);
        pose = s.pose;

        // ~3s later the model is hidden underground: nothing triggers while buried.
        let s = step(&pose, INVISIBLE_STATUS, 1);
        assert!(s.pose.hidden);
        assert_eq!(s.triggered, None);
        pose = s.pose;

        // It surfaces: visible again with the sub still set -> 'init' (the create path),
        // not a re-fire of ini1.
        let s = step(&pose, 0, 1);
        assert!(!s.pose.hidden);
        assert_eq!(s.triggered, Some(*b"init"));
        pose = s.pose;

        // ~2s later the sub clears and it settles back to locomotion.
        let s = step(&pose, 0, 0);
        assert_eq!(s.pose.active_routine, None);
        assert_eq!(s.triggered, None);
    }
}
