use bevy::input::gamepad::Gamepad;
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::prelude::*;

const MOUSE_MOTION_ACTIVITY_PX: f32 = 2.0;

const STICK_ACTIVITY_THRESHOLD: f32 = 0.5;

#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InputMethod {
    #[default]
    KeyboardMouse,
    Gamepad,
}

pub struct InputMethodPlugin;

impl Plugin for InputMethodPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InputMethod>()
            .add_systems(PreUpdate, track_active_input_method_system);
    }
}

fn track_active_input_method_system(
    mut method: ResMut<InputMethod>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mut mouse_motion: MessageReader<MouseMotion>,
    mut mouse_wheel: MessageReader<MouseWheel>,
    gamepads: Query<&Gamepad>,
) {
    let motion_px_sq = MOUSE_MOTION_ACTIVITY_PX * MOUSE_MOTION_ACTIVITY_PX;
    let keyboard_mouse_activity = keys.get_pressed().next().is_some()
        || mouse_buttons.get_pressed().next().is_some()
        || mouse_motion
            .read()
            .any(|e| e.delta.length_squared() > motion_px_sq)
        || mouse_wheel.read().next().is_some();

    if keyboard_mouse_activity {
        *method = InputMethod::KeyboardMouse;
        return;
    }

    let stick_sq = STICK_ACTIVITY_THRESHOLD * STICK_ACTIVITY_THRESHOLD;
    let gamepad_activity = gamepads.iter().any(|g| {
        g.get_pressed().next().is_some()
            || g.left_stick().length_squared() > stick_sq
            || g.right_stick().length_squared() > stick_sq
    });
    if gamepad_activity {
        *method = InputMethod::Gamepad;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseMotion>()
            .add_message::<MouseWheel>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_plugins(InputMethodPlugin);
        app
    }

    #[test]
    fn defaults_to_keyboard_mouse() {
        let app = test_app();
        assert_eq!(
            *app.world().resource::<InputMethod>(),
            InputMethod::KeyboardMouse
        );
    }

    #[test]
    fn held_key_keeps_keyboard_mouse_active() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        app.update();
        assert_eq!(
            *app.world().resource::<InputMethod>(),
            InputMethod::KeyboardMouse
        );
    }

    #[test]
    fn mouse_motion_below_threshold_is_ignored() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<InputMethod>()
            .clone_from(&InputMethod::KeyboardMouse);
        app.world_mut().write_message(MouseMotion {
            delta: Vec2::new(0.1, 0.1),
        });
        app.update();
        assert_eq!(
            *app.world().resource::<InputMethod>(),
            InputMethod::KeyboardMouse
        );
    }
}
