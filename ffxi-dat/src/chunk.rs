use crate::{DatError, Result};

#[derive(Debug, Clone, Copy)]
pub struct Chunk<'a> {
    pub name: [u8; 4],

    pub kind: u8,

    pub data: &'a [u8],

    pub offset: usize,
}

impl<'a> Chunk<'a> {
    pub fn name_str(&self) -> String {
        self.name
            .iter()
            .map(|&b| if b == 0 { '.' } else { b as char })
            .collect()
    }
}

#[derive(Debug)]
pub struct ChunkWalker<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> ChunkWalker<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

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

pub fn walk(bytes: &[u8]) -> ChunkWalker<'_> {
    ChunkWalker::new(bytes)
}

#[derive(Debug, Clone)]
pub struct ChunkNode<'a> {
    pub chunk: Chunk<'a>,
    pub children: Vec<ChunkNode<'a>>,
}

pub fn walk_tree<'a>(bytes: &'a [u8]) -> ChunkNode<'a> {
    let synthetic_root = Chunk {
        name: [0; 4],
        kind: 0xFF,
        data: &[],
        offset: 0,
    };
    let mut root = ChunkNode {
        chunk: synthetic_root,
        children: Vec::new(),
    };

    let mut path: Vec<usize> = Vec::new();

    fn at_path<'r, 'a>(root: &'r mut ChunkNode<'a>, path: &[usize]) -> &'r mut ChunkNode<'a> {
        let mut cur = root;
        for &i in path {
            cur = &mut cur.children[i];
        }
        cur
    }

    for result in walk(bytes) {
        let Ok(chunk) = result else { continue };
        match chunk.kind {
            0x00 => {
                path.pop();
            }
            0x01 => {
                let cur = at_path(&mut root, &path);
                let idx = cur.children.len();
                cur.children.push(ChunkNode {
                    chunk,
                    children: Vec::new(),
                });
                path.push(idx);
            }
            _ => {
                let cur = at_path(&mut root, &path);
                cur.children.push(ChunkNode {
                    chunk,
                    children: Vec::new(),
                });
            }
        }
    }

    root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_chunk(name: &[u8; 4], kind: u8, body: &[u8]) -> Vec<u8> {
        let total = 16 + body.len();
        let padded_total = total.div_ceil(16) * 16;
        let pad = padded_total - total;
        let size_units = (padded_total / 16) as u32;
        let value = (size_units << 7) | (kind as u32 & 0x7F);

        let mut out = Vec::with_capacity(padded_total);
        out.extend_from_slice(name);
        out.extend_from_slice(&value.to_le_bytes());
        out.extend(std::iter::repeat_n(0u8, 8));
        out.extend_from_slice(body);
        out.extend(std::iter::repeat_n(0u8, pad));
        out
    }

    #[test]
    fn walks_two_chunks() {
        let mut buf = synth_chunk(b"selp", 1, &[0u8; 16]);
        buf.extend(synth_chunk(b"wat1", 5, &[0xAB; 8]));

        let chunks: Vec<Chunk> = walk(&buf).map(|r| r.unwrap()).collect();
        assert_eq!(chunks.len(), 2);
        assert_eq!(&chunks[0].name, b"selp");
        assert_eq!(chunks[0].kind, 1);
        assert_eq!(chunks[0].offset, 0);
        assert_eq!(chunks[0].data.len(), 16);

        assert_eq!(&chunks[1].name, b"wat1");
        assert_eq!(chunks[1].kind, 5);
        assert_eq!(chunks[1].offset, 32);
        assert_eq!(chunks[1].data.len(), 16);
        assert_eq!(&chunks[1].data[..8], &[0xAB; 8]);
        assert_eq!(&chunks[1].data[8..16], &[0u8; 8]);
    }

    #[test]
    fn lax_unknown_fourcc_just_advances() {
        let buf = synth_chunk(b"ZZZZ", 42, &[0u8; 32]);
        let chunks: Vec<Chunk> = walk(&buf).map(|r| r.unwrap()).collect();
        assert_eq!(chunks.len(), 1);
        assert_eq!(&chunks[0].name, b"ZZZZ");
        assert_eq!(chunks[0].kind, 42);
    }

    #[test]
    fn truncated_body_errors() {
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
        let mut buf = synth_chunk(b"selp", 1, &[0u8; 16]);

        buf.extend_from_slice(b"end\0\0\0\0\0\0\0\0\0\0\0\0\0");

        let results: Vec<Result<Chunk>> = walk(&buf).collect();
        assert_eq!(results.len(), 1, "should stop at the size_units=0 sentinel");
        assert_eq!(&results[0].as_ref().unwrap().name, b"selp");
    }
}
