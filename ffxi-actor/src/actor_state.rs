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
        }
    }
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
}
