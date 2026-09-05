use kuluu_snapshot as wire;

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
        sub_area: s.sub_area,
        self_pos,
        entities: s.entities.iter().map(entity_to_wire).collect(),
        party: s.party.iter().map(party_to_wire).collect(),
        zone_generation: s.zone_generation,
        chat: s.chat.iter().map(chat_to_wire).collect(),
        chat_base_seq: s.chat_dropped,
        diagnostics: diagnostics_to_wire(&s.diagnostics),
        net_stats: net_stats_to_wire(&s.net_stats),
        current_goal: s.current_goal.as_ref().map(goal_to_wire),
        last_reconnect: s.last_reconnect.as_ref().map(reconnect_to_wire),

        producer_monotonic_ms: process_monotonic_ms(),

        self_char_id: s.char_id,

        dialog: s.dialog.as_ref().map(dialog_to_wire),

        shop: s.shop.as_ref().map(shop_to_wire),

        delivery_box: delivery_box_to_wire(&s.delivery_box),
        treasure_pool: s
            .treasure_pool
            .iter()
            .flatten()
            .map(treasure_slot_to_wire)
            .collect(),

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
        key_items: s.key_items.clone(),
        key_items_seen: s.key_items_seen.clone(),

        containers: project_containers(s),

        stats: s.char_stats.map(char_stats_to_wire),
        bazaar: s.bazaar.as_ref().map(bazaar_to_wire),
        auction: auction_to_wire(&s.auction),
        play_time_s: 0,

        self_fishing: s.self_fishing.map(|f| wire::SelfFishing {
            phase: f.phase,
            fish_max: f.fish.map(|p| p.stamina).unwrap_or(0),
            fish_hp: f.fish_hp,
            arrow: f.arrow.map(|a| wire::FishingArrow {
                left: a.left,
                golden: a.golden,
            }),
            size: f.size.map(|s| match s {
                crate::state::FishSize::Small => wire::FishSize::Small,
                crate::state::FishSize::Large => wire::FishSize::Large,
            }),
        }),

        self_server_status: s.self_server_status,

        self_mount: mount_to_wire(s.self_server_status, s.self_mount_id),

        self_casting: s.self_casting.as_ref().map(|c| wire::SelfCasting {
            name: c.name.clone(),
            elapsed_ms: c.elapsed_ms,
            total_ms: c.total_ms,
            interrupted: c.interrupted,
        }),

        myroom: s.myroom.map(|m| wire::MyRoom {
            model: m.model,
            sub_map: m.sub_map,
        }),

        mh_2f_unlocked: s.mh_2f_unlocked,

        emote_jobs: s.emote_jobs,
        emote_chairs: s.emote_chairs,

        check: s.check_result.as_ref().map(check_to_wire),

        check_message: s.check_message.as_ref().map(|m| wire::CheckMessage {
            name: m.name.clone(),
            message: m.message.clone(),
        }),

        widescan: widescan_to_wire(&s.widescan),

        death_menu_offer: s.death_menu_offer.map(|offer| match offer {
            ffxi_proto::decode::DeathMenuOffer::Raise => wire::DeathMenuOffer::Raise,
            ffxi_proto::decode::DeathMenuOffer::Tractor => wire::DeathMenuOffer::Tractor,
        }),
    }
}

fn widescan_to_wire(w: &crate::state::WidescanList) -> wire::WidescanList {
    wire::WidescanList {
        entries: w
            .entries
            .iter()
            .map(|e| wire::WidescanEntry {
                act_index: e.act_index,
                level: e.level,
                kind: e.kind,
                rel_x: e.rel_x,
                rel_z: e.rel_z,
                name: e.name.clone(),
            })
            .collect(),
        tracked: w.tracked.map(|t| wire::WidescanTracked {
            act_index: t.act_index,
            x: t.x,
            y: t.y,
            z: t.z,
        }),
    }
}

fn bazaar_to_wire(b: &crate::state::BazaarView) -> wire::BazaarView {
    wire::BazaarView {
        seller_id: b.seller_id,
        seller_name: b.seller_name.clone(),
        items: b
            .items
            .iter()
            .map(|it| wire::BazaarEntry {
                index: it.index,
                item_no: it.item_no,
                quantity: it.quantity,
                price: it.price,
                tax_rate: it.tax_rate,
            })
            .collect(),
    }
}

fn auction_to_wire(a: &crate::state::AuctionState) -> wire::AuctionUi {
    wire::AuctionUi {
        open: a.open,
        browse: a.browse.as_ref().map(|b| wire::AhCatalogView {
            category: b.category,
            total: b.total,
            listings: b
                .listings
                .iter()
                .map(|l| wire::AhListingView {
                    item_id: l.item_id,
                    singles_for_sale: l.singles_for_sale,
                    stacks_for_sale: l.stacks_for_sale,
                })
                .collect(),
        }),
        history: a.history.as_ref().map(|h| wire::AhHistoryView {
            item_id: h.item_id,
            stack: h.stack,
            open_listings: h.open_listings,
            category: h.category,
            sales: h
                .sales
                .iter()
                .map(|s| wire::AhSaleView {
                    price: s.price,
                    sell_date: s.sell_date,
                    seller: s.seller.clone(),
                    buyer: s.buyer.clone(),
                })
                .collect(),
        }),
        sales_status: a.sales_status.clone().map(|slot| {
            slot.map(|s| wire::AhSaleStatus {
                stat: s.stat,
                item_no: s.item_no,
                quantity: s.quantity,
                price: s.price,
                timestamp: s.timestamp,
            })
        }),
        fee_quote: a.fee_quote.map(|q| wire::AhFeeQuote {
            fee: q.fee,
            inventory_slot: q.inventory_slot,
            item_no: q.item_no,
            stack: q.stack,
            asking_price: q.asking_price,
        }),
        busy: a.busy.map(|b| match b {
            crate::state::AuctionBusy::Downloading => wire::AuctionBusy::Downloading,
            crate::state::AuctionBusy::PlacingBid => wire::AuctionBusy::PlacingBid,
        }),
    }
}

fn check_to_wire(c: &crate::state::CheckResult) -> wire::CheckResult {
    wire::CheckResult {
        target_id: c.target_id,
        equipped: c.equipped,
        main_job: c.main_job,
        sub_job: c.sub_job,
        main_job_lv: c.main_job_lv,
        sub_job_lv: c.sub_job_lv,
        master_lv: c.master_lv,
        linkshell: c.linkshell.clone(),
    }
}

fn project_containers(s: &SessionState) -> Vec<wire::ContainerView> {
    let mut out: Vec<wire::ContainerView> = s
        .inventory
        .containers
        .iter()
        .filter(|(_, c)| c.capacity > 0 || !c.slots.is_empty())
        .map(|(&id, c)| wire::ContainerView {
            id,
            capacity: c.capacity as u16,
            items: c
                .slots
                .iter()
                .map(|slot| wire::InventoryItem {
                    container: id,
                    index: slot.index,
                    item_no: slot.item_no,
                    quantity: slot.quantity,
                    locked: slot.locked,
                    charges_remaining: slot.charges_remaining,
                    next_use_vana_ts: slot.next_use_vana_ts,
                })
                .collect(),
        })
        .collect();
    out.sort_by_key(|c| c.id);
    out
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
        text_entry: d.text_entry,
        grid: d.grid.as_ref().map(grid_to_wire),
        custom_menu: d.custom_menu,
    }
}

fn delivery_box_to_wire(d: &crate::state::DeliveryBoxState) -> Option<wire::DeliveryBoxState> {
    let box_no = d.open?;
    Some(wire::DeliveryBoxState {
        box_no: match box_no {
            crate::state::DeliveryBoxNo::Incoming => wire::DeliveryBoxNo::Incoming,
            crate::state::DeliveryBoxNo::Outgoing => wire::DeliveryBoxNo::Outgoing,
        },
        slots: d
            .slots
            .iter()
            .map(|cell| {
                cell.as_ref().map(|item| wire::DeliverySlot {
                    item_no: item.item_no,
                    quantity: item.quantity,
                    counterpart: item.counterpart.clone(),
                    stat: item.stat,
                })
            })
            .collect(),
        queued: d.queued,
        recipient: d.recipient.clone(),
        recipient_status: match d.recipient_status {
            crate::state::RecipientStatus::Unset => wire::RecipientStatus::Unset,
            crate::state::RecipientStatus::Pending => wire::RecipientStatus::Pending,
            crate::state::RecipientStatus::Ok { same_account } => {
                wire::RecipientStatus::Ok { same_account }
            }
            crate::state::RecipientStatus::NoSuchChar => wire::RecipientStatus::NoSuchChar,
        },
    })
}

fn grid_to_wire(g: &crate::state::DialogGrid) -> wire::DialogGrid {
    wire::DialogGrid {
        cols: g.cols,
        rows: g.rows,
        cells: g
            .cells
            .iter()
            .map(|c| wire::DialogGridCell {
                choice: c.choice,
                item_no: c.item_no,
                quantity: c.quantity,
                sent: c.sent,
            })
            .collect(),
    }
}

pub fn event_to_viewer_event(ev: AgentEvent) -> Option<wire::ViewerEvent> {
    match ev {
        AgentEvent::ZoneChanged { from, to, .. } => {
            Some(wire::ViewerEvent::ZoneChanged { from, to })
        }
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
            target_id,
            result,
            animation,
        } => Some(wire::ViewerEvent::ActionStarted {
            actor_id,
            action_id,
            action_kind,
            target_id,
            result: result.map(ffxi_proto::melee::MeleeResult::to_wire),
            animation,
        }),
        AgentEvent::EntityEmoted {
            actor_id,
            target_id,
            emote_id,
            param,
            mode,
            ..
        } => Some(wire::ViewerEvent::EntityEmoted {
            actor_id,
            target_id,
            emote_id,
            param,
            mode,
        }),
        AgentEvent::VanaTimeSynced { game_time } => {
            Some(wire::ViewerEvent::VanaTimeSynced { game_time })
        }
        AgentEvent::AuctionMenuOpened => Some(wire::ViewerEvent::AuctionMenuOpened),
        AgentEvent::AuctionBidResult {
            ok,
            item_no,
            price,
            quantity,
            ..
        } => Some(wire::ViewerEvent::AuctionBidResult {
            ok,
            item_no,
            price,
            quantity,
        }),
        AgentEvent::AuctionSellResult { ok, .. } => {
            Some(wire::ViewerEvent::AuctionSellResult { ok })
        }
        AgentEvent::AuctionSellQuote {
            quote: None,
            result,
        } => Some(wire::ViewerEvent::AuctionSellRefused { result }),
        AgentEvent::AuctionCancelResult { slot, ok, .. } => {
            Some(wire::ViewerEvent::AuctionCancelResult { slot, ok })
        }
        AgentEvent::AuctionSearchFailed { message } => {
            Some(wire::ViewerEvent::AuctionSearchFailed { message })
        }
        AgentEvent::CutsceneStarted { event_id } => {
            Some(wire::ViewerEvent::CutsceneStarted { event_id })
        }
        AgentEvent::CutsceneCue { cue } => Some(wire::ViewerEvent::Cutscene {
            cue: cutscene_cue_to_wire(cue),
        }),
        AgentEvent::CutsceneEnded => Some(wire::ViewerEvent::CutsceneEnded),

        _ => None,
    }
}

fn cutscene_actor_to_wire(a: crate::state::CutsceneActor) -> wire::CutsceneActor {
    match a {
        crate::state::CutsceneActor::LocalPlayer => wire::CutsceneActor::LocalPlayer,
        crate::state::CutsceneActor::Entity { server_id } => {
            wire::CutsceneActor::Entity { server_id }
        }
    }
}

fn cutscene_cue_to_wire(cue: crate::state::CutsceneCue) -> wire::CutsceneCue {
    use crate::state::CutsceneCue as Cue;
    match cue {
        Cue::ActorMotion {
            actor,
            partner,
            key,
        } => wire::CutsceneCue::ActorMotion {
            actor: cutscene_actor_to_wire(actor),
            partner: cutscene_actor_to_wire(partner),
            key,
        },
        Cue::Scheduler {
            dat_id,
            actor,
            partner,
            tag,
            duration,
        } => wire::CutsceneCue::Scheduler {
            dat_id,
            actor: cutscene_actor_to_wire(actor),
            partner: cutscene_actor_to_wire(partner),
            tag,
            duration,
        },
        Cue::ActorHide { target, hide } => wire::CutsceneCue::ActorHide {
            target: cutscene_actor_to_wire(target),
            hide,
        },
        Cue::CameraLock { lock } => wire::CutsceneCue::CameraLock { lock },
        Cue::Mount {
            target,
            status_event,
            mount_id,
        } => wire::CutsceneCue::Mount {
            target: cutscene_actor_to_wire(target),
            status_event,
            mount_id,
        },
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
    use ffxi_proto::decode::{DoorId, LookData};
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
        LookData::Door { size, door_id } => wire::EntityLook::Door {
            size,
            door_id: door_id.map(DoorId::bytes),
        },
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

// MOUNTTYPE, vendor/server/src/map/entities/baseentity.h. Noble Chocobo
// is a chocobo despite sitting at the far end of the enum — the server routes it
// through ANIMATION_CHOCOBO like the plain one
// (charentity.cpp, CCharEntity::tryStartNextEvent).
const MOUNT_CHOCOBO: u8 = 0;
const MOUNT_NOBLE_CHOCOBO: u8 = 34;

/// The mount an entity is riding, from the pair of fields that only mean
/// something together: the animation byte says *whether*, the mount index says
/// *which*, and the index keeps its last value after a dismount
/// (vendor/server/src/map/packets/char_update.cpp, CCharUpdatePacket::updateWith).
pub fn mount_to_wire(animation: u8, mount_id: u8) -> Option<wire::Mount> {
    if !ffxi_proto::decode::animation::is_mounted(animation) {
        return None;
    }
    Some(match mount_id {
        MOUNT_CHOCOBO | MOUNT_NOBLE_CHOCOBO => wire::Mount::Chocobo {
            // The colour lives in CustomProperties, which 0x037 has no room for
            // and which only a Personal Chocobo ever sets; a rented one is yellow.
            colour: wire::ChocoboColour::default(),
        },
        mount_id => wire::Mount::Other { mount_id },
    })
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
        face_target: e.face_target,
        name_vis: e.name_vis,
        claim_id: e.claim_id,
        speed: e.speed,
        speed_base: e.speed_base,
        look: e.look.map(look_to_wire),
        animation: e.npc_state.map(|s| s.animation).unwrap_or_default(),
        animationsub: e.npc_state.map(|s| s.animationsub).unwrap_or_default(),
        mount: mount_to_wire(
            e.npc_state.map(|s| s.animation).unwrap_or_default(),
            e.mount_id.unwrap_or_default(),
        ),
        status: e.status,
        char_flags: e.char_flags.map(char_flags_to_wire).unwrap_or_default(),
    }
}

pub fn char_flags_to_wire(f: ffxi_proto::decode::CharFlags) -> wire::CharFlags {
    wire::CharFlags {
        monster: f.monster,
        lfg: f.lfg,
        anonymous: f.anonymous,
        yell: f.yell,
        away: f.away,
        play_online: f.play_online,
        linkshell: f.linkshell,
        linkdead: f.linkdead,
        gm_level: f.gm_level,
        bazaar: f.bazaar,
        linkshell_color: f.linkshell_color,
        charm: f.charm,
        gm_icon: f.gm_icon,
        auto_party: f.auto_party,
        trust: f.trust,
        lfg_master: f.lfg_master,
        pet: f.pet,
        allegiance: f.allegiance,
        new_character: f.new_character,
        mentor: f.mentor,
        untargetable: f.untargetable,
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
        spans: c
            .spans
            .iter()
            .map(|s| wire::ChatSpan {
                text: s.text.clone(),
                kind: span_kind_to_wire(s.kind),
            })
            .collect(),
        channel: channel_to_wire(c.channel),
        sender: c.sender.clone(),
        text: c.text.clone(),
        server_ts: c.server_ts,

        local_seq: 0,
    }
}

fn treasure_slot_to_wire(s: &crate::state::TreasurePoolSlot) -> wire::TreasurePoolSlot {
    use crate::state::TreasureEntry as E;
    wire::TreasurePoolSlot {
        slot: s.slot,
        item_id: s.item_id,
        item_name: s.item_name.clone(),
        count: s.count,
        dropper: s.dropper.clone(),
        own_entry: match s.own_entry {
            E::None => wire::TreasureEntry::None,
            E::Passed => wire::TreasureEntry::Passed,
            E::Lotted => wire::TreasureEntry::Lotted,
        },
        own_lot: s.own_lot,
        winner: s.winner.clone(),
        winner_lot: s.winner_lot,
    }
}

fn span_kind_to_wire(k: crate::state::ChatSpanKind) -> wire::ChatSpanKind {
    use crate::state::ChatSpanKind as K;
    match k {
        K::Text => wire::ChatSpanKind::Text,
        K::Item => wire::ChatSpanKind::Item,
        K::KeyItem => wire::ChatSpanKind::KeyItem,
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
        ChatChannel::Emote => wire::ChatChannel::Emote,
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
        party_no: m.party_no,
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
    fn mount_is_read_from_the_animation_byte_not_the_stale_mount_index() {
        use ffxi_proto::decode::animation;

        // LSB leaves Flags6.MountIndex set after a dismount, so the index alone
        // would keep a phantom mount under the player forever.
        assert_eq!(mount_to_wire(animation::NONE, MOUNT_NOBLE_CHOCOBO), None);
        assert_eq!(mount_to_wire(animation::ATTACK, 17), None);

        // Both chocobo ids route to the chocobo model family; everything else
        // carries its MOUNTTYPE through untouched.
        assert!(matches!(
            mount_to_wire(animation::CHOCOBO, MOUNT_CHOCOBO),
            Some(wire::Mount::Chocobo { .. })
        ));
        assert!(matches!(
            mount_to_wire(animation::CHOCOBO, MOUNT_NOBLE_CHOCOBO),
            Some(wire::Mount::Chocobo { .. })
        ));
        assert_eq!(
            mount_to_wire(animation::MOUNT, 17),
            Some(wire::Mount::Other { mount_id: 17 })
        );
    }

    #[test]
    fn snapshot_self_entity_carries_look() {
        const SELF_CHAR_ID: u32 = 7;
        let look = ffxi_proto::decode::LookData::Equipped {
            face: 3,
            race: 3,
            head: 0x011,
            body: 0x022,
            hands: 0x033,
            legs: 0x044,
            feet: 0x055,
            main: 0x066,
            sub: 0x077,
            ranged: 0x088,
        };

        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::Connected {
            account_id: 42,
            char_id: SELF_CHAR_ID,
            character: "Tester".into(),
            zone_id: 100,
        });
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: Entity {
                id: SELF_CHAR_ID,
                act_index: SELF_CHAR_ID as u16,
                kind: EntityKind::Pc,
                name: Some("Tester".into()),
                pos: Vec3::default(),
                heading: 0,
                hp_pct: Some(100),
                bt_target_id: 0,
                face_target: 0,
                name_vis: None,
                claim_id: 0,
                speed: 0,
                speed_base: 0,
                look: None,
                npc_state: None,
                char_flags: None,
                status: 0,
                mount_id: None,
            },
            pos_present: true,
        });
        s.apply_event(&AgentEvent::SelfLookUpdated { look });

        let snap = state_to_snapshot(&s);
        let self_entity = snap
            .entities
            .iter()
            .find(|e| Some(e.id) == snap.self_char_id)
            .expect("self entity present in snapshot");
        assert_eq!(self_entity.look, Some(look_to_wire(look)));
    }

    #[test]
    fn action_started_carries_target_id() {
        for target_id in [Some(0xBEEFu32), None] {
            let mapped = event_to_viewer_event(AgentEvent::ActionStarted {
                actor_id: 0xCAFE,
                action_id: 220,
                action_kind: 4,
                target_id,
                result: None,
                animation: None,
            });
            assert!(matches!(
                mapped,
                Some(wire::ViewerEvent::ActionStarted { target_id: t, .. }) if t == target_id
            ));
        }
    }

    #[test]
    fn action_started_keeps_absent_result_absent() {
        let hit_right = ffxi_proto::melee::MeleeResult {
            resolution: ffxi_proto::melee::ActionResolution::Hit,
            animation: ffxi_proto::melee::AttackAnimation::RightAttack,
        };
        for result in [None, Some(hit_right)] {
            let mapped = event_to_viewer_event(AgentEvent::ActionStarted {
                actor_id: 0xCAFE,
                action_id: 0,
                action_kind: ffxi_proto::melee::CATEGORY_BASIC_ATTACK,
                target_id: Some(0xBEEF),
                result,
                animation: None,
            });
            assert!(matches!(
                mapped,
                Some(wire::ViewerEvent::ActionStarted { result: r, .. })
                    if r == result.map(ffxi_proto::melee::MeleeResult::to_wire)
            ));
        }
    }

    /// Every populated container crosses the wire (sorted by id, real
    /// capacities), not just the main bag — the Mog House storage UI reads them.
    #[test]
    fn containers_project_sorted_with_capacities() {
        use crate::state::{ContainerInfo, ItemSlot};
        let mut s = SessionState::default();
        for (id, capacity, items) in [(4u8, 30u8, 1usize), (0, 30, 2), (1, 60, 0)] {
            let slots = (0..items)
                .map(|i| ItemSlot {
                    index: i as u8 + 1,
                    item_no: 4509,
                    quantity: 3,
                    locked: false,
                    price: 0,
                    charges_remaining: None,
                    next_use_vana_ts: None,
                })
                .collect();
            s.inventory
                .containers
                .insert(id, ContainerInfo { capacity, slots });
        }
        // Capacity 0 and empty = never granted; stays off the wire.
        s.inventory.containers.insert(
            9,
            ContainerInfo {
                capacity: 0,
                slots: Vec::new(),
            },
        );

        let out = project_containers(&s);
        assert_eq!(
            out.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![0, 1, 4],
            "sorted by container id, capacity-0 bag dropped"
        );
        assert_eq!(out[1].capacity, 60);
        assert_eq!(out[0].items.len(), 2);
        assert_eq!(out[2].items[0].container, 4, "items tag their source bag");
    }

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
    fn check_result_crosses_the_wire_boundary() {
        let mut s = SessionState::default();
        assert!(state_to_snapshot(&s).check.is_none());

        s.apply_event(&AgentEvent::CheckEquipReceived {
            target_id: 0xCAFE,
            act_index: 0x123,
            items: vec![(0, 17440), (15, 13465)],
        });
        s.apply_event(&AgentEvent::CheckGeneralReceived {
            target_id: 0xCAFE,
            act_index: 0x123,
            main_job: 1,
            sub_job: 13,
            main_job_lv: 75,
            sub_job_lv: 37,
            master_lv: 0,
            linkshell: "Kuluu".into(),
        });
        let snap = state_to_snapshot(&s);
        let c = snap.check.expect("check result on the wire");
        assert_eq!(c.target_id, 0xCAFE);
        assert_eq!(c.equipped[0], Some(17440));
        assert_eq!(c.equipped[15], Some(13465));
        assert_eq!(c.equipped[1], None);
        assert_eq!((c.main_job, c.main_job_lv), (1, 75));
        assert_eq!((c.sub_job, c.sub_job_lv), (13, 37));
        assert_eq!(c.linkshell, "Kuluu");
    }

    #[test]
    fn check_message_and_bazaar_cross_the_wire_boundary() {
        let mut s = SessionState::default();
        let snap = state_to_snapshot(&s);
        assert!(snap.check_message.is_none());
        assert!(snap.bazaar.is_none());

        s.apply_event(&AgentEvent::CheckMessageReceived {
            name: "Aliya".into(),
            message: "Sneak oil 2k".into(),
        });
        s.apply_event(&AgentEvent::BazaarOpened {
            seller_id: 0xCAFE,
            seller_index: 0x123,
            seller_name: "Aliya".into(),
        });
        s.apply_event(&AgentEvent::BazaarItemReceived {
            index: 3,
            item_no: 4096,
            quantity: 5,
            price: 1000,
            tax_rate: 500,
        });

        let snap = state_to_snapshot(&s);
        let m = snap.check_message.expect("inspect message on the wire");
        assert_eq!(
            (m.name.as_str(), m.message.as_str()),
            ("Aliya", "Sneak oil 2k")
        );

        let b = snap.bazaar.expect("bazaar on the wire");
        assert_eq!(b.seller_id, 0xCAFE);
        assert_eq!(b.seller_name, "Aliya");
        assert_eq!(b.items.len(), 1);
        let row = b.items[0];
        assert_eq!(
            (row.index, row.item_no, row.quantity, row.price),
            (3, 4096, 5, 1000)
        );
        // 5% zone tax, applied by the consumer rather than stored pre-taxed.
        assert_eq!(row.total_price(2), 2100);
    }

    #[test]
    fn auction_sales_slot_counts_agree() {
        assert_eq!(wire::AH_SALES_SLOT_COUNT, crate::state::AUCTION_SLOTS);
    }

    #[test]
    fn auction_state_crosses_the_wire_boundary() {
        use crate::state::{
            AhCatalogView, AhFeeQuote, AhHistoryView, AhListingView, AhSaleStatus, AhSaleView,
            AuctionBusy, AuctionState, AUCTION_SLOTS,
        };

        let mut s = SessionState::default();
        assert_eq!(state_to_snapshot(&s).auction, wire::AuctionUi::default());

        let mut sales_status: [Option<AhSaleStatus>; AUCTION_SLOTS] = Default::default();
        sales_status[6] = Some(AhSaleStatus {
            stat: 1,
            item_no: 4570,
            quantity: 12,
            price: 1_180,
            timestamp: 1_700_000_100,
        });
        s.auction = AuctionState {
            open: true,
            browse: Some(AhCatalogView {
                category: 7,
                total: 2,
                listings: vec![
                    AhListingView {
                        item_id: 4570,
                        singles_for_sale: 14,
                        stacks_for_sale: Some(3),
                    },
                    AhListingView {
                        item_id: 17440,
                        singles_for_sale: 0,
                        stacks_for_sale: None,
                    },
                ],
            }),
            history: Some(AhHistoryView {
                item_id: 4570,
                stack: true,
                open_listings: 3,
                category: 7,
                sales: vec![AhSaleView {
                    price: 1_180,
                    sell_date: 1_700_000_000,
                    seller: "Aliya".into(),
                    buyer: "Sylvie".into(),
                }],
            }),
            sales_status,
            fee_quote: Some(AhFeeQuote {
                fee: 9,
                inventory_slot: 5,
                item_no: 4570,
                stack: true,
                asking_price: 1_180,
            }),
            busy: Some(AuctionBusy::PlacingBid),
        };

        let a = state_to_snapshot(&s).auction;
        assert!(a.open);
        assert_eq!(a.busy, Some(wire::AuctionBusy::PlacingBid));

        let b = a.browse.expect("catalog on the wire");
        assert_eq!((b.category, b.total), (7, 2));
        assert_eq!(b.listings.len(), 2);
        assert_eq!(
            (
                b.listings[0].item_id,
                b.listings[0].singles_for_sale,
                b.listings[0].stacks_for_sale
            ),
            (4570, 14, Some(3))
        );
        assert_eq!(
            b.listings[1].stacks_for_sale, None,
            "unstackable stays None"
        );

        let h = a.history.expect("history on the wire");
        assert_eq!(
            (h.item_id, h.stack, h.open_listings, h.category),
            (4570, true, 3, 7)
        );
        assert_eq!(h.sales.len(), 1);
        assert_eq!(
            (
                h.sales[0].price,
                h.sales[0].seller.as_str(),
                h.sales[0].buyer.as_str()
            ),
            (1_180, "Aliya", "Sylvie")
        );

        assert!(a.sales_status[..6].iter().all(Option::is_none));
        let slot = a.sales_status[6].expect("sales slot on the wire");
        assert_eq!(
            (
                slot.stat,
                slot.item_no,
                slot.quantity,
                slot.price,
                slot.timestamp
            ),
            (1, 4570, 12, 1_180, 1_700_000_100)
        );

        let q = a.fee_quote.expect("fee quote on the wire");
        assert_eq!(
            (q.fee, q.inventory_slot, q.item_no, q.stack, q.asking_price),
            (9, 5, 4570, true, 1_180)
        );
    }

    #[test]
    fn myroom_crosses_the_wire_boundary() {
        let mut s = SessionState::default();
        assert!(state_to_snapshot(&s).myroom.is_none());

        s.myroom = Some(crate::state::MyRoomInfo {
            model: 615,
            sub_map: 2,
            exit_bit: 1,
        });
        let snap = state_to_snapshot(&s);
        // exit_bit stays session-side: the exit menu is DialogState-driven.
        assert_eq!(
            snap.myroom,
            Some(wire::MyRoom {
                model: 615,
                sub_map: 2
            })
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
            charges_remaining: None,
            next_use_vana_ts: None,
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

    /// Bytecode for one `LOADEVENTSCHEDULER2` (0x45) carrying the screen
    /// fade-out tag, followed by END. The file operand has to be a reference
    /// (bit 0x8000) because `getworkofs` reads a bare small literal out of the
    /// work array, not as an immediate.
    fn fade_out_block(event_id: u16) -> ffxi_dat::event_dat::EventBlock {
        const OP_LOADEVENTSCHEDULER2: u8 = 0x45;
        const REFERENCE_FLAG: u16 = 0x8000;
        /// 30704 + 200 == `ffxi_event::SCHEDULER_FADE_DAT_ID`, the pair every
        /// authored fade call site resolves to.
        const FADE_WORK_OPERAND: u32 = 200;

        let width = ffxi_event::opcode_meta::OPCODE_META[OP_LOADEVENTSCHEDULER2 as usize].size;
        let mut code = vec![0u8; width as usize];
        code[0] = OP_LOADEVENTSCHEDULER2;
        code[1..3].copy_from_slice(&REFERENCE_FLAG.to_le_bytes());
        code[3..7].copy_from_slice(&ffxi_event::ActorLookup::EVENT_ENTITY.0.to_le_bytes());
        code[7..11].copy_from_slice(&ffxi_event::ActorLookup::LOCAL_PLAYER.0.to_le_bytes());
        code[11..15].copy_from_slice(&ffxi_event::SCHEDULER_TAG_FADE_OUT);
        // Trailing zeros are OP_END, so the script terminates right after.
        code.push(0);

        ffxi_dat::event_dat::EventBlock {
            actor: 0,
            event_ids: vec![event_id],
            event_offsets: vec![0],
            references: vec![FADE_WORK_OPERAND],
            event_data: code,
        }
    }

    /// A one-entry dialog DAT: the VM needs a string table to run, but a
    /// choreography-only script never reads it.
    fn empty_strings() -> ffxi_dat::dmsg::StringDat {
        const TEXT_XOR: u8 = 0x80;
        const OFFSET_XOR: u32 = 0x8080_8080;
        const MAGIC_BASE: u32 = 0x1000_0000;
        let entry: &[u8] = b" ";
        let table_size = 4u32;
        let mut buf = Vec::new();
        buf.extend_from_slice(&(MAGIC_BASE + table_size + entry.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(table_size ^ OFFSET_XOR).to_le_bytes());
        buf.extend(entry.iter().map(|b| b ^ TEXT_XOR));
        ffxi_dat::dmsg::StringDat::parse(&buf).expect("synthetic dialog DAT parses")
    }

    /// The full channel: a cue the VM emits reaches the renderer as a
    /// `ViewerEvent`, with the FourCC the emitter exported still matching what
    /// a consumer would match on. Re-typing `b"fdo0"` here would defeat the
    /// point — the guard is that both sides name `SCHEDULER_TAG_FADE_OUT`.
    #[test]
    fn a_fade_cue_from_the_vm_arrives_as_a_viewer_event() {
        use crate::event_dialog::{resolve_cue, CutsceneScope, EventSessionExit};

        const EVENT_ID: u16 = 599;
        const NPC_ID: u32 = 0x010E_602F;

        let block = fade_out_block(EVENT_ID);
        let strings = empty_strings();
        let mut runner = ffxi_event::DialogRunner::start(&block, EVENT_ID, 0, Vec::new())
            .expect("block authors the event");
        let step = runner.advance(None, &strings);
        assert!(
            matches!(step, ffxi_event::DialogStep::Ended { .. }),
            "choreography-only script runs to completion: {step:?}"
        );

        let (tx, mut rx) = tokio::sync::broadcast::channel(32);
        let mut scope = CutsceneScope::default();
        scope.start(crate::event_dialog::agent_event_id(NPC_ID, EVENT_ID), &tx);
        for cue in runner.take_cues() {
            scope.push(resolve_cue(cue, NPC_ID), &tx);
        }
        scope.end(EventSessionExit::ScriptEnded, &tx);

        let mut viewer = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            viewer.extend(event_to_viewer_event(ev));
        }

        assert!(
            matches!(
                viewer.first(),
                Some(wire::ViewerEvent::CutsceneStarted { .. })
            ),
            "the session start opens the channel: {viewer:?}"
        );
        assert!(
            matches!(viewer.last(), Some(wire::ViewerEvent::CutsceneEnded)),
            "and the session end closes it: {viewer:?}"
        );
        let fade = viewer.iter().find(|ev| {
            matches!(
                ev,
                wire::ViewerEvent::Cutscene {
                    cue: wire::CutsceneCue::Scheduler { .. }
                }
            )
        });
        let Some(wire::ViewerEvent::Cutscene { cue }) = fade else {
            panic!("no scheduler cue crossed the wire: {viewer:?}");
        };
        assert_eq!(
            *cue,
            wire::CutsceneCue::Scheduler {
                dat_id: ffxi_event::SCHEDULER_FADE_DAT_ID,
                actor: wire::CutsceneActor::Entity { server_id: NPC_ID },
                partner: wire::CutsceneActor::LocalPlayer,
                // The emitter's const, never a re-typed b"fdo0": that identity
                // is the whole contract this test pins.
                tag: ffxi_event::SCHEDULER_TAG_FADE_OUT,
                duration: ffxi_event::SCHEDULER_DURATION_FROM_DAT,
            }
        );
    }
}
