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

pub struct DecompressTable {
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

        let base = raw[0].wrapping_sub(4);
        for v in raw.iter_mut() {
            if *v > 0xFF {
                *v = (v.wrapping_sub(base)) / 4;
            }
        }

        let root_idx = raw[0] as usize;
        if root_idx + 4 > raw.len() {
            return Err(ZlibError::MalformedTable);
        }
        Ok(Self {
            nodes: raw,
            root_idx,
        })
    }

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

            node = self.nodes[node + bit] as usize;
            if node + 4 > self.nodes.len() {
                return Err(ZlibError::MalformedTable);
            }

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

pub struct CompressTable {
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

    pub fn compress(&self, input: &[u8]) -> Result<(usize, Vec<u8>), ZlibError> {
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
                let bit = (v_bytes[j / 8] >> (j & 7)) & 1;
                let dst = bit_pos + j;
                let dst_byte = 1 + dst / 8;
                let dst_shift = dst & 7;
                out[dst_byte] = (out[dst_byte] & !(1 << dst_shift)) | (bit << dst_shift);
            }
            bit_pos += bit_count;
        }

        out[0] = 1;

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

            let payload_bits = bits - 8;
            let out = dec.decompress(&encoded, payload_bits).unwrap();
            assert_eq!(&out, input, "roundtrip failed for {input:?}");
        }
    }
}
