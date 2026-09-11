//! The two bit-packed LZ codings the patch server ships: `.slc` (a whole file)
//! and `.olc` (a delta against the previous version). Reverse-engineered from
//! PlayOnlineViewer/viewer/com/polcore.dll (viewer 1.18.15e, SHA-256
//! 73b1864b...): FUN_10042805 decodes `.slc`, FUN_10042d91 applies `.olc`,
//! FUN_10043e00 / FUN_10043f35 are the LSB-first bit reader.

const WINDOW_BITS: u32 = 16;
const LENGTH_BITS: u32 = 8;
const LITERAL_BITS: u32 = 8;

pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    total: usize,
}

impl<'a> BitReader<'a> {
    /// `coded` starts with a little-endian u32 bit count.
    pub fn new(coded: &'a [u8]) -> Result<Self, String> {
        let head: [u8; 4] = coded
            .get(..4)
            .and_then(|h| h.try_into().ok())
            .ok_or("coded stream shorter than its 4-byte header")?;
        let total = u32::from_le_bytes(head) as usize;
        let data = &coded[4..];
        if total > data.len() * 8 {
            return Err(format!(
                "coded stream declares {total} bits but holds {}",
                data.len() * 8
            ));
        }
        Ok(Self {
            data,
            pos: 0,
            total,
        })
    }

    pub fn remaining(&self) -> usize {
        self.total - self.pos
    }

    pub fn read(&mut self, bits: u32) -> Result<u32, String> {
        let bits = bits as usize;
        if self.pos + bits > self.total {
            return Err(format!(
                "coded stream truncated at bit {} (need {bits} of {})",
                self.pos, self.total
            ));
        }
        let mut v = 0u32;
        for i in 0..bits {
            let p = self.pos + i;
            v |= (((self.data[p >> 3] >> (p & 7)) & 1) as u32) << i;
        }
        self.pos += bits;
        Ok(v)
    }
}

fn copy_back(out: &mut Vec<u8>, distance: usize, len: usize) -> Result<(), String> {
    if distance == 0 || distance > out.len() {
        return Err(format!(
            "back-reference distance {distance} outside the {} bytes decoded",
            out.len()
        ));
    }
    let start = out.len() - distance;
    for i in 0..len {
        out.push(out[start + i]);
    }
    Ok(())
}

/// Decode a `.slc` stream: each item is a flag bit, then either a
/// `distance u16, length u8` copy from the output or a literal byte.
pub fn decode_direct(coded: &[u8]) -> Result<Vec<u8>, String> {
    let mut r = BitReader::new(coded)?;
    let mut out = Vec::with_capacity(coded.len() * 2);
    while r.remaining() > 0 {
        if r.read(1)? == 1 {
            let distance = r.read(WINDOW_BITS)? as usize;
            let len = r.read(LENGTH_BITS)? as usize;
            copy_back(&mut out, distance, len)?;
        } else {
            out.push(r.read(LITERAL_BITS)? as u8);
        }
    }
    Ok(out)
}

/// Apply an `.olc` delta to `base`: items are `0` literal byte, `10`
/// `distance u16, length u8` copy from the output, `11` `offset i16, length
/// u8` copy from `base` at the current output position plus `offset`.
pub fn apply_indirect(delta: &[u8], base: &[u8]) -> Result<Vec<u8>, String> {
    let mut r = BitReader::new(delta)?;
    let mut out = Vec::with_capacity(base.len());
    while r.remaining() > 0 {
        if r.read(1)? == 0 {
            out.push(r.read(LITERAL_BITS)? as u8);
        } else if r.read(1)? == 0 {
            let distance = r.read(WINDOW_BITS)? as usize;
            let len = r.read(LENGTH_BITS)? as usize;
            copy_back(&mut out, distance, len)?;
        } else {
            let offset = r.read(WINDOW_BITS)? as u16 as i16 as isize;
            let len = r.read(LENGTH_BITS)? as usize;
            let at = out.len() as isize + offset;
            let src = usize::try_from(at)
                .ok()
                .and_then(|at| base.get(at..at + len))
                .ok_or_else(|| {
                    format!(
                        "base reference {at}..{} outside the {}-byte base",
                        at + len as isize,
                        base.len()
                    )
                })?;
            out.extend_from_slice(src);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BitWriter {
        bits: Vec<bool>,
    }

    impl BitWriter {
        fn new() -> Self {
            Self { bits: Vec::new() }
        }
        fn put(&mut self, v: u32, n: u32) {
            for i in 0..n {
                self.bits.push((v >> i) & 1 == 1);
            }
        }
        fn finish(&self) -> Vec<u8> {
            let mut out = (self.bits.len() as u32).to_le_bytes().to_vec();
            let mut byte = 0u8;
            for (i, &b) in self.bits.iter().enumerate() {
                byte |= (b as u8) << (i & 7);
                if i & 7 == 7 {
                    out.push(byte);
                    byte = 0;
                }
            }
            if !self.bits.len().is_multiple_of(8) {
                out.push(byte);
            }
            out
        }
    }

    #[test]
    fn direct_literals_and_overlapping_copy() {
        let mut w = BitWriter::new();
        for &c in b"abc" {
            w.put(0, 1);
            w.put(c as u32, 8);
        }
        w.put(1, 1);
        w.put(3, 16);
        w.put(9, 8);
        assert_eq!(decode_direct(&w.finish()).unwrap(), b"abcabcabcabc");
    }

    #[test]
    fn direct_rejects_bad_distance_and_truncation() {
        let mut w = BitWriter::new();
        w.put(1, 1);
        w.put(1, 16);
        w.put(1, 8);
        assert!(decode_direct(&w.finish()).unwrap_err().contains("distance"));
        let mut coded = w.finish();
        coded[0] = 40;
        assert!(decode_direct(&coded).unwrap_err().contains("declares"));
        assert!(decode_direct(&[1, 2]).is_err());
    }

    #[test]
    fn indirect_mixes_base_copies_output_copies_and_literals() {
        let base = b"0123456789ABCDEF".to_vec();
        let mut w = BitWriter::new();
        w.put(0b11, 2);
        w.put(4, 16);
        w.put(3, 8);
        w.put(0, 1);
        w.put(b'-' as u32, 8);
        w.put(0b11, 2);
        w.put((-2i16) as u16 as u32, 16);
        w.put(4, 8);
        w.put(0b01, 2);
        w.put(4, 16);
        w.put(4, 8);
        assert_eq!(apply_indirect(&w.finish(), &base).unwrap(), b"456-23452345");
    }

    #[test]
    fn indirect_rejects_base_reference_outside_the_base() {
        let mut w = BitWriter::new();
        w.put(0b11, 2);
        w.put((-1i16) as u16 as u32, 16);
        w.put(1, 8);
        assert!(apply_indirect(&w.finish(), b"xyz")
            .unwrap_err()
            .contains("base"));
    }
}
