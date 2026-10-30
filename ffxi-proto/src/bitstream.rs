//! Bit-field codecs shared by every wire-format reader/writer in this crate.
//!
//! Faithful ports of `packBitsBE`/`unpackBitsBE`/`packBitsLE`/`unpackBitsLE` from
//! vendor/server/src/common/utils.cpp — the pair is self-consistent at any bit offset,
//! including the byte-reversed-window quirk the LE pair has when a field crosses a byte:
//! it copies the touched bytes into a `bytesNeeded` scratch reversed, writes through the BE
//! codec there, and un-reverses. Reading them as plain little-endian gives different bytes,
//! so anything that must interoperate with the server (linkshell names, the search TLV blob)
//! goes through these two functions and never a hand-rolled bit reader.
//!
//! Extracted verbatim from ffxi-proto/src/search.rs so the linkshell-name codec can share it.

// Faithful ports of packBitsBE/unpackBitsBE/packBitsLE/unpackBitsLE from
// vendor/server/src/common/utils.cpp — the pair is self-consistent at any bit
// offset, including the byte-reversed-window quirk when a field crosses a byte,
// so requests we build are exactly what _HandleSearchRequest will consume and
// responses AddPlayer packs come back through the mirror. Widths stay within
// the u64 domain LSB implements.
pub fn memcpy_le(target: &[u8], off: usize, n: usize) -> u64 {
    target
        .get(off..)
        .map(|s| {
            let mut acc = 0u64;
            for (i, b) in s.iter().take(n).enumerate() {
                acc |= (*b as u64) << (8 * i);
            }
            acc
        })
        .unwrap_or(0)
}

pub fn memcpy_le_write(target: &mut [u8], off: usize, n: usize, val: u64) {
    for i in 0..n {
        if let Some(slot) = target.get_mut(off + i) {
            *slot = (val >> (8 * i)) as u8;
        }
    }
}

pub fn pack_bits_be(target: &mut [u8], value: u64, byte_offset: usize, bit_offset: u32, len: u32) {
    let byte_offset = byte_offset + (bit_offset / 8) as usize;
    let bo = bit_offset % 8;
    let mut bitmask = if len >= 64 {
        !0u64
    } else {
        (!0u64 >> (64 - len)) << bo
    };
    let value = (value << bo) & bitmask;
    bitmask ^= !0u64;
    let actual = (bo + len).div_ceil(8) as usize;
    let mut data = memcpy_le(target, byte_offset, actual);
    data &= bitmask;
    data |= value;
    memcpy_le_write(target, byte_offset, actual, data);
}

pub fn unpack_bits_be(target: &[u8], byte_offset: usize, bit_offset: u32, len: u32) -> u64 {
    let byte_offset = byte_offset + (bit_offset / 8) as usize;
    let bo = bit_offset % 8;
    let bitmask = if len >= 64 {
        !0u64
    } else {
        (!0u64 >> (64 - len)) << bo
    };
    let actual = (bo + len).div_ceil(8) as usize;
    let data = memcpy_le(target, byte_offset, actual);
    (data & bitmask) >> bo
}

pub fn bytes_needed(bo: u32, len: u32) -> Option<usize> {
    match bo + len {
        0..=8 => Some(1),
        9..=16 => Some(2),
        17..=32 => Some(4),
        33..=64 => Some(8),
        _ => None,
    }
}

/// `packBitsLE(target, value, bitOffset, lengthInBit)` (utils.cpp): writes into the
/// byte-reversed `bytesNeeded` window and returns the next bit offset (`bitOffset + len`).
pub fn pack_bits_le(target: &mut [u8], value: u64, bit_offset: usize, len: u32) -> usize {
    let byte_offset = bit_offset / 8;
    let bo = (bit_offset % 8) as u32;
    let needed = bytes_needed(bo, len).expect("packBitsLE above 64 bits");
    let actual = (bo + len).div_ceil(8) as usize;
    let mut m = vec![0u8; needed];
    for c in 0..actual {
        m[needed - 1 - c] = target.get(byte_offset + c).copied().unwrap_or(0);
    }
    let new_bit_offset = (needed as u32 * 8) - (bo + len);
    pack_bits_be(&mut m, value, 0, new_bit_offset, len);
    for c in 0..actual {
        if let Some(slot) = target.get_mut(byte_offset + c) {
            *slot = m[needed - 1 - c];
        }
    }
    bit_offset + len as usize
}

/// `unpackBitsLE(target, bitOffset, lengthInBit)` (utils.cpp) — mirror of [`pack_bits_le`].
pub fn unpack_bits_le(target: &[u8], bit_offset: usize, len: u32) -> u64 {
    let byte_offset = bit_offset / 8;
    let bo = (bit_offset % 8) as u32;
    let needed = bytes_needed(bo, len).expect("unpackBitsLE above 64 bits");
    let actual = (bo + len).div_ceil(8) as usize;
    let m: Vec<u8> = (0..actual)
        .map(|c| {
            (
                needed - 1 - c,
                target.get(byte_offset + c).copied().unwrap_or(0),
            )
        })
        .fold(vec![0u8; needed], |mut acc, (slot, b)| {
            acc[slot] = b;
            acc
        });
    if needed == 1 {
        let bitmask = 0xFFu64 >> bo;
        return ((m[0] as u64 & bitmask) >> (8 - (len + bo))) as u64;
    }
    let new_bit_offset = (needed as u32 * 8) - (bo + len);
    unpack_bits_be(&m, 0, new_bit_offset, len)
}
