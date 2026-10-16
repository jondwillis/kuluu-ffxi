use super::*;

// vendor/server/src/map/enums/packet_c2s.h GP_CLI_COMMAND_*
pub(crate) const ITEM_USE: u16 = 0x037;
pub(crate) const CHAT_NAME: u16 = 0x0B6;
pub(crate) const EQUIP_INSPECT: u16 = 0x0DD;
pub(crate) const CAMP: u16 = 0x0E8;
pub(crate) const BUFFCANCEL: u16 = 0x0F1;

// GP_CLI_COMMAND_BUFFCANCEL, vendor/server/src/map/packets/c2s/0x0f1_buffcancel.h:
// BuffNo u16 (the status icon id), padding u16. The server runs
// DelStatusEffectsByIcon(BuffNo) and blocks the packet only while InEvent
// (0x0f1_buffcancel.cpp); it does NOT re-check cancelability, so the caller
// gates on ffxi_vocab::status_effects::is_cancelable.
pub fn build_subpacket_buffcancel(sync: u16, buff_no: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(BUFFCANCEL, 2, sync));
    buf[4..6].copy_from_slice(&buff_no.to_le_bytes());
    buf
}

pub fn build_subpacket_shop_buy(sync: u16, qty: u32, shop_no: u16, shop_index: u8) -> Vec<u8> {
    let mut buf = vec![0u8; 16];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::SHOP_BUY,
        4,
        sync,
    ));
    buf[4..8].copy_from_slice(&qty.to_le_bytes());
    buf[8..10].copy_from_slice(&shop_no.to_le_bytes());
    buf[10..12].copy_from_slice(&(shop_index as u16).to_le_bytes());
    buf[12] = 0;
    buf
}

// GP_CLI_COMMAND_SHOP_SELL_REQ, vendor/server/src/map/packets/c2s/0x084_shop_sell_req.h:
// ItemNum u32, ItemNo u16, ItemIndex u8, padding u8. The server appraises the item in
// that LOC_INVENTORY slot, clamps ItemNum to the held quantity, parks it in the shop
// trade container, and answers with s2c 0x03D SHOP_SELL (0x084_shop_sell_req.cpp).
pub fn build_subpacket_shop_sell_req(sync: u16, qty: u32, item_no: u16, item_index: u8) -> Vec<u8> {
    let mut buf = vec![0u8; 12];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::SHOP_SELL_REQ,
        3,
        sync,
    ));
    buf[4..8].copy_from_slice(&qty.to_le_bytes());
    buf[8..10].copy_from_slice(&item_no.to_le_bytes());
    buf[10] = item_index;
    buf
}

// GP_CLI_COMMAND_SHOP_SELL_SET, vendor/server/src/map/packets/c2s/0x085_shop_sell_set.h:
// SellFlag u16, padding u16. The server validator rejects the packet unless SellFlag
// equals 1 and a SHOP_SELL_REQ preceded it (0x085_shop_sell_set.cpp validate).
pub fn build_subpacket_shop_sell_set(sync: u16) -> Vec<u8> {
    const SELL_FLAG_CONFIRM: u16 = 1;
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::SHOP_SELL_SET,
        2,
        sync,
    ));
    buf[4..6].copy_from_slice(&SELL_FLAG_CONFIRM.to_le_bytes());
    buf
}

/// `Err` for what vendor/server/src/map/packets/c2s/0x01a_action.cpp
/// GP_CLI_COMMAND_ACTION::validate refuses under BlockedState::InEvent.
pub fn build_subpacket_action(
    sync: u16,
    unique_no: u32,
    act_index: u16,
    kind: &crate::state::ActionKind,
    in_event: bool,
) -> Result<Vec<u8>, &'static str> {
    if in_event && kind.blocked_in_event() {
        return Err("busy with an event");
    }
    let mut buf = vec![0u8; 28];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::ACTION,
        7,
        sync,
    ));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());
    buf[8..10].copy_from_slice(&act_index.to_le_bytes());
    buf[10..12].copy_from_slice(&kind.action_id().to_le_bytes());
    let mut action_buf = [0u8; 16];
    kind.fill_action_buf(&mut action_buf);
    buf[12..28].copy_from_slice(&action_buf);
    Ok(buf)
}

pub const C2S_ACTION_LOG_TARGET: &str = "c2s_action";

/// Call only after the transport reports the datagram written, so the line is
/// evidence the packet left the client, not merely that it was built.
pub fn log_action_sent(unique_no: u32, act_index: u16, kind: &crate::state::ActionKind) {
    tracing::debug!(
        target: C2S_ACTION_LOG_TARGET,
        opcode = ffxi_proto::map::c2s::ACTION,
        action_id = kind.action_id(),
        kind = ?kind,
        unique_no,
        act_index,
        "sent 0x01A ACTION"
    );
}

// c2s 0x05D GP_CLI_COMMAND_MOTION: UniqueNo u32 @4, ActIndex u16 @8, Number u8
// @10 (emote id), Mode u8 @11, Param u16 @12, pad u16 @14
// (vendor/server/src/map/packets/c2s/0x05d_motion.h GP_CLI_COMMAND_MOTION). Note the c2s Mode
// byte precedes Param, unlike the s2c 0x05A layout.
pub fn build_subpacket_motion(
    sync: u16,
    unique_no: u32,
    act_index: u16,
    number: u8,
    mode: u8,
    param: u16,
) -> Vec<u8> {
    let mut buf = vec![0u8; 16];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::MOTION,
        4,
        sync,
    ));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());
    buf[8..10].copy_from_slice(&act_index.to_le_bytes());
    buf[10] = number;
    buf[11] = mode;
    buf[12..14].copy_from_slice(&param.to_le_bytes());
    buf
}

// c2s 0x119 GP_CLI_COMMAND_EMOTE_LIST — header only
// (vendor/server/src/map/packets/c2s/0x119_emote_list.h declares no payload).
pub fn build_subpacket_emote_list_req(sync: u16) -> Vec<u8> {
    build_subpacket_header(ffxi_proto::map::c2s::EMOTE_LIST, 1, sync).to_vec()
}

// c2s 0x10F/0x115 GP_CLI_COMMAND_CURRENCIES_1/_2 — header only: the C++ structs are bare
// `GP_CLI_PACKET(...)` with no fields (vendor/server/src/map/packets/c2s/0x10f_currencies_1.h,
// 0x115_currencies_2.h), and retail builds them exactly so before committing one per
// Currencies-window open, caching nothing between opens (FFXiMain.dll retail-2026-09
// `.text 0xFD520`/`.text 0xFD550`; bead kuluu-o9qt7.35).
pub fn build_subpacket_currencies(sync: u16, mode_two: bool) -> Vec<u8> {
    let opcode = if mode_two {
        ffxi_proto::map::c2s::CURRENCIES_2
    } else {
        ffxi_proto::map::c2s::CURRENCIES_1
    };
    build_subpacket_header(opcode, 1, sync).to_vec()
}

/// Every Unity / alter ego point send but the bare 0x0C2 request is one 8-byte packet: header
/// plus one 4-byte body word (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).
const UNITY_FP_PACKET_WORDS: u16 = 2;
const UNITY_FP_PACKET_LEN: usize = 8;
const UNITY_FP_BODY: usize = 4;

// c2s 0x116 GP_CLI_COMMAND_UNITY_MENU: Kind u32, the ranking result set 0 or 1
// (vendor/server/src/map/packets/c2s/0x116_unity_menu.h; the validator ranges it 0..=1).
pub fn build_subpacket_unity_menu(sync: u16, kind: u32) -> Vec<u8> {
    let mut buf = vec![0u8; UNITY_FP_PACKET_LEN];
    buf[..UNITY_FP_BODY].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::UNITY_MENU,
        UNITY_FP_PACKET_WORDS,
        sync,
    ));
    buf[UNITY_FP_BODY..].copy_from_slice(&kind.to_le_bytes());
    buf
}

// c2s 0x117 GP_CLI_COMMAND_UNITY_QUEST: Kind u32, which LSB's validator requires to be 0
// (vendor/server/src/map/packets/c2s/0x117_unity_quest.cpp).
pub fn build_subpacket_unity_quest(sync: u16) -> Vec<u8> {
    let mut buf = vec![0u8; UNITY_FP_PACKET_LEN];
    buf[..UNITY_FP_BODY].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::UNITY_QUEST,
        UNITY_FP_PACKET_WORDS,
        sync,
    ));
    buf
}

// c2s 0x118 GP_CLI_COMMAND_UNITY_TOGGLE: Mode u8 + padding00[3]
// (vendor/server/src/map/packets/c2s/0x118_unity_toggle.h).
pub fn build_subpacket_unity_toggle(sync: u16, mode: u8) -> Vec<u8> {
    let mut buf = vec![0u8; UNITY_FP_PACKET_LEN];
    buf[..UNITY_FP_BODY].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::UNITY_TOGGLE,
        UNITY_FP_PACKET_WORDS,
        sync,
    ));
    buf[UNITY_FP_BODY] = mode;
    buf
}

// c2s 0x0C1 GP_CLI_COMMAND_ALTER_EGO_POINTS: CategoryIndex u16 + padding00 u16
// (vendor/server/src/map/packets/c2s/0x0c1_alter_ego_points.h).
pub fn build_subpacket_alter_ego_points(sync: u16, category_index: u16) -> Vec<u8> {
    let mut buf = vec![0u8; UNITY_FP_PACKET_LEN];
    buf[..UNITY_FP_BODY].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::ALTER_EGO_POINTS,
        UNITY_FP_PACKET_WORDS,
        sync,
    ));
    buf[UNITY_FP_BODY..UNITY_FP_BODY + 2].copy_from_slice(&category_index.to_le_bytes());
    buf
}

// c2s 0x0C2: the bare 4-byte header the record names as the alter ego points request
// (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md); LSB has no
// handler for it.
pub fn build_subpacket_alter_ego_points_request(sync: u16) -> Vec<u8> {
    build_subpacket_header(
        ffxi_proto::map::c2s_unnamed::ALTER_EGO_POINTS_REQUEST,
        1,
        sync,
    )
    .to_vec()
}

const MERITS_PACKET_WORDS: u16 = 3;
const MERITS_PACKET_LEN: usize = 12;
const MERITS_KIND_AT: usize = 4;
const MERITS_PARAM1_AT: usize = 5;
const MERITS_PARAM2_AT: usize = 6;

// c2s 0x0BE GP_CLI_COMMAND_MERITS: Kind u8, Param1 u8, Param2 u16, Param3 u32
// (vendor/server/src/map/packets/c2s/0x0be_merits.h). The merit windows always send Param3 0
// (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md).
pub fn build_subpacket_merits(sync: u16, kind: u8, param1: u8, param2: u16) -> Vec<u8> {
    let mut buf = vec![0u8; MERITS_PACKET_LEN];
    buf[..MERITS_KIND_AT].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::MERITS,
        MERITS_PACKET_WORDS,
        sync,
    ));
    buf[MERITS_KIND_AT] = kind;
    buf[MERITS_PARAM1_AT] = param1;
    buf[MERITS_PARAM2_AT..MERITS_PARAM2_AT + 2].copy_from_slice(&param2.to_le_bytes());
    buf
}

// c2s 0x0BF GP_CLI_COMMAND_JOB_POINTS_SPEND: Index u16 (the job point type id) + padding00 u16
// (vendor/server/src/map/packets/c2s/0x0bf_job_points_spend.h).
pub fn build_subpacket_job_points_spend(sync: u16, type_id: u16) -> Vec<u8> {
    const PACKET_WORDS: u16 = 2;
    const PACKET_LEN: usize = 8;
    const INDEX_AT: usize = 4;
    let mut buf = vec![0u8; PACKET_LEN];
    buf[..INDEX_AT].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::JOB_POINTS_SPEND,
        PACKET_WORDS,
        sync,
    ));
    buf[INDEX_AT..INDEX_AT + 2].copy_from_slice(&type_id.to_le_bytes());
    buf
}

// c2s 0x0C0 GP_CLI_COMMAND_JOB_POINTS_REQ: the bare header
// (vendor/server/src/map/packets/c2s/0x0c0_job_points_req.h).
pub fn build_subpacket_job_points_req(sync: u16) -> Vec<u8> {
    build_subpacket_header(ffxi_proto::map::c2s::JOB_POINTS_REQ, 1, sync).to_vec()
}

/// The packet a merit or job point window command sends: ChangeMode {2, 0 EXP or 1 Limit
/// Points, 0, 0}, EditMode {3, 1 raise or 0 lower, merit index, 0}, a job point spend or the job
/// point request (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md).
pub fn merit_command_payload(command: &AgentCommand, sync: u16) -> Option<Vec<u8>> {
    use ffxi_proto::map::merits_wire::{kind, param1};
    let raise_or = |raise: bool| if raise { param1::RAISE } else { param1::LOWER };
    Some(match *command {
        AgentCommand::SetMeritMode { limit_points } => {
            build_subpacket_merits(sync, kind::CHANGE_MODE, raise_or(limit_points), 0)
        }
        AgentCommand::EditMerit { index, raise } => {
            build_subpacket_merits(sync, kind::EDIT_MODE, raise_or(raise), index)
        }
        AgentCommand::SpendJobPoint { type_id } => build_subpacket_job_points_spend(sync, type_id),
        AgentCommand::RequestJobPoints => build_subpacket_job_points_req(sync),
        _ => return None,
    })
}

// c2s 0x0F4 GP_CLI_COMMAND_TRACKING_LIST: uint32 SendFlg (must be 1). Requests
// the wide-scan list; the server frames the reply with 0x0F6 ListStart/ListEnd.
// vendor/server/src/map/packets/c2s/0x0f4_tracking_list.h.
pub fn build_subpacket_tracking_list(sync: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::TRACKING_LIST,
        2,
        sync,
    ));
    buf[4..8].copy_from_slice(&ffxi_proto::map::tracking::SEND_FLG_REQUEST.to_le_bytes());
    buf
}

// c2s 0x0F5 GP_CLI_COMMAND_TRACKING_START: uint32 ActIndex. Begins tracking one
// entity; the server then streams 0x0F5 position updates.
// vendor/server/src/map/packets/c2s/0x0f5_tracking_start.h.
pub fn build_subpacket_tracking_start(sync: u16, act_index: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::TRACKING_START,
        2,
        sync,
    ));
    buf[4..8].copy_from_slice(&u32::from(act_index).to_le_bytes());
    buf
}

// c2s 0x0F6 GP_CLI_COMMAND_TRACKING_END: uint32 padding (Dammy). Stops tracking.
// vendor/server/src/map/packets/c2s/0x0f6_tracking_end.h.
pub fn build_subpacket_tracking_end(sync: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::TRACKING_END,
        2,
        sync,
    ));
    buf
}

/// c2s 0x110 GP_CLI_COMMAND_FISHING_2 — the mini-game request the client streams while
/// fishing (check-hook, end-game, release, timeout). `mode`/`para`/`para2` follow the
/// LSB validator: vendor/server/src/map/packets/c2s/0x110_fishing_2.{h,cpp}.
pub fn build_subpacket_fishing(
    sync: u16,
    unique_no: u32,
    act_index: u16,
    mode: crate::state::FishingMode,
    para: i32,
    para2: i32,
) -> Vec<u8> {
    let mut buf = vec![0u8; 20];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::FISHING_2,
        5,
        sync,
    ));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());
    buf[8..12].copy_from_slice(&para.to_le_bytes());
    buf[12..14].copy_from_slice(&act_index.to_le_bytes());
    buf[14] = mode as u8;
    buf[16..20].copy_from_slice(&para2.to_le_bytes());
    buf
}

pub fn build_subpacket_equip_inspect(
    sync: u16,
    unique_no: u32,
    act_index: u16,
    kind: u8,
) -> Vec<u8> {
    let mut buf = vec![0u8; 16];
    buf[0..4].copy_from_slice(&build_subpacket_header(EQUIP_INSPECT, 4, sync));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());

    buf[8..12].copy_from_slice(&(act_index as u32).to_le_bytes());
    buf[12] = kind;

    buf
}

// GP_CLI_COMMAND_BAZAAR_LIST, vendor/server/src/map/packets/c2s/0x105_bazaar_list.h:
// UniqueNo u32, ActIndex u16, padding u16. The server rejects it while we still
// hold a BazaarID, so leave the previous bazaar first (0x105_bazaar_list.cpp validate).
pub fn build_subpacket_bazaar_list(sync: u16, unique_no: u32, act_index: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 12];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::BAZAAR_LIST,
        3,
        sync,
    ));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());
    buf[8..10].copy_from_slice(&act_index.to_le_bytes());
    buf
}

// GP_CLI_COMMAND_BAZAAR_BUY, vendor/server/src/map/packets/c2s/0x106_bazaar_buy.h:
// BazaarItemIndex u8, padding u8[3], BuyNum u32. `index` is the seller-side
// LOC_INVENTORY slot from the s2c 0x105 row.
pub fn build_subpacket_bazaar_buy(sync: u16, index: u8, quantity: u32) -> Vec<u8> {
    let mut buf = vec![0u8; 12];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::BAZAAR_BUY,
        3,
        sync,
    ));
    buf[4] = index;
    buf[8..12].copy_from_slice(&quantity.to_le_bytes());
    buf
}

// GP_CLI_COMMAND_BAZAAR_EXIT, vendor/server/src/map/packets/c2s/0x104_bazaar_exit.h:
// header only. Clears our BazaarID server-side and notifies the seller.
pub fn build_subpacket_bazaar_exit(sync: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 4];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::BAZAAR_EXIT,
        1,
        sync,
    ));
    buf
}

// GP_CLI_COMMAND_CONFIG, vendor/server/src/map/packets/c2s/0x0dc_config.h:
// four SAVE_CONF mask bytes, two reserved uint32s, SetFlg, padding — the body
// law lives in ffxi_proto::config::build_config_change; only the named flag's
// bit is set and LSB applies guarded bits alone (vault main_menu.md §42).
pub fn build_subpacket_config(sync: u16, flag: ffxi_proto::config::ConfigBit, on: bool) -> Vec<u8> {
    let body = ffxi_proto::config::build_config_change(flag, on);
    let mut buf = vec![0u8; 20];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::CONFIG,
        5,
        sync,
    ));
    buf[4..20].copy_from_slice(&body);
    buf
}

// c2s **0x088** — the `conf12wi` store-0xBF pair announce. No LSB file exists
// for the id (vendor/server/src/map/packets/c2s/ carries no 0x088), so nothing
// folds back from it; kuluu emits the frame because retail does, body law in
// ffxi_proto::config::build_conf_pair_announce (writer FFXiMain.dll retail-2026-09
// RVA 0xE5520; vault main_menu.md §59 addendum 16).
pub fn build_subpacket_conf_bf_announce(sync: u16, mode: u8) -> Vec<u8> {
    let body = ffxi_proto::config::build_conf_pair_announce(mode);
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::config::C2S_CONF_PAIR_ANNOUNCE_ID,
        2,
        sync,
    ));
    buf[4..8].copy_from_slice(&body);
    buf
}

// GP_CLI_COMMAND_REQSUBMAPNUM, vendor/server/src/map/packets/c2s/
// 0x0eb_reqsubmapnum.h: header only. The event VM's 0xA6 case 0 sends it to
// fetch the event's sub-map number; the server answers with s2c 0x10E when
// the char is npc-locked and with nothing otherwise
// (0x0eb_reqsubmapnum.cpp process).
pub fn build_subpacket_reqsubmapnum(sync: u16) -> Vec<u8> {
    build_subpacket_header(ffxi_proto::map::c2s::REQSUBMAPNUM, 1, sync).to_vec()
}

pub fn build_subpacket_reqlogout(sync: u16, mode: u16, kind: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::REQ_LOGOUT,
        2,
        sync,
    ));
    buf[4..6].copy_from_slice(&mode.to_le_bytes());
    buf[6..8].copy_from_slice(&kind.to_le_bytes());
    buf
}

pub fn build_subpacket_camp(sync: u16, mode: HealMode) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(CAMP, 2, sync));
    buf[4..8].copy_from_slice(&mode.as_u32().to_le_bytes());
    buf
}

pub fn build_subpacket_item_use(
    sync: u16,
    unique_no: u32,
    act_index: u16,
    category: u8,
    slot: u8,
) -> Vec<u8> {
    let mut buf = vec![0u8; 20];
    buf[0..4].copy_from_slice(&build_subpacket_header(ITEM_USE, 5, sync));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());

    buf[12..14].copy_from_slice(&act_index.to_le_bytes());
    buf[14] = slot;

    buf[16..20].copy_from_slice(&(category as u32).to_le_bytes());
    buf
}

pub fn build_subpacket_equip_set(
    sync: u16,
    container_index: u8,
    equip_slot: u8,
    container: u8,
) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::EQUIP_SET,
        2,
        sync,
    ));
    buf[4] = container_index;
    buf[5] = equip_slot;
    buf[6] = container;

    buf
}

/// GP_CLI_COMMAND_ITEM_STACK, vendor/server/src/map/packets/c2s/0x03a_item_stack.h:
/// `uint32_t Category` (container id) after the 4-byte subpacket header, so 8 bytes
/// total (size_words = 2). The server consolidates same-id partial stacks.
/// `Err` for what 0x03a_item_stack.cpp GP_CLI_COMMAND_ITEM_STACK::validate
/// refuses: blockedBy InEvent, and a Category
/// PacketValidator::isValidContainer rejects.
pub fn build_subpacket_item_stack(
    sync: u16,
    container: u8,
    in_event: bool,
) -> Result<Vec<u8>, &'static str> {
    if in_event {
        return Err("busy with an event");
    }
    if !ffxi_proto::map::container::is_valid(container) {
        return Err("not a valid container");
    }
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::ITEM_STACK,
        2,
        sync,
    ));
    buf[4..8].copy_from_slice(&(container as u32).to_le_bytes());

    Ok(buf)
}

// GP_CLI_COMMAND_TROPHY_ENTRY / GP_CLI_COMMAND_TROPHY_ABSENCE,
// vendor/server/src/map/packets/c2s/0x041_trophy_entry.h and
// 0x042_trophy_absence.h: a single TrophyItemIndex after the header, the rest
// padding, so 8 bytes total (size_words = 2). The server rolls the lot value
// itself and ignores repeat lots/passes on a slot the player already acted on
// (0x041_trophy_entry.cpp / 0x042_trophy_absence.cpp process).
pub fn build_subpacket_trophy_lot(sync: u16, slot: u8) -> Vec<u8> {
    build_subpacket_trophy(ffxi_proto::map::c2s::TROPHY_ENTRY, sync, slot)
}

pub fn build_subpacket_trophy_pass(sync: u16, slot: u8) -> Vec<u8> {
    build_subpacket_trophy(ffxi_proto::map::c2s::TROPHY_ABSENCE, sync, slot)
}

fn build_subpacket_trophy(opcode: u16, sync: u16, slot: u8) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(opcode, 2, sync));
    buf[4] = slot;
    buf
}

pub fn build_subpacket_item_move(
    sync: u16,
    quantity: u32,
    from_container: u8,
    to_container: u8,
    from_slot: u8,
    to_slot: Option<u8>,
) -> Vec<u8> {
    let mut buf = vec![0u8; 12];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::ITEM_MOVE,
        3,
        sync,
    ));
    buf[4..8].copy_from_slice(&quantity.to_le_bytes());
    buf[8] = from_container;
    buf[9] = to_container;
    buf[10] = from_slot;
    buf[11] = to_slot.unwrap_or(ITEM_MOVE_AUTO_SLOT);
    buf
}

// GP_CLI_COMMAND_ITEM_DUMP, vendor/server/src/map/packets/c2s/0x028_item_dump.h:
// ItemNum u32 @4, Category u8 @8, ItemIndex u8 @9; 12 bytes because
// packet_system.cpp packetSizeRange rounds sizeof up to four.
pub fn build_subpacket_item_dump(sync: u16, quantity: u32, container: u8, index: u8) -> Vec<u8> {
    let mut buf = vec![0u8; 12];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::ITEM_DUMP,
        3,
        sync,
    ));
    buf[4..8].copy_from_slice(&quantity.to_le_bytes());
    buf[8] = container;
    buf[9] = index;
    buf
}

// GP_CLI_COMMAND_PBX, vendor/server/src/map/packets/c2s/0x04d_pbx.h: Command u8
// @4, BoxNo i8 @5, PostWorkNo i8 @6, ItemWorkNo i8 @7, ItemStacks i32 @8,
// Result/ResParam1-3 i8 @12-15 (the validator requires all four zero c2s),
// TargetName[16] @16 — 32 bytes (size_words = 8). Per-command field defaults
// mirror the LSB PacketValidator (0x04d_pbx.cpp validate): unused numeric
// fields are -1; Recv's ItemWorkNo must be 1; Set/Send/Cancel are pinned to the
// Outgoing box, Recv/Accept/Reject to Incoming, Query/Confirm/*Open/Close to
// BoxNo None.
pub fn build_subpacket_pbx(sync: u16, op: &crate::state::DeliveryBoxOp) -> Vec<u8> {
    use crate::state::DeliveryBoxOp as Op;
    use ffxi_proto::map::pbx::{boxno, command};
    type Fields<'a> = (u8, i8, i8, i8, i32, Option<&'a str>);
    let (cmd, box_no, post_work_no, item_work_no, item_stacks, name): Fields = match op {
        Op::Work { box_no } => (command::WORK, box_no.wire(), -1, -1, -1, None),
        Op::Set {
            slot,
            inventory_slot,
            quantity,
            recipient,
        } => (
            command::SET,
            boxno::OUTGOING,
            *slot as i8,
            *inventory_slot as i8,
            *quantity as i32,
            Some(recipient),
        ),
        Op::Send { slot } => (command::SEND, boxno::OUTGOING, *slot as i8, -1, -1, None),
        Op::Cancel { slot } => (command::CANCEL, boxno::OUTGOING, *slot as i8, -1, -1, None),
        Op::Check { box_no } => (command::CHECK, box_no.wire(), -1, -1, -1, None),
        Op::Recv { slot } => (command::RECV, boxno::INCOMING, *slot as i8, 1, -1, None),
        Op::Confirm => (command::CONFIRM, boxno::NONE, -1, -1, -1, None),
        Op::Accept { slot } => (command::ACCEPT, boxno::INCOMING, *slot as i8, -1, -1, None),
        Op::Reject { slot } => (command::REJECT, boxno::INCOMING, *slot as i8, -1, -1, None),
        Op::Get { box_no, slot } => (command::GET, box_no.wire(), *slot as i8, -1, -1, None),
        Op::Clear { box_no, slot } => (command::CLEAR, box_no.wire(), *slot as i8, -1, -1, None),
        Op::Query { recipient } => (command::QUERY, boxno::NONE, -1, -1, -1, Some(recipient)),
        Op::DeliOpen => (command::DELI_OPEN, boxno::NONE, -1, -1, -1, None),
        Op::PostOpen => (command::POST_OPEN, boxno::NONE, -1, -1, -1, None),
        Op::PostClose { .. } => (command::POST_CLOSE, boxno::NONE, -1, -1, -1, None),
    };
    let mut buf = vec![0u8; 32];
    buf[0..4].copy_from_slice(&build_subpacket_header(ffxi_proto::map::c2s::PBX, 8, sync));
    buf[4] = cmd;
    buf[5] = box_no as u8;
    buf[6] = post_work_no as u8;
    buf[7] = item_work_no as u8;
    buf[8..12].copy_from_slice(&item_stacks.to_le_bytes());
    if let Some(name) = name {
        let bytes = name.as_bytes();
        let n = bytes.len().min(15); // NUL terminator stays inside TargetName[16]
        buf[16..16 + n].copy_from_slice(&bytes[..n]);
    }
    buf
}

// GP_CLI_COMMAND_AUC (vendor/server/src/map/packets/c2s/0x04e_auc.h). The
// GP_AUC_PARAM union is 12 bytes, so sizeof = 60 — LSB's PacketSize gate —
// with the 40-byte Parcel left zeroed c2s (GP_CLI_COMMAND_AUC::validate
// requires Result/ResultStatus zero).
const AUC_SUBPACKET_LEN: usize = 60;
const AUC_COMMAND_OFFSET: usize = 4;
const AUC_WORK_INDEX_OFFSET: usize = 5;
const AUC_PARAM_PRICE_OFFSET: usize = 8;
const AUC_PARAM_WORD0_OFFSET: usize = 12;
const AUC_PARAM_WORD1_OFFSET: usize = 14;
const AUC_PARAM_STACKS_OFFSET: usize = 16;

fn build_subpacket_auc(sync: u16, command: decode::AuctionCommand, work_index: i8) -> Vec<u8> {
    let mut buf = vec![0u8; AUC_SUBPACKET_LEN];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::AUC,
        (AUC_SUBPACKET_LEN / 4) as u16,
        sync,
    ));
    buf[AUC_COMMAND_OFFSET] = command as u8;
    buf[AUC_WORK_INDEX_OFFSET] = work_index as u8;
    buf
}

fn write_auc_param(buf: &mut [u8], price: u32, word0: u16, word1: u16, stacks: u32) {
    buf[AUC_PARAM_PRICE_OFFSET..AUC_PARAM_PRICE_OFFSET + 4].copy_from_slice(&price.to_le_bytes());
    buf[AUC_PARAM_WORD0_OFFSET..AUC_PARAM_WORD0_OFFSET + 2].copy_from_slice(&word0.to_le_bytes());
    buf[AUC_PARAM_WORD1_OFFSET..AUC_PARAM_WORD1_OFFSET + 2].copy_from_slice(&word1.to_le_bytes());
    buf[AUC_PARAM_STACKS_OFFSET..AUC_PARAM_STACKS_OFFSET + 4]
        .copy_from_slice(&stacks.to_le_bytes());
}

/// AH open handshake (GP_CLI_COMMAND_AUC::validate pins AucWorkIndex to -1).
pub fn build_subpacket_auc_work_check(sync: u16) -> Vec<u8> {
    build_subpacket_auc(
        sync,
        decode::AuctionCommand::WorkCheck,
        decode::AUCTION_WORK_INDEX_NONE,
    )
}

/// Sales Status window open (auctionutils::OpenListOfSales).
pub fn build_subpacket_auc_info(sync: u16) -> Vec<u8> {
    build_subpacket_auc(
        sync,
        decode::AuctionCommand::Info,
        decode::AUCTION_WORK_INDEX_NONE,
    )
}

/// Fee quote before the sale confirm (GP_AUC_PARAM_ASKCOMMIT;
/// `item_work_index` is the LOC_INVENTORY slot).
pub fn build_subpacket_auc_ask_commit(
    sync: u16,
    commission: u32,
    item_work_index: u16,
    item_no: u16,
    stacks: u32,
) -> Vec<u8> {
    let mut buf = build_subpacket_auc(
        sync,
        decode::AuctionCommand::AskCommit,
        decode::AUCTION_WORK_INDEX_NONE,
    );
    write_auc_param(&mut buf, commission, item_work_index, item_no, stacks);
    buf
}

/// Sale confirm into slot `work_index` (0..=6); GP_AUC_PARAM_LOT.
pub fn build_subpacket_auc_lot_in(
    sync: u16,
    limit_price: u32,
    item_work_index: u16,
    stacks: u32,
    work_index: i8,
) -> Vec<u8> {
    let mut buf = build_subpacket_auc(sync, decode::AuctionCommand::LotIn, work_index);
    write_auc_param(&mut buf, limit_price, item_work_index, 0, stacks);
    buf
}

/// Bid against slot `work_index`; GP_AUC_PARAM_BID.
pub fn build_subpacket_auc_bid(
    sync: u16,
    bid_price: u32,
    item_no: u16,
    stacks: u32,
    work_index: i8,
) -> Vec<u8> {
    let mut buf = build_subpacket_auc(sync, decode::AuctionCommand::Bid, work_index);
    write_auc_param(&mut buf, bid_price, item_no, 0, stacks);
    buf
}

pub fn build_subpacket_auc_lot_cancel(sync: u16, work_index: i8) -> Vec<u8> {
    build_subpacket_auc(sync, decode::AuctionCommand::LotCancel, work_index)
}

pub fn build_subpacket_auc_lot_check(sync: u16, work_index: i8) -> Vec<u8> {
    build_subpacket_auc(sync, decode::AuctionCommand::LotCheck, work_index)
}

// c2s 0x100 GP_CLI_COMMAND_MYROOM_JOB: MainJobIndex u8 @4, SupportJobIndex u8 @5,
// u16 pad; 0 = keep the current job
// (vendor/server/src/map/packets/c2s/0x100_myroom_job.h GP_CLI_COMMAND_MYROOM_JOB).
pub fn build_subpacket_myroom_job(sync: u16, main_job: Option<u8>, sub_job: Option<u8>) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::MYROOM_JOB,
        2,
        sync,
    ));
    buf[4] = main_job.unwrap_or(0);
    buf[5] = sub_job.unwrap_or(0);
    buf
}

// c2s 0x0CB GP_CLI_COMMAND_MYROOM_IS: Kind u8 @4, Param1 u8 @5, Param2 u16 LE @6,
// (vendor/server/src/map/packets/c2s/0x0cb_myroom_is.h). The DLL builds this packet only
// through helper FFXiMain.dll retail-2026-09 .text 0xF0A40: FC3C0(0xCB,1,1) allocates the
// 8-byte subpacket, it writes Kind→[+4], Param1→[+5], Param2→u16[+6] and commits 8 bytes
// via FC4D0 (vault main_menu.md §59 addendum 20).

/// GP_CLI_COMMAND_MYROOM_IS_KIND::Open — the door-row kind for a closed door. Its twin Close=2 is
/// the other half of the same row (vendor/server/src/map/packets/c2s/0x0cb_myroom_is.h); which one
/// the select sends follows the client's own door byte, exactly as FFXiMain.dll retail-2026-09
/// `.text 0x14556C` branches on it.
pub(crate) const MYROOM_IS_KIND_OPEN: u8 = 1;

/// …and ::Close, sent when the client's door byte says the house is already open to visitors.
pub(crate) const MYROOM_IS_KIND_CLOSE: u8 = 2;

/// …and ::Remodel — the roomlist style rows' kind (bead kuluu-o9qt7.13.5; every remodel
/// select rides it through sender helper FFXiMain.dll retail-2026-09 .text 0xF0A40).
pub(crate) const MYROOM_IS_KIND_REMODEL: u8 = 5;

/// GP_CLI_COMMAND_MYROOM_IS_PARAM2::Unk1, sent when opening the mog house door (the header's
/// Unk2=1 pairs with Close; 615..618 are remodel style ids), same file.
pub(crate) const MYROOM_IS_PARAM_DOOR_OPEN: u16 = 0;

/// …and ::Unk2, its closing twin.
pub(crate) const MYROOM_IS_PARAM_DOOR_CLOSE: u16 = 1;

/// The (Kind, Param2) a single door-row select sends, chosen the way retail's arm picks them from
/// its door byte (FFXiMain.dll retail-2026-09 `.text 0x14556C` tests game-state `+0x40C40` via
/// predicate `.text 0xF96E0`, then sends Kind=Open with PARAM2::Unk1 for a shut house and Kind=Close
/// with PARAM2::Unk2 for an open one — GP_CLI_COMMAND_MYROOM_IS_KIND/::_PARAM2 in
/// vendor/server/src/map/packets/c2s/0x0cb_myroom_is.h).
pub(crate) fn moghouse_door_packet(door_open: bool) -> (u8, u16) {
    if door_open {
        (MYROOM_IS_KIND_CLOSE, MYROOM_IS_PARAM_DOOR_CLOSE)
    } else {
        (MYROOM_IS_KIND_OPEN, MYROOM_IS_PARAM_DOOR_OPEN)
    }
}

pub fn build_subpacket_myroom_is(sync: u16, kind: u8, param1: u8, param2: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::MYROOM_IS,
        2,
        sync,
    ));
    buf[4] = kind;
    buf[5] = param1;
    buf[6..8].copy_from_slice(&param2.to_le_bytes());
    buf
}

// The Mog House layout/planting family, c2s 0x0FA..0x0FF. Retail builds each one through its own
// helper in FFXiMain.dll retail-2026-09 — Layout `.text 0xF0CC0`, the all-zero closing variant
// `.text 0xF0C80`, Bankin `.text 0xF0D40`, PlantAdd `.text 0xF0DA0`, PlantCheck `.text 0xF0E10`,
// PlantCrop `.text 0xF0E70`, PlantStop `.text 0xF0ED0` — and every helper is the same shape: allocate
// with FC3C0(opcode, …), write the fields, commit through FC4D0 with the byte count that matches the
// LSB struct below. The flush that calls them walks one furniture object's dirty bytes (Layout flag
// obj+0x2C then Bankin flag obj+0x2D) at `.text 0x1CE7A0`. Byte offsets in those writes are identical
// to LSB's `GP_CLI_COMMAND_MYROOM_*` structs, so these builders write the same bytes the client does.
//
/// Layout request payload: the shelf address of one item plus the pose retail sends with it. Retail
/// fills this from its furniture object (`col`←obj+0x14, level←obj+0x18… see the flush at
/// `.text 0x1CE7B0`, which passes obj+0x14/+0x18/+0x1C as the three position bytes and obj+0x20 as the
/// rotation); kuluu fills it from [`crate::myroom_furniture::PlacedFurniture`], whose fields carry LSB's
/// own names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MyroomLayoutRequest {
    pub item_no: u16,
    /// `MyroomItemIndex`: the shelf slot the item sits in.
    pub slot: u8,
    /// `MyroomCategory`: LOC_MOGSAFE or LOC_MOGSAFE2.
    pub container: u8,
    /// `MyroomFloorFlg` — LSB validates it in 0..=1, which is bit 0 of the layer bits kuluu keeps.
    pub floor: u8,
    /// Packet `x`, i.e. Attr byte 6 (LSB's Col).
    pub col: i8,
    /// Packet `y` — LSB validates it in 0..=25 and calls it the stacking elevation (Attr byte 7).
    pub level: i8,
    /// Packet `z`, Attr byte 8 (LSB's Row).
    pub row: i8,
    /// Rotation `v`: LSB validates it in 0..=3 (Attr byte 9).
    pub rotation: i8,
}

pub fn build_subpacket_myroom_layout(sync: u16, layout: &MyroomLayoutRequest) -> Vec<u8> {
    let mut buf = vec![0u8; 16];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::MYROOM_LAYOUT,
        3,
        sync,
    ));
    buf[4..6].copy_from_slice(&layout.item_no.to_le_bytes());
    buf[6] = layout.slot;
    buf[7] = layout.container;
    buf[8] = layout.floor;
    buf[9] = layout.col as u8;
    buf[10] = layout.level as u8;
    buf[11] = layout.row as u8;
    buf[12] = layout.rotation as u8;
    // The struct's three trailing pad bytes stay zero: every retail field is written above.
    buf
}

/// The all-zero form of the same packet (FFXiMain.dll retail-2026-09 `.text 0xF0C80`: allocate, write
/// zeros into +4..+0xC, commit 0x10). Retail sends it once when layout mode ends rather than entering
/// it; LSB reads `MyroomItemNo == 0` as exactly that and refreshes the Mog House enhancements instead
/// of placing anything (vendor/server/src/map/packets/c2s/0x0fa_myroom_layout.cpp process).
pub fn build_subpacket_myroom_layout_done(sync: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 16];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::MYROOM_LAYOUT,
        3,
        sync,
    ));
    buf
}

/// c2s 0x0FB GP_CLI_COMMAND_MYROOM_BANKIN — take a placed piece back off the room floor
/// (vendor/server/src/map/packets/c2s/0x0fb_myroom_bankin.h; helper FFXiMain.dll retail-2026-09
/// `.text 0xF0D40`, which writes +4 item number, +6 index and +7 category then commits 8 bytes).
pub fn build_subpacket_myroom_bankin(sync: u16, item_no: u16, slot: u8, container: u8) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::MYROOM_BANKIN,
        2,
        sync,
    ));
    buf[4..6].copy_from_slice(&item_no.to_le_bytes());
    buf[6] = slot;
    buf[7] = container;
    buf
}

/// c2s 0x0FC GP_CLI_COMMAND_MYROOM_PLANT_ADD — sow a seed into a pot or feed a growing plant
/// (vendor/server/src/map/packets/c2s/0x0fc_myroom_plant_add.h; helper FFXiMain.dll retail-2026-09
/// `.text 0xF0DA0` commits 12 bytes: +4/+6 the two item numbers, +8/+9 their slots, +A/+B their
/// containers).
pub fn build_subpacket_myroom_plant_add(
    sync: u16,
    plant_item_no: u16,
    add_item_no: u16,
    plant_slot: u8,
    add_slot: u8,
    plant_container: u8,
    add_container: u8,
) -> Vec<u8> {
    let mut buf = vec![0u8; 12];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::MYROOM_PLANT_ADD,
        2,
        sync,
    ));
    buf[4..6].copy_from_slice(&plant_item_no.to_le_bytes());
    buf[6..8].copy_from_slice(&add_item_no.to_le_bytes());
    buf[8] = plant_slot;
    buf[9] = add_slot;
    buf[10] = plant_container;
    buf[11] = add_container;
    buf
}

/// c2s 0x0FD GP_CLI_COMMAND_MYROOM_PLANT_CHECK — examine a growing plant so the server reports its
/// state (vendor/server/src/map/packets/c2s/0x0fd_myroom_plant_check.h; helper FFXiMain.dll
/// retail-2026-09 `.text 0xF0E10`).
pub fn build_subpacket_myroom_plant_check(
    sync: u16,
    plant_item_no: u16,
    plant_slot: u8,
    plant_container: u8,
) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::MYROOM_PLANT_CHECK,
        2,
        sync,
    ));
    buf[4..6].copy_from_slice(&plant_item_no.to_le_bytes());
    buf[6] = plant_slot;
    buf[7] = plant_container;
    buf
}

/// c2s 0x0FE GP_CLI_COMMAND_MYROOM_PLANT_CROP — harvest. `cancel` is the header's `CancellFlg`: retail
/// passes 1 when the player backs out of the harvest prompt (call site FFXiMain.dll retail-2026-09
/// `.text 0x206951`) and 0 on the real harvest (`.text 0x2069B1`); the helper is `.text 0xF0E70` and
/// commits 12 bytes with the flag at +8
/// (vendor/server/src/map/packets/c2s/0x0fe_myroom_plant_crop.h).
pub fn build_subpacket_myroom_plant_crop(
    sync: u16,
    plant_item_no: u16,
    plant_slot: u8,
    plant_container: u8,
    cancel: bool,
) -> Vec<u8> {
    let mut buf = vec![0u8; 12];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::MYROOM_PLANT_CROP,
        2,
        sync,
    ));
    buf[4..6].copy_from_slice(&plant_item_no.to_le_bytes());
    buf[6] = plant_slot;
    buf[7] = plant_container;
    buf[8] = u8::from(cancel);
    buf
}

/// c2s 0x0FF GP_CLI_COMMAND_MYROOM_PLANT_STOP — stop drying/harvesting a plant
/// (vendor/server/src/map/packets/c2s/0x0ff_myroom_plant_stop.h; helper FFXiMain.dll retail-2026-09
/// `.text 0xF0ED0`).
pub fn build_subpacket_myroom_plant_stop(
    sync: u16,
    plant_item_no: u16,
    plant_slot: u8,
    plant_container: u8,
) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::MYROOM_PLANT_STOP,
        2,
        sync,
    ));
    buf[4..6].copy_from_slice(&plant_item_no.to_le_bytes());
    buf[6] = plant_slot;
    buf[7] = plant_container;
    buf
}

// c2s 0x0F2 GP_CLI_COMMAND_SUBMAPCHANGE: State u16 @4, SubMapNumber u16 @6
// (vendor/server/src/map/packets/c2s/0x0f2_submapchange.h). Sent on every
// sub-area latch change so `PChar->loc.boundary` — and the next relog's
// restored interior — stay in sync (0x0f2_submapchange.cpp process).
pub(crate) fn build_subpacket_submapchange(sync: u16, state: u16, sub_map_number: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::SUBMAPCHANGE,
        2,
        sync,
    ));
    buf[4..6].copy_from_slice(&state.to_le_bytes());
    buf[6..8].copy_from_slice(&sub_map_number.to_le_bytes());
    buf
}

pub(crate) fn build_subpacket_header(opcode: u16, size_words: u16, sync: u16) -> [u8; 4] {
    let id_and_size = framing::subpacket_header_word(opcode, size_words);
    let mut h = [0u8; 4];
    h[0..2].copy_from_slice(&id_and_size.to_le_bytes());
    h[2..4].copy_from_slice(&sync.to_le_bytes());
    h
}

pub(crate) fn build_subpacket_gameok(sync: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 12];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::GAMEOK,
        3,
        sync,
    ));
    buf
}

// c2s 0x061 GP_CLI_COMMAND_CLISTATUS: unknown00 u8 @4 (validated 0..=1), padded
// to a word. The server answers with SendLocalPlayerPackets — GROUP_ATTR for
// self, CLISTATUS, ... (vendor/server/src/map/packets/c2s/0x061_clistatus.cpp).
// Wire size is header(4) + body(4) = 8 bytes per the C++ struct
// (0x061_clistatus.h: unknown00 u8, padding00 u8, padding01 u16), so
// size_words must be 2 — declaring 1 makes LSB reject the packet at
// validation before dispatch.
pub(crate) fn build_subpacket_clistatus(sync: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::CLISTATUS,
        2,
        sync,
    ));
    buf
}

// c2s 0x076 GP_CLI_COMMAND_GROUP_LIST_REQ: Kind u8 @4 (validated == 0).
// Requests the full party table; the server answers with GROUP_TBL (0x0C8) +
// GROUP_LIST (0x0DD) for every member.
// Wire size is header(4) + body(4) = 8 bytes per the C++ struct
// (0x076_group_list_req.h: uint8_t Kind), so size_words must be 2 — declaring
// 1 makes LSB reject the packet at validation before dispatch.
pub(crate) fn build_subpacket_group_list_req(sync: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::GROUP_LIST_REQ,
        2,
        sync,
    ));
    // Kind = 0 (the only valid value per LSB's validator).
    buf
}

// c2s GROUP_LEAVE (0x06F)/GROUP_BREAKUP (0x070) builders arrive with their prtyN
// rows once the member/leader select mapping is dug (vault main_menu.md §24).

// c2s 0x074 GP_CLI_COMMAND_GROUP_SOLICIT_RES: Res u8 @4 (Decline = 0, Accept = 1;
// vendor/server/src/map/packets/c2s/0x074_group_solicit_res.h
// GP_CLI_COMMAND_GROUP_SOLICIT_RES_RES) + padding[3]. 8 bytes total.
pub(crate) fn build_subpacket_group_solicit_res(sync: u16, accept: bool) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::GROUP_SOLICIT_RES,
        2,
        sync,
    ));
    // GP_CLI_COMMAND_GROUP_SOLICIT_RES_RES { Decline = 0, Accept = 1 }.
    buf[4] = u8::from(accept);
    buf
}

/// c2s 0x032 TRADE_REQ: UniqueNo u32 + ActIndex u16 + padding u16
/// (vendor/server/src/map/packets/c2s/0x032_trade_req.h).
pub(crate) fn build_subpacket_trade_req(sync: u16, unique_no: u32, act_index: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 4 + 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::TRADE_REQ,
        3,
        sync,
    ));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());
    buf[8..10].copy_from_slice(&act_index.to_le_bytes());
    // body[10..12] = padding00 (dammy2), left zeroed.
    buf
}

/// c2s 0x033 TRADE_RES: Kind u32 + TradeCounter u16 (+pad) — kinds are
/// [`ffxi_proto::decode::TradeResKind`] numbers (Start = 0, Cancell = 1, Make = 2;
/// vendor/server/src/map/packets/c2s/0x033_trade_res.h). Retail stamps the live
/// window refresh counter (vault main_menu.md §25); LSB never reads the field
/// back (validate/process ignore it), so kuluu sends a live counter of 0 until
/// its own windows adopt one.
pub(crate) fn build_subpacket_trade_res(sync: u16, kind: u32, counter: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 4 + 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::TRADE_RES,
        3,
        sync,
    ));
    buf[4..8].copy_from_slice(&kind.to_le_bytes());
    buf[8..10].copy_from_slice(&counter.to_le_bytes());
    buf
}

/// c2s 0x034 TRADE_LIST: ItemNum u32 + ItemNo u16 + ItemIndex u8 + TradeIndex u8
/// (vendor/server/src/map/packets/c2s/0x034_trade_list.h). The gil offer uses the
/// GIL sentinel `item_no` on slot 0; clearing a slot is qty = 0.
pub(crate) fn build_subpacket_trade_list(
    sync: u16,
    qty: u32,
    item_no: u16,
    item_index: u8,
    trade_index: u8,
) -> Vec<u8> {
    let mut buf = vec![0u8; 4 + 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::TRADE_LIST,
        3,
        sync,
    ));
    buf[4..8].copy_from_slice(&qty.to_le_bytes());
    buf[8..10].copy_from_slice(&item_no.to_le_bytes());
    buf[10] = item_index;
    buf[11] = trade_index;
    buf
}

/// GP_CLI_COMMAND_COMBINE_ASK (vendor/server/src/map/packets/c2s/
/// 0x096_combine_ask.h; vault main_menu.md §28): header + the 32-byte body,
/// built by [`ffxi_proto::synth::encode_combine_ask`] with a zero HashNo
/// (LSB never reads that byte).
pub(crate) fn build_subpacket_combine_ask(
    sync: u16,
    crystal_id: u16,
    crystal_slot: u8,
    ingredients: &[ffxi_proto::synth::SynthIngredient],
) -> Vec<u8> {
    let mut buf = vec![0u8; 4 + ffxi_proto::synth::ASK_BODY_LEN];
    // One header word plus an eight-word body.
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::COMBINE_ASK,
        9,
        sync,
    ));
    let body = ffxi_proto::synth::encode_combine_ask(0, crystal_id, crystal_slot, ingredients);
    buf[4..].copy_from_slice(&body);
    buf
}

pub(crate) fn build_subpacket_zone_transition(sync: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::ZONE_TRANSITION,
        2,
        sync,
    ));

    buf[4] = 2;
    buf
}

pub(crate) fn build_subpacket_chat(sync: u16, kind: u8, text: &str) -> Vec<u8> {
    let str_bytes = text.as_bytes();
    let str_len = str_bytes.len().min(127);
    let body_unpadded = 2 + str_len + 1;
    let body_padded = (body_unpadded + 3) & !3;
    let total = 4 + body_padded;
    let size_words = (total / 4) as u16;

    let mut buf = vec![0u8; total];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::CHAT,
        size_words,
        sync,
    ));
    buf[4] = kind;

    buf[6..6 + str_len].copy_from_slice(&str_bytes[..str_len]);

    buf
}

// GP_CLI_COMMAND_CHAT_NAME.sName is char[15] (vendor/server/src/map/packets/
// c2s/0x0b6_chat_name.h sName); LSB reads it via asStringFromUntrustedSource(
// sName, sizeof(sName)) (0x0b6_chat_name.cpp GP_CLI_COMMAND_CHAT_NAME::process recipientName), whose strnlen form tolerates
// a fully-populated unterminated field, so all 15 bytes are usable.
pub(crate) const CHAT_NAME_SNAME_LEN: usize = 15;
const CHAT_NAME_MESSAGE_OFFSET: usize = 4 + 1 + 1 + CHAT_NAME_SNAME_LEN;

pub(crate) fn build_subpacket_tell(sync: u16, recipient: &str, text: &str) -> Vec<u8> {
    let r_bytes = recipient.as_bytes();
    let r_len = r_bytes.len().min(CHAT_NAME_SNAME_LEN);
    let t_bytes = text.as_bytes();
    let t_len = t_bytes.len().min(127);

    let body_unpadded = 1 + 1 + CHAT_NAME_SNAME_LEN + t_len + 1;
    let body_padded = (body_unpadded + 3) & !3;
    let total = 4 + body_padded;
    let size_words = (total / 4) as u16;

    let mut buf = vec![0u8; total];
    buf[0..4].copy_from_slice(&build_subpacket_header(CHAT_NAME, size_words, sync));
    // GP_CLI_COMMAND_CHAT_NAME.unknown00 must be 3 or the server rejects the
    // packet (PacketValidator .mustEqual(unknown00, 3), 0x0b6_chat_name.cpp);
    // the retail client always sends 3. Without it every tell — and the
    // customMenu reply — is silently dropped.
    buf[4] = CHAT_NAME_UNKNOWN00;

    buf[6..6 + r_len].copy_from_slice(&r_bytes[..r_len]);

    buf[CHAT_NAME_MESSAGE_OFFSET..CHAT_NAME_MESSAGE_OFFSET + t_len]
        .copy_from_slice(&t_bytes[..t_len]);
    buf
}

pub(crate) const CHAT_NAME_UNKNOWN00: u8 = 3;

// c2s 0x05B GP_CLI_COMMAND_EVENTEND (vendor/server/src/map/packets/c2s/
// 0x05b_eventend.h GP_CLI_COMMAND_EVENTEND): UniqueNo u32, EndPara u32, ActIndex u16,
// Mode u16 (GP_CLI_COMMAND_EVENTEND_MODE: 0 = End, 1 = UpdatePending),
// EventNum u16 (zone id; retail echoes the GP_SERV LOGIN EventNum,
// 0x00a_login.cpp GP_SERV_COMMAND_LOGIN::GP_SERV_COMMAND_LOGIN), EventPara u16 (the event id the validator matches
// against currentEvent->eventId, validation.cpp PacketValidator::isInEvent).
pub(crate) fn build_subpacket_event_end(
    sync: u16,
    unique_no: u32,
    act_index: u16,
    event_zone: u16,
    event_id: u16,
    choice: u32,
    mode: u16,
) -> Vec<u8> {
    let mut buf = vec![0u8; 20];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::EVENT_END,
        5,
        sync,
    ));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());
    buf[8..12].copy_from_slice(&choice.to_le_bytes());
    buf[12..14].copy_from_slice(&act_index.to_le_bytes());
    buf[14..16].copy_from_slice(&mode.to_le_bytes());

    buf[16..18].copy_from_slice(&event_zone.to_le_bytes());
    buf[18..20].copy_from_slice(&event_id.to_le_bytes());
    buf
}

// vendor/server/src/map/packets/c2s/0x05c_eventendxzy.h GP_CLI_COMMAND_EVENTENDXZY.
pub(crate) fn build_subpacket_event_position(
    sync: u16,
    identity: (u32, u16, u16),
    event_zone: u16,
    end_para: u32,
    position: crate::state::Position,
) -> Vec<u8> {
    const PACKET_BYTES: usize = 32;
    const PACKET_WORDS: u16 = (PACKET_BYTES / 4) as u16;
    let (unique_no, act_index, event_id) = identity;
    let mut buf = vec![0; PACKET_BYTES];
    buf[..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::event_position_wire::OPCODE,
        PACKET_WORDS,
        sync,
    ));
    buf[4..8].copy_from_slice(&position.pos.x.to_le_bytes());
    buf[8..12].copy_from_slice(&position.pos.z.to_le_bytes());
    buf[12..16].copy_from_slice(&position.pos.y.to_le_bytes());
    buf[16..20].copy_from_slice(&unique_no.to_le_bytes());
    buf[20..24].copy_from_slice(&end_para.to_le_bytes());
    buf[24..26].copy_from_slice(&event_zone.to_le_bytes());
    buf[26..28].copy_from_slice(&event_id.to_le_bytes());
    buf[28..30].copy_from_slice(&act_index.to_le_bytes());
    buf[30] = ffxi_proto::map::event_position_wire::UPDATE_PENDING as u8;
    buf[31] = position.heading;
    buf
}

// c2s 0x05C GP_CLI_COMMAND_EVENTENDXZY (vendor/server/src/map/packets/c2s/
// 0x05c_eventendxzy.h): x/y/z f32, UniqueNo u32, EndPara u32, EventNum u16,
// EventPara u16, ActIndex u16, Mode u8, dir i8. The validator accepts only
// Mode 1 (UpdatePending) (GP_CLI_COMMAND_EVENTENDXZY::validate), and the
// server stores dir into position_t.rotation on its 0..=255 scale
// (GP_CLI_COMMAND_EVENTENDXZY::process).
pub(crate) fn build_subpacket_event_end_xzy(
    sync: u16,
    unique_no: u32,
    x: f32,
    y: f32,
    z: f32,
    dir: u8,
    end_para: u32,
    act_index: u16,
    event_zone: u16,
    event_id: u16,
) -> Vec<u8> {
    let mut buf = vec![0u8; 32];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::EVENT_END_XZY,
        8,
        sync,
    ));
    buf[4..8].copy_from_slice(&x.to_le_bytes());
    buf[8..12].copy_from_slice(&y.to_le_bytes());
    buf[12..16].copy_from_slice(&z.to_le_bytes());
    buf[16..20].copy_from_slice(&unique_no.to_le_bytes());
    buf[20..24].copy_from_slice(&end_para.to_le_bytes());
    buf[24..26].copy_from_slice(&event_zone.to_le_bytes());
    buf[26..28].copy_from_slice(&event_id.to_le_bytes());
    buf[28..30].copy_from_slice(&act_index.to_le_bytes());
    buf[30] = ffxi_proto::map::c2s::event_end_mode::UPDATE_PENDING as u8;
    buf[31] = dir;
    buf
}

// Inverse of the s2c 0x055 id decode — LSB reads the bits back as
// keyItemId = TableIndex*512 + word*32 + bit (vendor/server/src/map/packets/
// c2s/0x064_scenarioitem.cpp GP_CLI_COMMAND_SCENARIOITEM::process keyItemId). Ids outside `table_index`'s range are
// ignored; returns whether any bit changed.
pub(crate) fn fold_seen_ids_into_look_flags(
    table_index: u16,
    ids: &[u16],
    look_flags: &mut [u32; decode::ScenarioItem::WORDS],
) -> bool {
    let word_bits = u32::BITS as usize;
    let mut changed = false;
    for id in ids {
        let global = *id as usize;
        if global / decode::ScenarioItem::BITS_PER_TABLE != table_index as usize {
            continue;
        }
        let local = global % decode::ScenarioItem::BITS_PER_TABLE;
        let mask = 1u32 << (local % word_bits);
        if look_flags[local / word_bits] & mask == 0 {
            look_flags[local / word_bits] |= mask;
            changed = true;
        }
    }
    changed
}

// LSB gates c2s 0x064 with blockedBy(InEvent) and silently drops it unless
// UniqueNo == char id and ActIndex == self targid (vendor/server/src/map/
// packets/c2s/0x064_scenarioitem.cpp PacketValidator), so an unseeded targid must skip
// the send; a table whose s2c 0x055 never arrived has only default-zeroed
// local flags, so marking against it would report the table empty. Ok carries
// the validated ActIndex.
pub(crate) fn mark_seen_send_block_reason(
    in_event: bool,
    self_act_index: Option<u16>,
    table_received: bool,
) -> Result<u16, &'static str> {
    if in_event {
        return Err("InEvent blocks 0x064");
    }
    let act_index = self_act_index.ok_or("self act_index not yet seeded")?;
    if !table_received {
        return Err("table's 0x055 not received this session");
    }
    Ok(act_index)
}

// c2s 0x064 GP_CLI_COMMAND_SCENARIOITEM (vendor/server/src/map/packets/c2s/
// 0x064_scenarioitem.h): UniqueNo u32, LookItemFlag u32[16], ActIndex u16,
// TableIndex u16. The server ORs every set LookItemFlag bit into the table's
// seen list and validates UniqueNo == char id, ActIndex == self targid,
// TableIndex < tables.size() (0x064_scenarioitem.cpp); blocked while InEvent.
pub(crate) fn build_subpacket_scenario_item(
    sync: u16,
    unique_no: u32,
    act_index: u16,
    table_index: u16,
    look_flags: &[u32; decode::ScenarioItem::WORDS],
) -> Vec<u8> {
    const TOTAL: usize = 4 + 4 + decode::ScenarioItem::WORDS * 4 + 2 + 2;
    let mut buf = vec![0u8; TOTAL];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::SCENARIO_ITEM,
        (TOTAL / 4) as u16,
        sync,
    ));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());
    for (i, w) in look_flags.iter().enumerate() {
        let o = 8 + i * 4;
        buf[o..o + 4].copy_from_slice(&w.to_le_bytes());
    }
    buf[72..74].copy_from_slice(&act_index.to_le_bytes());
    buf[74..76].copy_from_slice(&table_index.to_le_bytes());
    buf
}

// GP_CLI_COMMAND_ITEM_MOVE, vendor/server/src/map/packets/c2s/0x029_item_move.h:
// ItemNum u32 @4, Category1 u8 @8, Category2 u8 @9, ItemIndex1 u8 @10,
// ItemIndex2 u8 @11 — 12 bytes (size_words = 3). An ItemIndex2 < 82 asks for a
// same-id stack merge into that slot; anything larger lets the server pick a free
// slot (0x029_item_move.cpp process), which retail requests with 0xFF.
pub(crate) const ITEM_MOVE_AUTO_SLOT: u8 = 0xFF;

pub(crate) fn build_subpacket_maprect(
    sync: u16,
    rect_id: u32,
    x: f32,
    y: f32,
    z: f32,
    act_index: u16,
) -> Vec<u8> {
    let mut buf = vec![0u8; 24];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x05E, 6, sync));
    buf[4..8].copy_from_slice(&rect_id.to_le_bytes());
    buf[8..12].copy_from_slice(&x.to_le_bytes());
    buf[12..16].copy_from_slice(&y.to_le_bytes());
    buf[16..20].copy_from_slice(&z.to_le_bytes());
    buf[20..22].copy_from_slice(&act_index.to_le_bytes());

    buf
}

/// The RectID fourcc LSB matches for the universal MH exit
/// (vendor/server/src/map/packets/c2s/0x05e_maprect.cpp GP_CLI_COMMAND_MAPRECT::process isMogHouseExit). Emitted by
/// [`build_subpacket_maprect_mh_exit`]; also the `pending_maprect` line id.
pub(crate) const ZMRQ_LE: u32 = u32::from_le_bytes(*b"zmrq");

pub(crate) fn build_subpacket_maprect_mh_exit(
    sync: u16,
    exit_bit: u8,
    exit_mode: u8,
    x: f32,
    y: f32,
    z: f32,
    act_index: u16,
) -> Vec<u8> {
    let mut buf = vec![0u8; 24];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x05E, 6, sync));

    buf[4..8].copy_from_slice(&ZMRQ_LE.to_le_bytes());
    buf[8..12].copy_from_slice(&x.to_le_bytes());
    buf[12..16].copy_from_slice(&y.to_le_bytes());
    buf[16..20].copy_from_slice(&z.to_le_bytes());
    buf[20..22].copy_from_slice(&act_index.to_le_bytes());
    buf[22] = exit_bit;
    buf[23] = exit_mode;
    buf
}

/// Client-side mirror of the LSB 0x05D validator: `blockedBy InEvent`,
/// `oneOf<EmoteMode>`, `range Number Point..=Aim` (0x05d_motion.cpp GP_CLI_COMMAND_MOTION::validate) and
/// the bell note range (:82). `None` = OK to send. The bell-equip and
/// job-unlock checks stay server-side (the client lacks lockstyle state).
pub(crate) fn emote_send_block_reason(
    emote_id: u8,
    mode: u8,
    param: u16,
    in_event: bool,
) -> Option<String> {
    use ffxi_proto::map::emote;
    if in_event {
        return Some("busy with an event".into());
    }
    if mode > emote::mode::MOTION {
        return Some(format!("invalid mode {mode}"));
    }
    if ffxi_vocab::emote_names::lookup(emote_id).is_none() {
        return Some(format!("unknown emote id {emote_id}"));
    }
    if emote_id == emote::BELL && !(emote::BELL_NOTE_MIN..=emote::BELL_NOTE_MAX).contains(&param) {
        return Some(format!(
            "bell note {param} out of range {}..={}",
            emote::BELL_NOTE_MIN,
            emote::BELL_NOTE_MAX
        ));
    }
    None
}

/// vendor/server/src/map/packets/c2s/0x015_pos.cpp GP_CLI_COMMAND_POS::process
/// discards the update when x, y or z is not finite.
pub(crate) fn build_subpacket_pos(
    sync: u16,
    x: f32,
    y: f32,
    z: f32,
    heading: u8,
    face_target: u16,
) -> Option<Vec<u8>> {
    if !(x.is_finite() && y.is_finite() && z.is_finite()) {
        return None;
    }
    let mut buf = vec![0u8; 32];
    buf[0..4].copy_from_slice(&build_subpacket_header(ffxi_proto::map::c2s::POS, 8, sync));
    buf[4..8].copy_from_slice(&x.to_le_bytes());
    buf[8..12].copy_from_slice(&z.to_le_bytes());
    buf[12..16].copy_from_slice(&y.to_le_bytes());
    buf[20] = heading;
    // GP_CLI_COMMAND_POS.facetarget (vendor/server/src/map/packets/c2s/0x015_pos.h): the targid
    // we're looking at, relayed by the server so other clients turn our head. +21
    // is the TargetMode/RunMode/GroundMode bitfield, left 0.
    buf[22..24].copy_from_slice(&face_target.to_le_bytes());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);
    buf[24..28].copy_from_slice(&now.to_le_bytes());
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_proto::decode::{
        AUCTION_STACKS_SINGLE, AUCTION_STACKS_STACK, AUCTION_WORK_INDEX_NONE,
    };

    fn assert_auc_frame(buf: &[u8], sync: u16, command: decode::AuctionCommand) {
        assert_eq!(buf.len(), AUC_SUBPACKET_LEN, "sizeof(GP_CLI_COMMAND_AUC)");
        let hdr = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(
            hdr & ffxi_proto::framing::SUBPACKET_OPCODE_MASK,
            ffxi_proto::map::c2s::AUC,
            "opcode 0x04E"
        );
        assert_eq!(
            (hdr >> 9) as usize,
            AUC_SUBPACKET_LEN / 4,
            "size_words = 15"
        );
        assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), sync, "sync");
        assert_eq!(buf[AUC_COMMAND_OFFSET], command as u8, "Command");
        assert_eq!(buf[6], 0, "Result (GP_CLI_COMMAND_AUC::validate)");
        assert_eq!(buf[7], 0, "ResultStatus (GP_CLI_COMMAND_AUC::validate)");
        assert_eq!(&buf[20..AUC_SUBPACKET_LEN], &[0u8; 40], "Parcel unused c2s");
    }

    #[test]
    fn auc_ask_commit_layout_matches_server_struct() {
        let buf = build_subpacket_auc_ask_commit(0xBEEF, 9, 5, 4570, AUCTION_STACKS_STACK);
        assert_auc_frame(&buf, 0xBEEF, decode::AuctionCommand::AskCommit);
        assert_eq!(buf[AUC_WORK_INDEX_OFFSET] as i8, AUCTION_WORK_INDEX_NONE);
        assert_eq!(u32::from_le_bytes(buf[8..12].try_into().unwrap()), 9);
        assert_eq!(u16::from_le_bytes([buf[12], buf[13]]), 5);
        assert_eq!(u16::from_le_bytes([buf[14], buf[15]]), 4570);
        assert_eq!(
            u32::from_le_bytes(buf[16..20].try_into().unwrap()),
            AUCTION_STACKS_STACK
        );
    }

    #[test]
    fn auc_bid_layout_matches_server_struct() {
        let buf = build_subpacket_auc_bid(0x1234, 490, 17440, AUCTION_STACKS_SINGLE, 6);
        assert_auc_frame(&buf, 0x1234, decode::AuctionCommand::Bid);
        assert_eq!(buf[AUC_WORK_INDEX_OFFSET] as i8, 6);
        assert_eq!(u32::from_le_bytes(buf[8..12].try_into().unwrap()), 490);
        assert_eq!(u16::from_le_bytes([buf[12], buf[13]]), 17440);
        assert_eq!(u16::from_le_bytes([buf[14], buf[15]]), 0, "padding00");
        assert_eq!(
            u32::from_le_bytes(buf[16..20].try_into().unwrap()),
            AUCTION_STACKS_SINGLE
        );
    }

    #[test]
    fn submapchange_layout_matches_server_struct() {
        let buf =
            build_subpacket_submapchange(0x0102, ffxi_proto::map::submap::state::GENERAL, 400);
        assert_eq!(buf.len(), 8, "sizeof(GP_CLI_COMMAND_SUBMAPCHANGE) + header");
        let hdr = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(
            hdr & ffxi_proto::framing::SUBPACKET_OPCODE_MASK,
            ffxi_proto::map::c2s::SUBMAPCHANGE,
            "opcode 0x0F2"
        );
        assert_eq!((hdr >> 9) as usize, 2, "size_words");
        assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0x0102, "sync");
        assert_eq!(
            u16::from_le_bytes([buf[4], buf[5]]),
            ffxi_proto::map::submap::state::GENERAL,
            "State"
        );
        assert_eq!(u16::from_le_bytes([buf[6], buf[7]]), 400, "SubMapNumber");
    }

    #[test]
    fn submapchange_no_sub_area_sentinel_is_zero() {
        let buf = build_subpacket_submapchange(
            1,
            ffxi_proto::map::submap::state::GENERAL,
            ffxi_proto::map::submap::NO_SUB_AREA,
        );
        assert_eq!(u16::from_le_bytes([buf[6], buf[7]]), 0);
    }

    #[test]
    fn auc_work_check_pins_open_handshake() {
        let buf = build_subpacket_auc_work_check(0x0042);
        assert_auc_frame(&buf, 0x0042, decode::AuctionCommand::WorkCheck);
        assert_eq!(buf[AUC_WORK_INDEX_OFFSET] as i8, AUCTION_WORK_INDEX_NONE);
        assert_eq!(&buf[8..20], &[0u8; 12], "GP_AUC_PARAM unused");
    }

    #[test]
    fn group_solicit_res_bytes_match_lsb_packet() {
        // vendor/server/src/map/packets/c2s/0x074_group_solicit_res.h:
        // Res u8 { Decline = 0, Accept = 1 } + padding[3]; header says 2 words.
        for (accept, want) in [(false, 0u8), (true, 1)] {
            let buf = build_subpacket_group_solicit_res(0x7E, accept);
            assert_eq!(buf.len(), 8);
            let hdr = u16::from_le_bytes([buf[0], buf[1]]);
            assert_eq!(hdr & 0x01FF, ffxi_proto::map::c2s::GROUP_SOLICIT_RES);
            assert_eq!((hdr >> 9) as usize, 2, "size_words");
            assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0x7E, "sync");
            assert_eq!(buf[4], want, "Res");
            assert_eq!(&buf[5..8], &[0u8; 3], "padding");
        }
    }

    /// c2s 0x32/0x33/0x34 byte law per vendor/server headers: 12-byte
    /// subpackets (3 words), bodies UniqueNo|Kind|ItemNum first.
    #[test]
    fn trade_builders_pin_lsb_layouts() {
        let req = build_subpacket_trade_req(0x7F, 0x00A1_0002, 0x1A);
        assert_eq!(req.len(), 12);
        let hdr = u16::from_le_bytes([req[0], req[1]]);
        assert_eq!(hdr & 0x01FF, ffxi_proto::map::c2s::TRADE_REQ);
        assert_eq!((hdr >> 9) as usize, 3, "size_words");
        assert_eq!(u16::from_le_bytes([req[2], req[3]]), 0x7F, "sync");
        assert_eq!(
            u32::from_le_bytes(req[4..8].try_into().unwrap()),
            0x00A1_0002
        );
        assert_eq!(u16::from_le_bytes(req[8..10].try_into().unwrap()), 0x1A);
        assert_eq!(&req[10..], &[0u8; 2], "padding00");

        let res = build_subpacket_trade_res(0x7F, 2, 0);
        let hdr = u16::from_le_bytes([res[0], res[1]]);
        assert_eq!(hdr & 0x01FF, ffxi_proto::map::c2s::TRADE_RES);
        assert_eq!(
            u32::from_le_bytes(res[4..8].try_into().unwrap()),
            2,
            "Kind Make"
        );

        let list = build_subpacket_trade_list(0x7F, 12, 4212, 9, 3);
        let hdr = u16::from_le_bytes([list[0], list[1]]);
        assert_eq!(hdr & 0x01FF, ffxi_proto::map::c2s::TRADE_LIST);
        assert_eq!(u32::from_le_bytes(list[4..8].try_into().unwrap()), 12);
        assert_eq!(u16::from_le_bytes(list[8..10].try_into().unwrap()), 4212);
        assert_eq!(list[10], 9, "ItemIndex (bag slot)");
        assert_eq!(list[11], 3, "TradeIndex");
    }
}
