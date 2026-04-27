use mac_address::get_mac_address;

pub const PAYLOAD_LEN: usize = 102;

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

        buf[0x09..0x09 + username.len()].copy_from_slice(username.as_bytes());
        buf[0x19..0x19 + password.len()].copy_from_slice(password.as_bytes());
        buf[0x39] = cmd.code();

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
        let b = PayloadBuilder {
            mac: *b"01:23:45:67:89:AB",
            version: *b"1.0.0",
        };
        let buf = b.build("alice", "secret", Command::Login).unwrap();
        assert_eq!(buf.len(), 102);
        assert_eq!(buf[0x00], 0xFF);
        assert_eq!(&buf[0x09..0x0E], b"alice");
        assert_eq!(buf[0x0E], 0);
        assert_eq!(&buf[0x19..0x1F], b"secret");
        assert_eq!(buf[0x39], CMD_LOGIN);
        assert_eq!(&buf[0x40..0x50], &[0u8; 16][..]);
        assert_eq!(&buf[0x50..0x61], b"01:23:45:67:89:AB");
        assert_eq!(&buf[0x61..0x66], b"1.0.0");
    }

    #[test]
    fn change_password_overlap_matches_hxiloader_runtime() {
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
        let Ok(s) = local_mac_string() else {
            eprintln!("no local MAC available; skipping format check");
            return;
        };
        assert_eq!(s.len(), 17);
        let s = std::str::from_utf8(&s).unwrap();

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
