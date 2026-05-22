//! Centralized, data-driven keyboard bindings.
//!
//! Today's [`crate::input_mode::InputMode`] gating decides *which* handler
//! consumes a key; this module decides *which key* runs each handler. Every
//! rebindable verb the client knows about is named by an [`Action`] variant;
//! a [`Bindings`] resource maps actions → physical [`KeyBind`]s. The
//! presets that ship by default live in the [`presets`] submodule.
//!
//! # Why physical KeyCode, never logical Key
//!
//! Storing a logical [`Key`] in the binding would silently break for users
//! on non-US keyboard layouts: rebinding "open menu" to `-` on a US layout
//! and then handing that file to an AZERTY user would either bind by
//! character (the menu opens on the physical-position-of-`-`-on-US, which
//! is a different physical key on AZERTY) or by physical key (then it
//! prints `°` instead of `-`). Neither is right. By storing only
//! [`KeyCode`] (which names the physical key, layout-invariant) we sidestep
//! the ambiguity entirely.
//!
//! Today's [`crate::input_mode::InputMode`] router in
//! `ffxi-client/src/view_native/text_input.rs` matches on logical [`Key`]
//! (`Key::Enter`, `Key::Escape`, arrows, plus `Key::Character("/")` and
//! `Key::Character("-")`). The Character matches are the layout-fragile
//! ones; Stage 2a moves them to a physical-key reader in `input.rs`. The
//! named-key matches (Enter / Escape / Backspace / Arrows / Page*) are
//! layout-stable, and a small [`Bindings::matches_logical`] shim resolves
//! them per-action.
//!
//! # Action sharing across modes (deliberate)
//!
//! [`Action::NavUp`] / [`Action::NavDown`] / [`Action::NavConfirm`] /
//! [`Action::NavCancel`] are shared across Menu, QuickAction, Dialog, and
//! the (Stage 3) PassiveCursor mode. Retail FFXI uses arrows + Enter + Esc
//! for nav across every UI surface; one rebind that affects all of them
//! is the right default. If a future user wants split per-surface bindings
//! (e.g. arrows scroll chat but j/k navigate menus), introduce
//! `ChatScrollUp`/`MenuNavUp` distinct from `NavUp` then — the data layer
//! supports as many distinct actions as we want.

use std::collections::BTreeMap;

use bevy::input::keyboard::{Key, KeyCode};
use bevy::input::ButtonInput;
use bevy::prelude::Resource;

pub mod presets;

pub use presets::Preset;

/// Every rebindable client verb. Universal hard-wired keys (Cmd+Q close,
/// the OS WindowCloseRequested event) are NOT actions — they must work
/// regardless of bindings, so they stay inline in the input handlers.
#[derive(
    Debug, Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub enum Action {
    // ----- Movement (World mode) -----
    MoveForward,
    MoveBackward,
    /// Pure heading rotation in place. Camera yaw stays lock-step so the
    /// camera trails behind the rotated player. No implicit walk
    /// contribution — use [`Action::TurnLeft`]/[`Action::TurnRight`] for
    /// the combined "rotate-while-walking" verb that produces FFXI
    /// classic's orbital motion when held alone in 3rd person.
    RotateLeft,
    RotateRight,
    /// Combined heading-rotation + forward-walk. In 3rd person, when
    /// neither `MoveForward` nor `MoveBackward` is held, this implicitly
    /// drives `forward = 1` so holding the key alone produces orbital
    /// motion (heading rotates each tick, the forward vector sweeps with
    /// it, character traces a circle). In first-person it behaves like
    /// pure rotate — the forward implicit is suppressed since walking-
    /// in-place-while-looking-around isn't useful when the camera is the
    /// player's head.
    TurnLeft,
    TurnRight,
    StrafeLeft,
    StrafeRight,

    // ----- Camera (World mode) -----
    CameraYawLeft,
    CameraYawRight,
    CameraPitchUp,
    CameraPitchDown,
    /// Pull the chase camera in. Retail Compact 1 binds this to `.`.
    /// No-op in [`crate::camera::CameraMode::FirstPerson`].
    CameraZoomIn,
    /// Push the chase camera out. Retail Compact 1 binds this to `,`.
    /// No-op in [`crate::camera::CameraMode::FirstPerson`].
    CameraZoomOut,

    // ----- Mode toggles (always-edge, World mode) -----
    ToggleAutorun,
    ToggleLockOn,
    ToggleFirstPerson,
    /// Stage 3: enter/leave the FFXI-style passive cursor mode (chat focus +
    /// scroll). Toggle is also the exit, like retail.
    TogglePassiveCursor,

    // ----- Targeting (World mode) -----
    CycleTarget,
    ClearTarget,
    TargetSelf,
    TargetParty2,
    TargetParty3,
    TargetParty4,
    TargetParty5,
    TargetParty6,

    // ----- Combat (World mode) -----
    /// Toggle engagement on the current target. If `Target` is set and we
    /// are not currently engaged, dispatches `AgentCommand::Engage` (the
    /// reactor's first tick emits `ActionKind::Attack`/0x01A subkind 0x02).
    /// If already engaged, dispatches `AgentCommand::Cancel` which clears
    /// the reactor goal. The server's auto-attack timer drives subsequent
    /// 0x028 BATTLE2 packets while engaged.
    ToggleEngage,

    // ----- UI activation (World mode) -----
    /// Open chat with empty buffer (default Space).
    OpenChat,
    /// Open chat pre-seeded with `/` (default `/`).
    OpenChatCommand,
    /// Open the minus-key main menu (default `-`).
    OpenMenu,
    /// Quick-action picker / context-aware confirm (default Enter).
    ConfirmAction,

    // ----- Navigation (Menu / QuickAction / Dialog / PassiveCursor) -----
    NavUp,
    NavDown,
    NavLeft,
    NavRight,
    NavConfirm,
    NavCancel,

    // ----- Page nav (PassiveCursor) -----
    PageUp,
    PageDown,

    // ----- Chat editing (Chat mode) -----
    ChatSubmit,
    ChatExit,
    ChatBackspace,
}

/// Modifier-key state encoded as a small struct rather than bitflags so
/// it serializes cleanly to JSON.
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
    /// macOS Command / Windows Super / Linux Meta.
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

    /// Match this requirement against the live keyboard state. Modifier
    /// keys must match *exactly* — a binding without Shift will not fire
    /// when Shift is held, so accidentally-shifted hotkeys don't trigger.
    pub fn matches(&self, keys: &ButtonInput<KeyCode>) -> bool {
        let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
        let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
        let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let super_ = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
        self.ctrl == ctrl && self.alt == alt && self.shift == shift && self.super_ == super_
    }
}

/// One row in the bindings table: which physical key + modifier combo
/// triggers an [`Action`]. Multiple bindings per action are not supported
/// today; if needed, switch [`Bindings::map`] to `BTreeMap<Action, Vec<KeyBind>>`
/// — every read call already centralizes through [`Bindings::pressed`] /
/// [`Bindings::just_pressed`] / [`Bindings::matches_logical`], so the
/// surface change would be small.
#[derive(Debug, Clone, Copy, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KeyBind {
    pub key: KeyCode,
    #[serde(default)]
    pub mods: Modifiers,
}

impl KeyBind {
    /// Plain bind: just a key, no modifiers required.
    pub const fn new(key: KeyCode) -> Self {
        Self {
            key,
            mods: Modifiers::NONE,
        }
    }

    /// Bind with explicit modifiers — e.g. `KeyBind::with(KeyCode::Digit1, Modifiers::CTRL)`
    /// for the future Ctrl+1 macro slot.
    pub const fn with(key: KeyCode, mods: Modifiers) -> Self {
        Self { key, mods }
    }
}

/// The bindings resource consulted by every input system.
///
/// Held as [`BTreeMap`] (not `HashMap`) for two reasons: (1) deterministic
/// iteration order — `/keybinds list` chat output should be stable across
/// runs, and Stage 2c's JSON serialization wants the same property; (2)
/// the table is tiny (~30 actions), so `O(log n)` is irrelevant.
#[derive(Resource, Debug, Clone)]
pub struct Bindings {
    map: BTreeMap<Action, KeyBind>,
}

impl Default for Bindings {
    /// Default = the [`Preset::Compact2`] layout. Matches the current
    /// hard-coded WASD+Q/E behavior so day-zero behavior is identical.
    fn default() -> Self {
        Preset::Compact2.bindings()
    }
}

impl Bindings {
    /// Empty table — useful for tests and for the `Custom` preset's base.
    pub fn empty() -> Self {
        Self {
            map: BTreeMap::new(),
        }
    }

    /// Construct from a list of `(Action, KeyBind)` pairs. Duplicate keys
    /// in the iterator have last-write-wins semantics.
    pub fn from_pairs<I: IntoIterator<Item = (Action, KeyBind)>>(iter: I) -> Self {
        Self {
            map: iter.into_iter().collect(),
        }
    }

    /// Add or replace a binding.
    pub fn insert(&mut self, action: Action, bind: KeyBind) {
        self.map.insert(action, bind);
    }

    /// Remove a binding. Returns the removed value if present.
    pub fn remove(&mut self, action: Action) -> Option<KeyBind> {
        self.map.remove(&action)
    }

    /// Look up the binding for an action, if any.
    pub fn get(&self, action: Action) -> Option<KeyBind> {
        self.map.get(&action).copied()
    }

    /// Iterate `(Action, KeyBind)` pairs in deterministic action-name order.
    pub fn iter(&self) -> impl Iterator<Item = (Action, KeyBind)> + '_ {
        self.map.iter().map(|(a, b)| (*a, *b))
    }

    /// Number of bound actions.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// True if the action's bound key+mods are currently held.
    pub fn pressed(&self, action: Action, keys: &ButtonInput<KeyCode>) -> bool {
        match self.map.get(&action) {
            Some(b) => keys.pressed(b.key) && b.mods.matches(keys),
            None => false,
        }
    }

    /// True for the single frame the action's bound key transitions from
    /// up → down with matching modifiers.
    pub fn just_pressed(&self, action: Action, keys: &ButtonInput<KeyCode>) -> bool {
        match self.map.get(&action) {
            Some(b) => keys.just_pressed(b.key) && b.mods.matches(keys),
            None => false,
        }
    }

    /// True if the given logical [`Key`] event matches the action's binding,
    /// for the small set of named navigation keys (Enter / Escape /
    /// Backspace / Arrows / Page*). Used by the logical-key router in
    /// `text_input.rs` where the event stream surfaces [`Key`] rather than
    /// [`KeyCode`]. Modifiers must be empty — modifier-aware nav keys
    /// aren't a thing in retail FFXI.
    ///
    /// Returns false for actions whose binding key isn't a navigation key
    /// (you can bind NavConfirm to `J`, but `matches_logical` won't see it
    /// — the physical-key path handles those).
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

/// Map a logical named [`Key`] event to the corresponding physical
/// [`KeyCode`] for the named, layout-stable keys we route through the
/// logical-key path. Returns `None` for printable characters (those go
/// through the physical-key path in `input.rs` via `bindings.pressed` /
/// `just_pressed`).
///
/// `Key::Space` is included even though `Key::Character(" ")` would also
/// match Space on most platforms — winit emits `Key::Space` for the
/// physical Space key, and matching on the named variant is more honest.
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

        // Just the key without Ctrl: no fire.
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::Digit2);
        assert!(!b.just_pressed(Action::TargetParty2, &keys));

        // Key + Ctrl: fires.
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::ControlLeft);
        keys.press(KeyCode::Digit2);
        assert!(b.just_pressed(Action::TargetParty2, &keys));
    }

    #[test]
    fn pressed_rejects_extra_modifier() {
        // A binding without Shift should NOT fire when Shift is held —
        // accidentally-shifted hotkeys shouldn't silently match.
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

        // Wrong key for the action.
        assert!(!b.matches_logical(Action::NavConfirm, &Key::Escape));
    }

    #[test]
    fn matches_logical_ignores_printable_chars() {
        // Printable chars are layout-fragile and routed through the
        // physical-key path; matches_logical should refuse them.
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
        // matches_logical never matches modifier-bearing bindings — those
        // must go through the physical path.
        assert!(!b.matches_logical(Action::NavConfirm, &Key::Enter));
    }

    #[test]
    fn iter_is_deterministic() {
        let pairs = vec![
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
        // Q is bound to pure RotateLeft after the A/D=turn reshuffle;
        // StrafeLeft is unbound by default in every shipped preset.
        assert_eq!(b.get(Action::RotateLeft), Some(KeyBind::new(KeyCode::KeyQ)));
        assert_eq!(b.get(Action::StrafeLeft), None);
    }
}
