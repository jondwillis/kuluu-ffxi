//! The lobby session assembly: the community (category 4, opcode 5) request
//! body, and the transform that turns its reply into the 16-byte value and
//! 64-byte authCode the FFXI lobby validates.
//!
//! Read from polcore.dll build 73b1864b. The op5 reply is server-issued
//! entropy, so a session cannot be minted offline; but the assembly of the
//! reply into the lobby values is entirely client-side, over session state the
//! client already holds after the chat key agreement and member login. This
//! module reproduces `FUN_1001d7e0` (request body), `FUN_1001a2d0` (assembly),
//! and the polcore `*5`-chain cipher behind `0x10074318` that the assembly
//! encrypts with.

use crate::crypto::md5;

/// polcore `0x10074318`: the 8-byte key the assembly cipher schedules from.
const CIPHER_KEY: [u8; 8] = [0xCB, 0x83, 0x24, 0xB7, 0xBA, 0x3E, 0x9E, 0x0A];
/// polcore `0x10007d10`: the additive constant folded into each block.
const D10_ADD: u64 = 0xA165_2347;
/// polcore `0x100714e4`: the substitution table is `table[i] = (i + 0x88)`.
const SUB_ADD: u16 = 0x88;
const SCHEDULE_SEED_BUMP: u8 = 0x45;
const SCHEDULE_STEP_BIAS: u8 = 0x2C;
const SCHEDULE_WORDS: usize = 64;
const SCHEDULE_CHAIN_STEPS: usize = 0x1F;

/// polcore's `*5`-chain block cipher (`FUN_100081a0`), used to encipher the
/// authCode scratch and, elsewhere, to drive the chat keygen PRNG. It is a
/// counter-mode stream over 8-byte blocks with a running checksum.
struct AssemblyCipher {
    table: [u8; SCHEDULE_WORDS * 4],
    pos: u32,
    cksum: u32,
}

impl AssemblyCipher {
    /// polcore `0x10007bc0`: seed two words from the key, byte-mix them, then
    /// run a 64-bit `*5` chain to fill the 256-byte table.
    fn new(key: [u8; 8]) -> Self {
        let mut p = [0u32; SCHEDULE_WORDS];
        let k0 = u32::from_le_bytes(key[0..4].try_into().unwrap());
        let k1 = u32::from_le_bytes(key[4..8].try_into().unwrap());
        p[0] = k1.rotate_left(16);
        p[1] = k0.rotate_left(8);

        let mut b = [0u8; 8];
        b[0..4].copy_from_slice(&p[0].to_le_bytes());
        b[4..8].copy_from_slice(&p[1].to_le_bytes());
        b[0] = b[0].wrapping_add(SCHEDULE_SEED_BUMP);
        for i in 1..8 {
            let v = b[i].wrapping_add(b[i - 1]).wrapping_sub(SCHEDULE_STEP_BIAS);
            b[i] = (b[i - 1] << 2) ^ v ^ SCHEDULE_SEED_BUMP;
        }
        p[0] = u32::from_le_bytes(b[0..4].try_into().unwrap());
        p[1] = u32::from_le_bytes(b[4..8].try_into().unwrap());

        for i in 0..SCHEDULE_CHAIN_STEPS {
            let cur = (u64::from(p[2 * i + 1]) << 32) | u64::from(p[2 * i]);
            let prod = cur.wrapping_mul(5);
            p[2 * i + 2] = prod as u32;
            p[2 * i + 3] = (prod >> 32) as u32;
        }

        let mut table = [0u8; SCHEDULE_WORDS * 4];
        for (w, chunk) in p.iter().zip(table.chunks_exact_mut(4)) {
            chunk.copy_from_slice(&w.to_le_bytes());
        }
        Self {
            table,
            pos: 0,
            cksum: 0,
        }
    }

    fn sched64(&self, idx: usize) -> u64 {
        u64::from_le_bytes(self.table[idx * 8..idx * 8 + 8].try_into().unwrap())
    }

    /// polcore `0x10007d10`: swap halves, XOR the schedule word, XOR a
    /// counter expansion, add the schedule word back.
    fn block_mix(&self, block: [u8; 8]) -> [u8; 8] {
        let ctr = u64::from(self.pos);
        let idx = ((self.pos >> 3) & 0x1F) as usize;
        let in_lo = u32::from_le_bytes(block[0..4].try_into().unwrap());
        let in_hi = u32::from_le_bytes(block[4..8].try_into().unwrap());
        let mut o = (u64::from(in_lo) << 32) | u64::from(in_hi);
        let s = self.sched64(idx);
        o ^= s;
        let mut v = ctr;
        for _ in 0..3 {
            v = (v << 10) | ctr;
        }
        v = v.wrapping_add(D10_ADD);
        o ^= v;
        o = o.wrapping_add(s);
        o.to_le_bytes()
    }

    /// polcore `0x10007de0`: eight additive-table rounds per byte, then an
    /// output XOR chain seeded from the counter.
    fn block_substitute(&self, block: [u8; 8]) -> [u8; 8] {
        let idx = ((self.pos >> 3) & 0x1F) as usize;
        let mut chain = ((self.pos >> 3) as u8) ^ SCHEDULE_SEED_BUMP;
        let base = idx * 8;
        let mut out = [0u8; 8];
        for (i, o) in out.iter_mut().enumerate() {
            let mut cur = u16::from(block[i]);
            for j in 0..8 {
                cur = (u16::from(self.table[base + j]) + cur + SUB_ADD) & 0xFF;
            }
            let byte = (cur as u8) ^ chain;
            *o = byte;
            chain = byte;
        }
        out
    }

    /// polcore `0x10007c90`: encipher whole 8-byte blocks, advancing the
    /// counter and folding each block into the running checksum.
    fn run(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        for chunk in data.chunks_exact(8) {
            let block: [u8; 8] = chunk.try_into().unwrap();
            let acc = self.cksum.wrapping_add(u32::from(block[0]));
            self.cksum = u32::from(block[4]).wrapping_add(acc);
            let mixed = self.block_substitute(self.block_mix(block));
            out.extend_from_slice(&mixed);
            self.pos = self.pos.wrapping_add(8);
        }
        out
    }

    /// polcore `0x10007e50`: the ciphertext, plus the 8-byte finalization tag
    /// `{len, checksum}` the caller appends contiguously.
    fn stream(mut self, data: &[u8]) -> (Vec<u8>, [u8; 8]) {
        let cipher = self.run(data);
        let mut fin = [0u8; 8];
        fin[0..4].copy_from_slice(&(data.len() as u32).to_le_bytes());
        fin[4..8].copy_from_slice(&self.cksum.to_le_bytes());
        let tag = self.run(&fin);
        (cipher, tag.try_into().unwrap())
    }
}

fn cipher_encrypt(data: &[u8]) -> (Vec<u8>, [u8; 8]) {
    AssemblyCipher::new(CIPHER_KEY).stream(data)
}

/// The 0x24-byte community request payload. It carries the world/service
/// selection only; the member is bound by the request header authenticator.
/// The world select sends the head of this same buffer and its reply fills
/// the rest, so `SelectReply::confirm` is what normally builds one.
pub struct CommunityRequest {
    /// polcore `0x1001d7e0`: which of the account's content entries to play,
    /// as the six-bit ordinal the Viewer shows as "Content ID-N". It is not a
    /// world number; the world is the service slot below.
    pub content_index: u8,
    pub service_available: bool,
    /// Which of the content entry's eight service slots was selected.
    pub service_index: u8,
    /// polcore writes the negation of its context flag here, so this is set
    /// when no context has been established.
    pub context_unset: bool,
    pub region: u16,
    pub world_index_plus1: u8,
    pub service_flag: bool,
    /// Set when every parameter came from the cached defaults rather than
    /// being supplied by the caller.
    pub cached: bool,
}

/// The community request payload length before its checksum and encipherment.
pub const REQUEST_PAYLOAD_LEN: usize = 0x24;
/// polcore `0x1001d490` sends only this much of the buffer for a world select.
pub const SELECT_PAYLOAD_LEN: usize = 0x14;
const REQUEST_CONST_FLAG: u8 = 1;

/// polcore `0x1001d220`: the region the selection globals start at, before
/// any world select has confirmed one.
const REGION_DEFAULT: u16 = 1000;

impl CommunityRequest {
    /// What a client that has never selected anything asks with. polcore's
    /// selection globals start with no content entry and no world, and its
    /// request builder turns that into a zeroed entry and index; the cached
    /// flag is clear because nothing has been confirmed yet.
    pub fn initial() -> Self {
        Self {
            content_index: 0,
            service_available: false,
            service_index: 0,
            context_unset: true,
            region: REGION_DEFAULT,
            world_index_plus1: 0,
            service_flag: false,
            cached: false,
        }
    }

    /// polcore `0x1001d7e0`: build the request payload.
    pub fn payload(&self) -> [u8; REQUEST_PAYLOAD_LEN] {
        let mut body = [0u8; REQUEST_PAYLOAD_LEN];
        body[0x10] = self.content_index;
        body[0x11] = u8::from(self.service_available);
        body[0x12] = self.service_index;
        body[0x13] = u8::from(self.context_unset);
        body[0x14..0x16].copy_from_slice(&self.region.to_le_bytes());
        body[0x16] = self.world_index_plus1;
        body[0x17] = u8::from(self.service_flag);
        body[0x18] = u8::from(self.cached);
        body[0x19] = REQUEST_CONST_FLAG;
        body
    }
}

/// The world-select reply body as it arrives. polcore reads a fixed 0x80
/// bytes for it, which like every declared body length counts the four-byte
/// checksum the trailer carries.
pub const SELECT_REPLY_WIRE_LEN: usize = 0x80;
/// What is left once the trailer is stripped, which is what `parse` reads.
/// The client reads the leading sixteen bytes of it and one flag near the end.
pub const SELECT_REPLY_LEN: usize = SELECT_REPLY_WIRE_LEN - crate::profile::BODY_CHECKSUM_LEN;
/// A content entry is addressed by a six-bit ordinal, so an index at or above
/// this is not one.
const CONTENT_INDEX_LIMIT: u8 = 0x40;
/// polcore `0x1001d220` initialises the world index to this when the reply
/// names no world.
const WORLD_INDEX_DEFAULT: u8 = 4;
const SELECT_CACHED_OFFSET: usize = 0x76;
const SELECT_CACHED_BIT: u8 = 1;
const SELECT_CONTEXT_SET: u8 = 1;

/// polcore `0x1001d490` case 6: what the world select establishes. Every one
/// of these lands in a global that the next community request reads back, so
/// the reply is how a selection is confirmed rather than merely acknowledged.
/// It carries no lobby address; the two dwords at 0x08 are stored and never
/// read again by any code in the module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectReply {
    /// `None` when the reply names no world, which also resets the index.
    pub content_index: Option<u8>,
    pub service_available: bool,
    pub service_index: u8,
    pub context_set: bool,
    pub region: u16,
    pub world_index: u8,
    pub service_flag: u8,
    /// Stored by polcore and read by nothing in it.
    pub opaque: [u32; 2],
    pub cached: bool,
}

impl SelectReply {
    pub fn parse(body: &[u8]) -> crate::Result<Self> {
        if body.len() < SELECT_REPLY_LEN {
            return Err(crate::Error::protocol("a world-select reply is 0x80 bytes"));
        }
        let named = body[0x06] != 0;
        Ok(Self {
            content_index: (named && body[0x00] < CONTENT_INDEX_LIMIT).then_some(body[0x00]),
            service_available: body[0x01] != 0,
            service_index: body[0x02],
            context_set: body[0x03] == SELECT_CONTEXT_SET,
            region: u16::from_le_bytes(body[0x04..0x06].try_into().unwrap()),
            world_index: if named {
                body[0x06] - 1
            } else {
                WORLD_INDEX_DEFAULT
            },
            service_flag: body[0x07],
            opaque: [
                u32::from_le_bytes(body[0x08..0x0C].try_into().unwrap()),
                u32::from_le_bytes(body[0x0C..0x10].try_into().unwrap()),
            ],
            cached: body[SELECT_CACHED_OFFSET] & SELECT_CACHED_BIT != 0,
        })
    }

    /// The community request this reply establishes. `previous` supplies the
    /// content index when the reply named no world, which is how polcore's
    /// globals behave: that case leaves the stored index alone.
    pub fn confirm(&self, previous: &CommunityRequest) -> CommunityRequest {
        CommunityRequest {
            content_index: self.content_index.unwrap_or(previous.content_index),
            service_available: self.service_available,
            service_index: self.service_index,
            context_unset: !self.context_set,
            region: self.region,
            world_index_plus1: self.world_index + 1,
            service_flag: self.service_flag != 0,
            cached: self.cached,
        }
    }
}

/// The 16-byte lobby value the reply carries in its first sixteen bytes.
pub const VALUE_LEN: usize = 0x10;
/// The reply body, including its 4-byte checksum trailer.
pub const REPLY_LEN: usize = 0x20;
/// The bytes polcore fills of the authCode; the lobby field is 64, the rest is
/// uninitialised on the retail client and ignored by the server.
pub const AUTHCODE_MEANINGFUL_LEN: usize = 0x34;
pub const AUTHCODE_WIRE_LEN: usize = 0x40;

/// The session state the client holds when it assembles the reply.
pub struct AssemblyInputs<'a> {
    /// The 20-byte POL address struct (`0x100aa8d0`).
    pub addr20: &'a [u8; 20],
    /// The clock-sync word from the chat greeting (`0x100aa850`).
    pub clock: u32,
    /// The negotiated chat Blowfish key (`0x100aa858`).
    pub chat_key: [u8; 8],
    /// The profile host-index byte (`0x10404a92`).
    pub host: u8,
}

/// The 16-byte value from the reply's first sixteen bytes.
pub fn sixteen_byte_value(reply: &[u8]) -> [u8; VALUE_LEN] {
    reply[..VALUE_LEN].try_into().unwrap()
}

const SCRATCH_LEN: usize = 0x48;
const CIPHER_INPUT_LEN: usize = 0x28;

/// polcore `0x1001a2d0`: assemble the 0x34-byte authCode from the reply and
/// the client's session state.
pub fn build_authcode(reply: &[u8], inputs: &AssemblyInputs) -> [u8; AUTHCODE_MEANINGFUL_LEN] {
    let mut scratch = [0u8; SCRATCH_LEN];
    let mut mixed = Vec::with_capacity(16 + 4 + 8);
    mixed.extend_from_slice(&reply[..16]);
    mixed.extend_from_slice(&inputs.clock.to_le_bytes());
    mixed.extend_from_slice(&inputs.chat_key);
    let digest = md5(&mixed);
    scratch[0x18..0x28].copy_from_slice(&digest);
    scratch[0x04..0x18].copy_from_slice(inputs.addr20);
    scratch[0x00] = scratch[0x25] ^ scratch[0x1B];
    scratch[0x01] = inputs.host;

    let (cipher, tag) = cipher_encrypt(&scratch[..CIPHER_INPUT_LEN]);

    let mut out = [0u8; AUTHCODE_MEANINGFUL_LEN];
    out[0x04..0x04 + cipher.len()].copy_from_slice(&cipher);
    out[0x04 + CIPHER_INPUT_LEN..0x04 + CIPHER_INPUT_LEN + 8].copy_from_slice(&tag);
    for i in 5..AUTHCODE_MEANINGFUL_LEN {
        out[i] ^= out[i - 1];
    }
    out[0] = out[0x0E] ^ inputs.host;
    out[1] = out[0x0C] ^ out[6];
    out[2] = out[0x0A] ^ out[8];
    out[3] = out[0x0D] ^ out[9];
    out
}

/// The full state-5 transform: the 16-byte value and the authCode, widened to
/// the 64-byte lobby field (the tail bytes are left zero rather than the
/// retail client's uninitialised stack).
pub fn assemble_session(
    reply: &[u8],
    inputs: &AssemblyInputs,
) -> ([u8; VALUE_LEN], [u8; AUTHCODE_WIRE_LEN]) {
    let value = sixteen_byte_value(reply);
    let core = build_authcode(reply, inputs);
    let mut wire = [0u8; AUTHCODE_WIRE_LEN];
    wire[..AUTHCODE_MEANINGFUL_LEN].copy_from_slice(&core);
    (value, wire)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Vectors self-derived from the static reading of polcore.dll 73b1864b,
    // not captured from any live server.
    #[test]
    fn the_cipher_matches_the_pinned_vector() {
        let data: Vec<u8> = (0..0x28u8).collect();
        let (cipher, tag) = cipher_encrypt(&data);
        assert_eq!(
            hex::encode(&cipher),
            "41f9fdc5809ab6926b49ee2d4acccd2dcdb0cbfad809d59f1712f0d7d25596d7700405419bf3b2e8"
        );
        assert_eq!(hex::encode(tag), "b8b15ec730836d17");
    }

    fn a_request() -> CommunityRequest {
        CommunityRequest {
            content_index: 0x1C,
            service_available: true,
            service_index: 3,
            context_unset: true,
            region: 0x03E8,
            world_index_plus1: 5,
            service_flag: true,
            cached: false,
        }
    }

    #[test]
    fn the_request_body_places_the_world_selection() {
        let body = a_request().payload();
        assert_eq!(&body[..0x10], &[0u8; 0x10]);
        assert_eq!(body[0x10], 0x1C);
        assert_eq!(u16::from_le_bytes([body[0x14], body[0x15]]), 0x03E8);
        assert_eq!(body[0x19], 1);
        // The world select sends the head of the same buffer, so it carries
        // the content entry and its service slot but not the region.
        assert_eq!(&body[..SELECT_PAYLOAD_LEN][0x10..], &[0x1C, 1, 3, 1]);
    }

    #[test]
    fn the_world_select_reply_confirms_the_selection() {
        let mut reply = [0u8; SELECT_REPLY_LEN];
        reply[0x00] = 0x1C;
        reply[0x01] = 1;
        reply[0x02] = 3;
        reply[0x03] = 1;
        reply[0x04..0x06].copy_from_slice(&0x03E8u16.to_le_bytes());
        reply[0x06] = 5;
        reply[0x07] = 1;
        reply[0x76] = 1;

        let decoded = SelectReply::parse(&reply).unwrap();
        assert_eq!(decoded.content_index, Some(0x1C));
        assert_eq!(decoded.world_index, 4);
        assert_eq!(decoded.region, 0x03E8);
        assert!(decoded.context_set);
        assert!(decoded.cached);

        let confirmed = decoded.confirm(&a_request());
        assert_eq!(
            confirmed.payload()[0x10..0x1A],
            [0x1C, 1, 3, 0, 0xE8, 0x03, 5, 1, 1, 1]
        );
    }

    #[test]
    fn a_reply_that_names_no_world_keeps_the_previous_content() {
        let reply = [0u8; SELECT_REPLY_LEN];
        let decoded = SelectReply::parse(&reply).unwrap();
        assert_eq!(decoded.content_index, None);
        assert_eq!(decoded.world_index, 4);
        assert_eq!(decoded.confirm(&a_request()).content_index, 0x1C);
        assert!(SelectReply::parse(&reply[..SELECT_REPLY_LEN - 1]).is_err());
    }

    #[test]
    fn the_reply_is_sized_by_its_payload_not_its_wire_body() {
        // polcore reads a fixed 0x80-byte body, and like every declared
        // length that counts the checksum trailer, so a reply that parsed
        // only at the wire width would reject every real one.
        assert_eq!(
            SELECT_REPLY_LEN + crate::profile::BODY_CHECKSUM_LEN,
            SELECT_REPLY_WIRE_LEN
        );
        let wire = crate::profile::seal_body(&[0u8; SELECT_REPLY_LEN]);
        assert_eq!(wire.len(), SELECT_REPLY_WIRE_LEN);
        let payload = crate::profile::open_body(&wire).unwrap();
        assert!(SelectReply::parse(&payload).is_ok());
    }

    #[test]
    fn the_assembly_matches_the_pinned_vector() {
        let mut reply = [0u8; REPLY_LEN];
        reply[..16].copy_from_slice(&[
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE,
            0xFF, 0x01,
        ]);
        for (i, b) in reply[16..28].iter_mut().enumerate() {
            *b = (i + 2) as u8;
        }
        let mut addr20 = [0u8; 20];
        for (i, b) in addr20.iter_mut().enumerate() {
            *b = 0x20 + i as u8;
        }
        let inputs = AssemblyInputs {
            addr20: &addr20,
            clock: 0x5F5E_0100,
            chat_key: [1, 2, 3, 4, 5, 6, 7, 8],
            host: 0x07,
        };
        let (value, wire) = assemble_session(&reply, &inputs);
        assert_eq!(hex::encode(value), "112233445566778899aabbccddeeff01");
        assert_eq!(
            hex::encode(&wire[..AUTHCODE_MEANINGFUL_LEN]),
            "3c7d2e21ad948588c390edb7f8b13b2e7044adf8d1994e940051c877c82d5bc8afebe654639dea5eb7a4b246574cb8d54f66a11c"
        );
        // The fixups bind the head to the enciphered body.
        assert_eq!(wire[0], wire[0x0E] ^ inputs.host);
        assert_eq!(wire[1], wire[0x0C] ^ wire[6]);
    }
}
