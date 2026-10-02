//! The chat service's key agreement: an ephemeral RSA keypair, polcore's
//! custom base64, and the numeric-300 decode that recovers the Blowfish key.
//!
//! Read from polcore.dll build 73b1864b. The client draws a 256-bit modulus,
//! sends it (little-endian, base64) as the IRC USER realname, and the server
//! replies with numeric 300 carrying the session's Blowfish key encrypted to
//! that modulus. The public exponent is a fixed constant both ends know and is
//! never transmitted.
//!
//! Two byte orders meet here and disagree, which is the easiest thing to get
//! wrong: the modulus goes out least-significant-byte-first, while the numeric
//! 300 ciphertext arrives most-significant-byte-first.

use crate::rng::RandomBytes;
use num_bigint::{BigInt, BigUint, Sign};
use num_integer::Integer;

use crate::error::{Error, Result};

/// polcore `0x10065de0`; byte-identical in build f5af5837.
const B64_ALPHABET: &[u8; 64] = b"TSG8IncW3HFKokOg79qzeCmZs2yBYEQVAUxR5rbwi4P@jMDLtpvad0f_J1hlN6uX";

/// polcore's decoder fills a short final quartet with the alphabet's zero
/// character before decoding.
const B64_FILL: u8 = B64_ALPHABET[0];

/// polcore `0x1002bc57`: the fixed public exponent, never sent on the wire.
pub const PUBLIC_EXPONENT: u32 = 0xFFFF;

/// The modulus is 8 little-endian 32-bit limbs.
pub const MODULUS_BYTES: usize = 32;

/// The session Blowfish key recovered from numeric 300.
pub const SESSION_KEY_BYTES: usize = 8;

/// polcore `0x10016182`: the numeric-300 handler decodes a fixed count of
/// characters but only feeds the first `CIPHERTEXT_BYTES` to the private op.
const N300_DECODE_CHARS: usize = 0x2E;
const CIPHERTEXT_BYTES: usize = 0x20;

const PRIME_BYTES: usize = 16;

/// Encode bytes as polcore's MSB-first base64 with no padding: an n-byte input
/// becomes `ceil(n*8/6)` characters, and a partial final group emits only the
/// characters that carry input bits.
pub fn b64_encode(data: &[u8]) -> String {
    let mut out = Vec::with_capacity(data.len().div_ceil(3) * 4);
    let whole = (data.len() / 3) * 3;
    for chunk in data[..whole].chunks_exact(3) {
        let v = (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
        for shift in [18, 12, 6, 0] {
            out.push(B64_ALPHABET[((v >> shift) & 0x3F) as usize]);
        }
    }
    let rem = data.len() - whole;
    if rem > 0 {
        let mut group = [0u8; 3];
        group[..rem].copy_from_slice(&data[whole..]);
        let v = (u32::from(group[0]) << 16) | (u32::from(group[1]) << 8) | u32::from(group[2]);
        let nchars = if rem == 1 { 2 } else { 3 };
        for shift in [18, 12, 6, 0].into_iter().take(nchars) {
            out.push(B64_ALPHABET[((v >> shift) & 0x3F) as usize]);
        }
    }
    String::from_utf8(out).expect("the alphabet is ASCII")
}

fn b64_reverse(c: u8) -> u8 {
    B64_ALPHABET
        .iter()
        .position(|&a| a == c)
        .map_or(0, |i| i as u8)
}

/// Decode the first `nchars` characters of `text`, three bytes per quartet, a
/// short final quartet filled with the zero character. polcore validates
/// nothing here; an out-of-alphabet byte decodes as zero.
pub fn b64_decode(text: &[u8], nchars: usize) -> Vec<u8> {
    let take = nchars.min(text.len());
    let mut chunk: Vec<u8> = text[..take].to_vec();
    while !chunk.len().is_multiple_of(4) {
        chunk.push(B64_FILL);
    }
    let mut out = Vec::with_capacity(chunk.len() / 4 * 3);
    for quartet in chunk.chunks_exact(4) {
        let mut v = 0u32;
        for &c in quartet {
            v = (v << 6) | u32::from(b64_reverse(c));
        }
        out.push((v >> 16) as u8);
        out.push((v >> 8) as u8);
        out.push(v as u8);
    }
    out
}

/// An ephemeral keypair for one chat connection. Kept private to the process;
/// only the modulus leaves, and nothing on the wire commits to how it was
/// drawn, so a reimplementation uses a real CSPRNG rather than polcore's
/// GetTickCount-seeded generator.
#[derive(Clone)]
pub struct KeyPair {
    n: BigUint,
    d: BigUint,
}

impl KeyPair {
    /// polcore `0x1002bb40`: two distinct 128-bit primes, `n = p*q`,
    /// `d = e^-1 mod (p-1)(q-1)`.
    pub fn generate(rng: &mut dyn RandomBytes) -> Self {
        let p = next_prime(draw_prime_candidate(rng), rng);
        let q = loop {
            let cand = next_prime(draw_prime_candidate(rng), rng);
            if cand != p {
                break cand;
            }
        };
        let n = &p * &q;
        let phi = (&p - 1u32) * (&q - 1u32);
        let d = modinv(&BigUint::from(PUBLIC_EXPONENT), &phi)
            .expect("gcd(e, phi) == 1 is enforced by next_prime");
        Self { n, d }
    }

    /// The 32 little-endian modulus bytes the client base64-encodes into the
    /// IRC USER realname.
    pub fn modulus_wire_bytes(&self) -> [u8; MODULUS_BYTES] {
        le_fixed(&self.n)
    }

    /// The base64 of the modulus wire bytes: the realname field itself.
    pub fn realname(&self) -> String {
        b64_encode(&self.modulus_wire_bytes())
    }

    /// polcore `0x10063eb0` seeds the stream cipher from the low two limbs of
    /// the client's own modulus, so the IV is client-chosen per session.
    pub fn stream_iv(&self) -> (u32, u32) {
        let b = self.modulus_wire_bytes();
        (
            u32::from_le_bytes(b[0..4].try_into().unwrap()),
            u32::from_le_bytes(b[4..8].try_into().unwrap()),
        )
    }

    /// polcore `0x10016182`: decode numeric 300's payload token, RSA-decrypt
    /// the first 32 bytes with the private key, and return the 8-byte session
    /// Blowfish key (the first eight message bytes, reversed).
    pub fn recover_session_key(&self, payload: &[u8]) -> Result<[u8; SESSION_KEY_BYTES]> {
        let block = b64_decode(payload, N300_DECODE_CHARS);
        if block.len() < CIPHERTEXT_BYTES {
            return Err(Error::protocol("numeric 300 payload too short"));
        }
        let msg = self.decrypt_block(&block[..CIPHERTEXT_BYTES])?;
        if msg.len() < SESSION_KEY_BYTES {
            return Err(Error::protocol(
                "numeric 300 plaintext shorter than the key",
            ));
        }
        let mut key = [0u8; SESSION_KEY_BYTES];
        for (i, out) in key.iter_mut().enumerate() {
            *out = msg[SESSION_KEY_BYTES - 1 - i];
        }
        Ok(key)
    }

    /// polcore `0x1002bde0`: byte-reverse the big-endian ciphertext into the
    /// bignum, modexp with `d`, strip PKCS#1 v1.5 type-2 padding, and return
    /// the message in natural big-endian order.
    fn decrypt_block(&self, ciphertext_be: &[u8]) -> Result<Vec<u8>> {
        let c = BigUint::from_bytes_be(&ciphertext_be[..CIPHERTEXT_BYTES]);
        let m = c.modpow(&self.d, &self.n);
        let mut le = le_fixed(&m).to_vec();
        let msg_len = strip_pad_le(&mut le)
            .ok_or_else(|| Error::protocol("no PKCS#1 separator in numeric 300 plaintext"))?;
        le.truncate(msg_len);
        le.reverse();
        Ok(le)
    }
}

fn le_fixed(v: &BigUint) -> [u8; MODULUS_BYTES] {
    let mut out = [0u8; MODULUS_BYTES];
    let le = v.to_bytes_le();
    let n = le.len().min(MODULUS_BYTES);
    out[..n].copy_from_slice(&le[..n]);
    out
}

/// polcore `0x1002bf80`: PKCS#1 v1.5 type-2 unpad on the little-endian
/// plaintext. It zeroes the type byte without checking it and walks down until
/// the separator; the returned length excludes the `00 02` prefix and the
/// `00` separator. `None` when no separator is found in the block.
fn strip_pad_le(buf: &mut [u8]) -> Option<usize> {
    let mut consumed = 3usize;
    buf[MODULUS_BYTES - 2] = 0;
    let mut i = MODULUS_BYTES as isize - 3;
    while i >= 0 && buf[i as usize] != 0 {
        consumed += 1;
        buf[i as usize] = 0;
        i -= 1;
    }
    if i < 0 {
        return None;
    }
    Some(MODULUS_BYTES - consumed)
}

/// polcore `0x1002d550` tail with nwords 4: sixteen random bytes, bit 0 forced
/// on, top nibble forced to `1000`, so the candidate is exactly 128 bits.
fn draw_prime_candidate(rng: &mut dyn RandomBytes) -> BigUint {
    let mut b = [0u8; PRIME_BYTES];
    rng.fill(&mut b);
    b[0] |= 1;
    b[PRIME_BYTES - 1] = (b[PRIME_BYTES - 1] & 0x0F) | 0x80;
    BigUint::from_bytes_le(&b)
}

/// polcore `0x1002c530`: step up by two until `gcd(c-1, 0xFFFF) == 1` and `c`
/// is prime. polcore runs one Miller-Rabin round; we run several, which only
/// rejects composites polcore would have accepted, never a prime it would
/// have taken.
fn next_prime(mut c: BigUint, rng: &mut dyn RandomBytes) -> BigUint {
    let e = BigUint::from(PUBLIC_EXPONENT);
    loop {
        let g = (&c - 1u32).gcd(&e);
        if g == BigUint::from(1u32) && is_probable_prime(&c, rng) {
            return c;
        }
        c += 2u32;
    }
}

const MILLER_RABIN_ROUNDS: usize = 24;

fn is_probable_prime(n: &BigUint, rng: &mut dyn RandomBytes) -> bool {
    let one = BigUint::from(1u32);
    let two = BigUint::from(2u32);
    if *n < two {
        return false;
    }
    if n.is_even() {
        return *n == two;
    }
    let n_minus_1 = n - &one;
    let mut d = n_minus_1.clone();
    let mut r = 0u32;
    while d.is_even() {
        d >>= 1;
        r += 1;
    }
    'witness: for _ in 0..MILLER_RABIN_ROUNDS {
        let a = random_below(&n_minus_1, &two, rng);
        let mut x = a.modpow(&d, n);
        if x == one || x == n_minus_1 {
            continue;
        }
        for _ in 0..r.saturating_sub(1) {
            x = x.modpow(&two, n);
            if x == n_minus_1 {
                continue 'witness;
            }
        }
        return false;
    }
    true
}

fn random_below(exclusive_max: &BigUint, min: &BigUint, rng: &mut dyn RandomBytes) -> BigUint {
    let bytes = exclusive_max.to_bytes_le().len().max(1);
    loop {
        let mut buf = vec![0u8; bytes];
        rng.fill(&mut buf);
        let candidate = BigUint::from_bytes_le(&buf) % exclusive_max;
        if candidate >= *min {
            return candidate;
        }
    }
}

fn modinv(a: &BigUint, m: &BigUint) -> Option<BigUint> {
    let a = BigInt::from_biguint(Sign::Plus, a.clone());
    let m = BigInt::from_biguint(Sign::Plus, m.clone());
    let egcd = a.extended_gcd(&m);
    if egcd.gcd != BigInt::from(1) {
        return None;
    }
    let x = ((egcd.x % &m) + &m) % &m;
    x.to_biguint()
}

/// The stream IV a modulus (as little-endian wire bytes) seeds: its low two
/// limbs, matching `KeyPair::stream_iv` for the peer that only holds `n`.
#[cfg(test)]
pub(crate) fn stream_iv_from_modulus_le(modulus_le: &[u8]) -> (u32, u32) {
    let mut b = [0u8; MODULUS_BYTES];
    let n = modulus_le.len().min(MODULUS_BYTES);
    b[..n].copy_from_slice(&modulus_le[..n]);
    (
        u32::from_le_bytes(b[0..4].try_into().unwrap()),
        u32::from_le_bytes(b[4..8].try_into().unwrap()),
    )
}

/// The numeric-300 payload a conforming server would send to deliver
/// `session_key` to the holder of the modulus `modulus_le` (little-endian wire
/// bytes). Reconstructed from what the client's decryptor accepts (any PKCS#1
/// v1.5 type-2 block); it exists to close the handshake round trip in tests,
/// and is not the retail server.
#[cfg(test)]
pub(crate) fn encrypt_session_key(
    session_key: &[u8; SESSION_KEY_BYTES],
    modulus_le: &[u8],
    rng: &mut dyn RandomBytes,
) -> String {
    let n = BigUint::from_bytes_le(&modulus_le[..MODULUS_BYTES.min(modulus_le.len())]);
    let mut msg: Vec<u8> = session_key.iter().rev().copied().collect();
    let ps_len = MODULUS_BYTES - 3 - msg.len();
    let mut ps = Vec::with_capacity(ps_len);
    while ps.len() < ps_len {
        let mut b = [0u8; 1];
        rng.fill(&mut b);
        if b[0] != 0 {
            ps.push(b[0]);
        }
    }
    let mut em = vec![0x00, 0x02];
    em.extend_from_slice(&ps);
    em.push(0x00);
    em.append(&mut msg);
    let c = BigUint::from_bytes_be(&em).modpow(&BigUint::from(PUBLIC_EXPONENT), &n);
    let mut be = c.to_bytes_be();
    while be.len() < MODULUS_BYTES {
        be.insert(0, 0);
    }
    b64_encode(&be)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    #[test]
    fn the_alphabet_is_a_bijection() {
        let mut seen = [false; 256];
        for &c in B64_ALPHABET {
            assert!(!seen[c as usize], "duplicate {c}");
            seen[c as usize] = true;
        }
    }

    #[test]
    fn base64_round_trips_and_sizes_the_output() {
        for nb in 1usize..40 {
            let data: Vec<u8> = (0..nb).map(|i| ((i * 37 + nb) & 0xFF) as u8).collect();
            let enc = b64_encode(&data);
            assert_eq!(enc.len(), (nb * 8).div_ceil(6), "len for {nb}");
            let dec = b64_decode(enc.as_bytes(), enc.len());
            assert_eq!(&dec[..nb], &data[..], "round trip {nb}");
        }
        assert_eq!(b64_encode(&[0u8; MODULUS_BYTES]).len(), 43);
    }

    #[test]
    fn a_generated_key_has_the_shape_polcore_draws() {
        let mut rng = StdRng::seed_from_u64(0xC0FFEE);
        let key = KeyPair::generate(&mut rng);
        assert_eq!(key.n.bits(), 255);
        let realname = key.realname();
        assert_eq!(realname.len(), 43);
        let back = BigUint::from_bytes_le(&b64_decode(realname.as_bytes(), realname.len()));
        assert_eq!(back, key.n);
    }

    #[test]
    fn numeric_300_round_trips_a_session_key() {
        let mut rng = StdRng::seed_from_u64(1);
        let key = KeyPair::generate(&mut rng);
        let want = [0x01u8, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
        let payload = server_numeric_300(&want, &key.n, b"XIZZY!", &mut rng);
        let got = key.recover_session_key(payload.as_bytes()).unwrap();
        assert_eq!(got, want);
        // The over-long decode window polcore reads is harmless.
        let mut longer = payload.clone();
        longer.push_str("TTT");
        assert_eq!(key.recover_session_key(longer.as_bytes()).unwrap(), want);
    }

    // Reconstructed server side: polcore has no public-key op, so this models
    // what its decryptor accepts (any PKCS#1 v1.5 type-2 block, message at
    // least 8 bytes). It exists only to close the round trip in tests.
    fn server_numeric_300(
        session_key: &[u8; SESSION_KEY_BYTES],
        n: &BigUint,
        tail: &[u8],
        rng: &mut dyn RandomBytes,
    ) -> String {
        let mut msg: Vec<u8> = session_key.iter().rev().copied().collect();
        msg.extend_from_slice(tail);
        let ps_len = MODULUS_BYTES - 3 - msg.len();
        let mut ps = Vec::with_capacity(ps_len);
        while ps.len() < ps_len {
            let mut b = [0u8; 1];
            rng.fill(&mut b);
            if b[0] != 0 {
                ps.push(b[0]);
            }
        }
        let mut em = vec![0x00, 0x02];
        em.extend_from_slice(&ps);
        em.push(0x00);
        em.extend_from_slice(&msg);
        let c = BigUint::from_bytes_be(&em).modpow(&BigUint::from(PUBLIC_EXPONENT), n);
        let mut be = c.to_bytes_be();
        while be.len() < MODULUS_BYTES {
            be.insert(0, 0);
        }
        b64_encode(&be)
    }
}
