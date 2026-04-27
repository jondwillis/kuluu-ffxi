use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use ffxi_viewer_wire::ViewerEvent;

use crate::hud::vana_clock::EARTH_EPOCH_UNIX;

#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct VanaClock {
    anchor_earth_unix: Option<u64>,

    anchor_instant: Option<Instant>,
}

impl VanaClock {
    pub fn is_synced(&self) -> bool {
        self.anchor_earth_unix.is_some()
    }

    pub fn earth_unix_now(&self) -> f64 {
        if let (Some(anchor), Some(instant)) = (self.anchor_earth_unix, self.anchor_instant) {
            anchor as f64 + instant.elapsed().as_secs_f64()
        } else {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(EARTH_EPOCH_UNIX as f64)
        }
    }

    pub fn earth_unix_secs_now(&self) -> u64 {
        self.earth_unix_now() as u64
    }

    fn anchor(&mut self, game_time: u32) {
        self.anchor_earth_unix = Some(EARTH_EPOCH_UNIX + game_time as u64);
        self.anchor_instant = Some(Instant::now());
    }
}

pub fn ingest_vana_time(
    events: Res<crate::snapshot::EventLog>,
    mut clock: ResMut<VanaClock>,
    mut last_seen_len: Local<usize>,
) {
    let len = events.recent.len();
    let start = (*last_seen_len).min(len);
    for ev in events.recent.iter().skip(start) {
        if let ViewerEvent::VanaTimeSynced { game_time } = ev {
            clock.anchor(*game_time);
        }
    }
    *last_seen_len = len;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsynced_clock_falls_back_to_system_time() {
        let clock = VanaClock::default();
        assert!(!clock.is_synced());

        assert!(clock.earth_unix_now() > EARTH_EPOCH_UNIX as f64);
    }

    #[test]
    fn synced_clock_uses_server_anchor() {
        let mut clock = VanaClock::default();
        clock.anchor(12345);
        assert!(clock.is_synced());
        let expected = (EARTH_EPOCH_UNIX + 12345) as f64;

        let now = clock.earth_unix_now();
        assert!(now >= expected);
        assert!(now < expected + 1.0);
    }
}
