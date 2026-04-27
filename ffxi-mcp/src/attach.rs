use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use ffxi_client::state::{AgentCommand, AgentEvent};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, mpsc};

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
                        tracing::info!("agent socket peer closed; reconnecting");
                    }
                    Err(err) => {
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

                                let _ = event_tx.send(ev);
                            }
                            Err(err) => {
                                tracing::warn!(error = %err, line = %s,
                                    "failed to decode AgentEvent from agent socket");
                            }
                        }
                    }
                    Ok(None) => return Ok(()),
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
