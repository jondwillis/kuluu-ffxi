use anyhow::Result;
use tokio::sync::{broadcast, mpsc};

use crate::agent_codec;
use crate::state::{AgentCommand, AgentEvent};

pub async fn run(
    cmd_tx: mpsc::Sender<AgentCommand>,
    event_rx: broadcast::Receiver<AgentEvent>,
) -> Result<()> {
    agent_codec::run(
        tokio::io::stdin(),
        tokio::io::stdout(),
        cmd_tx,
        event_rx,
        None,
    )
    .await
}
