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
///
/// v2 added agent-observability fields to [`SceneSnapshot`]: `current_goal`,
/// `last_reconnect`, `recent_decisions`, `producer_monotonic_ms`. Postcard
/// is positional, so a v1 viewer cannot deserialize a v2 snapshot — the
/// `Hello { protocol_version }` mismatch refusal already gates this.
pub const PROTOCOL_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Position {
    pub pos: Vec3,
    /// 0..=255 mapping to 0°..360°. Mirrors `state::Position::heading`.
    pub heading: u8,
    /// Current effective movement speed (server-set). FFXI PC base = 25 →
    /// 5 yalms/sec; modifiers scale this. Mirrors `state::Position::speed`.
    pub speed: u8,
    /// Unmodified base speed.
    pub speed_base: u8,
}

impl Default for Position {
    fn default() -> Self {
        Self {
            pos: Vec3::default(),
            heading: 0,
            speed: 25,
            speed_base: 25,
        }
    }
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

/// Reactor goal mirror. Wire-side projection of
/// `state::ReactorGoalSnapshot`. Variant set is identical; we re-declare
/// here so the wire crate stays free of the producer-side state types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ReactorGoal {
    Idle,
    Following { target_id: u32, distance: f32 },
    Engaged { target_id: u32, attack_issued: bool },
    Pathing {
        x: f32,
        y: f32,
        z: f32,
        waypoints_remaining: u32,
    },
    Banking { threshold: u8, mog_house_zoneline: u32 },
}

/// Last supervisor reconnect. `at_unix_ms` is wall-clock, since it crosses
/// process boundaries (across the relay) and pairing two `Instant`s
/// across processes is meaningless.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconnectInfo {
    pub downtime_ms: u64,
    pub at_unix_ms: u64,
}

/// One LLM-decision data point — a notification we fired toward the
/// harness, or a tool the harness dispatched. Pairing the two surfaces
/// the round-trip "thinking time" the operator sees.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmDecision {
    pub kind: LlmDecisionKind,
    /// Microseconds for the in-process side of the call (notification
    /// send or tool dispatch). Round-trip across paired entries is
    /// computed at render time.
    pub latency_us: u64,
    /// Producer-process monotonic ms since process start. Only meaningful
    /// against `SceneSnapshot::producer_monotonic_ms` from the same
    /// process — viewers compute pulse-decay age as
    /// `producer_now - decision.at_monotonic_ms`.
    pub at_monotonic_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LlmDecisionKind {
    NotificationFired { uri: String },
    ToolDispatched { tool: String },
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
    /// Active reactor goal at snapshot time. `None` when the supervisor
    /// hasn't emitted a `ReactorGoalChanged` yet (fresh process).
    pub current_goal: Option<ReactorGoal>,
    /// Most recent supervisor reconnect, if any.
    pub last_reconnect: Option<ReconnectInfo>,
    /// LLM-decision log capped at the producer side
    /// (`state::RECENT_DECISIONS_CAP = 64`). Oldest-first.
    pub recent_decisions: Vec<LlmDecision>,
    /// Producer-process monotonic ms at the moment this snapshot was
    /// stamped. Paired with `LlmDecision::at_monotonic_ms` for pulse-
    /// decay age. Defaults to 0 when the producer hasn't stamped (e.g.
    /// in unit tests).
    pub producer_monotonic_ms: u64,
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

/// Viewer→server commands. Mirrors the operator-relevant subset of
/// `state::AgentCommand`. The action surface (Cast/Weaponskill/JobAbility/
/// UseItem) is flattened into named-field variants rather than nesting a
/// full `ActionKind` enum — viewers don't need the 25+ niche actions
/// (Fish, ChocoboDig, Sprint, …); just the tactical four. New variants
/// are additive — appending preserves compatibility with v2 clients that
/// only know the original 10 variants.
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
    /// 0x01A action 0x03 — magic. `pos_*` are the ground-target position
    /// for AoE-target spells (Tractor, certain BLU); zero for self/single-
    /// target casts.
    Cast {
        spell_id: u32,
        target_id: u32,
        target_index: u16,
        pos_x: f32,
        pos_y: f32,
        pos_z: f32,
    },
    /// 0x01A action 0x07 — weaponskill. Server validates TP / weapon.
    Weaponskill {
        skill_id: u32,
        target_id: u32,
        target_index: u16,
    },
    /// 0x01A action 0x09 — job ability. Server validates cooldown / job.
    JobAbility {
        ability_id: u32,
        target_id: u32,
        target_index: u16,
    },
    /// 0x037 — use a consumable / scroll / charged item. `(container,
    /// slot)` is the server-resolvable pair; `item_no` is the LLM's
    /// bookkeeping hint and goes on the wire as 0 (Phoenix's
    /// `0x037_item_use.cpp::validate` enforces `mustEqual(ItemNum, 0)`).
    UseItem {
        container: u8,
        slot: u8,
        item_no: u32,
        target_id: u32,
        target_index: u16,
    },
    /// Reactor goal: monitor inventory, zone to mog house when any
    /// non-mog container hits `threshold` slots filled. Survives
    /// reconnects via `goal_store`.
    BankWhenFull {
        threshold: u8,
        mog_house_zoneline: u32,
    },
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
                speed: 25,
                speed_base: 25,
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
            current_goal: Some(ReactorGoal::Engaged {
                target_id: 0x99,
                attack_issued: true,
            }),
            last_reconnect: Some(ReconnectInfo {
                downtime_ms: 1234,
                at_unix_ms: 1_700_000_001_000,
            }),
            recent_decisions: vec![
                LlmDecision {
                    kind: LlmDecisionKind::NotificationFired {
                        uri: "scene://current".into(),
                    },
                    latency_us: 412,
                    at_monotonic_ms: 1000,
                },
                LlmDecision {
                    kind: LlmDecisionKind::ToolDispatched { tool: "engage".into() },
                    latency_us: 25_000,
                    at_monotonic_ms: 1100,
                },
            ],
            producer_monotonic_ms: 1_500,
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
                // v2 fields survive roundtrip.
                match s.current_goal {
                    Some(ReactorGoal::Engaged { target_id, attack_issued }) => {
                        assert_eq!(target_id, 0x99);
                        assert!(attack_issued);
                    }
                    other => panic!("goal: {other:?}"),
                }
                let rc = s.last_reconnect.expect("last_reconnect");
                assert_eq!(rc.downtime_ms, 1234);
                assert_eq!(s.recent_decisions.len(), 2);
                match &s.recent_decisions[1].kind {
                    LlmDecisionKind::ToolDispatched { tool } => assert_eq!(tool, "engage"),
                    other => panic!("decision: {other:?}"),
                }
                assert_eq!(s.producer_monotonic_ms, 1_500);
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
    fn viewer_command_action_surface_postcard_roundtrip() {
        let cmds = vec![
            ViewerCommand::Cast {
                spell_id: 0x101,
                target_id: 0xCAFE,
                target_index: 7,
                pos_x: 1.5,
                pos_y: 0.0,
                pos_z: -2.5,
            },
            ViewerCommand::Weaponskill {
                skill_id: 0xBEEF,
                target_id: 0xCAFE,
                target_index: 7,
            },
            ViewerCommand::JobAbility {
                ability_id: 0xABCD,
                target_id: 0,
                target_index: 0,
            },
            ViewerCommand::UseItem {
                container: 0,
                slot: 4,
                item_no: 4112,
                target_id: 0,
                target_index: 0,
            },
            ViewerCommand::BankWhenFull {
                threshold: 60,
                mog_house_zoneline: 0xDEAD_BEEF,
            },
        ];
        for c in cmds {
            let bytes = postcard::to_allocvec(&c).expect("encode");
            let back: ViewerCommand = postcard::from_bytes(&bytes).expect("decode");
            // Round-trip equality via Debug — the variants don't impl Eq
            // because of f32 fields; a debug-string compare is sufficient
            // and avoids hand-matching every variant.
            assert_eq!(format!("{c:?}"), format!("{back:?}"));
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
            Frame::Hello { protocol_version } => {
                assert_eq!(protocol_version, PROTOCOL_VERSION)
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
