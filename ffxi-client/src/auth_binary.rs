//! Binary xiloader auth — wire format used by `hxiloader` v1.3
//! (github.com/HorizonFFXI/hxiloader) and any downstream server still on
//! the pre-LSB-JSON-rewrite handshake. 102-byte TLS write, 21-byte read.
//!
//! Layout was ported from `hxiloader/src/network.cpp::FFXiPolServer`:
//!
//! | offset | size | field                                                |
//! |--------|------|------------------------------------------------------|
//! | 0x00   |  1   | magic = 0xFF                                         |
//! | 0x01   |  8   | feature flags (reserved, zero)                       |
//! | 0x09   | 16   | username (null-padded ASCII)                         |
//! | 0x19   | 32   | password (null-padded ASCII)                         |
//! | 0x39   |  1   | command (0x10 login / 0x20 create / 0x30 change-pw)  |
//! | 0x40   | 16   | new_password first 16B (0x30 only; else zero)        |
//! | 0x50   | 17   | MAC ASCII "XX:XX:XX:XX:XX:XX" — clobbers 0x50..0x60 |
//! | 0x61   |  5   | version, ASCII "1.0.0"                               |
//!
//! Note on the 0x40 / 0x50 overlap: hxiloader's C++ does
//! `memcpy(0x40, new_pw, 32)` then `memcpy(0x50, mac, 17)` — MAC wins on
//! the second half. For login/create the new_pw memcpy is a buffer
//! over-read on the C++ side (empty string + size 32); we zero-init
//! instead, which the server treats identically.

use mac_address::get_mac_address;

pub const PAYLOAD_LEN: usize = 102;
/// Full success-reply length: `result(1) + account_id(4) + session_hash(16)`.
/// On auth failure the server typically writes only the 1-byte result code
/// and shuts the socket — hxiloader inspects `recvBuffer[0]` and ignores
/// the rest, which masks the partial reply on the wire.
pub const RESPONSE_LEN: usize = 21;
pub const RESPONSE_MIN_LEN: usize = 1;

pub const CMD_LOGIN: u8 = 0x10;
pub const CMD_CREATE: u8 = 0x20;
pub const CMD_CHANGE_PW: u8 = 0x30;

pub const RESULT_LOGIN_OK: u8 = 0x01;
pub const RESULT_LOGIN_FAIL: u8 = 0x02;
pub const RESULT_CREATE_OK: u8 = 0x03;
pub const RESULT_CREATE_TAKEN: u8 = 0x04;
pub const RESULT_CHANGE_PW_OK: u8 = 0x06;
pub const RESULT_CHANGE_PW_FAIL: u8 = 0x07;
pub const RESULT_ALREADY_LOGGED_IN: u8 = 0x0A;
pub const RESULT_VERSION_MISMATCH: u8 = 0x0B;

/// Version string sent at offset 0x61. hxiloader v1.3 hard-codes "1.0.0";
/// HorizonXI's auth server checks for an exact 5-byte match.
pub const DEFAULT_VERSION: [u8; 5] = *b"1.0.0";

#[derive(Debug, thiserror::Error)]
pub enum BinaryAuthError {
    #[error("username must be ≤15 bytes (got {0})")]
    UsernameTooLong(usize),
    #[error("password must be ≤31 bytes (got {0})")]
    PasswordTooLong(usize),
    #[error("could not read local MAC address: {0}")]
    MacUnavailable(String),
    #[error("server returned empty response (TCP closed before any data)")]
    EmptyResponse,
    #[error("server returned success code {RESULT_LOGIN_OK:#04x} but reply was {0} bytes; need {RESPONSE_LEN} for account+hash")]
    TruncatedSuccess(usize),
    #[error("server returned unexpected result code {0:#04x}")]
    UnexpectedResult(u8),
    #[error("login failed: invalid username or password")]
    LoginFailed,
    #[error("login failed: account already logged in")]
    AlreadyLoggedIn,
    #[error("login failed: xiloader version mismatch (sent {sent:?})")]
    VersionMismatch { sent: [u8; 5] },
    #[error("create failed: username already taken")]
    CreateTaken,
    #[error("change-password failed")]
    ChangePwFailed,
}

#[derive(Debug, Clone, Copy)]
pub enum Command<'a> {
    Login,
    Create,
    ChangePassword { new_password: &'a str },
}

impl Command<'_> {
    fn code(&self) -> u8 {
        match self {
            Command::Login => CMD_LOGIN,
            Command::Create => CMD_CREATE,
            Command::ChangePassword { .. } => CMD_CHANGE_PW,
        }
    }
}

/// Format the local interface MAC as the 17-byte ASCII string the server
/// expects. Lowercase hex, colon-separated — matches hxiloader's
/// `GetMacAddress()` (`%02x`) in `hxiloader/src/network.cpp`.
///
/// **`FFXI_MAC` env-var override** takes priority. Required when running
/// natively on macOS, because Wi-Fi MAC randomization sets the
/// locally-administered bit, which HorizonXI's server-side anti-VM
/// fingerprinting silently rejects as a `0x0B` version-mismatch. Set
/// `FFXI_MAC=XX:XX:XX:XX:XX:XX` to the MAC your real loader uses (e.g.
/// the VM NIC inside the Parallels/VMware install of `horizon-loader.exe`).
pub fn local_mac_string() -> Result<[u8; 17], BinaryAuthError> {
    if let Ok(env) = std::env::var("FFXI_MAC") {
        return parse_mac_override(&env);
    }
    let mac = get_mac_address()
        .map_err(|e| BinaryAuthError::MacUnavailable(e.to_string()))?
        .ok_or_else(|| BinaryAuthError::MacUnavailable("no interface".into()))?;
    let b = mac.bytes();
    let s = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5]
    );
    let mut out = [0u8; 17];
    out.copy_from_slice(s.as_bytes());
    Ok(out)
}

/// Validate and canonicalise an `FFXI_MAC` override. Accepts uppercase
/// or lowercase input; emits lowercase on the wire. Length must be
/// exactly 17 chars matching `XX:XX:XX:XX:XX:XX`.
fn parse_mac_override(s: &str) -> Result<[u8; 17], BinaryAuthError> {
    let trimmed = s.trim();
    if trimmed.len() != 17 {
        return Err(BinaryAuthError::MacUnavailable(format!(
            "FFXI_MAC must be 17 chars `XX:XX:XX:XX:XX:XX`, got {:?}",
            trimmed
        )));
    }
    let mut octets = [0u8; 6];
    for (i, part) in trimmed.split(':').enumerate() {
        if i >= 6 || part.len() != 2 {
            return Err(BinaryAuthError::MacUnavailable(format!(
                "FFXI_MAC malformed: {trimmed:?}"
            )));
        }
        octets[i] = u8::from_str_radix(part, 16).map_err(|e| {
            BinaryAuthError::MacUnavailable(format!("FFXI_MAC octet {i} not hex: {e}"))
        })?;
    }
    let s = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        octets[0], octets[1], octets[2], octets[3], octets[4], octets[5]
    );
    let mut out = [0u8; 17];
    out.copy_from_slice(s.as_bytes());
    Ok(out)
}

/// The hxiloader fallback when no adapter matches the local IP.
pub const NULL_MAC: [u8; 17] = *b"00:00:00:00:00:00";

pub struct PayloadBuilder {
    pub mac: [u8; 17],
    pub version: [u8; 5],
}

impl PayloadBuilder {
    pub fn new() -> Result<Self, BinaryAuthError> {
        Ok(Self {
            mac: local_mac_string()?,
            version: DEFAULT_VERSION,
        })
    }

    pub fn build(
        &self,
        username: &str,
        password: &str,
        cmd: Command<'_>,
    ) -> Result<[u8; PAYLOAD_LEN], BinaryAuthError> {
        if username.len() > 15 {
            return Err(BinaryAuthError::UsernameTooLong(username.len()));
        }
        if password.len() > 31 {
            return Err(BinaryAuthError::PasswordTooLong(password.len()));
        }

        let mut buf = [0u8; PAYLOAD_LEN];
        buf[0x00] = 0xFF;
        // 0x01..0x09: feature flags (zero).
        buf[0x09..0x09 + username.len()].copy_from_slice(username.as_bytes());
        buf[0x19..0x19 + password.len()].copy_from_slice(password.as_bytes());
        buf[0x39] = cmd.code();

        // New password lives at 0x40..0x4F (first 16 bytes only — MAC
        // clobbers 0x50..0x60). Mirrors hxiloader's `memcpy(0x40, _, 32)
        // ; memcpy(0x50, mac, 17)` ordering: MAC wins on overlap.
        if let Command::ChangePassword { new_password } = cmd {
            if new_password.len() > 31 {
                return Err(BinaryAuthError::PasswordTooLong(new_password.len()));
            }
            let n = new_password.len().min(16);
            buf[0x40..0x40 + n].copy_from_slice(&new_password.as_bytes()[..n]);
        }

        buf[0x50..0x61].copy_from_slice(&self.mac);
        buf[0x61..0x66].copy_from_slice(&self.version);
        Ok(buf)
    }
}

/// Parse the 21-byte server reply into `(account_id, session_hash)` on
/// success, or a typed error on failure. Layout from
/// `hxiloader/src/network.cpp` switch on `recvBuffer[0]`.
pub fn parse_response(
    buf: &[u8],
    sent_version: [u8; 5],
) -> Result<(u32, [u8; 16]), BinaryAuthError> {
    if buf.is_empty() {
        return Err(BinaryAuthError::EmptyResponse);
    }
    match buf[0] {
        RESULT_LOGIN_OK => {
            if buf.len() < RESPONSE_LEN {
                return Err(BinaryAuthError::TruncatedSuccess(buf.len()));
            }
            let account_id = u32::from_le_bytes(buf[1..5].try_into().unwrap());
            let mut hash = [0u8; 16];
            hash.copy_from_slice(&buf[5..21]);
            Ok((account_id, hash))
        }
        RESULT_LOGIN_FAIL => Err(BinaryAuthError::LoginFailed),
        RESULT_CREATE_TAKEN => Err(BinaryAuthError::CreateTaken),
        RESULT_CHANGE_PW_FAIL => Err(BinaryAuthError::ChangePwFailed),
        RESULT_ALREADY_LOGGED_IN => Err(BinaryAuthError::AlreadyLoggedIn),
        RESULT_VERSION_MISMATCH => Err(BinaryAuthError::VersionMismatch { sent: sent_version }),
        other => Err(BinaryAuthError::UnexpectedResult(other)),
    }
}

/// Parse the reply to a `CMD_CREATE` request. Server returns
/// `RESULT_CREATE_OK` with no payload; successful LOGIN happens on a
/// follow-up connection.
pub fn parse_create_response(buf: &[u8]) -> Result<(), BinaryAuthError> {
    if buf.is_empty() {
        return Err(BinaryAuthError::EmptyResponse);
    }
    match buf[0] {
        RESULT_CREATE_OK => Ok(()),
        RESULT_CREATE_TAKEN => Err(BinaryAuthError::CreateTaken),
        other => Err(BinaryAuthError::UnexpectedResult(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_payload_layout() {
        // MAC fixture casing is arbitrary here — the builder copies bytes
        // verbatim; the lowercase-formatting contract is exercised by
        // `local_mac_string_is_lowercase_with_colons` below.
        let b = PayloadBuilder {
            mac: *b"01:23:45:67:89:AB",
            version: *b"1.0.0",
        };
        let buf = b.build("alice", "secret", Command::Login).unwrap();
        assert_eq!(buf.len(), 102);
        assert_eq!(buf[0x00], 0xFF);
        assert_eq!(&buf[0x09..0x0E], b"alice");
        assert_eq!(buf[0x0E], 0); // padding
        assert_eq!(&buf[0x19..0x1F], b"secret");
        assert_eq!(buf[0x39], CMD_LOGIN);
        assert_eq!(&buf[0x40..0x50], &[0u8; 16][..]); // no new_password
        assert_eq!(&buf[0x50..0x61], b"01:23:45:67:89:AB");
        assert_eq!(&buf[0x61..0x66], b"1.0.0");
    }

    #[test]
    fn change_password_overlap_matches_hxiloader_runtime() {
        // new_password "abcdefghijklmnopqrst" (20 bytes). hxiloader writes
        // 32 bytes from new_pw to 0x40, then 17 bytes of MAC to 0x50.
        // MAC wins on 0x50..0x60. We pre-truncate new_pw to 16 bytes
        // (0x40..0x4F) for the same on-wire result.
        let b = PayloadBuilder {
            mac: *b"AA:BB:CC:DD:EE:FF",
            version: *b"1.0.0",
        };
        let buf = b
            .build(
                "alice",
                "old",
                Command::ChangePassword {
                    new_password: "abcdefghijklmnopqrst",
                },
            )
            .unwrap();
        assert_eq!(&buf[0x40..0x50], b"abcdefghijklmnop");
        assert_eq!(&buf[0x50..0x61], b"AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn parse_login_ok() {
        let mut buf = [0u8; RESPONSE_LEN];
        buf[0] = RESULT_LOGIN_OK;
        buf[1..5].copy_from_slice(&123_456u32.to_le_bytes());
        for i in 0..16 {
            buf[5 + i] = i as u8;
        }
        let (acct, hash) = parse_response(&buf, DEFAULT_VERSION).unwrap();
        assert_eq!(acct, 123_456);
        assert_eq!(hash[0], 0);
        assert_eq!(hash[15], 15);
    }

    #[test]
    fn local_mac_string_is_lowercase_with_colons() {
        // Skip if the host has no usable interface (rare on dev boxes; CI
        // sandboxes sometimes lack one). The contract under test is the
        // formatting, not the discovery.
        let Ok(s) = local_mac_string() else {
            eprintln!("no local MAC available; skipping format check");
            return;
        };
        assert_eq!(s.len(), 17);
        let s = std::str::from_utf8(&s).unwrap();
        // Every hex digit must be lowercase 0-9a-f — hxiloader's `%02x`.
        for (i, c) in s.chars().enumerate() {
            if i % 3 == 2 {
                assert_eq!(c, ':', "expected colon at position {i}: {s:?}");
            } else {
                assert!(
                    c.is_ascii_digit() || ('a'..='f').contains(&c),
                    "expected lowercase hex at position {i}: {s:?}"
                );
            }
        }
    }

    #[test]
    fn parse_version_mismatch_surfaces_sent_version() {
        let mut buf = [0u8; RESPONSE_LEN];
        buf[0] = RESULT_VERSION_MISMATCH;
        let err = parse_response(&buf, *b"9.9.9").unwrap_err();
        assert!(matches!(err, BinaryAuthError::VersionMismatch { sent } if &sent == b"9.9.9"));
    }
}
