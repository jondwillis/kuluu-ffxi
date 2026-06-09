//! FFXI's bespoke "zlib" — actually a static-Huffman-like bit codec backed by
//! lookup tables (`compress.dat` / `decompress.dat`). Has nothing to do with
//! real zlib. Port of `server/src/common/zlib.cpp`.
//!
//! Wire format coming back from the map server:
//!
//! ```text
//!   in[0]      = 0x01  (magic — anything else is an invalid frame)
//!   in[1..]    = bit-packed encoded bytes, LSB-first
//!   the u32 stored at `buff[len-20]` is the *bit count* of the encoded data
//!     (NOT the byte count) — `bytes = (bits + 7) / 8`.
//! ```
//!
//! Decompress: walk a binary trie. Each node has 4 entries (16 bytes), where
//! `nodes[i+0]` and `nodes[i+1]` are pointers to children for input bits 0/1
//! respectively. A leaf has both children null and stores the output byte at
//! `nodes[i+3]`. From the root, consume one input bit per branch hop; on a
//! leaf, emit the byte and reset to the root.
//!
//! Compress: for each input byte, look up `bits` (encoding length) and `value`
//! (the bit string) in the encoding table, then bit-pack into the output.

const COMPRESS_RAW: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/compress.dat"));
const DECOMPRESS_RAW: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/decompress.dat"));

#[derive(Debug, thiserror::Error)]
pub enum ZlibError {
    #[error("invalid magic byte: expected 0x01, got 0x{0:02x}")]
    BadMagic(u8),
    #[error("output buffer overflow ({0} bytes produced, max {1})")]
    OutputOverflow(usize, usize),
    #[error("input ran out before tree walk completed")]
    Truncated,
    #[error("malformed lookup table")]
    MalformedTable,
}

/// Lazy-initialized decompression jump table. Byte-for-byte equivalent to
/// what `populate_jump_table` builds at runtime in the C++.
pub struct DecompressTable {
    /// Raw u32 values from `decompress.dat`. Pointers > 0xFF have been
    /// translated to 0-based indices into this same vector; data bytes
    /// (≤ 0xFF) are stored verbatim.
    nodes: Vec<u32>,
    root_idx: usize,
}

impl DecompressTable {
    pub fn new() -> Result<Self, ZlibError> {
        if !DECOMPRESS_RAW.len().is_multiple_of(4) || DECOMPRESS_RAW.is_empty() {
            return Err(ZlibError::MalformedTable);
        }
        let mut raw: Vec<u32> = DECOMPRESS_RAW
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        // Pointer base — same rule as C++ `populate_jump_table`:
        //   base = dec[0] - sizeof(uint32);
        //   ptr_index = (dec[i] - base) / sizeof(uint32);
        let base = raw[0].wrapping_sub(4);
        for v in raw.iter_mut() {
            if *v > 0xFF {
                *v = (v.wrapping_sub(base)) / 4;
            }
            // Else: it's already a data byte, leave as-is.
        }
        // root is the node referenced by raw[0].
        let root_idx = raw[0] as usize;
        if root_idx + 4 > raw.len() {
            return Err(ZlibError::MalformedTable);
        }
        Ok(Self {
            nodes: raw,
            root_idx,
        })
    }

    /// Decompress `input` (whose first byte must be 0x01, followed by
    /// `bit_count` payload bits, LSB-first). Stops at `bit_count` or when
    /// input is exhausted.
    pub fn decompress(&self, input: &[u8], bit_count: usize) -> Result<Vec<u8>, ZlibError> {
        if input.is_empty() {
            return Err(ZlibError::Truncated);
        }
        if input[0] != 1 {
            return Err(ZlibError::BadMagic(input[0]));
        }
        let data = &input[1..];
        let mut out: Vec<u8> = Vec::with_capacity(bit_count);
        let mut node = self.root_idx;
        for i in 0..bit_count {
            let byte_off = i / 8;
            if byte_off >= data.len() {
                break;
            }
            let bit = ((data[byte_off] >> (i & 7)) & 1) as usize;
            // Move to child for this bit.
            node = self.nodes[node + bit] as usize;
            if node + 4 > self.nodes.len() {
                return Err(ZlibError::MalformedTable);
            }
            // Leaf check: children null means we reached a data node.
            let left = self.nodes[node];
            let right = self.nodes[node + 1];
            if left == 0 && right == 0 {
                let data_byte = self.nodes[node + 3] as u8;
                out.push(data_byte);
                node = self.root_idx;
            }
        }
        Ok(out)
    }
}

/// Compression encoding table — for each input byte (signed, range
/// `-128..=127`), maps to (bit_count, bit_pattern_u32).
pub struct CompressTable {
    /// Indexed by `(byte as i8) + 0x80` for the *value*, and
    /// `(byte as i8) + 0x180` for the *bit count*.
    raw: Vec<u32>,
}

impl CompressTable {
    pub fn new() -> Result<Self, ZlibError> {
        if !COMPRESS_RAW.len().is_multiple_of(4) || COMPRESS_RAW.is_empty() {
            return Err(ZlibError::MalformedTable);
        }
        let raw: Vec<u32> = COMPRESS_RAW
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        if raw.len() < 0x200 {
            return Err(ZlibError::MalformedTable);
        }
        Ok(Self { raw })
    }

    /// Encode `input`, returning (bit_count, packed_bytes). The first byte of
    /// the packed_bytes is always 0x01 (magic). Mirrors
    /// `server/src/common/zlib.cpp::zlib_compress`.
    pub fn compress(&self, input: &[u8]) -> Result<(usize, Vec<u8>), ZlibError> {
        // Each input byte produces up to 32 bits. Worst case: 8x expansion
        // plus the magic byte.
        let max_bits = input.len() * 32 + 8;
        let max_bytes = max_bits.div_ceil(8) + 1;
        let mut out = vec![0u8; max_bytes];
        let mut bit_pos: usize = 0;

        for &b in input {
            let signed = b as i8 as i32;
            let bit_count = self.raw[(signed + 0x180) as usize] as usize;
            let value = self.raw[(signed + 0x80) as usize];
            let v_bytes = value.to_le_bytes();
            for j in 0..bit_count {
                // Bit `j` of `v_bytes`, LSB-first across the four bytes.
                let bit = (v_bytes[j / 8] >> (j & 7)) & 1;
                let dst = bit_pos + j;
                let dst_byte = 1 + dst / 8; // +1 for the magic byte at out[0]
                let dst_shift = dst & 7;
                out[dst_byte] = (out[dst_byte] & !(1 << dst_shift)) | (bit << dst_shift);
            }
            bit_pos += bit_count;
        }

        out[0] = 1;
        // C++ returns `read + 8` bits, accounting for the magic-byte prefix.
        let total_bits = bit_pos + 8;
        let total_bytes = total_bits.div_ceil(8);
        out.truncate(total_bytes);
        Ok((total_bits, out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_compress_decompress() {
        let comp = CompressTable::new().unwrap();
        let dec = DecompressTable::new().unwrap();
        let inputs: &[&[u8]] = &[
            &[],
            &[0x00],
            &[0x01, 0x02, 0x03],
            b"hello world",
            &(0u8..=255).collect::<Vec<_>>(),
        ];
        for input in inputs {
            let (bits, encoded) = comp.compress(input).unwrap();
            // Decompress reads the first byte (magic) and `bits - 8` payload bits.
            let payload_bits = bits - 8;
            let out = dec.decompress(&encoded, payload_bits).unwrap();
            assert_eq!(&out, input, "roundtrip failed for {input:?}");
        }
    }
}
