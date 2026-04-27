//! JSON-line stdio adapter — the agent-facing interface.
//!
//! - Reads `AgentCommand` JSON objects, one per line, from stdin.
//! - Writes `AgentEvent` JSON objects, one per line, to stdout.
//!
//! Strictly enumerated to keep agents from inventing protocol opcodes
//! (failure mode #4 in the plan).

use anyhow::Result;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{broadcast, mpsc},
};

use crate::state::{AgentCommand, AgentEvent};

/// Run the JSON sidechannel: read commands from stdin, forward them via
/// `cmd_tx`; subscribe to `event_rx` and write each event as a JSON line
/// on stdout.
pub async fn run(
    cmd_tx: mpsc::Sender<AgentCommand>,
    event_rx: broadcast::Receiver<AgentEvent>,
) -> Result<()> {
    tokio::try_join!(read_commands(cmd_tx), write_events(event_rx))?;
    Ok(())
}

async fn read_commands(cmd_tx: mpsc::Sender<AgentCommand>) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        match serde_json::from_str::<AgentCommand>(trimmed) {
            Ok(cmd) => {
                if cmd_tx.send(cmd).await.is_err() {
                    break; // session actor closed the receiver
                }
            }
            Err(err) => {
                let event = AgentEvent::Error {
                    message: format!("invalid command JSON: {err} (input: {trimmed})"),
                };
                emit_event(&event).await?;
            }
        }
    }
    Ok(())
}

async fn write_events(mut event_rx: broadcast::Receiver<AgentEvent>) -> Result<()> {
    loop {
        match event_rx.recv().await {
            Ok(event) => emit_event(&event).await?,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                emit_event(&AgentEvent::Error {
                    message: format!("event stream lagged; dropped {n} events"),
                })
                .await?;
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    Ok(())
}

async fn emit_event(event: &AgentEvent) -> Result<()> {
    let mut line = serde_json::to_vec(event)?;
    line.push(b'\n');
    let mut stdout = tokio::io::stdout();
    stdout.write_all(&line).await?;
    stdout.flush().await?;
    Ok(())
}
