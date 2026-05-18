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
    process_monotonic_ms, AgentEvent, BlowfishStatus, ChatChannel, ChatLine, Diagnostics,
    DialogState, Entity, EntityKind, LlmDecision, LlmDecisionKind, PartyMember, Position,
    ReactorGoalSnapshot, ReconnectInfo, SessionState, ShopItem, ShopState, Stage, Vec3,
};

/// Snapshot the full `SessionState` into a wire `SceneSnapshot`.
///
/// Takes a reference because both call sites (the native bridge after a
/// `borrow_and_update`, the relay after a `borrow`) hold a guard on the
/// watch channel and cloning into the wire struct is cheaper than
/// cloning the entire state and then translating.
pub fn state_to_snapshot(s: &SessionState) -> wire::SceneSnapshot {
    // Derive self position from the entity list — `self_position()` reads
    // the entry whose `id == self.char_id`. Falls back to
    // `Position::default()` for the brief pre-LOGIN window where neither
    // `char_id` nor the self entity has been seeded yet.
    let self_pos = position_to_wire(s.self_position().unwrap_or_default());
    wire::SceneSnapshot {
        stage: stage_to_wire(s.stage),
        char_name: s.character.clone(),
        zone_id: s.zone_id,
        self_pos,
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
        // Surface the player's UniqueNo so the viewer can compare it
        // against per-mob `claim_id` for self-claim coloring.
        self_char_id: s.char_id,
        // Active NPC dialog, if any. Translated field-for-field from the
        // in-house `state::DialogState` to the wire form.
        dialog: s.dialog.as_ref().map(dialog_to_wire),
        // Active NPC shop, same translation pattern.
        shop: s.shop.as_ref().map(shop_to_wire),
        // Plain `Vec<u16>` clone — no per-element translation needed.
        status_icons: s.status_icons.clone(),
        // Direct field copy — `state::LogoutCountdown` and
        // `wire::LogoutCountdown` are structurally identical (two
        // primitive fields). Kept as separate types per the same
        // wire/state decoupling that DialogState and ShopState use.
        logout_countdown: s.logout_countdown.map(|c| wire::LogoutCountdown {
            seconds_remaining: c.seconds_remaining,
            shutdown: c.shutdown,
        }),
        // Map raw LSB `WeatherNumber` to the typed wire enum. The fold
        // stores u16 to keep state.rs decoupled from `ffxi-viewer-wire`;
        // `Weather::from_lsb` is the authoritative table (handles the
        // 0x14..=0x27 repeat range via mod-20).
        weather: s.current_weather.map(wire::Weather::from_lsb),
    }
}

pub fn shop_to_wire(s: &ShopState) -> wire::ShopState {
    wire::ShopState {
        offset_index: s.offset_index,
        items: s.items.iter().map(shop_item_to_wire).collect(),
        opened: s.opened,
    }
}

pub fn shop_item_to_wire(i: &ShopItem) -> wire::ShopItem {
    wire::ShopItem {
        price: i.price,
        item_no: i.item_no,
        shop_index: i.shop_index,
        skill: i.skill,
        guild_info: i.guild_info,
    }
}

pub fn dialog_to_wire(d: &DialogState) -> wire::DialogState {
    wire::DialogState {
        event_id: d.event_id,
        npc_id: d.npc_id,
        npc_name: d.npc_name.clone(),
        act_index: d.act_index,
        event_num: d.event_num,
        event_para: d.event_para,
        mode: d.mode,
        event_num2: d.event_num2,
        event_para2: d.event_para2,
        strings: d.strings.clone(),
        nums: d.nums.clone(),
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
        AgentEvent::MusicChanged { slot, track_id } => {
            Some(wire::ViewerEvent::MusicChanged { slot, track_id })
        }
        AgentEvent::MusicVolumeChanged { slot, volume } => {
            Some(wire::ViewerEvent::MusicVolumeChanged { slot, volume })
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

/// Convert the proto-layer `LookData` (no serde) into the wire-layer
/// `EntityLook` (serde-bearing). Pure mapping; variants are 1:1.
pub fn look_to_wire(l: ffxi_proto::decode::LookData) -> wire::EntityLook {
    use ffxi_proto::decode::LookData;
    match l {
        LookData::Standard { modelid } => wire::EntityLook::Standard { modelid },
        LookData::Equipped {
            face,
            race,
            head,
            body,
            hands,
            legs,
            feet,
            main,
            sub,
            ranged,
        } => wire::EntityLook::Equipped {
            face,
            race,
            head,
            body,
            hands,
            legs,
            feet,
            main,
            sub,
            ranged,
        },
        LookData::Door { size } => wire::EntityLook::Door { size },
        LookData::Transport { size } => wire::EntityLook::Transport { size },
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
        claim_id: e.claim_id,
        speed: e.speed,
        speed_base: e.speed_base,
        look: e.look.map(look_to_wire),
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
        // `local_seq` is the client-side monotonic arrival counter used
        // to interleave server chat with `push_local_toast` entries in
        // strict arrival order. Server-sourced lines (everything that
        // flows through this bridge) carry 0 — the same sentinel the
        // wire struct documents as "synthetic / test / pre-traffic".
        // Local-toast emitters stamp their own non-zero values.
        local_seq: 0,
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
        ChatChannel::Battle => wire::ChatChannel::Battle,
        ChatChannel::Debug => wire::ChatChannel::Debug,
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
        in_mog_house: m.in_mog_house,
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
        ReactorGoalSnapshot::Following {
            target_id,
            distance,
        } => wire::ReactorGoal::Following {
            target_id,
            distance,
        },
        ReactorGoalSnapshot::Engaged {
            target_id,
            attack_issued,
        } => wire::ReactorGoal::Engaged {
            target_id,
            attack_issued,
        },
        ReactorGoalSnapshot::Pathing {
            x,
            y,
            z,
            waypoints_remaining,
        } => wire::ReactorGoal::Pathing {
            x,
            y,
            z,
            waypoints_remaining,
        },
        ReactorGoalSnapshot::Banking {
            threshold,
            mog_house_zoneline,
        } => wire::ReactorGoal::Banking {
            threshold,
            mog_house_zoneline,
        },
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    #[test]
    fn goal_to_wire_covers_all_variants() {
        let cases = vec![
            (
                ReactorGoalSnapshot::Idle,
                matches_idle as fn(&wire::ReactorGoal) -> bool,
            ),
            (
                ReactorGoalSnapshot::Following {
                    target_id: 0x42,
                    distance: 3.0,
                },
                matches_following,
            ),
            (
                ReactorGoalSnapshot::Engaged {
                    target_id: 0x99,
                    attack_issued: true,
                },
                matches_engaged,
            ),
            (
                ReactorGoalSnapshot::Pathing {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                    waypoints_remaining: 4,
                },
                matches_pathing,
            ),
            (
                ReactorGoalSnapshot::Banking {
                    threshold: 60,
                    mog_house_zoneline: 0xDEAD,
                },
                matches_banking,
            ),
        ];
        for (g, check) in cases {
            let w = goal_to_wire(&g);
            assert!(check(&w), "wire form of {g:?} failed shape-check ({w:?})");
        }
    }

    fn matches_idle(w: &wire::ReactorGoal) -> bool {
        matches!(w, wire::ReactorGoal::Idle)
    }
    fn matches_following(w: &wire::ReactorGoal) -> bool {
        matches!(w, wire::ReactorGoal::Following { target_id: 0x42, distance } if (distance - 3.0).abs() < f32::EPSILON)
    }
    fn matches_engaged(w: &wire::ReactorGoal) -> bool {
        matches!(
            w,
            wire::ReactorGoal::Engaged {
                target_id: 0x99,
                attack_issued: true
            }
        )
    }
    fn matches_pathing(w: &wire::ReactorGoal) -> bool {
        match w {
            wire::ReactorGoal::Pathing {
                x,
                y,
                z,
                waypoints_remaining,
            } => {
                (*x - 1.0).abs() < f32::EPSILON
                    && (*y - 2.0).abs() < f32::EPSILON
                    && (*z - 3.0).abs() < f32::EPSILON
                    && *waypoints_remaining == 4
            }
            _ => false,
        }
    }
    fn matches_banking(w: &wire::ReactorGoal) -> bool {
        matches!(
            w,
            wire::ReactorGoal::Banking {
                threshold: 60,
                mog_house_zoneline: 0xDEAD
            }
        )
    }

    #[test]
    fn reconnect_to_wire_passes_through() {
        let r = ReconnectInfo {
            downtime_ms: 1234,
            at_unix_ms: 1_700_000_001_000,
        };
        let w = reconnect_to_wire(&r);
        assert_eq!(w.downtime_ms, 1234);
        assert_eq!(w.at_unix_ms, 1_700_000_001_000);
    }

    #[test]
    fn decision_to_wire_covers_both_kinds() {
        let nf = LlmDecision {
            kind: LlmDecisionKind::NotificationFired {
                uri: "scene://current".into(),
            },
            latency_us: 412,
            at_monotonic_ms: 1000,
        };
        let w = decision_to_wire(&nf);
        assert_eq!(w.latency_us, 412);
        assert_eq!(w.at_monotonic_ms, 1000);
        match w.kind {
            wire::LlmDecisionKind::NotificationFired { uri } => {
                assert_eq!(uri, "scene://current");
            }
            other => panic!("wrong kind: {other:?}"),
        }

        let td = LlmDecision {
            kind: LlmDecisionKind::ToolDispatched {
                tool: "engage".into(),
            },
            latency_us: 25_000,
            at_monotonic_ms: 1_100,
        };
        match decision_to_wire(&td).kind {
            wire::LlmDecisionKind::ToolDispatched { tool } => assert_eq!(tool, "engage"),
            other => panic!("wrong kind: {other:?}"),
        }
    }

    #[test]
    fn state_to_snapshot_populates_v2_fields() {
        // SessionState::default + manual writes to the four observability
        // fields. The translator must surface every one of them.
        let mut s = SessionState::default();
        s.character = Some("Sylvie".into());
        s.zone_id = Some(230);
        s.current_goal = Some(ReactorGoalSnapshot::Engaged {
            target_id: 0xCAFE,
            attack_issued: true,
        });
        s.last_reconnect = Some(ReconnectInfo {
            downtime_ms: 800,
            at_unix_ms: 1_700_000_002_000,
        });
        s.recent_decisions = VecDeque::from(vec![
            LlmDecision {
                kind: LlmDecisionKind::NotificationFired {
                    uri: "scene://current".into(),
                },
                latency_us: 200,
                at_monotonic_ms: 100,
            },
            LlmDecision {
                kind: LlmDecisionKind::ToolDispatched {
                    tool: "engage".into(),
                },
                latency_us: 25_000,
                at_monotonic_ms: 200,
            },
        ]);

        let snap = state_to_snapshot(&s);
        assert_eq!(snap.char_name.as_deref(), Some("Sylvie"));
        assert_eq!(snap.zone_id, Some(230));

        // Goal mirror.
        match snap.current_goal {
            Some(wire::ReactorGoal::Engaged {
                target_id,
                attack_issued,
            }) => {
                assert_eq!(target_id, 0xCAFE);
                assert!(attack_issued);
            }
            other => panic!("goal: {other:?}"),
        }

        // Reconnect mirror.
        let rc = snap.last_reconnect.expect("last_reconnect");
        assert_eq!(rc.downtime_ms, 800);
        assert_eq!(rc.at_unix_ms, 1_700_000_002_000);

        // Decisions vec preserves order and pairing-relevant kind variants.
        assert_eq!(snap.recent_decisions.len(), 2);
        assert!(matches!(
            &snap.recent_decisions[0].kind,
            wire::LlmDecisionKind::NotificationFired { uri } if uri == "scene://current"
        ));
        assert!(matches!(
            &snap.recent_decisions[1].kind,
            wire::LlmDecisionKind::ToolDispatched { tool } if tool == "engage"
        ));

        // producer_monotonic_ms is stamped from the live process clock.
        // The first call to `process_monotonic_ms()` initializes the
        // `OnceLock<Instant>` anchor, so the value can legitimately be 0
        // (zero ms elapsed since anchor set on this very call). We can't
        // meaningfully cross-compare with the hand-fabricated decision
        // timestamps either — those are arbitrary fixture values, not
        // anchored to the same clock.
        //
        // Monotonicity is what the badge clock actually relies on:
        // a snapshot taken later must have a >= producer_monotonic_ms.
        let snap2 = state_to_snapshot(&s);
        assert!(
            snap2.producer_monotonic_ms >= snap.producer_monotonic_ms,
            "producer_monotonic_ms must be monotonic across snapshots; \
             got {} then {}",
            snap.producer_monotonic_ms,
            snap2.producer_monotonic_ms,
        );
    }

    #[test]
    fn state_to_snapshot_v2_fields_default_empty() {
        // Default SessionState — no goal, no reconnect, empty decisions.
        // The translator should write `None` / empty Vec, not panic or
        // fabricate data.
        let s = SessionState::default();
        let snap = state_to_snapshot(&s);
        assert!(snap.current_goal.is_none());
        assert!(snap.last_reconnect.is_none());
        assert!(snap.recent_decisions.is_empty());

        // producer_monotonic_ms is still stamped from the live process
        // clock. We can't assert `> 0` — `process_monotonic_ms()` may
        // legitimately return 0 if this test is the first caller (the
        // OnceLock<Instant> anchor sets to `Instant::now()` on that
        // same call). Assert the real contract instead: monotonicity
        // across consecutive snapshots, which the badge clock relies on
        // to extrapolate `producer_now_ms` between fresh snapshots.
        let snap2 = state_to_snapshot(&s);
        assert!(
            snap2.producer_monotonic_ms >= snap.producer_monotonic_ms,
            "monotonic violation: {} then {}",
            snap.producer_monotonic_ms,
            snap2.producer_monotonic_ms,
        );
    }
}
