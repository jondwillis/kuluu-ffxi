//! FFXI game vocabulary: LSB-scraped lookup tables (names, ids, flags,
//! valid-target masks, cast/recast times) and the presentation helpers built
//! on them. Everything here is data about the game world; nothing here
//! parses or emits wire bytes — that boundary lives in `ffxi-proto`.

pub mod ability_names;
pub mod action_anim;
pub mod cast_time;
pub mod emote_names;
pub mod equip_info;
pub mod gil;
pub mod item_flags;
pub mod item_names;
pub mod item_usable;
pub mod job_names;
pub mod key_item_names;
pub mod magic;
pub mod msg_action_modifier;
pub mod msg_area;
pub mod msg_basic;
pub mod msg_channel;
pub mod msg_system;
pub mod recast;
pub mod skill_names;
pub mod spell_names;
pub mod status_effects;
pub mod status_names;
pub mod tp_move_names;
pub mod valid_target;
pub mod vana_time;
pub mod weapon_skill;

pub mod transport;
