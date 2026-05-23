//! Keyboard → `AgentCommand` for the native viewer.
//!
//! # Control model (camera-driven, FFXI default)
//!
//! ```text
//!   W/S       walk forward/back IN CAMERA DIRECTION (player heading
//!             snaps each tick to "away from camera" — ChaseCamera.yaw
//!             determines the move direction, not the player's prior heading).
//!   Q/E       strafe left/right perpendicular to current player heading.
//!             No camera-snap — strafe respects whatever direction A/D
//!             rotated the player to.
//!   A/D       rotate player heading AND camera yaw lock-step. Camera
//!             stays behind player when turning in place, AND A/D
//!             actually steers the path during W-held movement (yaw
//!             moves with heading, so snap is a no-op).
//!   ←/→       rotate camera yaw ONLY (free-look). Player heading
//!             unchanged until W/S press, which snaps it to camera-forward.
//!   ↑/↓       camera pitch (↑ raises camera/more overhead, ↓ lowers it).
//!   R         toggle autorun while forward is currently held.
//!   Tab       cycle target by 2D distance from self.
//!   Esc       clear target selection (does NOT close the window).
//!   F8        toggle first-person camera. In FP, the cursor is locked
//!             (pointer-lock on web) and mouse-look (C3) drives heading 1:1.
//!             Keyboard A/D still rotates lock-step in either mode.
//!   ⌘Q ⌘W     close window (macOS quit / close-window shortcuts). Also
//!             responds to the OS window-close-requested event so the red
//!             traffic light works.
//! ```
//!
//! # Speed
//!
//! Per-tick step is derived from `state::Position::speed` via the wire
//! crate: `step = speed * 0.01` Bevy units (so the documented FFXI base
//! of speed=25 → 0.25/tick × 20 Hz = 5 u/s, matching `reactor.rs:50`'s
//! "FFXI base ~5 yalms/sec" comment). When the server populates speed
//! from `PosHead::speed`, modifiers (Bind/Quickening/etc.) flow through
//! automatically. Until then, `Position::default()` returns 25.
//!
//! # Autorun
//!
//! Phantom W. Toggled by R only when forward is currently held (FFXI:
//! tap-R from a standstill is a noop). Cancels: S press immediately, or
//! A/D held ≥ STRAFE_CANCEL_MS so a quick sidestep tap doesn't kill it.

use std::time::{Duration, Instant};

use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::window::WindowCloseRequested;
use ffxi_viewer_core::{
    heading_for_yaw, toggle_camera_mode, Action, Bindings, CameraMode, ChaseCamera, ChatBuffer,
    CursorLockRequest, InputMode, LockOn, LockOnToggle, MenuStack, OperatorCamera,
    PassiveCursorState, SceneState, Target,
};
use ffxi_viewer_wire::{Entity as WireEntity, Vec3 as WireVec3};
use tokio::sync::mpsc;

use crate::state::{ActionKind, AgentCommand};

/// Player A/D heading turn rate, **radians per second** of held key. Frame-
/// rate-independent so the dispatch tick rate (currently 60 Hz; see
/// `view_native::mod`) can change without retuning. 0.86 rad/s ≈ 49 °/s —
/// matches the retail FFXI feel of a visible but unhurried pivot (full
/// 180° in ~3.7 s) and keeps the older ROTATE_STEP_HELD=2-at-20-Hz target
/// of ~56°/s within ±13%. Camera yaw turns lock-step at the same rate so
/// the chase camera stays glued behind the player while turning in place.
pub const HEADING_TURN_RATE: f32 = 0.86;
/// Camera yaw delta per second when ←/→ held — same angular rate as
/// player rotation so free-look and steered turns feel comparable.
const CAMERA_YAW_RATE: f32 = HEADING_TURN_RATE;
/// Camera pitch delta per tick when ↑/↓ held. ~17 °/s @ 20 Hz — slow on
/// purpose so taps make small adjustments.
const PITCH_STEP_HELD: f32 = 0.015;
/// Sustained A/D hold required to cancel autorun. A brief tap (single
/// 50 ms tick) won't trip this; a held sidestep will.
const STRAFE_CANCEL_MS: u64 = 300;
/// Yalms-per-second contributed per unit of server-set speed. FFXI base
/// `speed = 25` gives 25 × 0.2 = 5 yalms/sec, matching the documented
/// "FFXI base ~5 yalms/sec" reactor comment. Step per tick is then
/// `speed * SPEED_TO_YPS * delta_secs` — frame-rate-independent so the
/// dispatch rate (currently 60 Hz; see `view_native::mod`) can change
/// without retuning movement speed.
const SPEED_TO_YPS: f32 = 0.2;

#[derive(Resource, Clone)]
pub struct CommandTx(pub mpsc::Sender<AgentCommand>);

/// Autorun state. `phantom_forward` is the "is W virtually held?" flag
/// the dispatch system reads. `strafe_held_since` tracks how long A/D
/// have been continuously pressed, for the sustained-cancel rule.
#[derive(Resource, Default)]
pub struct AutoRun {
    pub phantom_forward: bool,
    pub strafe_held_since: Option<Instant>,
}

/// Fractional carry for the time-based A/D heading turn. Heading is a
/// u8 (256 units = 2π), but at 60 Hz dispatch the per-tick delta from a
/// finite turn rate (≈0.58 u8/tick at 0.86 rad/s) rounds to zero —
/// holding A/D would never accumulate enough to flip a unit. We keep a
/// signed f32 accumulator across ticks and only emit whole-unit
/// `wrapping_add` deltas when |accum| ≥ 1.0, draining the integer part
/// each time. Reset to 0 when no turn key is held so a paused press
/// doesn't replay leftover fraction.
#[derive(Resource, Default)]
pub struct HeadingTurnAccum {
    pub units: f32,
}

/// Advance the A/D heading-turn accumulator by one tick. Returns
/// `(integer_u8_delta_this_tick, float_u8_delta_this_tick)`:
/// - `integer_u8_delta_this_tick` is what gets `wrapping_add`-ed to the
///   server-visible u8 heading (and only emitted when nonzero).
/// - `float_u8_delta_this_tick` is what the chase camera yaw consumes —
///   smooth every tick so the camera doesn't visibly snap on integer
///   ticks of the discretized heading.
///
/// On release (`dir == 0`) the leftover fractional units are discarded
/// so a re-press doesn't start with a phantom partial unit. Pure
/// function — extracted for unit testability since `dispatch_movement_system`
/// is Bevy-ECS-heavy and can't be exercised in isolation.
pub fn advance_heading_turn(
    accum_units: &mut f32,
    dir: i32,
    dt_secs: f32,
) -> (i32, f32) {
    let turn_units_per_sec = HEADING_TURN_RATE * 256.0 / std::f32::consts::TAU;
    let float_delta = dir as f32 * turn_units_per_sec * dt_secs;
    if dir == 0 {
        *accum_units = 0.0;
        return (0, 0.0);
    }
    *accum_units += float_delta;
    let whole = accum_units.trunc();
    *accum_units -= whole;
    (whole as i32, float_delta)
}

/// Edge-trigger handler: window-close shortcuts, target/autorun/lock-on
/// toggles, and the Slash/Minus chat/menu opens (moved here from
/// `text_input.rs` so they're keyboard-layout-safe — see
/// `keybinds::mod` doc-comment).
///
/// Window-close runs *before* the [`InputMode`] gate so Cmd+Q / Cmd+W /
/// the red traffic light always work — even mid-chat-typing or with a
/// menu open. World-only actions only run when the user isn't focused in
/// some other UI (`text_input_system` owns most logical-key routing when
/// chat / menu / quick-action is active).
pub fn handle_input_system(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<Bindings>,
    mut window_close: MessageReader<WindowCloseRequested>,
    mut state: ResMut<SceneState>,
    cmd_tx: Res<CommandTx>,
    mut mode: ResMut<InputMode>,
    mut target: ResMut<Target>,
    mut autorun: ResMut<AutoRun>,
    mut camera_mode: ResMut<CameraMode>,
    mut chase: ResMut<ChaseCamera>,
    mut cursor_lock: ResMut<CursorLockRequest>,
    mut lock_on: ResMut<LockOn>,
    cam_q: Query<(&Camera, &Transform), With<OperatorCamera>>,
    mut exit: MessageWriter<AppExit>,
) {
    // Close-window: Cmd+Q, Cmd+W, or the OS-level WindowCloseRequested
    // event (red traffic light, App→Quit menu, etc.). Hard-wired (not
    // bindings-driven) — quitting must work in any input mode and isn't
    // an Action the user should ever rebind away.
    let cmd_held = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
    let close_shortcut =
        cmd_held && (keys.just_pressed(KeyCode::KeyQ) || keys.just_pressed(KeyCode::KeyW));
    let os_close = window_close.read().next().is_some();
    if close_shortcut || os_close {
        let _ = cmd_tx.0.try_send(AgentCommand::Disconnect);
        exit.write_default();
        return;
    }

    // First-person toggle. Default `V` (retail Compact 1), rebindable
    // via `Action::ToggleFirstPerson`. Runs unconditionally — the
    // operator must always be able to escape FP even while a UI is
    // focused.
    //
    // Cursor stays unlocked in FP: the OG client's FP didn't capture
    // the mouse, and our `mouse_camera_system` now gates FP look on
    // RMB-drag (with snap-back on release), so there's no need to
    // hide the cursor either.
    if bindings.just_pressed(Action::ToggleFirstPerson, &keys) {
        toggle_camera_mode(&mut camera_mode, &mut chase);
        cursor_lock.locked = false;
    }

    // PassiveCursor toggle. Runs in BOTH directions and only from
    // World ↔ PassiveCursor — pressing the toggle key while in Chat /
    // Menu / Dialog / QuickAction is a no-op (the active UI takes
    // priority; user must Esc out first). The same key is the exit so
    // a single muscle-memory keypress always lands you back in World
    // from passive cursor.
    if bindings.just_pressed(Action::TogglePassiveCursor, &keys) {
        match *mode {
            InputMode::World => {
                *mode = InputMode::PassiveCursor(PassiveCursorState::fresh_chat());
                return;
            }
            InputMode::PassiveCursor(_) => {
                *mode = InputMode::World;
                return;
            }
            _ => {}
        }
    }

    // Anything below is a world-mode action — let the text-input router
    // own these keys when a UI is focused.
    if !matches!(*mode, InputMode::World) {
        return;
    }

    // UI activation triggers — moved from `text_input.rs`'s logical-key
    // handler to keep them layout-safe. The triggering KeyboardInput
    // event still flows through `text_input_system` after the mode
    // change; for `/` it lands in handle_chat_key and appends `/` to the
    // (now Chat) buffer, reproducing the prior `with_prefix("/")` shape.
    // For `-` and Space, handle_chat_key/handle_menu_key either no-op or
    // produce the documented buffer state.
    if bindings.just_pressed(Action::OpenChatCommand, &keys) {
        *mode = InputMode::Chat(ChatBuffer::empty());
        return;
    }
    if bindings.just_pressed(Action::OpenMenu, &keys) {
        *mode = InputMode::Menu(MenuStack::root());
        return;
    }

    if bindings.just_pressed(Action::ClearTarget, &keys) {
        // FFXI: Esc deselects (target → none, menus close). No quit.
        target.id = None;
    }
    if bindings.just_pressed(Action::CycleTarget, &keys) {
        // Viewport-aware Tab: only entities currently inside the camera
        // frustum are candidates. First press picks the *nearest*; later
        // presses cycle left-to-right across the screen. Mirrors retail
        // FFXI better than "all targetable, sorted by distance" — Tab
        // shouldn't ever land on something behind the player or off-screen.
        if let Ok((camera, cam_t)) = cam_q.single() {
            let cam_global = GlobalTransform::from(*cam_t);
            target.id = cycle_target_viewport(
                &state.snapshot.entities,
                state.snapshot.self_pos.pos,
                target.id,
                |world_pos| camera.world_to_ndc(&cam_global, world_pos),
            );
        }
    }
    if bindings.just_pressed(Action::ToggleAutorun, &keys) {
        // Tap-from-standstill is a no-op (FFXI parity). "Forward held"
        // means the rebound MoveForward key (KeyW under Compact 2,
        // Numpad8 under Standard) is currently down.
        if bindings.pressed(Action::MoveForward, &keys) {
            autorun.phantom_forward = !autorun.phantom_forward;
        }
    }
    if bindings.just_pressed(Action::ToggleLockOn, &keys) {
        let result = lock_on.toggle(target.id);
        let toast = match result {
            LockOnToggle::Locked(id) => {
                let name = state
                    .snapshot
                    .entities
                    .iter()
                    .find(|e| e.id == id)
                    .and_then(|e| e.name.clone())
                    .unwrap_or_else(|| format!("#{id:08X}"));
                format!("lock-on: {name}")
            }
            LockOnToggle::Cleared => "lock-on cleared".into(),
            LockOnToggle::NoTarget => "lock-on: no target".into(),
        };
        state.push_local_toast(ffxi_viewer_wire::ChatLine {
            channel: ffxi_viewer_wire::ChatChannel::System,
            sender: "client".into(),
            text: toast,
            server_ts: 0,
        });
    }

    // Auto-clear lock-on if the target despawned/zoned out so we don't
    // sit silently overriding heading toward a ghost id.
    if let Some(id) = lock_on.target_id {
        let still_visible = state.snapshot.entities.iter().any(|e| e.id == id);
        if !still_visible {
            lock_on.target_id = None;
        }
    }
}

/// Notify the server when the local `Target` changes. Centralizes what used
/// to be ad-hoc: Tab cycling, click-to-target, Esc deselect, and the
/// `/target Name` slash command all just mutate `Target.id`; this single
/// system catches every change and emits the corresponding 0x01A
/// `ChangeTarget` action (id 0x0F).
///
/// Without this, the server's notion of the player's target stays stale —
/// `/check`, /assist's "current target" semantics, action targeting
/// fallbacks, and any other target-aware verb would all misfire because
/// the server still thinks we're looking at whatever the last server-
/// initiated change was.
///
/// Deselect (Target.id → None) is sent as `target_id = 0, target_index =
/// 0`; Phoenix's `0x01a_action.cpp::process` treats id=0 as "no target",
/// matching retail behavior on Esc.
pub fn dispatch_target_change_system(
    target: Res<Target>,
    state: Res<SceneState>,
    cmd_tx: Res<CommandTx>,
    mode: Res<InputMode>,
) {
    if !target.is_changed() {
        return;
    }
    // Suppress the very first tick after world spawn — `Target::default()`
    // is `id: None`, and Bevy reports `is_changed()` for newly-inserted
    // resources. Sending a deselect on first frame would be a phantom
    // packet, and worse, would race with the lobby-handshake `InZone`
    // transition.
    if !matches!(
        *mode,
        InputMode::World
            | InputMode::Menu(_)
            | InputMode::QuickAction(_)
            | InputMode::PassiveCursor(_)
    ) {
        // Chat-mode target changes don't happen (the input router blocks
        // Tab/Esc), so this branch is mostly belt-and-suspenders.
        return;
    }

    let (target_id, target_index) = match target.id {
        Some(id) => match state.snapshot.entities.iter().find(|e| e.id == id) {
            Some(ent) => (id, ent.act_index),
            // Target points at an id we don't have in the snapshot (raced
            // with despawn). Skip — next snapshot tick will reconcile.
            None => return,
        },
        None => (0, 0),
    };

    let _ = cmd_tx.0.try_send(AgentCommand::Action {
        target_id,
        target_index,
        kind: ActionKind::ChangeTarget,
    });
}

/// 20 Hz movement + camera-pitch/yaw dispatch. One Move command per tick
/// (or none if no inputs are active). Suspended while any non-`World`
/// [`InputMode`] is active so the player doesn't walk into a wall while
/// typing in chat or navigating a menu.
pub fn dispatch_movement_system(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<Bindings>,
    time: Res<Time<Fixed>>,
    state: Res<SceneState>,
    cmd_tx: Res<CommandTx>,
    mode: Res<InputMode>,
    camera_mode: Res<CameraMode>,
    lock_on: Res<LockOn>,
    mut autorun: ResMut<AutoRun>,
    mut chase: ResMut<ChaseCamera>,
    mut turn_accum: ResMut<HeadingTurnAccum>,
    navmesh: Res<super::navmesh_overlay::NavmeshState>,
) {
    // Pause walking only when the operator's actively typing (Chat) or
    // making an event choice (Dialog). Menu and QuickAction overlays
    // navigate with arrow keys — those don't conflict with WASD movement,
    // and FFXI's quick-target picker historically stayed walkable. Esc
    // out of typing reliably resets autorun so a half-finished chat
    // session can't resume the player into a wall.
    if matches!(*mode, InputMode::Chat(_) | InputMode::Dialog(_)) {
        autorun.phantom_forward = false;
        autorun.strafe_held_since = None;
        return;
    }
    // Suppress arrow-key camera pitch/yaw while a Menu / QuickAction /
    // PassiveCursor is open so those keys steer the picker cursor (or
    // scroll the chat log, in PassiveCursor's case) instead of fighting
    // for the camera.
    let in_picker = matches!(
        *mode,
        InputMode::Menu(_) | InputMode::QuickAction(_) | InputMode::PassiveCursor(_)
    );

    // --- camera pitch: ↑ raises camera (more overhead), ↓ lowers it. ---
    // FP gets a wider clamp so the operator can mouse-/keyboard-look up
    // and down past horizontal; chase keeps the orbit-style range.
    let mut pitch_d = 0.0;
    if !in_picker && bindings.pressed(Action::CameraPitchUp, &keys) {
        pitch_d += PITCH_STEP_HELD;
    }
    if !in_picker && bindings.pressed(Action::CameraPitchDown, &keys) {
        pitch_d -= PITCH_STEP_HELD;
    }
    if pitch_d != 0.0 {
        let (lo, hi) = match *camera_mode {
            CameraMode::Chase => (ChaseCamera::PITCH_MIN, ChaseCamera::PITCH_MAX),
            CameraMode::FirstPerson => (ChaseCamera::FP_PITCH_MIN, ChaseCamera::FP_PITCH_MAX),
        };
        chase.pitch = (chase.pitch + pitch_d).clamp(lo, hi);
    }

    // --- camera yaw: ←/→ orbit the camera (player unaffected). ---
    let mut yaw_d = 0.0;
    let yaw_step = CAMERA_YAW_RATE * time.delta_secs();
    if !in_picker && bindings.pressed(Action::CameraYawLeft, &keys) {
        yaw_d += yaw_step;
    }
    if !in_picker && bindings.pressed(Action::CameraYawRight, &keys) {
        yaw_d -= yaw_step;
    }
    if yaw_d != 0.0 {
        chase.yaw += yaw_d;
    }

    // --- camera zoom: `.` in, `,` out (defaults). Chase mode only —
    //     in FirstPerson there's no `distance` to step, and retail
    //     blocks the same keys. Held (`pressed`) and time-scaled so
    //     the rate is framerate-independent (`KEYBOARD_ZOOM_RATE`
    //     yalms/sec). Holding a key produces continuous smooth zoom;
    //     a quick tap produces ~1 fixed-tick step at 60 Hz.
    if matches!(*camera_mode, CameraMode::Chase) && !in_picker {
        let mut zoom_d = 0.0;
        let step = ChaseCamera::KEYBOARD_ZOOM_RATE * time.delta_secs();
        if bindings.pressed(Action::CameraZoomIn, &keys) {
            zoom_d -= step;
        }
        if bindings.pressed(Action::CameraZoomOut, &keys) {
            zoom_d += step;
        }
        if zoom_d != 0.0 {
            chase.distance =
                (chase.distance + zoom_d).clamp(ChaseCamera::DIST_MIN, ChaseCamera::DIST_MAX);
        }
    }

    // --- forward / back ---
    let mut forward: i32 = 0;
    if bindings.pressed(Action::MoveForward, &keys) {
        forward += 1;
    }
    if bindings.pressed(Action::MoveBackward, &keys) {
        forward -= 1;
    }
    // Back-press immediately kills autorun (FFXI: backing out drops the lock).
    if bindings.just_pressed(Action::MoveBackward, &keys) {
        autorun.phantom_forward = false;
    }

    // --- rotate player (and chase camera lock-step). Time-based so the
    //     turn rate is framerate-independent: `HEADING_TURN_RATE` rad/s
    //     mapped onto the 256-unit FFXI heading scale. Holding A or D
    //     should sweep the player heading at ~0.86 rad/s (~49°/s) — a
    //     visible pivot rather than an instant snap, matching retail
    //     FFXI. The fractional carry (`HeadingTurnAccum`) is what makes
    //     sub-unit per-tick deltas at 60 Hz actually accumulate into
    //     u8 ticks instead of rounding to zero every frame. ---
    let mut player_rotate_dir: i32 = 0;
    if bindings.pressed(Action::RotateLeft, &keys) {
        player_rotate_dir -= 1;
    }
    if bindings.pressed(Action::RotateRight, &keys) {
        player_rotate_dir += 1;
    }
    let (player_rotate_u8, heading_delta_units) =
        advance_heading_turn(&mut turn_accum.units, player_rotate_dir, time.delta_secs());

    // --- strafe perpendicular to current heading. Unbound under
    //     Compact 1 / Standard — `pressed` returns false for unbound
    //     actions, so the strafe contribution is naturally zero there. ---
    let mut strafe: i32 = 0;
    if bindings.pressed(Action::StrafeLeft, &keys) {
        strafe -= 1;
    }
    if bindings.pressed(Action::StrafeRight, &keys) {
        strafe += 1;
    }

    // Sustained A/D-or-Q/E hold cancels autorun. A brief tap won't.
    let any_strafe_or_rotate = player_rotate_dir != 0 || strafe != 0;
    if any_strafe_or_rotate {
        let now = Instant::now();
        let started = *autorun.strafe_held_since.get_or_insert(now);
        if autorun.phantom_forward
            && now.duration_since(started) >= Duration::from_millis(STRAFE_CANCEL_MS)
        {
            autorun.phantom_forward = false;
        }
    } else {
        autorun.strafe_held_since = None;
    }

    // Apply autorun: virtual W held.
    if autorun.phantom_forward {
        forward = forward.max(1);
    }

    // Lock-on heading override — computed before the no-input bail-out
    // so the camera pivots to follow the target even when the player
    // is standing still. Returns the new heading u8 if a usable target
    // is in the snapshot, else `None`.
    let self_pos = state.snapshot.self_pos;
    let locked_heading: Option<u8> = lock_on.target_id.and_then(|id| {
        state
            .snapshot
            .entities
            .iter()
            .find(|e| e.id == id)
            .and_then(|ent| {
                let dx = ent.pos.x - self_pos.pos.x;
                let dy = ent.pos.y - self_pos.pos.y;
                if dx.abs() <= 0.001 && dy.abs() <= 0.001 {
                    None
                } else {
                    // LSB convention — mirror `heading_toward` in `reactor.rs`
                    // so lock-on and reactor-driven facing produce the same
                    // byte for the same geometry. `dy.atan2(dx)` matches LSB's
                    // `worldAngle(A, B) = atan2(B.z-A.z, B.x-A.x)`; the
                    // negative scale flips CCW to FFXI's CW.
                    let radians = dy.atan2(dx);
                    let raw = radians * -(128.0 / std::f32::consts::PI);
                    Some((raw.round() as i32).rem_euclid(256) as u8)
                }
            })
    });

    // Nothing to send? Bail UNLESS lock-on wants to rotate us. In that
    // case dispatch a heading-only Move (same position, new heading) so
    // the server sees the operator's facing track the target. Cheap —
    // only fires when the operator is locked AND the heading actually
    // moved by ≥1 u8 unit (~1.4°).
    // Update camera yaw smoothly every tick while A/D is held (float
    // domain — doesn't wait for the u8 heading-accumulator to tick). This
    // is what keeps the chase camera visibly glued behind the player
    // mid-pivot, even on frames where the integer heading didn't advance.
    if player_rotate_dir != 0 {
        chase.yaw -= heading_delta_units * std::f32::consts::TAU / 256.0;
    }

    // Bail-out: nothing to send this tick. We still fall through to the
    // lock-on heading-only branch below when the operator is locked on,
    // and we still need to emit a Move when the u8 heading-accumulator
    // advanced (so the server hears the new heading even when the player
    // is standing still and just turning).
    let no_input =
        forward == 0 && strafe == 0 && player_rotate_u8 == 0;
    if no_input {
        if let Some(h) = locked_heading {
            if h != self_pos.heading {
                chase.yaw = ffxi_viewer_core::yaw_for_heading(h);
                let _ = cmd_tx.0.try_send(AgentCommand::Move {
                    x: self_pos.pos.x,
                    y: self_pos.pos.y,
                    z: self_pos.pos.z,
                    heading: h,
                });
            }
        }
        return;
    }

    // Compute heading. Two effects compose, in this order:
    //   1. If forward != 0: snap heading to camera-forward (the "reify
    //      inverse heading from camera" rule). This lets W/S walk in the
    //      direction the camera looks regardless of where the player was
    //      previously facing — FFXI third-person default.
    //   2. A/D rotation is applied AFTER the snap. Crucially, A/D rotates
    //      BOTH player heading AND camera yaw lock-step — so the camera
    //      stays fixed behind the player while turning in place, AND
    //      A/D actually steers the path during W-held movement (because
    //      yaw moved with heading, the next tick's snap is a no-op).
    //      ←/→ rotates ONLY camera yaw — that's the "free look" path,
    //      and W/S's snap is what makes the player follow camera direction.
    let mut heading = self_pos.heading;
    if forward != 0 {
        heading = heading_for_yaw(chase.yaw);
    }
    if player_rotate_u8 != 0 {
        let delta = player_rotate_u8.rem_euclid(256) as u8;
        heading = heading.wrapping_add(delta);
        // Note: camera yaw was already advanced smoothly above (float
        // domain, every tick). No second yaw delta here, otherwise the
        // camera would double-step on integer-tick frames and drift.
    }

    // Lock-on: heading already computed at the top of this function
    // (see `locked_heading` shadowed above). Apply it after WASD's
    // camera-forward snap and A/D rotation so movement intent still
    // composes — W walks toward the target, A/D shifts the
    // player→camera offset around the target axis. Yaw is also pinned
    // so the chase camera trails the locked player→target line.
    if let Some(h) = locked_heading {
        heading = h;
        chase.yaw = ffxi_viewer_core::yaw_for_heading(h);
    }

    // Step magnitude is time-based: `yalms/tick = speed * SPEED_TO_YPS *
    // dt`. `speed=0` (entity hasn't been populated yet) → 0 step, which
    // silently skips movement instead of teleporting somewhere weird.
    // Speed_base is the unmodified value.
    let step = self_pos.speed as f32 * SPEED_TO_YPS * time.delta_secs();
    let mut x = self_pos.pos.x;
    let mut y = self_pos.pos.y;
    if forward != 0 && step > 0.0 {
        let (fwd_x, fwd_y) = heading_to_forward(heading);
        x += fwd_x * step * forward as f32;
        y += fwd_y * step * forward as f32;
    }
    if strafe != 0 && step > 0.0 {
        // Strafe-right = heading + 90° (clockwise viewed from above, FFXI
        // convention). Strafe-left is the negation.
        let right_heading = heading.wrapping_add(64);
        let (right_x, right_y) = heading_to_forward(right_heading);
        x += right_x * step * strafe as f32;
        y += right_y * step * strafe as f32;
    }

    // Wall-slide: if a navmesh is loaded for this zone, ask Detour
    // to clamp the proposed step. `slide_along` returns the input
    // unchanged when the start position isn't on any poly (player
    // off-mesh), and the move passes through. If anything fails the
    // raw target is used — wall-slide should never *break* movement.
    let (final_x, final_y, final_z) = if let Some(nav) = &navmesh.nav {
        let from = ffxi_nav::glam::Vec3::new(self_pos.pos.x, self_pos.pos.y, self_pos.pos.z);
        let to = ffxi_nav::glam::Vec3::new(x, y, self_pos.pos.z);
        let slid = nav
            .lock()
            .ok()
            .and_then(|guard| guard.slide_along(from, to));
        // TEMP: stuck-on-geometry probe. Log when WASD proposed a real
        // step (≥0.1 yalm) but the slide produced near-zero progress
        // (<0.1 yalm) — that's the "stuck" symptom. Cases distinguished:
        //   slide=None       → start off-mesh (would pass through, not stick)
        //   slide=Some(p≈from) → clamped to a single-poly cell (real stuck)
        // Remove once the wall-slide regression is diagnosed.
        let proposed = ((x - self_pos.pos.x).powi(2) + (y - self_pos.pos.y).powi(2)).sqrt();
        if proposed > 0.1 {
            let (resulting, branch) = match &slid {
                Some(p) => {
                    let r =
                        ((p.x - self_pos.pos.x).powi(2) + (p.y - self_pos.pos.y).powi(2)).sqrt();
                    (r, "slide_some")
                }
                None => (proposed, "slide_none_passthrough"),
            };
            if resulting < 0.1 {
                tracing::warn!(
                    branch,
                    from_xy = format!("({:.2},{:.2},{:.2})", from.x, from.y, from.z),
                    to_xy = format!("({:.2},{:.2})", to.x, to.y),
                    slid_xy = ?slid.as_ref().map(|p| (p.x, p.y, p.z)),
                    proposed_step = proposed,
                    resulting_step = resulting,
                    "wall-slide probe: proposed move but stuck (resulting <0.1)"
                );
            }
        }
        match slid {
            Some(p) => (p.x, p.y, p.z),
            None => (x, y, self_pos.pos.z),
        }
    } else {
        (x, y, self_pos.pos.z)
    };

    let _ = cmd_tx.0.try_send(AgentCommand::Move {
        x: final_x,
        y: final_y,
        z: final_z,
        heading,
    });
}

/// FFXI heading 0..=255 → (forward.x, forward.y) unit vector in our
/// horizontal plane. LSB convention (matches `heading_toward` in
/// `reactor.rs`): heading 0 = +x (east), 64 = south, 128 = west, 192 =
/// north, CW from above. With `angle = h·τ/256`, the +x component is
/// `cos(angle)` and the +y component is `-sin(angle)` because FFXI
/// rotates clockwise while math `atan2` is CCW positive.
fn heading_to_forward(heading: u8) -> (f32, f32) {
    let angle = (heading as f32) * std::f32::consts::TAU / 256.0;
    (angle.cos(), -angle.sin())
}

/// Pure helper: viewport-aware Tab cycle.
///
/// `project` maps an FFXI world position to NDC (`[-1, 1]` x/y; z `[0, 1]`
/// for in-front-of-camera, outside that range = behind / clipped).
/// Returns `None` when the math fails (camera at the same point as the
/// entity, etc.) — those entities are silently dropped.
///
/// Cycle behavior:
/// - First press (no current target, or current target is off-screen):
///   pick the *nearest visible* entity by 2D world distance — that's
///   what feels natural when starting from nothing.
/// - Subsequent presses: order visible entities left-to-right by NDC.x
///   and step to the entry after the current target. Wraps at the end.
///
/// The synthetic self entity (id == 0) doesn't appear in the wire
/// snapshot's entity list, so no explicit self-filter is needed here.
pub fn cycle_target_viewport<F>(
    entities: &[WireEntity],
    from: WireVec3,
    current: Option<u32>,
    project: F,
) -> Option<u32>
where
    F: Fn(Vec3) -> Option<Vec3>,
{
    let mut visible: Vec<(u32, f32, f32)> = entities
        .iter()
        .filter_map(|e| {
            // FFXI position → Bevy world: same mapping as `ffxi_to_bevy`.
            // Inlined here so we don't pull a Bevy dep into this fn for
            // unit tests; the conversion is one-line.
            let world_pos = Vec3::new(e.pos.x, e.pos.z, -e.pos.y);
            let ndc = project(world_pos)?;
            if ndc.x < -1.0 || ndc.x > 1.0 || ndc.y < -1.0 || ndc.y > 1.0 {
                return None;
            }
            // `world_to_ndc` returns z>1 for points behind the camera in
            // Bevy's reverse-Z projection, and z<0 past the far plane.
            // Treat both as off-screen.
            if ndc.z < 0.0 || ndc.z > 1.0 {
                return None;
            }
            let dx = e.pos.x - from.x;
            let dy = e.pos.y - from.y;
            Some((e.id, ndc.x, dx * dx + dy * dy))
        })
        .collect();

    if visible.is_empty() {
        return None;
    }

    let current_visible =
        current.and_then(|id| visible.iter().any(|&(i, _, _)| i == id).then_some(id));

    match current_visible {
        Some(curr) => {
            // Order by NDC.x ascending = left-to-right on screen.
            visible.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            let pos = visible.iter().position(|&(id, _, _)| id == curr)?;
            Some(visible[(pos + 1) % visible.len()].0)
        }
        None => {
            // No current target (or current is off-screen) → nearest.
            visible.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
            Some(visible[0].0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_viewer_wire::{Entity as WireEntity, EntityKind, Vec3 as WireVec3};

    fn ent(id: u32, x: f32, y: f32) -> WireEntity {
        WireEntity {
            id,
            act_index: 0,
            kind: EntityKind::Mob,
            name: None,
            pos: WireVec3 { x, y, z: 0.0 },
            heading: 0,
            hp_pct: None,
            bt_target_id: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
        }
    }

    /// Project that places every entity at NDC (x = pos.x / 100, y = 0,
    /// z = 0.5) — i.e. all visible, in left-to-right order matching FFXI x.
    fn fake_proj(p: Vec3) -> Option<Vec3> {
        Some(Vec3::new(p.x / 100.0, 0.0, 0.5))
    }

    /// Project that culls everything behind x>50 (i.e. simulating a
    /// view frustum that only contains entities with FFXI x ≤ 50).
    fn culled_proj(p: Vec3) -> Option<Vec3> {
        if p.x > 50.0 {
            None
        } else {
            Some(Vec3::new(p.x / 100.0, 0.0, 0.5))
        }
    }

    /// Holding A/D for one second at 60 Hz should accumulate close to
    /// `HEADING_TURN_RATE * 256/τ` u8 units (~35 units = ~49°). This is
    /// the "finite turn rate" contract: visible, framerate-independent
    /// sweep at the documented rad/s.
    #[test]
    fn heading_turn_accumulates_to_finite_rate_over_one_second() {
        let mut accum = 0.0_f32;
        let dt = 1.0 / 60.0;
        let mut total_u8: i32 = 0;
        for _ in 0..60 {
            let (whole, _f) = advance_heading_turn(&mut accum, 1, dt);
            total_u8 += whole;
        }
        let expected = (HEADING_TURN_RATE * 256.0 / std::f32::consts::TAU).round() as i32;
        // Should match expected within 1 u8 of accumulator slack.
        assert!(
            (total_u8 - expected).abs() <= 1,
            "1s of held turn produced {total_u8} u8 (expected ~{expected})",
        );
        // Verify ≈ 49°/s — the finite, retail-feel rate.
        let degrees = total_u8 as f32 * 360.0 / 256.0;
        assert!(
            (degrees - 49.0).abs() < 3.0,
            "1s of held turn = {degrees:.1}°, expected ~49°",
        );
    }

    /// At 60 Hz the per-tick float delta (≈0.58 u8/tick) is below 1.0 —
    /// without an accumulator, every tick would round to zero and the
    /// player would never turn. This regression-guards the fractional-
    /// carry behavior.
    #[test]
    fn heading_turn_does_not_round_to_zero_per_tick() {
        let mut accum = 0.0_f32;
        let dt = 1.0 / 60.0;
        // First tick: no whole unit yet (sub-1 delta).
        let (whole_1, float_1) = advance_heading_turn(&mut accum, 1, dt);
        assert_eq!(whole_1, 0, "first 60Hz tick must not yet flip a u8");
        assert!(float_1 > 0.0 && float_1 < 1.0);
        assert!(accum > 0.0, "fractional units must carry over");
        // After enough ticks the carry must eventually flip.
        let mut flipped = false;
        for _ in 0..10 {
            let (w, _) = advance_heading_turn(&mut accum, 1, dt);
            if w != 0 {
                flipped = true;
                break;
            }
        }
        assert!(flipped, "accumulator never produced a whole-unit step");
    }

    /// Releasing the turn key drops the fractional carry so a re-press
    /// doesn't start with a phantom partial unit (which would feel like
    /// a tiny snap on every tap).
    #[test]
    fn heading_turn_release_clears_fraction() {
        let mut accum = 0.0_f32;
        let dt = 1.0 / 60.0;
        // Build up some fractional carry.
        let _ = advance_heading_turn(&mut accum, 1, dt);
        assert!(accum > 0.0);
        // Release: accum must reset to exactly 0.
        let (whole, fdelta) = advance_heading_turn(&mut accum, 0, dt);
        assert_eq!(whole, 0);
        assert_eq!(fdelta, 0.0);
        assert_eq!(accum, 0.0);
    }

    /// Left and right turns are exact negatives of each other — holding
    /// A then D for the same duration must net zero net heading change.
    #[test]
    fn heading_turn_is_symmetric() {
        let dt = 1.0 / 60.0;
        let mut accum_l = 0.0_f32;
        let mut accum_r = 0.0_f32;
        let mut total_l: i32 = 0;
        let mut total_r: i32 = 0;
        for _ in 0..30 {
            total_l += advance_heading_turn(&mut accum_l, -1, dt).0;
            total_r += advance_heading_turn(&mut accum_r, 1, dt).0;
        }
        assert_eq!(total_l, -total_r);
    }

    #[test]
    fn first_press_picks_nearest_visible() {
        let from = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let entities = vec![ent(1, 30.0, 0.0), ent(2, 10.0, 0.0), ent(3, 20.0, 0.0)];
        let next = cycle_target_viewport(&entities, from, None, fake_proj);
        assert_eq!(next, Some(2)); // closest to origin
    }

    #[test]
    fn subsequent_presses_cycle_left_to_right() {
        let from = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        // ndc.x = pos.x / 100 → entity 1 leftmost, then 2, then 3.
        let entities = vec![ent(1, -50.0, 0.0), ent(2, 0.0, 0.0), ent(3, 50.0, 0.0)];
        // Starting from 1 (leftmost) → next is 2.
        assert_eq!(
            cycle_target_viewport(&entities, from, Some(1), fake_proj),
            Some(2)
        );
        // From 2 → next is 3.
        assert_eq!(
            cycle_target_viewport(&entities, from, Some(2), fake_proj),
            Some(3)
        );
        // From 3 → wraps to 1.
        assert_eq!(
            cycle_target_viewport(&entities, from, Some(3), fake_proj),
            Some(1)
        );
    }

    #[test]
    fn off_screen_entities_are_skipped() {
        let from = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        // entity 4 at x=100 will be culled by `culled_proj`.
        let entities = vec![ent(1, 0.0, 0.0), ent(4, 100.0, 0.0)];
        let next = cycle_target_viewport(&entities, from, None, culled_proj);
        assert_eq!(next, Some(1));
        // From off-screen current → falls back to nearest visible.
        let next = cycle_target_viewport(&entities, from, Some(4), culled_proj);
        assert_eq!(next, Some(1));
    }

    #[test]
    fn empty_or_all_off_screen_returns_none() {
        let from = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let entities: Vec<WireEntity> = vec![];
        assert_eq!(
            cycle_target_viewport(&entities, from, None, fake_proj),
            None
        );
        // All off-screen.
        let entities = vec![ent(1, 100.0, 0.0), ent(2, 200.0, 0.0)];
        assert_eq!(
            cycle_target_viewport(&entities, from, None, culled_proj),
            None
        );
    }

    #[test]
    fn current_offscreen_falls_back_to_nearest() {
        let from = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let entities = vec![ent(1, 30.0, 0.0), ent(99, 1000.0, 0.0)];
        // 99 not visible — should pick nearest visible (1).
        let next = cycle_target_viewport(&entities, from, Some(99), culled_proj);
        assert_eq!(next, Some(1));
    }
}
