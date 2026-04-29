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
use ffxi_viewer_core::{heading_for_yaw, ChaseCamera, InputMode, SceneState, Target};
use ffxi_viewer_wire::{Entity as WireEntity, Vec3 as WireVec3};
use tokio::sync::mpsc;

use crate::state::AgentCommand;

/// 20 Hz rotation: heading delta per tick (~56 °/s — matches view3d).
const ROTATE_STEP_HELD: u8 = 2;
/// Camera yaw delta per tick when ←/→ held. Same angular rate as player
/// rotation so the two feel comparable.
const CAMERA_YAW_STEP: f32 = ROTATE_STEP_HELD as f32 * std::f32::consts::TAU / 256.0;
/// Camera pitch delta per tick when ↑/↓ held. ~17 °/s @ 20 Hz — slow on
/// purpose so taps make small adjustments.
const PITCH_STEP_HELD: f32 = 0.015;
/// Sustained A/D hold required to cancel autorun. A brief tap (single
/// 50 ms tick) won't trip this; a held sidestep will.
const STRAFE_CANCEL_MS: u64 = 300;
/// Per-unit-of-server-speed Bevy step. Wire `speed = 25` (FFXI base) yields
/// 0.25 unit/tick → 5 u/s @ 20 Hz dispatch.
const SPEED_TO_STEP: f32 = 0.01;

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

/// Edge-trigger handler: window-close shortcuts, Tab/Esc/R.
///
/// Window-close runs *before* the [`InputMode`] gate so Cmd+Q / Cmd+W /
/// the red traffic light always work — even mid-chat-typing or with a
/// menu open. Tab/Esc/R are world-mode actions and only run when the
/// user isn't focused in some other UI (`text_input_system` owns those
/// keys when chat / menu / quick-action is active).
pub fn handle_input_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut window_close: MessageReader<WindowCloseRequested>,
    state: Res<SceneState>,
    cmd_tx: Res<CommandTx>,
    mode: Res<InputMode>,
    mut target: ResMut<Target>,
    mut autorun: ResMut<AutoRun>,
    mut exit: MessageWriter<AppExit>,
) {
    // Close-window: Cmd+Q, Cmd+W, or the OS-level WindowCloseRequested
    // event (red traffic light, App→Quit menu, etc.). Esc no longer quits.
    // Runs unconditionally — quitting must work in any input mode.
    let cmd_held = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
    let close_shortcut =
        cmd_held && (keys.just_pressed(KeyCode::KeyQ) || keys.just_pressed(KeyCode::KeyW));
    let os_close = window_close.read().next().is_some();
    if close_shortcut || os_close {
        let _ = cmd_tx.0.try_send(AgentCommand::Disconnect);
        exit.write_default();
        return;
    }

    // Anything below is a world-mode action — let the text-input router
    // own these keys when a UI is focused.
    if !matches!(*mode, InputMode::World) {
        return;
    }

    if keys.just_pressed(KeyCode::Escape) {
        // FFXI: Esc deselects (target → none, menus close). No quit.
        target.id = None;
    }
    if keys.just_pressed(KeyCode::Tab) {
        target.id = next_target_by_distance(
            &state.snapshot.entities,
            state.snapshot.self_pos.pos,
            target.id,
        );
    }
    if keys.just_pressed(KeyCode::KeyR) {
        let forward_held = keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp);
        if forward_held {
            autorun.phantom_forward = !autorun.phantom_forward;
        }
    }
}

/// 20 Hz movement + camera-pitch/yaw dispatch. One Move command per tick
/// (or none if no inputs are active). Suspended while any non-`World`
/// [`InputMode`] is active so the player doesn't walk into a wall while
/// typing in chat or navigating a menu.
pub fn dispatch_movement_system(
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<SceneState>,
    cmd_tx: Res<CommandTx>,
    mode: Res<InputMode>,
    mut autorun: ResMut<AutoRun>,
    mut chase: ResMut<ChaseCamera>,
) {
    if !matches!(*mode, InputMode::World) {
        // Drop any pending autorun so a chat session doesn't auto-resume
        // a forward run on Esc-back-to-world.
        autorun.phantom_forward = false;
        autorun.strafe_held_since = None;
        return;
    }

    // --- camera pitch: ↑ raises camera (more overhead), ↓ lowers it. ---
    let mut pitch_d = 0.0;
    if keys.pressed(KeyCode::ArrowUp) {
        pitch_d += PITCH_STEP_HELD;
    }
    if keys.pressed(KeyCode::ArrowDown) {
        pitch_d -= PITCH_STEP_HELD;
    }
    if pitch_d != 0.0 {
        chase.pitch = (chase.pitch + pitch_d).clamp(ChaseCamera::PITCH_MIN, ChaseCamera::PITCH_MAX);
    }

    // --- camera yaw: ←/→ orbit the camera (player unaffected). ---
    let mut yaw_d = 0.0;
    if keys.pressed(KeyCode::ArrowLeft) {
        yaw_d += CAMERA_YAW_STEP;
    }
    if keys.pressed(KeyCode::ArrowRight) {
        yaw_d -= CAMERA_YAW_STEP;
    }
    if yaw_d != 0.0 {
        chase.yaw += yaw_d;
    }

    // --- forward / back ---
    let mut forward: i32 = 0;
    if keys.pressed(KeyCode::KeyW) {
        forward += 1;
    }
    if keys.pressed(KeyCode::KeyS) {
        forward -= 1;
    }
    // S press immediately kills autorun (FFXI: backing out drops the lock).
    if keys.just_pressed(KeyCode::KeyS) {
        autorun.phantom_forward = false;
    }

    // --- A/D rotate player only (no strafe, no camera). ---
    let mut player_rotate: i32 = 0;
    if keys.pressed(KeyCode::KeyA) {
        player_rotate -= 1;
    }
    if keys.pressed(KeyCode::KeyD) {
        player_rotate += 1;
    }

    // --- Q/E strafe perpendicular to current heading. ---
    let mut strafe: i32 = 0;
    if keys.pressed(KeyCode::KeyQ) {
        strafe -= 1;
    }
    if keys.pressed(KeyCode::KeyE) {
        strafe += 1;
    }

    // Sustained A/D-or-Q/E hold cancels autorun. A brief tap won't.
    let any_strafe_or_rotate = player_rotate != 0 || strafe != 0;
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

    // Nothing to send? Bail before touching state.
    if forward == 0 && strafe == 0 && player_rotate == 0 {
        return;
    }

    let self_pos = state.snapshot.self_pos;

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
    if player_rotate != 0 {
        let delta = (ROTATE_STEP_HELD as i32 * player_rotate).rem_euclid(256) as u8;
        heading = heading.wrapping_add(delta);
        // Lock-step camera rotation: yaw = -heading_angle, so a +Δh in
        // heading u8 → -Δh·τ/256 in yaw radians.
        chase.yaw -= player_rotate as f32
            * ROTATE_STEP_HELD as f32
            * std::f32::consts::TAU
            / 256.0;
    }

    // Step magnitude scales with server-driven speed. `speed=0` (entity hasn't
    // been populated yet) → 0 step, which silently skips movement instead
    // of teleporting somewhere weird. Speed_base is the unmodified value.
    let step = self_pos.speed as f32 * SPEED_TO_STEP;
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

    let _ = cmd_tx.0.try_send(AgentCommand::Move {
        x,
        y,
        z: self_pos.pos.z,
        heading,
    });
}

/// FFXI heading 0..=255 → (forward.x, forward.y) unit vector.
fn heading_to_forward(heading: u8) -> (f32, f32) {
    let angle = (heading as f32) * std::f32::consts::TAU / 256.0;
    (angle.sin(), angle.cos())
}

/// Cycle target by 2D distance — wire-types version of
/// `state::next_target_by_distance`.
fn next_target_by_distance(
    entities: &[WireEntity],
    from: WireVec3,
    current: Option<u32>,
) -> Option<u32> {
    if entities.is_empty() {
        return None;
    }
    let mut order: Vec<(&WireEntity, f32)> = entities
        .iter()
        .map(|e| {
            let dx = e.pos.x - from.x;
            let dy = e.pos.y - from.y;
            (e, dx * dx + dy * dy)
        })
        .collect();
    order.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let ids: Vec<u32> = order.iter().map(|(e, _)| e.id).collect();
    match current.and_then(|id| ids.iter().position(|&i| i == id)) {
        Some(p) => Some(ids[(p + 1) % ids.len()]),
        None => Some(ids[0]),
    }
}
