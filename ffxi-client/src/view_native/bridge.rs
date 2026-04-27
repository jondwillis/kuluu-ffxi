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

    pub last_rebuild_us: u64,
    pub last_entity_count: usize,
    pub rebuilds_total: u64,
}

impl NativeSource {
    pub fn new(
        state_rx: watch::Receiver<SessionState>,
        event_rx: broadcast::Receiver<AgentEvent>,
    ) -> Self {
        Self {
            state_rx,
            event_rx,
            last_rebuild_us: 0,
            last_entity_count: 0,
            rebuilds_total: 0,
        }
    }
}

impl SceneSource for NativeSource {
    fn poll_snapshot(&mut self) -> Option<Box<wire::SceneSnapshot>> {
        if self.state_rx.has_changed().unwrap_or(false) {
            let guard = self.state_rx.borrow_and_update();
            let started = std::time::Instant::now();
            let snap = state_to_snapshot(&guard);
            drop(guard);
            self.last_rebuild_us = started.elapsed().as_micros() as u64;
            self.last_entity_count = snap.entities.len();
            self.rebuilds_total = self.rebuilds_total.wrapping_add(1);
            Some(Box::new(snap))
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
