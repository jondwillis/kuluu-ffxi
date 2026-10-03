//! The profile service: fixed-size request/reply framing, the per-request
//! authenticator, the member-id codec, and the member-login body.
//!
//! Read from polcore.dll build 73b1864b, and from app.dll build 7ba99828 for
//! the credential the Viewer stages into it. A transaction is a 0x28-byte
//! request and a 0x18-byte reply header, optionally followed by a checksummed
//! body; everything after the plaintext connect handshake is enciphered with
//! the Blowfish key the chat service agreed. The bodies here are pure
//! functions of bytes; the transport that carries them is `crate::transport`.

use md5::{Digest as _, Md5};
use sha1::Sha1;

use crate::crypto::checksum;
use crate::error::{Error, Result};

/// polcore `0x100742ec`: the base-36 alphabet a member id renders through.
const ID_ALPHABET: &[u8; 36] = b"EFKAOYMJVNGTDSWBQLPCIRHZXU6328401795";
/// A member id is exactly eight base-36 digits.
pub const MEMBER_ID_LEN: usize = 8;

/// polcore `0x10075430`.
pub const HOST_FORMAT: &str = "pp{:03}.pol.com";
/// polcore `0x1001f0f0`, stored host-order in POL's own address struct.
pub const PORT: u16 = 51220;

const REQUEST_HEADER_LEN: usize = 0x28;
const REPLY_HEADER_LEN: usize = 0x18;
/// polcore `0x1001f5e0`: request[0] on every application request.
const REQUEST_MAGIC: u8 = 2;
const DIGEST_LEN: usize = 16;
/// polcore `0x1001f970`: every body ends with a 4-byte checksum, and the
/// declared length includes it.
pub const BODY_CHECKSUM_LEN: usize = 4;
/// Only 15 of the 16-byte secret slot are secret; byte 15 is always filler.
pub const SECRET_MAX_LEN: usize = 0x0F;

/// polcore `0x1001f690` / `0x1001f400`: a non-zero status maps to this base.
const ERROR_BASE: i32 = -0x1450;

/// What a refusal means, in our own words. The Viewer has a message for each
/// of these in its own resources, which are game content and are not
/// reproduced here; these are descriptions of the branch each status takes.
///
/// The distinction the Viewer's visible text loses is worth keeping: it shows
/// the same sentence for a refused identifier and for an account that cannot
/// sign in at all, and only the status byte tells them apart.
pub fn status_meaning(status: u8) -> Option<&'static str> {
    match status {
        STATUS_ADDRESS_BLOCKED => Some("this address is blocked from the account service"),
        STATUS_IDENTITY_REFUSED => {
            Some("the Square Enix id, password or one-time password was refused")
        }
        STATUS_ACCOUNT_STATE => Some(
            "the Square Enix account itself cannot sign in, which covers a closed or              suspended account and one with an unpaid balance, rather than a mistyped              password",
        ),
        STATUS_SERVICE_UNAVAILABLE => Some("the account service could not certify the account"),
        _ => None,
    }
}

const STATUS_ADDRESS_BLOCKED: u8 = 0x6E;
const STATUS_IDENTITY_REFUSED: u8 = 0x6F;
const STATUS_ACCOUNT_STATE: u8 = 0x70;
const STATUS_SERVICE_UNAVAILABLE: u8 = 0xE2;

/// A category and opcode name a profile transaction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Transaction {
    pub category: u8,
    pub opcode: u8,
}

impl Transaction {
    pub const fn new(category: u8, opcode: u8) -> Self {
        Self { category, opcode }
    }

    /// What this transaction is for, so a refusal names the step rather than
    /// a category and an opcode.
    pub fn name(self) -> &'static str {
        match self {
            MEMBER_LOGIN => "the member login",
            FRIEND_LIST => "the friend list",
            SELECT_SERVICE => "the world select",
            ENTER_COMMUNITY => "entering the community service",
            _ => "a request",
        }
    }
}

/// polcore `0x1001e5d0`: the member login, the first application transaction.
pub const MEMBER_LOGIN: Transaction = Transaction::new(4, 7);
/// polcore `0x100237f0`: fetch the friend and ignore lists. app.dll builds
/// its friend records from the reply, so despite its position in the
/// transaction catalogue this is a social feature, not part of a login.
pub const FRIEND_LIST: Transaction = Transaction::new(2, 3);
/// polcore `0x1001d490`: select the world / service context.
pub const SELECT_SERVICE: Transaction = Transaction::new(4, 6);
/// polcore `0x1001db90`: enter the community service; its 0x20-byte reply is
/// the source the lobby session (the 16-byte value and the 64-byte authCode)
/// is built from.
pub const ENTER_COMMUNITY: Transaction = Transaction::new(4, 5);

/// polcore `0x10019e20`: eight base-36 digits, most significant first.
pub fn id_encode(mut value: u64) -> [u8; MEMBER_ID_LEN] {
    let mut out = [0u8; MEMBER_ID_LEN];
    for slot in out.iter_mut().rev() {
        *slot = ID_ALPHABET[(value % 36) as usize];
        value /= 36;
    }
    out
}

/// polcore `0x10019c70`: eight base-36 digits back to the integer.
pub fn id_decode(text: &[u8]) -> Result<u64> {
    if text.len() != MEMBER_ID_LEN {
        return Err(Error::protocol("a member id is eight characters"));
    }
    let mut value = 0u64;
    for &c in text {
        let digit = ID_ALPHABET
            .iter()
            .position(|&a| a == c)
            .ok_or_else(|| Error::protocol("member id has a non-base-36 character"))?;
        value = value * 36 + digit as u64;
    }
    Ok(value)
}

/// polcore `0x1001e8d0`: the profile host an account's own host index selects.
/// The index is not derivable from the member id: it is the 7-bit routing
/// field the chat service assigns after registration, so it reaches this from
/// `crate::chat::Routing`.
pub fn host(host_index: u8) -> String {
    format!("pp{:03}.pol.com", host_index & HOST_INDEX_MASK)
}

/// polcore `0x10019e20` takes the host index out of bits 9..15 of the packed
/// identity's high dword, so it is seven bits wide.
pub const HOST_INDEX_MASK: u8 = 0x7F;

/// polcore `0x1001f5e0`: the per-request authenticator, `MD5(id8 || secret ||
/// token)`. The token is a per-connection nonce from the handshake reply, so
/// this binds the member and the connection, not the individual request.
pub fn authenticator(member_id: &[u8], secret: &[u8], token: [u8; 4]) -> Result<[u8; DIGEST_LEN]> {
    if member_id.len() != MEMBER_ID_LEN {
        return Err(Error::protocol("a member id is eight characters"));
    }
    if secret.len() > SECRET_MAX_LEN {
        return Err(Error::protocol("the profile secret is at most 15 bytes"));
    }
    let mut h = Md5::new();
    h.update(member_id);
    h.update(secret);
    h.update(token);
    Ok(h.finalize().into())
}

/// polcore `0x1001f5e0`: the 0x28-byte application request header, plaintext
/// before the stream cipher is applied. `declared_body_len` includes the
/// 4-byte body checksum.
pub fn request_header(
    tx: Transaction,
    declared_body_len: u32,
    digest: [u8; DIGEST_LEN],
) -> [u8; REQUEST_HEADER_LEN] {
    let mut head = [0u8; REQUEST_HEADER_LEN];
    head[0x00] = REQUEST_MAGIC;
    head[0x01] = tx.category;
    head[0x02] = tx.opcode;
    head[0x04..0x08].copy_from_slice(&declared_body_len.to_le_bytes());
    head[0x18..0x28].copy_from_slice(&digest);
    head
}

/// The four fields the client reads from a reply header; the rest is ignored.
#[derive(Clone, Copy, Debug)]
pub struct ReplyHeader {
    pub status: u8,
    pub body_len: u32,
    pub redirect_addr: u32,
    pub token: [u8; 4],
}

impl ReplyHeader {
    pub fn is_ok(&self) -> bool {
        self.status == 0
    }

    /// polcore `0x1001f400`: a non-zero status as a client error code.
    pub fn error_code(&self) -> i32 {
        if self.is_ok() {
            0
        } else {
            ERROR_BASE - i32::from(self.status)
        }
    }
}

/// polcore `0x1001f690`: parse the 0x18-byte reply header.
pub fn parse_reply_header(buf: &[u8]) -> Result<ReplyHeader> {
    if buf.len() != REPLY_HEADER_LEN {
        return Err(Error::protocol("a reply header is 0x18 bytes"));
    }
    Ok(ReplyHeader {
        status: buf[0x01],
        body_len: u32::from_le_bytes(buf[0x04..0x08].try_into().unwrap()),
        redirect_addr: u32::from_le_bytes(buf[0x08..0x0C].try_into().unwrap()),
        token: buf[0x14..0x18].try_into().unwrap(),
    })
}

/// polcore `0x1001f970`: a final body chunk is the payload with its own
/// 4-byte checksum appended; the sum is the value the header declares.
pub fn seal_body(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + BODY_CHECKSUM_LEN);
    out.extend_from_slice(payload);
    out.extend_from_slice(&checksum(payload, 0).to_le_bytes());
    out
}

/// polcore `0x1001f800`: verify and strip a final body chunk's checksum.
pub fn open_body(body: &[u8]) -> Result<Vec<u8>> {
    if body.len() < BODY_CHECKSUM_LEN {
        return Err(Error::protocol("a body carries a 4-byte checksum"));
    }
    let (payload, trailer) = body.split_at(body.len() - BODY_CHECKSUM_LEN);
    let want = u32::from_le_bytes(trailer.try_into().unwrap());
    if checksum(payload, 0) != want {
        return Err(Error::protocol("body checksum mismatch"));
    }
    Ok(payload.to_vec())
}

/// The declared length a request header carries for a payload of this size.
pub fn declared_len(payload_len: usize) -> u32 {
    (payload_len + BODY_CHECKSUM_LEN) as u32
}

/// polcore `0x1001e4b0` wipes its stack copies with these widths right after
/// the call, which is the binary's own statement of the argument sizes.
pub const SECRET_LEN: usize = 20;
pub const OTP_LEN: usize = 6;
/// app.dll `0x1019ab29` appends this before the second digest.
const SECRET_SALT: &[u8] = b"playonline";

/// The account credential the member login proves possession of. The name is
/// the Square Enix id, which the Viewer leaves empty for an account that has
/// none; every request is bound to the member by its authenticator either
/// way. `secret20` is what `member_secret` derives from the typed password,
/// and `otp` is set only for a token account.
pub struct MemberCredential {
    pub name: String,
    pub secret20: [u8; SECRET_LEN],
    pub otp: Option<[u8; OTP_LEN]>,
}

impl MemberCredential {
    /// Build the credential the Viewer would stage from a typed password.
    pub fn new(name: impl Into<String>, password: &str, otp: Option<[u8; OTP_LEN]>) -> Self {
        Self {
            name: name.into(),
            secret20: member_secret(password),
            otp,
        }
    }
}

/// app.dll `0x1019ab29`: the 20-byte secret the member login proves possession
/// of, `SHA1(hex(SHA1(password)) || "playonline")` with the inner digest
/// rendered as forty lowercase hex characters. The Viewer holds the password
/// as UTF-16 and feeds the low byte of each code unit, so a character outside
/// Latin-1 is truncated before it is hashed rather than encoded.
pub fn member_secret(password: &str) -> [u8; SECRET_LEN] {
    let mut inner = Sha1::new();
    for unit in password.encode_utf16() {
        inner.update([unit as u8]);
    }
    let first: [u8; SECRET_LEN] = inner.finalize().into();

    let mut outer = Sha1::new();
    outer.update(hex_lower(&first));
    outer.update(SECRET_SALT);
    outer.finalize().into()
}

fn hex_lower(bytes: &[u8; SECRET_LEN]) -> [u8; 2 * SECRET_LEN] {
    let mut out = [0u8; 2 * SECRET_LEN];
    for (i, b) in bytes.iter().enumerate() {
        out[2 * i] = HEX_DIGITS[(b >> NIBBLE_BITS) as usize];
        out[2 * i + 1] = HEX_DIGITS[(b & NIBBLE_MASK) as usize];
    }
    out
}

/// polcore `0x1001f4d0(ctx, 4, 7, 0x40)` declares 0x40 bytes, which includes
/// the 4-byte trailer `seal_body` appends.
const LOGIN_BODY_LEN: usize = 0x40;
/// The payload the login declares, before its checksum.
pub const LOGIN_PAYLOAD_LEN: usize = LOGIN_BODY_LEN - BODY_CHECKSUM_LEN;
/// polcore `0x1001e760` rounds the stamp down to the minute so the client and
/// the server agree on it without a clock exchange.
const SECONDS_PER_MINUTE: u64 = 60;
/// polcore `0x1001e760` copies the name while `i < 0x10`, into a body it has
/// already zeroed, so sixteen characters fit with their terminator landing on
/// the byte before the one-time password.
pub const LOGIN_NAME_MAX: usize = 16;
const LOGIN_DIGEST_OFFSET: usize = 0x20;
const LOGIN_OTP_OFFSET: usize = 0x12;

/// polcore `0x1001e760`: the member-login payload. Offset 0 is the mode
/// (1 id+password, 2 with a one-time password), offset 1 the NUL-terminated
/// name, offset 0x20 a SHA-1 over the lowercase hex of the 20-byte secret
/// concatenated with the minute-rounded unix time as decimal.
pub fn member_login_body(
    cred: &MemberCredential,
    unix_secs: u64,
) -> Result<[u8; LOGIN_PAYLOAD_LEN]> {
    let name = cred.name.as_bytes();
    if name.len() > LOGIN_NAME_MAX {
        return Err(Error::protocol("a Square Enix ID is at most 16 characters"));
    }
    let mut body = [0u8; LOGIN_PAYLOAD_LEN];
    body[0] = if cred.otp.is_some() { 2 } else { 1 };
    body[1..1 + name.len()].copy_from_slice(name);

    let hex = hex_lower(&cred.secret20);
    let minute = unix_secs - unix_secs % SECONDS_PER_MINUTE;
    let stamp = minute.to_string();
    let mut h = Sha1::new();
    h.update(hex);
    h.update(stamp.as_bytes());
    let digest = h.finalize();
    body[LOGIN_DIGEST_OFFSET..LOGIN_DIGEST_OFFSET + digest.len()].copy_from_slice(&digest);

    if let Some(otp) = cred.otp {
        body[LOGIN_OTP_OFFSET..LOGIN_OTP_OFFSET + otp.len()].copy_from_slice(&otp);
    }
    Ok(body)
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
const NIBBLE_MASK: u8 = 0x0F;
const NIBBLE_BITS: u32 = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_member_id_round_trips_through_base36() {
        let text = b"EFKAOYMJ";
        let value = id_decode(text).unwrap();
        assert_eq!(&id_encode(value), text);
        assert_eq!(id_encode(0), *b"EEEEEEEE");
    }

    #[test]
    fn the_host_index_selects_the_profile_host() {
        assert_eq!(host(0), "pp000.pol.com");
        assert_eq!(host(3), "pp003.pol.com");
    }

    #[test]
    fn a_body_round_trips_through_its_checksum() {
        let payload = vec![0x41u8; 0x3C];
        let sealed = seal_body(&payload);
        assert_eq!(sealed.len(), payload.len() + BODY_CHECKSUM_LEN);
        assert_eq!(open_body(&sealed).unwrap(), payload);
        let mut tampered = sealed.clone();
        tampered[0] ^= 1;
        assert!(open_body(&tampered).is_err());
    }

    #[test]
    fn the_request_header_and_authenticator_match_the_reference() {
        let digest = authenticator(b"EFKAOYMJ", b"hunter2", [1, 2, 3, 4]).unwrap();
        // Pinned from the profile reference self-check (static reading of
        // polcore.dll 73b1864b), not captured from any live server.
        assert_eq!(hex::encode(digest), "23e1051d63b167b9b32bf449d7d2de10");
        let head = request_header(MEMBER_LOGIN, 0x40, digest);
        assert_eq!(head[0], REQUEST_MAGIC);
        assert_eq!(head[1], 4);
        assert_eq!(head[2], 7);
        assert_eq!(u32::from_le_bytes(head[4..8].try_into().unwrap()), 0x40);
        assert_eq!(&head[0x18..0x28], &digest);
    }

    #[test]
    fn a_refused_identity_and_a_refused_account_are_told_apart() {
        // The Viewer shows one sentence for both, so only the status
        // distinguishes them; a client that collapses them would send a
        // player to re-type a password that was never wrong.
        let refused = status_meaning(STATUS_IDENTITY_REFUSED).unwrap();
        let state = status_meaning(STATUS_ACCOUNT_STATE).unwrap();
        assert_ne!(refused, state);
        assert!(state.contains("unpaid"));
        assert!(status_meaning(0x00).is_none());
        assert!(status_meaning(0x79).is_none());
    }

    #[test]
    fn a_transaction_names_the_step_it_is() {
        assert_eq!(MEMBER_LOGIN.name(), "the member login");
        assert_eq!(SELECT_SERVICE.name(), "the world select");
        assert_ne!(Transaction::new(9, 9).name(), MEMBER_LOGIN.name());
    }

    #[test]
    fn the_reply_header_reads_only_the_four_live_fields() {
        let mut buf = [0u8; REPLY_HEADER_LEN];
        buf[0x01] = 0;
        buf[0x04..0x08].copy_from_slice(&0x20u32.to_le_bytes());
        buf[0x14..0x18].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let reply = parse_reply_header(&buf).unwrap();
        assert!(reply.is_ok());
        assert_eq!(reply.body_len, 0x20);
        assert_eq!(reply.token, [0xDE, 0xAD, 0xBE, 0xEF]);
        buf[0x01] = 0x79;
        assert!(!parse_reply_header(&buf).unwrap().is_ok());
    }

    #[test]
    fn the_login_body_takes_a_full_width_name_and_an_empty_one() {
        let mut cred = MemberCredential::new("SIXTEENCHARSXYZ0", "hunter2", None);
        assert_eq!(cred.name.len(), LOGIN_NAME_MAX);
        let body = member_login_body(&cred, 0).unwrap();
        assert_eq!(&body[1..1 + LOGIN_NAME_MAX], cred.name.as_bytes());
        // The terminator lands on the byte before the one-time password.
        assert_eq!(body[1 + LOGIN_NAME_MAX], 0);

        cred.name.push('X');
        assert!(member_login_body(&cred, 0).is_err());

        // The Viewer allows an account with no Square Enix id at all.
        cred.name.clear();
        let body = member_login_body(&cred, 0).unwrap();
        assert_eq!(&body[1..1 + LOGIN_NAME_MAX], &[0u8; LOGIN_NAME_MAX]);
    }

    #[test]
    fn the_password_secret_is_two_sha1_passes_with_the_salt() {
        // Self-derived from the static reading of app.dll 7ba99828, not
        // captured from any live server.
        assert_eq!(
            hex::encode(member_secret("hunter2")),
            "505b52b912143468cece9546d5118d9a20689222"
        );
        let want: [u8; SECRET_LEN] = {
            let inner: [u8; SECRET_LEN] = Sha1::digest(b"hunter2").into();
            let mut outer = Sha1::new();
            outer.update(hex::encode(inner).as_bytes());
            outer.update(SECRET_SALT);
            outer.finalize().into()
        };
        assert_eq!(member_secret("hunter2"), want);
    }

    #[test]
    fn the_password_secret_hashes_the_low_byte_of_each_code_unit() {
        // The Viewer feeds UTF-16 code units one low byte at a time, so a
        // character above U+00FF is truncated rather than encoded.
        assert_eq!(member_secret("\u{100}"), member_secret("\u{0}"));
        assert_ne!(member_secret("\u{e9}"), member_secret("e"));
    }

    #[test]
    fn a_credential_from_a_password_carries_the_derived_secret() {
        let cred = MemberCredential::new("TESTMEMBER", "hunter2", None);
        assert_eq!(cred.secret20, member_secret("hunter2"));
        assert!(cred.otp.is_none());
    }

    #[test]
    fn the_member_login_body_has_the_pinned_shape() {
        let cred = MemberCredential {
            name: "TESTMEMBER".to_string(),
            secret20: [0x11; 20],
            otp: None,
        };
        let unix_secs = 1_700_000_077u64;
        let body = member_login_body(&cred, unix_secs).unwrap();
        assert_eq!(declared_len(body.len()) as usize, LOGIN_BODY_LEN);
        assert_eq!(body[0], 1);
        assert_eq!(&body[1..11], b"TESTMEMBER");
        // SHA-1 over hex(20x 0x11) and the minute-floored timestamp.
        let mut h = Sha1::new();
        h.update(b"1111111111111111111111111111111111111111");
        h.update(
            (unix_secs - unix_secs % SECONDS_PER_MINUTE)
                .to_string()
                .as_bytes(),
        );
        let want: [u8; 20] = h.finalize().into();
        assert_eq!(&body[0x20..0x34], &want);
    }
}
