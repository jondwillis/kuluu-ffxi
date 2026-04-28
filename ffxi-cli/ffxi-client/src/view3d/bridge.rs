//! tokio ↔ Bevy bridge. The session actor lives on the tokio runtime and
//! publishes `SessionState` snapshots through a `watch::Receiver`. Bevy
//! systems poll that receiver each frame (sync API — no runtime needed)
//! and copy the snapshot into a Bevy resource that downstream systems
//! consume. `CommandTx` is the reverse direction for input handlers,
//! unused in Stage 2 but defined here so Stage 3 can plug straight in.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use bevy::prelude::*;
use tokio::sync::{mpsc, watch};

use crate::state::{AgentCommand, SessionState};

/// Cap on retained log lines. Position/Diagnostics events fire ~1-2 Hz
/// even when filtered out, so a generous buffer (~10 s of unfiltered
/// stream at 20 Hz dispatch) stays bounded without flushing high-signal
/// lines like chat or zone-change.
pub const MAX_LOG_LINES: usize = 200;

/// Wraps the watch::Receiver from `spawn_session`. `Send + Sync` because
/// `tokio::sync::watch::Receiver<T>` is `Send + Sync` when `T: Send + Sync`.
#[derive(Resource)]
pub struct SessionStateRx(pub watch::Receiver<SessionState>);

/// Latest `SessionState` snapshot, updated every frame by `ingest_state_system`.
/// Systems read this instead of touching the watch directly so they don't
/// each call `borrow_and_update()` and confuse the change-tracking.
#[derive(Resource, Default)]
pub struct SessionStateSnapshot(pub SessionState);

/// Sender for outbound commands (movement, action, disconnect, …).
/// Stage 3 input handlers will use `try_send` to push without blocking
/// the Bevy frame; if the channel is full we'll drop the input rather
/// than stall the render loop — the same trade-off the current TUI makes
/// (`tui.rs:170-191`'s held-key dispatcher uses non-blocking `send`).
#[derive(Resource)]
#[allow(dead_code)] // wired by Stage 3 input handlers
pub struct CommandTx(pub mpsc::Sender<AgentCommand>);

/// Pull the freshest `SessionState` into the snapshot resource. Skips the
/// copy if the watch hasn't changed since last poll — at 60 Hz Bevy would
/// otherwise do 60 clones/sec of a state struct that updates 1-2 Hz from
/// the wire. `borrow_and_update` is the watch idiom for "I read it; mark
/// the change consumed."
pub fn ingest_state_system(
    mut rx: ResMut<SessionStateRx>,
    mut snapshot: ResMut<SessionStateSnapshot>,
) {
    if rx.0.has_changed().unwrap_or(false) {
        snapshot.0 = rx.0.borrow_and_update().clone();
    }
}

/// Receives JSON-formatted log lines from a tokio task that subscribes to
/// the broadcast event stream. Unbounded because the producer side is
/// already throttled by the wire — at most a few hundred events/sec, well
/// under what an unbounded queue can handle between Bevy frames.
#[derive(Resource)]
pub struct EventLogRx(pub mpsc::UnboundedReceiver<String>);

/// Producer handle for the same channel. Held by the input system so
/// outgoing `AgentCommand`s can be tee'd into the log alongside events.
#[derive(Resource, Clone)]
pub struct LogTx(pub mpsc::UnboundedSender<String>);

/// Shared filter flag, toggled by the `L` key in input.rs and read by the
/// tokio feeder task. `AtomicBool` is the lightest sync primitive that
/// works across the runtime/Bevy boundary — `Relaxed` is fine because we
/// don't care about ordering relative to anything else; the worst case is
/// one event slips through with the old setting after a toggle.
#[derive(Resource, Clone)]
pub struct ShowAllEvents(pub Arc<AtomicBool>);

/// Ring buffer of recent log lines, bounded at `MAX_LOG_LINES`. Drained
/// each frame by `ingest_log_system` and rendered by `chrome::draw_event_log`.
#[derive(Resource, Default)]
pub struct EventLog {
    pub lines: VecDeque<String>,
}

/// Drain whatever the tokio feeder has produced since last frame. Runs in
/// `Update` (not `PreUpdate`) so the renderer's `EventLog` view doesn't
/// shift mid-frame. `try_recv` in a loop is the right shape for an
/// unbounded mpsc — we don't want to block the Bevy schedule.
pub fn ingest_log_system(mut rx: ResMut<EventLogRx>, mut log: ResMut<EventLog>) {
    while let Ok(line) = rx.0.try_recv() {
        if log.lines.len() >= MAX_LOG_LINES {
            log.lines.pop_front();
        }
        log.lines.push_back(line);
    }
}
