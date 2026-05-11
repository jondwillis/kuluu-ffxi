//! VTABLEN.DAT: one `u8` per file_id, marking whether the file lives in this APPID's ROM.
//!
//! Value semantics (from POLUtils — the install ships one VTABLEN.DAT *per*
//! ROM directory, and the marker value in each file equals N):
//!   0  → file is not in this APPID's ROM
//!   N  → file is in this APPID's ROM (N is the ROM index; e.g. VTABLE2.DAT has 0s and 2s)
//!   anything else → data corruption; POLUtils treats it as "not present" and we mirror that.
//!
//! The `DatRoot` in `archive.rs` iterates all VTABLEs and returns the first
//! hit — that's where the per-ROM dispatch lives.

use std::fs;
use std::path::{Path, PathBuf};

use crate::{DatError, Result};

#[derive(Debug, Clone)]
pub struct VTable {
    /// One byte per file_id. `bytes[file_id]` is the app_byte (0 = missing).
    bytes: Box<[u8]>,
    /// Path the table was loaded from (for error messages).
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

    /// Number of file_ids covered by this VTABLE.
    pub fn len(&self) -> u32 {
        self.bytes.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    /// Returns the raw byte at `file_id`, or `None` if file_id is out
    /// of range for this table (which means "file not in this APPID",
    /// not an error — different APPIDs may cover different id ranges).
    pub fn marker_at(&self, file_id: u32) -> Option<u8> {
        self.bytes.get(file_id as usize).copied()
    }

    /// True iff this VTABLE marks `file_id` as present with the expected
    /// marker value (i.e. equal to this APPID's ROM index).
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
        // Tracking the API change: VTable no longer claims ownership of rom_dir
        // computation (that's owned by DatRoot, which knows which VTABLE this is
        // and what marker to expect). The invariant being pinned: byte 0 means
        // "not in this APPID's ROM" — same semantic as before, narrower API.
        // For a hypothetical VTABLE2.DAT (marker=2), byte 0 → not present.
        let v = synth_vtable(&[0, 2, 2, 0]);
        assert!(!v.contains(0, 2));
        assert!(v.contains(1, 2));
        assert!(v.contains(2, 2));
        assert!(!v.contains(3, 2));
    }

    #[test]
    fn marker_mismatch_means_not_in_this_app() {
        // Previously this test encoded a WRONG mental model: that one VTABLE's
        // bytes 1,2,3,10 each mapped to a different ROM directory inside the same
        // file. The real POLUtils algorithm: each ROM has its own VTABLEN.DAT, and
        // each only contains 0s and Ns. If a VTABLE.DAT (marker=1) ever contains
        // some other nonzero byte, POLUtils silently treats it as "not in this
        // APPID" — the next APPID's VTABLE may legitimately claim it.
        let v = synth_vtable(&[0, 1, 1, 5]);
        assert!(v.contains(1, 1));
        assert!(!v.contains(3, 1));
        assert_eq!(v.marker_at(3), Some(5));
    }

    #[test]
    fn out_of_range_is_not_in_app() {
        // Behavior CHANGE driven by API correction: different APPIDs cover
        // different file_id spaces, so out-of-range must soft-fail to "not in
        // this APPID" rather than hard-error — otherwise the resolver would
        // bail when probing one APPID and never check the next one.
        let v = synth_vtable(&[1, 1]);
        assert!(!v.contains(5, 1));
        assert_eq!(v.marker_at(5), None);
    }
}
