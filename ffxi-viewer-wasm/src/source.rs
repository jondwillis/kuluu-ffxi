//! `WasmSource` — `SceneSource` impl for the browser viewer.
//!
//! Architecture:
//!
//! ```text
//! gloo_net::websocket ──► spawn_local task ──► flume channels ──► WasmSource ──► ECS
//! ```
//!
//! Bevy systems are sync, but `gloo_net::websocket` is `Stream`-based async.
//! A `wasm_bindgen_futures::spawn_local` task owns the WebSocket and decodes
//! postcard `Frame`s into bounded flume channels; `SceneSource::poll_*` then
//! `try_recv` from those channels at frame rate, never blocking.
//!
//! Channels are unbounded — the producer is one decode task running on the
//! browser's microtask queue, the consumer is the Bevy frame loop on the
//! same thread (wasm32 is single-threaded). The producer can't outpace the
//! consumer by more than a tick or two in practice, and bounding would just
//! risk dropping frames during burst delivery on connect.

use bevy::prelude::*;
use ffxi_viewer_wire::{
    ClientFrame, Frame, SceneDelta, SceneSnapshot, ViewerEvent, PROTOCOL_VERSION,
};
use ffxi_viewer_core::SceneSource;
use flume::{Receiver, Sender};
use futures_util::{SinkExt, StreamExt};
use gloo_net::websocket::{futures::WebSocket, Message};
use log::{info, warn};
use wasm_bindgen_futures::spawn_local;

/// Bevy `Resource` that owns the receiving ends of the decode-task channels.
#[derive(Resource)]
pub struct WasmSource {
    snapshot_rx: Receiver<Box<SceneSnapshot>>,
    event_rx: Receiver<ViewerEvent>,
    /// Outbound command channel. Stage 3 deferred — kept here so the wiring
    /// is in place for a follow-up; nothing in the browser viewer issues
    /// commands today.
    #[allow(dead_code)]
    command_tx: Sender<ClientFrame>,
}

impl WasmSource {
    /// Open a WebSocket against `ws_url`, spawn the decode task, and return
    /// a `Resource` ready to be inserted into the Bevy app.
    ///
    /// Failure to connect is reported via `log::warn!` from the decode task;
    /// the resource is still constructed (with empty channels) so the app
    /// boots and the user can see the chrome HUD even with no relay.
    pub fn connect(ws_url: &str) -> Self {
        let (snapshot_tx, snapshot_rx) = flume::unbounded::<Box<SceneSnapshot>>();
        let (event_tx, event_rx) = flume::unbounded::<ViewerEvent>();
        let (command_tx, command_rx) = flume::unbounded::<ClientFrame>();

        let url = ws_url.to_owned();
        spawn_local(async move {
            run_socket(url, snapshot_tx, event_tx, command_rx).await;
        });

        Self {
            snapshot_rx,
            event_rx,
            command_tx,
        }
    }
}

impl SceneSource for WasmSource {
    /// Drain the snapshot channel and return the most recent one (older
    /// snapshots are wholesale replaced by newer ones, so dropping them is
    /// the right thing).
    fn poll_snapshot(&mut self) -> Option<Box<SceneSnapshot>> {
        let mut latest: Option<Box<SceneSnapshot>> = None;
        while let Ok(s) = self.snapshot_rx.try_recv() {
            latest = Some(s);
        }
        latest
    }

    /// Stage 2.0 sends full snapshots only. When Stage 2.1 starts emitting
    /// `Frame::Delta`, the decode task will need a third channel and this
    /// returns its drained contents.
    fn drain_deltas(&mut self) -> Vec<SceneDelta> {
        Vec::new()
    }

    fn drain_events(&mut self) -> Vec<ViewerEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.event_rx.try_recv() {
            out.push(ev);
        }
        out
    }
}

/// The decode task. Owns the WebSocket; lives until the connection closes
/// or this future is dropped (which happens on tab close).
async fn run_socket(
    url: String,
    snapshot_tx: Sender<Box<SceneSnapshot>>,
    event_tx: Sender<ViewerEvent>,
    command_rx: Receiver<ClientFrame>,
) {
    let ws = match WebSocket::open(&url) {
        Ok(ws) => ws,
        Err(e) => {
            warn!("ffxi-viewer-wasm: WebSocket::open({url}) failed: {e:?}");
            return;
        }
    };
    info!("ffxi-viewer-wasm: connected to {url}");

    let (mut sink, mut stream) = ws.split();

    // Optional outbound command pump — drained on each iteration of the
    // recv select. Today nothing fills `command_rx`, but the channel exists
    // so a future input layer can issue `ClientFrame::Command`s.
    let _ = command_rx; // suppress unused-import warning; sender held in resource

    while let Some(msg) = stream.next().await {
        match msg {
            Ok(Message::Bytes(bytes)) => {
                match postcard::from_bytes::<Frame>(&bytes) {
                    Ok(Frame::Hello { protocol_version }) => {
                        if protocol_version != PROTOCOL_VERSION {
                            warn!(
                                "ffxi-viewer-wasm: protocol version mismatch \
                                 (server={protocol_version}, viewer={PROTOCOL_VERSION}); \
                                 continuing optimistically"
                            );
                        } else {
                            info!(
                                "ffxi-viewer-wasm: hello, protocol_version={protocol_version}"
                            );
                        }
                    }
                    Ok(Frame::Snapshot(snap)) => {
                        if snapshot_tx.send(snap).is_err() {
                            // Receiver dropped — app is shutting down.
                            break;
                        }
                    }
                    Ok(Frame::Delta(_)) => {
                        // Stage 2.1 — ignore for now.
                    }
                    Ok(Frame::Event(ev)) => {
                        if event_tx.send(ev).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("ffxi-viewer-wasm: postcard decode error: {e:?}");
                    }
                }
            }
            Ok(Message::Text(t)) => {
                // The relay defaults to binary; text arrives only via the
                // `?format=json` debug query param, which we don't request.
                warn!("ffxi-viewer-wasm: unexpected text frame ({} bytes), ignoring", t.len());
            }
            Err(e) => {
                warn!("ffxi-viewer-wasm: websocket recv error: {e:?}");
                break;
            }
        }
    }

    // Best-effort: close the sink so the relay sees a clean shutdown.
    let _ = sink.close().await;
    info!("ffxi-viewer-wasm: socket closed");
}
