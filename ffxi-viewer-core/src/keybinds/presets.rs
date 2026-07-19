use bevy::input::keyboard::KeyCode;

use super::{Action, Bindings, KeyBind};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum Preset {
    Compact1,

    #[default]
    Compact2,

    Standard,

    Custom,
}

impl Preset {
    pub const ALL: &'static [Preset] = &[
        Preset::Compact1,
        Preset::Compact2,
        Preset::Standard,
        Preset::Custom,
    ];

    pub fn slug(&self) -> &'static str {
        match self {
            Preset::Compact1 => "compact1",
            Preset::Compact2 => "compact2",
            Preset::Standard => "standard",
            Preset::Custom => "custom",
        }
    }

    pub fn from_slug(s: &str) -> Option<Preset> {
        match s.to_ascii_lowercase().as_str() {
            "compact1" | "c1" => Some(Preset::Compact1),
            "compact2" | "c2" => Some(Preset::Compact2),
            "standard" | "full" => Some(Preset::Standard),
            "custom" => Some(Preset::Custom),
            _ => None,
        }
    }

    pub fn bindings(&self) -> Bindings {
        match self {
            Preset::Compact1 => compact1(),
            Preset::Compact2 => compact2(),
            Preset::Standard => standard(),
            Preset::Custom => compact2(),
        }
    }
}

fn shared() -> Vec<(Action, KeyBind)> {
    vec![
        (Action::OpenChat, KeyBind::new(KeyCode::Space)),
        (Action::OpenChatCommand, KeyBind::new(KeyCode::Slash)),
        (Action::OpenMenu, KeyBind::new(KeyCode::Minus)),
        (Action::ConfirmAction, KeyBind::new(KeyCode::Enter)),
        (Action::ToggleAutorun, KeyBind::new(KeyCode::KeyR)),
        (Action::ToggleLockOn, KeyBind::new(KeyCode::KeyH)),
        (Action::ToggleFirstPerson, KeyBind::new(KeyCode::KeyV)),
        (Action::ToggleWalk, KeyBind::new(KeyCode::KeyZ)),
        (Action::CameraZoomIn, KeyBind::new(KeyCode::Period)),
        (Action::CameraZoomOut, KeyBind::new(KeyCode::Comma)),
        (Action::TogglePassiveCursor, KeyBind::new(KeyCode::Insert)),
        (Action::ToggleHud, KeyBind::new(KeyCode::ScrollLock)),
        (Action::Screenshot, KeyBind::new(KeyCode::PrintScreen)),
        (Action::CycleTarget, KeyBind::new(KeyCode::Tab)),
        (Action::ClearTarget, KeyBind::new(KeyCode::Escape)),
        (Action::SelectActiveWindow, KeyBind::new(KeyCode::KeyF)),
        (Action::TargetSelf, KeyBind::new(KeyCode::F1)),
        (Action::TargetParty2, KeyBind::new(KeyCode::F2)),
        (Action::TargetParty3, KeyBind::new(KeyCode::F3)),
        (Action::TargetParty4, KeyBind::new(KeyCode::F4)),
        (Action::TargetParty5, KeyBind::new(KeyCode::F5)),
        (Action::TargetParty6, KeyBind::new(KeyCode::F6)),
        (Action::NavUp, KeyBind::new(KeyCode::ArrowUp)),
        (Action::NavDown, KeyBind::new(KeyCode::ArrowDown)),
        (Action::NavLeft, KeyBind::new(KeyCode::ArrowLeft)),
        (Action::NavRight, KeyBind::new(KeyCode::ArrowRight)),
        (Action::NavConfirm, KeyBind::new(KeyCode::Enter)),
        (Action::NavCancel, KeyBind::new(KeyCode::Escape)),
        (Action::PageUp, KeyBind::new(KeyCode::PageUp)),
        (Action::PageDown, KeyBind::new(KeyCode::PageDown)),
        (Action::ChatSubmit, KeyBind::new(KeyCode::Enter)),
        (Action::ChatExit, KeyBind::new(KeyCode::Escape)),
        (Action::ChatBackspace, KeyBind::new(KeyCode::Backspace)),
        // Retail fishing: Enter sets the hook, ←/→ answer the arrow prompt,
        // Escape gives up the cast. Modal while fishing, so the overlaps with
        // ConfirmAction/NavLeft/NavRight/NavCancel are intentional.
        (Action::FishingHook, KeyBind::new(KeyCode::Enter)),
        (Action::FishingReelLeft, KeyBind::new(KeyCode::ArrowLeft)),
        (Action::FishingReelRight, KeyBind::new(KeyCode::ArrowRight)),
        (Action::FishingCancel, KeyBind::new(KeyCode::Escape)),
    ]
}

pub fn compact1() -> Bindings {
    let mut pairs = shared();
    pairs.extend([
        (Action::MoveForward, KeyBind::new(KeyCode::KeyW)),
        (Action::MoveBackward, KeyBind::new(KeyCode::KeyS)),
        (Action::TurnLeft, KeyBind::new(KeyCode::KeyA)),
        (Action::TurnRight, KeyBind::new(KeyCode::KeyD)),
        (Action::RotateLeft, KeyBind::new(KeyCode::KeyQ)),
        (Action::RotateRight, KeyBind::new(KeyCode::KeyE)),
        (Action::CameraPitchUp, KeyBind::new(KeyCode::ArrowUp)),
        (Action::CameraPitchDown, KeyBind::new(KeyCode::ArrowDown)),
        (Action::CameraYawLeft, KeyBind::new(KeyCode::ArrowLeft)),
        (Action::CameraYawRight, KeyBind::new(KeyCode::ArrowRight)),
    ]);
    Bindings::from_pairs(pairs)
}

pub fn compact2() -> Bindings {
    let mut pairs = shared();
    pairs.extend([
        (Action::MoveForward, KeyBind::new(KeyCode::KeyW)),
        (Action::MoveBackward, KeyBind::new(KeyCode::KeyS)),
        (Action::TurnLeft, KeyBind::new(KeyCode::KeyA)),
        (Action::TurnRight, KeyBind::new(KeyCode::KeyD)),
        (Action::RotateLeft, KeyBind::new(KeyCode::KeyQ)),
        (Action::RotateRight, KeyBind::new(KeyCode::KeyE)),
        (Action::CameraPitchUp, KeyBind::new(KeyCode::ArrowUp)),
        (Action::CameraPitchDown, KeyBind::new(KeyCode::ArrowDown)),
        (Action::CameraYawLeft, KeyBind::new(KeyCode::ArrowLeft)),
        (Action::CameraYawRight, KeyBind::new(KeyCode::ArrowRight)),
    ]);
    Bindings::from_pairs(pairs)
}

pub fn standard() -> Bindings {
    let mut pairs = shared();
    pairs.extend([
        (Action::MoveForward, KeyBind::new(KeyCode::Numpad8)),
        (Action::MoveBackward, KeyBind::new(KeyCode::Numpad2)),
        (Action::TurnLeft, KeyBind::new(KeyCode::Numpad4)),
        (Action::TurnRight, KeyBind::new(KeyCode::Numpad6)),
        (Action::CameraPitchUp, KeyBind::new(KeyCode::ArrowUp)),
        (Action::CameraPitchDown, KeyBind::new(KeyCode::ArrowDown)),
        (Action::CameraYawLeft, KeyBind::new(KeyCode::ArrowLeft)),
        (Action::CameraYawRight, KeyBind::new(KeyCode::ArrowRight)),
        (Action::ToggleAutorun, KeyBind::new(KeyCode::Numpad7)),
        // Retail full keyboard: "Select active window" is Numpad + (F is the
        // compact-keyboard binding inherited from shared()).
        (Action::SelectActiveWindow, KeyBind::new(KeyCode::NumpadAdd)),
    ]);
    Bindings::from_pairs(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact2_movement_uses_wasd_qe() {
        let b = compact2();
        assert_eq!(
            b.get(Action::MoveForward),
            Some(KeyBind::new(KeyCode::KeyW))
        );
        assert_eq!(
            b.get(Action::MoveBackward),
            Some(KeyBind::new(KeyCode::KeyS))
        );

        assert_eq!(b.get(Action::TurnLeft), Some(KeyBind::new(KeyCode::KeyA)));
        assert_eq!(b.get(Action::TurnRight), Some(KeyBind::new(KeyCode::KeyD)));

        assert_eq!(b.get(Action::RotateLeft), Some(KeyBind::new(KeyCode::KeyQ)));
        assert_eq!(
            b.get(Action::RotateRight),
            Some(KeyBind::new(KeyCode::KeyE))
        );

        assert_eq!(b.get(Action::StrafeLeft), None);
        assert_eq!(b.get(Action::StrafeRight), None);
    }

    #[test]
    fn compact1_omits_strafe() {
        let b = compact1();
        assert_eq!(
            b.get(Action::MoveForward),
            Some(KeyBind::new(KeyCode::KeyW))
        );

        assert_eq!(b.get(Action::TurnLeft), Some(KeyBind::new(KeyCode::KeyA)));
        assert_eq!(b.get(Action::TurnRight), Some(KeyBind::new(KeyCode::KeyD)));
        assert_eq!(b.get(Action::RotateLeft), Some(KeyBind::new(KeyCode::KeyQ)));
        assert_eq!(
            b.get(Action::RotateRight),
            Some(KeyBind::new(KeyCode::KeyE))
        );
        assert_eq!(b.get(Action::StrafeLeft), None);
        assert_eq!(b.get(Action::StrafeRight), None);
    }

    #[test]
    fn standard_uses_numpad_movement() {
        let b = standard();
        assert_eq!(
            b.get(Action::MoveForward),
            Some(KeyBind::new(KeyCode::Numpad8))
        );
        assert_eq!(
            b.get(Action::MoveBackward),
            Some(KeyBind::new(KeyCode::Numpad2))
        );

        assert_eq!(
            b.get(Action::TurnLeft),
            Some(KeyBind::new(KeyCode::Numpad4))
        );
        assert_eq!(
            b.get(Action::TurnRight),
            Some(KeyBind::new(KeyCode::Numpad6))
        );
        assert_eq!(b.get(Action::RotateLeft), None);
        assert_eq!(b.get(Action::RotateRight), None);
        assert_eq!(b.get(Action::StrafeLeft), None);
        assert_eq!(b.get(Action::StrafeRight), None);

        assert_eq!(
            b.get(Action::ToggleAutorun),
            Some(KeyBind::new(KeyCode::Numpad7))
        );
    }

    #[test]
    fn shared_targeting_present_in_every_preset() {
        for preset in [Preset::Compact1, Preset::Compact2, Preset::Standard] {
            let b = preset.bindings();
            assert_eq!(b.get(Action::TargetSelf), Some(KeyBind::new(KeyCode::F1)));
            assert_eq!(b.get(Action::TargetParty6), Some(KeyBind::new(KeyCode::F6)));
            assert_eq!(b.get(Action::CycleTarget), Some(KeyBind::new(KeyCode::Tab)));
            assert_eq!(
                b.get(Action::TogglePassiveCursor),
                Some(KeyBind::new(KeyCode::Insert))
            );
        }
    }

    #[test]
    fn nav_actions_use_arrow_keys_in_every_preset() {
        for preset in [Preset::Compact1, Preset::Compact2, Preset::Standard] {
            let b = preset.bindings();
            assert_eq!(b.get(Action::NavUp), Some(KeyBind::new(KeyCode::ArrowUp)));
            assert_eq!(
                b.get(Action::NavConfirm),
                Some(KeyBind::new(KeyCode::Enter))
            );
            assert_eq!(
                b.get(Action::NavCancel),
                Some(KeyBind::new(KeyCode::Escape))
            );
        }
    }

    #[test]
    fn hud_toggle_and_screenshot_bound_in_every_preset() {
        for preset in [Preset::Compact1, Preset::Compact2, Preset::Standard] {
            let b = preset.bindings();
            assert_eq!(
                b.get(Action::ToggleHud),
                Some(KeyBind::new(KeyCode::ScrollLock))
            );
            assert_eq!(
                b.get(Action::Screenshot),
                Some(KeyBind::new(KeyCode::PrintScreen))
            );
        }
    }

    #[test]
    fn slug_round_trip() {
        for preset in Preset::ALL {
            assert_eq!(Preset::from_slug(preset.slug()), Some(*preset));
        }

        assert_eq!(Preset::from_slug("c1"), Some(Preset::Compact1));
        assert_eq!(Preset::from_slug("c2"), Some(Preset::Compact2));
        assert_eq!(Preset::from_slug("full"), Some(Preset::Standard));

        assert_eq!(Preset::from_slug("COMPACT2"), Some(Preset::Compact2));

        assert_eq!(Preset::from_slug("bogus"), None);
    }

    #[test]
    fn custom_falls_back_to_compact2_base() {
        let custom = Preset::Custom.bindings();
        let compact2 = Preset::Compact2.bindings();
        let c1: Vec<_> = custom.iter().collect();
        let c2: Vec<_> = compact2.iter().collect();
        assert_eq!(c1, c2);
    }
}
