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
    pub party: Vec<PartyMember>,
    pub chat: Vec<ChatLine>,
    pub diagnostics: Diagnostics,
}

/// One row in the agent's view of the party. Populated from
/// `0x0DD GROUP_LIST` (other members; provides name + leader flags) and
/// `0x0DF GROUP_ATTR` (self + Trusts; HP/MP/TP refreshes only). Apply
/// rules: same-id updates merge, with `name`/`is_party_leader` preserved
/// across attr-only refreshes.
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
            AgentEvent::PartyMemberUpdated { member } => {
                if let Some(existing) = self.party.iter_mut().find(|m| m.id == member.id) {
                    // Preserve name / leader flags across 0x0DF attr-only updates
                    // (which don't carry them). The new packet always wins for
                    // HP/MP/TP/job/zone — those it always has.
                    let preserved_name = if member.name.is_some() {
                        member.name.clone()
                    } else {
                        existing.name.clone()
                    };
                    let preserved_leader = if !member.name.is_some() {
                        existing.is_party_leader
                    } else {
                        member.is_party_leader
                    };
                    let preserved_alliance = if !member.name.is_some() {
                        existing.is_alliance_leader
                    } else {
                        member.is_alliance_leader
                    };
                    *existing = PartyMember {
                        name: preserved_name,
                        is_party_leader: preserved_leader,
                        is_alliance_leader: preserved_alliance,
                        ..member.clone()
                    };
                } else {
                    self.party.push(member.clone());
                }
            }
            // High-signal events the LLM wakes for. They don't mutate
            // SessionState — the data is already there (HP via entity
            // updates, chat via ChatLine). They're notifications, not state.
            AgentEvent::LowHp { .. }
            | AgentEvent::PartyMemberLowHp { .. }
            | AgentEvent::EngagedBy { .. }
            | AgentEvent::TellReceived { .. }
            | AgentEvent::Reconnected { .. }
            | AgentEvent::SceneSummary { .. } => {}
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
    /// 0x0DD GROUP_LIST (other) or 0x0DF GROUP_ATTR (self + Trust). The
    /// merge rules in `apply_event` preserve `name` / `is_party_leader`
    /// across attr-only refreshes.
    PartyMemberUpdated {
        member: PartyMember,
    },
    /// Self HP crossed below a low-HP threshold (default 25%, configurable
    /// in the reactor). Wakes the LLM strategically without burning
    /// tokens on every HP tick.
    LowHp {
        pct: u8,
    },
    /// A party member's HP crossed below the low-HP threshold. Drives
    /// healer co-play.
    PartyMemberLowHp {
        id: u32,
        pct: u8,
    },
    /// A mob took the player as its battle target — i.e. aggro. The agent
    /// may want to react (engage, flee, kite).
    EngagedBy {
        entity_id: u32,
    },
    /// Received a /tell from another player. The most user-directed
    /// channel; nearly always worth waking the LLM for.
    TellReceived {
        from: String,
        text: String,
    },
    /// Supervisor restored the session after a disconnect. The agent
    /// should re-orient to the (possibly changed) zone state.
    Reconnected {
        downtime_ms: u64,
    },
    /// Pre-rendered scene summary, emitted in response to `Snapshot`.
    /// Distinct from `Diagnostics` (operational) — this is the agent's
    /// view of "what's happening right now" in compact prose.
    SceneSummary {
        text: String,
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
    /// `target_id` is its `UniqueNo`. `kind` is a strictly enumerated
    /// `ActionKind` carrying both the action type and any sub-parameters
    /// (spell id, weaponskill id, mount id, ground-target position, …).
    /// Strict enumeration is the lid: an agent cannot pair an arbitrary
    /// `action_id` with mismatched parameters — the type system says no.
    Action {
        target_id: u32,
        target_index: u16,
        kind: ActionKind,
    },
}

/// Tagged-union of every `0x01A` action the agent can perform. The variant
/// chosen determines both the wire `ActionID` and the layout of the 16-byte
/// `ActionBuf` payload — this is the typed alternative to letting the agent
/// invent (action_id, buf) pairs.
///
/// Mirrors `Phoenix/src/map/packets/c2s/0x01a_action.h`. Variants are
/// additive — when a new action type ships in LSB upstream, add a variant
/// here without breaking existing agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActionKind {
    /// 0x00 — Interact with NPC/Trust.
    Talk,
    /// 0x02 — Engage auto-attack on `target_id`.
    Attack,
    /// 0x03 — Cast magic. `spell_id` indexes into `Spells.dat`. `pos_*` are
    /// the ground-target position for AoE-target spells (Tractor, certain
    /// black/blue magic); zero for self/single-target casts.
    CastMagic {
        spell_id: u32,
        pos_x: f32,
        pos_y: f32,
        pos_z: f32,
    },
    /// 0x04 — Disengage auto-attack.
    AttackOff,
    /// 0x05 — Help flag (call for help on a mob).
    Help,
    /// 0x07 — Use a weaponskill.
    Weaponskill { skill_id: u32 },
    /// 0x09 — Use a job ability.
    JobAbility { ability_id: u32 },
    /// 0x0B — Respond to homepoint warp menu. 0=Accept, 1=MonstrosityCancel,
    /// 2=MonstrosityRetry.
    HomepointMenu { status_id: u32 },
    /// 0x0C — /assist (acquire your target's target).
    Assist,
    /// 0x0D — Respond to /raise menu.
    RaiseMenu { accept: bool },
    /// 0x0E — Fishing.
    Fish,
    /// 0x0F — Change target without engaging.
    ChangeTarget,
    /// 0x10 — Ranged attack.
    Shoot,
    /// 0x11 — Chocobo dig.
    ChocoboDig,
    /// 0x12 — Dismount.
    Dismount,
    /// 0x13 — Respond to /tractor menu.
    TractorMenu { accept: bool },
    /// 0x14 — Request character update from the server (rarely useful for
    /// agents; the server pushes updates unprompted).
    SendResRdy,
    /// 0x15 — Mining / Quarrying gather attempt.
    Quarry,
    /// 0x16 — Sprint (Run mode).
    Sprint,
    /// 0x17 — Scout.
    Scout,
    /// 0x18 — Toggle blockaid. 0=Disable, 1=Enable, 2=Toggle.
    Blockaid { status_id: u32 },
    /// 0x19 — Use a monster skill (Monstrosity).
    MonsterSkill { skill_id: u32 },
    /// 0x1A — Summon a mount (chocobo etc.) by `mount_id`.
    Mount { mount_id: u32 },
}

impl ActionKind {
    /// Wire `ActionID` for this action.
    pub fn action_id(&self) -> u16 {
        match self {
            ActionKind::Talk => 0x00,
            ActionKind::Attack => 0x02,
            ActionKind::CastMagic { .. } => 0x03,
            ActionKind::AttackOff => 0x04,
            ActionKind::Help => 0x05,
            ActionKind::Weaponskill { .. } => 0x07,
            ActionKind::JobAbility { .. } => 0x09,
            ActionKind::HomepointMenu { .. } => 0x0B,
            ActionKind::Assist => 0x0C,
            ActionKind::RaiseMenu { .. } => 0x0D,
            ActionKind::Fish => 0x0E,
            ActionKind::ChangeTarget => 0x0F,
            ActionKind::Shoot => 0x10,
            ActionKind::ChocoboDig => 0x11,
            ActionKind::Dismount => 0x12,
            ActionKind::TractorMenu { .. } => 0x13,
            ActionKind::SendResRdy => 0x14,
            ActionKind::Quarry => 0x15,
            ActionKind::Sprint => 0x16,
            ActionKind::Scout => 0x17,
            ActionKind::Blockaid { .. } => 0x18,
            ActionKind::MonsterSkill { .. } => 0x19,
            ActionKind::Mount { .. } => 0x1A,
        }
    }

    /// Fill the 16-byte `ActionBuf` slot per the union layout in
    /// `Phoenix/src/map/packets/c2s/0x01a_action.h`. Variants without a
    /// payload leave the buffer zero-filled, which the server tolerates.
    pub fn fill_action_buf(&self, buf: &mut [u8; 16]) {
        buf.fill(0);
        match self {
            ActionKind::CastMagic {
                spell_id,
                pos_x,
                pos_y,
                pos_z,
            } => {
                buf[0..4].copy_from_slice(&spell_id.to_le_bytes());
                buf[4..8].copy_from_slice(&pos_x.to_le_bytes());
                // Wire order is (PosX, PosZ, PosY) per ACTIONBUF_CASTMAGIC,
                // matching 0x015 POS — different from 0x05E MAPRECT's
                // (x, y, z). FFXI is inconsistent about this; mirror it.
                buf[8..12].copy_from_slice(&pos_z.to_le_bytes());
                buf[12..16].copy_from_slice(&pos_y.to_le_bytes());
            }
            ActionKind::Weaponskill { skill_id }
            | ActionKind::MonsterSkill { skill_id } => {
                buf[0..4].copy_from_slice(&skill_id.to_le_bytes());
            }
            ActionKind::JobAbility { ability_id } => {
                buf[0..4].copy_from_slice(&ability_id.to_le_bytes());
            }
            ActionKind::HomepointMenu { status_id }
            | ActionKind::Blockaid { status_id } => {
                buf[0..4].copy_from_slice(&status_id.to_le_bytes());
            }
            ActionKind::RaiseMenu { accept }
            | ActionKind::TractorMenu { accept } => {
                let id: u32 = if *accept { 0 } else { 1 };
                buf[0..4].copy_from_slice(&id.to_le_bytes());
            }
            ActionKind::Mount { mount_id } => {
                buf[0..4].copy_from_slice(&mount_id.to_le_bytes());
            }
            // Buf-less actions: Talk, Attack, AttackOff, Help, Assist, Fish,
            // ChangeTarget, Shoot, ChocoboDig, Dismount, SendResRdy, Quarry,
            // Sprint, Scout. Already zeroed.
            _ => {}
        }
    }
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
    fn action_kind_talk_decodes() {
        let line = r#"{"cmd":"action","target_id":42,"target_index":7,"kind":{"kind":"talk"}}"#;
        let cmd: AgentCommand = serde_json::from_str(line).unwrap();
        match cmd {
            AgentCommand::Action { target_id, target_index, kind } => {
                assert_eq!((target_id, target_index), (42, 7));
                assert!(matches!(kind, ActionKind::Talk));
                assert_eq!(kind.action_id(), 0x00);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn action_kind_castmagic_fills_buf() {
        let kind = ActionKind::CastMagic { spell_id: 0x101, pos_x: 1.5, pos_y: 0.0, pos_z: -2.5 };
        assert_eq!(kind.action_id(), 0x03);
        let mut buf = [0u8; 16];
        kind.fill_action_buf(&mut buf);
        assert_eq!(u32::from_le_bytes(buf[0..4].try_into().unwrap()), 0x101);
        assert_eq!(f32::from_le_bytes(buf[4..8].try_into().unwrap()), 1.5);
        // Wire order is (PosX, PosZ, PosY) — matches ACTIONBUF_CASTMAGIC.
        assert_eq!(f32::from_le_bytes(buf[8..12].try_into().unwrap()), -2.5);
        assert_eq!(f32::from_le_bytes(buf[12..16].try_into().unwrap()), 0.0);
    }

    #[test]
    fn action_kind_weaponskill_fills_skill_id() {
        let kind = ActionKind::Weaponskill { skill_id: 0xCAFE };
        assert_eq!(kind.action_id(), 0x07);
        let mut buf = [0u8; 16];
        kind.fill_action_buf(&mut buf);
        assert_eq!(u32::from_le_bytes(buf[0..4].try_into().unwrap()), 0xCAFE);
        // Trailing bytes stay zero.
        assert!(buf[4..].iter().all(|&b| b == 0));
    }

    #[test]
    fn party_member_upsert_preserves_name_across_attr_only_update() {
        let mut s = SessionState::default();
        let from_list = PartyMember {
            id: 42,
            act_index: 7,
            name: Some("Vanari".into()),
            hp: 2000,
            mp: 100,
            tp: 0,
            hp_pct: 100,
            mp_pct: 100,
            zone_no: 230,
            main_job: 1,
            main_job_lv: 75,
            sub_job: 6,
            sub_job_lv: 37,
            is_party_leader: true,
            is_alliance_leader: false,
        };
        s.apply_event(&AgentEvent::PartyMemberUpdated { member: from_list });
        assert_eq!(s.party.len(), 1);
        assert_eq!(s.party[0].name.as_deref(), Some("Vanari"));
        assert!(s.party[0].is_party_leader);

        // Subsequent 0x0DF GROUP_ATTR-shaped update: name None, leader false
        // (since attr-only). Must NOT clobber the preserved fields.
        let from_attr = PartyMember {
            id: 42,
            act_index: 7,
            name: None,
            hp: 1500, // took damage
            mp: 100,
            tp: 1234,
            hp_pct: 75,
            mp_pct: 100,
            zone_no: 230,
            main_job: 1,
            main_job_lv: 75,
            sub_job: 6,
            sub_job_lv: 37,
            is_party_leader: false,
            is_alliance_leader: false,
        };
        s.apply_event(&AgentEvent::PartyMemberUpdated { member: from_attr });
        assert_eq!(s.party.len(), 1, "upsert by id");
        assert_eq!(s.party[0].name.as_deref(), Some("Vanari"), "name preserved");
        assert!(s.party[0].is_party_leader, "leader preserved");
        assert_eq!(s.party[0].hp, 1500, "HP overwritten");
        assert_eq!(s.party[0].hp_pct, 75);
    }

    #[test]
    fn action_kind_raise_menu_accept_zero_reject_one() {
        let mut buf = [0u8; 16];
        ActionKind::RaiseMenu { accept: true }.fill_action_buf(&mut buf);
        assert_eq!(u32::from_le_bytes(buf[0..4].try_into().unwrap()), 0);
        ActionKind::RaiseMenu { accept: false }.fill_action_buf(&mut buf);
        assert_eq!(u32::from_le_bytes(buf[0..4].try_into().unwrap()), 1);
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
