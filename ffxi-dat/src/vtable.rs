use std::fs;
use std::path::{Path, PathBuf};

use crate::{DatError, Result};

#[derive(Debug, Clone)]
pub struct VTable {
    bytes: Box<[u8]>,

    source: PathBuf,
}

impl VTable {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).map_err(|source| DatError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self {
            bytes: bytes.into_boxed_slice(),
            source: path.to_path_buf(),
        })
    }

    pub fn len(&self) -> u32 {
        self.bytes.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    pub fn marker_at(&self, file_id: u32) -> Option<u8> {
        self.bytes.get(file_id as usize).copied()
    }

    pub fn contains(&self, file_id: u32, expected_marker: u8) -> bool {
        self.marker_at(file_id) == Some(expected_marker)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_vtable(bytes: &[u8]) -> VTable {
        VTable {
            bytes: bytes.to_vec().into_boxed_slice(),
            source: PathBuf::from("<synthetic>"),
        }
    }

    #[test]
    fn zero_marker_means_not_in_this_app() {
        let v = synth_vtable(&[0, 2, 2, 0]);
        assert!(!v.contains(0, 2));
        assert!(v.contains(1, 2));
        assert!(v.contains(2, 2));
        assert!(!v.contains(3, 2));
    }

    #[test]
    fn marker_mismatch_means_not_in_this_app() {
        let v = synth_vtable(&[0, 1, 1, 5]);
        assert!(v.contains(1, 1));
        assert!(!v.contains(3, 1));
        assert_eq!(v.marker_at(3), Some(5));
    }

    #[test]
    fn out_of_range_is_not_in_app() {
        let v = synth_vtable(&[1, 1]);
        assert!(!v.contains(5, 1));
        assert_eq!(v.marker_at(5), None);
    }
}
