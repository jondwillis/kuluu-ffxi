//! Bridge between the in-process FFXI session and the viewer-core.
//!
//! `NativeSource` impl `SceneSource` by polling the session's
//! `watch::Receiver<SessionState>` (sync API, no runtime needed) and
//! `broadcast::Receiver<AgentEvent>` (try_recv loop). Translates
//! `state.rs` types to wire types — the wire crate is the boundary
//! between this binary and the viewer-core.

use bevy::prelude::Resource;
use ffxi_viewer_core::SceneSource;
use ffxi_viewer_wire as wire;
use tokio::sync::{broadcast, watch};

use crate::state::{
    self as st, AgentEvent, BlowfishStatus, ChatChannel, ChatLine, Diagnostics, Entity, EntityKind,
    PartyMember, Position, SessionState, Stage, Vec3,
};

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
            let s = self.state_rx.borrow_and_update().clone();
            Some(Box::new(state_to_snapshot(s)))
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

fn state_to_snapshot(s: SessionState) -> wire::SceneSnapshot {
    wire::SceneSnapshot {
        stage: stage_to_wire(s.stage),
        char_name: s.character,
        zone_id: s.zone_id,
        self_pos: position_to_wire(s.self_pos),
        entities: s.entities.into_iter().map(entity_to_wire).collect(),
        party: s.party.into_iter().map(party_to_wire).collect(),
        chat: s.chat.into_iter().map(chat_to_wire).collect(),
        diagnostics: diagnostics_to_wire(s.diagnostics),
    }
}

fn event_to_viewer_event(ev: AgentEvent) -> Option<wire::ViewerEvent> {
    match ev {
        AgentEvent::ZoneChanged { from, to } => Some(wire::ViewerEvent::ZoneChanged { from, to }),
        AgentEvent::EntityRemoved { id } => Some(wire::ViewerEvent::EntityRemoved { id }),
        AgentEvent::Disconnected { reason } => {
            Some(wire::ViewerEvent::Disconnected { reason })
        }
        AgentEvent::LowHp { pct } => Some(wire::ViewerEvent::LowHp { pct }),
        AgentEvent::EngagedBy { entity_id } => Some(wire::ViewerEvent::EngagedBy { entity_id }),
        AgentEvent::TellReceived { from, text } => {
            Some(wire::ViewerEvent::TellReceived { from, text })
        }
        AgentEvent::Reconnected { downtime_ms } => {
            Some(wire::ViewerEvent::Reconnected { downtime_ms })
        }
        // Snapshot-folded signals (Connected, StageChanged, PositionChanged,
        // EntityUpserted, ChatLine, PartyMemberUpdated, Diagnostics) are
        // already visible through the state watch — no need to push them as
        // events. Internal-only signals (KeyRotated, EventStart/Ended,
        // Inventory*, ReactorGoalChanged, LlmDecision, SceneSummary,
        // PartyMemberLowHp, Error) don't drive renderer behavior.
        _ => None,
    }
}

fn stage_to_wire(s: Stage) -> wire::Stage {
    match s {
        Stage::Idle => wire::Stage::Idle,
        Stage::Authenticating => wire::Stage::Authenticating,
        Stage::LobbyHandshake => wire::Stage::LobbyHandshake,
        Stage::MapBootstrap => wire::Stage::MapBootstrap,
        Stage::Zoning => wire::Stage::Zoning,
        Stage::InZone => wire::Stage::InZone,
        Stage::Disconnected => wire::Stage::Disconnected,
    }
}

fn position_to_wire(p: Position) -> wire::Position {
    wire::Position {
        pos: vec3_to_wire(p.pos),
        heading: p.heading,
    }
}

fn vec3_to_wire(v: Vec3) -> wire::Vec3 {
    wire::Vec3 {
        x: v.x,
        y: v.y,
        z: v.z,
    }
}

fn entity_to_wire(e: Entity) -> wire::Entity {
    wire::Entity {
        id: e.id,
        act_index: e.act_index,
        kind: kind_to_wire(e.kind),
        name: e.name,
        pos: vec3_to_wire(e.pos),
        heading: e.heading,
        hp_pct: e.hp_pct,
        bt_target_id: e.bt_target_id,
    }
}

fn kind_to_wire(k: EntityKind) -> wire::EntityKind {
    match k {
        EntityKind::Pc => wire::EntityKind::Pc,
        EntityKind::Npc => wire::EntityKind::Npc,
        EntityKind::Mob => wire::EntityKind::Mob,
        EntityKind::Pet => wire::EntityKind::Pet,
        EntityKind::Other => wire::EntityKind::Other,
    }
}

fn chat_to_wire(c: ChatLine) -> wire::ChatLine {
    wire::ChatLine {
        channel: channel_to_wire(c.channel),
        sender: c.sender,
        text: c.text,
        server_ts: c.server_ts,
    }
}

fn channel_to_wire(c: ChatChannel) -> wire::ChatChannel {
    match c {
        ChatChannel::Say => wire::ChatChannel::Say,
        ChatChannel::Shout => wire::ChatChannel::Shout,
        ChatChannel::Tell => wire::ChatChannel::Tell,
        ChatChannel::Party => wire::ChatChannel::Party,
        ChatChannel::Linkshell => wire::ChatChannel::Linkshell,
        ChatChannel::Yell => wire::ChatChannel::Yell,
        ChatChannel::System => wire::ChatChannel::System,
        ChatChannel::Other => wire::ChatChannel::Other,
    }
}

fn party_to_wire(m: PartyMember) -> wire::PartyMember {
    wire::PartyMember {
        id: m.id,
        act_index: m.act_index,
        name: m.name,
        hp: m.hp,
        mp: m.mp,
        tp: m.tp,
        hp_pct: m.hp_pct,
        mp_pct: m.mp_pct,
        zone_no: m.zone_no,
        main_job: m.main_job,
        main_job_lv: m.main_job_lv,
        sub_job: m.sub_job,
        sub_job_lv: m.sub_job_lv,
        is_party_leader: m.is_party_leader,
        is_alliance_leader: m.is_alliance_leader,
    }
}

fn diagnostics_to_wire(d: Diagnostics) -> wire::Diagnostics {
    wire::Diagnostics {
        stage: d.stage.map(stage_to_wire),
        blowfish_status: d.blowfish_status.map(blowfish_to_wire),
        sync_in: d.sync_in,
        sync_out: d.sync_out,
        last_server_packet_age_ms: d.last_server_packet_age_ms,
        map_server_addr: d.map_server_addr,
    }
}

fn blowfish_to_wire(b: BlowfishStatus) -> wire::BlowfishStatus {
    match b {
        BlowfishStatus::Waiting => wire::BlowfishStatus::Waiting,
        BlowfishStatus::Sent => wire::BlowfishStatus::Sent,
        BlowfishStatus::Accepted => wire::BlowfishStatus::Accepted,
        BlowfishStatus::PendingZone => wire::BlowfishStatus::PendingZone,
    }
}

// `st` import lives at the top so this module can grow translations in one
// place; the unused-import shim keeps clippy/rustc quiet during scaffolding.
#[allow(dead_code)]
fn _unused_state_import() -> Option<&'static str> {
    let _: Option<st::SessionState> = None;
    None
}
