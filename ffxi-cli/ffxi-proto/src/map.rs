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

/// Selected server→client opcodes used by v1.
pub mod s2c {
    pub const ENTERZONE: u16 = 0x008;
    pub const MESSAGE: u16 = 0x009;
    pub const LOGIN: u16 = 0x00A;
    pub const LOGOUT: u16 = 0x00B;
    pub const CHAR_PC: u16 = 0x00D;
    pub const CHAR_NPC: u16 = 0x00E;
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
    pub const EVENT: u16 = 0x032;
    pub const EVENTSTR: u16 = 0x033;
    pub const EVENTNUM: u16 = 0x034;
    pub const EQUIP_CLEAR: u16 = 0x04F;
    pub const EQUIP_LIST: u16 = 0x050;
    pub const GRAP_LIST: u16 = 0x051;
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
