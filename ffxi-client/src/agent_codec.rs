//! Generic JSON-line codec for the agent-facing protocol.
//!
//! Reads [`AgentCommand`] JSON objects (one per line) from an
//! `AsyncRead`, forwards them to `cmd_tx`. Subscribes to an
//! `AgentEvent` broadcast and writes each event back as a JSON line on
//! an `AsyncWrite`. Used by:
//!
//! - [`crate::agent_io`] over stdin/stdout (the `play` subcommand)
//! - [`crate::agent_socket`] over a Unix-domain socket
//!   (the `--agent-listen` mode)
//!
//! The writer is shared between the read half (for parse-error events)
//! and the write half (for the broadcast event stream) via
//! `Arc<Mutex<W>>`; contention is one lock per JSON line, which is
//! orders of magnitude below the broadcast tick rate.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc, Mutex};

use crate::state::{AgentCommand, AgentEvent};

/// Run both halves of the JSON-line protocol until either side closes.
///
/// Returns on the first half to finish (stdin EOF, broadcast closed,
/// peer hangup, or unrecoverable write error). The caller is expected
/// to treat that as "the agent peer is gone" and clean up.
///
/// `pause` is an optional shared "human in control" flag. When `Some`
/// and set to `true`, incoming commands are silently dropped — the
/// human operator (`/agent pause` in the native viewer) has taken
/// over and agent-originated wire packets must not fire. The transition
/// notifications themselves (`AgentEvent::HumanInControl` /
/// `HumanReleased`) are emitted by the slash-command handler, not
/// here — keeping the codec stateless about transitions avoids
/// per-command event spam while paused.
pub async fn run<R, W>(
    reader: R,
    writer: W,
    cmd_tx: mpsc::Sender<AgentCommand>,
    event_rx: broadcast::Receiver<AgentEvent>,
    pause: Option<Arc<AtomicBool>>,
) -> Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let writer = Arc::new(Mutex::new(writer));
    let writer_for_reader = Arc::clone(&writer);
    tokio::try_join!(
        read_commands(reader, cmd_tx, writer_for_reader, pause),
        write_events(event_rx, writer),
    )?;
    Ok(())
}

async fn read_commands<R, W>(
    reader: R,
    cmd_tx: mpsc::Sender<AgentCommand>,
    writer: Arc<Mutex<W>>,
    pause: Option<Arc<AtomicBool>>,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        match serde_json::from_str::<AgentCommand>(trimmed) {
            Ok(cmd) => {
                // Human-in-control gate: drop agent-originated
                // commands silently while paused. The transition
                // events live elsewhere; we don't want to emit
                // per-dropped-command noise to the harness.
                if let Some(flag) = pause.as_ref() {
                    if flag.load(Ordering::Acquire) {
                        tracing::debug!(?cmd, "agent paused — dropping command");
                        continue;
                    }
                }
                if cmd_tx.send(cmd).await.is_err() {
                    break; // session actor closed the receiver
                }
            }
            Err(err) => {
                let ev = AgentEvent::Error {
                    message: format!("invalid command JSON: {err} (input: {trimmed})"),
                };
                let mut w = writer.lock().await;
                emit_event(&mut *w, &ev).await?;
            }
        }
    }
    Ok(())
}

async fn write_events<W>(
    mut event_rx: broadcast::Receiver<AgentEvent>,
    writer: Arc<Mutex<W>>,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    loop {
        match event_rx.recv().await {
            Ok(event) => {
                let mut w = writer.lock().await;
                emit_event(&mut *w, &event).await?;
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                let ev = AgentEvent::Error {
                    message: format!("event stream lagged; dropped {n} events"),
                };
                let mut w = writer.lock().await;
                emit_event(&mut *w, &ev).await?;
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    Ok(())
}

async fn emit_event<W>(writer: &mut W, event: &AgentEvent) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut line = serde_json::to_vec(event)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, AsyncWriteExt};

    /// End-to-end: feed a JSON command line into the reader half, see
    /// the matching `AgentCommand` arrive on `cmd_rx`. Then broadcast
    /// an `AgentEvent`, see the matching JSON line emerge on the
    /// writer half. Exercises the full duplex pipeline including the
    /// shared writer Mutex.
    #[tokio::test]
    async fn duplex_roundtrip() {
        // Peer ←→ codec wiring. `peer_*` is what an external client
        // would hold; the codec runs on the inside.
        let (peer_io, codec_io) = duplex(4096);
        let (peer_reader, mut peer_writer) = tokio::io::split(peer_io);
        let (codec_reader, codec_writer) = tokio::io::split(codec_io);

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<AgentCommand>(8);
        let (event_tx, _) = broadcast::channel::<AgentEvent>(8);
        let event_rx = event_tx.subscribe();

        let codec_task = tokio::spawn(run(codec_reader, codec_writer, cmd_tx, event_rx, None));

        // Peer sends a Cancel command as JSON.
        let line = serde_json::to_string(&AgentCommand::Cancel).unwrap() + "\n";
        peer_writer.write_all(line.as_bytes()).await.unwrap();
        peer_writer.flush().await.unwrap();

        // Codec forwards it on cmd_rx.
        let got = tokio::time::timeout(std::time::Duration::from_secs(1), cmd_rx.recv())
            .await
            .expect("cmd_rx recv timed out")
            .expect("cmd_rx closed");
        assert!(matches!(got, AgentCommand::Cancel));

        // Producer broadcasts an event; peer should read its JSON.
        event_tx
            .send(AgentEvent::Error {
                message: "smoke".into(),
            })
            .unwrap();
        let mut reader = BufReader::new(peer_reader);
        let mut buf = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            reader.read_line(&mut buf),
        )
        .await
        .expect("read_line timed out")
        .expect("read_line io");
        let ev: AgentEvent = serde_json::from_str(buf.trim()).expect("decode event");
        match ev {
            AgentEvent::Error { message } => assert_eq!(message, "smoke"),
            other => panic!("expected Error, got {other:?}"),
        }

        // Drop peer to close the connection; codec returns.
        drop(peer_writer);
        drop(reader);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), codec_task).await;
    }

    /// When the pause flag is `true`, agent commands are dropped
    /// silently. Verifies the Stage-5 takeover-gate behavior — the
    /// codec drops the inbound command and `cmd_rx` never observes it.
    #[tokio::test]
    async fn paused_codec_drops_commands_silently() {
        let (peer_io, codec_io) = duplex(4096);
        let (peer_reader, mut peer_writer) = tokio::io::split(peer_io);
        let (codec_reader, codec_writer) = tokio::io::split(codec_io);

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<AgentCommand>(8);
        let (event_tx, _) = broadcast::channel::<AgentEvent>(8);
        let event_rx = event_tx.subscribe();
        let pause = Arc::new(AtomicBool::new(true));

        let _codec_task = tokio::spawn(run(
            codec_reader,
            codec_writer,
            cmd_tx,
            event_rx,
            Some(pause.clone()),
        ));

        let line = serde_json::to_string(&AgentCommand::Cancel).unwrap() + "\n";
        peer_writer.write_all(line.as_bytes()).await.unwrap();
        peer_writer.flush().await.unwrap();

        // Give the codec time to read; cmd_rx should remain empty.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            cmd_rx.try_recv().is_err(),
            "paused codec must drop incoming commands"
        );

        // Unpause; subsequent command should flow.
        pause.store(false, Ordering::Release);
        let line = serde_json::to_string(&AgentCommand::Snapshot).unwrap() + "\n";
        peer_writer.write_all(line.as_bytes()).await.unwrap();
        peer_writer.flush().await.unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(1), cmd_rx.recv())
            .await
            .expect("cmd_rx timeout after resume")
            .expect("cmd_rx closed");
        assert!(matches!(got, AgentCommand::Snapshot));

        // Silence the unused-imports lint cleanly: ensure peer_reader
        // is held for the duration of the test.
        drop(peer_reader);
    }

    /// Malformed JSON on the read side produces an `AgentEvent::Error`
    /// on the write side rather than crashing the codec.
    #[tokio::test]
    async fn malformed_command_yields_error_event() {
        let (peer_io, codec_io) = duplex(4096);
        let (peer_reader, mut peer_writer) = tokio::io::split(peer_io);
        let (codec_reader, codec_writer) = tokio::io::split(codec_io);

        let (cmd_tx, _cmd_rx) = mpsc::channel::<AgentCommand>(8);
        let (event_tx, _) = broadcast::channel::<AgentEvent>(8);
        let event_rx = event_tx.subscribe();

        let _codec_task = tokio::spawn(run(codec_reader, codec_writer, cmd_tx, event_rx, None));

        peer_writer.write_all(b"not json\n").await.unwrap();
        peer_writer.flush().await.unwrap();

        let mut reader = BufReader::new(peer_reader);
        let mut buf = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            reader.read_line(&mut buf),
        )
        .await
        .expect("read_line timed out")
        .expect("read_line io");
        let ev: AgentEvent = serde_json::from_str(buf.trim()).expect("decode event");
        match ev {
            AgentEvent::Error { message } => {
                assert!(message.contains("invalid command JSON"), "got: {message}");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }
}
