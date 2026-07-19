pub const MAP_PORT: u16 = 54230;

pub const MAX_DATAGRAM: usize = 2500;

pub mod c2s {
    pub const LOGIN: u16 = 0x00A;

    pub const GAMEOK: u16 = 0x00C;
    pub const NETEND: u16 = 0x00D;
    pub const ZONE_TRANSITION: u16 = 0x011;
    pub const POS: u16 = 0x015;

    pub const ACTION: u16 = 0x01A;
    pub const EVENT_END: u16 = 0x05B;

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

    // GP_CLI_COMMAND_MYROOM_JOB, vendor/server/src/map/packets/c2s/0x100_myroom_job.h.
    // Mog House job change: MainJobIndex u8, SupportJobIndex u8 (0 = keep current).
    pub const MYROOM_JOB: u16 = 0x100;

    // GP_CLI_COMMAND_ITEM_STACK, vendor/server/src/map/packets/c2s/0x03a_item_stack.h.
    // "Sort" for a container: the server consolidates same-id partial stacks.
    // Payload is a single u32 Category = container id (LOC_INVENTORY = 0).
    pub const ITEM_STACK: u16 = 0x03A;

    // GP_CLI_COMMAND_ITEM_MOVE, vendor/server/src/map/packets/c2s/0x029_item_move.h.
    // Moves ItemNum of Category1/ItemIndex1 into Category2; ItemIndex2 < 82 targets
    // a same-id stack merge into that slot, anything larger lets the server pick a
    // free slot (0x029_item_move.cpp process).
    pub const ITEM_MOVE: u16 = 0x029;

    pub const REQ_LOGOUT: u16 = 0x0E7;
}

// LSB CONTAINER_ID, vendor/server/src/map/item_container.h:32-49.
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

// LSB ITEMID::GIL, vendor/server/src/map/items.h:150 — never movable between
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
    // The high bits can carry an event id, so match on the low byte.
    pub const FISHING: u32 = 4;
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

    // GP_SERV_COMMAND_FISH, vendor/server/src/map/enums/packet_s2c.h:172. Sent to start
    // the fishing mini-game with the hooked fish's stats.
    pub const FISH: u16 = 0x115;

    pub const CHAT: u16 = 0x017;

    pub const ITEM_MAX: u16 = 0x01C;

    pub const ITEM_SAME: u16 = 0x01D;

    pub const ITEM_NUM: u16 = 0x01E;

    pub const ITEM_LIST: u16 = 0x01F;

    pub const ITEM_ATTR: u16 = 0x020;

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

    pub const MISCDATA: u16 = 0x063;

    pub const SYSTEMMES: u16 = 0x053;

    // GP_SERV_COMMAND_EVENTUCOFF, vendor/server/src/map/packets/s2c/0x052_eventucoff.h.
    // Mode 4 (Fishing) releases the fishing event lock — sent on a rejected cast or at the
    // end of fishing.
    pub const EVENTUCOFF: u16 = 0x052;

    pub const SCENARIO_ITEM: u16 = 0x055;

    // vendor/server/src/map/packets/s2c/0x057_weather.h
    pub const WEATHER: u16 = 0x057;

    pub const MUSIC: u16 = 0x05F;

    pub const MUSIC_VOLUME: u16 = 0x060;
    pub const EQUIP_CLEAR: u16 = 0x04F;
    pub const EQUIP_LIST: u16 = 0x050;
    pub const GRAP_LIST: u16 = 0x051;

    pub const WPOS: u16 = 0x05B;

    pub const WPOS2: u16 = 0x065;

    pub const MAGIC_DATA: u16 = 0x0AA;

    pub const COMMAND_DATA: u16 = 0x0AC;

    pub const ABIL_RECAST: u16 = 0x119;
    pub const ENTITY_UPDATE1: u16 = 0x067;
    pub const ENTITY_UPDATE2: u16 = 0x068;

    pub const GROUP_LIST: u16 = 0x0DD;

    pub const GROUP_ATTR: u16 = 0x0DF;

    // GP_SERV_COMMAND_EQUIP_INSPECT, vendor/server/src/map/enums/packet_s2c.h:143.
    // /check answer for a PC target: EQUIPMENT batches then one GENERAL packet
    // (vendor/server/src/map/packets/c2s/0x0dd_equip_inspect.cpp:135-136).
    pub const EQUIP_INSPECT: u16 = 0x0C9;
}
