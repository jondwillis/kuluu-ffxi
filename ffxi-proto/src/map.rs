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

    // GP_CLI_COMMAND_FISHING_2, vendor/server/src/map/enums/packet_c2s.h. The current
    // mini-game uses 0x110; 0x066 (GP_CLI_COMMAND_FISHING) is the pre-overhaul system,
    // aliased to the same struct server-side.
    pub const FISHING_2: u16 = 0x110;
    pub const FISHING: u16 = 0x066;

    pub const EQUIP_SET: u16 = 0x050;

    pub const REQ_LOGOUT: u16 = 0x0E7;
}

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

    pub const SHOP_OPEN: u16 = 0x03E;

    pub const MISCDATA: u16 = 0x063;

    pub const SYSTEMMES: u16 = 0x053;

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
}
