//! `DatRoot` loads VTABLE/FTABLE pairs for every ROM directory present
//! in an FFXI install and resolves `file_id` to a typed `DatLocation`
//! by probing each APPID in turn.
//!
//! Install layout (HorizonXI confirmed; vanilla SE-FFXI identical):
//!   {install}/VTABLE.DAT       <- APPID 1 ("ROM")
//!   {install}/FTABLE.DAT
//!   {install}/ROM/{dir}/{file}.DAT
//!
//!   {install}/ROM2/VTABLE2.DAT <- APPID 2 ("ROM2")
//!   {install}/ROM2/FTABLE2.DAT
//!   {install}/ROM2/{dir}/{file}.DAT
//!   ... and similarly for ROM3..ROM10.
//!
//! Per-APPID encoding (POLUtils `FFXI.cs`):
//!   VTABLEN.DAT[file_id] == N → file lives in this ROM
//!   VTABLEN.DAT[file_id] == 0 → file not in this ROM (try next APPID)
//!   FTABLEN.DAT[2*file_id]    → u16 LE, encodes dir/file within the ROM
//!
//! Env var `FFXI_DAT_PATH` should point at the install root (the
//! "FINAL FANTASY XI" folder).

use std::env;
use std::path::{Path, PathBuf};

use crate::ftable::{FTable, SubPath};
use crate::vtable::VTable;
use crate::{DatError, Result};

/// Maximum ROM index POLUtils probes for. Retail tops out at 10
/// today; the slack covers a hypothetical future expansion.
const MAX_ROM_INDEX: u8 = 19;

/// Decoded location of a DAT within the install. Preserves the structured
/// rom/dir/file triple so callers can log, dedupe, or compose paths
/// without re-running the table lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatLocation {
    /// "ROM", "ROM2", ..., "ROM10".
    pub rom_dir: String,
    pub sub_path: SubPath,
}

impl DatLocation {
    /// Build an absolute path under the install root.
    pub fn path_under(&self, root: &Path) -> PathBuf {
        root.join(&self.rom_dir)
            .join(self.sub_path.dir.to_string())
            .join(format!("{}.DAT", self.sub_path.file))
    }
}

/// One ROM's worth of tables. `rom_index` is also the marker byte expected
/// in `vtable` to indicate "file is in my ROM".
#[derive(Debug)]
struct AppTables {
    rom_index: u8,
    rom_dir: String,
    vtable: VTable,
    ftable: FTable,
}

#[derive(Debug)]
pub struct DatRoot {
    root: PathBuf,
    apps: Vec<AppTables>,
}

impl DatRoot {
    /// Load all VTABLE/FTABLE pairs present under `root`. APPIDs with
    /// no VTABLE on disk are silently skipped (gaps are fine — e.g. an
    /// install without expansion N just lacks `ROMN/VTABLEN.DAT`).
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let mut apps = Vec::new();

        for i in 1..=MAX_ROM_INDEX {
            let (rom_dir, vt_path, ft_path) = appid_paths(&root, i);
            if !vt_path.exists() {
                continue;
            }
            let vtable = VTable::load(&vt_path)?;
            let ftable = FTable::load(&ft_path)?;
            apps.push(AppTables {
                rom_index: i,
                rom_dir,
                vtable,
                ftable,
            });
        }

        if apps.is_empty() {
            return Err(DatError::Io {
                path: root.join("VTABLE.DAT"),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no VTABLE.DAT or VTABLEN.DAT found under root",
                ),
            });
        }

        Ok(Self { root, apps })
    }

    /// Reads `FFXI_DAT_PATH` from env. Should point at the
    /// "FINAL FANTASY XI" directory inside the install.
    pub fn from_env() -> Result<Self> {
        let root = env::var_os("FFXI_DAT_PATH").ok_or(DatError::EnvMissing)?;
        Self::open(PathBuf::from(root))
    }

    /// Tries `FFXI_DAT_PATH` first; if unset, falls back to the
    /// workspace-relative vendor path that the dev environment uses
    /// (`./vendor/Game/SquareEnix/FINAL FANTASY XI`). The fallback
    /// resolves relative to the process CWD, so it works for
    /// `cargo run` from the workspace root but not for installed
    /// binaries run from elsewhere — those must set the env var.
    ///
    /// Returns `DatError::EnvMissing` only when both attempts fail
    /// (env unset *and* fallback path doesn't exist), giving callers
    /// a single signal to decide between soft-degrade and hard-fail.
    pub fn from_env_or_default() -> Result<Self> {
        if let Some(root) = env::var_os("FFXI_DAT_PATH") {
            return Self::open(PathBuf::from(root));
        }
        let fallback = PathBuf::from("vendor/Game/SquareEnix/FINAL FANTASY XI");
        if !fallback.join("VTABLE.DAT").exists() {
            return Err(DatError::EnvMissing);
        }
        Self::open(fallback)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Per-APPID summary, useful for the example binary to print after
    /// loading. `(rom_dir, vtable_len, ftable_len)`.
    pub fn app_summary(&self) -> Vec<(String, u32, u32)> {
        self.apps
            .iter()
            .map(|a| (a.rom_dir.clone(), a.vtable.len(), a.ftable.len()))
            .collect()
    }

    /// Resolve `file_id` by probing every loaded APPID in order. First
    /// hit (`VTABLEN[file_id] == N`) wins — matches POLUtils' loop.
    pub fn resolve(&self, file_id: u32) -> Result<DatLocation> {
        for app in &self.apps {
            if app.vtable.contains(file_id, app.rom_index) {
                let sub_path = app.ftable.sub_path(file_id)?;
                return Ok(DatLocation {
                    rom_dir: app.rom_dir.clone(),
                    sub_path,
                });
            }
        }
        Err(DatError::FileNotPresent { file_id })
    }
}

/// `(rom_dir, vtable_path, ftable_path)` for APPID `i`.
fn appid_paths(root: &Path, i: u8) -> (String, PathBuf, PathBuf) {
    if i == 1 {
        (
            "ROM".to_string(),
            root.join("VTABLE.DAT"),
            root.join("FTABLE.DAT"),
        )
    } else {
        let rd = format!("ROM{}", i);
        (
            rd.clone(),
            root.join(&rd).join(format!("VTABLE{}.DAT", i)),
            root.join(&rd).join(format!("FTABLE{}.DAT", i)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct SynthApp {
        rom_index: u8,
        vtable: Vec<u8>,
        ftable_words: Vec<u16>,
    }

    /// Build a multi-APPID DatRoot in a tempdir. For each app, writes
    /// VTABLE.DAT/FTABLE.DAT (rom_index 1) or ROM{N}/VTABLE{N}.DAT etc.
    fn synth_root(apps: &[SynthApp]) -> (tempfile::TempDir, DatRoot) {
        let dir = tempfile::tempdir().unwrap();
        for app in apps {
            let (_rom_dir, vt_path, ft_path) = appid_paths(dir.path(), app.rom_index);
            if let Some(parent) = vt_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&vt_path, &app.vtable).unwrap();
            let mut ft_bytes = Vec::with_capacity(app.ftable_words.len() * 2);
            for w in &app.ftable_words {
                ft_bytes.extend_from_slice(&w.to_le_bytes());
            }
            fs::write(&ft_path, ft_bytes).unwrap();
        }
        let root = DatRoot::open(dir.path()).unwrap();
        (dir, root)
    }

    #[test]
    fn resolve_picks_first_appid_that_claims_file_id() {
        // ROM (marker=1) claims file_ids 1 and 2.
        // ROM2 (marker=2) claims file_id 3.
        // ROM3 (marker=3) claims file_id 4.
        let (_tmp, root) = synth_root(&[
            SynthApp {
                rom_index: 1,
                vtable: vec![0, 1, 1, 0, 0],
                ftable_words: vec![0x0000, 0x0080, 0x00FF, 0x0000, 0x0000],
            },
            SynthApp {
                rom_index: 2,
                vtable: vec![0, 0, 0, 2, 0],
                ftable_words: vec![0x0000, 0x0000, 0x0000, 0x0001, 0x0000],
            },
            SynthApp {
                rom_index: 3,
                vtable: vec![0, 0, 0, 0, 3],
                ftable_words: vec![0x0000, 0x0000, 0x0000, 0x0000, 0xFFFF],
            },
        ]);

        let loc1 = root.resolve(1).unwrap();
        assert_eq!(loc1.rom_dir, "ROM");
        assert_eq!(loc1.sub_path, SubPath { dir: 1, file: 0 });

        let loc3 = root.resolve(3).unwrap();
        assert_eq!(loc3.rom_dir, "ROM2");
        assert_eq!(loc3.sub_path, SubPath { dir: 0, file: 1 });

        let loc4 = root.resolve(4).unwrap();
        assert_eq!(loc4.rom_dir, "ROM3");
        assert_eq!(
            loc4.sub_path,
            SubPath {
                dir: 511,
                file: 127
            }
        );
    }

    #[test]
    fn resolve_returns_missing_when_no_app_claims_it() {
        let (_tmp, root) = synth_root(&[SynthApp {
            rom_index: 1,
            vtable: vec![1, 1],
            ftable_words: vec![0x0000, 0x0080],
        }]);
        assert!(matches!(
            root.resolve(5),
            Err(DatError::FileNotPresent { file_id: 5 })
        ));
    }

    #[test]
    fn path_under_assembles_correct_layout() {
        let (tmp, root) = synth_root(&[SynthApp {
            rom_index: 2,
            vtable: vec![0, 0, 2],
            ftable_words: vec![0x0000, 0x0000, 0xFFFF],
        }]);
        let loc = root.resolve(2).unwrap();
        let p = loc.path_under(root.root());
        assert_eq!(p, tmp.path().join("ROM2").join("511").join("127.DAT"));
    }

    #[test]
    fn empty_install_errors() {
        let dir = tempfile::tempdir().unwrap();
        let err = DatRoot::open(dir.path()).unwrap_err();
        assert!(matches!(err, DatError::Io { .. }));
    }
}
