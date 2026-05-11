//! JSON-line stdio adapter — the `play` subcommand's agent-facing
//! interface. Reads [`AgentCommand`] JSON from stdin (one per line),
//! writes [`AgentEvent`] JSON to stdout (one per line).
//!
//! Implementation lives in [`crate::agent_codec`], which is generic
//! over `AsyncRead`/`AsyncWrite` and is also used by
//! [`crate::agent_socket`] to back the `--agent-listen` mode.

use anyhow::Result;
use tokio::sync::{broadcast, mpsc};

use crate::agent_codec;
use crate::state::{AgentCommand, AgentEvent};

/// Run the JSON sidechannel: read commands from stdin, forward them via
/// `cmd_tx`; subscribe to `event_rx` and write each event as a JSON
/// line on stdout.
pub async fn run(
    cmd_tx: mpsc::Sender<AgentCommand>,
    event_rx: broadcast::Receiver<AgentEvent>,
) -> Result<()> {
    // Headless mode has no GUI takeover surface, so the pause flag is
    // always `None` here. The agent_socket path can wire one through.
    agent_codec::run(
        tokio::io::stdin(),
        tokio::io::stdout(),
        cmd_tx,
        event_rx,
        None,
    )
    .await
}
