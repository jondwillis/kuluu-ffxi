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

use crate::camera::{yaw_for_heading, CameraMode, ChaseCamera};
use crate::input_mode::InputMode;
use crate::snapshot::SceneState;

/// Yaw-sensitivity for RMB-drag (Chase) and free-look (FP), in radians per
/// raw pixel of mouse delta. Picked to feel like a low-DPI mouse on a 1080p
/// monitor — drag a third of the screen ≈ 90° rotation.
const MOUSE_YAW_SENS: f32 = 0.005;
/// Pitch sensitivity. Same scale as yaw so X/Y mouse motion turns at the
/// same angular rate.
const MOUSE_PITCH_SENS: f32 = 0.005;
/// One wheel notch shrinks/grows distance by this many Bevy units.
/// macOS trackpads / Magic Mice fire many wheel events per gesture
/// (often 30+ per swipe), so a step that feels right on a desktop
/// scroll wheel (1.0 — once per click) saturates instantly on trackpad
/// (single swipe → traverses the full DIST_MIN..DIST_MAX range,
/// producing the binary in/out behavior the user reported). 0.25
/// keeps the desktop feel acceptable (4 notches = 1 yalm of zoom)
/// while spreading a trackpad swipe across a few yalms instead of
/// the entire range.
// macOS trackpad / Magic Mouse inertial swipes can fire 30+ wheel
// events per gesture. 0.25/event made a single swipe traverse ~1/4
// of the DIST_MIN..DIST_MAX range — way too aggressive. 0.05 puts a
// full inertial swipe at ~1.5 yalms, more like a discrete-notch
// desktop wheel. Discrete-wheel desktop users get a slower per-click
// step in exchange, which feels fine because individual clicks are
// the intentional unit there.
const WHEEL_ZOOM_STEP: f32 = 0.05;
// Distance clamps live on `ChaseCamera` (`DIST_MIN`/`DIST_MAX`) so the
// keyboard zoom path in `view_native::input` shares them with the wheel.
const DIST_MIN: f32 = ChaseCamera::DIST_MIN;
const DIST_MAX: f32 = ChaseCamera::DIST_MAX;

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
    /// True if the LMB has moved more than [`DRAG_THRESHOLD_PX`] since
    /// the last press. Cleared on the next press; persists through the
    /// release event so click handlers can distinguish a click from a
    /// drag-release.
    pub left_dragged: bool,
    /// Right-button equivalent of [`Self::left_dragged`].
    pub right_dragged: bool,
}

/// Squared pixel threshold above which a button-held + cursor-motion
/// pair is considered a "drag" instead of a click. 5 px ≈ a couple of
/// trackpad units; anything beyond is intentional cursor motion.
const DRAG_THRESHOLD_PX_SQ: f32 = 25.0;

/// Operator-set free-look lock request (F8). Applied onto the primary
/// window's `CursorOptions` by [`crate::cursor::apply_rotate_lock_system`],
/// which unions it with the transient camera-drag lock. Decoupling the
/// request from the window mutation keeps the toggling code (F8 in
/// `view_native/input.rs`) free of `Query<&mut CursorOptions>` plumbing — it
/// just flips a bool.
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
            .add_systems(PreUpdate, collect_mouse_system)
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
            MouseButton::Left => {
                if pressed {
                    // Rising edge: start a fresh drag-tracking window.
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

    // After button + motion events are drained, latch the dragged flags
    // if the operator moved the cursor far enough while a button is held.
    // The flag survives until the next press of that button — that way
    // the release-frame click handler can still see it.
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

/// Mouse → camera. Mutates [`ChaseCamera`] (yaw, pitch, distance) based on
/// the pointer state and the active [`CameraMode`].
///
/// - `Chase`: either LMB or RMB drag rotates the camera; wheel zooms
///   within `[DIST_MIN, DIST_MAX]`. Retail FFXI accepts primary-button
///   drag for camera orbit, so we honor both.
/// - `FirstPerson`: either-button drag rotates the look direction;
///   **on release** yaw/pitch snap back to "straight ahead" (aligned
///   with the player's heading, level pitch). Matches retail's "peek
///   with the mouse, release to recenter" feel. Wheel is ignored.
///
/// LMB *clicks* (no drag) are routed through Bevy's `Pointer<Click>`
/// events for click-to-target, which are dispatched separately from
/// button-held drags — the two don't conflict.
///
/// Pitch is clamped to the mode-specific range so chase doesn't dive
/// underground and FP doesn't roll past vertical.
pub fn mouse_camera_system(
    pointer: Res<MousePointer>,
    camera_mode: Res<CameraMode>,
    state: Res<SceneState>,
    mut chase: ResMut<ChaseCamera>,
    // Tracks the previous frame's drag state so we can detect the
    // pressed→released edge that triggers FP snap-back.
    mut prev_drag: Local<bool>,
) {
    let mode = *camera_mode;
    let drag_active = pointer.left || pointer.right;
    if drag_active && pointer.delta != Vec2::ZERO {
        // Drag right (delta.x > 0) pans the view right; drag left pans left.
        // `+=` matches the keyboard ←/→ yaw sign in `view_native::input` —
        // the earlier `-=` read inverted (drag right turned the view left).
        chase.yaw += pointer.delta.x * MOUSE_YAW_SENS;
        // Bevy delivers MouseMotion with `delta.y > 0` for downward cursor
        // motion; mouse-up should pitch up, hence the sign flip.
        let pitch_d = -pointer.delta.y * MOUSE_PITCH_SENS;
        let (lo, hi) = match mode {
            CameraMode::Chase => (ChaseCamera::PITCH_MIN, ChaseCamera::PITCH_MAX),
            CameraMode::FirstPerson => (ChaseCamera::FP_PITCH_MIN, ChaseCamera::FP_PITCH_MAX),
        };
        chase.pitch = (chase.pitch + pitch_d).clamp(lo, hi);
    }

    // FP snap-back: pressed→released edge re-centers the look on the
    // player's facing direction. Chase mode keeps the operator's last
    // orbit angle on release.
    if matches!(mode, CameraMode::FirstPerson) && *prev_drag && !drag_active {
        chase.yaw = yaw_for_heading(state.snapshot.self_pos.heading);
        chase.pitch = 0.0;
    }
    *prev_drag = drag_active;

    // Wheel zoom — chase only. Positive wheel = scroll up = zoom in =
    // distance shrinks.
    if pointer.wheel != 0.0 && matches!(mode, CameraMode::Chase) {
        chase.distance =
            (chase.distance - pointer.wheel * WHEEL_ZOOM_STEP).clamp(DIST_MIN, DIST_MAX);
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
            .init_resource::<SceneState>()
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

        // RMB held: yaw moves by +delta.x * MOUSE_YAW_SENS (drag right
        // pans the view right).
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

    /// First-person look is gated on a held button (RMB drag), same as
    /// chase orbit — see the F8 doc in `view_native::input`. With the
    /// button held, drag right (delta.x > 0) pans yaw right (+) and
    /// mouse-up (delta.y < 0) pitches up.
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
            "distance {d} should equal 18.0 - 3*WHEEL_ZOOM_STEP"
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
        // `mouse_camera_system` reads `SceneState` for the FP snap-back
        // path; insert a default so the tests can drive the system without
        // the full viewer scene.
        app.insert_resource(SceneState::default());
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
