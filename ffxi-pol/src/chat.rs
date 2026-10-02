//! The chat service: IRC line framing with polcore's 4-character checksum, the
//! member-handle codec, the NICK digest and token, the greeting codec, and the
//! USER/NICK builders.
//!
//! Read from polcore.dll build 73b1864b. The chat service is where the session
//! Blowfish key is agreed: the client sends its RSA modulus in the USER
//! realname, the server returns the key in numeric 300 (see `crate::rsa`), and
//! every line after that is enciphered (see `crate::crypto`). These are pure
//! functions; sequencing lives in `crate::transport`.

use md5::{Digest as _, Md5};

use crate::crypto::{checksum, md5};
use crate::profile::MEMBER_ID_LEN;
use crate::rsa::{b64_encode, KeyPair};

const CHECKSUM_CHARS: usize = 4;
/// polcore `0x10013730`: each checksum character is a 6-bit group biased here,
/// giving the printable range '?'..'~'.
const CHECKSUM_BIAS: u8 = 0x3F;
const LINE_TERMINATOR: &[u8] = b"\r\n";
/// The checksum encodes bits 8..31 of the sum, most significant group first.
const CHECKSUM_SHIFTS: [u32; CHECKSUM_CHARS] = [26, 20, 14, 8];

/// The ports the chat service listens on; the viewer picks one per attempt.
pub const PORTS: [u16; 3] = [51240, 51241, 51242];

/// polcore `0x10013730`: encode bits 8..31 of the line checksum as four biased
/// characters. Bits 0..7 are discarded.
fn encode_checksum(sum: u32) -> [u8; CHECKSUM_CHARS] {
    let mut out = [0u8; CHECKSUM_CHARS];
    for (o, shift) in out.iter_mut().zip(CHECKSUM_SHIFTS) {
        *o = (((sum >> shift) & 0x3F) as u8) + CHECKSUM_BIAS;
    }
    out
}

fn decode_checksum(chars: &[u8]) -> u32 {
    let mut v = 0u32;
    for (&c, shift) in chars.iter().zip(CHECKSUM_SHIFTS) {
        v = v.wrapping_add((u32::from(c.wrapping_sub(CHECKSUM_BIAS)) & 0x3F) << shift);
    }
    v & 0xFFFF_FF00
}

/// polcore `0x10013730`: frame a command line as the bytes handed to the
/// transport. The checksum covers the body only, not the checksum field or the
/// CRLF.
pub fn frame_line(body: &str) -> Vec<u8> {
    let body = body.as_bytes();
    let mut out = Vec::with_capacity(body.len() + CHECKSUM_CHARS + LINE_TERMINATOR.len());
    out.extend_from_slice(body);
    out.extend_from_slice(&encode_checksum(checksum(body, 0)));
    out.extend_from_slice(LINE_TERMINATOR);
    out
}

/// polcore `0x10015e80`: verify the 4-character checksum of a received line
/// (after the stream cipher has been undone).
pub fn verify_line(raw: &[u8]) -> bool {
    let line = strip_terminator(raw);
    if line.len() < CHECKSUM_CHARS {
        return false;
    }
    let (body, field) = line.split_at(line.len() - CHECKSUM_CHARS);
    decode_checksum(field) == (checksum(body, 0) & 0xFFFF_FF00)
}

fn strip_terminator(raw: &[u8]) -> &[u8] {
    let mut end = raw.len();
    while end > 0 && (raw[end - 1] == b'\r' || raw[end - 1] == b'\n') {
        end -= 1;
    }
    &raw[..end]
}

/// The body of a received line with the terminator and, when present, the
/// 4-character checksum removed.
pub fn line_body(raw: &[u8], has_checksum: bool) -> &[u8] {
    let line = strip_terminator(raw);
    if has_checksum && line.len() >= CHECKSUM_CHARS {
        &line[..line.len() - CHECKSUM_CHARS]
    } else {
        line
    }
}

const ID_ALPHABET: &[u8; 36] = b"EFKAOYMJVNGTDSWBQLPCIRHZXU6328401795";
const NICK_PREFIX: u8 = b'U';
const NICK_DIGITS: usize = 8;
const NICK_ID_BITS: u32 = 41;
/// polcore `0x1001a390` sets bits 46 and 47 before the diffusion pass and
/// clears them after, so byte 5 is a known constant during the pass.
const NICK_GUARD: u64 = 0xC000 << 32;

fn nick_diffuse(mut v: u64, descending: bool) -> u64 {
    v |= NICK_GUARD;
    let order: [u32; 5] = if descending {
        [4, 3, 2, 1, 0]
    } else {
        [0, 1, 2, 3, 4]
    };
    for k in order {
        v ^= (0xFFu64 << (8 * k)) & (v >> 8);
    }
    v & !NICK_GUARD
}

/// polcore `0x1001a390`: render a 41-bit member handle as the nine-character
/// NICK string (a 'U' prefix and eight base-36 digits).
pub fn id_to_nick(account_id: u64) -> String {
    let mut v = nick_diffuse(account_id & ((1 << NICK_ID_BITS) - 1), true);
    let mut digits = [0u8; NICK_DIGITS];
    for slot in digits.iter_mut().rev() {
        *slot = ID_ALPHABET[(v % 36) as usize];
        v /= 36;
    }
    let mut out = String::with_capacity(NICK_DIGITS + 1);
    out.push(NICK_PREFIX as char);
    out.push_str(std::str::from_utf8(&digits).unwrap());
    out
}

/// polcore `0x10016ae0`: the MD5 whose hex half goes into NICK, over the
/// challenge bytes then the credential up to but not including its NUL.
pub fn nick_digest(challenge: &[u8], credential: &[u8]) -> [u8; 16] {
    let cred = credential
        .iter()
        .position(|&b| b == 0)
        .map_or(credential, |n| &credential[..n]);
    let mut h = Md5::new();
    h.update(challenge);
    h.update(cred);
    h.finalize().into()
}

/// polcore `0x10071464`: the base32 alphabet the greeting and the routing
/// struct are carried in. The bit packing is the ordinary big-endian five-bit
/// stream; only the alphabet is scrambled.
const B32_ALPHABET: &[u8; 32] = b"N43OVHBJ1Y2C0WSXED5QFILRZMUTAPGK";
const B32_BITS: u32 = 5;
const B32_MASK: u32 = (1 << B32_BITS) - 1;
const BITS_PER_BYTE: u32 = 8;

/// polcore `0x10007210`: decode a base32 field. polcore maps every character
/// through a reverse table and trims by length instead; this stops at the
/// first character outside the alphabet, which is equivalent for every caller
/// here because each one trims the line's checksum characters first.
pub fn b32_decode(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * B32_BITS as usize / BITS_PER_BYTE as usize);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &c in text {
        let Some(v) = B32_ALPHABET.iter().position(|&a| a == c) else {
            break;
        };
        acc = (acc << B32_BITS) | v as u32;
        bits += B32_BITS;
        if bits >= BITS_PER_BYTE {
            bits -= BITS_PER_BYTE;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// polcore `0x10007590`: the encoder, kept so the decoder can be pinned
/// against it and so a mock server can produce a greeting.
pub fn b32_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * BITS_PER_BYTE as usize / B32_BITS as usize);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &b in data {
        acc = (acc << BITS_PER_BYTE) | u32::from(b);
        bits += BITS_PER_BYTE;
        while bits >= B32_BITS {
            bits -= B32_BITS;
            out.push(B32_ALPHABET[((acc >> bits) & B32_MASK) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(B32_ALPHABET[((acc << (B32_BITS - bits)) & B32_MASK) as usize] as char);
    }
    out
}

/// polcore `0x10019f20`: the 8-byte pad the process derives from the member
/// identity. In the Viewer it obfuscates the identity and the profile secret
/// in memory, which a reimplementation has no need of; it matters here only
/// because the NICK token hashes it, so it does reach the wire.
const PAD_SEED: u64 = 0x7048_860D_DF79;
const PAD_LCG_MUL: u32 = 0x425F_0CBD;
const PAD_LCG_ADD: u32 = 0x7F4F;
/// The character the LCG step count is measured from; polcore skips a byte at
/// or below it rather than running the loop a negative number of times.
const PAD_LCG_FLOOR: u8 = 0x30;
const PAD_DIFFUSE_FLAG: u8 = 2;
const PAD_DIFFUSE_SET: u8 = 0x80;

pub fn identity_pad(identity: &[u8; MEMBER_ID_LEN]) -> [u8; 8] {
    let mut b = *identity;
    for i in 1..MEMBER_ID_LEN {
        b[i] ^= b[i - 1];
        if b[i] & PAD_DIFFUSE_FLAG != 0 {
            b[i] |= PAD_DIFFUSE_SET;
        }
    }

    let mut acc = 1u64;
    let mut mixed = PAD_SEED;
    for i in 1..MEMBER_ID_LEN {
        acc = acc.wrapping_add(u64::from(b[i]));
        mixed = mixed.wrapping_mul(u64::from(b[i - 1]) * u64::from(b[i]));
    }

    let mut lcg = 0u32;
    for &c in b.iter() {
        for _ in PAD_LCG_FLOOR..c {
            lcg = lcg.wrapping_mul(PAD_LCG_MUL).wrapping_add(PAD_LCG_ADD);
        }
    }

    let mixed = mixed.wrapping_add(acc);
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&((mixed as u32) ^ lcg).to_le_bytes());
    out[4..].copy_from_slice(&((mixed >> 32) as u32).to_le_bytes());
    out
}

/// The NICK token record and the host record it digests, in polcore
/// `0x100198f0`.
const TOKEN_RECORD_LEN: usize = 0x18;
const TOKEN_LEAD: [u8; 5] = [0x00, 0x00, 0x00, 0x02, 0x01];
const TOKEN_VERSION_OFFSET: usize = 0x05;
const TOKEN_SESSION_OFFSET: usize = 0x08;
const TOKEN_HOST_OFFSET: usize = 0x10;
const TOKEN_FIELD_LEN: usize = 8;
const HOST_RECORD_LEN: usize = 0x52;
const HOST_NAME_LEN: usize = 0x40;
/// polcore copies the name with a cap one short of the field width, so the
/// record always holds a terminator.
const HOST_NAME_MAX: usize = HOST_NAME_LEN - 1;
const HOST_PAD_OFFSET: usize = 0x40;
const HOST_SERIAL_OFFSET: usize = 0x48;
const HOST_ADDRESS_OFFSET: usize = 0x4C;
const HOST_ADDRESS_LEN: usize = 6;

/// polcore `0x10011fd0` substitutes this literal when its setter is handed a
/// null name, and no caller inside the module ever hands it another, so it is
/// what a faithful client sends unless it is told otherwise.
pub const DEFAULT_HOST_NAME: &str = "NOTIMPLEMENTED_YET";

/// The machine-scoped inputs to the NICK token. Every one of them is written
/// by a setter outside polcore, and none of those setters is reachable from
/// inside it, so on this build they hold their load-time values: the name
/// falls back to `DEFAULT_HOST_NAME` and the rest are zero.
#[derive(Clone, Debug)]
pub struct TokenInputs<'a> {
    /// polcore `0x100a666c`, at most 63 characters.
    pub host_name: &'a str,
    /// polcore `0x100a66dc`.
    pub host_serial: u32,
    /// polcore `0x100a66b4`; its width is that of a hardware address.
    pub host_address: [u8; HOST_ADDRESS_LEN],
    /// polcore `0x100aa8f0`, the leading half of a digest its setter takes.
    pub session_digest: [u8; TOKEN_FIELD_LEN],
    /// polcore `0x1007427d`, a version stamp its setter folds from two bytes.
    pub version: u8,
}

impl Default for TokenInputs<'_> {
    fn default() -> Self {
        Self {
            host_name: DEFAULT_HOST_NAME,
            host_serial: 0,
            host_address: [0; HOST_ADDRESS_LEN],
            session_digest: [0; TOKEN_FIELD_LEN],
            version: 0,
        }
    }
}

/// polcore `0x100198f0`: the 32-character third part of the NICK. It is a
/// 24-byte record in the scrambled base64, and the only part of it that
/// varies with the account is the digest over the host record, which carries
/// the identity pad.
pub fn nick_token(identity: &[u8; MEMBER_ID_LEN], inputs: &TokenInputs) -> String {
    let mut host = [0u8; HOST_RECORD_LEN];
    let name = inputs.host_name.as_bytes();
    let kept = name.len().min(HOST_NAME_MAX);
    host[..kept].copy_from_slice(&name[..kept]);
    host[HOST_PAD_OFFSET..HOST_SERIAL_OFFSET].copy_from_slice(&identity_pad(identity));
    host[HOST_SERIAL_OFFSET..HOST_ADDRESS_OFFSET]
        .copy_from_slice(&inputs.host_serial.to_le_bytes());
    host[HOST_ADDRESS_OFFSET..].copy_from_slice(&inputs.host_address);

    let mut record = [0u8; TOKEN_RECORD_LEN];
    record[..TOKEN_LEAD.len()].copy_from_slice(&TOKEN_LEAD);
    record[TOKEN_VERSION_OFFSET] = inputs.version;
    record[TOKEN_SESSION_OFFSET..TOKEN_HOST_OFFSET].copy_from_slice(&inputs.session_digest);
    record[TOKEN_HOST_OFFSET..].copy_from_slice(&md5(&host)[..TOKEN_FIELD_LEN]);
    b64_encode(&record)
}

/// polcore `0x10015e80` state 5: the greeting's third field also base32-decodes
/// to this many bytes, of which the client keeps every one.
pub const GREETING_DECODED_LEN: usize = 0x18;
/// polcore `0x100aa8d0`: POL's own address struct, `{u16 family; u16 port;
/// u32 address}` in host order followed by twelve bytes no reader touches.
pub const POL_ADDRESS_LEN: usize = 0x14;
/// Every field of the decoded record is big-endian; polcore byteswaps each one
/// as it interprets it.
const GREETING_CLOCK: std::ops::Range<usize> = 0x00..0x04;
const GREETING_ADDRESS: std::ops::Range<usize> = 0x04..0x08;
const GREETING_REDIRECT: std::ops::Range<usize> = 0x08..0x0C;
const GREETING_REDIRECT_PORT: std::ops::Range<usize> = 0x0C..0x0E;
const GREETING_PORT: std::ops::Range<usize> = 0x14..0x16;
const ADDRESS_FAMILY: u16 = 1;

/// The greeting line the chat service opens with. The client hashes the third
/// field's raw characters into the NICK digest and, separately, base32-decodes
/// the same characters for the addresses below, so both forms are kept.
#[derive(Clone, Debug)]
pub struct Greeting {
    pub challenge: Vec<u8>,
    pub decoded: [u8; GREETING_DECODED_LEN],
}

impl Greeting {
    /// polcore `0x10015e80`: take the challenge from field 3 of a plaintext
    /// line, whose four checksum characters are still attached and which the
    /// handler drops by length rather than by verifying them.
    pub fn parse(field3: &[u8]) -> crate::Result<Self> {
        let challenge = field3
            .len()
            .checked_sub(CHECKSUM_CHARS)
            .map(|n| field3[..n].to_vec())
            .ok_or_else(|| crate::Error::protocol("the chat greeting field is too short"))?;
        let decoded = b32_decode(&challenge);
        let decoded = decoded
            .get(..GREETING_DECODED_LEN)
            .and_then(|d| d.try_into().ok())
            .ok_or_else(|| {
                crate::Error::protocol("the chat greeting does not decode to 0x18 bytes")
            })?;
        Ok(Self { challenge, decoded })
    }

    /// polcore `0x10019b80`: the service's own clock, which the client adopts
    /// as its epoch and which the authCode assembly digests. It is the only
    /// reason the profile leg needs no clock of its own.
    pub fn clock(&self) -> u32 {
        u32::from_be_bytes(self.decoded[GREETING_CLOCK].try_into().unwrap())
    }

    /// A non-zero redirect makes the client reconnect once to that address,
    /// on the port at `redirect_port` when that is set.
    pub fn redirect(&self) -> u32 {
        u32::from_be_bytes(self.decoded[GREETING_REDIRECT].try_into().unwrap())
    }

    pub fn redirect_port(&self) -> u16 {
        u16::from_be_bytes(self.decoded[GREETING_REDIRECT_PORT].try_into().unwrap())
    }

    /// polcore `0x10015e80`: the client's own endpoint as the service reports
    /// it, byteswapped into POL's host-order address struct. The profile
    /// service's plaintext handshake announces the first eight bytes of this.
    pub fn address(&self) -> [u8; POL_ADDRESS_LEN] {
        let mut out = [0u8; POL_ADDRESS_LEN];
        out[0x00..0x02].copy_from_slice(&ADDRESS_FAMILY.to_le_bytes());
        let port = u16::from_be_bytes(self.decoded[GREETING_PORT].try_into().unwrap());
        out[0x02..0x04].copy_from_slice(&port.to_le_bytes());
        let address = u32::from_be_bytes(self.decoded[GREETING_ADDRESS].try_into().unwrap());
        out[0x04..0x08].copy_from_slice(&address.to_le_bytes());
        out
    }
}

/// polcore `0x100a8258`: the struct a numeric 300 outside the key-agreement
/// state carries, which is where the profile service's host number comes from.
pub const ROUTING_LEN: usize = 0x18;
const ROUTING_GROUP: usize = 4;
const ROUTING_SWAPPED: usize = 2;
const ROUTING_HOST_INDEX: usize = 0x02;
const ROUTING_FLAGS: usize = 0x03;
const ROUTING_REFUSAL: usize = 0x04;
const ROUTING_REFUSED_BIT: u8 = 1;

/// polcore `0x10015a00`: the struct is stored with the leading 16-bit word of
/// every four-byte group byteswapped.
pub fn routing_host_order(wire: &[u8; ROUTING_LEN]) -> [u8; ROUTING_LEN] {
    let mut out = *wire;
    for group in out.chunks_mut(ROUTING_GROUP) {
        group[..ROUTING_SWAPPED].reverse();
    }
    out
}

/// What the client reads out of the routing struct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Routing {
    /// polcore `0x10019dc0` writes this into bits 48..63 of the identity.
    pub region: u16,
    /// polcore `0x10019dc0` writes this into bits 41..47, and `0x1001e8d0`
    /// formats it into the profile host name.
    pub host_index: u8,
    pub flags: u8,
    /// polcore `0x10044a50` state 0x18 fails the login when this is set.
    pub refused: bool,
}

impl Routing {
    /// Read the fields polcore takes out of the host-order struct.
    pub fn parse(host_order: &[u8; ROUTING_LEN]) -> Self {
        Self {
            region: u16::from_le_bytes(host_order[..ROUTING_SWAPPED].try_into().unwrap()),
            host_index: host_order[ROUTING_HOST_INDEX],
            flags: host_order[ROUTING_FLAGS],
            refused: host_order[ROUTING_REFUSAL] & ROUTING_REFUSED_BIT != 0,
        }
    }
}

const USER_ARG0: &str = "x";
const USER_ARG2: &str = "*";

/// polcore `0x10016cf0`: the USER command carrying the client's RSA modulus
/// (base64 of the little-endian modulus bytes) in the realname field.
pub fn build_user(key: &KeyPair, mode: u8) -> Vec<u8> {
    frame_line(&format!(
        "USER {USER_ARG0} {} {USER_ARG2} :{}",
        mode & 0x0F,
        key.realname()
    ))
}

/// polcore `0x10016ae0`: the NICK command. `trailing` is what `nick_token`
/// builds; it is the third colon-separated part, with no trailing-parameter
/// colon anywhere on this command.
pub fn build_nick(account_id: u64, challenge: &[u8], credential: &[u8], trailing: &str) -> Vec<u8> {
    frame_line(&format!(
        "NICK {}:{}:{}",
        id_to_nick(account_id),
        hex::encode(nick_digest(challenge, credential)),
        trailing
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_framed_line_verifies_and_ends_with_crlf() {
        let framed = frame_line("USER x 0 * :blob");
        assert_eq!(&framed[framed.len() - 2..], b"\r\n");
        assert!(verify_line(&framed));
        // The checksum covers bits 8..31 of the dword sum, so a change in a
        // byte above the lowest lane is caught.
        let mut broken = framed.clone();
        broken[1] ^= 0x20;
        assert!(!verify_line(&broken));
    }

    #[test]
    fn line_body_strips_the_checksum_and_terminator() {
        let framed = frame_line("300 target payload");
        assert_eq!(line_body(&framed, true), b"300 target payload");
        assert_eq!(line_body(b"greeting field3\r\n", false), b"greeting field3");
    }

    #[test]
    fn the_nick_handle_round_trips_and_is_nine_characters() {
        for id in [0u64, 1, 0x1_2345, (1 << NICK_ID_BITS) - 1] {
            let nick = id_to_nick(id);
            assert_eq!(nick.len(), NICK_DIGITS + 1);
            assert!(nick.starts_with('U'));
        }
    }

    #[test]
    fn base32_round_trips_and_stops_at_a_foreign_character() {
        let data: Vec<u8> = (0u8..24).collect();
        let text = b32_encode(&data);
        // Self-derived from the static reading of polcore.dll 73b1864b, not
        // captured from any live server.
        assert_eq!(text, "NNNEVNZVNFONS3NY41HEZO1S4A143VEQ3E2D0HZ");
        assert_eq!(&b32_decode(text.as_bytes())[..data.len()], &data[..]);
        // A plaintext line still carries its checksum characters; the decoder
        // stops rather than folding them in.
        let mut trailing = text.clone();
        trailing.push('?');
        assert_eq!(b32_decode(trailing.as_bytes()), b32_decode(text.as_bytes()));
    }

    #[test]
    fn the_identity_pad_matches_the_reference() {
        assert_eq!(hex::encode(identity_pad(b"EFKAOYMJ")), "6cc447f2f5608141");
        assert_ne!(identity_pad(b"EFKAOYMJ"), identity_pad(b"EFKAOYMK"));
    }

    #[test]
    fn the_nick_token_is_thirty_two_characters_over_the_account_pad() {
        let token = nick_token(b"EFKAOYMJ", &TokenInputs::default());
        assert_eq!(token.len(), 32);
        assert_eq!(token, "TTTTTAITTTTTTTTTTTTTTWuYENLoh8kl");
        // Only the host digest varies with the account; the lead, the version
        // and the session slot are the same for every member on this build.
        let other = nick_token(b"EFKAOYMK", &TokenInputs::default());
        assert_eq!(token[..21], other[..21]);
        assert_ne!(token, other);
    }

    #[test]
    fn the_nick_digest_stops_at_the_credential_nul() {
        let with_nul = nick_digest(b"challenge", b"secret\0ignored");
        let without = nick_digest(b"challenge", b"secret");
        assert_eq!(with_nul, without);
    }

    #[test]
    fn the_user_line_carries_the_modulus_realname() {
        use crate::rng::RandomBytes;
        struct Fixed(u64);
        impl RandomBytes for Fixed {
            fn fill(&mut self, out: &mut [u8]) {
                for b in out {
                    self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
                    *b = (self.0 >> 33) as u8;
                }
            }
        }
        let key = KeyPair::generate(&mut Fixed(1));
        let line = build_user(&key, 0);
        let body = line_body(&line, true);
        let text = std::str::from_utf8(body).unwrap();
        assert!(text.starts_with("USER x 0 * :"));
        assert!(text.ends_with(&key.realname()));
    }
}
