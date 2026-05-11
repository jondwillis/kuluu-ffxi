//! Generic FFXI DAT chunk walker.
//!
//! FFXI DATs are sequences of self-describing records ("chunks").
//! Header layout (16 bytes total — the first 8 are meaningful, the rest is
//! reserved-zero padding so the whole chunk stays 16-byte aligned):
//!
//!   offset 0..4    FourCC name (ASCII, e.g. "selp", "mot_", "RID\0")
//!   offset 4..8    `value: u32 LE`, packed:
//!                    bits 0..7   `kind`       (7-bit discriminant)
//!                    bits 7..26  `size_units` (19-bit total size in 16-byte units, INCLUDING header)
//!                    bits 26..32 reserved / unknown
//!   offset 8..16   reserved (observed as zeros)
//!
//! Body size in bytes = `16 * size_units - 16` (i.e. total chunk = `16 * size_units`,
//! body starts at offset 16). The "16" subtrahend IS the header size — they happen
//! to be the same number, which makes this easy to misread.
//! Reference (Apache/GPL-compatible): LSB FFXI-NavMesh-Builder
//! `Common/dat/ParseZoneModelDat.cs` (GPL-3) — same decode formula.
//!
//! Lax policy: unknown FourCC names and unknown `kind` values are *not*
//! errors. The walker advances past them using the declared size. The
//! only failure mode is a chunk whose declared size walks off the end of
//! the buffer (real corruption or wrong file passed).

use crate::{DatError, Result};

/// One decoded chunk pointing into the source buffer.
#[derive(Debug, Clone, Copy)]
pub struct Chunk<'a> {
    /// 4-byte FourCC tag (raw ASCII bytes; some FFXI tags contain NUL).
    pub name: [u8; 4],
    /// 7-bit kind discriminant. Correlates with `name` for known
    /// record types; treat as opaque at this layer.
    pub kind: u8,
    /// Body bytes (excluding the 8-byte header).
    pub data: &'a [u8],
    /// Byte offset of this chunk's header in the source buffer.
    /// Useful for error messages and for cross-chunk references that
    /// some FFXI formats encode by absolute file offset.
    pub offset: usize,
}

impl<'a> Chunk<'a> {
    /// FourCC as a UTF-8 string with NULs replaced by `.` for printing.
    pub fn name_str(&self) -> String {
        self.name
            .iter()
            .map(|&b| if b == 0 { '.' } else { b as char })
            .collect()
    }
}

/// Iterator over a DAT's chunks. Borrows the source buffer.
#[derive(Debug)]
pub struct ChunkWalker<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> ChunkWalker<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    /// Current cursor position (next chunk's start, or buffer end).
    pub fn position(&self) -> usize {
        self.cursor
    }
}

impl<'a> Iterator for ChunkWalker<'a> {
    type Item = Result<Chunk<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.bytes.len() {
            return None;
        }

        let header_off = self.cursor;
        let Some(header) = self.bytes.get(header_off..header_off + 16) else {
            return Some(Err(DatError::TruncatedChunk {
                offset: header_off,
                needed: 16,
                available: self.bytes.len() - header_off,
            }));
        };

        let name = [header[0], header[1], header[2], header[3]];
        let value = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        let kind = (value & 0x7F) as u8;
        let size_units = (value >> 7) & 0x7FFFF;
        let total_bytes = (size_units as usize).saturating_mul(16);

        if total_bytes < 16 {
            // A size_units of 0 would mean a zero-sized chunk including
            // its own header — pathological. POLUtils-derived parsers
            // treat this as "end of stream" since FFXI never emits it.
            self.cursor = self.bytes.len();
            return None;
        }

        let body_bytes = total_bytes - 16;
        let body_start = header_off + 16;
        let body_end = body_start + body_bytes;

        if body_end > self.bytes.len() {
            self.cursor = self.bytes.len();
            return Some(Err(DatError::TruncatedChunk {
                offset: header_off,
                needed: total_bytes,
                available: self.bytes.len() - header_off,
            }));
        }

        let chunk = Chunk {
            name,
            kind,
            data: &self.bytes[body_start..body_end],
            offset: header_off,
        };
        self.cursor = header_off + total_bytes;
        Some(Ok(chunk))
    }
}

/// Convenience wrapper around the iterator.
pub fn walk(bytes: &[u8]) -> ChunkWalker<'_> {
    ChunkWalker::new(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic chunk: 16-byte header (4 name + 4 value + 8 reserved-zero),
    /// followed by `body`. Total length is padded to a 16-byte multiple, and the
    /// size_units in the header reflects that padded total.
    fn synth_chunk(name: &[u8; 4], kind: u8, body: &[u8]) -> Vec<u8> {
        let total = 16 + body.len();
        let padded_total = total.div_ceil(16) * 16;
        let pad = padded_total - total;
        let size_units = (padded_total / 16) as u32;
        let value = (size_units << 7) | (kind as u32 & 0x7F);

        let mut out = Vec::with_capacity(padded_total);
        out.extend_from_slice(name);
        out.extend_from_slice(&value.to_le_bytes());
        out.extend(std::iter::repeat_n(0u8, 8)); // reserved
        out.extend_from_slice(body);
        out.extend(std::iter::repeat_n(0u8, pad));
        out
    }

    #[test]
    fn walks_two_chunks() {
        // First chunk: 16-byte header + 16-byte body = 32 bytes, size_units = 2.
        // Second chunk: 16-byte header + 8-byte body padded to 16 = 32 bytes, size_units = 2.
        let mut buf = synth_chunk(b"selp", 1, &[0u8; 16]);
        buf.extend(synth_chunk(b"wat1", 5, &[0xAB; 8]));

        let chunks: Vec<Chunk> = walk(&buf).map(|r| r.unwrap()).collect();
        assert_eq!(chunks.len(), 2);
        assert_eq!(&chunks[0].name, b"selp");
        assert_eq!(chunks[0].kind, 1);
        assert_eq!(chunks[0].offset, 0);
        assert_eq!(chunks[0].data.len(), 16);

        // The walker returns the full on-disk body including any
        // padding bytes — it can't know the format's "real" payload
        // length without parsing the chunk-type-specific header inside.
        assert_eq!(&chunks[1].name, b"wat1");
        assert_eq!(chunks[1].kind, 5);
        assert_eq!(chunks[1].offset, 32);
        assert_eq!(chunks[1].data.len(), 16);
        assert_eq!(&chunks[1].data[..8], &[0xAB; 8]);
        assert_eq!(&chunks[1].data[8..16], &[0u8; 8]); // padding
    }

    #[test]
    fn lax_unknown_fourcc_just_advances() {
        // A wholly unknown FourCC with valid size: walker yields it
        // and moves on, no error.
        let buf = synth_chunk(b"ZZZZ", 42, &[0u8; 32]);
        let chunks: Vec<Chunk> = walk(&buf).map(|r| r.unwrap()).collect();
        assert_eq!(chunks.len(), 1);
        assert_eq!(&chunks[0].name, b"ZZZZ");
        assert_eq!(chunks[0].kind, 42);
    }

    #[test]
    fn truncated_body_errors() {
        // synth body=24 → padded total=48 → declares size_units=3.
        // Truncate to 20 bytes (just past the header) → body unreachable.
        let mut buf = synth_chunk(b"selp", 1, &[0u8; 24]);
        buf.truncate(20);
        let results: Vec<Result<Chunk>> = walk(&buf).collect();
        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0],
            Err(DatError::TruncatedChunk {
                offset: 0,
                needed: 48,
                available: 20,
            })
        ));
    }

    #[test]
    fn name_str_handles_nulls() {
        let buf = synth_chunk(b"RID\0", 3, &[]);
        let chunks: Vec<Chunk> = walk(&buf).map(|r| r.unwrap()).collect();
        assert_eq!(chunks[0].name_str(), "RID.");
    }

    #[test]
    fn zero_size_terminates_stream() {
        // FFXI uses size_units=0 as an implicit "end of records"
        // sentinel in some DATs. Treat it as graceful EOF, not an error.
        let mut buf = synth_chunk(b"selp", 1, &[0u8; 16]); // 32-byte chunk
        // Full 16-byte sentinel header: name="end.", value=0 (kind=0, size_units=0).
        buf.extend_from_slice(b"end\0\0\0\0\0\0\0\0\0\0\0\0\0");

        let results: Vec<Result<Chunk>> = walk(&buf).collect();
        assert_eq!(results.len(), 1, "should stop at the size_units=0 sentinel");
        assert_eq!(&results[0].as_ref().unwrap().name, b"selp");
    }
}
