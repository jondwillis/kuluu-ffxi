use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, watch};

use crate::agent_codec;
use crate::state::{AgentCommand, AgentEvent};

pub const AGENT_PIDFILE_NAME: &str = "ffxi-agent.pid";

// kuluu-smlg: macOS $TMPDIR purges have unlinked the live socket mid-run, so
// poll our own path between accepts and re-bind when it vanishes.
const SOCKET_LIVENESS_INTERVAL: Duration = Duration::from_secs(2);

pub fn resolve_listen(arg: &str) -> ResolvedListen {
    if arg.eq_ignore_ascii_case("auto") {
        let tmp = std::env::temp_dir();
        let pid = std::process::id();
        ResolvedListen {
            sock: tmp.join(format!("ffxi-agent-{pid}.sock")),
            pidfile: Some(tmp.join(AGENT_PIDFILE_NAME)),
        }
    } else {
        ResolvedListen {
            sock: PathBuf::from(arg),
            pidfile: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedListen {
    pub sock: PathBuf,

    pub pidfile: Option<PathBuf>,
}

/// Channels of the session the socket currently serves. Swapping the value
/// mid-listen drops connected peers, so an agent reconnecting after a relogin
/// in the same window lands on the live session instead of the dead one.
#[derive(Clone)]
pub struct SessionChannels {
    pub cmd_tx: mpsc::Sender<AgentCommand>,
    pub event_tx: broadcast::Sender<AgentEvent>,
    pub pause: Option<Arc<AtomicBool>>,
    pub debug_ctrl: Option<crate::debug_control::SharedDebugControl>,
}

pub async fn serve(
    listen: ResolvedListen,
    cmd_tx: mpsc::Sender<AgentCommand>,
    event_tx: broadcast::Sender<AgentEvent>,
    pause: Option<Arc<AtomicBool>>,
    debug_ctrl: Option<crate::debug_control::SharedDebugControl>,
) -> Result<()> {
    let (sessions_tx, sessions_rx) = watch::channel(Some(SessionChannels {
        cmd_tx,
        event_tx,
        pause,
        debug_ctrl,
    }));
    let _keep_sender_alive = sessions_tx;
    serve_dynamic(listen, sessions_rx).await
}

/// Like `serve`, but the session behind the socket can be swapped (or cleared
/// with `None`, e.g. while the window sits at the launcher) by sending on the
/// watch channel. The listener itself is bound once for the life of the task.
/// Each peer reads the current channels with `borrow_and_update` so the shared
/// marker is current; the per-peer receiver clone then fires only on the NEXT
/// swap, not the one already in effect.
pub async fn serve_dynamic(
    listen: ResolvedListen,
    mut sessions: watch::Receiver<Option<SessionChannels>>,
) -> Result<()> {
    let ResolvedListen { sock, pidfile } = listen;

    let mut listener = bind_listener(&sock).await?;

    eprintln!("agent socket listening on {}", sock.display());
    tracing::info!(path = %sock.display(), "ffxi agent socket listening");

    write_pidfile(pidfile.as_deref(), &sock);

    let _cleanup = SocketCleanup {
        sock: sock.clone(),
        pidfile: pidfile.clone(),
    };

    let mut liveness = tokio::time::interval(SOCKET_LIVENESS_INTERVAL);
    liveness.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        let (stream, _addr) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(pair) => pair,
                Err(err) => {
                    tracing::warn!(error = %err, "agent socket accept failed");
                    continue;
                }
            },
            _ = liveness.tick() => {
                if !sock.exists() {
                    tracing::warn!(path = %sock.display(),
                        "agent socket path vanished from disk; re-binding");
                    match bind_listener(&sock).await {
                        Ok(rebound) => {
                            listener = rebound;
                            // Reclaim the shared pidfile only when it is gone or
                            // still ours: a newer instance's pointer must win.
                            let reclaim = pidfile
                                .as_deref()
                                .filter(|p| !p.exists() || pidfile_bears_our_pid(p));
                            write_pidfile(reclaim, &sock);
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, path = %sock.display(),
                                "agent socket re-bind failed; retrying next liveness tick");
                        }
                    }
                }
                continue;
            }
        };
        let (reader, writer) = stream.into_split();
        let Some(channels) = sessions.borrow_and_update().clone() else {
            let mut writer = writer;
            let ev = AgentEvent::Error {
                message: "no active session (window is at the launcher)".into(),
            };
            let _ = agent_codec::emit_event(&mut writer, &ev).await;
            continue;
        };
        tracing::info!("agent socket peer connected");
        let event_rx = channels.event_tx.subscribe();
        let mut swap = sessions.clone();

        let codec = agent_codec::run(
            reader,
            writer,
            channels.cmd_tx,
            event_rx,
            channels.pause,
            channels.debug_ctrl,
        );
        tokio::select! {
            result = codec => {
                if let Err(err) = result {
                    tracing::debug!(error = %err, "agent socket peer ended with error");
                } else {
                    tracing::info!("agent socket peer disconnected");
                }
            }
            changed = swap.changed() => {
                if changed.is_ok() {
                    tracing::info!("agent socket session swapped; dropping peer");
                }
            }
        }
    }
}

async fn bind_listener(sock: &Path) -> Result<UnixListener> {
    if sock.exists() {
        match UnixStream::connect(sock).await {
            Ok(_) => {
                anyhow::bail!(
                    "agent socket {} is already in use (another kuluu is listening); \
                     pick a different `--agent-listen` path or stop the other instance",
                    sock.display()
                );
            }
            Err(_) => {
                let _ = std::fs::remove_file(sock);
            }
        }
    }

    UnixListener::bind(sock).with_context(|| format!("binding agent socket at {}", sock.display()))
}

fn write_pidfile(pidfile: Option<&Path>, sock: &Path) {
    let Some(path) = pidfile else {
        return;
    };
    let body = serde_json::json!({
        "pid": std::process::id(),
        "sock": sock.to_string_lossy(),
    });
    if let Err(err) = std::fs::write(path, body.to_string()) {
        tracing::warn!(error = %err, path = %path.display(),
            "failed to write agent pidfile (continuing without autodiscovery)");
    }
}

fn pidfile_bears_our_pid(path: &Path) -> bool {
    let Ok(body) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
        return false;
    };
    v.get("pid").and_then(serde_json::Value::as_u64) == Some(u64::from(std::process::id()))
}

struct SocketCleanup {
    sock: PathBuf,
    pidfile: Option<PathBuf>,
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.sock);
        if let Some(p) = self.pidfile.as_ref() {
            if pidfile_bears_our_pid(p) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(label: &str, ext: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "kuluu-session-agent-{label}-{}.{ext}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[tokio::test]
    async fn serve_rebinds_when_socket_path_vanishes() {
        let sock = temp_path("rebind", "sock");
        let (cmd_tx, _cmd_rx) = mpsc::channel::<AgentCommand>(8);
        let (event_tx, _keep_alive) = broadcast::channel::<AgentEvent>(8);
        let listen = ResolvedListen {
            sock: sock.clone(),
            pidfile: None,
        };
        let _serve = tokio::spawn(serve(listen, cmd_tx, event_tx.clone(), None, None));

        for _ in 0..100 {
            if sock.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(sock.exists(), "serve never bound {}", sock.display());

        let peer = UnixStream::connect(&sock)
            .await
            .expect("connect while live");
        drop(peer);
        for _ in 0..20 {
            let _ = event_tx.send(AgentEvent::Error {
                message: "unstick peer writer".into(),
            });
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        std::fs::remove_file(&sock).expect("remove live socket out from under serve");

        let deadline = std::time::Instant::now() + SOCKET_LIVENESS_INTERVAL * 3;
        let mut reappeared = false;
        while std::time::Instant::now() < deadline {
            if sock.exists() {
                reappeared = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(reappeared, "socket was not re-bound after external removal");

        UnixStream::connect(&sock)
            .await
            .expect("connect after re-bind");
        let _ = std::fs::remove_file(&sock);
    }

    #[tokio::test]
    async fn swapped_session_drops_peers_and_serves_new_session() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

        let sock = temp_path("swap", "sock");
        let (cmd_tx_a, mut cmd_rx_a) = mpsc::channel::<AgentCommand>(8);
        let (event_tx_a, _keep_a) = broadcast::channel::<AgentEvent>(8);
        let (cmd_tx_b, mut cmd_rx_b) = mpsc::channel::<AgentCommand>(8);
        let (event_tx_b, _keep_b) = broadcast::channel::<AgentEvent>(8);

        let channels = |cmd_tx, event_tx| {
            Some(SessionChannels {
                cmd_tx,
                event_tx,
                pause: None,
                debug_ctrl: None,
            })
        };
        let (swap_tx, swap_rx) = watch::channel(channels(cmd_tx_a, event_tx_a));
        let listen = ResolvedListen {
            sock: sock.clone(),
            pidfile: None,
        };
        let _serve = tokio::spawn(serve_dynamic(listen, swap_rx));

        for _ in 0..100 {
            if sock.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            sock.exists(),
            "serve_dynamic never bound {}",
            sock.display()
        );

        let cancel_line = serde_json::to_string(&AgentCommand::Cancel).unwrap() + "\n";

        let peer_a = UnixStream::connect(&sock)
            .await
            .expect("connect to session A");
        let (mut peer_a_reader, mut peer_a_writer) = peer_a.into_split();
        peer_a_writer
            .write_all(cancel_line.as_bytes())
            .await
            .unwrap();
        let got = tokio::time::timeout(Duration::from_secs(1), cmd_rx_a.recv())
            .await
            .expect("session A cmd timeout")
            .expect("session A cmd closed");
        assert!(matches!(got, AgentCommand::Cancel));

        swap_tx
            .send(channels(cmd_tx_b, event_tx_b))
            .expect("swap to session B");

        let mut byte = [0u8; 1];
        let n = tokio::time::timeout(Duration::from_secs(2), peer_a_reader.read(&mut byte))
            .await
            .expect("pre-swap peer never saw EOF")
            .expect("pre-swap peer read error");
        assert_eq!(n, 0, "pre-swap peer must be dropped on session swap");

        let peer_b = UnixStream::connect(&sock)
            .await
            .expect("connect after swap");
        let (_reader_b, mut writer_b) = peer_b.into_split();
        writer_b.write_all(cancel_line.as_bytes()).await.unwrap();
        let got = tokio::time::timeout(Duration::from_secs(1), cmd_rx_b.recv())
            .await
            .expect("session B cmd timeout")
            .expect("session B cmd closed");
        assert!(matches!(got, AgentCommand::Cancel));

        swap_tx.send(None).expect("clear session");
        let peer_none = UnixStream::connect(&sock)
            .await
            .expect("connect with no active session");
        let (reader_none, _writer_none) = peer_none.into_split();
        let mut line = String::new();
        tokio::time::timeout(
            Duration::from_secs(1),
            BufReader::new(reader_none).read_line(&mut line),
        )
        .await
        .expect("no-session error event timeout")
        .expect("no-session read error");
        let ev: AgentEvent = serde_json::from_str(line.trim()).expect("decode error event");
        match ev {
            AgentEvent::Error { message } => {
                assert!(message.contains("no active session"), "got: {message}");
            }
            other => panic!("expected Error, got {other:?}"),
        }

        let _ = std::fs::remove_file(&sock);
    }

    #[test]
    fn socket_cleanup_preserves_foreign_pidfile() {
        let pidfile = temp_path("foreign", "pid");
        let foreign_pid = u64::from(std::process::id()) + 1;
        let body = serde_json::json!({ "pid": foreign_pid, "sock": "/nonexistent.sock" });
        std::fs::write(&pidfile, body.to_string()).expect("write foreign pidfile");

        drop(SocketCleanup {
            sock: temp_path("foreign-sock", "sock"),
            pidfile: Some(pidfile.clone()),
        });

        assert!(
            pidfile.exists(),
            "drop removed a pidfile bearing a foreign pid"
        );
        let _ = std::fs::remove_file(&pidfile);
    }

    #[test]
    fn socket_cleanup_removes_own_pidfile() {
        let pidfile = temp_path("own", "pid");
        let body = serde_json::json!({ "pid": std::process::id(), "sock": "/nonexistent.sock" });
        std::fs::write(&pidfile, body.to_string()).expect("write own pidfile");

        drop(SocketCleanup {
            sock: temp_path("own-sock", "sock"),
            pidfile: Some(pidfile.clone()),
        });

        assert!(!pidfile.exists(), "drop left our own pidfile behind");
    }
}
