use ffxi_viewer_wire as wire;

use crate::state::{
    process_monotonic_ms, AgentEvent, BlowfishStatus, ChatChannel, ChatLine, Diagnostics,
    DialogState, Entity, EntityKind, PartyMember, Position, ReactorGoalSnapshot, ReconnectInfo,
    SessionState, ShopItem, ShopState, Stage, Vec3,
};

pub fn state_to_snapshot(s: &SessionState) -> wire::SceneSnapshot {
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
        net_stats: net_stats_to_wire(&s.net_stats),
        current_goal: s.current_goal.as_ref().map(goal_to_wire),
        last_reconnect: s.last_reconnect.as_ref().map(reconnect_to_wire),

        producer_monotonic_ms: process_monotonic_ms(),

        self_char_id: s.char_id,

        dialog: s.dialog.as_ref().map(dialog_to_wire),

        shop: s.shop.as_ref().map(shop_to_wire),

        status_icons: s.status_icons.clone(),

        status_icon_expiries: s.status_icon_expiries.clone(),

        ability_recasts: s.ability_recasts.clone(),

        logout_countdown: s.logout_countdown.map(|c| wire::LogoutCountdown {
            seconds_remaining: c.seconds_remaining,
            shutdown: c.shutdown,
        }),

        death_homepoint_secs: s.death_homepoint_secs,

        weather: s.current_weather.map(wire::Weather::from_lsb),

        equipped: resolve_equipment(s),

        spells_known: s.spells_known.clone(),
        job_abilities_known: s.job_abilities_known.clone(),
        weaponskills_known: s.weaponskills_known.clone(),
        pet_abilities_known: s.pet_abilities_known.clone(),

        inventory_main: project_inventory_main(s),

        stats: s.char_stats.map(char_stats_to_wire),
        bazaar: Vec::new(),
        play_time_s: 0,

        self_fishing: s.self_fishing.map(|f| wire::SelfFishing {
            phase: f.phase,
            fish_max: f.fish.map(|p| p.stamina).unwrap_or(0),
            fish_hp: f.fish_hp,
            arrow: f.arrow.map(|a| wire::FishingArrow {
                left: a.left,
                golden: a.golden,
            }),
        }),
    }
}

fn project_inventory_main(s: &SessionState) -> Vec<wire::InventoryItem> {
    let Some(bag) = s.inventory.containers.get(&0) else {
        return Vec::new();
    };
    bag.slots
        .iter()
        .map(|slot| wire::InventoryItem {
            container: 0,
            index: slot.index,
            item_no: slot.item_no,
            quantity: slot.quantity,
        })
        .collect()
}

fn resolve_equipment(s: &SessionState) -> [Option<u16>; 16] {
    let mut out = [None; 16];
    for (i, slot) in s.equipment.iter().enumerate() {
        let Some(r) = slot else { continue };
        out[i] = s
            .inventory
            .containers
            .get(&r.container)
            .and_then(|c| c.slots.iter().find(|s| s.index == r.container_index))
            .map(|s| s.item_no);
    }
    out
}

fn char_stats_to_wire(c: crate::state::CharStatsRaw) -> wire::CharStats {
    // CLISTATUS sends item level as the amount above 99 (0 = no iLv gear), so retail
    // adds 99 back for display. vendor/server/src/map/utils/charutils.cpp getItemLevelDifference.
    const ILVL_BASE: u16 = 99;
    let item_level = if c.ilvl >= 1 {
        ILVL_BASE + c.ilvl as u16
    } else {
        0
    };
    wire::CharStats {
        item_level,
        str_: c.bp_base[0],
        dex: c.bp_base[1],
        vit: c.bp_base[2],
        agi: c.bp_base[3],
        int_: c.bp_base[4],
        mnd: c.bp_base[5],
        chr: c.bp_base[6],
        hp_max: c.hp_max,
        mp_max: c.mp_max,
        attack: c.attack,
        defense: c.defense,
        bonus: c.bonus,
        resist: c.resist,
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
        prompt: d.prompt.clone(),
        choices: d.choices.clone(),
    }
}

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
        AgentEvent::LevelUp { player_id } => Some(wire::ViewerEvent::LevelUp { player_id }),
        AgentEvent::SkillLevelUp { skill_id, level } => {
            Some(wire::ViewerEvent::SkillLevelUp { skill_id, level })
        }
        AgentEvent::ActionStarted {
            actor_id,
            action_id,
            action_kind,
        } => Some(wire::ViewerEvent::ActionStarted {
            actor_id,
            action_id,
            action_kind,
        }),
        AgentEvent::VanaTimeSynced { game_time } => {
            Some(wire::ViewerEvent::VanaTimeSynced { game_time })
        }

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
        animation: e.npc_state.map(|s| s.animation).unwrap_or_default(),
        animationsub: e.npc_state.map(|s| s.animationsub).unwrap_or_default(),
        status: e.status,
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

pub fn net_stats_to_wire(n: &crate::state::NetStats) -> wire::NetStats {
    wire::NetStats {
        send_bps: n.send_bps,
        recv_bps: n.recv_bps,
        send_health: n.send_health,
        recv_health: n.recv_health,
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

#[cfg(test)]
mod tests {
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
    fn state_to_snapshot_populates_v2_fields() {
        let s = SessionState {
            character: Some("Sylvie".into()),
            zone_id: Some(230),
            current_goal: Some(ReactorGoalSnapshot::Engaged {
                target_id: 0xCAFE,
                attack_issued: true,
            }),
            last_reconnect: Some(ReconnectInfo {
                downtime_ms: 800,
                at_unix_ms: 1_700_000_002_000,
            }),
            ..Default::default()
        };

        let snap = state_to_snapshot(&s);
        assert_eq!(snap.char_name.as_deref(), Some("Sylvie"));
        assert_eq!(snap.zone_id, Some(230));

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

        let rc = snap.last_reconnect.expect("last_reconnect");
        assert_eq!(rc.downtime_ms, 800);
        assert_eq!(rc.at_unix_ms, 1_700_000_002_000);

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
        let s = SessionState::default();
        let snap = state_to_snapshot(&s);
        assert!(snap.current_goal.is_none());
        assert!(snap.last_reconnect.is_none());

        let snap2 = state_to_snapshot(&s);
        assert!(
            snap2.producer_monotonic_ms >= snap.producer_monotonic_ms,
            "monotonic violation: {} then {}",
            snap.producer_monotonic_ms,
            snap2.producer_monotonic_ms,
        );
    }

    #[test]
    fn resolve_equipment_joins_equipment_against_inventory() {
        use crate::state::{ContainerInfo, EquippedRef, ItemSlot};
        let mut s = SessionState::default();

        let mut inv0 = ContainerInfo::default();
        inv0.slots.push(ItemSlot {
            index: 3,
            item_no: 16448,
            quantity: 1,
            locked: false,
            price: 0,
        });
        s.inventory.containers.insert(0, inv0);

        s.equipment[0] = Some(EquippedRef {
            container: 0,
            container_index: 3,
        });

        s.equipment[4] = Some(EquippedRef {
            container: 0,
            container_index: 99,
        });

        let snap = state_to_snapshot(&s);
        assert_eq!(snap.equipped[0], Some(16448), "main slot resolves");
        assert_eq!(snap.equipped[4], None, "dangling ref → None");
        assert_eq!(snap.equipped[5], None, "empty slot → None");

        assert_eq!(snap.equipped.len(), 16);
    }

    #[test]
    fn equip_cleared_resets_all_slots() {
        use crate::state::EquippedRef;
        let mut s = SessionState::default();

        for cell in s.equipment.iter_mut() {
            *cell = Some(EquippedRef {
                container: 0,
                container_index: 0,
            });
        }
        s.apply_event(&AgentEvent::EquipCleared);
        assert!(s.equipment.iter().all(|c| c.is_none()));
    }
}
