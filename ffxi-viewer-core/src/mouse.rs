//! Cross-platform mouse input plumbing.
//!
//! Bevy already publishes raw `MouseMotion`, `MouseButtonInput`, `MouseWheel`,
//! and `CursorMoved` messages; the work here is to coalesce them into a
//! single per-frame [`MousePointer`] snapshot every consumer (camera
//! drag-rotate, click-to-target, future HUD picking) can read without
//! duplicating the `MessageReader` plumbing — and without each consumer
//! accidentally draining the underlying message channels for the others.
//!
//! Native and WASM share this module: Bevy's winit integration translates
//! browser pointer events into the same event types as native OS events, and
//! `CursorOptions::grab_mode = Locked` maps to Pointer Lock on web. The
//! browser refuses pointer-lock without a user gesture, so the code that
//! flips [`CursorLockRequest::locked = true`] must run inside a real key /
//! click event handler (the F8 toggle in `view_native/input.rs` qualifies).

use bevy::input::mouse::{MouseButtonInput, MouseMotion, MouseWheel};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};

use crate::camera::{CameraMode, ChaseCamera};
use crate::input_mode::InputMode;

/// Yaw-sensitivity for RMB-drag (Chase) and free-look (FP), in radians per
/// raw pixel of mouse delta. Picked to feel like a low-DPI mouse on a 1080p
/// monitor — drag a third of the screen ≈ 90° rotation.
const MOUSE_YAW_SENS: f32 = 0.005;
/// Pitch sensitivity. Same scale as yaw so X/Y mouse motion turns at the
/// same angular rate.
const MOUSE_PITCH_SENS: f32 = 0.005;
/// One wheel notch shrinks/grows distance by this many Bevy units.
const WHEEL_ZOOM_STEP: f32 = 1.0;
/// Closest the chase camera can pull in. Below ~3.0 the player capsule
/// clips through the near plane; for closer-than-3 use FirstPerson.
const DIST_MIN: f32 = 3.0;
/// Furthest the chase camera can pull out. Beyond 30 the avatar is too
/// small to read on the operator's screen and HUD becomes the main signal.
const DIST_MAX: f32 = 30.0;

/// Per-frame snapshot of the mouse. Replaces `MessageReader<...>` for any
/// consumer that just needs "what is the cursor doing right now" — the
/// shared collector below drains the events once per frame and writes here.
///
/// `delta` is **frame-relative** (zero unless the cursor moved this frame),
/// so consumers don't need to track previous positions. `cursor_pos` is the
/// last absolute position reported (logical pixels relative to the window
/// top-left); it's `None` until the first `CursorMoved`.
#[derive(Resource, Debug, Default, Clone)]
pub struct MousePointer {
    pub cursor_pos: Option<Vec2>,
    /// Pressed-state for each mouse button; `true` means held.
    pub left: bool,
    pub right: bool,
    pub middle: bool,
    /// Total motion accumulated this frame, in raw device units.
    pub delta: Vec2,
    /// Total wheel scroll this frame, in lines (Bevy normalizes pixels and
    /// lines into the same `MouseScrollUnit::Line` axis under `.y`).
    pub wheel: f32,
}

/// Operator-set lock request. The cursor-lock system mirrors this onto the
/// primary window's `CursorOptions`. Decoupling the request from the actual
/// window mutation keeps the toggling code (F8 in `view_native/input.rs`)
/// free of `Query<&mut CursorOptions>` plumbing — it just flips a bool.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct CursorLockRequest {
    pub locked: bool,
}

/// Plugin: registers the resources and systems. Add once per app, before
/// any system that reads `MousePointer`.
pub struct MousePlugin;

impl Plugin for MousePlugin {
    fn build(&self, app: &mut App) {
        // Resources are idempotent (`init_resource` only inserts if absent),
        // so double-registration with `ViewerCorePlugin` is fine. Listing
        // them here lets `MousePlugin` stand alone in tests / harnesses
        // that don't pull in the full viewer.
        app.init_resource::<MousePointer>()
            .init_resource::<CursorLockRequest>()
            .init_resource::<CameraMode>()
            .init_resource::<ChaseCamera>()
            .add_systems(
                PreUpdate,
                (collect_mouse_system, apply_cursor_lock_system),
            )
            // mouse_camera_system runs in Update so it sees the pointer
            // state already collected this frame (PreUpdate) and the
            // camera systems pick up the result on the same Update tick.
            .add_systems(Update, mouse_camera_system);
    }
}

/// Drain Bevy's raw mouse event readers and refresh [`MousePointer`].
///
/// Frame-local fields (`delta`, `wheel`) are reset to zero before
/// accumulation so a quiet frame correctly reports no motion. Button state
/// is sticky (held until released).
///
/// While `InputMode != World`, drag/wheel are zeroed *after* drain so we
/// still consume the events (no stale backlog) but surface no signal to the
/// camera. The cursor position itself is always tracked — chat / menu HUDs
/// may want it for pointer-aware highlighting.
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
        // `MouseWheel.unit` distinguishes Lines vs Pixels; for camera-zoom
        // semantics one notch == one step, so we don't normalize here. The
        // consumer (C3) decides the magnitude per-step.
        state.wheel += ev.y;
    }
    for ev in buttons.read() {
        let pressed = ev.state == ButtonState::Pressed;
        match ev.button {
            MouseButton::Left => state.left = pressed,
            MouseButton::Right => state.right = pressed,
            MouseButton::Middle => state.middle = pressed,
            _ => {}
        }
    }

    if !matches!(*mode, InputMode::World) {
        state.delta = Vec2::ZERO;
        state.wheel = 0.0;
    }
}

/// Mouse → camera. Mutates [`ChaseCamera`] (yaw, pitch, distance) based on
/// the pointer state and the active [`CameraMode`].
///
/// - `Chase`: RMB-drag rotates the camera; wheel zooms within
///   `[DIST_MIN, DIST_MAX]`.
/// - `FirstPerson`: cursor is locked, so any motion is intentional — drives
///   yaw/pitch without requiring a button. Wheel is ignored (no distance
///   in FP).
///
/// Pitch is clamped to the mode-specific range so chase doesn't dive
/// underground and FP doesn't roll past vertical.
pub fn mouse_camera_system(
    pointer: Res<MousePointer>,
    camera_mode: Res<CameraMode>,
    mut chase: ResMut<ChaseCamera>,
) {
    let mode = *camera_mode;
    let drag_active = match mode {
        CameraMode::Chase => pointer.right,
        CameraMode::FirstPerson => true,
    };
    if drag_active && pointer.delta != Vec2::ZERO {
        chase.yaw -= pointer.delta.x * MOUSE_YAW_SENS;
        // Bevy delivers MouseMotion with `delta.y > 0` for downward cursor
        // motion; mouse-up should pitch up, hence the sign flip.
        let pitch_d = -pointer.delta.y * MOUSE_PITCH_SENS;
        let (lo, hi) = match mode {
            CameraMode::Chase => (ChaseCamera::PITCH_MIN, ChaseCamera::PITCH_MAX),
            CameraMode::FirstPerson => (ChaseCamera::FP_PITCH_MIN, ChaseCamera::FP_PITCH_MAX),
        };
        chase.pitch = (chase.pitch + pitch_d).clamp(lo, hi);
    }
    // Wheel zoom — chase only. Positive wheel = scroll up = zoom in =
    // distance shrinks.
    if pointer.wheel != 0.0 && matches!(mode, CameraMode::Chase) {
        chase.distance =
            (chase.distance - pointer.wheel * WHEEL_ZOOM_STEP).clamp(DIST_MIN, DIST_MAX);
    }
}

/// Mirror [`CursorLockRequest`] onto the primary window's `CursorOptions`.
///
/// Bevy maps `CursorGrabMode::Locked` to Pointer Lock on the web target;
/// browsers will silently no-op this if it didn't originate from a real
/// user gesture. Toggle from inside a key handler (F8) so the gesture
/// requirement is satisfied transitively.
pub fn apply_cursor_lock_system(
    request: Res<CursorLockRequest>,
    mut q: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    let Ok(mut opts) = q.single_mut() else {
        return;
    };
    let want = if request.locked {
        bevy::window::CursorGrabMode::Locked
    } else {
        bevy::window::CursorGrabMode::None
    };
    if opts.grab_mode != want {
        opts.grab_mode = want;
        opts.visible = !request.locked;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `collect_mouse_system` zeros `delta`/`wheel` every frame; without
    /// this, a single mouse motion would persist forever and the camera
    /// would spin. The system is implemented in terms of `EventReader`s
    /// which we can't trivially populate from outside Bevy's app loop, so
    /// the test sets up a minimal `App` with synthetic events to verify the
    /// reset behavior end-to-end.
    #[test]
    fn collect_mouse_resets_per_frame_signals() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseMotion>()
            .add_message::<MouseButtonInput>()
            .add_message::<MouseWheel>()
            .add_message::<CursorMoved>()
            .init_resource::<InputMode>()
            .add_plugins(MousePlugin);

        // Frame 1: send one motion + one wheel notch; expect both reflected.
        app.world_mut().write_message(MouseMotion {
            delta: Vec2::new(3.0, 4.0),
        });
        app.world_mut().write_message(MouseWheel {
            unit: bevy::input::mouse::MouseScrollUnit::Line,
            x: 0.0,
            y: 1.0,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        let p = app.world().resource::<MousePointer>();
        assert_eq!(p.delta, Vec2::new(3.0, 4.0));
        assert_eq!(p.wheel, 1.0);

        // Frame 2: no events. Both must have been zeroed.
        app.update();
        let p = app.world().resource::<MousePointer>();
        assert_eq!(p.delta, Vec2::ZERO);
        assert_eq!(p.wheel, 0.0);
    }

    /// Button state is sticky across frames — pressing left in frame N must
    /// remain `true` through frame N+1 even with no further events, until a
    /// release event arrives. This catches the easy mistake of clearing the
    /// button flags alongside the per-frame deltas.
    #[test]
    fn collect_mouse_button_state_is_sticky() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseMotion>()
            .add_message::<MouseButtonInput>()
            .add_message::<MouseWheel>()
            .add_message::<CursorMoved>()
            .init_resource::<InputMode>()
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

    /// RMB-drag in Chase mode mutates yaw/pitch by the configured
    /// sensitivity. Without RMB held, the same delta should not move the
    /// camera (drag-rotate is a verb, not a passive effect).
    #[test]
    fn mouse_camera_chase_drag_requires_right_button() {
        let initial_yaw = ChaseCamera::default().yaw;

        // No button held: drag delta is ignored.
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

        // RMB held: yaw moves by -delta.x * MOUSE_YAW_SENS.
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
            (yaw - (initial_yaw - 20.0 * MOUSE_YAW_SENS)).abs() < 1e-6,
            "yaw {yaw} should match expected after drag"
        );
    }

    /// In first-person mode the cursor is locked, so any motion is
    /// intentional — yaw/pitch update without requiring a button held.
    #[test]
    fn mouse_camera_fp_freelook_no_button_required() {
        let pointer = MousePointer {
            right: false,
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
            (chase.yaw - (-10.0 * MOUSE_YAW_SENS)).abs() < 1e-6,
            "FP yaw moves on motion alone"
        );
        // delta.y = -10 (mouse-up) → pitch += 10 * MOUSE_PITCH_SENS.
        assert!(
            (chase.pitch - (ChaseCamera::default().pitch + 10.0 * MOUSE_PITCH_SENS)).abs() < 1e-6
        );
    }

    /// Wheel zoom shrinks/grows distance, clamped to `[DIST_MIN, DIST_MAX]`,
    /// and only in Chase mode (FP has no distance to zoom).
    #[test]
    fn mouse_camera_wheel_zooms_chase_within_bounds() {
        // Positive wheel → zoom in → distance shrinks.
        let mut app = test_app();
        app.insert_resource(MousePointer {
            wheel: 3.0,
            ..Default::default()
        })
        .insert_resource(CameraMode::Chase)
        .insert_resource(ChaseCamera::default()) // distance = 18.0
        .add_systems(Update, mouse_camera_system);
        app.update();
        let d = app.world().resource::<ChaseCamera>().distance;
        assert!(
            (d - (18.0 - 3.0 * WHEEL_ZOOM_STEP)).abs() < 1e-6,
            "distance {d} should equal 15.0"
        );

        // Negative wheel and clamping at DIST_MAX.
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

        // FP ignores the wheel entirely — distance unchanged.
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

    /// Helper: minimal Bevy app for camera-system tests. Doesn't spin up
    /// the full MousePlugin (that's tested separately) — these tests
    /// drive `mouse_camera_system` directly with synthetic resources.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app
    }

    /// While the operator is typing in chat, mouse motion must NOT bleed
    /// into the camera-drag layer. Verify by setting `InputMode::Chat` and
    /// confirming the motion is consumed (not requeued) but reported as
    /// zero in the snapshot.
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
