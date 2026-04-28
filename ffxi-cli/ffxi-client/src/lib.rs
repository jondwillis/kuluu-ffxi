//! Library entry point exposing the modules that the integration tests
//! (and any future external embedders) need to drive a session.

pub mod agent_io;
pub mod auth_client;
pub mod chrome;
pub mod goal_store;
pub mod lobby_client;
pub mod map_client;
pub mod reactor;
pub mod scene;
pub mod session;
pub mod state;
pub mod supervisor;
pub mod tls;

use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;

/// Bundled handles for a running session, with the broadcast→watch event
/// folder already wired. Both the ratatui TUI and the Bevy 3D view consume
/// this — the only difference between them is which renderer subscribes to
/// `state_rx`.
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
/// Channel sizes match what `Tui` has used since day one (cmd=64, event=256);
/// ratatui drops intermediate state under load via watch's "latest" semantics
/// and Bevy will do the same.
pub fn spawn_session(cfg: session::Config) -> SessionHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel(64);
    let (event_tx, _) = broadcast::channel(256);
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
