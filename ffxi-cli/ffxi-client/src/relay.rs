//! WebSocket relay that publishes the same in-process state stream the
//! native viewer reads.
//!
//! This is Stage 2 of the operator-viewer plan: external tooling (and the
//! future wasm browser viewer) connects over a local WebSocket and receives
//! `ffxi_viewer_wire::Frame` values — the exact same shape the in-process
//! `NativeSource` produces. Each connection is independent: it owns a
//! `broadcast::Receiver<AgentEvent>` (via `event_tx.subscribe()`) and a
//! cloned `watch::Receiver<SessionState>`. n viewers cost n tokio tasks —
//! no fan-out machinery in the producer.
//!
//! # Wire format
//!
//! - Default: postcard binary, sent as `Message::Binary`.
//! - With `?format=json` query parameter: serde-JSON, sent as `Message::Text`.
//!   For human inspection (`wscat`, browser DevTools); commands inbound are
//!   binary-only either way (clients control encoding for both directions
//!   with a single setting; we keep the inbound side simple).
//!
//! # Per-connection lifecycle
//!
//! 1. Accept TCP, do the WebSocket upgrade.
//! 2. Send `Frame::Hello { protocol_version }`.
//! 3. Send an initial `Frame::Snapshot` from the current `state_rx` borrow.
//! 4. Loop on `tokio::select!`:
//!    - `state_rx.changed()` → re-emit `Frame::Snapshot`.
//!    - `event_rx.recv()` → translate to `ViewerEvent` and emit `Frame::Event`.
//!    - inbound `Message::Binary` → decode `ClientFrame::Command` and forward
//!      to `cmd_tx`. Other inbound messages (Text, Ping, Close) are handled
//!      via tungstenite's auto-pong; close ends the loop.
//!
//! Stage 2.0 sends a full snapshot on every state change. `Frame::Delta`
//! optimization is Stage 2.1; deferred per the plan.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use ffxi_viewer_wire::{self as wire, ClientFrame, Frame, PROTOCOL_VERSION};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_tungstenite::tungstenite::{
    handshake::server::{ErrorResponse, Request, Response},
    Message,
};

use crate::state::{AgentCommand, AgentEvent, SessionState};
use crate::wire_translate::{event_to_viewer_event, state_to_snapshot};

/// Run the WebSocket listener until the listener task is cancelled or the
/// channels shut down. One `serve` call per `--relay-listen` flag — call
/// from `tokio::spawn`.
///
/// # Panics
///
/// Doesn't. Errors during accept are logged and the loop continues; only
/// a fatal listener bind failure surfaces as the returned error.
pub async fn serve(
    addr: SocketAddr,
    state_rx: watch::Receiver<SessionState>,
    event_tx: broadcast::Sender<AgentEvent>,
    cmd_tx: mpsc::Sender<AgentCommand>,
) -> Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding relay listener to {addr}"))?;
    let bound = listener
        .local_addr()
        .ok()
        .map(|a| a.to_string())
        .unwrap_or_else(|| addr.to_string());
    tracing::info!(addr = %bound, "ffxi viewer relay listening");

    let cmd_tx = Arc::new(cmd_tx);

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(error = %err, "relay accept failed");
                continue;
            }
        };
        let state_rx = state_rx.clone();
        let event_rx = event_tx.subscribe();
        let cmd_tx = cmd_tx.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, peer, state_rx, event_rx, cmd_tx).await {
                tracing::debug!(peer = %peer, error = %err, "relay connection ended");
            }
        });
    }
}

/// Per-connection driver. Holds its own subscribers; a slow consumer
/// only affects its own queue.
async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    mut state_rx: watch::Receiver<SessionState>,
    mut event_rx: broadcast::Receiver<AgentEvent>,
    cmd_tx: Arc<mpsc::Sender<AgentCommand>>,
) -> Result<()> {
    // Capture the request URI during the handshake so we can read the
    // `?format=json` query parameter. tungstenite's accept_hdr_async hands
    // us the request right before the upgrade.
    let mut want_json = false;
    let want_json_ref = &mut want_json;
    let ws_stream = tokio_tungstenite::accept_hdr_async(
        stream,
        |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
            if let Some(query) = req.uri().query() {
                if query.split('&').any(|kv| kv == "format=json") {
                    *want_json_ref = true;
                }
            }
            Ok(resp)
        },
    )
    .await
    .context("websocket handshake")?;

    let format = if want_json {
        WireFormat::Json
    } else {
        WireFormat::Postcard
    };
    tracing::debug!(peer = %peer, ?format, "relay client connected");

    let (mut sink, mut stream) = ws_stream.split();

    // 1. Hello.
    send_frame(
        &mut sink,
        format,
        &Frame::Hello {
            protocol_version: PROTOCOL_VERSION,
        },
    )
    .await?;

    // 2. Initial snapshot. We borrow without `_and_update` here so we
    //    don't race with the changed() in the main loop — the first
    //    iteration will see a "no change" until something actually moves.
    {
        let snap = {
            let guard = state_rx.borrow();
            state_to_snapshot(&guard)
        };
        // Mark seen so the first state_rx.changed() below waits for an
        // *actual* change, not the initial value.
        let _ = state_rx.borrow_and_update();
        send_frame(&mut sink, format, &Frame::Snapshot(Box::new(snap))).await?;
    }

    // 3. Main loop.
    loop {
        tokio::select! {
            // State change → re-snapshot.
            changed = state_rx.changed() => {
                if changed.is_err() {
                    // Producer side dropped; clean exit.
                    break;
                }
                let snap = {
                    let guard = state_rx.borrow_and_update();
                    state_to_snapshot(&guard)
                };
                send_frame(&mut sink, format, &Frame::Snapshot(Box::new(snap))).await?;
            }
            // Agent event → translate → forward.
            ev = event_rx.recv() => match ev {
                Ok(ev) => {
                    if let Some(translated) = event_to_viewer_event(ev) {
                        send_frame(&mut sink, format, &Frame::Event(translated)).await?;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Slow client missed events — they'll catch up on the
                    // next snapshot. State, by definition, converges.
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            // Inbound from client.
            msg = stream.next() => match msg {
                Some(Ok(Message::Binary(data))) => {
                    match postcard::from_bytes::<ClientFrame>(&data) {
                        Ok(ClientFrame::Command(cmd)) => {
                            if let Some(translated) = viewer_command_to_agent(cmd) {
                                if cmd_tx.send(translated).await.is_err() {
                                    // Session shut down.
                                    break;
                                }
                            }
                        }
                        Ok(ClientFrame::Hello { .. }) => {
                            // Optional client hello; we accept any version
                            // for now and let the schema's additive
                            // discipline handle compat.
                        }
                        Err(err) => {
                            tracing::debug!(peer = %peer, error = %err, "decoding ClientFrame failed");
                        }
                    }
                }
                Some(Ok(Message::Text(_))) => {
                    // Inbound JSON commands are intentionally not supported;
                    // the relay's JSON mode is for outbound debugging only.
                    tracing::trace!(peer = %peer, "ignoring inbound text frame");
                }
                Some(Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {} // Ping/Pong/Frame handled by tungstenite.
                Some(Err(err)) => {
                    tracing::debug!(peer = %peer, error = %err, "websocket read error");
                    break;
                }
                None => break,
            }
        }
    }

    let _ = sink.close().await;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum WireFormat {
    Postcard,
    Json,
}

async fn send_frame<S>(sink: &mut S, format: WireFormat, frame: &Frame) -> Result<()>
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let msg = match format {
        WireFormat::Postcard => {
            let bytes = postcard::to_allocvec(frame).context("postcard encoding Frame")?;
            Message::Binary(bytes.into())
        }
        WireFormat::Json => {
            let s = serde_json::to_string(frame).context("json encoding Frame")?;
            Message::Text(s.into())
        }
    };
    sink.send(msg).await.context("sending websocket frame")?;
    Ok(())
}

/// Map a `wire::ViewerCommand` (a deliberately small subset of
/// `state::AgentCommand`) onto the full session command. The wire's
/// command vocabulary is a strict subset by design — adding a richer
/// command requires extending the wire schema first.
fn viewer_command_to_agent(cmd: wire::ViewerCommand) -> Option<AgentCommand> {
    Some(match cmd {
        wire::ViewerCommand::Move { x, y, z, heading } => {
            AgentCommand::Move { x, y, z, heading }
        }
        wire::ViewerCommand::StopMove => AgentCommand::StopMove,
        wire::ViewerCommand::EndEvent => AgentCommand::EndEvent,
        wire::ViewerCommand::Snapshot => AgentCommand::Snapshot,
        wire::ViewerCommand::Chat { kind, text } => AgentCommand::Chat { kind, text },
        wire::ViewerCommand::Tell { to, text } => AgentCommand::Tell { to, text },
        wire::ViewerCommand::Follow { target_id, distance } => {
            AgentCommand::Follow { target_id, distance }
        }
        wire::ViewerCommand::Engage { target_id } => AgentCommand::Engage { target_id },
        wire::ViewerCommand::PathTo { x, y, z } => AgentCommand::PathTo { x, y, z },
        wire::ViewerCommand::Cancel => AgentCommand::Cancel,
    })
}
