//! Library entry point exposing the modules that the integration tests
//! (and any future external embedders) need to drive a session.

// Bevy ECS dictates system signatures (see ffxi-viewer-core; insurmountable).
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

pub mod agent_codec;
pub mod agent_io;
#[cfg(unix)]
pub mod agent_socket;
pub mod auth_binary;
pub mod auth_client;
pub mod goal_store;
pub mod launcher_store;
pub mod secret_store;
// Both keybinds_store and graphics_store depend on ffxi-viewer-core
// types (`Bindings`/`Preset` and `GraphicsSettings` respectively),
// which is only pulled in when the native-window feature is on.
#[cfg(feature = "native-window")]
pub mod graphics_store;
#[cfg(feature = "native-window")]
pub mod keybinds_store;
pub mod lobby_client;
pub mod map_client;
pub mod reactor;
pub mod scene;
pub mod session;
pub mod state;
pub mod supervisor;
pub mod tls;

// Wire translation (state/event ⇄ ffxi-viewer-wire) is shared between the
// in-process native bridge (`view_native::bridge`, lives under main.rs)
// and the WebSocket relay. Promoting it to the library lets `ffxi-mcp`
// reuse the relay without dragging in any of the binary's view modules.
#[cfg(any(feature = "native-window", feature = "relay"))]
pub mod wire_translate;

// WebSocket relay lives at the library level so any consumer (the binary's
// `play`/`native` paths plus `ffxi-mcp`) can spawn it against their own
// (state_rx, event_tx, cmd_tx) triple.
#[cfg(feature = "relay")]
pub mod relay;

use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;

/// Bundled handles for a running session, with the broadcast→watch event
/// folder already wired. The Bevy native viewer and headless agent paths
/// both consume this — the only difference between them is which consumer
/// subscribes to `state_rx`.
///
/// Drop semantics: dropping `cmd_tx` and the original `state_rx` will let
/// the session actor finish naturally on its next channel-closed check.
/// For deterministic shutdown, send `AgentCommand::Disconnect` and `await`
/// `session_task`.
pub struct SessionHandle {
    pub state_rx: watch::Receiver<state::SessionState>,
    pub cmd_tx: mpsc::Sender<state::AgentCommand>,
    pub event_tx: broadcast::Sender<state::AgentEvent>,
    pub session_task: JoinHandle<anyhow::Result<()>>,
    pub folder_task: JoinHandle<()>,
}

/// Spawn the session actor + event folder and return their channels.
/// Channel sizes: cmd=64; event=1024 (4x the original 256, sized for the
/// frame-rate reactor's higher `PositionChanged` volume — up to ~30/sec
/// per active mover during pathing). Watch's "latest" semantics still
/// drops intermediate state under load.
pub fn spawn_session(cfg: session::Config) -> SessionHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel(64);
    let (event_tx, _) = broadcast::channel(1024);
    let (state_tx, state_rx) = watch::channel(state::SessionState::default());

    let event_rx_for_folder = event_tx.subscribe();
    let session_task = tokio::spawn(session::run(cfg, cmd_rx, event_tx.clone()));
    let folder_task = tokio::spawn(session::run_event_folder(event_rx_for_folder, state_tx));

    SessionHandle {
        state_rx,
        cmd_tx,
        event_tx,
        session_task,
        folder_task,
    }
}

/// Spawn the session **with `reactor::run` middleware in front**. Use
/// this instead of [`spawn_session`] when you want goal-level commands
/// (`PathTo`, `Follow`, `Engage`, `Cancel`) to be absorbed and driven
/// by the reactor's per-tick state machine — without it, those land
/// in `session::run` which logs a `(reactor middleware not wired)`
/// error and drops them.
///
/// Non-goal commands (`Move`, `Action`, `Chat`, …) pass through the
/// reactor with zero added latency. The 200 ms reactor tick only
/// drives the goal-level loop.
///
/// `session_task` here is the *reactor* task, which itself spawns
/// `session::run` internally. Same JoinHandle semantics.
pub fn spawn_session_with_reactor(
    cfg: session::Config,
    reactor_cfg: reactor::ReactorConfig,
) -> SessionHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel(64);
    let (event_tx, _) = broadcast::channel(1024);
    let (state_tx, state_rx) = watch::channel(state::SessionState::default());

    let event_rx_for_folder = event_tx.subscribe();
    let session_task = tokio::spawn(reactor::run(cfg, cmd_rx, event_tx.clone(), reactor_cfg));
    let folder_task = tokio::spawn(session::run_event_folder(event_rx_for_folder, state_tx));

    SessionHandle {
        state_rx,
        cmd_tx,
        event_tx,
        session_task,
        folder_task,
    }
}
