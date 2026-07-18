use serde::{Deserialize, Serialize};

use crate::state::{ChatChannel, EntityKind, SessionState, Stage};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneSummary {
    pub text: String,

    pub stage: Stage,
    pub zone_id: Option<u16>,

    pub self_hp_pct: Option<u8>,
    pub self_mp_pct: Option<u8>,
    pub self_tp: Option<u32>,
    pub party_size: u32,

    pub nearby_pcs: u32,
    pub nearby_npcs: u32,
    pub nearby_mobs: u32,
    pub last_chat_count: u32,
}

impl SceneSummary {
    pub fn from_state(state: &SessionState) -> Self {
        let self_id = state.char_id;
        let self_party = self_id.and_then(|id| state.party.iter().find(|m| m.id == id));
        let self_hp_pct = self_party.map(|m| m.hp_pct);
        let self_mp_pct = self_party.map(|m| m.mp_pct);
        let self_tp = self_party.map(|m| m.tp);

        let mut nearby_pcs = 0u32;
        let mut nearby_npcs = 0u32;
        let mut nearby_mobs = 0u32;
        for e in &state.entities {
            if Some(e.id) == self_id {
                continue;
            }
            match e.kind {
                EntityKind::Pc => nearby_pcs += 1,
                EntityKind::Npc => nearby_npcs += 1,
                EntityKind::Mob => nearby_mobs += 1,
                _ => {}
            }
        }

        let party_size = state.party.len() as u32;
        let last_chat_count = state.chat.len().min(u32::MAX as usize) as u32;

        let text = render_prose(state, self_party, nearby_pcs, nearby_npcs, nearby_mobs);

        Self {
            text,
            stage: state.stage,
            zone_id: state.zone_id,
            self_hp_pct,
            self_mp_pct,
            self_tp,
            party_size,
            nearby_pcs,
            nearby_npcs,
            nearby_mobs,
            last_chat_count,
        }
    }
}

fn render_prose(
    state: &SessionState,
    self_party: Option<&crate::state::PartyMember>,
    nearby_pcs: u32,
    nearby_npcs: u32,
    nearby_mobs: u32,
) -> String {
    let mut out = String::with_capacity(256);

    match state.stage {
        Stage::Idle => {
            out.push_str("Session not started.");
            return out;
        }
        Stage::Authenticating => {
            out.push_str("Authenticating with login server.");
            return out;
        }
        Stage::LobbyHandshake => {
            out.push_str("Lobby handshake in progress.");
            return out;
        }
        Stage::MapBootstrap => {
            out.push_str("Connecting to map server.");
            return out;
        }
        Stage::Zoning => {
            out.push_str("Zoning. ");
        }
        Stage::InZone => {}
        Stage::Disconnected => {
            out.push_str("Disconnected. Supervisor will attempt to reconnect.");
            return out;
        }
    }

    match (state.character.as_deref(), state.zone_id) {
        (Some(name), Some(z)) => {
            out.push_str("You are ");
            out.push_str(name);
            out.push_str(" in zone ");
            out.push_str(&z.to_string());
            out.push('.');
        }
        (Some(name), None) => {
            out.push_str("You are ");
            out.push_str(name);
            out.push_str(", zone unknown.");
        }
        (None, Some(z)) => {
            out.push_str("In zone ");
            out.push_str(&z.to_string());
            out.push('.');
        }
        (None, None) => {
            out.push_str("In zone.");
        }
    }

    if let Some(m) = self_party {
        out.push_str(" HP ");
        out.push_str(&m.hp_pct.to_string());
        out.push_str("%, MP ");
        out.push_str(&m.mp_pct.to_string());
        out.push_str("%, TP ");
        out.push_str(&m.tp.to_string());
        out.push('.');
    }

    out.push(' ');
    out.push_str(&format_entity_counts(nearby_pcs, nearby_npcs, nearby_mobs));

    if let Some(last) = state.chat.last() {
        let trimmed = trim_chat_text(&last.text, 60);
        let channel = chat_channel_label(last.channel);
        out.push_str(" Last ");
        out.push_str(channel);
        out.push_str(": ");
        out.push_str(&last.sender);
        out.push_str(": ");
        out.push_str(&trimmed);
    }

    out
}

fn format_entity_counts(pcs: u32, npcs: u32, mobs: u32) -> String {
    if pcs == 0 && npcs == 0 && mobs == 0 {
        return "Nothing nearby.".into();
    }
    let mut parts: Vec<String> = Vec::new();
    if pcs > 0 {
        parts.push(format!("{pcs} PC{}", plural(pcs)));
    }
    if npcs > 0 {
        parts.push(format!("{npcs} NPC{}", plural(npcs)));
    }
    if mobs > 0 {
        parts.push(format!("{mobs} mob{}", plural(mobs)));
    }
    format!("{} nearby.", parts.join(", "))
}

fn plural(n: u32) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn chat_channel_label(c: ChatChannel) -> &'static str {
    match c {
        ChatChannel::Say => "say",
        ChatChannel::Shout => "shout",
        ChatChannel::Tell => "tell",
        ChatChannel::Party => "party",
        ChatChannel::Linkshell => "ls",
        ChatChannel::Yell => "yell",
        ChatChannel::System => "sys",
        ChatChannel::Battle => "battle",
        ChatChannel::Debug => "dbg",
        ChatChannel::Other => "chat",
        ChatChannel::Emote => "emote",
    }
}

fn trim_chat_text(text: &str, max: usize) -> String {
    let cleaned: String = text.chars().filter(|c| !c.is_control()).collect();
    if cleaned.len() <= max {
        cleaned
    } else {
        let mut s: String = cleaned.chars().take(max - 1).collect();
        s.push('…');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{
        AgentEvent, ChatLine, Entity, EntityKind, PartyMember, SessionState, Stage, Vec3,
    };

    fn base_state() -> SessionState {
        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::Connected {
            account_id: 1,
            char_id: 42,
            character: "Vanari".into(),
            zone_id: 230,
        });
        s.apply_event(&AgentEvent::StageChanged {
            stage: Stage::InZone,
        });
        s
    }

    #[test]
    fn idle_state_renders_minimal() {
        let s = SessionState::default();
        let scene = SceneSummary::from_state(&s);
        assert_eq!(scene.stage, Stage::Idle);
        assert_eq!(scene.text, "Session not started.");
        assert_eq!(scene.party_size, 0);
        assert_eq!(scene.nearby_pcs, 0);
    }

    #[test]
    fn in_zone_renders_name_and_zone() {
        let s = base_state();
        let scene = SceneSummary::from_state(&s);
        assert!(scene.text.contains("Vanari"));
        assert!(scene.text.contains("230"));
        assert!(scene.text.contains("Nothing nearby"));
    }

    #[test]
    fn nearby_entities_excludes_self() {
        let mut s = base_state();

        s.apply_event(&AgentEvent::EntityUpserted {
            entity: Entity {
                id: 42,
                act_index: 1,
                kind: EntityKind::Pc,
                name: Some("Vanari".into()),
                pos: Vec3::default(),
                heading: 0,
                hp_pct: Some(100),
                bt_target_id: 0,
                face_target: 0,
                claim_id: 0,
                speed: 0,
                speed_base: 0,
                look: None,
                npc_state: None,
                status: 0,
            },
            pos_present: true,
        });

        s.apply_event(&AgentEvent::EntityUpserted {
            entity: Entity {
                id: 100,
                act_index: 2,
                kind: EntityKind::Pc,
                name: Some("Stranger".into()),
                pos: Vec3 {
                    x: 5.0,
                    y: 0.0,
                    z: 0.0,
                },
                heading: 0,
                hp_pct: Some(100),
                bt_target_id: 0,
                face_target: 0,
                claim_id: 0,
                speed: 0,
                speed_base: 0,
                look: None,
                npc_state: None,
                status: 0,
            },
            pos_present: true,
        });

        s.apply_event(&AgentEvent::EntityUpserted {
            entity: Entity {
                id: 200,
                act_index: 3,
                kind: EntityKind::Npc,
                name: Some("Innkeeper".into()),
                pos: Vec3::default(),
                heading: 0,
                hp_pct: Some(100),
                bt_target_id: 0,
                face_target: 0,
                claim_id: 0,
                speed: 0,
                speed_base: 0,
                look: None,
                npc_state: None,
                status: 0,
            },
            pos_present: true,
        });

        s.apply_event(&AgentEvent::EntityUpserted {
            entity: Entity {
                id: 300,
                act_index: 4,
                kind: EntityKind::Mob,
                name: None,
                pos: Vec3::default(),
                heading: 0,
                hp_pct: Some(100),
                bt_target_id: 0,
                face_target: 0,
                claim_id: 0,
                speed: 0,
                speed_base: 0,
                look: None,
                npc_state: None,
                status: 0,
            },
            pos_present: true,
        });

        let scene = SceneSummary::from_state(&s);
        assert_eq!(scene.nearby_pcs, 1, "self excluded");
        assert_eq!(scene.nearby_npcs, 1);
        assert_eq!(scene.nearby_mobs, 1);
        assert!(scene.text.contains("1 PC"));
        assert!(scene.text.contains("1 NPC"));
        assert!(scene.text.contains("1 mob"));
    }

    #[test]
    fn self_hp_mp_tp_pulled_from_party_row_for_self() {
        let mut s = base_state();
        s.apply_event(&AgentEvent::PartyMemberUpdated {
            member: PartyMember {
                id: 42,
                act_index: 1,
                name: None,
                hp: 1500,
                mp: 200,
                tp: 1750,
                hp_pct: 75,
                mp_pct: 50,
                zone_no: 230,
                main_job: 1,
                main_job_lv: 75,
                sub_job: 6,
                sub_job_lv: 37,
                is_party_leader: false,
                is_alliance_leader: false,
                in_mog_house: false,
            },
        });
        let scene = SceneSummary::from_state(&s);
        assert_eq!(scene.self_hp_pct, Some(75));
        assert_eq!(scene.self_mp_pct, Some(50));
        assert_eq!(scene.self_tp, Some(1750));
        assert!(scene.text.contains("HP 75%"));
        assert!(scene.text.contains("MP 50%"));
        assert!(scene.text.contains("TP 1750"));
    }

    #[test]
    fn last_chat_appears_in_text() {
        let mut s = base_state();
        s.apply_event(&AgentEvent::ChatLine {
            line: ChatLine {
                channel: ChatChannel::Say,
                sender: "Foo".into(),
                text: "hello world".into(),
                server_ts: 0,
            },
        });
        let scene = SceneSummary::from_state(&s);
        assert!(
            scene.text.contains("Last say: Foo: hello world"),
            "got: {}",
            scene.text
        );
        assert_eq!(scene.last_chat_count, 1);
    }

    #[test]
    fn long_chat_is_truncated_with_ellipsis() {
        let mut s = base_state();
        let long: String = "x".repeat(120);
        s.apply_event(&AgentEvent::ChatLine {
            line: ChatLine {
                channel: ChatChannel::Tell,
                sender: "F".into(),
                text: long,
                server_ts: 0,
            },
        });
        let scene = SceneSummary::from_state(&s);
        assert!(
            scene.text.contains('…'),
            "long chat should be truncated: {}",
            scene.text
        );
    }

    #[test]
    fn disconnected_state_short_circuits() {
        let mut s = base_state();
        s.apply_event(&AgentEvent::Disconnected {
            reason: "test".into(),
        });
        let scene = SceneSummary::from_state(&s);
        assert_eq!(scene.stage, Stage::Disconnected);
        assert!(scene.text.contains("Disconnected"));
    }

    #[test]
    fn party_size_counts_all_members() {
        let mut s = base_state();
        for id in [42u32, 43, 44] {
            s.apply_event(&AgentEvent::PartyMemberUpdated {
                member: PartyMember {
                    id,
                    act_index: 1,
                    name: Some("M".into()),
                    hp: 100,
                    mp: 100,
                    tp: 0,
                    hp_pct: 100,
                    mp_pct: 100,
                    zone_no: 230,
                    main_job: 1,
                    main_job_lv: 75,
                    sub_job: 6,
                    sub_job_lv: 37,
                    is_party_leader: id == 42,
                    is_alliance_leader: false,
                    in_mog_house: false,
                },
            });
        }
        let scene = SceneSummary::from_state(&s);
        assert_eq!(scene.party_size, 3);
    }

    #[test]
    fn diagnostics_alone_does_not_resync_stage_for_scene() {
        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::Diagnostics {
            diagnostics: crate::state::Diagnostics {
                stage: Some(Stage::InZone),
                ..Default::default()
            },
        });
        let scene = SceneSummary::from_state(&s);
        assert_eq!(
            scene.text, "Session not started.",
            "Diagnostics alone cannot resync state.stage — \
             this is the precondition the Snapshot burst exists to fix"
        );

        s.apply_event(&AgentEvent::Connected {
            account_id: 0,
            char_id: 42,
            character: "Vanari".into(),
            zone_id: 230,
        });
        s.apply_event(&AgentEvent::StageChanged {
            stage: Stage::InZone,
        });
        let scene = SceneSummary::from_state(&s);
        assert_ne!(scene.text, "Session not started.");
        assert!(
            scene.text.contains("Vanari") && scene.text.contains("230"),
            "expected in-zone prose with name and zone, got: {}",
            scene.text
        );
    }
}
