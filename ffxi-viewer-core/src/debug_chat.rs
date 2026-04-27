use bevy::prelude::*;
use ffxi_viewer_wire::ViewerEvent;

use crate::snapshot::{EventLog, SceneState};

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
            ViewerEvent::LowHp { pct } => Some(format!("❤ Low HP: self at {}%", pct)),
            _ => None,
        };
        if let Some(text) = line {
            toasts.write(crate::snapshot::ToastEvent::system(text));
        }
    }
    cursor.pos = len;
}

#[derive(Resource, Default)]
pub struct SpeedSuppressionLatch {
    prev_suppressed: Option<bool>,
}

pub fn report_speed_state_system(
    mut latch: ResMut<SpeedSuppressionLatch>,
    scene_state: Res<SceneState>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
) {
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
