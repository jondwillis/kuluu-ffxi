use std::collections::BTreeMap;

use bevy::input::keyboard::{Key, KeyCode};
use bevy::input::ButtonInput;
use bevy::prelude::Resource;

pub mod presets;

pub use presets::Preset;

#[derive(
    Debug, Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub enum Action {
    MoveForward,
    MoveBackward,

    RotateLeft,
    RotateRight,

    TurnLeft,
    TurnRight,
    StrafeLeft,
    StrafeRight,

    CameraYawLeft,
    CameraYawRight,
    CameraPitchUp,
    CameraPitchDown,

    CameraZoomIn,

    CameraZoomOut,

    ToggleAutorun,
    ToggleLockOn,
    ToggleFirstPerson,

    TogglePassiveCursor,

    CycleTarget,
    ClearTarget,
    TargetSelf,
    TargetParty2,
    TargetParty3,
    TargetParty4,
    TargetParty5,
    TargetParty6,

    ToggleEngage,

    Sit,

    Heal,

    ToggleWalk,

    OpenChat,

    OpenChatCommand,

    OpenMenu,

    ConfirmAction,

    NavUp,
    NavDown,
    NavLeft,
    NavRight,
    NavConfirm,
    NavCancel,

    PageUp,
    PageDown,

    ChatSubmit,
    ChatExit,
    ChatBackspace,
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    Eq,
    PartialEq,
    Hash,
    Ord,
    PartialOrd,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,

    pub super_: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
        super_: false,
    };
    pub const CTRL: Self = Self {
        ctrl: true,
        alt: false,
        shift: false,
        super_: false,
    };
    pub const ALT: Self = Self {
        ctrl: false,
        alt: true,
        shift: false,
        super_: false,
    };
    pub const SHIFT: Self = Self {
        ctrl: false,
        alt: false,
        shift: true,
        super_: false,
    };

    pub fn matches(&self, keys: &ButtonInput<KeyCode>) -> bool {
        let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
        let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
        let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let super_ = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
        self.ctrl == ctrl && self.alt == alt && self.shift == shift && self.super_ == super_
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KeyBind {
    pub key: KeyCode,
    #[serde(default)]
    pub mods: Modifiers,
}

impl KeyBind {
    pub const fn new(key: KeyCode) -> Self {
        Self {
            key,
            mods: Modifiers::NONE,
        }
    }

    pub const fn with(key: KeyCode, mods: Modifiers) -> Self {
        Self { key, mods }
    }
}

#[derive(Resource, Debug, Clone)]
pub struct Bindings {
    map: BTreeMap<Action, KeyBind>,
}

impl Default for Bindings {
    fn default() -> Self {
        Preset::Compact2.bindings()
    }
}

impl Bindings {
    pub fn empty() -> Self {
        Self {
            map: BTreeMap::new(),
        }
    }

    pub fn from_pairs<I: IntoIterator<Item = (Action, KeyBind)>>(iter: I) -> Self {
        Self {
            map: iter.into_iter().collect(),
        }
    }

    pub fn insert(&mut self, action: Action, bind: KeyBind) {
        self.map.insert(action, bind);
    }

    pub fn remove(&mut self, action: Action) -> Option<KeyBind> {
        self.map.remove(&action)
    }

    pub fn get(&self, action: Action) -> Option<KeyBind> {
        self.map.get(&action).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (Action, KeyBind)> + '_ {
        self.map.iter().map(|(a, b)| (*a, *b))
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn pressed(&self, action: Action, keys: &ButtonInput<KeyCode>) -> bool {
        match self.map.get(&action) {
            Some(b) => keys.pressed(b.key) && b.mods.matches(keys),
            None => false,
        }
    }

    pub fn just_pressed(&self, action: Action, keys: &ButtonInput<KeyCode>) -> bool {
        match self.map.get(&action) {
            Some(b) => keys.just_pressed(b.key) && b.mods.matches(keys),
            None => false,
        }
    }

    pub fn matches_logical(&self, action: Action, key: &Key) -> bool {
        let Some(b) = self.map.get(&action) else {
            return false;
        };
        if b.mods != Modifiers::NONE {
            return false;
        }
        nav_keycode_for(key) == Some(b.key)
    }
}

fn nav_keycode_for(key: &Key) -> Option<KeyCode> {
    match key {
        Key::Enter => Some(KeyCode::Enter),
        Key::Escape => Some(KeyCode::Escape),
        Key::Backspace => Some(KeyCode::Backspace),
        Key::Tab => Some(KeyCode::Tab),
        Key::Space => Some(KeyCode::Space),
        Key::ArrowUp => Some(KeyCode::ArrowUp),
        Key::ArrowDown => Some(KeyCode::ArrowDown),
        Key::ArrowLeft => Some(KeyCode::ArrowLeft),
        Key::ArrowRight => Some(KeyCode::ArrowRight),
        Key::PageUp => Some(KeyCode::PageUp),
        Key::PageDown => Some(KeyCode::PageDown),
        Key::Home => Some(KeyCode::Home),
        Key::End => Some(KeyCode::End),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonInput;

    fn pressed_keys(held: &[KeyCode]) -> ButtonInput<KeyCode> {
        let mut input = ButtonInput::<KeyCode>::default();
        for k in held {
            input.press(*k);
        }
        input
    }

    #[test]
    fn pressed_returns_true_when_bound_key_held() {
        let mut b = Bindings::empty();
        b.insert(Action::MoveForward, KeyBind::new(KeyCode::KeyW));
        let keys = pressed_keys(&[KeyCode::KeyW]);
        assert!(b.pressed(Action::MoveForward, &keys));
    }

    #[test]
    fn pressed_returns_false_for_unbound_action() {
        let b = Bindings::empty();
        let keys = pressed_keys(&[KeyCode::KeyW]);
        assert!(!b.pressed(Action::MoveForward, &keys));
    }

    #[test]
    fn just_pressed_requires_modifier_match() {
        let mut b = Bindings::empty();
        b.insert(
            Action::TargetParty2,
            KeyBind::with(KeyCode::Digit2, Modifiers::CTRL),
        );

        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::Digit2);
        assert!(!b.just_pressed(Action::TargetParty2, &keys));

        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::ControlLeft);
        keys.press(KeyCode::Digit2);
        assert!(b.just_pressed(Action::TargetParty2, &keys));
    }

    #[test]
    fn pressed_rejects_extra_modifier() {
        let mut b = Bindings::empty();
        b.insert(Action::MoveForward, KeyBind::new(KeyCode::KeyW));
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::KeyW);
        keys.press(KeyCode::ShiftLeft);
        assert!(!b.pressed(Action::MoveForward, &keys));
    }

    #[test]
    fn matches_logical_resolves_named_keys() {
        let mut b = Bindings::empty();
        b.insert(Action::NavConfirm, KeyBind::new(KeyCode::Enter));
        b.insert(Action::NavCancel, KeyBind::new(KeyCode::Escape));
        b.insert(Action::NavUp, KeyBind::new(KeyCode::ArrowUp));

        assert!(b.matches_logical(Action::NavConfirm, &Key::Enter));
        assert!(b.matches_logical(Action::NavCancel, &Key::Escape));
        assert!(b.matches_logical(Action::NavUp, &Key::ArrowUp));

        assert!(!b.matches_logical(Action::NavConfirm, &Key::Escape));
    }

    #[test]
    fn matches_logical_ignores_printable_chars() {
        let mut b = Bindings::empty();
        b.insert(Action::OpenMenu, KeyBind::new(KeyCode::Minus));
        assert!(!b.matches_logical(Action::OpenMenu, &Key::Character("-".into())));
    }

    #[test]
    fn matches_logical_requires_no_modifiers() {
        let mut b = Bindings::empty();
        b.insert(
            Action::NavConfirm,
            KeyBind::with(KeyCode::Enter, Modifiers::CTRL),
        );

        assert!(!b.matches_logical(Action::NavConfirm, &Key::Enter));
    }

    #[test]
    fn iter_is_deterministic() {
        let pairs = [
            (Action::MoveForward, KeyBind::new(KeyCode::KeyW)),
            (Action::CycleTarget, KeyBind::new(KeyCode::Tab)),
            (Action::OpenMenu, KeyBind::new(KeyCode::Minus)),
        ];
        let b1 = Bindings::from_pairs(pairs.iter().copied());
        let b2 = Bindings::from_pairs(pairs.iter().rev().copied());
        let v1: Vec<_> = b1.iter().collect();
        let v2: Vec<_> = b2.iter().collect();
        assert_eq!(v1, v2);
    }

    #[test]
    fn default_is_compact2() {
        let b = Bindings::default();
        assert_eq!(
            b.get(Action::MoveForward),
            Some(KeyBind::new(KeyCode::KeyW))
        );

        assert_eq!(b.get(Action::RotateLeft), Some(KeyBind::new(KeyCode::KeyQ)));
        assert_eq!(b.get(Action::StrafeLeft), None);
    }
}
