//! Session state — the single source of truth that both the TUI and the
//! JSON sidechannel subscribe to.

use serde::{Deserialize, Serialize};

/// Stage of the end-to-end login flow we're currently in.
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

/// FFXI Blowfish lifecycle, mirrored from `server/src/common/blowfish.h::BLOWFISH`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlowfishStatus {
    Waiting,
    Sent,
    Accepted,
    PendingZone,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Position {
    pub pos: Vec3,
    /// 0..=255 mapping to 0°..360°, matches `GP_CLI_COMMAND_POS::dir`.
    pub heading: u8,
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
    /// Per-zone short index (the `ActIndex` server-side). Required when
    /// targeting via `0x01A ACTION` — `id` alone is not enough; the server
    /// looks the entity up by `ActIndex` within the current zone.
    pub act_index: u16,
    pub kind: EntityKind,
    pub name: Option<String>,
    pub pos: Vec3,
    pub heading: u8,
    /// HP percentage, 0..=100. None if unknown.
    pub hp_pct: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatLine {
    pub channel: ChatChannel,
    pub sender: String,
    pub text: String,
    /// Vana'diel (server) timestamp in seconds since epoch.
    pub server_ts: u32,
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

/// Per-connection diagnostics surfaced to humans (TUI footer) and agents
/// (JSON sidechannel). These are the *first-class* signals an agent needs to
/// know the session is healthy — surfacing sync_in/sync_out is what catches
/// silent sequence desync (failure mode #1 in the plan).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Diagnostics {
    pub stage: Option<Stage>,
    pub blowfish_status: Option<BlowfishStatus>,
    pub sync_in: Option<u16>,
    pub sync_out: Option<u16>,
    /// Milliseconds since we last received a server bundle.
    pub last_server_packet_age_ms: Option<u64>,
    /// SHA-256 of the auth-server TLS cert, hex-encoded.
    pub cert_sha256: Option<String>,
    pub map_server_addr: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    pub stage: Stage,
    pub account_id: Option<u32>,
    pub char_id: Option<u32>,
    pub character: Option<String>,
    pub zone_id: Option<u16>,
    pub self_pos: Position,
    pub entities: Vec<Entity>,
    pub chat: Vec<ChatLine>,
    pub diagnostics: Diagnostics,
}

/// Cap on retained chat history. The TUI only ever shows the last N visible
/// lines; older entries are dropped to keep allocations bounded under long
/// sessions. 256 is generous for ~10 minutes of social chat.
const CHAT_HISTORY_CAP: usize = 256;

impl SessionState {
    /// Pure fold: apply an `AgentEvent` to derive the new state. Kept free of
    /// I/O so it's trivially testable and so a watch-based renderer never
    /// blocks the event-broadcast task.
    pub fn apply_event(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::Connected {
                account_id,
                char_id,
                character,
                zone_id,
            } => {
                self.account_id = Some(*account_id);
                self.char_id = Some(*char_id);
                self.character = Some(character.clone());
                self.zone_id = Some(*zone_id);
            }
            AgentEvent::StageChanged { stage } => {
                self.stage = *stage;
                self.diagnostics.stage = Some(*stage);
            }
            AgentEvent::ZoneChanged { to, .. } => {
                self.zone_id = Some(*to);
                // Entities from the old zone are stale on zone change; the
                // new zone's flood will repopulate.
                self.entities.clear();
            }
            AgentEvent::PositionChanged { pos } => {
                self.self_pos = *pos;
            }
            AgentEvent::EntityUpserted { entity } => {
                if let Some(existing) = self.entities.iter_mut().find(|e| e.id == entity.id) {
                    *existing = entity.clone();
                } else {
                    self.entities.push(entity.clone());
                }
            }
            AgentEvent::EntityRemoved { id } => {
                self.entities.retain(|e| e.id != *id);
            }
            AgentEvent::ChatLine { line } => {
                self.chat.push(line.clone());
                if self.chat.len() > CHAT_HISTORY_CAP {
                    let drop = self.chat.len() - CHAT_HISTORY_CAP;
                    self.chat.drain(0..drop);
                }
            }
            AgentEvent::Diagnostics { diagnostics } => {
                self.diagnostics = diagnostics.clone();
            }
            AgentEvent::Disconnected { .. } => {
                self.stage = Stage::Disconnected;
                self.diagnostics.stage = Some(Stage::Disconnected);
            }
            // Surface errors as system chat so the user sees them in the TUI
            // without needing a separate pane. The stage doesn't change —
            // many errors are recoverable.
            AgentEvent::Error { message } => {
                self.chat.push(ChatLine {
                    channel: ChatChannel::System,
                    sender: "<error>".into(),
                    text: message.clone(),
                    server_ts: 0,
                });
                if self.chat.len() > CHAT_HISTORY_CAP {
                    let drop = self.chat.len() - CHAT_HISTORY_CAP;
                    self.chat.drain(0..drop);
                }
            }
            // EventStart / EventEnded / KeyRotated are flow signals; the TUI
            // doesn't render them as state today. Left as no-ops — extending
            // is a non-breaking change.
            AgentEvent::EventStart { .. }
            | AgentEvent::EventEnded
            | AgentEvent::KeyRotated { .. } => {}
        }
    }
}

impl Default for Stage {
    fn default() -> Self {
        Stage::Idle
    }
}

/// Events emitted by the Session actor. The JSON sidechannel writes these
/// one-per-line to stdout; the TUI consumes them via a `tokio::sync::broadcast`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Connected {
        account_id: u32,
        char_id: u32,
        character: String,
        zone_id: u16,
    },
    StageChanged {
        stage: Stage,
    },
    ZoneChanged {
        from: Option<u16>,
        to: u16,
    },
    PositionChanged {
        pos: Position,
    },
    EntityUpserted {
        entity: Entity,
    },
    EntityRemoved {
        id: u32,
    },
    ChatLine {
        line: ChatLine,
    },
    EventStart {
        event_id: u32,
    },
    EventEnded,
    KeyRotated {
        previous_status: BlowfishStatus,
    },
    Disconnected {
        reason: String,
    },
    Error {
        message: String,
    },
    Diagnostics {
        diagnostics: Diagnostics,
    },
}

/// Strictly enumerated commands an agent can issue. **Do not** add a generic
/// `SendPacket` escape hatch — Claude Code knows about FFXI from training and
/// will hallucinate opcodes; the lid stays on by design.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum AgentCommand {
    /// Set the next position the keepalive will report.
    Move {
        x: f32,
        y: f32,
        z: f32,
        heading: u8,
    },
    /// Stop sending position updates (revert keepalive to last-known position).
    StopMove,
    /// Request a zone change at a known zoneline. Server still has to honor it.
    RequestZoneChange { line_id: u32 },
    /// Auto-end any in-progress event/cutscene.
    EndEvent,
    /// Disconnect cleanly.
    Disconnect,
    /// Echo back the current SessionState as a JSON event (debugging convenience).
    Snapshot,
    /// Send a chat-channel message. Server-side say messages beginning with
    /// `@` are dispatched as GM commands when the account has gmlevel ≥ 1.
    /// `kind` matches `GP_CLI_COMMAND_CHAT_STD::Kind` (0=say, 1=shout, 4=party, …).
    Chat { kind: u8, text: String },
    /// `GP_CLI_COMMAND_ACTION` — universal "do thing to target" packet.
    /// `target_index` is the server-side `ActIndex` of the entity to target;
    /// `target_id` is its `UniqueNo`. `action_id` matches
    /// `GP_CLI_COMMAND_ACTION_ACTIONID` (0=Talk, 2=Attack, 4=AttackOff, …).
    /// Strict enumeration is the lid: an agent cannot send arbitrary opcodes,
    /// only this curated `Action` with a known `action_id`.
    Action {
        target_id: u32,
        target_index: u16,
        action_id: u16,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_event_roundtrip() {
        let ev = AgentEvent::PositionChanged {
            pos: Position {
                pos: Vec3 { x: 1.0, y: 2.0, z: 3.0 },
                heading: 64,
            },
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::PositionChanged { pos } => {
                assert_eq!(pos.heading, 64);
                assert_eq!(pos.pos.y, 2.0);
            }
            _ => panic!("wrong variant: {back:?}"),
        }
    }

    #[test]
    fn agent_command_roundtrip() {
        let line = r#"{"cmd":"move","x":1.0,"y":2.0,"z":3.0,"heading":42}"#;
        let cmd: AgentCommand = serde_json::from_str(line).unwrap();
        match cmd {
            AgentCommand::Move { x, y, z, heading } => {
                assert_eq!((x, y, z, heading), (1.0, 2.0, 3.0, 42));
            }
            _ => panic!("wrong variant: {cmd:?}"),
        }
    }

    #[test]
    fn apply_event_folds_in_documented_order() {
        let mut s = SessionState::default();
        assert_eq!(s.stage, Stage::Idle);

        s.apply_event(&AgentEvent::StageChanged { stage: Stage::Authenticating });
        assert_eq!(s.stage, Stage::Authenticating);
        assert_eq!(s.diagnostics.stage, Some(Stage::Authenticating));

        s.apply_event(&AgentEvent::Connected {
            account_id: 42,
            char_id: 7,
            character: "Tester".into(),
            zone_id: 100,
        });
        assert_eq!(s.account_id, Some(42));
        assert_eq!(s.char_id, Some(7));
        assert_eq!(s.character.as_deref(), Some("Tester"));
        assert_eq!(s.zone_id, Some(100));

        s.apply_event(&AgentEvent::EntityUpserted {
            entity: Entity {
                id: 999,
                act_index: 1,
                kind: EntityKind::Pc,
                name: Some("Other".into()),
                pos: Vec3 { x: 1.0, y: 0.0, z: 2.0 },
                heading: 64,
                hp_pct: Some(80),
            },
        });
        assert_eq!(s.entities.len(), 1);

        // Re-upsert with same id should update, not duplicate.
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: Entity {
                id: 999,
                act_index: 1,
                kind: EntityKind::Pc,
                name: Some("Other".into()),
                pos: Vec3 { x: 5.0, y: 0.0, z: 6.0 },
                heading: 32,
                hp_pct: Some(50),
            },
        });
        assert_eq!(s.entities.len(), 1, "upsert must not duplicate by id");
        assert_eq!(s.entities[0].pos.x, 5.0, "upsert must overwrite");

        // ZoneChanged clears entities (stale zone state) and updates zone_id.
        s.apply_event(&AgentEvent::ZoneChanged { from: Some(100), to: 230 });
        assert_eq!(s.zone_id, Some(230));
        assert!(s.entities.is_empty(), "zone change must clear stale entities");

        // Disconnected lands the terminal stage.
        s.apply_event(&AgentEvent::Disconnected { reason: "test".into() });
        assert_eq!(s.stage, Stage::Disconnected);
    }

    #[test]
    fn apply_event_caps_chat_history() {
        let mut s = SessionState::default();
        for i in 0..(CHAT_HISTORY_CAP + 50) {
            s.apply_event(&AgentEvent::ChatLine {
                line: ChatLine {
                    channel: ChatChannel::Say,
                    sender: "x".into(),
                    text: format!("msg {i}"),
                    server_ts: 0,
                },
            });
        }
        assert_eq!(s.chat.len(), CHAT_HISTORY_CAP);
        // The oldest 50 should have been dropped.
        assert_eq!(s.chat[0].text, "msg 50");
    }
}
