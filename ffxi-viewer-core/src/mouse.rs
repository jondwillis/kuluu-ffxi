use bevy::input::mouse::{MouseButtonInput, MouseMotion, MouseWheel};
use bevy::input::ButtonState;
use bevy::prelude::*;

use crate::camera::{yaw_for_heading, CameraMode, ChaseCamera};
use crate::input_mode::InputMode;
use crate::snapshot::SceneState;

const MOUSE_YAW_SENS: f32 = 0.005;

const MOUSE_PITCH_SENS: f32 = 0.005;

const WHEEL_ZOOM_STEP: f32 = 0.05;

const DIST_MIN: f32 = ChaseCamera::DIST_MIN;
const DIST_MAX: f32 = ChaseCamera::DIST_MAX;

#[derive(Resource, Debug, Default, Clone)]
pub struct MousePointer {
    pub cursor_pos: Option<Vec2>,

    pub left: bool,
    pub right: bool,
    pub middle: bool,

    pub delta: Vec2,

    pub wheel: f32,

    pub left_dragged: bool,

    pub right_dragged: bool,
}

const DRAG_THRESHOLD_PX_SQ: f32 = 25.0;

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct CursorLockRequest {
    pub locked: bool,
}

pub struct MousePlugin;

impl Plugin for MousePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MousePointer>()
            .init_resource::<CursorLockRequest>()
            .init_resource::<CameraMode>()
            .init_resource::<ChaseCamera>()
            .add_systems(PreUpdate, collect_mouse_system)
            .add_systems(Update, mouse_camera_system);
    }
}

pub fn collect_mouse_system(
    mode: Res<InputMode>,
    mut motion: MessageReader<MouseMotion>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut wheel: MessageReader<MouseWheel>,
    mut cursor: MessageReader<CursorMoved>,
    mut state: ResMut<MousePointer>,
) {
    state.delta = Vec2::ZERO;
    state.wheel = 0.0;

    for ev in motion.read() {
        state.delta += ev.delta;
    }
    for ev in cursor.read() {
        state.cursor_pos = Some(ev.position);
    }
    for ev in wheel.read() {
        state.wheel += ev.y;
    }
    for ev in buttons.read() {
        let pressed = ev.state == ButtonState::Pressed;
        match ev.button {
            MouseButton::Left => {
                if pressed {
                    state.left_dragged = false;
                }
                state.left = pressed;
            }
            MouseButton::Right => {
                if pressed {
                    state.right_dragged = false;
                }
                state.right = pressed;
            }
            MouseButton::Middle => state.middle = pressed,
            _ => {}
        }
    }

    if state.delta.length_squared() > DRAG_THRESHOLD_PX_SQ {
        if state.left {
            state.left_dragged = true;
        }
        if state.right {
            state.right_dragged = true;
        }
    }

    if !matches!(*mode, InputMode::World) {
        state.delta = Vec2::ZERO;
        state.wheel = 0.0;
    }
}

pub fn mouse_camera_system(
    pointer: Res<MousePointer>,
    camera_mode: Res<CameraMode>,
    state: Res<SceneState>,
    mut chase: ResMut<ChaseCamera>,

    mut prev_drag: Local<bool>,
) {
    let mode = *camera_mode;
    let drag_active = pointer.left || pointer.right;
    if drag_active && pointer.delta != Vec2::ZERO {
        chase.yaw += pointer.delta.x * MOUSE_YAW_SENS;

        let pitch_d = -pointer.delta.y * MOUSE_PITCH_SENS;
        let (lo, hi) = match mode {
            CameraMode::Chase => (ChaseCamera::PITCH_MIN, ChaseCamera::PITCH_MAX),
            CameraMode::FirstPerson => (ChaseCamera::FP_PITCH_MIN, ChaseCamera::FP_PITCH_MAX),
        };
        chase.pitch = (chase.pitch + pitch_d).clamp(lo, hi);
    }

    if matches!(mode, CameraMode::FirstPerson) && *prev_drag && !drag_active {
        chase.yaw = yaw_for_heading(state.snapshot.self_pos.heading);
        chase.pitch = 0.0;
    }
    *prev_drag = drag_active;

    if pointer.wheel != 0.0 && matches!(mode, CameraMode::Chase) {
        chase.distance =
            (chase.distance - pointer.wheel * WHEEL_ZOOM_STEP).clamp(DIST_MIN, DIST_MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_mouse_resets_per_frame_signals() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseMotion>()
            .add_message::<MouseButtonInput>()
            .add_message::<MouseWheel>()
            .add_message::<CursorMoved>()
            .init_resource::<InputMode>()
            .init_resource::<SceneState>()
            .add_plugins(MousePlugin);

        app.world_mut().write_message(MouseMotion {
            delta: Vec2::new(3.0, 4.0),
        });
        app.world_mut().write_message(MouseWheel {
            phase: bevy::input::touch::TouchPhase::Moved,
            unit: bevy::input::mouse::MouseScrollUnit::Line,
            x: 0.0,
            y: 1.0,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        let p = app.world().resource::<MousePointer>();
        assert_eq!(p.delta, Vec2::new(3.0, 4.0));
        assert_eq!(p.wheel, 1.0);

        app.update();
        let p = app.world().resource::<MousePointer>();
        assert_eq!(p.delta, Vec2::ZERO);
        assert_eq!(p.wheel, 0.0);
    }

    #[test]
    fn collect_mouse_button_state_is_sticky() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseMotion>()
            .add_message::<MouseButtonInput>()
            .add_message::<MouseWheel>()
            .add_message::<CursorMoved>()
            .init_resource::<InputMode>()
            .init_resource::<SceneState>()
            .add_plugins(MousePlugin);

        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Right,
            state: ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert!(app.world().resource::<MousePointer>().right);

        app.update();
        assert!(
            app.world().resource::<MousePointer>().right,
            "button stays pressed across frames until a release event"
        );

        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Right,
            state: ButtonState::Released,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert!(!app.world().resource::<MousePointer>().right);
    }

    #[test]
    fn mouse_camera_chase_drag_requires_right_button() {
        let initial_yaw = ChaseCamera::default().yaw;

        let mut app = test_app();
        app.insert_resource(MousePointer {
            right: false,
            delta: Vec2::new(20.0, 0.0),
            ..Default::default()
        })
        .insert_resource(CameraMode::Chase)
        .insert_resource(ChaseCamera::default())
        .add_systems(Update, mouse_camera_system);
        app.update();
        assert_eq!(app.world().resource::<ChaseCamera>().yaw, initial_yaw);

        let mut app = test_app();
        app.insert_resource(MousePointer {
            right: true,
            delta: Vec2::new(20.0, 0.0),
            ..Default::default()
        })
        .insert_resource(CameraMode::Chase)
        .insert_resource(ChaseCamera::default())
        .add_systems(Update, mouse_camera_system);
        app.update();
        let yaw = app.world().resource::<ChaseCamera>().yaw;
        assert!(
            (yaw - (initial_yaw + 20.0 * MOUSE_YAW_SENS)).abs() < 1e-6,
            "yaw {yaw} should match expected after drag"
        );
    }

    #[test]
    fn mouse_camera_fp_freelook_drag() {
        let pointer = MousePointer {
            right: true,
            delta: Vec2::new(10.0, -10.0),
            ..Default::default()
        };
        let mut app = test_app();
        app.insert_resource(pointer)
            .insert_resource(CameraMode::FirstPerson)
            .insert_resource(ChaseCamera::default())
            .add_systems(Update, mouse_camera_system);
        app.update();
        let chase = app.world().resource::<ChaseCamera>();
        assert!(
            (chase.yaw - (10.0 * MOUSE_YAW_SENS)).abs() < 1e-6,
            "FP yaw moves +delta.x on drag"
        );

        assert!(
            (chase.pitch - (ChaseCamera::default().pitch + 10.0 * MOUSE_PITCH_SENS)).abs() < 1e-6
        );
    }

    #[test]
    fn mouse_camera_wheel_zooms_chase_within_bounds() {
        let mut app = test_app();
        app.insert_resource(MousePointer {
            wheel: 3.0,
            ..Default::default()
        })
        .insert_resource(CameraMode::Chase)
        .insert_resource(ChaseCamera::default())
        .add_systems(Update, mouse_camera_system);
        app.update();
        let d = app.world().resource::<ChaseCamera>().distance;
        assert!(
            (d - (18.0 - 3.0 * WHEEL_ZOOM_STEP)).abs() < 1e-6,
            "distance {d} should equal 18.0 - 3*WHEEL_ZOOM_STEP"
        );

        let mut app = test_app();
        app.insert_resource(MousePointer {
            wheel: -100.0,
            ..Default::default()
        })
        .insert_resource(CameraMode::Chase)
        .insert_resource(ChaseCamera::default())
        .add_systems(Update, mouse_camera_system);
        app.update();
        assert_eq!(app.world().resource::<ChaseCamera>().distance, DIST_MAX);

        let mut app = test_app();
        app.insert_resource(MousePointer {
            wheel: 5.0,
            ..Default::default()
        })
        .insert_resource(CameraMode::FirstPerson)
        .insert_resource(ChaseCamera::default())
        .add_systems(Update, mouse_camera_system);
        app.update();
        assert_eq!(app.world().resource::<ChaseCamera>().distance, 18.0);
    }

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        app.insert_resource(SceneState::default());
        app
    }

    #[test]
    fn collect_mouse_zeros_signals_when_input_mode_is_not_world() {
        use crate::input_mode::ChatBuffer;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseMotion>()
            .add_message::<MouseButtonInput>()
            .add_message::<MouseWheel>()
            .add_message::<CursorMoved>()
            .insert_resource(InputMode::Chat(ChatBuffer::empty()))
            .init_resource::<SceneState>()
            .add_plugins(MousePlugin);

        app.world_mut().write_message(MouseMotion {
            delta: Vec2::new(10.0, 0.0),
        });
        app.update();
        assert_eq!(
            app.world().resource::<MousePointer>().delta,
            Vec2::ZERO,
            "non-World input mode suppresses motion delta"
        );
    }
}
