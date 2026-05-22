//! Map-server packet types — UDP, encrypted with per-session FFXI Blowfish.
//!
//! Filled in by Steps 6–10 of the build sequence; this module is a stub now.

/// Default UDP port for the map server (`MAP_PORT` in
/// `server/settings/default/network.lua`).
pub const MAP_PORT: u16 = 54230;

/// Maximum UDP datagram size the map server accepts. Source:
/// `server/src/map/map_constants.h::kMaxBufferSize = 2500`.
pub const MAX_DATAGRAM: usize = 2500;

/// Selected client→server opcodes used by v1.
pub mod c2s {
    pub const LOGIN: u16 = 0x00A;
    pub const NETEND: u16 = 0x00D;
    pub const ZONE_TRANSITION: u16 = 0x011;
    pub const POS: u16 = 0x015;
    /// `GP_CLI_COMMAND_ACTION` — universal "perform action on target" packet.
    /// Body: `UniqueNo:u32 ActIndex:u16 ActionID:u16 ActionBuf[16]`.
    /// `ActionID` 0=Talk, 0x02=Attack, 0x04=AttackOff, 0x0F=ChangeTarget…
    pub const ACTION: u16 = 0x01A;
    pub const EVENT_END: u16 = 0x05B;
    /// `GP_CLI_COMMAND_CHAT_STD` — see `session::build_subpacket_chat`.
    pub const CHAT: u16 = 0x0B5;
    /// `GP_CLI_COMMAND_SHOP_BUY` — purchase an item from an open NPC shop.
    /// Body: `u32 ItemNum (qty), u16 ShopNo, u16 ShopItemIndex, u8 PropertyItemIndex, u8 pad[3]`.
    /// See `vendor/server/src/map/packets/c2s/0x083_shop_buy.h`.
    pub const SHOP_BUY: u16 = 0x083;
    /// `GP_CLI_COMMAND_REQLOGOUT` — request `/logout` (return to char-select)
    /// or `/shutdown` (exit game). Body: `u16 Mode, u16 Kind` = 4 bytes
    /// (size_words=2). The server applies `EFFECT_LEAVEGAME` (5s tick
    /// interval); the Lua handler at `scripts/effects/leavegame.lua`
    /// calls `leaveGame()` after `elapsedTicks > 5` (≈30s total) for
    /// normal players, and **immediately** in `onEffectGain` if the
    /// player is a GM (`getGMLevel() > 0`) or in a Mog House. The s2c
    /// 0x00B `LOGOUT` lands when `leaveGame()` fires. Sending a second
    /// `Toggle` while the effect is active cancels it. See
    /// `vendor/server/src/map/packets/c2s/0x0e7_reqlogout.{h,cpp}`.
    pub const REQ_LOGOUT: u16 = 0x0E7;
}

/// `GP_CLI_COMMAND_REQLOGOUT_MODE` / `_KIND` — wire enums for the 0x0E7
/// `/logout` and `/shutdown` request packet. Mirrors
/// `vendor/server/src/map/packets/c2s/0x0e7_reqlogout.h`.
pub mod reqlogout {
    /// `Mode` field. The server validates the value is one of these via
    /// `oneOf<GP_CLI_COMMAND_REQLOGOUT_MODE>` (see 0x0e7_reqlogout.cpp);
    /// any other value is rejected outright.
    pub mod mode {
        /// `/logout` and `/shutdown` with no argument — flips the
        /// in-progress LeaveGame effect on or off.
        pub const TOGGLE: u16 = 0x00;
        /// `/logout on` — start the logout timer (no-op if already running).
        pub const LOGOUT_ON: u16 = 0x01;
        /// `/logout off` and `/shutdown off` — cancel an in-progress
        /// LeaveGame effect of either kind.
        pub const OFF: u16 = 0x02;
        /// `/shutdown on` — start the shutdown timer (no-op if running).
        pub const SHUTDOWN_ON: u16 = 0x03;
    }

    /// `Kind` field. Selects between returning to char-select (`LOGOUT`)
    /// and fully closing the game (`SHUTDOWN`). Note the wire enum jumps
    /// 1 → 3; `2` is intentionally unused.
    pub mod kind {
        /// `/logout` — return to character select.
        pub const LOGOUT: u16 = 0x01;
        /// `/shutdown` — exit the game entirely.
        pub const SHUTDOWN: u16 = 0x03;
    }
}

/// `GP_CLI_COMMAND_ACTION_ACTIONID` — see
/// `Phoenix/src/map/packets/c2s/0x01a_action.h`. We only enumerate the values
/// the TUI/agent can drive today; rest are u16 passthrough.
pub mod action_id {
    pub const TALK: u16 = 0x00;
    pub const ATTACK: u16 = 0x02;
    pub const CAST_MAGIC: u16 = 0x03;
    pub const ATTACK_OFF: u16 = 0x04;
    pub const HELP: u16 = 0x05;
    pub const WEAPONSKILL: u16 = 0x07;
    pub const JOB_ABILITY: u16 = 0x09;
    pub const ASSIST: u16 = 0x0C;
    pub const CHANGE_TARGET: u16 = 0x0F;
    pub const SHOOT: u16 = 0x10;
}

/// `CHAT_MESSAGE_TYPE` byte carried in s2c 0x017 and as the high byte of c2s
/// 0x0B5's `Kind`. Mirrors `vendor/server/src/map/enums/chat_message_type.h`
/// — only the values the chat panel actually surfaces are listed; unknown
/// kinds fall through to `ChatChannel::Other` at the call site.
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

/// Selected server→client opcodes used by v1.
pub mod s2c {
    pub const ENTERZONE: u16 = 0x008;
    pub const MESSAGE: u16 = 0x009;
    pub const LOGIN: u16 = 0x00A;
    pub const LOGOUT: u16 = 0x00B;
    pub const CHAR_PC: u16 = 0x00D;
    pub const CHAR_NPC: u16 = 0x00E;
    /// `GP_SERV_COMMAND_CHAT_STD` — player/NPC chat. Body: `Kind:u8 Attr:u8
    /// Data:u16 sName[15] Mes[var]`. See
    /// `vendor/server/src/map/packets/s2c/0x017_chat_std.h`.
    pub const CHAT: u16 = 0x017;
    /// `GP_SERV_COMMAND_ITEM_MAX` — container size table. Body carries
    /// `ItemNum[18]` (legacy u8 capacity) + padding + `ItemNum2[18]`
    /// (u16 capacity for >255-slot containers) for all 18 CONTAINER_IDs.
    pub const ITEM_MAX: u16 = 0x01C;
    /// `GP_SERV_COMMAND_ITEM_SAME` — load-state flag. `State == 1`
    /// (`AllLoaded`) signals the initial inventory flood is complete.
    pub const ITEM_SAME: u16 = 0x01D;
    /// `GP_SERV_COMMAND_ITEM_NUM` — quantity change for one slot.
    pub const ITEM_NUM: u16 = 0x01E;
    /// `GP_SERV_COMMAND_ITEM_LIST` — full slot definition (item_no + qty
    /// + lock flags). Sent during the zone-in flood and on item swaps.
    pub const ITEM_LIST: u16 = 0x01F;
    /// `GP_SERV_COMMAND_ITEM_ATTR` — slot definition + 24-byte
    /// item-type-specific extdata. We surface the extdata as raw bytes;
    /// interpretation lives in upstream (Phoenix's per-item logic).
    pub const ITEM_ATTR: u16 = 0x020;
    /// `GP_SERV_COMMAND_BATTLE2` — bitpacked combat-action stream. The wire
    /// format isn't directly mappable; we don't decode it, but the symbolic
    /// id is here so the dispatcher can ignore it explicitly without an
    /// "unknown opcode" line each frame of combat.
    pub const BATTLE2: u16 = 0x028;
    /// `GP_SERV_COMMAND_BATTLE_MESSAGE` — combat text message. Body:
    /// `UniqueNoCas:u32 UniqueNoTar:u32 Data:u32 Data2:u32 ActIndexCas:u16
    /// ActIndexTar:u16 MessageNum:u16 Type:u8 padding:u8` = 24 bytes. The
    /// text is looked up in `msg_basic` and `<user>`/`<target>`/`<amount>`
    /// placeholders are substituted client-side from the cas/tar entity
    /// names + Data fields. See
    /// `vendor/server/src/map/packets/s2c/0x029_battle_message.h`.
    pub const BATTLE_MESSAGE: u16 = 0x029;
    pub const EVENT: u16 = 0x032;
    pub const EVENTSTR: u16 = 0x033;
    pub const EVENTNUM: u16 = 0x034;
    /// `GP_SERV_COMMAND_BATTLE_MESSAGE2` — end-of-combat / chain / XP gain
    /// messages. Same 24-byte layout as 0x029 but field order differs:
    /// `UniqueNoCas:u32 UniqueNoTar:u32 ActIndexCas:u16 ActIndexTar:u16
    /// Data:u32 Data2:u32 MessageNum:u16 Type:u8 padding:u8`. See
    /// `vendor/server/src/map/packets/s2c/0x02d_battle_message2.h`.
    pub const BATTLE_MESSAGE2: u16 = 0x02D;
    /// `GP_SERV_COMMAND_SHOP_LIST` — list of items an NPC shop is selling.
    /// Body: `u16 ShopItemOffsetIndex, u8 Flags, u8 pad, GP_SHOP[N]` where
    /// each `GP_SHOP` is 10 bytes:
    /// `u32 ItemPrice, u16 ItemNo, u8 ShopIndex, u8 pad, u16 Skill, u16 GuildInfo`.
    /// `N = (body_len - 4) / 10`. See
    /// `vendor/server/src/map/packets/s2c/0x03c_shop_list.h`.
    pub const SHOP_LIST: u16 = 0x03C;
    /// `GP_SERV_COMMAND_SHOP_OPEN` — "show the shop window now". Body:
    /// `u16 ShopListNum, u16 pad`. See
    /// `vendor/server/src/map/packets/s2c/0x03e_shop_open.h`.
    pub const SHOP_OPEN: u16 = 0x03E;
    /// `GP_SERV_COMMAND_MISCDATA` — multiplexed misc-data carrier. The
    /// first u16 of the body is a `GP_SERV_COMMAND_MISCDATA_TYPE`:
    /// `0x02 Merits, 0x03/0x04 Monstrosity, 0x05 JobPoints, 0x06 Homepoints,
    /// 0x07 Unity, 0x09 StatusIcons, 0x0A Unknown`. See
    /// `vendor/server/src/map/packets/s2c/0x063_miscdata.h`.
    pub const MISCDATA: u16 = 0x063;
    /// `GP_SERV_COMMAND_SYSTEMMES` — formatted system message. Body:
    /// `u32 para, u32 para2, MsgStd Number(u16), u16 padding` = 12 bytes.
    /// `Number` is an id into `xi.msg.system` (see
    /// `ffxi_proto::msg_system`); `<seconds>` etc. placeholders in the
    /// looked-up text are filled from `para`/`para2`. The /logout
    /// countdown ticker (id 7) hits this opcode every 5s. See
    /// `vendor/server/src/map/packets/s2c/0x053_systemmes.h`.
    pub const SYSTEMMES: u16 = 0x053;
    /// `GP_SERV_COMMAND_WEATHER` — current zone weather. Body:
    /// `u32 StartTime, u16 WeatherNumber, u16 WeatherOffsetTime` = 8 bytes.
    /// `WeatherNumber` indexes LSB's `Weather` enum (values 0x00..=0x13;
    /// 0x14..=0x27 wrap via mod-20 — see `Weather::from_lsb`). See
    /// `vendor/server/src/map/packets/s2c/0x057_weather.h` and
    /// `vendor/server/src/map/enums/weather.h`.
    pub const WEATHER: u16 = 0x057;
    /// `GP_SERV_COMMAND_MUSIC` — server-pushed BGM slot assignment.
    /// Body: `u16 Slot, u16 MusicNum` = 4 bytes. `Slot` indexes the
    /// LSB `MusicSlot` enum (0=ZoneDay, 1=ZoneNight, 2=CombatSolo,
    /// 3=CombatParty, 4=Mount, 5=Dead, 6=MogHouse, 7=Fishing); the
    /// client picks which slot is currently audible based on its
    /// own state machine. See
    /// `vendor/server/src/map/packets/s2c/0x05f_music.{h,cpp}` and
    /// `vendor/server/src/map/enums/music_slot.h`.
    pub const MUSIC: u16 = 0x05F;
    /// `GP_SERV_COMMAND_MUSICVOLUME` — per-slot music volume tweak.
    /// Body shape mirrors `MUSIC` (u16 slot, u16 volume). See
    /// `vendor/server/src/map/packets/s2c/0x060_musicvolume.h`.
    pub const MUSIC_VOLUME: u16 = 0x060;
    pub const EQUIP_CLEAR: u16 = 0x04F;
    pub const EQUIP_LIST: u16 = 0x050;
    pub const GRAP_LIST: u16 = 0x051;
    /// `GP_SERV_COMMAND_MAGIC_DATA` — 128-byte (1024-bit) bitmap of
    /// learned spells, indexed by spell id. See
    /// `vendor/server/src/map/packets/s2c/0x0aa_magic_data.h` and
    /// `CCharEntity::m_SpellList` (`xi::bitset<1024>`). Sent once on
    /// login and again on every spell-learned event.
    pub const MAGIC_DATA: u16 = 0x0AA;
    /// `GP_SERV_COMMAND_COMMAND_DATA` — 224 bytes: four bitmaps in
    /// order (WeaponSkills[64], JobAbilities[64], PetAbilities[64],
    /// Traits[32]). Each is a packed bitset where bit N indexes the
    /// corresponding id-table. See
    /// `vendor/server/src/map/packets/s2c/0x0ac_command_data.h`.
    pub const COMMAND_DATA: u16 = 0x0AC;
    /// `GP_SERV_COMMAND_ABIL_RECAST` — current cooldown snapshot for
    /// up to 31 ability/recast groups + the mount recast. Used by
    /// the HUD to grey out unavailable abilities and show
    /// "X.Xs" overlays on Stage-2+ Abilities menu rows.
    pub const ABIL_RECAST: u16 = 0x119;
    pub const ENTITY_UPDATE1: u16 = 0x067;
    pub const ENTITY_UPDATE2: u16 = 0x068;
    /// `GP_SERV_COMMAND_GROUP_LIST` — sent for OTHER party members
    /// (other PCs and Trusts). Carries name + leader flags, in addition
    /// to HP/MP/TP/job. See `Phoenix/src/map/packets/s2c/0x0dd_group_list.h`.
    pub const GROUP_LIST: u16 = 0x0DD;
    /// `GP_SERV_COMMAND_GROUP_ATTR` — sent for the LOCAL player and
    /// Trust members. Same HP/MP/TP/job fields, no name (we know our own)
    /// and no leader flag. See `Phoenix/src/map/packets/s2c/0x0df_group_attr.h`.
    pub const GROUP_ATTR: u16 = 0x0DF;
}
