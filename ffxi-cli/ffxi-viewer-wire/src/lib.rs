//! Wire schema for the operator-viewer relay protocol.
//!
//! This crate is the **single source of truth** for what crosses the boundary
//! between `ffxi-client` (the FFXI session process) and the viewers (native
//! Bevy window via in-process bridge, browser via WebSocket). It is
//! deliberately *smaller* than `ffxi_client::state::SessionState`: a viewer
//! renders entities, chat, party, diagnostics — it has no business seeing
//! inventory rows, LLM decision telemetry, or reactor goal internals.
//!
//! Smaller schema = more stable schema. Adding a new internal `AgentEvent`
//! variant to `ffxi-client/src/state.rs` does not break this wire.
//!
//! # Encoding
//!
//! Default: postcard binary (compact, fast). The relay also supports
//! serde-JSON for human inspection via the `?format=json` query param.
//!
//! # Versioning
//!
//! Bump [`PROTOCOL_VERSION`] on incompatible schema changes. The relay sends
//! `Frame::Hello { protocol_version }` first; viewers refuse to connect on
//! mismatch. Additive changes (new variants on `ViewerEvent`, new fields on
//! struct payloads) do not require a version bump as long as old viewers
//! degrade gracefully.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// Wire protocol version. Bump on incompatible schema changes.
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Position {
    pub pos: Vec3,
    /// 0..=255 mapping to 0°..360°. Mirrors `state::Position::heading`.
    pub heading: u8,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Idle,
    Authenticating,
    LobbyHandshake,
    MapBootstrap,
    Zoning,
    InZone,
    Disconnected,
}

impl Default for Stage {
    fn default() -> Self {
        Stage::Idle
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlowfishStatus {
    Waiting,
    Sent,
    Accepted,
    PendingZone,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Pc,
    Npc,
    Mob,
    Pet,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: u32,
    pub act_index: u16,
    pub kind: EntityKind,
    pub name: Option<String>,
    pub pos: Vec3,
    pub heading: u8,
    pub hp_pct: Option<u8>,
    pub bt_target_id: u32,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatChannel {
    Say,
    Shout,
    Tell,
    Party,
    Linkshell,
    Yell,
    System,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatLine {
    pub channel: ChatChannel,
    pub sender: String,
    pub text: String,
    pub server_ts: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartyMember {
    pub id: u32,
    pub act_index: u16,
    pub name: Option<String>,
    pub hp: u32,
    pub mp: u32,
    pub tp: u32,
    pub hp_pct: u8,
    pub mp_pct: u8,
    pub zone_no: u16,
    pub main_job: u8,
    pub main_job_lv: u8,
    pub sub_job: u8,
    pub sub_job_lv: u8,
    pub is_party_leader: bool,
    pub is_alliance_leader: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Diagnostics {
    pub stage: Option<Stage>,
    pub blowfish_status: Option<BlowfishStatus>,
    pub sync_in: Option<u16>,
    pub sync_out: Option<u16>,
    pub last_server_packet_age_ms: Option<u64>,
    pub map_server_addr: Option<String>,
}

/// Full state at a point in time. Sent on connect, and (Stage 2.0) on every
/// `state_rx.changed()` tick. Stage 2.1 may switch to delta-only with
/// periodic snapshot resync.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneSnapshot {
    pub stage: Stage,
    pub char_name: Option<String>,
    pub zone_id: Option<u16>,
    pub self_pos: Position,
    pub entities: Vec<Entity>,
    pub party: Vec<PartyMember>,
    /// Recent chat, ordered oldest-first. Capped at the producer side to
    /// match `state::CHAT_HISTORY_CAP`.
    pub chat: Vec<ChatLine>,
    pub diagnostics: Diagnostics,
}

/// Minimal patch between snapshots. Reserved for Stage 2.1; the Stage 2.0
/// relay sends `Frame::Snapshot` on every change for simplicity.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneDelta {
    pub stage: Option<Stage>,
    pub zone_id: Option<u16>,
    pub self_pos: Option<Position>,
    pub entities_upserted: Vec<Entity>,
    pub entities_removed: Vec<u32>,
    pub party_upserted: Vec<PartyMember>,
    pub chat_appended: Vec<ChatLine>,
    pub diagnostics: Option<Diagnostics>,
}

/// Subset of `state::AgentEvent` relevant to a renderer. Excludes:
/// - `Connected` / `Diagnostics` — already in `SceneSnapshot`
/// - `StageChanged` / `PositionChanged` / `EntityUpserted` / `ChatLine` /
///   `PartyMemberUpdated` — folded into snapshot/delta
/// - `Error` — surfaces via the system chat channel already
/// - `KeyRotated` / `EventStart` / `EventEnded` / `InventoryUpdated` /
///   `InventoryReady` / `ReactorGoalChanged` / `LlmDecision` /
///   `SceneSummary` / `PartyMemberLowHp` — internal signal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ViewerEvent {
    ZoneChanged { from: Option<u16>, to: u16 },
    EntityRemoved { id: u32 },
    Disconnected { reason: String },
    LowHp { pct: u8 },
    EngagedBy { entity_id: u32 },
    TellReceived { from: String, text: String },
    Reconnected { downtime_ms: u64 },
}

/// Server→viewer frame on the WebSocket. `Snapshot` and `Delta` are boxed
/// so the enum stays a single pointer wide regardless of payload size.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Frame {
    Hello { protocol_version: u32 },
    Snapshot(Box<SceneSnapshot>),
    Delta(Box<SceneDelta>),
    Event(ViewerEvent),
}

/// Viewer→server commands. Subset of `state::AgentCommand` excluding
/// commands that need richer payloads (`Action` carries `ActionKind`,
/// `UseItem` and `BankWhenFull` are tactical reactor goals not yet in the
/// viewer's vocabulary). Adding them later is an additive schema change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ViewerCommand {
    Move { x: f32, y: f32, z: f32, heading: u8 },
    StopMove,
    EndEvent,
    Snapshot,
    Chat { kind: u8, text: String },
    Tell { to: String, text: String },
    Follow { target_id: u32, distance: f32 },
    Engage { target_id: u32 },
    PathTo { x: f32, y: f32, z: f32 },
    Cancel,
}

/// Viewer→server frame on the WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientFrame {
    Hello { protocol_version: u32 },
    Command(ViewerCommand),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> SceneSnapshot {
        SceneSnapshot {
            stage: Stage::InZone,
            char_name: Some("Sylvie".into()),
            zone_id: Some(230),
            self_pos: Position {
                pos: Vec3 { x: -10.5, y: 0.0, z: 42.25 },
                heading: 64,
            },
            entities: vec![Entity {
                id: 0x1701234,
                act_index: 7,
                kind: EntityKind::Pc,
                name: Some("Other".into()),
                pos: Vec3 { x: 1.0, y: 2.0, z: 3.0 },
                heading: 32,
                hp_pct: Some(80),
                bt_target_id: 0,
            }],
            party: vec![],
            chat: vec![ChatLine {
                channel: ChatChannel::Say,
                sender: "Other".into(),
                text: "hi".into(),
                server_ts: 1_700_000_000,
            }],
            diagnostics: Diagnostics {
                stage: Some(Stage::InZone),
                blowfish_status: Some(BlowfishStatus::Accepted),
                sync_in: Some(42),
                sync_out: Some(43),
                last_server_packet_age_ms: Some(123),
                map_server_addr: Some("127.0.0.1:54230".into()),
            },
        }
    }

    #[test]
    fn frame_snapshot_postcard_roundtrip() {
        let frame = Frame::Snapshot(Box::new(sample_snapshot()));
        let bytes = postcard::to_allocvec(&frame).expect("encode");
        let back: Frame = postcard::from_bytes(&bytes).expect("decode");
        match back {
            Frame::Snapshot(s) => {
                assert_eq!(s.stage, Stage::InZone);
                assert_eq!(s.char_name.as_deref(), Some("Sylvie"));
                assert_eq!(s.entities.len(), 1);
                assert_eq!(s.entities[0].id, 0x1701234);
                assert_eq!(s.chat[0].text, "hi");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn frame_event_postcard_roundtrip() {
        let frame = Frame::Event(ViewerEvent::TellReceived {
            from: "Friend".into(),
            text: "@cure".into(),
        });
        let bytes = postcard::to_allocvec(&frame).expect("encode");
        let back: Frame = postcard::from_bytes(&bytes).expect("decode");
        match back {
            Frame::Event(ViewerEvent::TellReceived { from, text }) => {
                assert_eq!(from, "Friend");
                assert_eq!(text, "@cure");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn client_frame_command_postcard_roundtrip() {
        let cf = ClientFrame::Command(ViewerCommand::Follow {
            target_id: 0x42,
            distance: 3.0,
        });
        let bytes = postcard::to_allocvec(&cf).expect("encode");
        let back: ClientFrame = postcard::from_bytes(&bytes).expect("decode");
        match back {
            ClientFrame::Command(ViewerCommand::Follow { target_id, distance }) => {
                assert_eq!(target_id, 0x42);
                assert!((distance - 3.0).abs() < f32::EPSILON);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn frame_hello_json_debuggable() {
        let f = Frame::Hello {
            protocol_version: PROTOCOL_VERSION,
        };
        let s = serde_json::to_string(&f).unwrap();
        // Externally-tagged enum encoding.
        assert!(s.contains("\"Hello\""), "shape: {s}");
        let back: Frame = serde_json::from_str(&s).unwrap();
        match back {
            Frame::Hello { protocol_version } => assert_eq!(protocol_version, 1),
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
