use std::collections::BTreeMap;

use bevy::input::keyboard::{Key, KeyCode};
use bevy::input::ButtonInput;
use bevy::prelude::Resource;

pub mod links;
pub mod pad;
pub mod presets;

pub use pad::{PadAction, PadBindings};
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

    ToggleHud,
    Screenshot,

    CycleTarget,
    ClearTarget,
    TargetSelf,
    TargetParty2,
    TargetParty3,
    TargetParty4,
    TargetParty5,
    TargetParty6,

    SelectActiveWindow,

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

    // Fishing mini-game inputs. Modal: consumed only while the self player is
    // fishing, so they may share keys with nav/camera actions (same precedent
    // as NavUp/CameraPitchUp both living on ArrowUp).
    FishingHook,
    FishingReelLeft,
    FishingReelRight,
    FishingCancel,
}

impl Action {
    /// kuluu's Controls-page name for the action (bead kuluu-o9qt7.26.1). The
    /// page lists one row per entry of the live store, so every variant needs a
    /// name — no wildcard arm, and adding an action must add its label here.
    /// Retail's own key-assignment rows are numeric ids law'd in
    /// [`crate::hud::controls_law`]; kuluu deliberately does NOT map them onto
    /// these names (only the Confirm/Cancel link groups in the `links` module
    /// overlap), so the page is a kuluu-only surface (page 3).
    pub const fn display_name(&self) -> &'static str {
        match self {
            Action::MoveForward => "Move forward",
            Action::MoveBackward => "Move backward",
            Action::RotateLeft => "Rotate left",
            Action::RotateRight => "Rotate right",
            Action::TurnLeft => "Turn left",
            Action::TurnRight => "Turn right",
            Action::StrafeLeft => "Strafe left",
            Action::StrafeRight => "Strafe right",
            Action::CameraYawLeft => "Camera pan left",
            Action::CameraYawRight => "Camera pan right",
            Action::CameraPitchUp => "Camera up",
            Action::CameraPitchDown => "Camera down",
            Action::CameraZoomIn => "Zoom in",
            Action::CameraZoomOut => "Zoom out",
            Action::ToggleAutorun => "Autorun",
            Action::ToggleLockOn => "Lock on",
            Action::ToggleFirstPerson => "First person",
            Action::TogglePassiveCursor => "Passive cursor",
            Action::ToggleHud => "Hide HUD",
            Action::Screenshot => "Screenshot",
            Action::CycleTarget => "Cycle target",
            Action::ClearTarget => "Clear target",
            Action::TargetSelf => "Target self",
            Action::TargetParty2 => "Target party 2",
            Action::TargetParty3 => "Target party 3",
            Action::TargetParty4 => "Target party 4",
            Action::TargetParty5 => "Target party 5",
            Action::TargetParty6 => "Target party 6",
            Action::SelectActiveWindow => "Active window",
            Action::Sit => "Sit",
            Action::Heal => "Heal",
            Action::ToggleWalk => "Walk / run",
            Action::OpenChat => "Open chat",
            Action::OpenChatCommand => "Chat command",
            Action::OpenMenu => "Open menu",
            Action::ConfirmAction => "Confirm",
            Action::NavUp => "Cursor up",
            Action::NavDown => "Cursor down",
            Action::NavLeft => "Cursor left",
            Action::NavRight => "Cursor right",
            Action::NavConfirm => "Menu confirm",
            Action::NavCancel => "Menu cancel",
            Action::PageUp => "Page up",
            Action::PageDown => "Page down",
            Action::ChatSubmit => "Send chat",
            Action::ChatExit => "Leave chat",
            Action::ChatBackspace => "Chat erase",
            Action::FishingHook => "Fishing: hook",
            Action::FishingReelLeft => "Fishing: reel left",
            Action::FishingReelRight => "Fishing: reel right",
            Action::FishingCancel => "Fishing: cancel",
        }
    }
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

    /// Match against the physical key code of a keyboard event. Needed where a
    /// bind lives on a non-letter printable (e.g. CameraZoomIn/Out on
    /// Period/Comma), which `matches_logical` deliberately never resolves.
    pub fn matches_keycode(&self, action: Action, key_code: KeyCode) -> bool {
        self.map
            .get(&action)
            .is_some_and(|b| b.mods == Modifiers::NONE && b.key == key_code)
    }

    /// Short display name for the key bound to `action`, for UI hints like
    /// "Sort  [F]". Falls back to `None` when the action is unbound or the
    /// bound key has no label yet.
    pub fn key_label(&self, action: Action) -> Option<&'static str> {
        let b = self.map.get(&action)?;
        if b.mods != Modifiers::NONE {
            return None;
        }
        keycode_label(b.key)
    }
}

/// Inverse of [`nav_keycode_for`] over the same fixed table: the `Key` a
/// synthesized `KeyboardInput` must carry so `matches_logical` resolves it
/// back to `key`. `None` means the key is outside the navigable set and no
/// event should be synthesized for it (kuluu-obha: a silent fallback here
/// misdispatched pad buttons after a rebind).
pub fn logical_key_for(key: KeyCode) -> Option<Key> {
    let named = match key {
        KeyCode::Enter => Some(Key::Enter),
        KeyCode::Escape => Some(Key::Escape),
        KeyCode::Backspace => Some(Key::Backspace),
        KeyCode::Tab => Some(Key::Tab),
        KeyCode::Space => Some(Key::Space),
        KeyCode::ArrowUp => Some(Key::ArrowUp),
        KeyCode::ArrowDown => Some(Key::ArrowDown),
        KeyCode::ArrowLeft => Some(Key::ArrowLeft),
        KeyCode::ArrowRight => Some(Key::ArrowRight),
        KeyCode::PageUp => Some(Key::PageUp),
        KeyCode::PageDown => Some(Key::PageDown),
        KeyCode::Home => Some(Key::Home),
        KeyCode::End => Some(Key::End),
        KeyCode::NumpadAdd => Some(Key::Character("+".into())),
        _ => None,
    };
    named.or_else(|| {
        keycode_letter(key).map(|c| Key::Character(c.to_ascii_lowercase().to_string().into()))
    })
}

fn keycode_letter(key: KeyCode) -> Option<char> {
    ('A'..='Z').find(|c| letter_keycode(*c) == Some(key))
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
        // Letter keys arrive as printable characters in logical key events.
        // Resolve single ASCII letters so letter-bound actions (e.g. the
        // window-change key, F in the compact presets) work inside menus.
        // Other printable characters stay unmapped on purpose: symbol-bound
        // actions like OpenMenu ("-") must not fire from text-like input.
        Key::Character(s) => {
            let mut chars = s.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return None;
            };
            // "+" resolves to Numpad + so the Standard preset's retail
            // window-change key works inside menus too.
            if c == '+' {
                return Some(KeyCode::NumpadAdd);
            }
            letter_keycode(c.to_ascii_uppercase())
        }
        _ => None,
    }
}

fn letter_keycode(c: char) -> Option<KeyCode> {
    Some(match c {
        'A' => KeyCode::KeyA,
        'B' => KeyCode::KeyB,
        'C' => KeyCode::KeyC,
        'D' => KeyCode::KeyD,
        'E' => KeyCode::KeyE,
        'F' => KeyCode::KeyF,
        'G' => KeyCode::KeyG,
        'H' => KeyCode::KeyH,
        'I' => KeyCode::KeyI,
        'J' => KeyCode::KeyJ,
        'K' => KeyCode::KeyK,
        'L' => KeyCode::KeyL,
        'M' => KeyCode::KeyM,
        'N' => KeyCode::KeyN,
        'O' => KeyCode::KeyO,
        'P' => KeyCode::KeyP,
        'Q' => KeyCode::KeyQ,
        'R' => KeyCode::KeyR,
        'S' => KeyCode::KeyS,
        'T' => KeyCode::KeyT,
        'U' => KeyCode::KeyU,
        'V' => KeyCode::KeyV,
        'W' => KeyCode::KeyW,
        'X' => KeyCode::KeyX,
        'Y' => KeyCode::KeyY,
        'Z' => KeyCode::KeyZ,
        _ => return None,
    })
}

fn keycode_label(key: KeyCode) -> Option<&'static str> {
    Some(match key {
        KeyCode::KeyA => "A",
        KeyCode::KeyB => "B",
        KeyCode::KeyC => "C",
        KeyCode::KeyD => "D",
        KeyCode::KeyE => "E",
        KeyCode::KeyF => "F",
        KeyCode::KeyG => "G",
        KeyCode::KeyH => "H",
        KeyCode::KeyI => "I",
        KeyCode::KeyJ => "J",
        KeyCode::KeyK => "K",
        KeyCode::KeyL => "L",
        KeyCode::KeyM => "M",
        KeyCode::KeyN => "N",
        KeyCode::KeyO => "O",
        KeyCode::KeyP => "P",
        KeyCode::KeyQ => "Q",
        KeyCode::KeyR => "R",
        KeyCode::KeyS => "S",
        KeyCode::KeyT => "T",
        KeyCode::KeyU => "U",
        KeyCode::KeyV => "V",
        KeyCode::KeyW => "W",
        KeyCode::KeyX => "X",
        KeyCode::KeyY => "Y",
        KeyCode::KeyZ => "Z",
        KeyCode::Enter => "Enter",
        KeyCode::Escape => "Esc",
        KeyCode::Backspace => "Bksp",
        KeyCode::Tab => "Tab",
        KeyCode::Space => "Space",
        KeyCode::ArrowUp => "Up",
        KeyCode::ArrowDown => "Down",
        KeyCode::ArrowLeft => "Left",
        KeyCode::ArrowRight => "Right",
        KeyCode::PageUp => "PgUp",
        KeyCode::PageDown => "PgDn",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::Insert => "Ins",
        KeyCode::Delete => "Del",
        KeyCode::Minus => "-",
        KeyCode::NumpadAdd => "Num+",
        KeyCode::Slash => "/",
        KeyCode::Period => ".",
        KeyCode::Comma => ",",
        // The presets bind F1..F6 (party targets) and the HUD keys, so the
        // Controls page needs their names too (bead kuluu-o9qt7.26.1).
        KeyCode::F1 => "F1",
        KeyCode::F2 => "F2",
        KeyCode::F3 => "F3",
        KeyCode::F4 => "F4",
        KeyCode::F5 => "F5",
        KeyCode::F6 => "F6",
        KeyCode::F7 => "F7",
        KeyCode::F8 => "F8",
        KeyCode::F9 => "F9",
        KeyCode::F10 => "F10",
        KeyCode::F11 => "F11",
        KeyCode::F12 => "F12",
        KeyCode::ScrollLock => "ScrLk",
        KeyCode::PrintScreen => "PrtSc",
        _ => return None,
    })
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
    fn matches_keycode_resolves_symbol_binds_logical_never_matches() {
        let b = Bindings::default();

        assert!(b.matches_keycode(Action::CameraZoomIn, KeyCode::Period));
        assert!(b.matches_keycode(Action::CameraZoomOut, KeyCode::Comma));
        assert!(!b.matches_keycode(Action::CameraZoomIn, KeyCode::Comma));

        assert!(!b.matches_logical(Action::CameraZoomIn, &Key::Character(".".into())));
        assert!(!b.matches_logical(Action::CameraZoomOut, &Key::Character(",".into())));
    }

    #[test]
    fn matches_keycode_requires_no_modifiers() {
        let mut b = Bindings::empty();
        b.insert(
            Action::CameraZoomIn,
            KeyBind::with(KeyCode::Period, Modifiers::CTRL),
        );
        assert!(!b.matches_keycode(Action::CameraZoomIn, KeyCode::Period));
        assert!(!b.matches_keycode(Action::CameraZoomOut, KeyCode::Period));
    }

    #[test]
    fn matches_logical_resolves_letter_chars() {
        let mut b = Bindings::empty();
        b.insert(Action::SelectActiveWindow, KeyBind::new(KeyCode::KeyF));

        assert!(b.matches_logical(Action::SelectActiveWindow, &Key::Character("f".into())));
        assert!(b.matches_logical(Action::SelectActiveWindow, &Key::Character("F".into())));

        assert!(!b.matches_logical(Action::SelectActiveWindow, &Key::Character("g".into())));
        assert!(!b.matches_logical(Action::SelectActiveWindow, &Key::Character("ff".into())));
    }

    #[test]
    fn key_label_reports_bound_key() {
        let mut b = Bindings::empty();
        b.insert(Action::SelectActiveWindow, KeyBind::new(KeyCode::KeyF));
        b.insert(Action::PageDown, KeyBind::new(KeyCode::PageDown));

        assert_eq!(b.key_label(Action::SelectActiveWindow), Some("F"));
        assert_eq!(b.key_label(Action::PageDown), Some("PgDn"));
        assert_eq!(b.key_label(Action::PageUp), None);
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

    /// kuluu-obha guard: `logical_key_for` and `nav_keycode_for` must share
    /// one table, so a synthesized event for any bound navigable key resolves
    /// back to the same key — and unmappable keys synthesize nothing instead
    /// of falling back to Enter.
    #[test]
    fn logical_key_for_roundtrips_the_nav_table() {
        let mut covered = 0;
        let named = [
            KeyCode::Enter,
            KeyCode::Escape,
            KeyCode::Backspace,
            KeyCode::Tab,
            KeyCode::Space,
            KeyCode::ArrowUp,
            KeyCode::ArrowDown,
            KeyCode::ArrowLeft,
            KeyCode::ArrowRight,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::NumpadAdd,
        ];
        for kc in named {
            let key = logical_key_for(kc).expect("named nav key must map");
            assert_eq!(nav_keycode_for(&key), Some(kc), "roundtrip for {kc:?}");
            covered += 1;
        }
        for c in 'A'..='Z' {
            let kc = letter_keycode(c).unwrap();
            let key = logical_key_for(kc).expect("letter must map");
            assert_eq!(nav_keycode_for(&key), Some(kc), "roundtrip for {kc:?}");
            covered += 1;
        }
        assert_eq!(covered, 14 + 26);

        assert_eq!(logical_key_for(KeyCode::Minus), None);
        assert_eq!(logical_key_for(KeyCode::F1), None);
        assert_eq!(logical_key_for(KeyCode::PrintScreen), None);
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
