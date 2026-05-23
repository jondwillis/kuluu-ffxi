//! Server-anchored Vana'diel clock.
//!
//! Single source of truth for Vana time across the viewer. Seeded by
//! the `GameTime` field of LSB's 0x00A LOGIN packet (surfaced as
//! [`ffxi_viewer_wire::ViewerEvent::VanaTimeSynced`]) and extrapolated
//! between zone-ins with a monotonic [`std::time::Instant`] so it
//! survives wall-clock adjustments. Falls back to system time before
//! the first sync so headless tests + early-startup frames still get a
//! coherent (but un-synced) reading.
//!
//! All Vana-time consumers — [`crate::sun_moon::sun_moon_system`],
//! [`crate::hud::vana_clock::update_vana_clock`],
//! [`crate::weather::apply_zone_weather`] — read through
//! [`current_earth_unix`] so they stay in lock-step with the server.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use ffxi_viewer_wire::ViewerEvent;

use crate::hud::vana_clock::EARTH_EPOCH_UNIX;

/// Server time anchor. `None` fields = no sync yet; consumers fall
/// back to system time.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct VanaClock {
    /// Server's Earth-Unix timestamp at the moment we received the
    /// last `VanaTimeSynced`. `EARTH_EPOCH_UNIX + game_time`.
    anchor_earth_unix: Option<u64>,
    /// Local monotonic instant captured when the anchor was set. Used
    /// to extrapolate forward without trusting wall-clock changes.
    anchor_instant: Option<Instant>,
}

impl VanaClock {
    /// True after at least one `VanaTimeSynced` event has been folded in.
    pub fn is_synced(&self) -> bool {
        self.anchor_earth_unix.is_some()
    }

    /// Current Earth-Unix time as a continuous f64. Uses the
    /// server-anchored value when available, else system time.
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

    /// Integer-second form for the HUD label.
    pub fn earth_unix_secs_now(&self) -> u64 {
        self.earth_unix_now() as u64
    }

    fn anchor(&mut self, game_time: u32) {
        self.anchor_earth_unix = Some(EARTH_EPOCH_UNIX + game_time as u64);
        self.anchor_instant = Some(Instant::now());
    }
}

/// PreUpdate system: consume `VanaTimeSynced` events from the event
/// log and re-anchor [`VanaClock`]. Idempotent — re-anchoring on every
/// zone-in just resets the drift accumulated since the last sync.
pub fn ingest_vana_time(
    events: Res<crate::snapshot::EventLog>,
    mut clock: ResMut<VanaClock>,
    mut last_seen_len: Local<usize>,
) {
    // EventLog is a ring buffer the snapshot ingest appends to. We
    // walk the tail since our last visit so we don't re-apply old
    // events when the ring wraps.
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
        // Should be roughly "now" — at least past the Vana epoch.
        assert!(clock.earth_unix_now() > EARTH_EPOCH_UNIX as f64);
    }

    #[test]
    fn synced_clock_uses_server_anchor() {
        let mut clock = VanaClock::default();
        clock.anchor(12345); // Vana day ~3.5
        assert!(clock.is_synced());
        let expected = (EARTH_EPOCH_UNIX + 12345) as f64;
        // Within a few ms of the anchor — extrapolation is monotonic.
        let now = clock.earth_unix_now();
        assert!(now >= expected);
        assert!(now < expected + 1.0);
    }
}
