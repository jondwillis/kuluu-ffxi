//! Login-server protocol types — auth (54231 JSON), data (54230 IXFF binary),
//! view (54001 IXFF binary).
//!
//! Filled in by Steps 4–5 of the build sequence; this module is a stub now.

/// IXFF binary frame magic — appears at offset 4 of every login-data and
/// login-view packet header. As `u32`, this writes the bytes 'I','X','F','F'
/// (`0x49 0x58 0x46 0x46`) when serialized little-endian (matches
/// `loginPackets::getTerminator()` in `server/src/login/login_packets.h`).
pub const IXFF_TERMINATOR: u32 = u32::from_le_bytes(*b"IXFF");

/// Default ports (from `server/settings/default/network.lua`).
pub const LOGIN_AUTH_PORT: u16 = 54231;
pub const LOGIN_DATA_PORT: u16 = 54230;
pub const LOGIN_VIEW_PORT: u16 = 54001;
