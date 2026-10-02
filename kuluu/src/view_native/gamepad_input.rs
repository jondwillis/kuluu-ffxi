use std::collections::BTreeSet;

use bevy::input::gamepad::{Gamepad, GamepadConnectionEvent};
use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use bevy::input::gamepad::GamepadButton;
use kuluu_render::keybinds::pad::apply_stick_deadzone;
use kuluu_render::{Action, Bindings, InputMode, PadAction, PadBindings};

/// Pins gamepad-reading systems to one physical device, rather than each
/// calling `gamepads.iter().next()` independently. Steam Input can mirror one
/// physical Deck controller as two simultaneous `Gamepad` entities (see the
/// doc comment on `gamepad_launcher_nav_system`); if the launcher's and the
/// in-game systems each pick a different one of the pair, a mirrored press
/// can still read as `just_pressed` on the *other* entity on the very first
/// in-game frame after a screen transition (e.g. login's character-select
/// confirm bleeding into an in-game target-action confirm). Latching to the
/// first-ever-connected entity and holding it across screens closes that gap.
#[derive(Resource, Default)]
pub(super) struct PrimaryGamepad(Option<Entity>);

pub(super) fn track_primary_gamepad_system(
    mut primary: ResMut<PrimaryGamepad>,
    mut connections: MessageReader<GamepadConnectionEvent>,
) {
    for ev in connections.read() {
        if ev.connected() {
            if primary.0.is_none() {
                primary.0 = Some(ev.gamepad);
            }
        } else if primary.0 == Some(ev.gamepad) {
            primary.0 = None;
        }
    }
}

fn primary_gamepad<'a>(
    primary: &PrimaryGamepad,
    gamepads: &'a Query<&Gamepad>,
) -> Option<&'a Gamepad> {
    primary.0.and_then(|e| gamepads.get(e).ok())
}

/// Deadzone-processed stick state, refreshed every render frame and consumed
/// by `dispatch_movement_system` / `camera_polish_system` as true analog
/// axes — never digitized into synthetic key holds. `movement` is
/// (right, forward) in the camera/lock frame; `camera` is (yaw, pitch) with
/// `PadBindings::invert_camera_y` already applied.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq)]
pub struct PadStickIntent {
    pub movement: Vec2,
    pub camera: Vec2,
}

/// `Action`s the pad fired this frame, consumed by `handle_input_system`
/// alongside `Bindings::just_pressed`. Direct dispatch instead of pulsing
/// synthetic `KeyCode`s into `ButtonInput` — a synthesized press has no
/// matching OS release event, so it poisoned held-key state (kuluu-obha).
#[derive(Resource, Default, Debug)]
pub struct PadPressed {
    fired: BTreeSet<Action>,
}

impl PadPressed {
    pub fn just_pressed(&self, action: Action) -> bool {
        self.fired.contains(&action)
    }
}

/// A pad-synthesized key event for `text_input_system`'s raw-event handlers,
/// carried on its own message channel so Bevy's `keyboard_input_system` never
/// sees it (a synthetic press in the global `KeyboardInput` queue also lands
/// in `ButtonInput<KeyCode>` the next frame, with no release to clear it).
#[derive(Message, Debug, Clone)]
pub struct PadKeyEvent(pub KeyboardInput);

/// One step of launcher focus movement, in screen space: `+y` is down, the
/// convention `ComputedNode`/`UiGlobalTransform` centers are scored in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NavDir {
    Up,
    Down,
    Left,
    Right,
}

impl NavDir {
    pub(super) fn as_vec2(self) -> Vec2 {
        match self {
            NavDir::Up => Vec2::new(0.0, -1.0),
            NavDir::Down => Vec2::new(0.0, 1.0),
            NavDir::Left => Vec2::new(-1.0, 0.0),
            NavDir::Right => Vec2::new(1.0, 0.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PageDir {
    Prev,
    Next,
}

/// Pad intent for the launcher, consumed by `launcher_ui::common`. A message
/// rather than direct focus manipulation so the keyboard and mouse paths keep
/// their own, unchanged handlers.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LauncherNav {
    Move(NavDir),
    Confirm,
    Cancel,
    Page(PageDir),
}

/// Deflection past the deadzone that arms a fresh stick-driven move, and the
/// lower one it must fall back under to re-arm. A stick parked exactly at the
/// activation edge would otherwise cross it every frame and walk the ring.
const STICK_NAV_ACTIVATE: f32 = 0.55;
const STICK_NAV_RELEASE: f32 = 0.33;

/// Hold time before a held direction starts repeating, and the gap between
/// repeats after that: slow enough that one flick is one step, fast enough
/// that holding crosses a long settings list without feeling stuck.
const NAV_REPEAT_DELAY_SECS: f32 = 0.42;
const NAV_REPEAT_INTERVAL_SECS: f32 = 0.11;

/// Maps a stick deflection past its deadzone to a nav direction. A pad
/// stick's +y is up; screen space's is down.
pub(super) fn stick_nav_dir(stick: Vec2, deadzone: f32, currently_held: bool) -> Option<NavDir> {
    let v = apply_stick_deadzone(stick, deadzone);
    let threshold = if currently_held {
        STICK_NAV_RELEASE
    } else {
        STICK_NAV_ACTIVATE
    };
    if v.length() < threshold {
        return None;
    }
    if v.x.abs() >= v.y.abs() {
        Some(if v.x > 0.0 {
            NavDir::Right
        } else {
            NavDir::Left
        })
    } else {
        Some(if v.y > 0.0 { NavDir::Up } else { NavDir::Down })
    }
}

/// Edge-plus-repeat state for the one shared launcher direction slot, so a
/// held d-pad or stick moves focus on a timer instead of every frame.
#[derive(Resource, Default, Debug)]
pub(super) struct LauncherNavRepeat {
    held: Option<NavDir>,
    elapsed: f32,
    repeating: bool,
    stick_held: bool,
}

impl LauncherNavRepeat {
    /// Advances the held direction and returns a step on the repeat timer.
    /// The timer resets rather than subtracts: a frame spike longer than
    /// several intervals is still one step, not a burst of banked ones.
    pub(super) fn update(&mut self, dir: Option<NavDir>, dt: f32) -> Option<NavDir> {
        let Some(dir) = dir else {
            self.held = None;
            self.elapsed = 0.0;
            self.repeating = false;
            return None;
        };
        if self.held != Some(dir) {
            self.held = Some(dir);
            self.elapsed = 0.0;
            self.repeating = false;
            return Some(dir);
        }
        self.elapsed += dt;
        let threshold = if self.repeating {
            NAV_REPEAT_INTERVAL_SECS
        } else {
            NAV_REPEAT_DELAY_SECS
        };
        if self.elapsed < threshold {
            return None;
        }
        self.elapsed = 0.0;
        self.repeating = true;
        Some(dir)
    }
}

fn pad_button(bindings: &PadBindings, action: PadAction) -> Option<GamepadButton> {
    bindings
        .button(action)
        .or_else(|| PadBindings::retail().button(action))
}

/// Turns the pad into [`LauncherNav`] intent for every launcher screen. Pure
/// producer: the focus ring, activation and paging all live in
/// `launcher_ui::common`, so no screen needs its own pad code. Paging uses
/// the shoulder bumpers, not the analog triggers (`*Trigger2`); their
/// in-game `PadAction` roles do not apply on launcher screens.
pub(super) fn gamepad_launcher_nav_system(
    gamepads: Query<&Gamepad>,
    primary: Res<PrimaryGamepad>,
    pad_bindings: Res<PadBindings>,
    time: Res<Time<Real>>,
    mut repeat: ResMut<LauncherNavRepeat>,
    mut nav: MessageWriter<LauncherNav>,
) {
    let Some(gamepad) = primary_gamepad(&primary, &gamepads) else {
        *repeat = LauncherNavRepeat::default();
        return;
    };

    let dpad = [
        (GamepadButton::DPadUp, NavDir::Up),
        (GamepadButton::DPadDown, NavDir::Down),
        (GamepadButton::DPadLeft, NavDir::Left),
        (GamepadButton::DPadRight, NavDir::Right),
    ]
    .into_iter()
    .find(|(button, _)| gamepad.pressed(*button))
    .map(|(_, dir)| dir);

    let stick = stick_nav_dir(
        gamepad.left_stick(),
        pad_bindings.stick_deadzone,
        repeat.stick_held,
    );
    repeat.stick_held = stick.is_some();

    if let Some(dir) = repeat.update(dpad.or(stick), time.delta_secs()) {
        nav.write(LauncherNav::Move(dir));
    }

    for (action, msg) in [
        (PadAction::Confirm, LauncherNav::Confirm),
        (PadAction::Cancel, LauncherNav::Cancel),
    ] {
        if pad_button(&pad_bindings, action).is_some_and(|b| gamepad.just_pressed(b)) {
            nav.write(msg);
        }
    }

    for (button, page) in [
        (GamepadButton::LeftTrigger, PageDir::Prev),
        (GamepadButton::RightTrigger, PageDir::Next),
    ] {
        if gamepad.just_pressed(button) {
            nav.write(LauncherNav::Page(page));
        }
    }
}

pub(super) fn drain_launcher_nav(mut repeat: ResMut<LauncherNavRepeat>) {
    *repeat = LauncherNavRepeat::default();
}

pub(super) fn gamepad_stick_system(
    gamepads: Query<&Gamepad>,
    primary: Res<PrimaryGamepad>,
    pad_bindings: Res<PadBindings>,
    mut intent: ResMut<PadStickIntent>,
) {
    let Some(gamepad) = primary_gamepad(&primary, &gamepads) else {
        *intent = PadStickIntent::default();
        return;
    };
    let dz = pad_bindings.stick_deadzone;
    let mut camera = apply_stick_deadzone(gamepad.right_stick(), dz);
    if pad_bindings.invert_camera_y {
        camera.y = -camera.y;
    }
    *intent = PadStickIntent {
        movement: apply_stick_deadzone(gamepad.left_stick(), dz),
        camera,
    };
}

/// What one retail pad function does when its button fires: `Action`s pushed
/// into [`PadPressed`] for `handle_input_system`'s `ButtonInput`-style
/// readers, and at most one action synthesized as a raw key event for
/// `text_input_system`'s modal router. Fishing actions ride along with their
/// world equivalents because `handle_input_system` consumes the fishing set
/// first (and returns) while a cast is live, exactly like the shared
/// Enter/arrow keybinds.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct PadEffect {
    pub presses: &'static [Action],
    pub key: Option<Action>,
}

pub(super) fn pad_action_effect(action: PadAction, in_world: bool) -> PadEffect {
    let none = PadEffect::default();
    match (action, in_world) {
        (PadAction::Confirm, true) => PadEffect {
            presses: &[Action::ConfirmAction, Action::FishingHook],
            key: Some(Action::ConfirmAction),
        },
        (PadAction::Confirm, false) => PadEffect {
            presses: &[],
            key: Some(Action::NavConfirm),
        },
        (PadAction::Cancel, true) => PadEffect {
            presses: &[Action::ClearTarget, Action::FishingCancel],
            key: None,
        },
        (PadAction::Cancel, false) => PadEffect {
            presses: &[],
            key: Some(Action::NavCancel),
        },
        (PadAction::MainMenu, true) => PadEffect {
            presses: &[Action::OpenMenu],
            key: None,
        },
        (PadAction::MainMenu, false) => none,
        (PadAction::ActiveWindow, _) => PadEffect {
            presses: &[Action::TogglePassiveCursor],
            key: None,
        },
        (PadAction::Autorun, true) => PadEffect {
            presses: &[Action::ToggleAutorun],
            key: None,
        },
        (PadAction::Autorun, false) => none,
        (PadAction::HealLock, true) => PadEffect {
            presses: &[Action::ToggleLockOn],
            key: None,
        },
        (PadAction::HealLock, false) => none,
        (PadAction::ViewToggle, _) => PadEffect {
            presses: &[Action::ToggleFirstPerson],
            key: None,
        },
        (PadAction::Screenshot, _) => PadEffect {
            presses: &[Action::Screenshot],
            key: None,
        },
        (PadAction::HideWindows, _) => PadEffect {
            presses: &[Action::ToggleHud],
            key: None,
        },
        (PadAction::OpenChat, true) => PadEffect {
            presses: &[],
            key: Some(Action::OpenChat),
        },
        (PadAction::OpenChat, false) => none,
        // Inert until their features exist: macro bars are kuluu-mco, the
        // logout window has no direct opener yet (kuluu-uos3.5).
        (PadAction::CtrlMacroBar | PadAction::AltMacroBar | PadAction::Logout, _) => none,
    }
}

fn bound_key(bindings: &Bindings, action: Action) -> Option<KeyCode> {
    let bind = bindings.get(action)?;
    if bind.mods != Default::default() {
        return None;
    }
    Some(bind.key)
}

fn emit_pad_key(
    writer: &mut MessageWriter<PadKeyEvent>,
    window: Entity,
    bindings: &Bindings,
    action: Action,
) {
    let Some(key_code) = bound_key(bindings, action) else {
        return;
    };
    let Some(logical_key) = kuluu_render::keybinds::logical_key_for(key_code) else {
        return;
    };
    writer.write(PadKeyEvent(KeyboardInput {
        key_code,
        logical_key,
        state: ButtonState::Pressed,
        text: None,
        repeat: false,
        window,
    }));
}

/// Retail-layout digital dispatch: each configured [`PadAction`] fires its
/// [`pad_action_effect`], plus the fixed d-pad roles (field targeting in
/// `World` mode, cursor movement in menus — retail `padsin` slots 21-24).
/// Reads the same pinned device every other gamepad system does — see
/// `PrimaryGamepad`'s doc comment.
pub(super) fn gamepad_action_system(
    gamepads: Query<&Gamepad>,
    primary: Res<PrimaryGamepad>,
    bindings: Res<Bindings>,
    pad_bindings: Res<PadBindings>,
    mode: Res<InputMode>,
    trade_state: Res<kuluu_render::hud::trade::TradeState>,
    mut pad_pressed: ResMut<PadPressed>,
    mut pad_key_writer: MessageWriter<PadKeyEvent>,
    windows: Query<Entity, With<PrimaryWindow>>,
) {
    pad_pressed.fired.clear();
    let Ok(window) = windows.single() else {
        return;
    };
    let Some(gamepad) = primary_gamepad(&primary, &gamepads) else {
        return;
    };
    // A trade window doesn't change InputMode (text_input.rs checks
    // trade_state.open ahead of the InputMode match), so without this it's
    // treated as World and D-pad/Confirm/Cancel go to combat/target actions
    // instead of navigating trade slots.
    let in_world = matches!(*mode, InputMode::World) && !trade_state.open;

    for (pad_action, button) in pad_bindings.iter() {
        if !gamepad.just_pressed(button) {
            continue;
        }
        let effect = pad_action_effect(pad_action, in_world);
        pad_pressed.fired.extend(effect.presses.iter().copied());
        if let Some(action) = effect.key {
            emit_pad_key(&mut pad_key_writer, window, &bindings, action);
        }
    }

    let dpad = [
        (GamepadButton::DPadUp, Action::NavUp, None),
        (GamepadButton::DPadDown, Action::NavDown, None),
        (
            GamepadButton::DPadLeft,
            Action::NavLeft,
            Some([Action::CycleTarget, Action::FishingReelLeft]),
        ),
        (
            GamepadButton::DPadRight,
            Action::NavRight,
            Some([Action::CycleTarget, Action::FishingReelRight]),
        ),
    ];
    for (button, nav_action, world_actions) in dpad {
        if !gamepad.just_pressed(button) {
            continue;
        }
        if in_world {
            if let Some(actions) = world_actions {
                pad_pressed.fired.extend(actions);
            }
        } else {
            emit_pad_key(&mut pad_key_writer, window, &bindings, nav_action);
        }
    }
}

/// Zone/logout transitions must not carry pad state across (the
/// bevy-lifecycle-symmetry rule kuluu-obha's stuck-key drain bug violated).
pub(super) fn drain_pad_state(mut intent: ResMut<PadStickIntent>, mut pressed: ResMut<PadPressed>) {
    *intent = PadStickIntent::default();
    pressed.fired.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuluu_render::keybinds::pad::STICK_DEADZONE_DEFAULT;

    const TEST_FRAME_DT: f32 = 1.0 / 60.0;

    fn full_up() -> Vec2 {
        Vec2::new(0.0, 1.0)
    }

    #[test]
    fn nav_repeat_fires_on_press_then_holds_for_the_delay() {
        let mut repeat = LauncherNavRepeat::default();
        assert_eq!(repeat.update(Some(NavDir::Down), 0.0), Some(NavDir::Down));

        let mut waited = 0.0;
        while waited + TEST_FRAME_DT < NAV_REPEAT_DELAY_SECS {
            assert_eq!(
                repeat.update(Some(NavDir::Down), TEST_FRAME_DT),
                None,
                "repeated before the delay elapsed"
            );
            waited += TEST_FRAME_DT;
        }
        assert_eq!(
            repeat.update(Some(NavDir::Down), NAV_REPEAT_DELAY_SECS),
            Some(NavDir::Down)
        );
    }

    #[test]
    fn nav_repeat_ticks_once_per_interval_while_held() {
        let mut repeat = LauncherNavRepeat::default();
        repeat.update(Some(NavDir::Up), 0.0);
        repeat.update(Some(NavDir::Up), NAV_REPEAT_DELAY_SECS);

        let half = NAV_REPEAT_INTERVAL_SECS * 0.5;
        assert_eq!(repeat.update(Some(NavDir::Up), half), None);
        assert_eq!(repeat.update(Some(NavDir::Up), half), Some(NavDir::Up));

        let many_intervals = NAV_REPEAT_INTERVAL_SECS * 10.0;
        assert_eq!(
            repeat.update(Some(NavDir::Up), many_intervals),
            Some(NavDir::Up)
        );
        assert_eq!(
            repeat.update(Some(NavDir::Up), 0.0),
            None,
            "a long frame must not bank extra steps"
        );
    }

    #[test]
    fn nav_repeat_rearms_on_release_and_on_direction_change() {
        let mut repeat = LauncherNavRepeat::default();
        repeat.update(Some(NavDir::Down), 0.0);
        repeat.update(Some(NavDir::Down), NAV_REPEAT_DELAY_SECS);
        assert_eq!(repeat.update(None, TEST_FRAME_DT), None);
        assert_eq!(repeat.update(Some(NavDir::Down), 0.0), Some(NavDir::Down));
        assert_eq!(repeat.update(Some(NavDir::Down), TEST_FRAME_DT), None);

        assert_eq!(repeat.update(Some(NavDir::Up), 0.0), Some(NavDir::Up));
        assert_eq!(
            repeat.update(Some(NavDir::Up), NAV_REPEAT_INTERVAL_SECS),
            None,
            "a direction change restarts the longer initial delay"
        );
    }

    #[test]
    fn stick_inside_deadzone_reports_no_direction() {
        for stick in [
            Vec2::ZERO,
            full_up() * STICK_DEADZONE_DEFAULT,
            full_up() * (STICK_DEADZONE_DEFAULT * 0.5),
        ] {
            assert_eq!(stick_nav_dir(stick, STICK_DEADZONE_DEFAULT, false), None);
            assert_eq!(stick_nav_dir(stick, STICK_DEADZONE_DEFAULT, true), None);
        }
    }

    /// The raw stick value undoes the deadzone renormalization so the
    /// processed magnitude lands between the two thresholds.
    #[test]
    fn stick_hysteresis_needs_activate_then_holds_until_release() {
        let between = (STICK_NAV_ACTIVATE + STICK_NAV_RELEASE) * 0.5;
        let raw = between * (1.0 - STICK_DEADZONE_DEFAULT) + STICK_DEADZONE_DEFAULT;
        let stick = full_up() * raw;
        assert_eq!(stick_nav_dir(stick, STICK_DEADZONE_DEFAULT, false), None);
        assert_eq!(
            stick_nav_dir(stick, STICK_DEADZONE_DEFAULT, true),
            Some(NavDir::Up)
        );
    }

    /// A pad's +y is up; screen space's is down, so the Up step vector
    /// matches the arrow-key convention the focus picker scores in.
    #[test]
    fn stick_reports_the_dominant_axis_direction() {
        assert_eq!(
            stick_nav_dir(Vec2::new(0.9, 0.1), STICK_DEADZONE_DEFAULT, false),
            Some(NavDir::Right)
        );
        assert_eq!(
            stick_nav_dir(Vec2::new(-0.9, 0.1), STICK_DEADZONE_DEFAULT, false),
            Some(NavDir::Left)
        );
        assert_eq!(
            stick_nav_dir(Vec2::new(0.1, 0.9), STICK_DEADZONE_DEFAULT, false),
            Some(NavDir::Up)
        );
        assert_eq!(
            stick_nav_dir(Vec2::new(0.1, -0.9), STICK_DEADZONE_DEFAULT, false),
            Some(NavDir::Down)
        );
        assert_eq!(NavDir::Up.as_vec2(), Vec2::new(0.0, -1.0));
    }

    #[test]
    fn dpad_direction_wins_over_the_stick_in_one_repeat_slot() {
        let dpad = Some(NavDir::Down);
        let stick = stick_nav_dir(Vec2::new(0.0, 1.0), STICK_DEADZONE_DEFAULT, false);
        assert_eq!(stick, Some(NavDir::Up));
        assert_eq!(dpad.or(stick), Some(NavDir::Down));

        let mut repeat = LauncherNavRepeat::default();
        let both = Some(NavDir::Down);
        assert_eq!(repeat.update(both, 0.0), Some(NavDir::Down));
        assert_eq!(
            repeat.update(both, TEST_FRAME_DT),
            None,
            "d-pad and stick share one repeat slot"
        );
    }

    #[test]
    fn launcher_buttons_follow_pad_bindings_with_retail_fallback() {
        let mut bindings = PadBindings::retail();
        bindings.set(PadAction::Confirm, Some(GamepadButton::North));
        assert_eq!(
            pad_button(&bindings, PadAction::Confirm),
            Some(GamepadButton::North)
        );

        bindings.set(PadAction::Confirm, None);
        bindings.set(PadAction::Cancel, None);
        assert_eq!(
            pad_button(&bindings, PadAction::Confirm),
            PadBindings::retail().button(PadAction::Confirm)
        );
        assert_eq!(
            pad_button(&bindings, PadAction::Cancel),
            PadBindings::retail().button(PadAction::Cancel)
        );
    }

    #[test]
    fn confirm_mirrors_a_bound_key_on_both_channels() {
        let world = pad_action_effect(PadAction::Confirm, true);
        assert!(world.presses.contains(&Action::ConfirmAction));
        assert!(world.presses.contains(&Action::FishingHook));
        assert_eq!(world.key, Some(Action::ConfirmAction));

        let menu = pad_action_effect(PadAction::Confirm, false);
        assert_eq!(menu.presses, &[] as &[Action]);
        assert_eq!(menu.key, Some(Action::NavConfirm));
    }

    #[test]
    fn cancel_clears_target_in_world_and_navigates_in_menus() {
        let world = pad_action_effect(PadAction::Cancel, true);
        assert!(world.presses.contains(&Action::ClearTarget));
        assert!(world.presses.contains(&Action::FishingCancel));
        assert_eq!(world.key, None);

        assert_eq!(
            pad_action_effect(PadAction::Cancel, false).key,
            Some(Action::NavCancel)
        );
    }

    #[test]
    fn retail_world_roles_dispatch_their_actions() {
        for (pad, action) in [
            (PadAction::MainMenu, Action::OpenMenu),
            (PadAction::Autorun, Action::ToggleAutorun),
            (PadAction::HealLock, Action::ToggleLockOn),
            (PadAction::ViewToggle, Action::ToggleFirstPerson),
            (PadAction::Screenshot, Action::Screenshot),
            (PadAction::HideWindows, Action::ToggleHud),
            (PadAction::ActiveWindow, Action::TogglePassiveCursor),
        ] {
            let effect = pad_action_effect(pad, true);
            assert!(
                effect.presses.contains(&action),
                "{pad:?} must press {action:?}"
            );
            assert_eq!(effect.key, None, "{pad:?}");
        }
    }

    #[test]
    fn unimplemented_functions_are_inert() {
        for pad in [
            PadAction::CtrlMacroBar,
            PadAction::AltMacroBar,
            PadAction::Logout,
        ] {
            for in_world in [true, false] {
                assert_eq!(pad_action_effect(pad, in_world), PadEffect::default());
            }
        }
    }

    #[test]
    fn open_chat_only_fires_in_world() {
        assert_eq!(
            pad_action_effect(PadAction::OpenChat, true).key,
            Some(Action::OpenChat)
        );
        assert_eq!(pad_action_effect(PadAction::OpenChat, false).key, None);
    }
}
