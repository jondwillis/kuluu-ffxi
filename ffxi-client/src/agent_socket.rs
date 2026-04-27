use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};

use crate::agent_codec;
use crate::state::{AgentCommand, AgentEvent};

pub fn resolve_listen(arg: &str) -> ResolvedListen {
    if arg.eq_ignore_ascii_case("auto") {
        let tmp = std::env::temp_dir();
        let pid = std::process::id();
        ResolvedListen {
            sock: tmp.join(format!("ffxi-agent-{pid}.sock")),
            pidfile: Some(tmp.join("ffxi-agent.pid")),
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

pub async fn serve(
    listen: ResolvedListen,
    cmd_tx: mpsc::Sender<AgentCommand>,
    event_tx: broadcast::Sender<AgentEvent>,
    pause: Option<Arc<AtomicBool>>,
) -> Result<()> {
    let ResolvedListen { sock, pidfile } = listen;

    if sock.exists() {
        match UnixStream::connect(&sock).await {
            Ok(_) => {
                anyhow::bail!(
                    "agent socket {} is already in use (another ffxi-client is listening); \
                     pick a different `--agent-listen` path or stop the other instance",
                    sock.display()
                );
            }
            Err(_) => {
                let _ = std::fs::remove_file(&sock);
            }
        }
    }

    let listener = UnixListener::bind(&sock)
        .with_context(|| format!("binding agent socket at {}", sock.display()))?;

    eprintln!("agent socket listening on {}", sock.display());
    tracing::info!(path = %sock.display(), "ffxi agent socket listening");

    if let Some(path) = pidfile.as_ref() {
        let pid = std::process::id();
        let body = serde_json::json!({
            "pid": pid,
            "sock": sock.to_string_lossy(),
        });
        if let Err(err) = std::fs::write(path, body.to_string()) {
            tracing::warn!(error = %err, path = %path.display(),
                "failed to write agent pidfile (continuing without autodiscovery)");
        }
    }

    let _cleanup = SocketCleanup {
        sock: sock.clone(),
        pidfile: pidfile.clone(),
    };

    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(error = %err, "agent socket accept failed");
                continue;
            }
        };
        tracing::info!("agent socket peer connected");
        let (reader, writer) = stream.into_split();
        let cmd_tx = cmd_tx.clone();
        let event_rx = event_tx.subscribe();
        let pause = pause.clone();

        if let Err(err) = agent_codec::run(reader, writer, cmd_tx, event_rx, pause).await {
            tracing::debug!(error = %err, "agent socket peer ended with error");
        } else {
            tracing::info!("agent socket peer disconnected");
        }
    }
}

struct SocketCleanup {
    sock: PathBuf,
    pidfile: Option<PathBuf>,
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.sock);
        if let Some(p) = self.pidfile.as_ref() {
            let _ = std::fs::remove_file(p);
        }
    }
}
