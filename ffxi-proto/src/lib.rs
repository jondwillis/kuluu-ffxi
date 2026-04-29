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
pub mod zlib;
