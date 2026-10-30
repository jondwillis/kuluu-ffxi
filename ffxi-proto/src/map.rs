mod event_query_wire {
    include!(concat!(env!("OUT_DIR"), "/event_query_wire.rs"));
}

include!(concat!(env!("OUT_DIR"), "/map_settings_table.rs"));

pub const MAX_DATAGRAM: usize = 2500;

pub mod c2s {
    pub const LOGIN: u16 = 0x00A;

    // GP_CLI_COMMAND_CONFIG — change client option flags (bitfield mask +
    // SetFlg; vendor/server/src/map/packets/c2s/0x0dc_config.h). Direction
    // sibling of s2c GROUP_SOLICIT_REQ at the same number: the two never
    // collide because each lives in its own direction's table.
    pub const CONFIG: u16 = 0x0DC;

    // GP_CLI_COMMAND_CLISTATUS, vendor/server/src/map/packets/c2s/0x061_clistatus.h.
    // Requests the local player's status block; the server answers with
    // SendLocalPlayerPackets (charutils.cpp) — s2c GROUP_ATTR (0x0DF) for self +
    // CLISTATUS + ... This is how a solo player gets its own group entry after
    // zone-in/login: LSB pushes no 0x0DD/0x0DF to players without a party.
    pub const CLISTATUS: u16 = 0x061;

    // GP_CLI_COMMAND_CURRENCIES_1 / _2, vendor/server/src/map/packets/c2s/
    // 0x10f_currencies_1.h + 0x115_currencies_2.h: header-only requests (`GP_CLI_PACKET`
    // with no fields) for the two Currencies lists; LSB answers each with its s2c
    // CURRENCIES reply below (FFXiMain.dll retail-2026-09 `.text 0xFD520`/`.text 0xFD550`,
    // vault main_menu.md §59 addendum 27). Direction siblings of s2c FISH at 0x115.
    pub const CURRENCIES_1: u16 = 0x10F;
    pub const CURRENCIES_2: u16 = 0x115;

    pub const GAMEOK: u16 = 0x00C;
    pub const NETEND: u16 = 0x00D;
    pub const ZONE_TRANSITION: u16 = 0x011;
    pub const POS: u16 = 0x015;

    pub const ACTION: u16 = 0x01A;
    pub const EVENT_END: u16 = 0x05B;

    // GP_CLI_COMMAND_EVENTENDXZY, vendor/server/src/map/packets/c2s/
    // 0x05c_eventendxzy.h: x/y/z f32, UniqueNo u32, EndPara u32 (Work_Zone[1]),
    // EventNum/EventPara/ActIndex u16, Mode u8, dir i8. The validator accepts
    // only Mode 1 (UpdatePending); this packet never ends an event
    // (0x05c_eventendxzy.cpp GP_CLI_COMMAND_EVENTENDXZY::validate).
    pub const EVENT_END_XZY: u16 = 0x05C;

    /// The `Mode` field of the c2s event-end packets.
    /// GP_CLI_COMMAND_EVENTEND_MODE, vendor/server/src/map/packets/c2s/0x05b_eventend.h.
    pub mod event_end_mode {
        /// End the event (OnEventFinish).
        pub const END: u16 = 0;
        /// Update a pending event state (OnEventUpdate); also the only Mode
        /// the `EVENT_END_XZY` validator accepts.
        pub const UPDATE_PENDING: u16 = 1;
    }

    // GP_CLI_COMMAND_MOTION, vendor/server/src/map/enums/packet_c2s.h.
    // Emote request: UniqueNo u32, ActIndex u16, Number u8 (emote id), Mode u8,
    // Param u16 (vendor/server/src/map/packets/c2s/0x05d_motion.h GP_CLI_COMMAND_MOTION).
    pub const MOTION: u16 = 0x05D;

    // GP_CLI_COMMAND_EMOTE_LIST, vendor/server/src/map/enums/packet_c2s.h.
    // Header-only request for the job-emote/chair unlock flags; answered by
    // s2c 0x11A (vendor/server/src/map/packets/c2s/0x119_emote_list.h).
    pub const EMOTE_LIST: u16 = 0x119;

    pub const CHAT: u16 = 0x0B5;

    pub const SHOP_BUY: u16 = 0x083;

    // GP_CLI_COMMAND_SHOP_SELL_REQ, vendor/server/src/map/packets/c2s/0x084_shop_sell_req.h.
    // Sell appraisal: ItemNum u32, ItemNo u16, ItemIndex u8 (LOC_INVENTORY slot), padding u8.
    // The server answers with s2c SHOP_SELL (0x03D) carrying the unit sale price.
    pub const SHOP_SELL_REQ: u16 = 0x084;

    // GP_CLI_COMMAND_SHOP_SELL_SET, vendor/server/src/map/packets/c2s/0x085_shop_sell_set.h.
    // Confirms the appraised sale: SellFlag u16, padding u16. Must follow SHOP_SELL_REQ
    // (0x085_shop_sell_set.cpp requiresPriorPacket) and SellFlag must equal 1 (mustEqual).
    pub const SHOP_SELL_SET: u16 = 0x085;

    // GP_CLI_COMMAND_FISHING_2, vendor/server/src/map/enums/packet_c2s.h. The current
    // mini-game uses 0x110; 0x066 (GP_CLI_COMMAND_FISHING) is the pre-overhaul system,
    // aliased to the same struct server-side.
    pub const FISHING_2: u16 = 0x110;
    pub const FISHING: u16 = 0x066;

    pub const EQUIP_SET: u16 = 0x050;

    // GP_CLI_COMMAND_EQUIPSET_SET, vendor/server/src/map/packets/c2s/0x051_equipset_set.h: wear one
    // equipment set whole; the server runs the equip calls (see `equipset::equipset_set_body`).
    pub const EQUIPSET_SET: u16 = 0x051;

    // GP_CLI_COMMAND_EQUIPSET_CHECK, vendor/server/src/map/packets/c2s/0x052_equipset_check.h: one edit to a
    // set's contents, answered by s2c 0x116 EQUIPSET_VALID.
    pub const EQUIPSET_CHECK: u16 = 0x052;

    // GP_CLI_COMMAND_LOCKSTYLE, vendor/server/src/map/packets/c2s/0x053_lockstyle.h: the Mode byte picks the
    // operation; kuluu's set arm sends Mode = Set (`equipset::lockstyle_set_body`).
    pub const LOCKSTYLE: u16 = 0x053;

    // GP_CLI_COMMAND_MYROOM_JOB, vendor/server/src/map/packets/c2s/0x100_myroom_job.h.
    // Mog House job change: MainJobIndex u8, SupportJobIndex u8 (0 = keep current).
    pub const MYROOM_JOB: u16 = 0x100;

    // GP_CLI_COMMAND_MYROOM_IS, vendor/server/src/map/packets/c2s/0x0cb_myroom_is.h.
    // Mog House door + remodel traffic: Kind u8 @4 (GP_CLI_COMMAND_MYROOM_IS_KIND:
    // Open=1, Close=2, Remodel=5), Param1 u8 @5, Param2 u16 @6
    // (GP_CLI_COMMAND_MYROOM_IS_PARAM2: Unk1=0 opening the door, Unk2=1 closing it,
    // 615..618 remodel styles). The DLL's only sender for this opcode is the helper
    // FFXiMain.dll retail-2026-09 .text 0xF0A40, which commits exactly those four
    // body bytes. Body-value constants live in kuluu-session/src/session/codec.rs —
    // build.rs checks every u16 const below for opcode drift.
    pub const MYROOM_IS: u16 = 0x0CB;

    // The Mog House layout/planting family. Every one of them is built by a dedicated helper in
    // FFXiMain.dll retail-2026-09 (`.text 0xF0CC0` Layout, `.text 0xF0C80` the all-zero closing
    // variant, `.text 0xF0D40` Bankin, `.text 0xF0DA0` PlantAdd, `.text 0xF0E10` PlantCheck,
    // `.text 0xF0E70` PlantCrop, `.text 0xF0ED0` PlantStop); the field tables below come from LSB's
    // own packet structs. Vault main_menu.md §59 addendum 30.

    /// GP_CLI_COMMAND_MYROOM_LAYOUT — place/move one piece of furniture, or (all-zero) announce that
    /// layout mode ended (vendor/server/src/map/packets/c2s/0x0fa_myroom_layout.h).
    pub const MYROOM_LAYOUT: u16 = 0x0FA;

    /// GP_CLI_COMMAND_MYROOM_BANKIN — take a placed piece back into the Mog House shelf
    /// (vendor/server/src/map/packets/c2s/0x0fb_myroom_bankin.h).
    pub const MYROOM_BANKIN: u16 = 0x0FB;

    /// GP_CLI_COMMAND_MYROOM_PLANT_ADD — sow a seed or feed a plant
    /// (vendor/server/src/map/packets/c2s/0x0fc_myroom_plant_add.h).
    pub const MYROOM_PLANT_ADD: u16 = 0x0FC;

    /// GP_CLI_COMMAND_MYROOM_PLANT_CHECK — examine a plant
    /// (vendor/server/src/map/packets/c2s/0x0fd_myroom_plant_check.h).
    pub const MYROOM_PLANT_CHECK: u16 = 0x0FD;

    /// GP_CLI_COMMAND_MYROOM_PLANT_CROP — harvest a plant
    /// (vendor/server/src/map/packets/c2s/0x0fe_myroom_plant_crop.h).
    pub const MYROOM_PLANT_CROP: u16 = 0x0FE;

    /// GP_CLI_COMMAND_MYROOM_PLANT_STOP — stop drying/harvesting a plant
    /// (vendor/server/src/map/packets/c2s/0x0ff_myroom_plant_stop.h).
    pub const MYROOM_PLANT_STOP: u16 = 0x0FF;

    // GP_CLI_COMMAND_ITEM_STACK, vendor/server/src/map/packets/c2s/0x03a_item_stack.h.
    // "Sort" for a container: the server consolidates same-id partial stacks.
    // Payload is a single u32 Category = container id (LOC_INVENTORY = 0).
    pub const ITEM_STACK: u16 = 0x03A;

    // GP_CLI_COMMAND_ITEM_MOVE, vendor/server/src/map/packets/c2s/0x029_item_move.h.
    // Moves ItemNum of Category1/ItemIndex1 into Category2; ItemIndex2 < 82 targets
    // a same-id stack merge into that slot, anything larger lets the server pick a
    // free slot (0x029_item_move.cpp process).
    pub const ITEM_MOVE: u16 = 0x029;

    // GP_CLI_COMMAND_ITEM_DUMP, vendor/server/src/map/packets/c2s/0x028_item_dump.h.
    // Throws ItemNum of Category/ItemIndex away (retail "Drop"); inventory drops
    // land in the recycle bin when the server enables it (0x028_item_dump.cpp
    // process).
    pub const ITEM_DUMP: u16 = 0x028;

    // GP_CLI_COMMAND_PBX, vendor/server/src/map/packets/c2s/0x04d_pbx.h.
    // Delivery box sub-protocol; see [`crate::map::pbx`] for the Command bytes.
    pub const PBX: u16 = 0x04D;

    // GP_CLI_COMMAND_SCENARIOITEM, vendor/server/src/map/packets/c2s/
    // 0x064_scenarioitem.h — mark key items seen (full LookItemFlag bitset
    // for one table; the server ORs each set bit).
    pub const SCENARIO_ITEM: u16 = 0x064;

    pub const REQ_LOGOUT: u16 = 0x0E7;

    // vendor/server/src/map/packets/c2s/0x0eb_reqsubmapnum.h GP_CLI_COMMAND_REQSUBMAPNUM.
    pub const REQSUBMAPNUM: u16 = super::event_query_wire::C2S_REQSUBMAPNUM;

    // GP_CLI_COMMAND_SUBMAPCHANGE, vendor/server/src/map/packets/c2s/
    // 0x0f2_submapchange.h. Sent whenever the client crosses into a different
    // sub-area within the current zone (State/SubMapNumber below); the server
    // assigns SubMapNumber to PChar->loc.boundary and saves it, so an unsent
    // change desyncs the next relog's restored interior
    // (0x0f2_submapchange.cpp process).
    pub const SUBMAPCHANGE: u16 = 0x0F2;

    // GP_CLI_COMMAND_TRACKING_LIST, vendor/server/src/map/packets/c2s/0x0f4_tracking_list.h.
    // Wide-scan list request: uint32 SendFlg (must be 1, `crate::map::tracking::SEND_FLG_REQUEST`).
    pub const TRACKING_LIST: u16 = 0x0F4;

    // GP_CLI_COMMAND_TRACKING_START, vendor/server/src/map/packets/c2s/0x0f5_tracking_start.h.
    // Begin tracking one entity: uint32 ActIndex.
    pub const TRACKING_START: u16 = 0x0F5;

    // GP_CLI_COMMAND_TRACKING_END, vendor/server/src/map/packets/c2s/0x0f6_tracking_end.h.
    // Stop tracking: uint32 padding (Dammy).
    pub const TRACKING_END: u16 = 0x0F6;

    // GP_CLI_COMMAND_BAZAAR_EXIT, vendor/server/src/map/packets/c2s/0x104_bazaar_exit.h.
    // Header-only: leaves the bazaar we are browsing. The server clears our
    // BazaarID, so it must be sent before browsing another one
    // (0x105_bazaar_list.cpp validate mustEqual BazaarID.id 0).
    pub const BAZAAR_EXIT: u16 = 0x104;

    // GP_CLI_COMMAND_BAZAAR_LIST, vendor/server/src/map/packets/c2s/0x105_bazaar_list.h.
    // View a PC's bazaar: UniqueNo u32, ActIndex u16, padding u16. Answered by
    // one s2c 0x105 per priced slot.
    pub const BAZAAR_LIST: u16 = 0x105;

    // GP_CLI_COMMAND_BAZAAR_BUY, vendor/server/src/map/packets/c2s/0x106_bazaar_buy.h.
    // Buy from the bazaar we are browsing: BazaarItemIndex u8, padding u8[3],
    // BuyNum u32 (validator range 1..=99).
    pub const BAZAAR_BUY: u16 = 0x106;

    // GP_CLI_COMMAND_BAZAAR_CLOSE, vendor/server/src/map/packets/c2s/0x10b_bazaar_close.h:
    // what the client sends when it ENTERS its own `Set Prices` menu (LSB names the id
    // after XiPackets, not after what it does). AllListClearFlg u32 @4, validated always
    // 0; the handler raises `isSettingBazaarPrices` and clears BazaarCustomers.
    pub const BAZAAR_PRICES_ENTER: u16 = 0x10B;

    // GP_CLI_COMMAND_BAZAAR_ITEMSET, vendor/server/src/map/packets/c2s/0x10a_bazaar_itemset.h:
    // price one LOC_INVENTORY slot of our own stall. ItemIndex u8 @4, padding u8[3],
    // Price u32 @8 (validator range 0..=99_999_999; 0 frees the item again). The server
    // answers ITEM_ATTR + ITEM_SAME for that slot.
    pub const BAZAAR_ITEMSET: u16 = 0x10A;

    // GP_CLI_COMMAND_BAZAAR_OPEN, vendor/server/src/map/packets/c2s/0x109_bazaar_open.h:
    // header only, sent when the client LEAVES its `Set Prices` menu; clears
    // `isSettingBazaarPrices` and raises UPDATE_HP so the stall re-publishes.
    pub const BAZAAR_PRICES_DONE: u16 = 0x109;

    // GP_CLI_COMMAND_INSPECT_MESSAGE, vendor/server/src/map/packets/c2s/0x0de_inspect_message.h:
    // the only writer of our bazaar comment. Body is a fixed uint8[123] buffer (plus one
    // pad byte to fill the last word) and LSB stores at most its first 120 bytes verbatim;
    // there is no ack, and the string comes back to us only as s2c 0x0CA when someone
    // checks us. Not to be confused with c2s 0x0E0 SET_USERMSG (the Seacom seek message).
    pub const INSPECT_MESSAGE: u16 = 0x0DE;

    // GP_CLI_COMMAND_TROPHY_ENTRY, vendor/server/src/map/packets/c2s/0x041_trophy_entry.h.
    // Cast lots on a pool item: TrophyItemIndex u8, PropertyItemIndex u8,
    // padding u8[2]. The server rolls the number itself — the client sends no
    // lot value (0x041_trophy_entry.cpp process).
    pub const TROPHY_ENTRY: u16 = 0x041;

    // GP_CLI_COMMAND_TROPHY_ABSENCE, vendor/server/src/map/packets/c2s/0x042_trophy_absence.h.
    // Pass on a pool item: TrophyItemIndex u8, padding u8.
    pub const TROPHY_ABSENCE: u16 = 0x042;

    // GP_CLI_COMMAND_AUC, vendor/server/src/map/packets/c2s/0x04e_auc.h.
    // Auction house sub-protocol; command bytes: [`crate::decode::AuctionCommand`].
    pub const AUC: u16 = 0x04E;

    // GP_CLI_COMMAND_GROUP_LIST_REQ, vendor/server/src/map/packets/c2s/0x076_group_list_req.h.
    // Requests the full party table; the server answers with GROUP_TBL (0x0C8) +
    // GROUP_LIST (0x0DD) for every member. The retail client sends this on zone-in
    // to guarantee the party HUD gets a clean refresh.
    pub const GROUP_LIST_REQ: u16 = 0x076;

    // GP_CLI_COMMAND_GROUP_LEAVE, vendor/server/src/map/packets/c2s/0x06f_group_leave.h.
    // Leave one's own party/alliance: PartyKind Kind (Party = 0, Alliance = 5).
    pub const GROUP_LEAVE: u16 = 0x06F;

    // GP_CLI_COMMAND_GROUP_BREAKUP, vendor/server/src/map/packets/c2s/0x070_group_breakup.h.
    // Leader disbands the group: PartyKind Kind.
    pub const GROUP_BREAKUP: u16 = 0x070;

    // GP_CLI_COMMAND_GROUP_SOLICIT_RES, vendor/server/src/map/packets/c2s/0x074_group_solicit_res.h.
    // Answer to a pending invite from the party window (prtyjoin YES/NO): Res u8
    // (Decline = 0, Accept = 1) + padding[3].
    pub const GROUP_SOLICIT_RES: u16 = 0x074;

    // GP_CLI_COMMAND_TRADE_REQ, vendor/server/src/map/packets/c2s/0x032_trade_req.h.
    // Ask a target to trade: UniqueNo u32 + ActIndex u16 + padding u16. The server
    // pushes the target s2c ITEM_TRADE_REQ; nothing comes back to us until they answer.
    pub const TRADE_REQ: u16 = 0x032;

    // GP_CLI_COMMAND_TRADE_RES, vendor/server/src/map/packets/c2s/0x033_trade_res.h.
    // Kind u32 ([`crate::decode::TradeResReqKind`]: Start=0/Cancell=1/Make=2/MakeCancel=3)
    // + TradeCounter u16 (the window's refresh counter, s2c 0x23 carries it).
    pub const TRADE_RES: u16 = 0x033;

    // GP_CLI_COMMAND_TRADE_LIST, vendor/server/src/map/packets/c2s/0x034_trade_list.h.
    // Stage one slot of our offer: ItemNum u32 (qty; gil amount when ItemNo is GIL)
    // + ItemNo u16 + ItemIndex u8 (bag slot) + TradeIndex u8 (slot 0 = gil).
    pub const TRADE_LIST: u16 = 0x034;

    // GP_CLI_COMMAND_COMBINE_ASK, vendor/server/src/map/packets/c2s/0x096_combine_ask.h.
    // One synthesis attempt: crystal item id + its bag slot, then up to 8
    // ingredient (id, bag-slot) pairs in the recipe row's own order ([`crate::synth`]).
    pub const COMBINE_ASK: u16 = 0x096;

    // GP_CLI_COMMAND_ALTER_EGO_POINTS, vendor/server/src/map/packets/c2s/0x0c1_alter_ego_points.h:
    // CategoryIndex u16 + padding u16, spending alter ego points on one upgrade id.
    pub const ALTER_EGO_POINTS: u16 = 0x0C1;

    // GP_CLI_COMMAND_UNITY_MENU, vendor/server/src/map/packets/c2s/0x116_unity_menu.h: Kind u32,
    // the ranking result set (0 or 1).
    pub const UNITY_MENU: u16 = 0x116;

    // GP_CLI_COMMAND_UNITY_QUEST, vendor/server/src/map/packets/c2s/0x117_unity_quest.h: Kind u32,
    // which LSB requires to be 0.
    pub const UNITY_QUEST: u16 = 0x117;

    // GP_CLI_COMMAND_UNITY_TOGGLE, vendor/server/src/map/packets/c2s/0x118_unity_toggle.h: Mode u8
    // (GP_CLI_COMMAND_UNITY_TOGGLE_MODE Inactive 0 / Active 1) + 3 padding bytes.
    // Direction sibling of s2c CURRENCIES_2 at 0x118.
    pub const UNITY_TOGGLE: u16 = 0x118;

    // GP_CLI_COMMAND_MERITS, vendor/server/src/map/packets/c2s/0x0be_merits.h: Kind u8, Param1 u8,
    // Param2 u16, Param3 u32 ([`super::merits_wire`] names Kind and Param1).
    pub const MERITS: u16 = 0x0BE;

    // GP_CLI_COMMAND_JOB_POINTS_SPEND, vendor/server/src/map/packets/c2s/0x0bf_job_points_spend.h:
    // Index u16 (the job point type id) + padding00 u16.
    pub const JOB_POINTS_SPEND: u16 = 0x0BF;

    // GP_CLI_COMMAND_JOB_POINTS_REQ, vendor/server/src/map/packets/c2s/0x0c0_job_points_req.h:
    // header only; LSB answers with s2c 0x08D once the character holds JOB_BREAKER.
    pub const JOB_POINTS_REQ: u16 = 0x0C0;
}

// vendor/server/src/map/packets/c2s/0x0be_merits.h GP_CLI_COMMAND_MERITS_KIND / _PARAM1.
pub mod merits_wire {
    include!(concat!(env!("OUT_DIR"), "/merits_wire.rs"));
}

/// Client opcodes the retail client sends that LSB's `PacketC2S` enum does not name
/// (vendor/server/src/map/enums/packet_c2s.h), so build.rs has no upstream id to check them against.
pub mod c2s_unnamed {
    /// The header-only alter ego points request
    /// (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md); LSB has no
    /// handler for it.
    pub const ALTER_EGO_POINTS_REQUEST: u16 = 0x0C2;
}

/// vendor/server/src/map/packets/c2s/0x118_unity_toggle.h GP_CLI_COMMAND_UNITY_TOGGLE_MODE.
pub mod unity_toggle_mode {
    include!(concat!(env!("OUT_DIR"), "/unity_toggle_mode.rs"));
}

/// Wide-scan (tracking) State bytes shared by the s2c 0x0F5/0x0F6 decoders and
/// the c2s 0x0F4 request flag.
pub mod tracking {
    // GP_TRACKING_POS_STATE, vendor/server/src/map/packets/s2c/0x0f5_tracking_pos.h.
    pub mod pos_state {
        pub const NONE: u8 = 0;
        pub const START: u8 = 1;
        pub const LOSE: u8 = 2;
        pub const END: u8 = 3;
    }

    // GP_TRACKING_STATE, vendor/server/src/map/packets/s2c/0x0f6_tracking_state.h.
    pub mod list_state {
        pub const NONE: u8 = 0x00;
        pub const LIST_START: u8 = 0x01;
        pub const LIST_END: u8 = 0x02;
        pub const END: u8 = 0x03;
        pub const ERR_ETC: u8 = 0x0A;
    }

    /// c2s 0x0F4 SendFlg must equal 1 to request the wide-scan list
    /// (vendor/server/src/map/packets/c2s/0x0f4_tracking_list.h SendFlg).
    pub const SEND_FLG_REQUEST: u32 = 1;

    // s2c 0x0F4 packed Type = objtype/2
    // (vendor/server/src/map/packets/s2c/0x0f4_tracking_list.cpp). Retail
    // resolves npc/mob names client-side from the ActIndex; a CHAR entry with
    // no sName is hidden entirely (research/XiPackets world/server/0x00F4).
    pub mod kind {
        pub const CHAR: u8 = 0;
        pub const NPC: u8 = 1;
        pub const MOB: u8 = 2;
    }
}

/// c2s 0x0F2 SUBMAPCHANGE wire vocabulary
/// (vendor/server/src/map/packets/c2s/0x0f2_submapchange.h).
pub mod submap {
    /// GP_CLI_COMMAND_SUBMAPCHANGE_STATE (0x0f2_submapchange.h). `Event` requires
    /// an active `currentEvent` server-side (GP_CLI_COMMAND_SUBMAPCHANGE::validate)
    /// so ordinary free-roam sub-area crossings always send `General`.
    pub mod state {
        pub const GENERAL: u16 = 0x01;
        pub const EVENT: u16 = 0x02;
    }

    /// Retail clamps a negative ("no sub-area") boundary to 0 on the wire.
    pub const NO_SUB_AREA: u16 = 0;
}

/// Delivery box ("post box") wire vocabulary shared by c2s 0x04D and s2c 0x04B.
pub mod pbx {
    // GP_CLI_COMMAND_PBX_COMMAND, vendor/server/src/map/packets/c2s/0x04d_pbx.h.
    pub mod command {
        pub const WORK: u8 = 0x01;
        pub const SET: u8 = 0x02;
        pub const SEND: u8 = 0x03;
        pub const CANCEL: u8 = 0x04;
        pub const CHECK: u8 = 0x05;
        pub const RECV: u8 = 0x06;
        pub const CONFIRM: u8 = 0x07;
        pub const ACCEPT: u8 = 0x08;
        pub const REJECT: u8 = 0x09;
        pub const GET: u8 = 0x0A;
        pub const CLEAR: u8 = 0x0B;
        pub const QUERY: u8 = 0x0C;
        pub const DELI_OPEN: u8 = 0x0D;
        pub const POST_OPEN: u8 = 0x0E;
        pub const POST_CLOSE: u8 = 0x0F;
    }

    // GP_CLI_COMMAND_PBX_BOXNO, vendor/server/src/map/packets/c2s/0x04d_pbx.h.
    pub mod boxno {
        pub const NONE: i8 = -1;
        pub const INCOMING: i8 = 1;
        pub const OUTGOING: i8 = 2;
    }

    // s2c 0x04B Result byte, vendor/server/src/map/packets/s2c/0x04b_pbx_result.cpp
    // and utils/dboxutils.cpp push sites. OK/PENDING form the dual-push pattern
    // (the server answers most commands twice: Result=PENDING then Result=OK).
    pub mod result {
        pub const OK: u8 = 0x01;
        pub const PENDING: u8 = 0x02;
        /// TakeItemFromCell with a full inventory (dboxutils.cpp dboxutils::TakeItemFromCell).
        pub const INVENTORY_FULL: u8 = 0xB9;
        /// TakeItemFromCell transaction failure (dboxutils.cpp std::runtime_error).
        pub const TAKE_FAILED: u8 = 0xBA;
        /// SendNewItems/ReturnToSender transaction failure (dboxutils.cpp std::runtime_error,616).
        pub const DB_ERROR: u8 = 0xEB;
        /// ConfirmNameBeforeSending: recipient account not found (dboxutils.cpp dboxutils::ConfirmNameBeforeSending).
        pub const NO_SUCH_CHAR: u8 = 0xFB;
        /// Recipient's inflight queue at capacity (dboxutils.cpp dboxutils::SendConfirmation,582-583).
        pub const RECIPIENT_FULL: u8 = 0xFE;
        /// CancelSendingItem fallback, pushed as -1: "Delivery orders are
        /// currently backlogged." (dboxutils.cpp std::runtime_error).
        pub const BACKLOGGED: u8 = 0xFF;
    }

    // GP_POST_BOX_STATE::Stat values, vendor/server/src/map/packets/s2c/
    // 0x04b_pbx_result.cpp GP_SERV_COMMAND_PBX_RESULT::GP_SERV_COMMAND_PBX_RESULT.
    pub mod stat {
        /// Outgoing item staged in a slot, not yet dispatched (Set).
        pub const STAGED: u32 = 0x01;
        /// Outgoing item dispatched (Send / isSent).
        pub const SENT: u32 = 0x03;
        /// Cancel with message 0x02 (partial cancel state).
        pub const CANCEL_PENDING: u32 = 0x04;
        /// Outgoing item whose dispatch was canceled (back to sendable).
        pub const CANCELED: u32 = 0x05;
        /// Incoming item sitting in the inbox.
        pub const INCOMING: u32 = 0x07;
    }

    /// PostWorkNo slot range on both boxes (LSB PacketValidator range 0..8).
    pub const SLOT_COUNT: usize = 8;
}

// LSB CONTAINER_ID, vendor/server/src/map/item_container.h CONTAINER_ID LOC_INVENTORY.
pub mod container {
    pub const LOC_INVENTORY: u8 = 0;
    pub const LOC_MOGSAFE: u8 = 1;
    pub const LOC_STORAGE: u8 = 2;
    pub const LOC_TEMPITEMS: u8 = 3;
    pub const LOC_MOGLOCKER: u8 = 4;
    pub const LOC_MOGSATCHEL: u8 = 5;
    pub const LOC_MOGSACK: u8 = 6;
    pub const LOC_MOGCASE: u8 = 7;
    pub const LOC_WARDROBE: u8 = 8;
    pub const LOC_MOGSAFE2: u8 = 9;
    pub const LOC_WARDROBE2: u8 = 10;
    pub const LOC_WARDROBE3: u8 = 11;
    pub const LOC_WARDROBE4: u8 = 12;
    pub const LOC_WARDROBE5: u8 = 13;
    pub const LOC_WARDROBE6: u8 = 14;
    pub const LOC_WARDROBE7: u8 = 15;
    pub const LOC_WARDROBE8: u8 = 16;
    pub const LOC_RECYCLEBIN: u8 = 17;

    /// vendor/server/src/map/item_container.h CONTAINER_ID MAX_CONTAINER_ID.
    pub const MAX_CONTAINER_ID: u8 = LOC_RECYCLEBIN + 1;

    /// The container ids a c2s packet may name
    /// (vendor/server/src/map/packets/c2s/validation.cpp
    /// PacketValidator::isValidContainer).
    pub fn is_valid(id: u8) -> bool {
        id < MAX_CONTAINER_ID
    }

    /// Retail bag names as the item window shows them.
    pub fn name(id: u8) -> Option<&'static str> {
        Some(match id {
            LOC_INVENTORY => "Inventory",
            LOC_MOGSAFE => "Mog Safe",
            LOC_STORAGE => "Storage",
            LOC_TEMPITEMS => "Temporary",
            LOC_MOGLOCKER => "Mog Locker",
            LOC_MOGSATCHEL => "Mog Satchel",
            LOC_MOGSACK => "Mog Sack",
            LOC_MOGCASE => "Mog Case",
            LOC_WARDROBE => "Mog Wardrobe",
            LOC_MOGSAFE2 => "Mog Safe 2",
            LOC_WARDROBE2 => "Mog Wardrobe 2",
            LOC_WARDROBE3 => "Mog Wardrobe 3",
            LOC_WARDROBE4 => "Mog Wardrobe 4",
            LOC_WARDROBE5 => "Mog Wardrobe 5",
            LOC_WARDROBE6 => "Mog Wardrobe 6",
            LOC_WARDROBE7 => "Mog Wardrobe 7",
            LOC_WARDROBE8 => "Mog Wardrobe 8",
            LOC_RECYCLEBIN => "Recycle Bin",
            _ => return None,
        })
    }

    /// Only equipment/weapons may enter a wardrobe
    /// (vendor/server/src/map/packets/c2s/0x029_item_move.cpp isValidMovement).
    pub fn is_wardrobe(id: u8) -> bool {
        id == LOC_WARDROBE || (LOC_WARDROBE2..=LOC_WARDROBE8).contains(&id)
    }
}

// LSB ITEMID::GIL, vendor/server/src/map/items.h — never movable between
// containers (0x029_item_move.cpp isValidMovement).
pub const GIL_ITEM_NO: u16 = 65535;

pub mod reqlogout {

    pub mod mode {

        pub const TOGGLE: u16 = 0x00;

        pub const LOGOUT_ON: u16 = 0x01;

        pub const OFF: u16 = 0x02;

        pub const SHUTDOWN_ON: u16 = 0x03;
    }

    pub mod kind {

        pub const LOGOUT: u16 = 0x01;

        pub const SHUTDOWN: u16 = 0x03;
    }
}

pub mod action_id {
    pub const TALK: u16 = 0x00;
    pub const ATTACK: u16 = 0x02;
    pub const CAST_MAGIC: u16 = 0x03;
    pub const ATTACK_OFF: u16 = 0x04;
    pub const HELP: u16 = 0x05;
    pub const WEAPONSKILL: u16 = 0x07;
    pub const JOB_ABILITY: u16 = 0x09;
    pub const ASSIST: u16 = 0x0C;
    pub const FISH: u16 = 0x0E;
    pub const CHANGE_TARGET: u16 = 0x0F;
    pub const SHOOT: u16 = 0x10;
}

pub mod eventucoff_mode {
    // GP_SERV_COMMAND_EVENTUCOFF_MODE, vendor/server/src/map/packets/s2c/0x052_eventucoff.h.
    // CancelEvent packs the cancelled event id in the high bits
    // (0x052_eventucoff.cpp GP_SERV_COMMAND_EVENTUCOFF::GP_SERV_COMMAND_EVENTUCOFF), so match on the low byte.
    pub const MODE_MASK: u32 = 0xFF;
    /// The ack both c2s event-end process() functions push after handling a
    /// tag; clears the client's pending flag (0x05b_eventend.cpp,
    /// 0x05c_eventendxzy.cpp GP_CLI_COMMAND_EVENTEND*::process).
    pub const EVENT_RECV_PENDING: u32 = 1;
    pub const CANCEL_EVENT: u32 = 2;
    pub const FISHING: u32 = 4;
}

/// Emote wire vocabulary shared by c2s 0x05D MOTION and s2c 0x05A MOTIONMES.
pub mod emote {
    // EmoteMode, vendor/server/src/map/enums/emote.h.
    pub mod mode {
        pub const ALL: u8 = 0;
        pub const TEXT: u8 = 1;
        pub const MOTION: u8 = 2;
    }

    // Emote ids referenced by name in code; the full table is the build-time
    // scrape of vendor/server/src/map/enums/emote.h
    // (`ffxi_vocab::emote_names::EMOTES`), and `pinned_ids_match_scraped_table`
    // guards these against it.
    pub const BELL: u8 = 73;
    pub const JOB: u8 = 74;
    /// Server-initiated only ("Only used for HELM", emote.h) — no retail slash
    /// command, so keep them out of slash aliases and the emote-list menu.
    pub const HELM_ONLY: [u8; 3] = [40, 41, 42];

    /// /bell note Param range (vendor/server/src/map/packets/c2s/
    /// 0x05d_motion.cpp GP_CLI_COMMAND_MOTION::process: `Param < 0x06 || Param > 0x1e` is rejected).
    pub const BELL_NOTE_MIN: u16 = 0x06;
    pub const BELL_NOTE_MAX: u16 = 0x1E;

    /// /jobemote Param = job id + 0x1E, so WAR(1) → 0x1F
    /// (vendor/server/src/map/packets/c2s/0x05d_motion.cpp GP_CLI_COMMAND_MOTION::process checks
    /// `jobs.unlocked & (1 << (Param - 0x1E))`).
    pub const JOB_PARAM_BASE: u16 = 0x1F;

    /// s2c MesNum for a job emote = 74 + (Param - 0x1F), giving 74..=95
    /// (vendor/server/src/map/packets/s2c/0x05a_motionmes.cpp GP_SERV_COMMAND_MOTIONMES::GP_SERV_COMMAND_MOTIONMES; the 22-job
    /// span WAR..RUN mirrors the 0x11A jobemotes_t bitfield).
    pub const JOB_MESNUM_BASE: u16 = 74;
    pub const JOB_MESNUM_MAX: u16 = 95;
}

pub mod chat_kind {
    pub const SAY: u8 = 0;
    pub const SHOUT: u8 = 1;
    pub const TELL: u8 = 3;
    pub const PARTY: u8 = 4;
    pub const LINKSHELL: u8 = 5;
    pub const SYSTEM_1: u8 = 6;
    pub const SYSTEM_2: u8 = 7;
    pub const EMOTION: u8 = 8;
    pub const NS_SAY: u8 = 13;
    pub const NS_SHOUT: u8 = 14;
    pub const NS_PARTY: u8 = 15;
    pub const NS_LINKSHELL: u8 = 16;
    pub const YELL: u8 = 26;
    pub const LINKSHELL2: u8 = 27;
    pub const NS_LINKSHELL2: u8 = 28;
    pub const SYSTEM_3: u8 = 29;
}

pub mod s2c {
    pub const ENTERZONE: u16 = 0x008;
    pub const MESSAGE: u16 = 0x009;
    pub const LOGIN: u16 = 0x00A;
    pub const LOGOUT: u16 = 0x00B;
    pub const CHAR_PC: u16 = 0x00D;
    pub const CHAR_NPC: u16 = 0x00E;

    pub const CHAR_STATUS: u16 = 0x037;

    // GP_SERV_COMMAND_JOB_INFO, vendor/server/src/map/packets/s2c/0x01b_job_info.h.
    // Per-job levels + unlocked-jobs bitmask for the self character.
    pub const JOB_INFO: u16 = 0x01B;

    // GP_SERV_COMMAND_OPENMOGMENU, vendor/server/src/map/packets/s2c/0x02e_openmogmenu.h.
    // Header-only: tells the client to open the Mog House menu.
    pub const OPENMOGMENU: u16 = 0x02E;

    // GP_SERV_COMMAND_CLISTATUS, vendor/server/src/map/packets/s2c/0x061_clistatus.h.
    // Self-character stat block: HP/MP max, base+gear stats, attack/defense, resists, iLv.
    pub const CLISTATUS: u16 = 0x061;

    // GP_SERV_COMMAND_CLISTATUS2, vendor/server/src/map/packets/s2c/0x062_clistatus2.h.
    // The skill block behind the Status window's Combat/Magic/Craft rows:
    // CommandRecast[31] then skill_base[64], indexed by skill id.
    pub const CLISTATUS2: u16 = 0x062;

    // GP_SERV_COMMAND_FISH, vendor/server/src/map/enums/packet_s2c.h. Sent to start
    // the fishing mini-game with the hooked fish's stats.
    pub const FISH: u16 = 0x115;

    // GP_SERV_COMMAND_CURRENCIES_1 / _2, vendor/server/src/map/packets/s2c/
    // 0x113_currencies_1.h + 0x118_currencies_2.h: the currency reports answering the
    // header-only c2s requests above. Kuluu keeps them RAW — the client-side getter law
    // (`kuluu_render::hud::currency_law`) reads fields out of the block directly, exactly
    // as the DLL does; field layout cross-check is in vault main_menu.md §59 addendum 27.
    pub const CURRENCIES_1: u16 = 0x113;
    pub const CURRENCIES_2: u16 = 0x118;

    pub const CHAT: u16 = 0x017;

    // GP_SERV_COMMAND_MOTIONMES, vendor/server/src/map/enums/packet_s2c.h.
    // Emote broadcast (vendor/server/src/map/packets/s2c/0x05a_motionmes.h).
    pub const MOTIONMES: u16 = 0x05A;

    // GP_SERV_COMMAND_ASSIST, vendor/server/src/map/enums/packet_s2c.h.
    // Server-pushed retarget: the answer to a c2s ASSIST request and to every
    // other server-side target change (vendor/server/src/map/packets/s2c/
    // 0x058_assist.h, [`crate::decode::Assist`]).
    pub const ASSIST: u16 = 0x058;

    // GP_SERV_COMMAND_EMOTE_LIST, vendor/server/src/map/enums/packet_s2c.h.
    // Job-emote (u32) + chair (u16) unlock bitfields
    // (vendor/server/src/map/packets/s2c/0x11a_emote_list.h).
    pub const EMOTE_LIST: u16 = 0x11A;

    pub const ITEM_MAX: u16 = 0x01C;

    pub const ITEM_SAME: u16 = 0x01D;

    pub const ITEM_NUM: u16 = 0x01E;

    pub const ITEM_LIST: u16 = 0x01F;

    pub const ITEM_ATTR: u16 = 0x020;

    // GP_SERV_COMMAND_ITEM_TRADE_REQ, vendor/server/src/map/packets/s2c/0x021_item_trade_req.h.
    // Someone asked to trade: UniqueNo + ActIndex. Answer with c2s TRADE_RES (0x033).
    pub const ITEM_TRADE_REQ: u16 = 0x021;

    // GP_SERV_COMMAND_ITEM_TRADE_RES, vendor/server/src/map/packets/s2c/0x022_item_trade_res.h.
    // Trade state changes: Kind sees [`crate::decode::TradeResKind`] (Start opens both
    // windows after the target accepted; End closes them after commit).
    pub const ITEM_TRADE_RES: u16 = 0x022;

    // GP_SERV_COMMAND_ITEM_TRADE_LIST, vendor/server/src/map/packets/s2c/0x023_item_trade_list.h.
    // One slot of the PARTNER's trade offer refreshed (TradeIndex 0 is their gil).
    pub const ITEM_TRADE_LIST: u16 = 0x023;

    // GP_SERV_COMMAND_ITEM_TRADE_MYLIST, vendor/server/src/map/packets/s2c/0x025_item_trade_mylist.h.
    // One slot of MY trade offer refreshed (server echo after c2s TRADE_LIST).
    pub const ITEM_TRADE_MYLIST: u16 = 0x025;

    // GP_SERV_COMMAND_COMBINE_ANS, vendor/server/src/map/packets/s2c/0x06f_combine_ans.h.
    // The answer to a synthesis ask (result code, gained item, broken
    // ingredients; [`crate::synth::parse_combine_ans`]).
    pub const COMBINE_ANS: u16 = 0x06F;
    // GP_SERV_COMMAND_MISSION, vendor/server/src/map/packets/s2c/0x056_mission.h;
    // vault main_menu.md section 30.
    pub const MISSION: u16 = 0x056;

    // GP_SERV_COMMAND_CONFIG — the server's copy of the client option flags
    // (SAVE_CONF prefix; vendor/server/src/map/packets/s2c/0x0b4_config.h).
    // Answered back after every c2s CONFIG, on login gameok, and on party law
    // pushes (party.cpp) [V].
    pub const CONFIG: u16 = 0x0B4;

    // GP_SERV_COMMAND_EXTENDED_JOB — BLU/PUP/MON extended info
    // (vendor/server/src/map/packets/s2c/0x044_extended_job_pup.h and friends;
    // vault main_menu.md §39). Parse with [`crate::puppet::parse_extended_job`].
    pub const EXTENDED_JOB: u16 = 0x044;

    pub const BATTLE2: u16 = 0x028;

    pub const BATTLE_MESSAGE: u16 = 0x029;
    pub const EVENT: u16 = 0x032;
    pub const EVENTSTR: u16 = 0x033;
    pub const EVENTNUM: u16 = 0x034;

    pub const BATTLE_MESSAGE2: u16 = 0x02D;

    pub const SHOP_LIST: u16 = 0x03C;

    // GP_SERV_COMMAND_SHOP_SELL, vendor/server/src/map/packets/s2c/0x03d_shop_sell.h.
    // Reply to c2s SHOP_SELL_REQ: Price u32 (per unit), PropertyItemIndex u8 (inventory
    // slot), Type u8 (0 = appraisal, 0x03d_shop_sell.cpp), padding u16, Count u32.
    pub const SHOP_SELL: u16 = 0x03D;

    pub const SHOP_OPEN: u16 = 0x03E;

    // GP_SERV_COMMAND_PBX_RESULT, vendor/server/src/map/packets/s2c/0x04b_pbx_result.h.
    // Delivery box reply; short (0x14) or full (0x58, with item payload) form.
    pub const PBX_RESULT: u16 = 0x04B;

    // GP_SERV_COMMAND_AUC, vendor/server/src/map/packets/s2c/0x04c_auc.h.
    // Auction house reply, same body layout as c2s AUC
    // ([`crate::decode::Auction`]).
    pub const AUC: u16 = 0x04C;

    pub const MISCDATA: u16 = 0x063;

    // GP_SERV_PACKET_ALTER_EGO_POINTS, vendor/server/src/map/packets/s2c/0x08e_alter_ego_points.h.
    // The alter ego point balance with each upgrade's level and cost ([`crate::decode::AlterEgoPoints`]).
    pub const ALTER_EGO_POINTS: u16 = 0x08E;

    // GP_SERV_COMMAND_UNITY, vendor/server/src/map/packets/s2c/0x110_unity.h
    // ([`crate::decode::UnityQuest`]). Direction sibling of c2s FISHING_2 at 0x110.
    pub const UNITY: u16 = 0x110;

    // GP_SERV_COMMAND_MERIT, vendor/server/src/map/packets/s2c/0x08c_merit.h
    // ([`crate::decode::MeritList`]).
    pub const MERIT: u16 = 0x08C;

    // GP_SERV_COMMAND_JOB_POINTS, vendor/server/src/map/packets/s2c/0x08d_job_points.h
    // ([`crate::decode::JobPointList`]).
    pub const JOB_POINTS: u16 = 0x08D;

    pub const SYSTEMMES: u16 = 0x053;

    // GP_SERV_COMMAND_EVENTUCOFF, vendor/server/src/map/packets/s2c/0x052_eventucoff.h.
    // Mode 4 (Fishing) releases the fishing event lock — sent on a rejected cast or at the
    // end of fishing.
    pub const EVENTUCOFF: u16 = 0x052;

    // GP_SERV_COMMAND_TALKNUMWORK, vendor/server/src/map/packets/s2c/
    // 0x02a_talknumwork.h — zone-dialog message (lua messageSpecial) with
    // numeric params; MesNum indexes the zone dialog DAT.
    pub const TALKNUMWORK: u16 = 0x02A;

    // The rest of the TALKNUM family — same zone dialog DAT lookup, different
    // parameter payloads. LSB routes every fishing line through these
    // (vendor/server/src/map/utils/fishingutils.cpp).
    //
    // GP_SERV_COMMAND_TALKNUM, .../s2c/0x036_talknum.h — bare message id.
    pub const TALKNUM: u16 = 0x036;
    // GP_SERV_COMMAND_TALKNUMWORK2, .../s2c/0x027_talknumwork2.h — message id
    // plus 4+8 numeric params and two string params.
    pub const TALKNUMWORK2: u16 = 0x027;
    // GP_SERV_COMMAND_TALKNUMNAME, .../s2c/0x043_talknumname.h — message id
    // plus a 16-byte name.
    pub const TALKNUMNAME: u16 = 0x043;

    pub const SCENARIO_ITEM: u16 = 0x055;

    // vendor/server/src/map/packets/s2c/0x057_weather.h
    pub const WEATHER: u16 = 0x057;

    pub const MUSIC: u16 = 0x05F;

    pub const MUSIC_VOLUME: u16 = 0x060;
    pub const EQUIP_CLEAR: u16 = 0x04F;
    pub const EQUIP_LIST: u16 = 0x050;
    pub const GRAP_LIST: u16 = 0x051;

    pub const WPOS: u16 = 0x05B;

    // These event opcodes are wire packet numbers, one namespace per direction:
    // c2s EVENT_END/EVENT_END_XZY (0x05B/0x05C) and s2c PENDINGNUM/PENDINGSTR
    // (0x05C/0x05D). The cross-direction hex collisions are intentional, and the
    // values are unrelated to the in-script bytecode opcodes OP_LOADEXTSCHEDULER
    // (0x5B) / OP_LOADEXTSCHEDULER2 (0x66) that ffxi-event::vm executes.

    // GP_SERV_PENDINGNUM, research/XiPackets/world/server/0x005C: int32 num[8]
    // the client copies into Work_Zone from index 2. The opcode number collides
    // with c2s EVENT_END_XZY; different directions, intentional.
    pub const PENDINGNUM: u16 = 0x05C;

    // GP_SERV_PENDINGSTR (repurposed), research/XiPackets/world/server/0x005D:
    // int32 num[9] the client ignores plus four 16-byte strings copied into
    // PTR_EventStrings. The opcode number collides with c2s MOTION; different
    // directions, intentional.
    pub const PENDINGSTR: u16 = 0x05D;

    pub const WPOS2: u16 = 0x065;

    pub const MAGIC_DATA: u16 = 0x0AA;

    pub const COMMAND_DATA: u16 = 0x0AC;

    pub const ABIL_RECAST: u16 = 0x119;
    pub const ENTITY_UPDATE1: u16 = 0x067;
    pub const ENTITY_UPDATE2: u16 = 0x068;

    // GP_SERV_COMMAND_GROUP_TBL, vendor/server/src/map/packets/s2c/0x0c8_group_tbl.h.
    // Party definition: member IDs, targids, party numbers, zones, leader flags.
    // Sent before every GROUP_LIST burst as a "here is the full party roster" header.
    // The client should clear its party table on receipt and repopulate from the
    // subsequent GROUP_LIST (0x0DD) packets.
    pub const GROUP_TBL: u16 = 0x0C8;

    pub const GROUP_LIST: u16 = 0x0DD;

    pub const GROUP_ATTR: u16 = 0x0DF;

    // GP_SERV_COMMAND_GROUP_SOLICIT_REQ, vendor/server/src/map/packets/s2c/0x0dc_group_solicit_req.h.
    // Incoming party/alliance invite; answered with c2s GROUP_SOLICIT_RES (0x074).
    pub const GROUP_SOLICIT_REQ: u16 = 0x0DC;

    // GP_SERV_COMMAND_GROUP_SOLICIT_NO, vendor/server/src/map/packets/s2c/0x0de_group_solicit_no.h.
    // The server withdrawing/noting a failed solicit; the client drops its pending
    // invite window state (retail closes `prtyjoin` and flags `alarm`).
    pub const GROUP_SOLICIT_NO: u16 = 0x0DE;

    // GP_SERV_COMMAND_EQUIP_INSPECT, vendor/server/src/map/enums/packet_s2c.h.
    // /check answer for a PC target: EQUIPMENT batches then one GENERAL packet
    // (vendor/server/src/map/packets/c2s/0x0dd_equip_inspect.cpp GP_CLI_COMMAND_EQUIP_INSPECT::process).
    pub const EQUIP_INSPECT: u16 = 0x0C9;

    // GP_SERV_COMMAND_INSPECT_MESSAGE, vendor/server/src/map/packets/s2c/0x0ca_inspect_message.h.
    // The checked PC's bazaar message and title; pushed just ahead of the 0x0C9
    // batches and carrying no target id of its own.
    pub const INSPECT_MESSAGE: u16 = 0x0CA;

    // GP_SERV_COMMAND_BAZAAR_LIST, vendor/server/src/map/packets/s2c/0x105_bazaar_list.h.
    // One priced row of the bazaar we are browsing; re-pushed per row after each
    // purchase (0x106_bazaar_buy.cpp GP_CLI_COMMAND_BAZAAR_BUY::process).
    pub const BAZAAR_LIST: u16 = 0x105;

    // GP_SERV_COMMAND_BAZAAR_BUY, vendor/server/src/map/packets/s2c/0x106_bazaar_buy.h.
    // Result of our c2s 0x106 purchase attempt (OK / ERR / END).
    pub const BAZAAR_BUY: u16 = 0x106;

    // GP_SERV_COMMAND_BAZAAR_CLOSE, vendor/server/src/map/packets/s2c/0x107_bazaar_close.h.
    // The browsed bazaar emptied (its last priced row sold) or closed.
    pub const BAZAAR_CLOSE: u16 = 0x107;

    // GP_SERV_COMMAND_BAZAAR_SELL, vendor/server/src/map/packets/s2c/0x109_bazaar_sell.h.
    // Another customer bought a row of the bazaar we are browsing.
    pub const BAZAAR_SELL: u16 = 0x109;

    // GP_SERV_COMMAND_GROUP_COMLINK, vendor/server/src/map/packets/s2c/0x0e0_group_comlink.h.
    // Which inventory item is now the pearl equipped in linkshell slot 1 or 2; pushed for both
    // slots at login and after every equip/unequip/create (vault main_menu.md §27).
    pub const GROUP_COMLINK: u16 = 0x0E0;

    // vendor/server/src/map/packets/s2c/0x10e_reqsubmapnum.h GP_SERV_COMMAND_REQSUBMAPNUM.
    pub const REQSUBMAPNUM: u16 = super::event_query_wire::S2C_REQSUBMAPNUM;

    // vendor/server/src/map/packets/s2c/0x0bf_registration.h GP_SERV_COMMAND_REGISTRATION.
    pub const REGISTRATION: u16 = super::event_query_wire::S2C_REGISTRATION;

    // GP_SERV_COMMAND_TRACKING_LIST, vendor/server/src/map/packets/s2c/0x0f4_tracking_list.h.
    // One wide-scan entry (ActIndex/Level/Type + relative x/z + sName[16]).
    pub const TRACKING_LIST: u16 = 0x0F4;

    // GP_SERV_COMMAND_TRACKING_POS, vendor/server/src/map/packets/s2c/0x0f5_tracking_pos.h.
    // Tracked-entity absolute position update (State Start/Lose).
    pub const TRACKING_POS: u16 = 0x0F5;

    // GP_SERV_COMMAND_TRACKING_STATE, vendor/server/src/map/packets/s2c/0x0f6_tracking_state.h.
    // Wide-scan list framing (ListStart/ListEnd).
    pub const TRACKING_STATE: u16 = 0x0F6;

    // GP_SERV_COMMAND_RES, vendor/server/src/map/enums/packet_s2c.h.
    // Updates the death menu to home-point-only, Raise/Reraise, or Tractor
    // (vendor/server/src/map/packets/s2c/0x0f9_res.h).
    pub const DEATH_MENU: u16 = 0x0F9;

    // GP_SERV_COMMAND_TROPHY_LIST, vendor/server/src/map/enums/packet_s2c.h.
    // One item (and/or gil) entering the treasure pool
    // (vendor/server/src/map/packets/s2c/0x0d2_trophy_list.h).
    pub const TROPHY_LIST: u16 = 0x0D2;

    // GP_SERV_COMMAND_TROPHY_SOLUTION, vendor/server/src/map/enums/packet_s2c.h.
    // An action taken against a pool item: lot, win, or loss
    // (vendor/server/src/map/packets/s2c/0x0d3_trophy_solution.h).
    pub const TROPHY_SOLUTION: u16 = 0x0D3;
}

#[cfg(test)]
mod tests {
    /// The Unity / alter ego point opcodes are LSB's own enum entries, and 0x0C2 is the one
    /// the record names with no LSB entry at all.
    #[test]
    fn unity_and_alter_ego_opcodes_match_lsb_packet_enums() {
        use crate::packet_names::{c2s_name, s2c_name};
        assert_eq!(s2c_name(super::s2c::UNITY), Some("UNITY"));
        assert!(
            s2c_name(super::s2c::ALTER_EGO_POINTS).is_some_and(|n| n.ends_with("ALTER_EGO_POINTS"))
        );
        assert_eq!(
            c2s_name(super::c2s::ALTER_EGO_POINTS),
            Some("ALTER_EGO_POINTS")
        );
        assert_eq!(c2s_name(super::c2s_unnamed::ALTER_EGO_POINTS_REQUEST), None);
        assert_eq!(c2s_name(super::c2s::UNITY_MENU), Some("UNITY_MENU"));
        assert_eq!(c2s_name(super::c2s::UNITY_QUEST), Some("UNITY_QUEST"));
        assert_eq!(c2s_name(super::c2s::UNITY_TOGGLE), Some("UNITY_TOGGLE"));
    }

    #[test]
    fn merit_and_job_point_opcodes_match_lsb_packet_enums() {
        use crate::packet_names::{c2s_name, s2c_name};
        assert_eq!(s2c_name(super::s2c::MERIT), Some("MERIT"));
        assert_eq!(s2c_name(super::s2c::JOB_POINTS), Some("JOB_POINTS"));
        assert_eq!(c2s_name(super::c2s::MERITS), Some("MERITS"));
        assert_eq!(
            c2s_name(super::c2s::JOB_POINTS_SPEND),
            Some("JOB_POINTS_SPEND")
        );
        assert_eq!(c2s_name(super::c2s::JOB_POINTS_REQ), Some("JOB_POINTS_REQ"));
    }

    /// The record's c2s 0x0BE bodies: ChangeMode is Kind 2 with Param1 0 (EXP) or 1 (Limit
    /// Points), EditMode is Kind 3 with Param1 1 (raise) or 0 (lower)
    /// (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md). The
    /// scraped LSB enums agree.
    #[test]
    fn merits_kind_and_param1_match_the_record() {
        use super::merits_wire::{kind, param1};
        const RECORD_PINNED_CHANGE_MODE: u8 = 2;
        const RECORD_PINNED_EDIT_MODE: u8 = 3;
        const RECORD_PINNED_LOWER_OR_EXP: u8 = 0;
        const RECORD_PINNED_RAISE_OR_LIMIT: u8 = 1;
        assert_eq!(kind::CHANGE_MODE, RECORD_PINNED_CHANGE_MODE);
        assert_eq!(kind::EDIT_MODE, RECORD_PINNED_EDIT_MODE);
        assert_eq!(param1::LOWER, RECORD_PINNED_LOWER_OR_EXP);
        assert_eq!(param1::RAISE, RECORD_PINNED_RAISE_OR_LIMIT);
    }

    /// The record's c2s 0x118 Mode: the Unity chat panel's item 1 sends 1 (on), item 2 sends 0
    /// (off) (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md). The
    /// scraped LSB enum agrees.
    #[test]
    fn unity_toggle_modes_match_the_record() {
        use super::unity_toggle_mode::{ACTIVE, INACTIVE};
        const RECORD_PINNED_ACTIVE: u8 = 1;
        const RECORD_PINNED_INACTIVE: u8 = 0;
        assert_eq!(ACTIVE, RECORD_PINNED_ACTIVE);
        assert_eq!(INACTIVE, RECORD_PINNED_INACTIVE);
    }

    #[test]
    fn pinned_emote_ids_match_scraped_table() {
        use super::emote;
        assert_eq!(ffxi_vocab::emote_names::lookup(emote::BELL), Some("Bell"));
        assert_eq!(ffxi_vocab::emote_names::lookup(emote::JOB), Some("Job"));
        assert_eq!(
            emote::HELM_ONLY.map(ffxi_vocab::emote_names::lookup),
            [Some("Logging"), Some("Excavation"), Some("Harvesting")]
        );
    }

    /// vendor/server/src/map/packets/s2c/0x05a_motionmes.cpp GP_SERV_COMMAND_MOTIONMES::GP_SERV_COMMAND_MOTIONMES —
    /// `MesNum = Emote::Job + (extra - 0x1F)`, so the s2c job-emote span must
    /// start at the scraped Emote::Job id and cover the 22-job jobemotes_t
    /// width (WAR..RUN, 0x11a_emote_list.h).
    #[test]
    fn job_mesnum_rebase_matches_lsb_broadcast() {
        use super::emote;
        assert_eq!(
            emote::JOB_MESNUM_BASE,
            u16::from(emote::JOB),
            "rebase starts at Emote::Job itself (WAR Param 0x1F adds 0)"
        );
        let run_job_id: u16 = 22;
        let run_param = emote::JOB_PARAM_BASE + run_job_id - 1;
        assert_eq!(
            emote::JOB_MESNUM_BASE + (run_param - emote::JOB_PARAM_BASE),
            emote::JOB_MESNUM_MAX,
            "RUN (job 22, top jobemotes_t bit) lands on JOB_MESNUM_MAX"
        );
    }
}

// vendor/server/src/map/packets/c2s/0x05c_eventendxzy.h GP_CLI_COMMAND_EVENTENDXZY.
pub mod event_position_wire {
    include!(concat!(env!("OUT_DIR"), "/event_position_wire.rs"));
}
