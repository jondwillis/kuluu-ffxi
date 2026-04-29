//! Bridge between the in-process FFXI session and the viewer-core.
//!
//! `NativeSource` impl `SceneSource` by polling the session's
//! `watch::Receiver<SessionState>` (sync API, no runtime needed) and
//! `broadcast::Receiver<AgentEvent>` (try_recv loop). Translation from
//! `state.rs` types to wire types lives in [`crate::wire_translate`] so
//! the relay (`relay.rs`) and the in-process viewer share the same
//! mappings — there's exactly one definition of "what does the wire
//! look like" in this crate.

use bevy::prelude::Resource;
use ffxi_viewer_core::SceneSource;
use ffxi_viewer_wire as wire;
use tokio::sync::{broadcast, watch};

use crate::state::{AgentEvent, SessionState};
use crate::wire_translate::{event_to_viewer_event, state_to_snapshot};

#[derive(Resource)]
pub struct NativeSource {
    state_rx: watch::Receiver<SessionState>,
    event_rx: broadcast::Receiver<AgentEvent>,
}

impl NativeSource {
    pub fn new(
        state_rx: watch::Receiver<SessionState>,
        event_rx: broadcast::Receiver<AgentEvent>,
    ) -> Self {
        Self { state_rx, event_rx }
    }
}

impl SceneSource for NativeSource {
    fn poll_snapshot(&mut self) -> Option<Box<wire::SceneSnapshot>> {
        if self.state_rx.has_changed().unwrap_or(false) {
            let guard = self.state_rx.borrow_and_update();
            Some(Box::new(state_to_snapshot(&guard)))
        } else {
            None
        }
    }

    fn drain_deltas(&mut self) -> Vec<wire::SceneDelta> {
        Vec::new()
    }

    fn drain_events(&mut self) -> Vec<wire::ViewerEvent> {
        let mut out = Vec::new();
        loop {
            match self.event_rx.try_recv() {
                Ok(ev) => {
                    if let Some(translated) = event_to_viewer_event(ev) {
                        out.push(translated);
                    }
                }
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Closed) => break,
            }
        }
        out
    }
}
