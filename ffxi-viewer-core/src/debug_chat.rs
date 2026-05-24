//! In-game debug chat surfacing for engine + protocol events.
//!
//! Bridges three signal sources to the System / Debug chat panes:
//!   * [`report_engagement_events_system`] walks `EventLog.recent`
//!     for the rare-but-load-bearing reactor signals (zone change,
//!     aggro, low HP) and pushes a System chat line per event.
//!   * [`report_speed_state_system`] edge-detects the local PC's
//!     server-set speed crossing zero — Bind / Stun / Sleep —
//!     and surfaces both the suppression and the recovery.
//!
//! Both drains keep their own cursors / latches so they coexist with
//! the SFX drains in `audio.rs` (each EventLog consumer must track its
//! own position; the log is a shared `VecDeque<ViewerEvent>` and
//! pop_front shifts indices).

use bevy::prelude::*;
use ffxi_viewer_wire::ViewerEvent;

use crate::snapshot::{EventLog, SceneState};

/// Walks `EventLog.recent` since `pos` and emits one System chat line
/// per `ZoneChanged` / `EngagedBy` / `LowHp` event. Same shape as
/// `audio::SystemSfxCursor` — each consumer of `EventLog.recent` keeps
/// its own cursor because the log is a shared `VecDeque` that
/// `pop_front`s on overflow.
#[derive(Resource, Default)]
pub struct EngagementChatCursor {
    pos: usize,
}

pub fn report_engagement_events_system(
    events: Res<EventLog>,
    mut cursor: ResMut<EngagementChatCursor>,
    scene_state: Res<SceneState>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
) {
    let len = events.recent.len();
    // VecDeque pop_front shifts indices — a cursor that survived a
    // drain would now point past the live tail. Clamp + restart from
    // zero; we trade exact replay for "process the tail since last
    // call," which is the same trade `drain_music_events_system` makes.
    if cursor.pos > len {
        cursor.pos = 0;
    }
    for i in cursor.pos..len {
        let line = match &events.recent[i] {
            ViewerEvent::ZoneChanged { from, to } => Some(match from {
                Some(prev) => format!("→ Zone change: 0x{:04X} → 0x{:04X}", prev, to),
                None => format!("→ Zone entered: 0x{:04X}", to),
            }),
            ViewerEvent::EngagedBy { entity_id } => {
                // Look up the mob's name from the live entity table so
                // the toast reads "Engaged by Goblin" rather than a
                // bare hex id. Falls back to the id when the spawn
                // packet hasn't carried a name yet.
                let name = scene_state
                    .snapshot
                    .entities
                    .iter()
                    .find(|e| e.id == *entity_id)
                    .and_then(|e| e.name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| format!("0x{:08X}", entity_id));
                Some(format!("⚔ Engaged by {} (0x{:08X})", name, entity_id))
            }
            ViewerEvent::LowHp { pct } => {
                Some(format!("❤ Low HP: self at {}%", pct))
            }
            _ => None,
        };
        if let Some(text) = line {
            toasts.write(crate::snapshot::ToastEvent::system(text));
        }
    }
    cursor.pos = len;
}

/// Latched on the local PC's `speed == 0` state. `None` before the
/// first PosHead arrives so the first observation seeds without firing
/// — otherwise a fresh login would always announce "Speed restored"
/// (None → mobile) before the user has even moved.
#[derive(Resource, Default)]
pub struct SpeedSuppressionLatch {
    prev_suppressed: Option<bool>,
}

pub fn report_speed_state_system(
    mut latch: ResMut<SpeedSuppressionLatch>,
    scene_state: Res<SceneState>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
) {
    // Read self entity's current speed. If we don't have a self id or
    // self entity yet, leave the latch unset — the first frame after
    // the PosHead arrives will seed it.
    let snap = &scene_state.snapshot;
    let Some(self_id) = snap.self_char_id else {
        return;
    };
    let Some(self_entity) = snap.entities.iter().find(|e| e.id == self_id) else {
        return;
    };
    let speed = self_entity.speed;
    let speed_base = self_entity.speed_base;
    let suppressed_now = speed == 0;

    let line = match latch.prev_suppressed {
        None => None,
        Some(prev) if prev == suppressed_now => None,
        Some(_prev) => Some(if suppressed_now {
            "✋ Speed suppressed (Bind/Stun/Sleep?)".to_string()
        } else {
            format!("✋ Speed restored ({}/{})", speed, speed_base)
        }),
    };
    latch.prev_suppressed = Some(suppressed_now);

    if let Some(text) = line {
        // System pane — the player wants to see why their character
        // stopped moving even when /devhud is off.
        toasts.write(crate::snapshot::ToastEvent::system(text));
    }
}

pub struct DebugChatPlugin;

impl Plugin for DebugChatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EngagementChatCursor>()
            .init_resource::<SpeedSuppressionLatch>()
            .add_systems(
                Update,
                (report_engagement_events_system, report_speed_state_system),
            );
    }
}
