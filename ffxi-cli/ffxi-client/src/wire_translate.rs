//! Translation helpers between `ffxi_client::state` and `ffxi_viewer_wire`.
//!
//! Both the in-process native bridge (`view_native::bridge`) and the
//! WebSocket relay (`relay`) need to convert `SessionState` into
//! `wire::SceneSnapshot` and `AgentEvent` into `wire::ViewerEvent`. Keeping
//! the translations here means the wire shape has exactly one mapping —
//! which is what makes the wire schema the authoritative boundary it
//! claims to be in the doc comments.
//!
//! Pure functions; no async, no IO, no Bevy. Cheap to call from any
//! context. Both the Bevy-side bridge and the tokio-side relay use these
//! identical translators, so a snapshot generated for the native viewer
//! is bit-identical to one sent over the websocket.

use ffxi_viewer_wire as wire;

use crate::state::{
    process_monotonic_ms, AgentEvent, BlowfishStatus, ChatChannel, ChatLine, Diagnostics, Entity,
    EntityKind, LlmDecision, LlmDecisionKind, PartyMember, Position, ReactorGoalSnapshot,
    ReconnectInfo, SessionState, Stage, Vec3,
};

/// Snapshot the full `SessionState` into a wire `SceneSnapshot`.
///
/// Takes a reference because both call sites (the native bridge after a
/// `borrow_and_update`, the relay after a `borrow`) hold a guard on the
/// watch channel and cloning into the wire struct is cheaper than
/// cloning the entire state and then translating.
pub fn state_to_snapshot(s: &SessionState) -> wire::SceneSnapshot {
    wire::SceneSnapshot {
        stage: stage_to_wire(s.stage),
        char_name: s.character.clone(),
        zone_id: s.zone_id,
        self_pos: position_to_wire(s.self_pos),
        entities: s.entities.iter().map(entity_to_wire).collect(),
        party: s.party.iter().map(party_to_wire).collect(),
        chat: s.chat.iter().map(chat_to_wire).collect(),
        diagnostics: diagnostics_to_wire(&s.diagnostics),
        current_goal: s.current_goal.as_ref().map(goal_to_wire),
        last_reconnect: s.last_reconnect.as_ref().map(reconnect_to_wire),
        recent_decisions: s.recent_decisions.iter().map(decision_to_wire).collect(),
        // Stamp at translation time, not at SessionState fold time. Pulse
        // decay needs `producer_now` to be the time the snapshot was
        // *emitted*, so the viewer can compute `producer_now -
        // decision.at_monotonic_ms` and get a useful "age".
        producer_monotonic_ms: process_monotonic_ms(),
    }
}

/// Translate an `AgentEvent` into a `wire::ViewerEvent`. Returns `None`
/// for events that are folded into snapshot state (no need to surface
/// them as standalone events) or that are internal-only signals not
/// useful to a renderer.
pub fn event_to_viewer_event(ev: AgentEvent) -> Option<wire::ViewerEvent> {
    match ev {
        AgentEvent::ZoneChanged { from, to } => Some(wire::ViewerEvent::ZoneChanged { from, to }),
        AgentEvent::EntityRemoved { id } => Some(wire::ViewerEvent::EntityRemoved { id }),
        AgentEvent::Disconnected { reason } => Some(wire::ViewerEvent::Disconnected { reason }),
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

pub fn stage_to_wire(s: Stage) -> wire::Stage {
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

pub fn position_to_wire(p: Position) -> wire::Position {
    wire::Position {
        pos: vec3_to_wire(p.pos),
        heading: p.heading,
        speed: p.speed,
        speed_base: p.speed_base,
    }
}

pub fn vec3_to_wire(v: Vec3) -> wire::Vec3 {
    wire::Vec3 {
        x: v.x,
        y: v.y,
        z: v.z,
    }
}

pub fn entity_to_wire(e: &Entity) -> wire::Entity {
    wire::Entity {
        id: e.id,
        act_index: e.act_index,
        kind: kind_to_wire(e.kind),
        name: e.name.clone(),
        pos: vec3_to_wire(e.pos),
        heading: e.heading,
        hp_pct: e.hp_pct,
        bt_target_id: e.bt_target_id,
    }
}

pub fn kind_to_wire(k: EntityKind) -> wire::EntityKind {
    match k {
        EntityKind::Pc => wire::EntityKind::Pc,
        EntityKind::Npc => wire::EntityKind::Npc,
        EntityKind::Mob => wire::EntityKind::Mob,
        EntityKind::Pet => wire::EntityKind::Pet,
        EntityKind::Other => wire::EntityKind::Other,
    }
}

pub fn chat_to_wire(c: &ChatLine) -> wire::ChatLine {
    wire::ChatLine {
        channel: channel_to_wire(c.channel),
        sender: c.sender.clone(),
        text: c.text.clone(),
        server_ts: c.server_ts,
    }
}

pub fn channel_to_wire(c: ChatChannel) -> wire::ChatChannel {
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

pub fn party_to_wire(m: &PartyMember) -> wire::PartyMember {
    wire::PartyMember {
        id: m.id,
        act_index: m.act_index,
        name: m.name.clone(),
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

pub fn diagnostics_to_wire(d: &Diagnostics) -> wire::Diagnostics {
    wire::Diagnostics {
        stage: d.stage.map(stage_to_wire),
        blowfish_status: d.blowfish_status.map(blowfish_to_wire),
        sync_in: d.sync_in,
        sync_out: d.sync_out,
        last_server_packet_age_ms: d.last_server_packet_age_ms,
        map_server_addr: d.map_server_addr.clone(),
    }
}

pub fn blowfish_to_wire(b: BlowfishStatus) -> wire::BlowfishStatus {
    match b {
        BlowfishStatus::Waiting => wire::BlowfishStatus::Waiting,
        BlowfishStatus::Sent => wire::BlowfishStatus::Sent,
        BlowfishStatus::Accepted => wire::BlowfishStatus::Accepted,
        BlowfishStatus::PendingZone => wire::BlowfishStatus::PendingZone,
    }
}

pub fn goal_to_wire(g: &ReactorGoalSnapshot) -> wire::ReactorGoal {
    match *g {
        ReactorGoalSnapshot::Idle => wire::ReactorGoal::Idle,
        ReactorGoalSnapshot::Following { target_id, distance } => {
            wire::ReactorGoal::Following { target_id, distance }
        }
        ReactorGoalSnapshot::Engaged { target_id, attack_issued } => {
            wire::ReactorGoal::Engaged { target_id, attack_issued }
        }
        ReactorGoalSnapshot::Pathing { x, y, z, waypoints_remaining } => {
            wire::ReactorGoal::Pathing { x, y, z, waypoints_remaining }
        }
        ReactorGoalSnapshot::Banking { threshold, mog_house_zoneline } => {
            wire::ReactorGoal::Banking { threshold, mog_house_zoneline }
        }
    }
}

pub fn reconnect_to_wire(r: &ReconnectInfo) -> wire::ReconnectInfo {
    wire::ReconnectInfo {
        downtime_ms: r.downtime_ms,
        at_unix_ms: r.at_unix_ms,
    }
}

pub fn decision_to_wire(d: &LlmDecision) -> wire::LlmDecision {
    wire::LlmDecision {
        kind: decision_kind_to_wire(&d.kind),
        latency_us: d.latency_us,
        at_monotonic_ms: d.at_monotonic_ms,
    }
}

pub fn decision_kind_to_wire(k: &LlmDecisionKind) -> wire::LlmDecisionKind {
    match k {
        LlmDecisionKind::NotificationFired { uri } => {
            wire::LlmDecisionKind::NotificationFired { uri: uri.clone() }
        }
        LlmDecisionKind::ToolDispatched { tool } => {
            wire::LlmDecisionKind::ToolDispatched { tool: tool.clone() }
        }
    }
}
