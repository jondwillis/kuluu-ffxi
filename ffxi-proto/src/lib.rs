//! Protocol layer for LandSandBoat / Phoenix FFXI private-server emulators.
//!
//! Pure data layer — no networking, no async, no globals. The `ffxi-client`
//! crate is responsible for socket I/O and session orchestration.

pub mod blowfish;
pub mod checksum;
pub mod decode;
pub mod framing;
pub mod login;
pub mod map;
pub mod md5;
pub mod msg_action_modifier;
pub mod msg_area;
pub mod msg_basic;
pub mod msg_channel;
pub mod msg_system;
pub mod zlib;
