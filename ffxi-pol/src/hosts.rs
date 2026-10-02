//! The PlayOnline host names the Viewer and the game carry, read from the
//! binaries of a retail install. The profile host is not a constant: its
//! index arrives on the chat connection, so it lives with its transaction
//! layer in `crate::profile::host`.

/// The FFXI lobby server the game resolves in its default connection mode.
/// FFXiMain.dll of the retail-2026-09 row (sha256 f2245d1c9d06e02c): the
/// string at VA 0x10362044, resolved from VA 0x100ed84d when the
/// connection-mode global is 0; the other modes are development paths.
pub const LOBBY_HOST: &str = "ffxi00.pol.com";

/// The chat service a member account signs in against. app.dll 7ba99828
/// writes this literal into the chat-host field of every member record it
/// creates (`0x101a2695`, `0x101ada50`, `0x101ae280`), and polcore's login
/// driver resolves whatever that field holds. A record saved with another
/// host overrides it, which is why the Viewer keeps the name in settings
/// rather than in code.
pub const CHAT_HOST: &str = "ci000.pol.com";
