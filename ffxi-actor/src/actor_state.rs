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

/// Burrowing-mob animation phase. Driven by the entity's `status`/`animationsub`
/// transitions (see [`burrow_clip`]); `None` for every non-burrowing actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BurrowPhase {
    /// On the surface and idle, or not a burrowing entity — no clip override.
    None,

    /// Digging into the ground: plays `sp1?` (the DAT's dig-down clip) once and holds
    /// the buried end frame. Retail needs no status byte for this — the held pose is
    /// already underground; `status == INVISIBLE` only destroys the actor.
    DigDown,

    /// Fully underground. The model is hidden by `status == INVISIBLE`, so no clip.
    Underground,

    /// Emerging from the ground: plays `sp0?` (the DAT's pop-up clip) once, then returns
    /// to idle. In retail this moment is a fresh model load running the DAT's `init`
    /// routine; we replay the same routine + clip on our hidden actor instead.
    PopUp,
}

/// Maps a burrow phase to its model clip. FFXI burrowing mobs (tunnel worms and
/// kin) ship two dedicated clips in their model DAT: `sp1?` drives the body down
/// into the ground, `sp0?` raises it back out (verified against ROM/5/64.DAT joint
/// deltas and the retail client's parsed records — F19-F22).
///
/// Retail runs these via the DAT's effect routines. Dig is a sub set on a live
/// actor, which plays `ini1` (Motion sp1? + dirt generators + sound); pop-up is a
/// visible status on an entity with no actor — a fresh model load running `init`
/// (Motion sp0? + dirt generators + sound). We drive the clip directly from the
/// entity's status/animationsub transitions so no per-mob action code is needed.
pub fn burrow_clip(phase: BurrowPhase) -> Option<DatId> {
    match phase {
        BurrowPhase::None | BurrowPhase::Underground => None,
        BurrowPhase::DigDown => Some(DatId::from_str("sp1?")),
        BurrowPhase::PopUp => Some(DatId::from_str("sp0?")),
    }
}

/// LSB `STATUS_TYPE::INVISIBLE` (vendor/server/src/map/entities/baseentity.h):
/// the server hides the model entirely while a burrowing mob is underground.
pub const BURROW_INVISIBLE_STATUS: u8 = 3;

/// Advance a burrowing entity's [`BurrowPhase`] from its previous phase to the new
/// one, given the current `status` byte and raw `animationsub` byte. Pure so it can
/// be unit-tested without a render context.
///
/// The server (vendor/server/src/map/ai/controllers/mob_controller.cpp) drives the
/// cycle: dig-down sets `animationsub = 1` while still visible, then flips
/// `status -> INVISIBLE` ~3s later; pop-up sends an explicit position update (which
/// carries the still-INVISIBLE status byte) and then flips `status` back to visible
/// with `animationsub` still set for ~2s before it returns to 0. The status
/// transition is the reliable discriminator between "digging" and "just surfaced",
/// because `animationsub != 0` is true in both windows.
///
/// Retail semantics (FFXiMain.dll, findings F19-F22): dig = sub set on a live actor,
/// which runs the DAT's `ini1` routine; its clip ends underground and holds — no
/// status byte is needed to hide. `status == INVISIBLE` destroys the actor outright.
/// Pop-up = visible status on an entity with *no* actor: the client constructs a
/// fresh model and runs the load routine (`init`). A sub-clear arriving mid-routine
/// does nothing — the routine finishes; there is no early-cancel path. Our port
/// keeps one hidden actor instead of destroying/rebuilding it, so `Underground`
/// stands in for "actor destroyed" and `DigDown` holding its buried end frame stands
/// in for retail's clip-hold.
///
/// Interruption: if the worm gets engaged mid-cycle, the server's queued sub-clear
/// still fires (LSB runs its action queue every tick regardless of battle state), so
/// the client sees `animationsub` return to 0 while the model is visible. A cleared
/// sub with a visible status means "no burrow state" in any phase — that is how an
/// interrupted dig or pop-up settles back to idle instead of holding its pose.
/// (The dirt/sound routine keeps running out; only the clip selection stops.)
pub fn next_burrow_phase(prev: BurrowPhase, status: u8, animationsub: u8) -> BurrowPhase {
    // Bit 2 (0x04) of the raw byte is a spawn flag LSB ORs in on spawn; mask it so
    // only the bare sub-selector counts as an active effect.
    let sub = animationsub & !0b100;
    match prev {
        BurrowPhase::None => {
            if status == BURROW_INVISIBLE_STATUS {
                BurrowPhase::Underground
            } else if sub != 0 {
                // On the surface with an active sub-animation: starting to dig.
                BurrowPhase::DigDown
            } else {
                BurrowPhase::None
            }
        }
        BurrowPhase::DigDown => {
            if status == BURROW_INVISIBLE_STATUS {
                BurrowPhase::Underground
            } else if sub != 0 {
                // Still in the visible dig window: hold buried. (Mobs serialize their
                // raw status byte, so visible ticks carry UPDATE=1 — a visible+sub
                // state is indistinguishable from "just surfaced" on values alone;
                // what separates them is that the pop-up's explicit position update
                // carries the still-INVISIBLE byte first, which lands us in
                // Underground before the status flip arrives ~250ms later.)
                BurrowPhase::DigDown
            } else {
                // The server cleared the sub while still visible — the dig was aborted
                // (the worm got engaged mid-dig): back to idle, not a ghost holding the
                // buried pose on the surface.
                BurrowPhase::None
            }
        }
        BurrowPhase::Underground => {
            if status == BURROW_INVISIBLE_STATUS {
                BurrowPhase::Underground
            } else {
                // Surfaced: play the pop-up clip.
                BurrowPhase::PopUp
            }
        }
        BurrowPhase::PopUp => {
            if status == BURROW_INVISIBLE_STATUS {
                // The server re-hid the worm mid-settle (it burrows again before the sub
                // clears): back underground, so the next surface transition replays the
                // pop-up clip instead of holding the emerged pose visible.
                BurrowPhase::Underground
            } else if sub != 0 {
                // Still settling after emerging (server keeps animationsub set ~2s).
                BurrowPhase::PopUp
            } else {
                BurrowPhase::None
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ActorAnimInputs {
    pub moving: bool,
    pub walking: bool,

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

    /// Burrowing-mob phase (see [`burrow_clip`]); `None` for non-burrowers. Driven by
    /// the entity's status/animationsub transitions in the render layer.
    pub burrow: BurrowPhase,
}

impl Default for ActorAnimInputs {
    fn default() -> Self {
        ActorAnimInputs {
            moving: false,
            walking: false,
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
            burrow: BurrowPhase::None,
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
/// 6=stop/cancel. research/xim Actor.kt:361 (`updateFishingState`).
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
        return animation_mode_variant(DatId::from_str("cor?"), inputs.idle_mode, "cr");
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

pub fn corpse_routine_id() -> DatId {
    DatId::from_str("corp")
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

    #[test]
    fn burrow_clip_maps_phases() {
        assert!(burrow_clip(BurrowPhase::None).is_none());
        assert!(burrow_clip(BurrowPhase::Underground).is_none());
        assert_eq!(idstr(burrow_clip(BurrowPhase::DigDown).unwrap()), "sp1?");
        assert_eq!(idstr(burrow_clip(BurrowPhase::PopUp).unwrap()), "sp0?");
    }

    #[test]
    fn burrow_none_transitions() {
        // Idle on the surface: no sub, visible -> stays idle.
        assert_eq!(
            next_burrow_phase(BurrowPhase::None, 0, 0),
            BurrowPhase::None
        );
        // Surface with an active sub-animation: start digging down.
        assert_eq!(
            next_burrow_phase(BurrowPhase::None, 0, 1),
            BurrowPhase::DigDown
        );
        // First observed already hidden -> straight to underground (no dig clip).
        assert_eq!(
            next_burrow_phase(BurrowPhase::None, BURROW_INVISIBLE_STATUS, 0),
            BurrowPhase::Underground
        );
        // Status takes priority over sub: an invisible entity is buried even if the
        // server still carries a sub-selector.
        assert_eq!(
            next_burrow_phase(BurrowPhase::None, BURROW_INVISIBLE_STATUS, 1),
            BurrowPhase::Underground
        );
    }

    #[test]
    fn burrow_spawn_flag_is_masked() {
        // Bit 2 (0x04) is a spawn flag LSB ORs in on spawn; it must not read as an
        // active sub-animation. A bare 0x04 therefore means "no effect".
        assert_eq!(
            next_burrow_phase(BurrowPhase::None, 0, 0b100),
            BurrowPhase::None
        );
        assert_eq!(
            next_burrow_phase(BurrowPhase::PopUp, 0, 0b100),
            BurrowPhase::None
        );
        // A real sub-selector combined with the spawn flag still counts as active.
        assert_eq!(
            next_burrow_phase(BurrowPhase::None, 0, 0b101),
            BurrowPhase::DigDown
        );
    }

    #[test]
    fn burrow_digdown_holds_until_hidden() {
        // Still in the visible dig window: hold buried.
        assert_eq!(
            next_burrow_phase(BurrowPhase::DigDown, 0, 1),
            BurrowPhase::DigDown
        );
        // Server flips to INVISIBLE ~3s later -> fully underground.
        assert_eq!(
            next_burrow_phase(BurrowPhase::DigDown, BURROW_INVISIBLE_STATUS, 1),
            BurrowPhase::Underground
        );
    }

    #[test]
    fn burrow_digdown_aborted_by_sub_clear() {
        // The server cleared animationsub while the worm is still visible — the dig was
        // interrupted (engaged mid-dig): back to idle, not a ghost holding the buried pose.
        assert_eq!(
            next_burrow_phase(BurrowPhase::DigDown, 0, 0),
            BurrowPhase::None
        );
        // STATUS_TYPE::UPDATE (1) is the visible status LSB uses for surfaced mobs.
        assert_eq!(
            next_burrow_phase(BurrowPhase::DigDown, 1, 0),
            BurrowPhase::None
        );
    }

    #[test]
    fn burrow_cycle_interrupted_mid_pop() {
        // Dig -> underground -> pop up; the worm gets engaged during the settle window.
        let mut phase = BurrowPhase::None;
        phase = next_burrow_phase(phase, 0, 1);
        assert_eq!(phase, BurrowPhase::DigDown);
        phase = next_burrow_phase(phase, BURROW_INVISIBLE_STATUS, 1);
        assert_eq!(phase, BurrowPhase::Underground);
        // Pop-up starts: visible again, sub still set.
        phase = next_burrow_phase(phase, 0, 1);
        assert_eq!(phase, BurrowPhase::PopUp);
        // The server's queued sub-clear fires ~2s later even mid-battle ("poof"): the
        // worm settles to idle while visible and targetable.
        phase = next_burrow_phase(phase, 0, 0);
        assert_eq!(phase, BurrowPhase::None);
    }

    #[test]
    fn burrow_underground_pops_on_surface() {
        // Still buried: stays underground.
        assert_eq!(
            next_burrow_phase(BurrowPhase::Underground, BURROW_INVISIBLE_STATUS, 0),
            BurrowPhase::Underground
        );
        // Surfaced with the sub still set (~2s settle window) -> pop-up clip.
        assert_eq!(
            next_burrow_phase(BurrowPhase::Underground, 0, 1),
            BurrowPhase::PopUp
        );
        // Surfaced even if the sub already cleared -> still pops up (status is the
        // discriminator, not the sub).
        assert_eq!(
            next_burrow_phase(BurrowPhase::Underground, 0, 0),
            BurrowPhase::PopUp
        );
    }

    #[test]
    fn burrow_popup_settles_to_idle() {
        // Still settling after emerging: hold the pop-up pose.
        assert_eq!(
            next_burrow_phase(BurrowPhase::PopUp, 0, 1),
            BurrowPhase::PopUp
        );
        // Sub cleared -> back to idle.
        assert_eq!(
            next_burrow_phase(BurrowPhase::PopUp, 0, 0),
            BurrowPhase::None
        );
    }

    #[test]
    fn burrow_popup_rehides_to_underground() {
        // Server flips back to INVISIBLE while still settling -> buried again, so the
        // next surface transition replays the pop-up clip.
        assert_eq!(
            next_burrow_phase(BurrowPhase::PopUp, BURROW_INVISIBLE_STATUS, 1),
            BurrowPhase::Underground
        );
    }

    #[test]
    fn burrow_full_cycle() {
        let mut phase = BurrowPhase::None;
        // Idle on the surface.
        assert_eq!(phase, next_burrow_phase(phase, 0, 0));
        // Server starts the dig: sub set while still visible.
        phase = next_burrow_phase(phase, 0, 1);
        assert_eq!(phase, BurrowPhase::DigDown);
        // ~3s later the model is hidden underground.
        phase = next_burrow_phase(phase, BURROW_INVISIBLE_STATUS, 1);
        assert_eq!(phase, BurrowPhase::Underground);
        // It surfaces: visible again with the sub still set for a moment.
        phase = next_burrow_phase(phase, 0, 1);
        assert_eq!(phase, BurrowPhase::PopUp);
        // ~2s later the sub clears and it settles back to idle.
        phase = next_burrow_phase(phase, 0, 0);
        assert_eq!(phase, BurrowPhase::None);
    }
}
