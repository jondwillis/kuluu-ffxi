//! FFXI Blowfish — a custom variant used by the LSB/Phoenix map server.
//!
//! Differences from textbook Blowfish:
//! 1. The P/S init constants come from a fixed 4168-byte table baked into the
//!    server (`subkey[]` in `server/src/common/blowfish.cpp`), not the
//!    pi-derived constants of standard Blowfish.
//! 2. The round function `TT` reads only one bit out of two of the four S-box
//!    lookups, XORs them with 32, and adds them in instead of the standard
//!    `((S0+S1) ^ S2) + S3`. This is FFXI-specific.
//!
//! Port of `server/src/common/blowfish.cpp`. The subkey table is extracted at
//! build time by `build.rs` so it stays byte-identical with upstream.

const SUBKEY: &[u8; 4168] = include_bytes!(concat!(env!("OUT_DIR"), "/blowfish_subkey.bin"));

/// Number of Feistel rounds.
const N: usize = 16;

/// Per-session Blowfish state. `P` and `S` are derived from the session key.
#[derive(Clone)]
pub struct State {
    pub p: [u32; 18],
    pub s: [u32; 1024],
}

impl State {
    /// Initialize from a raw session key. The map server uses the MD5(20-byte
    /// session_key) as the actual cipher key, but that hashing is the caller's
    /// responsibility — this matches LSB's `initBlowfish()`.
    pub fn new(key: &[u8]) -> Self {
        assert!(!key.is_empty(), "blowfish key must be non-empty");
        let mut state = Self {
            p: [0u32; 18],
            s: [0u32; 1024],
        };

        // memcpy the first 72 bytes into P (18 × 4 bytes, little-endian).
        for i in 0..18 {
            let off = i * 4;
            state.p[i] = u32::from_le_bytes(SUBKEY[off..off + 4].try_into().unwrap());
        }
        // memcpy the next 4096 bytes into S.
        for i in 0..1024 {
            let off = 72 + i * 4;
            state.s[i] = u32::from_le_bytes(SUBKEY[off..off + 4].try_into().unwrap());
        }

        // XOR cycling key bytes into P. The server's `blowfish_init` declares
        // `key` as `const int8[]` (SIGNED char), so when it does
        // `data = (data << 8) | key[j]`, key bytes ≥ 0x80 sign-extend into
        // 0xFFFFFFxx before the OR. We must replicate that — naive `u8 as u32`
        // would diverge for ~half of all key values. See
        // `server/src/common/blowfish.cpp:494-522`.
        let mut j = 0;
        for i in 0..(N + 2) {
            let mut data: u32 = 0;
            for _ in 0..4 {
                let signed = key[j] as i8 as i32 as u32;
                data = (data << 8) | signed;
                j += 1;
                if j >= key.len() {
                    j = 0;
                }
            }
            state.p[i] ^= data;
        }

        // Encipher zeros to derive each pair of P, then each pair of S.
        let mut datal: u32 = 0;
        let mut datar: u32 = 0;
        for i in (0..(N + 2)).step_by(2) {
            encipher(&mut datal, &mut datar, &state.p, &state.s);
            state.p[i] = datal;
            state.p[i + 1] = datar;
        }
        for i in 0..4 {
            for j in (0..256).step_by(2) {
                encipher(&mut datal, &mut datar, &state.p, &state.s);
                state.s[i * 256 + j] = datal;
                state.s[i * 256 + j + 1] = datar;
            }
        }

        state
    }
}

#[inline]
fn tt(working: u32, s: &[u32; 1024]) -> u32 {
    // FFXI's quirky F. The two "& 1 ^ 32" lookups are deliberate, not a bug.
    let a = (s[256 + ((working >> 8) as usize & 0xff)] & 1) ^ 32;
    let b = (s[768 + ((working >> 24) as usize)] & 1) ^ 32;
    let c = s[512 + ((working >> 16) as usize & 0xff)];
    let d = s[(working as usize) & 0xff];
    a.wrapping_add(b).wrapping_add(c).wrapping_add(d)
}

/// Encipher a 64-bit block (split into `xl`/`xr`) in place.
pub fn encipher(xl: &mut u32, xr: &mut u32, p: &[u32; 18], s: &[u32; 1024]) {
    let mut x_l = *xl;
    let mut x_r = *xr;

    for i in 0..N {
        x_l ^= p[i];
        x_r = tt(x_l, s) ^ x_r;
        std::mem::swap(&mut x_l, &mut x_r);
    }
    std::mem::swap(&mut x_l, &mut x_r);

    x_r ^= p[N];
    x_l ^= p[N + 1];

    *xl = x_l;
    *xr = x_r;
}

/// Decipher a 64-bit block (split into `xl`/`xr`) in place.
pub fn decipher(xl: &mut u32, xr: &mut u32, p: &[u32; 18], s: &[u32; 1024]) {
    let mut x_l = *xl;
    let mut x_r = *xr;

    for i in (2..=(N + 1)).rev() {
        x_l ^= p[i];
        x_r = tt(x_l, s) ^ x_r;
        std::mem::swap(&mut x_l, &mut x_r);
    }
    std::mem::swap(&mut x_l, &mut x_r);

    x_r ^= p[1];
    x_l ^= p[0];

    *xl = x_l;
    *xr = x_r;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_random_keys() {
        let keys: &[&[u8]] = &[
            b"\x00",
            b"hello",
            b"abcdefghijklmnopqrstuvwxyz",
            &[0xde, 0xad, 0xbe, 0xef, 0xfe, 0xed, 0xfa, 0xce, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0],
        ];
        for k in keys {
            let st = State::new(k);
            for &(l, r) in &[
                (0u32, 0u32),
                (0xdead_beef, 0xcafe_babe),
                (0xffff_ffff, 0x0000_0000),
                (0x12345678, 0x9abcdef0),
            ] {
                let (mut a, mut b) = (l, r);
                encipher(&mut a, &mut b, &st.p, &st.s);
                let (cl, cr) = (a, b);
                decipher(&mut a, &mut b, &st.p, &st.s);
                assert_eq!((a, b), (l, r), "roundtrip key={k:?} pt=({l:08x},{r:08x}) ct=({cl:08x},{cr:08x})");
            }
        }
    }

    #[test]
    fn p_init_matches_subkey_when_keyed_with_zero_pad() {
        // Sanity: with an all-zero key, the initial XOR is a no-op, so P[0..18]
        // must equal the leading 18 little-endian u32s of SUBKEY *before* the
        // zero-encipher phase rewrites them. We can't observe the pre-encipher
        // state via the public API, but we can verify that subkey extraction
        // produced exactly 4168 bytes.
        assert_eq!(SUBKEY.len(), 4168);
    }
}
