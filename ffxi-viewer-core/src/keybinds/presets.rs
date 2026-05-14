//! The three retail FFXI keyboard layouts, plus a `Custom` placeholder for
//! user-overridden bindings.
//!
//! Reference: SE forum threads + FFXI Online docs (FFXIclopedia 403's
//! anonymous fetches). The mappings here favor *retail behavior* where it
//! conflicts with our prior hard-coded set; the prior set was loosely
//! "Compact 2" so most rows are unchanged.
//!
//! When a key has different meaning in different [`crate::input_mode::InputMode`]s
//! (e.g. Escape = ClearTarget in World, NavCancel in Menu) it appears in
//! the table TWICE under different actions — there's no conflict because
//! the active mode picks which handler runs.

use bevy::input::keyboard::KeyCode;

use super::{Action, Bindings, KeyBind};

/// Which retail-style layout the user has selected. `Custom` means the
/// bindings on disk override the named preset — the file is the source of
/// truth.
#[derive(
    Debug, Clone, Copy, Eq, PartialEq, Default,
    serde::Serialize, serde::Deserialize,
)]
pub enum Preset {
    /// WASD movement, no strafe, arrows for camera. Most common
    /// keyboard-only retail layout.
    Compact1,
    /// Compact 1 + Q/E strafe. Closest to today's hard-coded behavior.
    /// Default for new installs.
    #[default]
    Compact2,
    /// Numpad-driven movement (8/4/6/2), real arrows for camera. Retail's
    /// original "Full Keyboard" preset; rare on modern laptops.
    Standard,
    /// User-edited bindings; loaded from disk. Stage 2c (`keybinds_store`)
    /// is the source of truth when this variant is active.
    Custom,
}

impl Preset {
    /// All-presets enumeration — useful for `/keybinds list` and tests.
    pub const ALL: &'static [Preset] = &[
        Preset::Compact1,
        Preset::Compact2,
        Preset::Standard,
        Preset::Custom,
    ];

    /// Stable identifier suitable for the on-disk JSON `preset` field and
    /// the `/keybinds preset <id>` slash command. Lowercase so the slash
    /// command parser doesn't have to canonicalize.
    pub fn slug(&self) -> &'static str {
        match self {
            Preset::Compact1 => "compact1",
            Preset::Compact2 => "compact2",
            Preset::Standard => "standard",
            Preset::Custom => "custom",
        }
    }

    /// Parse a slug. Accepts the canonical lowercase form plus a couple of
    /// common alternates (`c1` / `c2`) — the slash command is for humans.
    pub fn from_slug(s: &str) -> Option<Preset> {
        match s.to_ascii_lowercase().as_str() {
            "compact1" | "c1" => Some(Preset::Compact1),
            "compact2" | "c2" => Some(Preset::Compact2),
            "standard" | "full" => Some(Preset::Standard),
            "custom" => Some(Preset::Custom),
            _ => None,
        }
    }

    /// The default [`Bindings`] for this preset. `Custom` returns the
    /// `Compact2` bindings as a base — Stage 2c overlays user overrides
    /// on top.
    pub fn bindings(&self) -> Bindings {
        match self {
            Preset::Compact1 => compact1(),
            Preset::Compact2 => compact2(),
            Preset::Standard => standard(),
            Preset::Custom => compact2(),
        }
    }
}

/// Bindings shared across all presets — chat editing keys, nav keys,
/// universal targeting, mode toggles. These are layout-stable and have
/// no retail variant.
fn shared() -> Vec<(Action, KeyBind)> {
    vec![
        // ----- UI activation -----
        (Action::OpenChat, KeyBind::new(KeyCode::Space)),
        (Action::OpenChatCommand, KeyBind::new(KeyCode::Slash)),
        (Action::OpenMenu, KeyBind::new(KeyCode::Minus)),
        (Action::ConfirmAction, KeyBind::new(KeyCode::Enter)),
        // ----- Mode toggles -----
        (Action::ToggleAutorun, KeyBind::new(KeyCode::KeyR)),
        (Action::ToggleLockOn, KeyBind::new(KeyCode::KeyH)),
        // Retail FFXI binds first-person toggle to `V` in the Compact
        // layouts. We use the same default across every preset so
        // muscle memory carries between layouts.
        (Action::ToggleFirstPerson, KeyBind::new(KeyCode::KeyV)),
        // Retail Compact 1 zoom: `.` in, `,` out. Suppressed by the
        // dispatcher when the camera is in FirstPerson (no chase
        // distance to step). `Comma`/`Period` are the unshifted
        // KeyCodes for `,`/`.`.
        (Action::CameraZoomIn, KeyBind::new(KeyCode::Period)),
        (Action::CameraZoomOut, KeyBind::new(KeyCode::Comma)),
        // Stage 3 introduces the PassiveCursor handler. Insert is unbound
        // in retail (Scroll Lock is hide-HUD per research, not the focus
        // toggle) and unbound in our prior set, so it's collision-free.
        (Action::TogglePassiveCursor, KeyBind::new(KeyCode::Insert)),
        // ----- Targeting -----
        (Action::CycleTarget, KeyBind::new(KeyCode::Tab)),
        (Action::ClearTarget, KeyBind::new(KeyCode::Escape)),
        (Action::TargetSelf, KeyBind::new(KeyCode::F1)),
        (Action::TargetParty2, KeyBind::new(KeyCode::F2)),
        (Action::TargetParty3, KeyBind::new(KeyCode::F3)),
        (Action::TargetParty4, KeyBind::new(KeyCode::F4)),
        (Action::TargetParty5, KeyBind::new(KeyCode::F5)),
        (Action::TargetParty6, KeyBind::new(KeyCode::F6)),
        // ----- Navigation (any cursor mode) -----
        (Action::NavUp, KeyBind::new(KeyCode::ArrowUp)),
        (Action::NavDown, KeyBind::new(KeyCode::ArrowDown)),
        (Action::NavLeft, KeyBind::new(KeyCode::ArrowLeft)),
        (Action::NavRight, KeyBind::new(KeyCode::ArrowRight)),
        (Action::NavConfirm, KeyBind::new(KeyCode::Enter)),
        (Action::NavCancel, KeyBind::new(KeyCode::Escape)),
        (Action::PageUp, KeyBind::new(KeyCode::PageUp)),
        (Action::PageDown, KeyBind::new(KeyCode::PageDown)),
        // ----- Chat editing -----
        (Action::ChatSubmit, KeyBind::new(KeyCode::Enter)),
        (Action::ChatExit, KeyBind::new(KeyCode::Escape)),
        (Action::ChatBackspace, KeyBind::new(KeyCode::Backspace)),
    ]
}

/// Compact 1: WASD + arrow-key camera. No Q/E strafe. Retail's original
/// "compact" mapping — the most common keyboard-only retail layout.
pub fn compact1() -> Bindings {
    let mut pairs = shared();
    pairs.extend([
        // Movement (no strafe).
        (Action::MoveForward, KeyBind::new(KeyCode::KeyW)),
        (Action::MoveBackward, KeyBind::new(KeyCode::KeyS)),
        (Action::RotateLeft, KeyBind::new(KeyCode::KeyA)),
        (Action::RotateRight, KeyBind::new(KeyCode::KeyD)),
        // Camera on real arrows.
        (Action::CameraPitchUp, KeyBind::new(KeyCode::ArrowUp)),
        (Action::CameraPitchDown, KeyBind::new(KeyCode::ArrowDown)),
        (Action::CameraYawLeft, KeyBind::new(KeyCode::ArrowLeft)),
        (Action::CameraYawRight, KeyBind::new(KeyCode::ArrowRight)),
    ]);
    Bindings::from_pairs(pairs)
}

/// Compact 2: Compact 1 + Q/E strafe + (in retail) mouse-wheel zoom.
/// Mouse-wheel zoom isn't wired in our viewer; the only delta from
/// Compact 1 here is the strafe pair. Default preset.
pub fn compact2() -> Bindings {
    let mut pairs = shared();
    pairs.extend([
        (Action::MoveForward, KeyBind::new(KeyCode::KeyW)),
        (Action::MoveBackward, KeyBind::new(KeyCode::KeyS)),
        (Action::RotateLeft, KeyBind::new(KeyCode::KeyA)),
        (Action::RotateRight, KeyBind::new(KeyCode::KeyD)),
        (Action::StrafeLeft, KeyBind::new(KeyCode::KeyQ)),
        (Action::StrafeRight, KeyBind::new(KeyCode::KeyE)),
        (Action::CameraPitchUp, KeyBind::new(KeyCode::ArrowUp)),
        (Action::CameraPitchDown, KeyBind::new(KeyCode::ArrowDown)),
        (Action::CameraYawLeft, KeyBind::new(KeyCode::ArrowLeft)),
        (Action::CameraYawRight, KeyBind::new(KeyCode::ArrowRight)),
    ]);
    Bindings::from_pairs(pairs)
}

/// Standard ("Full Keyboard"): retail's original numpad-driven layout.
/// Rare on modern hardware — most laptops without a true numpad will see
/// the OS report `ArrowUp` instead of `Numpad8` when NumLock is off, so
/// this preset will partially "not work" until the user toggles NumLock.
/// We ship it for retail parity; the user's manual verification step
/// covers both NumLock states.
///
/// caveat: NumLock — `Numpad*` codes only fire on a true numpad with
/// NumLock on. F-key targeting and `Tab`/`Esc`/`Enter` work either way.
pub fn standard() -> Bindings {
    let mut pairs = shared();
    pairs.extend([
        (Action::MoveForward, KeyBind::new(KeyCode::Numpad8)),
        (Action::MoveBackward, KeyBind::new(KeyCode::Numpad2)),
        (Action::RotateLeft, KeyBind::new(KeyCode::Numpad4)),
        (Action::RotateRight, KeyBind::new(KeyCode::Numpad6)),
        // Standard has no strafe — Q/E unbound. Camera lives on the real
        // arrow keys (NavUp/NavDown still bound via shared(), so menus
        // navigate as expected).
        (Action::CameraPitchUp, KeyBind::new(KeyCode::ArrowUp)),
        (Action::CameraPitchDown, KeyBind::new(KeyCode::ArrowDown)),
        (Action::CameraYawLeft, KeyBind::new(KeyCode::ArrowLeft)),
        (Action::CameraYawRight, KeyBind::new(KeyCode::ArrowRight)),
        // Numpad 7 = autorun in retail Standard. Overrides the shared `R`.
        (Action::ToggleAutorun, KeyBind::new(KeyCode::Numpad7)),
        // TODO: cross-reference retail for Standard's exact rest /
        // zoom-in / zoom-out keys (research turned up Numpad 9/3 zoom +
        // NumpadMultiply rest, but our viewer has no zoom action and
        // no rest action yet).
    ]);
    Bindings::from_pairs(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spot-check a handful of actions per preset against retail expectations.
    /// Not exhaustive — full coverage would just re-state the constructor.
    #[test]
    fn compact2_movement_uses_wasd_qe() {
        let b = compact2();
        assert_eq!(b.get(Action::MoveForward), Some(KeyBind::new(KeyCode::KeyW)));
        assert_eq!(
            b.get(Action::MoveBackward),
            Some(KeyBind::new(KeyCode::KeyS))
        );
        assert_eq!(b.get(Action::RotateLeft), Some(KeyBind::new(KeyCode::KeyA)));
        assert_eq!(
            b.get(Action::RotateRight),
            Some(KeyBind::new(KeyCode::KeyD))
        );
        assert_eq!(b.get(Action::StrafeLeft), Some(KeyBind::new(KeyCode::KeyQ)));
        assert_eq!(
            b.get(Action::StrafeRight),
            Some(KeyBind::new(KeyCode::KeyE))
        );
    }

    #[test]
    fn compact1_omits_strafe() {
        let b = compact1();
        assert_eq!(b.get(Action::MoveForward), Some(KeyBind::new(KeyCode::KeyW)));
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
            b.get(Action::RotateLeft),
            Some(KeyBind::new(KeyCode::Numpad4))
        );
        assert_eq!(
            b.get(Action::RotateRight),
            Some(KeyBind::new(KeyCode::Numpad6))
        );
        assert_eq!(b.get(Action::StrafeLeft), None);
        assert_eq!(b.get(Action::StrafeRight), None);
        // Numpad 7 overrides shared R for autorun.
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
            assert_eq!(b.get(Action::NavConfirm), Some(KeyBind::new(KeyCode::Enter)));
            assert_eq!(b.get(Action::NavCancel), Some(KeyBind::new(KeyCode::Escape)));
        }
    }

    #[test]
    fn slug_round_trip() {
        for preset in Preset::ALL {
            assert_eq!(Preset::from_slug(preset.slug()), Some(*preset));
        }
        // Common alternates.
        assert_eq!(Preset::from_slug("c1"), Some(Preset::Compact1));
        assert_eq!(Preset::from_slug("c2"), Some(Preset::Compact2));
        assert_eq!(Preset::from_slug("full"), Some(Preset::Standard));
        // Case-insensitive.
        assert_eq!(Preset::from_slug("COMPACT2"), Some(Preset::Compact2));
        // Unknown.
        assert_eq!(Preset::from_slug("bogus"), None);
    }

    #[test]
    fn custom_falls_back_to_compact2_base() {
        // Without on-disk overrides, `Custom` is just Compact 2 — Stage 2c
        // overlays user changes on top.
        let custom = Preset::Custom.bindings();
        let compact2 = Preset::Compact2.bindings();
        let c1: Vec<_> = custom.iter().collect();
        let c2: Vec<_> = compact2.iter().collect();
        assert_eq!(c1, c2);
    }
}
