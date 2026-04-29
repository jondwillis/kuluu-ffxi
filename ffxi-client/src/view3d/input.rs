//! Bevy keyboard → `AgentCommand` bridge. Mirrors the held-key semantics
//! of `tui.rs` so the wire output is identical between the 2D TUI and the
//! 3D view: hold W/S to walk forward/back, A/D to rotate, q/Esc to quit.
//!
//! Input arrives via `bevy_ratatui` 0.10's `KeyMessage` events, which are
//! crossterm KeyEvents pumped through the Bevy event bus by `RatatuiPlugins`.
//! Whether we get true Release events (held-key continuous mode) or only
//! Press/Repeat (terminal-auto-repeat fallback) is signalled by the
//! `KittyEnabled` resource — same fork the TUI takes at `tui.rs:46-50`.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Instant;

use bevy::{app::AppExit, prelude::*};
use bevy_ratatui::{event::KeyMessage, kitty::KittyEnabled};
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

use crate::state::{AgentCommand, SessionState, heading_to_forward, next_target_by_distance};

use super::bridge::{CommandTx, LogTx, SessionStateSnapshot, ShowAllEvents};
use super::scene::Target;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum HeldDir {
    Forward,
    Back,
    Left,
    Right,
}

/// Press timestamps for each held cardinal, plus a one-shot queue used in
/// fallback (no-kitty) mode. Cleared each FixedUpdate after dispatch.
#[derive(Resource, Default)]
pub struct HeldDirs {
    held: HashMap<HeldDir, Instant>,
    pending_nudge: Vec<HeldDir>,
}

/// Once-per-run guard for the kitty-status startup log line. Resource so
/// it persists across system runs without a `Local` migration if we ever
/// move the system between schedules.
#[derive(Resource, Default)]
pub struct KittyHintLogged(pub bool);

/// Log whether the kitty keyboard protocol is enabled. Without it, the
/// terminal only auto-repeats the most recently held key — so chords like
/// "W+D to walk-and-turn" don't work no matter what the input handler does
/// (the OS just won't send simultaneous Press events). The log routes to
/// `/tmp/ffxi-client.log` (alt-screen-safe) so it doesn't corrupt the
/// rendered scene.
pub fn log_kitty_hint_system(
    kitty: Option<Res<KittyEnabled>>,
    mut logged: ResMut<KittyHintLogged>,
) {
    if logged.0 {
        return;
    }
    logged.0 = true;
    if kitty.is_some() {
        info!(
            "kitty keyboard protocol enabled — multi-key chords (e.g. W+D \
             walk-and-turn) supported, Ctrl+C handled in-app"
        );
    } else {
        warn!(
            "kitty keyboard protocol NOT enabled in this terminal. Multi-key \
             input is limited: most terminals only auto-repeat the *latest* \
             held key, so chords like W+D won't behave as continuous \
             walk-and-turn. Recommended terminals: Kitty, Ghostty, WezTerm, \
             or iTerm2 with 'Report modifiers using CSI u' enabled."
        );
    }
}

// ---- Tunables, kept identical to `tui.rs:71-88` so the wire-side cadence
// matches between 2D and 3D views. The Bevy `FixedUpdate` schedule runs at
// 20 Hz (see `view3d::run` setup), giving us the same TICK_MS=50 rate the
// TUI uses for held-key dispatch.

/// One-shot fallback: distance per discrete keypress.
const MOVE_STEP: f32 = 1.0;
/// Continuous mode: distance per 20 Hz tick → 5 u/s, FFXI normal run speed.
const MOVE_STEP_HELD: f32 = 0.25;
/// One-shot fallback: heading delta per keypress.
const ROTATE_STEP: u8 = 8;
/// Continuous mode: heading delta per 20 Hz tick → ~56 °/s.
const ROTATE_STEP_HELD: u8 = 2;
/// In fallback mode, treat a key as released if we haven't seen it for this
/// long. Terminal auto-repeat fires faster than this once warmed up.
const FALLBACK_HOLD_MS: u128 = 250;
/// In kitty mode, GC truly-stuck keys after this long. Most kitty-protocol
/// terminals send `Press` once and `Release` once with NO `Repeat` events
/// in between, so a key's timestamp stays frozen at its initial press for
/// as long as it's held. A short threshold here would auto-clear "held W"
/// the moment the user taps any second key — the bug surfaces as "W+D
/// makes you stop instead of walk-and-turn." 30s is "if a Release event
/// genuinely got lost, we'll still recover within half a minute."
const KITTY_HOLD_MS: u128 = 30_000;

/// Read `KeyMessage`s, update `HeldDirs`, route quit-keys to AppExit
/// (after sending `AgentCommand::Disconnect` so the session shuts down
/// cleanly), and Tab to target cycling. Runs in `PreUpdate` so movement
/// dispatch (FixedUpdate) and the scene sync system both see fresh state.
///
/// Quit keys: `q`, `Esc`, `Ctrl+C`. The third matters because with the
/// kitty keyboard protocol enabled, terminals forward Ctrl+C as a key
/// event instead of delivering SIGINT to the process — so without an
/// explicit handler the app would ignore it.
pub fn handle_input_system(
    mut keys: MessageReader<KeyMessage>,
    mut exit: MessageWriter<AppExit>,
    mut held: ResMut<HeldDirs>,
    mut target: ResMut<Target>,
    snapshot: Res<SessionStateSnapshot>,
    cmd_tx: Res<CommandTx>,
    log_tx: Res<LogTx>,
    show_all: Res<ShowAllEvents>,
    kitty: Option<Res<KittyEnabled>>,
) {
    let kitty_ok = kitty.is_some();
    for k in keys.read() {
        let dir = match k.code {
            KeyCode::Char('w') | KeyCode::Up => Some(HeldDir::Forward),
            KeyCode::Char('s') | KeyCode::Down => Some(HeldDir::Back),
            KeyCode::Char('a') | KeyCode::Left => Some(HeldDir::Left),
            KeyCode::Char('d') | KeyCode::Right => Some(HeldDir::Right),
            _ => None,
        };
        if let Some(d) = dir {
            match k.kind {
                KeyEventKind::Press | KeyEventKind::Repeat => {
                    held.held.insert(d, Instant::now());
                    if !kitty_ok {
                        // Fallback: each press is a discrete nudge, dispatched
                        // once. (Kitty mode reads the held set instead.)
                        held.pending_nudge.push(d);
                    }
                }
                KeyEventKind::Release => {
                    held.held.remove(&d);
                }
            }
            continue;
        }
        if !matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        let is_ctrl_c =
            matches!(k.code, KeyCode::Char('c')) && k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                request_quit(&cmd_tx, &log_tx, &mut exit);
            }
            KeyCode::Tab => {
                target.id = next_target_by_distance(
                    &snapshot.0.entities,
                    snapshot.0.self_pos.pos,
                    target.id,
                );
            }
            KeyCode::Char('l') | KeyCode::Char('L') => {
                // Flip the shared filter flag the tokio feeder reads
                // before pushing each event. fetch_xor with `true` is the
                // atomic toggle idiom — returns the *previous* value, so
                // the marker line below describes the new state.
                let prev = show_all.0.fetch_xor(true, Ordering::Relaxed);
                let new_state = !prev;
                let marker = if new_state {
                    "✦ filter: showing all events (Position + Diagnostics included)"
                } else {
                    "✦ filter: high-signal only (Position + Diagnostics suppressed)"
                };
                let _ = log_tx.0.send(marker.to_string());
            }
            _ if is_ctrl_c => request_quit(&cmd_tx, &log_tx, &mut exit),
            _ => {}
        }
    }
}

/// Send `Disconnect` to the session actor, then trigger Bevy `AppExit`.
/// Order matters: dropping `cmd_tx` (which happens implicitly when the
/// Bevy app exits and resources drop) doesn't *immediately* close the
/// channel from the receiver's perspective if there's still a sender
/// reference — the explicit Disconnect message gives the session a clean
/// shutdown signal it can act on inside its `select!` loop. Without it,
/// `folder_task.await` in `main.rs` hangs because the session keeps
/// `event_tx` alive.
fn request_quit(cmd_tx: &CommandTx, log_tx: &LogTx, exit: &mut MessageWriter<AppExit>) {
    // Order matters: send Disconnect first so the session actor can act
    // on it inside its own select! loop. Dropping cmd_tx (which happens
    // when Bevy resources tear down post-AppExit) doesn't *immediately*
    // close the channel, so without the explicit Disconnect the session
    // and folder tasks would hang and main() would never return.
    let _ = cmd_tx.0.try_send(AgentCommand::Disconnect);
    log_command(log_tx, &AgentCommand::Disconnect);
    exit.write_default();
}

/// Tee an outbound `AgentCommand` into the JSON log. Best-effort —
/// silently drops on serialization failure (none of the variants can
/// fail in practice) or if the receiver has already been torn down.
fn log_command(log_tx: &LogTx, cmd: &AgentCommand) {
    if let Ok(json) = serde_json::to_string(cmd) {
        let _ = log_tx.0.send(format!("← {json}"));
    }
}

/// 20 Hz movement dispatch. Reads the latest server-echoed self position
/// from the snapshot, applies one tick of held-key motion on top, and
/// fires a `Move` command. The reactor (Stage 1 of harness plan, lives in
/// `ffxi-client`) will eventually take over per-tick movement; this is
/// the operator override path, same as `tui.rs:224-264`.
pub fn dispatch_movement_system(
    mut held: ResMut<HeldDirs>,
    snapshot: Res<SessionStateSnapshot>,
    cmd_tx: Res<CommandTx>,
    log_tx: Res<LogTx>,
    show_all: Res<ShowAllEvents>,
    kitty: Option<Res<KittyEnabled>>,
) {
    let kitty_ok = kitty.is_some();
    let log_moves = show_all.0.load(Ordering::Relaxed);

    // Fallback path: dispatch any queued one-shot nudges, then exit.
    if !kitty_ok {
        if !held.pending_nudge.is_empty() {
            let dirs = std::mem::take(&mut held.pending_nudge);
            send_movement(
                &snapshot.0,
                &dirs,
                MOVE_STEP,
                ROTATE_STEP,
                &cmd_tx,
                &log_tx,
                log_moves,
            );
        }
        // Even in fallback we GC the held set (terminal auto-repeat refills it).
        held.held
            .retain(|_, t| t.elapsed().as_millis() < FALLBACK_HOLD_MS);
        return;
    }

    // Kitty: continuous integration while keys are held.
    if !held.held.is_empty() {
        let dirs: Vec<HeldDir> = held.held.keys().copied().collect();
        send_movement(
            &snapshot.0,
            &dirs,
            MOVE_STEP_HELD,
            ROTATE_STEP_HELD,
            &cmd_tx,
            &log_tx,
            log_moves,
        );
    }
    held.held
        .retain(|_, t| t.elapsed().as_millis() < KITTY_HOLD_MS);
}

/// Pure compute of one tick's `Move` command from current state + held
/// directions. Forward+Back cancel, Left+Right cancel — same as
/// `tui.rs:230-264`. Sent via `try_send` so a full channel drops the
/// command rather than stalling the render thread.
fn send_movement(
    state: &SessionState,
    dirs: &[HeldDir],
    move_step: f32,
    rotate_step: u8,
    cmd_tx: &CommandTx,
    log_tx: &LogTx,
    log_moves: bool,
) {
    let mut forward: i32 = 0;
    let mut rotate: i32 = 0;
    for d in dirs {
        match d {
            HeldDir::Forward => forward += 1,
            HeldDir::Back => forward -= 1,
            HeldDir::Left => rotate -= 1,
            HeldDir::Right => rotate += 1,
        }
    }
    if forward == 0 && rotate == 0 {
        return;
    }
    let mut heading = state.self_pos.heading;
    if rotate != 0 {
        let delta = (rotate_step as i32 * rotate).rem_euclid(256) as u8;
        heading = state.self_pos.heading.wrapping_add(delta);
    }
    let (mut x, mut y) = (state.self_pos.pos.x, state.self_pos.pos.y);
    if forward != 0 {
        let (fx, fy) = heading_to_forward(heading);
        let dist = move_step * forward as f32;
        x += fx * dist;
        y += fy * dist;
    }
    let cmd = AgentCommand::Move {
        x,
        y,
        z: state.self_pos.pos.z,
        heading,
    };
    let _ = cmd_tx.0.try_send(cmd.clone());
    if log_moves {
        log_command(log_tx, &cmd);
    }
}
