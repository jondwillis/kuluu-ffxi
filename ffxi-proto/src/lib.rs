//! Protocol layer for LandSandBoat / Phoenix FFXI private-server emulators.
//!
//! Pure data layer — no networking, no async, no globals. The `ffxi-client`
//! crate is responsible for socket I/O and session orchestration.

pub mod ability_names;
pub mod autotranslate;
pub mod blowfish;
pub mod checksum;
pub mod decode;
pub mod equip_info;
pub mod framing;
pub mod item_names;
pub mod job_names;
pub mod login;
pub mod map;
pub mod md5;
pub mod msg_action_modifier;
pub mod msg_area;
pub mod msg_basic;
pub mod msg_channel;
pub mod msg_system;
pub mod skill_names;
pub mod spell_names;
pub mod status_names;
pub mod zlib;
