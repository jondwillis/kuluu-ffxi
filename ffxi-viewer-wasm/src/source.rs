use bevy::prelude::*;
use ffxi_viewer_core::SceneSource;
use ffxi_viewer_wire::{
    ClientFrame, Frame, SceneDelta, SceneSnapshot, ViewerEvent, PROTOCOL_VERSION,
};
use flume::{Receiver, Sender};
use futures_util::{SinkExt, StreamExt};
use gloo_net::websocket::{futures::WebSocket, Message};
use log::{info, warn};
use wasm_bindgen_futures::spawn_local;

#[derive(Resource)]
pub struct WasmSource {
    snapshot_rx: Receiver<Box<SceneSnapshot>>,
    event_rx: Receiver<ViewerEvent>,

    #[allow(dead_code)]
    command_tx: Sender<ClientFrame>,
}

impl WasmSource {
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
    fn poll_snapshot(&mut self) -> Option<Box<SceneSnapshot>> {
        let mut latest: Option<Box<SceneSnapshot>> = None;
        while let Ok(s) = self.snapshot_rx.try_recv() {
            latest = Some(s);
        }
        latest
    }

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

    let _ = command_rx;

    while let Some(msg) = stream.next().await {
        match msg {
            Ok(Message::Bytes(bytes)) => match postcard::from_bytes::<Frame>(&bytes) {
                Ok(Frame::Hello { protocol_version }) => {
                    if protocol_version != PROTOCOL_VERSION {
                        warn!(
                            "ffxi-viewer-wasm: protocol version mismatch \
                                 (server={protocol_version}, viewer={PROTOCOL_VERSION}); \
                                 continuing optimistically"
                        );
                    } else {
                        info!("ffxi-viewer-wasm: hello, protocol_version={protocol_version}");
                    }
                }
                Ok(Frame::Snapshot(snap)) => {
                    if snapshot_tx.send(snap).is_err() {
                        break;
                    }
                }
                Ok(Frame::Delta(_)) => {}
                Ok(Frame::Event(ev)) => {
                    if event_tx.send(ev).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    warn!("ffxi-viewer-wasm: postcard decode error: {e:?}");
                }
            },
            Ok(Message::Text(t)) => {
                warn!(
                    "ffxi-viewer-wasm: unexpected text frame ({} bytes), ignoring",
                    t.len()
                );
            }
            Err(e) => {
                warn!("ffxi-viewer-wasm: websocket recv error: {e:?}");
                break;
            }
        }
    }

    let _ = sink.close().await;
    info!("ffxi-viewer-wasm: socket closed");
}
