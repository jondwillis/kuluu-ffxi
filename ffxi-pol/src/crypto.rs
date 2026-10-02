//! polcore's cipher suite: a key-derived Blowfish variant driven in output
//! feedback with a CR/LF escape, and the profile service's dword-sum checksum.
//!
//! Read statically from polcore.dll build 73b1864b (viewer 1.18.15e),
//! cross-checked against f5af5837 (viewer 1.18.00n). The block cipher and its
//! F function are textbook Blowfish; the key schedule is not, and a stock
//! Blowfish will not interoperate. The three departures that matter:
//!   - the P-array and S-boxes derive from an MD5 expansion of the key, never
//!     from the digits of pi;
//!   - that expansion reuses one MD5 context across rounds without
//!     re-initialising it, so every round after the first hashes under a
//!     zeroed IV (the RFC 1321 reference `MD5Final` zeroes the context);
//!   - the self-encryption pass encrypts the constant zero block each time
//!     rather than chaining the previous output.
//!
//! `PolBlowfish::schedule`, `encrypt_block`, and `stream` cite the polcore
//! routine each reproduces.

use std::sync::LazyLock;

const STATE_WORDS: usize = 4;
const BLOCK_BYTES: usize = 64;

/// RFC 1321 MD5, but with a caller-visible finalize that zeroes the context the
/// way the reference implementation's `MD5Final` does. polcore's key schedule
/// depends on that: it finalizes and then keeps hashing into the same context,
/// so rounds past the first run with an all-zero IV.
struct Md5 {
    state: [u32; STATE_WORDS],
    len_bits: u64,
    buf: Vec<u8>,
}

/// RFC 1321 T[i] = floor(2^32 * abs(sin(i + 1 radians))).
static SINE_TABLE: LazyLock<[u32; 64]> = LazyLock::new(|| {
    let mut t = [0u32; 64];
    for (i, entry) in t.iter_mut().enumerate() {
        let s = ((i + 1) as f64).sin().abs();
        *entry = (s * f64::from(u32::MAX).mul_add(1.0, 1.0)) as u64 as u32;
    }
    t
});

const SHIFTS: [u32; 16] = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];
const MD5_IV: [u32; STATE_WORDS] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476];

impl Md5 {
    fn new() -> Self {
        Self {
            state: MD5_IV,
            len_bits: 0,
            buf: Vec::new(),
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.len_bits = self.len_bits.wrapping_add((data.len() as u64) * 8);
        self.buf.extend_from_slice(data);
        while self.buf.len() >= BLOCK_BYTES {
            let mut block = [0u8; BLOCK_BYTES];
            block.copy_from_slice(&self.buf[..BLOCK_BYTES]);
            self.transform(&block);
            self.buf.drain(..BLOCK_BYTES);
        }
    }

    fn finalize(&mut self) -> [u8; 16] {
        let index = ((self.len_bits >> 3) & 0x3F) as usize;
        let pad_len = if index < 56 { 56 - index } else { 120 - index };
        let mut pad = vec![0u8; pad_len];
        pad[0] = 0x80;
        let bits = self.len_bits;
        self.update(&pad);
        self.update(&bits.to_le_bytes());

        let mut out = [0u8; 16];
        for (word, chunk) in self.state.iter().zip(out.chunks_exact_mut(4)) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        // MD5Final ends in memset(ctx, 0, sizeof *ctx); the schedule reuses the
        // context afterwards without re-init, so the next round hashes under a
        // zero IV.
        self.state = [0; STATE_WORDS];
        self.len_bits = 0;
        self.buf.clear();
        out
    }

    fn transform(&mut self, block: &[u8; BLOCK_BYTES]) {
        let mut x = [0u32; 16];
        for (word, chunk) in x.iter_mut().zip(block.chunks_exact(4)) {
            *word = u32::from_le_bytes(chunk.try_into().unwrap());
        }
        let [mut a, mut b, mut c, mut d] = self.state;
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) & 15),
                32..=47 => (b ^ c ^ d, (3 * i + 5) & 15),
                _ => (c ^ (b | !d), (7 * i) & 15),
            };
            let tmp = d;
            d = c;
            c = b;
            let sum = a
                .wrapping_add(f)
                .wrapping_add(SINE_TABLE[i])
                .wrapping_add(x[g]);
            b = b.wrapping_add(sum.rotate_left(SHIFTS[(i / 16) * 4 + (i % 4)]));
            a = tmp;
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
    }
}

/// Stock RFC 1321 MD5, used by the profile and chat services outside the key
/// schedule (the per-packet authenticator, the NICK digest).
pub fn md5(data: &[u8]) -> [u8; 16] {
    let mut h = Md5::new();
    h.update(data);
    h.finalize()
}

const ROUNDS: usize = 16;
const P_WORDS: usize = 18;
const S_WORDS: usize = 1024;
const S_BOX_ENTRIES: usize = 256;
const EXPAND_BYTES: usize = 0x1000;
const KEYSTREAM_BYTES: usize = 8;

/// The stream layer resets its keystream to the IV at a message boundary and
/// continues across a message body. polcore's `0x10063ef0` takes this as an
/// argument (1 for each IRC line and each fixed profile header, 0 to continue).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StreamReset {
    /// Restart the keystream from the IV (a new message).
    Boundary,
    /// Continue the running keystream (a message body after its header).
    Continue,
}

/// polcore's Blowfish context. The 18 P words and 1024 S words come from the
/// key; the running keystream and IV drive the output-feedback stream layer.
#[derive(Clone)]
pub struct PolBlowfish {
    p: [u32; P_WORDS],
    s: [u32; S_WORDS],
    iv_lo: u32,
    iv_hi: u32,
    run_lo: u32,
    run_hi: u32,
    pos: usize,
}

impl PolBlowfish {
    /// Schedule from the 8-byte session key. The IV seeds the output-feedback
    /// stream; polcore sets it to the low two limbs of the client's ephemeral
    /// RSA modulus, so it is client-chosen per session.
    pub fn new(key: &[u8], iv: (u32, u32)) -> Self {
        let mut bf = Self {
            p: [0; P_WORDS],
            s: [0; S_WORDS],
            iv_lo: iv.0,
            iv_hi: iv.1,
            run_lo: 0,
            run_hi: 0,
            pos: 0,
        };
        bf.schedule(key);
        bf
    }

    /// polcore `0x10064300`. MD5-expand the key to `EXPAND_BYTES`, take P from
    /// overlapping big-endian stride-1 windows and S from big-endian stride-4
    /// words of that buffer, XOR P against the buffer (which the redirected key
    /// pointer has become), then run the non-chaining self-encryption pass.
    fn schedule(&mut self, key: &[u8]) {
        assert!(
            key.len() < EXPAND_BYTES,
            "polcore schedules a >= 0x1000-byte key from uninitialised stack"
        );
        let buf = expand_key(key);

        for (i, entry) in self.p.iter_mut().enumerate() {
            *entry = u32::from_be_bytes(buf[i..i + 4].try_into().unwrap());
        }
        for (j, entry) in self.s.iter_mut().enumerate() {
            *entry = u32::from_be_bytes(buf[4 * j..4 * j + 4].try_into().unwrap());
        }
        // XOR P with the expansion buffer cycled at its own length. The key
        // pointer was redirected to the buffer and the modulus is EXPAND_BYTES,
        // so 18 words never wrap: this reduces to P[i] ^= S[i].
        let mut k = 0usize;
        for entry in &mut self.p {
            let mut w = 0u32;
            for _ in 0..4 {
                w = (w << 8) | u32::from(buf[k]);
                k += 1;
                if k >= EXPAND_BYTES {
                    k = 0;
                }
            }
            *entry ^= w;
        }
        // Self-encryption pass: every block encrypts the constant zero block;
        // polcore does not chain the previous output.
        for i in (0..P_WORDS).step_by(2) {
            let (lo, hi) = self.encrypt_block(0, 0);
            self.p[i] = lo;
            self.p[i + 1] = hi;
        }
        for j in (0..S_WORDS).step_by(2) {
            let (lo, hi) = self.encrypt_block(0, 0);
            self.s[j] = lo;
            self.s[j + 1] = hi;
        }
    }

    /// polcore `0x10064220`: textbook Blowfish-16 over the key-derived tables.
    pub fn encrypt_block(&self, xl: u32, xr: u32) -> (u32, u32) {
        let mut xl = xl;
        let mut xr = xr;
        for i in 0..ROUNDS {
            xl ^= self.p[i];
            xr ^= self.feistel(xl);
            std::mem::swap(&mut xl, &mut xr);
        }
        std::mem::swap(&mut xl, &mut xr);
        (xl ^ self.p[17], xr ^ self.p[16])
    }

    fn feistel(&self, x: u32) -> u32 {
        let a = self.s[(x >> 24) as usize];
        let b = self.s[S_BOX_ENTRIES + ((x >> 16) & 0xFF) as usize];
        let c = self.s[2 * S_BOX_ENTRIES + ((x >> 8) & 0xFF) as usize];
        let d = self.s[3 * S_BOX_ENTRIES + (x & 0xFF) as usize];
        (a.wrapping_add(b) ^ c).wrapping_add(d)
    }

    /// polcore `0x10063ef0`. Output-feedback keystream with a literal
    /// pass-through for CR and LF: a byte that is `\n`/`\r` before or after the
    /// XOR is left as its plaintext, and either way consumes its keystream
    /// byte. The operation is an involution, so one method both enciphers and
    /// deciphers. `StreamReset::Boundary` restarts from the IV; there is one
    /// running state shared by both directions.
    pub fn stream(&mut self, data: &[u8], reset: StreamReset) -> Vec<u8> {
        let (mut lo, mut hi, mut pos) = match reset {
            StreamReset::Boundary => (self.iv_lo, self.iv_hi, 0),
            StreamReset::Continue => (self.run_lo, self.run_hi, self.pos),
        };
        let mut out = Vec::with_capacity(data.len());
        for &b in data {
            pos &= KEYSTREAM_BYTES - 1;
            if pos == 0 {
                let (nlo, nhi) = self.encrypt_block(lo, hi);
                lo = nlo;
                hi = nhi;
            }
            let block = u64::from(lo) | (u64::from(hi) << 32);
            let ks = (block >> (8 * pos)) as u8;
            pos += 1;
            let c = b ^ ks;
            out.push(if is_line_break(b) || is_line_break(c) {
                b
            } else {
                c
            });
        }
        self.run_lo = lo;
        self.run_hi = hi;
        self.pos = pos;
        out
    }
}

fn is_line_break(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

fn expand_key(key: &[u8]) -> [u8; EXPAND_BYTES] {
    let mut buf = [0u8; EXPAND_BYTES];
    let mut h = Md5::new();
    buf[..key.len()].copy_from_slice(key);
    let mut filled = key.len();
    let mut hashed = key.len();
    while filled < EXPAND_BYTES {
        h.update(&buf[..hashed]);
        let digest = h.finalize();
        let n = (EXPAND_BYTES - filled).min(digest.len());
        buf[filled..filled + n].copy_from_slice(&digest[..n]);
        filled += n;
        hashed += n;
    }
    buf
}

/// polcore `0x10063df0`: a wrapping 32-bit sum of little-endian dwords plus a
/// right-shifted assembly of the unaligned tail.
pub fn checksum(data: &[u8], init: u32) -> u32 {
    let mut total = init;
    let whole = (data.len() / 4) * 4;
    for chunk in data[..whole].chunks_exact(4) {
        total = total.wrapping_add(u32::from_le_bytes(chunk.try_into().unwrap()));
    }
    let mut tail = 0u32;
    for &b in &data[whole..] {
        tail = (tail >> 8) | (u32::from(b) << 24);
    }
    total.wrapping_add(tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];

    // Vectors self-derived from the static reading of polcore.dll 73b1864b,
    // not captured from any live server. They pin this port against that
    // reading; they do not confirm the reading interoperates.
    const P_PINNED: [u32; P_WORDS] = [
        0x217c_ce55,
        0x0554_332e,
        0x52c0_fd18,
        0xf8a3_5c48,
        0x26cb_2d1f,
        0x4346_1008,
        0x9348_15b2,
        0x770e_a8f1,
        0xb468_2d1d,
        0xd385_42b3,
        0x649a_8d12,
        0xdacb_ebab,
        0xf744_99ca,
        0xc883_c832,
        0x181c_c2e2,
        0x6a86_70e5,
        0x2d94_c6fe,
        0xeace_de68,
    ];

    #[test]
    fn md5_is_stock_rfc1321() {
        assert_eq!(hex::encode(md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex::encode(md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hex::encode(md5(&[b'a'; 1_000_000])),
            "7707d6ae4e027c70eea2a935c2296f21"
        );
    }

    #[test]
    fn the_key_schedule_matches_the_static_reading() {
        let bf = PolBlowfish::new(&KEY, (0, 0));
        assert_eq!(bf.p, P_PINNED);
        // The self-encryption pass leaves the S-boxes with repeated word pairs,
        // an artefact of encrypting the constant zero block; the last pair is
        // the encryption of (0, 0).
        assert_eq!(
            (bf.s[S_WORDS - 2], bf.s[S_WORDS - 1]),
            (0x82ca_c3ea, 0x338c_f935)
        );
    }

    #[test]
    fn a_block_encrypts_to_the_pinned_vector() {
        let bf = PolBlowfish::new(&KEY, (0, 0));
        assert_eq!(bf.encrypt_block(0, 0), (0x82ca_c3ea, 0x338c_f935));
        assert_eq!(
            bf.encrypt_block(0x0123_4567, 0x89ab_cdef),
            (0xe7aa_29d5, 0x3800_a019)
        );
    }

    #[test]
    fn the_stream_layer_is_an_involution_and_passes_line_breaks() {
        let iv = (0x0badc0de_u32, 0xfeedface_u32);
        let plain = b"NICK Testchar:0123456789abcdef\r\n";
        let mut enc = PolBlowfish::new(&KEY, iv);
        let cipher = enc.stream(plain, StreamReset::Boundary);
        assert_eq!(
            hex::encode(&cipher),
            "376b71111913c09a0e12fb61c95d1f298170406b57f36d1b48a1fffbf8fe0d0a"
        );
        // The trailing CR LF survive as plaintext.
        assert_eq!(&cipher[cipher.len() - 2..], b"\r\n");
        let mut dec = PolBlowfish::new(&KEY, iv);
        assert_eq!(dec.stream(&cipher, StreamReset::Boundary), plain);
    }

    #[test]
    fn the_keystream_continues_across_a_message_body() {
        let iv = (0x0badc0de_u32, 0xfeedface_u32);
        let mut bf = PolBlowfish::new(&KEY, iv);
        let _ = bf.stream(b"NICK Testchar:0123456789abcdef\r\n", StreamReset::Boundary);
        let cont = bf.stream(b"USER a b c :d\r\n", StreamReset::Continue);
        assert_eq!(hex::encode(cont), "c99c93f92ee4375afa51cd844b0d0a");
    }

    #[test]
    fn the_checksum_sums_dwords_and_folds_the_tail() {
        assert_eq!(checksum(b"", 0), 0);
        assert_eq!(checksum(&[0, 1, 2, 3, 4, 5, 6, 7], 0), 0x0a08_0604);
        assert_eq!(checksum(b"PlayOnline", 0), 0x483b_da9f);
        assert_eq!(checksum(b"PlayOnline", 1), 0x483b_daa0);
    }
}
