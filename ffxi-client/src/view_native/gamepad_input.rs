use std::collections::HashSet;

use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::{ButtonInput, ButtonState};
use bevy::input_focus::tab_navigation::{NavAction, TabNavigation, TabNavigationError};
use bevy::input_focus::{InputFocus, InputFocusVisible};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use ffxi_viewer_core::{Action, Bindings, InputMode, Target};

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

fn synth_nav_key(writer: &mut MessageWriter<KeyboardInput>, window: Entity, key_code: KeyCode) {
    let logical_key = match key_code {
        KeyCode::Escape => Key::Escape,
        _ => Key::Enter,
    };
    writer.write(KeyboardInput {
        key_code,
        logical_key,
        state: ButtonState::Pressed,
        text: None,
        repeat: false,
        window,
    });
}

/// South is a single context-sensitive action button: whenever `InputMode`
/// isn't `World` (a menu, dialog, or other overlay is open) it mirrors
/// `NavConfirm` (Enter), exactly like `gamepad_launcher_nav_system` does for
/// the launcher. In `World` mode it mirrors `ToggleEngage` if a target is
/// selected, otherwise `OpenMenu` — matching `handle_input_system`'s own
/// `InputMode::World` gating, so the two can never both fire off one press
/// the way a raw keyboard-emulated `F` from Steam Input's Desktop profile
/// could. East mirrors `NavCancel` (Escape) so a menu opened this way can
/// also be closed with the gamepad.
pub(super) fn gamepad_ingame_action_system(
    gamepads: Query<&Gamepad>,
    bindings: Res<Bindings>,
    mode: Res<InputMode>,
    target: Res<Target>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut keyboard_writer: MessageWriter<KeyboardInput>,
    windows: Query<Entity, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    for gamepad in &gamepads {
        if gamepad.just_pressed(GamepadButton::South) {
            if matches!(*mode, InputMode::World) {
                let action = if target.id.is_some() {
                    Action::ToggleEngage
                } else {
                    Action::OpenMenu
                };
                if let Some(key) = bound_key(&bindings, action) {
                    keys.press(key);
                    keys.release(key);
                }
            } else {
                synth_nav_key(&mut keyboard_writer, window, KeyCode::Enter);
            }
        }

        if gamepad.just_pressed(GamepadButton::East) && !matches!(*mode, InputMode::World) {
            synth_nav_key(&mut keyboard_writer, window, KeyCode::Escape);
        }
    }
}
