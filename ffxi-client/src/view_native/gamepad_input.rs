use std::collections::HashSet;

use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::{ButtonInput, ButtonState};
use bevy::input_focus::tab_navigation::{NavAction, TabNavigation, TabNavigationError};
use bevy::input_focus::{InputFocus, InputFocusVisible};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use ffxi_viewer_core::{Action, Bindings};

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
    for gamepad in &gamepads {
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
