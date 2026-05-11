//! Unix-domain-socket listener for the `--agent-listen` mode.
//!
//! Speaks the same JSON-line [`AgentCommand`] / [`AgentEvent`] protocol
//! as [`crate::agent_io`]. Used by `ffxi-mcp` in attach mode
//! (`FFXI_ATTACH=…`) to drive a long-lived `native`-window client
//! without spawning a fresh headless subprocess.
//!
//! # Single-peer policy
//!
//! Only one peer is served at a time — the per-peer handler runs
//! inline inside the accept loop, so the next `accept()` is gated on
//! the current peer finishing. Subsequent connections queue in the
//! kernel's accept backlog. If two MCP servers raced for the same
//! client they'd issue contradictory commands; refusing that
//! upstream is much simpler than reconciling it downstream.
//!
//! # Stale-socket cleanup
//!
//! On startup, if the socket path already exists we probe-connect to
//! it. If the probe succeeds, another process is listening — we
//! refuse, rather than steal the path. If the probe fails, the file
//! is leftover from a previous crash; we unlink it before binding.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};

use crate::agent_codec;
use crate::state::{AgentCommand, AgentEvent};

/// Resolve a `--agent-listen` / `FFXI_AGENT_LISTEN` value.
///
/// - `auto` → `${TMPDIR}/ffxi-agent-{pid}.sock`, with a discovery
///   pidfile at `${TMPDIR}/ffxi-agent.pid` containing `{"pid":…,
///   "sock":…}`. The MCP attach mode reads this pidfile when invoked
///   with `FFXI_ATTACH=auto`.
/// - Any other value is taken as a literal filesystem path; no
///   pidfile is written.
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

/// Resolved listen target produced by [`resolve_listen`].
#[derive(Debug, Clone)]
pub struct ResolvedListen {
    pub sock: PathBuf,
    /// `Some` only when the user passed `auto` — the file the MCP side
    /// reads to autodiscover the socket path.
    pub pidfile: Option<PathBuf>,
}

/// Bind the socket and serve one peer at a time forever. Returns
/// `Err` on bind failure or unexpected I/O. On drop, the socket file
/// and pidfile (if any) are unlinked.
///
/// `pause` is an optional "human in control" flag — when set to
/// `true`, agent commands from the connected peer are dropped
/// (`/agent pause` in the native viewer). See [`agent_codec::run`].
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

    // Loud, unconditional stderr line mirroring relay::serve so the
    // bound path is discoverable even when RUST_LOG filters info-level
    // tracing. The MCP attach mode prints a complementary line on the
    // other side.
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
        // Block the accept loop until this peer disconnects. Single
        // peer at a time by construction.
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
