//! FTABLE.DAT: one `u16 LE` per file_id, encoding the sub-path
//! (`{dir}/{file}.DAT`) within a ROM directory.
//!
//! Encoding (POLUtils `FFXI.cs`):
//!   file_dir = u16_le_at(2 * file_id)
//!   dir  = file_dir >> 7    (upper 9 bits; range 0..=511)
//!   file = file_dir & 0x7F  (lower 7 bits; range 0..=127)

use std::fs;
use std::path::{Path, PathBuf};

use crate::{DatError, Result};

#[derive(Debug, Clone)]
pub struct FTable {
    /// One u16 LE per file_id, stored as the raw byte sequence.
    bytes: Box<[u8]>,
    source: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubPath {
    /// Directory index within the ROM (0..=511).
    pub dir: u16,
    /// File index within the directory (0..=127).
    pub file: u8,
}

impl FTable {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).map_err(|source| DatError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if bytes.len() % 2 != 0 {
            return Err(DatError::InvalidTableSize {
                path: path.to_path_buf(),
                len: bytes.len() as u64,
                stride: 2,
            });
        }
        Ok(Self {
            bytes: bytes.into_boxed_slice(),
            source: path.to_path_buf(),
        })
    }

    pub fn len(&self) -> u32 {
        (self.bytes.len() / 2) as u32
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    pub fn sub_path(&self, file_id: u32) -> Result<SubPath> {
        let table_len = self.len();
        let off = (file_id as usize) * 2;
        let raw = self
            .bytes
            .get(off..off + 2)
            .ok_or(DatError::FileIdOutOfRange { file_id, table_len })?;
        let file_dir = u16::from_le_bytes([raw[0], raw[1]]);
        Ok(SubPath {
            dir: file_dir >> 7,
            file: (file_dir & 0x7F) as u8,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_ftable(words: &[u16]) -> FTable {
        let mut bytes = Vec::with_capacity(words.len() * 2);
        for w in words {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        FTable {
            bytes: bytes.into_boxed_slice(),
            source: PathBuf::from("<synthetic>"),
        }
    }

    #[test]
    fn bit_split_known_values() {
        // file_dir = 0x0000 → dir 0, file 0
        // file_dir = 0x007F → dir 0, file 127
        // file_dir = 0x0080 → dir 1, file 0    (boundary)
        // file_dir = 0x00FF → dir 1, file 127
        // file_dir = 0x0100 → dir 2, file 0
        let f = synth_ftable(&[0x0000, 0x007F, 0x0080, 0x00FF, 0x0100]);
        assert_eq!(f.sub_path(0).unwrap(), SubPath { dir: 0, file: 0 });
        assert_eq!(f.sub_path(1).unwrap(), SubPath { dir: 0, file: 127 });
        assert_eq!(f.sub_path(2).unwrap(), SubPath { dir: 1, file: 0 });
        assert_eq!(f.sub_path(3).unwrap(), SubPath { dir: 1, file: 127 });
        assert_eq!(f.sub_path(4).unwrap(), SubPath { dir: 2, file: 0 });
    }

    #[test]
    fn full_9_bit_dir_range() {
        // Max dir = 511 (9 bits). file_dir = (511 << 7) | 0x7F = 0xFFFF.
        let f = synth_ftable(&[0xFFFF]);
        assert_eq!(
            f.sub_path(0).unwrap(),
            SubPath {
                dir: 511,
                file: 127
            }
        );
    }

    #[test]
    fn odd_length_rejected() {
        let bad = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(bad.path(), [0x01, 0x02, 0x03]).unwrap();
        let err = FTable::load(bad.path()).unwrap_err();
        assert!(matches!(err, DatError::InvalidTableSize { stride: 2, .. }));
    }
}
