//! The PlayOnline account services as a client library.
//!
//! Retail FFXI has no auth server of its own: the PlayOnline Viewer signs the
//! account in over two services, then hands the game a 16-byte value and a
//! 64-byte authCode that the lobby validates. This crate is the client half of
//! those services, so a session can be produced without the Viewer running.
//!
//! The wire rules are read from the Viewer binaries of an install the user
//! supplies; the observation records under
//! `.agents/skills/retail-observe/references/` name the build behind each one.
//! The host names the binaries carry are in `hosts` and `profile::host`; the
//! transport layer takes a connector, so nothing here opens a socket.

pub mod authcode;
pub mod chat;
pub mod crypto;
pub mod error;
pub mod hosts;
pub mod profile;
pub mod rng;
pub mod rsa;
pub mod transport;

pub use error::{Error, Result};
