use std::collections::HashSet;

use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::{ButtonInput, ButtonState};
use bevy::input_focus::tab_navigation::{NavAction, TabNavigation, TabNavigationError};
use bevy::input_focus::{InputFocus, InputFocusVisible};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use ffxi_viewer_core::{Action, Bindings, InputMode};

const STICK_DEADZONE: f32 = 0.35;

#[derive(Resource, Default)]
pub(super) struct GamepadAxisHeld {
    held: HashSet<KeyCode>,
}

fn sync_axis_key(
    keys: &mut ButtonInput<KeyCode>,
    held: &mut HashSet<KeyCode>,
    key: KeyCode,
    should_hold: bool,
) {
    if should_hold {
        if held.insert(key) {
            keys.press(key);
        }
    } else if held.remove(&key) {
        keys.release(key);
    }
}

fn bound_key(bindings: &Bindings, action: Action) -> Option<KeyCode> {
    let bind = bindings.get(action)?;
    if bind.mods != Default::default() {
        return None;
    }
    Some(bind.key)
}

/// D-pad moves focus between launcher UI widgets (mirrors Tab/Shift+Tab); South
/// activates the focused widget (mirrors Enter). Both ride the same
/// `bevy_input_focus`/`bevy_ui_widgets` machinery every launcher_ui screen
/// already uses for keyboard/mouse, so no per-screen changes are needed.
///
/// Only the first connected `Gamepad` entity is read: Steam Input can expose
/// one physical controller as two simultaneous devices (e.g. the Deck's own
/// pad plus a virtual Xbox-pad mirror it creates for compatibility), and
/// processing every entity would fire each button press once per device.
pub(super) fn gamepad_launcher_nav_system(
    gamepads: Query<&Gamepad>,
    nav: TabNavigation,
    mut focus: ResMut<InputFocus>,
    mut visible: ResMut<InputFocusVisible>,
    windows: Query<Entity, With<PrimaryWindow>>,
    mut keyboard_writer: MessageWriter<KeyboardInput>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let Some(gamepad) = gamepads.iter().next() else {
        return;
    };

    let nav_action = if gamepad.just_pressed(GamepadButton::DPadDown)
        || gamepad.just_pressed(GamepadButton::DPadRight)
    {
        Some(NavAction::Next)
    } else if gamepad.just_pressed(GamepadButton::DPadUp)
        || gamepad.just_pressed(GamepadButton::DPadLeft)
    {
        Some(NavAction::Previous)
    } else {
        None
    };
    if let Some(action) = nav_action {
        match nav.navigate(&focus, action) {
            Ok(next) => {
                focus.set(next);
                visible.0 = true;
            }
            Err(TabNavigationError::NoTabGroupForCurrentFocus { new_focus, .. }) => {
                focus.set(new_focus);
                visible.0 = true;
            }
            Err(_) => {}
        }
    }

    if gamepad.just_pressed(GamepadButton::South) {
        keyboard_writer.write(KeyboardInput {
            key_code: KeyCode::Enter,
            logical_key: Key::Enter,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window,
        });
    }
}

/// Left stick drives move/strafe, right stick drives camera yaw/pitch, bumpers
/// zoom. Rather than duplicating `input::handle_input_system` /
/// `dispatch_movement_system`'s logic, this holds/releases whatever `KeyCode`
/// is currently bound to each `Action` (respecting rebinding), so both systems
/// see it exactly as if the bound key were physically held.
pub(super) fn gamepad_movement_camera_system(
    gamepads: Query<&Gamepad>,
    bindings: Res<Bindings>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut held: ResMut<GamepadAxisHeld>,
) {
    let Some(gamepad) = gamepads.iter().next() else {
        for key in held.held.drain() {
            keys.release(key);
        }
        return;
    };

    let left = gamepad.left_stick();
    let right = gamepad.right_stick();

    if let Some(key) = bound_key(&bindings, Action::MoveForward) {
        sync_axis_key(&mut keys, &mut held.held, key, left.y > STICK_DEADZONE);
    }
    if let Some(key) = bound_key(&bindings, Action::MoveBackward) {
        sync_axis_key(&mut keys, &mut held.held, key, left.y < -STICK_DEADZONE);
    }
    if let Some(key) = bound_key(&bindings, Action::StrafeRight) {
        sync_axis_key(&mut keys, &mut held.held, key, left.x > STICK_DEADZONE);
    }
    if let Some(key) = bound_key(&bindings, Action::StrafeLeft) {
        sync_axis_key(&mut keys, &mut held.held, key, left.x < -STICK_DEADZONE);
    }

    if let Some(key) = bound_key(&bindings, Action::CameraYawRight) {
        sync_axis_key(&mut keys, &mut held.held, key, right.x > STICK_DEADZONE);
    }
    if let Some(key) = bound_key(&bindings, Action::CameraYawLeft) {
        sync_axis_key(&mut keys, &mut held.held, key, right.x < -STICK_DEADZONE);
    }
    if let Some(key) = bound_key(&bindings, Action::CameraPitchUp) {
        sync_axis_key(&mut keys, &mut held.held, key, right.y > STICK_DEADZONE);
    }
    if let Some(key) = bound_key(&bindings, Action::CameraPitchDown) {
        sync_axis_key(&mut keys, &mut held.held, key, right.y < -STICK_DEADZONE);
    }

    if let Some(key) = bound_key(&bindings, Action::CameraZoomIn) {
        sync_axis_key(
            &mut keys,
            &mut held.held,
            key,
            gamepad.pressed(GamepadButton::LeftTrigger),
        );
    }
    if let Some(key) = bound_key(&bindings, Action::CameraZoomOut) {
        sync_axis_key(
            &mut keys,
            &mut held.held,
            key,
            gamepad.pressed(GamepadButton::RightTrigger),
        );
    }
}

/// Mirrors `keybinds::nav_keycode_for`'s reverse direction (private to that
/// module), for the same fixed key set `Bindings::matches_logical` accepts.
fn logical_key_for(key_code: KeyCode) -> Key {
    match key_code {
        KeyCode::Escape => Key::Escape,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Tab => Key::Tab,
        KeyCode::Space => Key::Space,
        KeyCode::ArrowUp => Key::ArrowUp,
        KeyCode::ArrowDown => Key::ArrowDown,
        KeyCode::ArrowLeft => Key::ArrowLeft,
        KeyCode::ArrowRight => Key::ArrowRight,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        _ => Key::Enter,
    }
}

fn synth_nav_key(writer: &mut MessageWriter<KeyboardInput>, window: Entity, key_code: KeyCode) {
    writer.write(KeyboardInput {
        key_code,
        logical_key: logical_key_for(key_code),
        state: ButtonState::Pressed,
        text: None,
        repeat: false,
        window,
    });
}

/// Pulses (press then release within the same frame) whatever `KeyCode` is
/// bound to `action`, so a `ButtonInput::just_pressed` reader (`bound_key`'s
/// other caller, `gamepad_movement_camera_system`, holds instead) sees a
/// one-frame press exactly like a physical key tap.
fn pulse_bound_key(keys: &mut ButtonInput<KeyCode>, bindings: &Bindings, action: Action) {
    if let Some(key) = bound_key(bindings, action) {
        keys.press(key);
        keys.release(key);
    }
}

/// Synthesizes a `KeyboardInput` press for whatever `KeyCode` is bound to
/// `action`, for actions `text_input.rs` reads as raw key events rather than
/// `ButtonInput` state (`ConfirmAction`, `OpenChat`, `NavConfirm`,
/// `NavCancel` — all restricted to `logical_key_for`'s fixed key set).
fn synth_bound_key(
    writer: &mut MessageWriter<KeyboardInput>,
    window: Entity,
    bindings: &Bindings,
    action: Action,
) {
    if let Some(key) = bound_key(bindings, action) {
        synth_nav_key(writer, window, key);
    }
}

/// Face buttons + D-pad-right; combat/targeting ones are gated to
/// `InputMode::World`, mirroring `handle_input_system`'s own gate.
///
/// - South: `ConfirmAction` in `World` mode — FFXI's actual "talk to this NPC
///   / open the trade-check-invite menu for this target" dispatch (also the
///   /return-home dispatch while the death prompt is up, both handled by
///   `text_input.rs`'s own `InputMode::World` branch), not `ToggleEngage`.
///   Outside `World` mode (a menu/dialog is open), `NavConfirm`.
/// - East: `ClearTarget` in `World` mode, `NavCancel` otherwise — so a menu
///   opened via South can also be closed with the gamepad.
/// - West: `ToggleEngage` — combat engage/disengage gets its own button now
///   that South is the general-purpose interact button.
/// - North: `OpenMenu`.
/// - D-pad right: `CycleTarget` — without this there was no gamepad-only way
///   to select an NPC or player to interact with at all.
/// - Right trigger: `OpenChat`.
///
/// Only the first connected `Gamepad` entity is read — see the doc comment
/// on `gamepad_launcher_nav_system` for why.
pub(super) fn gamepad_ingame_action_system(
    gamepads: Query<&Gamepad>,
    bindings: Res<Bindings>,
    mode: Res<InputMode>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut keyboard_writer: MessageWriter<KeyboardInput>,
    windows: Query<Entity, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let Some(gamepad) = gamepads.iter().next() else {
        return;
    };
    let in_world = matches!(*mode, InputMode::World);

    if gamepad.just_pressed(GamepadButton::South) {
        if in_world {
            synth_bound_key(
                &mut keyboard_writer,
                window,
                &bindings,
                Action::ConfirmAction,
            );
        } else {
            synth_bound_key(&mut keyboard_writer, window, &bindings, Action::NavConfirm);
        }
    }

    if gamepad.just_pressed(GamepadButton::East) {
        if in_world {
            pulse_bound_key(&mut keys, &bindings, Action::ClearTarget);
        } else {
            synth_bound_key(&mut keyboard_writer, window, &bindings, Action::NavCancel);
        }
    }

    if in_world && gamepad.just_pressed(GamepadButton::West) {
        pulse_bound_key(&mut keys, &bindings, Action::ToggleEngage);
    }

    if in_world && gamepad.just_pressed(GamepadButton::North) {
        pulse_bound_key(&mut keys, &bindings, Action::OpenMenu);
    }

    if in_world && gamepad.just_pressed(GamepadButton::DPadRight) {
        pulse_bound_key(&mut keys, &bindings, Action::CycleTarget);
    }

    if in_world && gamepad.just_pressed(GamepadButton::RightTrigger2) {
        synth_bound_key(&mut keyboard_writer, window, &bindings, Action::OpenChat);
    }
}
