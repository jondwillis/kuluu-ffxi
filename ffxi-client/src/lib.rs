#![allow(clippy::type_complexity, clippy::too_many_arguments)]

pub mod agent_codec;
pub mod agent_io;
#[cfg(unix)]
pub mod agent_socket;
pub mod auth_binary;
pub mod auth_client;
pub mod config_dir;
pub mod event_dialog;
pub mod fishing;
pub mod goal_store;
pub mod launcher_store;
pub mod secret_store;

#[cfg(feature = "native-window")]
pub mod graphics_store;
#[cfg(feature = "native-window")]
pub mod keybinds_store;
pub mod lobby_client;
pub mod map_client;
pub mod net_health;
pub mod reactor;
pub mod scene;
pub mod session;
pub mod state;
pub mod supervisor;
pub mod tls;

#[cfg(any(feature = "native-window", feature = "relay"))]
pub mod wire_translate;

#[cfg(feature = "relay")]
pub mod relay;

use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;

pub struct SessionHandle {
    pub state_rx: watch::Receiver<state::SessionState>,
    pub cmd_tx: mpsc::Sender<state::AgentCommand>,
    pub event_tx: broadcast::Sender<state::AgentEvent>,
    pub session_task: JoinHandle<anyhow::Result<()>>,
    pub folder_task: JoinHandle<()>,
}

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
