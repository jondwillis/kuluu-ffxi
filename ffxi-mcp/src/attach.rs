//! Attach mode: connect to a long-lived `ffxi-client play
//! --agent-listen` Unix socket instead of spawning a headless
//! supervisor subprocess.
//!
//! Enabled when `FFXI_ATTACH=<path|auto>` is set. Replaces the
//! `supervisor::run` task in `main.rs`. The MCP server's surrounding
//! plumbing (state mirror, notifier, sidecar, FfxiServer) is
//! transport-agnostic and works unchanged: this module just bridges
//! the socket's JSON-line `AgentCommand`/`AgentEvent` protocol into
//! the same `(cmd_rx, event_tx)` channels the supervisor would have
//! produced.
//!
//! # Reconnect
//!
//! On peer disconnect we sleep with capped exponential backoff
//! (250 ms → 8 s) and retry. On every successful (re)connect we
//! immediately send `AgentCommand::Snapshot` so the state mirror
//! rebuilds from a known-good frame.
//!
//! # Known limitations (v1)
//!
//! - `goal://current` reads from the MCP-side `goal_store` on disk;
//!   in attach mode no supervisor is writing it locally. The
//!   in-process state mirror still has the correct `current_goal`
//!   from the event stream, but the disk-backed resource will lag
//!   until we wire a parallel goal-persister here.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use ffxi_client::state::{AgentCommand, AgentEvent};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, mpsc};

/// Resolve a `FFXI_ATTACH` value to a concrete socket path.
///
/// - `auto` → read `${TMPDIR}/ffxi-agent.pid` (written by
///   `ffxi-client` when invoked with `--agent-listen auto`), parse
///   its `"sock"` field, use that path.
/// - Any other value is taken as a literal path.
pub fn resolve_attach(arg: &str) -> Result<PathBuf> {
    if arg.eq_ignore_ascii_case("auto") {
        let pidfile = std::env::temp_dir().join("ffxi-agent.pid");
        let body = std::fs::read_to_string(&pidfile).with_context(|| {
            format!(
                "reading agent pidfile {} (was ffxi-client started with `--agent-listen auto`?)",
                pidfile.display()
            )
        })?;
        let v: serde_json::Value = serde_json::from_str(&body)
            .with_context(|| format!("parsing pidfile JSON at {}", pidfile.display()))?;
        let sock = v
            .get("sock")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("pidfile missing `sock` field"))?;
        Ok(PathBuf::from(sock))
    } else {
        Ok(PathBuf::from(arg))
    }
}

/// Connect, bridge, reconnect forever. Returns only on unrecoverable
/// error (currently: the `cmd_rx` producer was dropped, which means
/// the MCP server is shutting down).
pub async fn run(
    sock: PathBuf,
    mut cmd_rx: mpsc::Receiver<AgentCommand>,
    event_tx: broadcast::Sender<AgentEvent>,
) -> Result<()> {
    let mut backoff = Duration::from_millis(250);
    let max_backoff = Duration::from_secs(8);

    loop {
        match UnixStream::connect(&sock).await {
            Ok(stream) => {
                eprintln!("ffxi-mcp attached to agent socket at {}", sock.display());
                tracing::info!(path = %sock.display(), "attached to ffxi-client agent socket");
                backoff = Duration::from_millis(250);

                match serve_peer(stream, &mut cmd_rx, &event_tx).await {
                    Ok(()) => {
                        // The peer cleanly closed (EOF on the read
                        // half). Treat it like any disconnect — back
                        // off and retry.
                        tracing::info!("agent socket peer closed; reconnecting");
                    }
                    Err(err) => {
                        // cmd_rx producer dropped: nothing left to
                        // do, propagate so main can shut down cleanly.
                        if err.to_string().contains("cmd_rx closed") {
                            return Err(err);
                        }
                        tracing::warn!(error = %err,
                            "agent socket peer ended with error; reconnecting");
                    }
                }
            }
            Err(err) => {
                tracing::debug!(error = %err, path = %sock.display(),
                    "agent socket connect failed; will retry");
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = std::cmp::min(backoff * 2, max_backoff);
    }
}

async fn serve_peer(
    stream: UnixStream,
    cmd_rx: &mut mpsc::Receiver<AgentCommand>,
    event_tx: &broadcast::Sender<AgentEvent>,
) -> Result<()> {
    let (reader_h, mut writer_h) = stream.into_split();
    let mut lines = BufReader::new(reader_h).lines();

    // On (re)attach, immediately ask the producer for a Snapshot so
    // the local SessionState mirror rebuilds from a known-good frame.
    // Without this the harness sees stale `scene://current` data
    // right after reattach, which is confusing.
    write_command(&mut writer_h, &AgentCommand::Snapshot).await?;

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else {
                    anyhow::bail!("cmd_rx closed");
                };
                write_command(&mut writer_h, &cmd).await?;
            }
            line = lines.next_line() => {
                match line {
                    Ok(Some(s)) => {
                        let s = s.trim();
                        if s.is_empty() { continue; }
                        match serde_json::from_str::<AgentEvent>(s) {
                            Ok(ev) => {
                                // `send` returns Err only when no receivers exist.
                                // The state mirror, notifier, and (optionally)
                                // sidecar all subscribe at startup, so a 0-receiver
                                // outcome implies catastrophic teardown; drop and
                                // continue is fine — the next loop will catch the
                                // shutdown via `cmd_rx.recv() == None`.
                                let _ = event_tx.send(ev);
                            }
                            Err(err) => {
                                tracing::warn!(error = %err, line = %s,
                                    "failed to decode AgentEvent from agent socket");
                            }
                        }
                    }
                    Ok(None) => return Ok(()), // peer closed (EOF)
                    Err(err) => return Err(err.into()),
                }
            }
        }
    }
}

async fn write_command<W>(writer: &mut W, cmd: &AgentCommand) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let mut line = serde_json::to_vec(cmd)?;
    line.push(b'\n');
    writer
        .write_all(&line)
        .await
        .context("writing AgentCommand to agent socket")?;
    writer
        .flush()
        .await
        .context("flushing agent socket writer")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_client::agent_socket;
    use std::time::Duration;

    fn temp_sock(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "ffxi-mcp-attach-{}-{}.sock",
            label,
            std::process::id()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    /// End-to-end smoke: a real Unix-socket listener served by
    /// `agent_socket::serve` plays the role of `ffxi-client`. `attach::run`
    /// is the MCP-side bridge. Verifies that the bootstrap Snapshot
    /// arrives, that a follow-up command flows MCP → listener, and that
    /// an event flows listener → MCP.
    #[tokio::test]
    async fn attach_bridges_commands_and_events() {
        let sock = temp_sock("commands");

        let (client_cmd_tx, mut client_cmd_rx) = mpsc::channel::<AgentCommand>(8);
        let (client_event_tx, _) = broadcast::channel::<AgentEvent>(8);
        let listen = agent_socket::ResolvedListen {
            sock: sock.clone(),
            pidfile: None,
        };
        let serve_event_tx = client_event_tx.clone();
        let _serve = tokio::spawn(async move {
            let _ = agent_socket::serve(listen, client_cmd_tx, serve_event_tx, None).await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let (mcp_cmd_tx, mcp_cmd_rx) = mpsc::channel::<AgentCommand>(8);
        let (mcp_event_tx, mut mcp_event_rx) = broadcast::channel::<AgentEvent>(16);
        let attach_event_tx = mcp_event_tx.clone();
        let attach_sock = sock.clone();
        let _attach = tokio::spawn(async move {
            let _ = run(attach_sock, mcp_cmd_rx, attach_event_tx).await;
        });

        // On (re)attach the bridge sends an immediate Snapshot.
        let first = tokio::time::timeout(Duration::from_secs(2), client_cmd_rx.recv())
            .await
            .expect("listener never received bootstrap Snapshot")
            .expect("listener cmd_rx closed");
        assert!(
            matches!(first, AgentCommand::Snapshot),
            "expected bootstrap Snapshot, got {first:?}"
        );

        mcp_cmd_tx
            .send(AgentCommand::Cancel)
            .await
            .expect("mcp_cmd_tx send");
        let got = tokio::time::timeout(Duration::from_secs(2), client_cmd_rx.recv())
            .await
            .expect("listener never received Cancel")
            .expect("listener cmd_rx closed");
        assert!(matches!(got, AgentCommand::Cancel), "got {got:?}");

        client_event_tx
            .send(AgentEvent::Error {
                message: "from-listener".into(),
            })
            .expect("client_event_tx broadcast");
        let got = tokio::time::timeout(Duration::from_secs(2), mcp_event_rx.recv())
            .await
            .expect("mcp_event_rx never received event")
            .expect("mcp_event_rx closed");
        match got {
            AgentEvent::Error { message } => assert_eq!(message, "from-listener"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_attach_literal_path_passes_through() {
        let p = resolve_attach("/tmp/explicit.sock").expect("resolve");
        assert_eq!(p, PathBuf::from("/tmp/explicit.sock"));
    }

    #[test]
    fn resolve_attach_auto_reads_pidfile() {
        // The pidfile path is process-global (`${TMPDIR}/ffxi-agent.pid`), so
        // run this serially via a sentinel to avoid colliding with a real
        // ffxi-client. We write, read, then delete.
        let pidfile = std::env::temp_dir().join("ffxi-agent.pid");
        let sock = std::env::temp_dir().join(format!(
            "ffxi-agent-attach-resolve-{}.sock",
            std::process::id()
        ));
        let body = serde_json::json!({ "pid": 42u32, "sock": sock.to_string_lossy() });
        std::fs::write(&pidfile, body.to_string()).expect("write pidfile");

        let p = resolve_attach("auto").expect("resolve auto");
        assert_eq!(p, sock);

        let _ = std::fs::remove_file(&pidfile);
    }
}
