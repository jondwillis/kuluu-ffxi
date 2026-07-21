use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc, Mutex};

use crate::state::{AgentCommand, AgentEvent};

pub async fn run<R, W>(
    reader: R,
    writer: W,
    cmd_tx: mpsc::Sender<AgentCommand>,
    event_rx: broadcast::Receiver<AgentEvent>,
    pause: Option<Arc<AtomicBool>>,
    debug_ctrl: Option<crate::debug_control::SharedDebugControl>,
) -> Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let writer = Arc::new(Mutex::new(writer));
    let writer_for_reader = Arc::clone(&writer);
    tokio::try_join!(
        read_commands(reader, cmd_tx, writer_for_reader, pause, debug_ctrl),
        write_events(event_rx, writer),
    )?;
    Ok(())
}

// Focus-less GUI driving (kuluu-0pof): these route to the Bevy input path via
// the shared handle, never to the session. Returns true if consumed here.
fn apply_debug_command(
    cmd: &AgentCommand,
    debug_ctrl: &Option<crate::debug_control::SharedDebugControl>,
) -> bool {
    let Some(ctrl) = debug_ctrl.as_ref() else {
        return false;
    };
    match cmd {
        AgentCommand::DebugDrive {
            forward,
            strafe,
            duration_ms,
        } => {
            if let Ok(mut c) = ctrl.lock() {
                c.set_drive(*forward, *strafe, *duration_ms);
            }
            true
        }
        AgentCommand::DebugHeights => {
            if let Ok(mut c) = ctrl.lock() {
                c.request_heights();
            }
            true
        }
        _ => false,
    }
}

async fn read_commands<R, W>(
    reader: R,
    cmd_tx: mpsc::Sender<AgentCommand>,
    writer: Arc<Mutex<W>>,
    pause: Option<Arc<AtomicBool>>,
    debug_ctrl: Option<crate::debug_control::SharedDebugControl>,
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
                if apply_debug_command(&cmd, &debug_ctrl) {
                    continue;
                }
                if let Some(flag) = pause.as_ref() {
                    if flag.load(Ordering::Acquire) {
                        tracing::debug!(?cmd, "agent paused — dropping command");
                        continue;
                    }
                }
                if cmd_tx.send(cmd).await.is_err() {
                    break;
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

    #[tokio::test]
    async fn duplex_roundtrip() {
        let (peer_io, codec_io) = duplex(4096);
        let (peer_reader, mut peer_writer) = tokio::io::split(peer_io);
        let (codec_reader, codec_writer) = tokio::io::split(codec_io);

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<AgentCommand>(8);
        let (event_tx, _) = broadcast::channel::<AgentEvent>(8);
        let event_rx = event_tx.subscribe();

        let codec_task = tokio::spawn(run(
            codec_reader,
            codec_writer,
            cmd_tx,
            event_rx,
            None,
            None,
        ));

        let line = serde_json::to_string(&AgentCommand::Cancel).unwrap() + "\n";
        peer_writer.write_all(line.as_bytes()).await.unwrap();
        peer_writer.flush().await.unwrap();

        let got = tokio::time::timeout(std::time::Duration::from_secs(1), cmd_rx.recv())
            .await
            .expect("cmd_rx recv timed out")
            .expect("cmd_rx closed");
        assert!(matches!(got, AgentCommand::Cancel));

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

        drop(peer_writer);
        drop(reader);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), codec_task).await;
    }

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
            None,
        ));

        let line = serde_json::to_string(&AgentCommand::Cancel).unwrap() + "\n";
        peer_writer.write_all(line.as_bytes()).await.unwrap();
        peer_writer.flush().await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            cmd_rx.try_recv().is_err(),
            "paused codec must drop incoming commands"
        );

        pause.store(false, Ordering::Release);
        let line = serde_json::to_string(&AgentCommand::Snapshot).unwrap() + "\n";
        peer_writer.write_all(line.as_bytes()).await.unwrap();
        peer_writer.flush().await.unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(1), cmd_rx.recv())
            .await
            .expect("cmd_rx timeout after resume")
            .expect("cmd_rx closed");
        assert!(matches!(got, AgentCommand::Snapshot));

        drop(peer_reader);
    }

    #[tokio::test]
    async fn malformed_command_yields_error_event() {
        let (peer_io, codec_io) = duplex(4096);
        let (peer_reader, mut peer_writer) = tokio::io::split(peer_io);
        let (codec_reader, codec_writer) = tokio::io::split(codec_io);

        let (cmd_tx, _cmd_rx) = mpsc::channel::<AgentCommand>(8);
        let (event_tx, _) = broadcast::channel::<AgentEvent>(8);
        let event_rx = event_tx.subscribe();

        let _codec_task = tokio::spawn(run(
            codec_reader,
            codec_writer,
            cmd_tx,
            event_rx,
            None,
            None,
        ));

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
