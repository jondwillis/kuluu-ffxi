use std::env;
use std::path::{Path, PathBuf};

use crate::ftable::{FTable, SubPath};
use crate::vtable::VTable;
use crate::{DatError, Result};

const MAX_ROM_INDEX: u8 = 19;

pub const DEFAULT_INSTALL_DIR: &str = "vendor/game-files/SquareEnix/FINAL FANTASY XI";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatLocation {
    pub rom_dir: String,
    pub sub_path: SubPath,
}

impl DatLocation {
    pub fn path_under(&self, root: &Path) -> PathBuf {
        root.join(&self.rom_dir)
            .join(self.sub_path.dir.to_string())
            .join(format!("{}.DAT", self.sub_path.file))
    }
}

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

    pub fn from_env() -> Result<Self> {
        let root = env::var_os("FFXI_DAT_PATH").ok_or(DatError::EnvMissing)?;
        Self::open(PathBuf::from(root))
    }

    pub fn from_env_or_default() -> Result<Self> {
        if let Some(root) = env::var_os("FFXI_DAT_PATH") {
            return Self::open(PathBuf::from(root));
        }
        let fallback = PathBuf::from(DEFAULT_INSTALL_DIR);
        if !fallback.join("VTABLE.DAT").exists() {
            return Err(DatError::EnvMissing);
        }
        Self::open(fallback)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn app_summary(&self) -> Vec<(String, u32, u32)> {
        self.apps
            .iter()
            .map(|a| (a.rom_dir.clone(), a.vtable.len(), a.ftable.len()))
            .collect()
    }

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

/// Open the retail install for gated tests: `FFXI_DAT_PATH` if set, else the
/// default install resolved relative to the crate (works regardless of the test
/// CWD, unlike [`DatRoot::from_env_or_default`]'s relative path). `None` when no
/// install is present so tests self-skip.
#[cfg(test)]
pub(crate) fn open_test_install() -> Option<DatRoot> {
    if let Ok(root) = DatRoot::from_env() {
        return Some(root);
    }
    let default = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(DEFAULT_INSTALL_DIR);
    default
        .join("VTABLE.DAT")
        .exists()
        .then(|| DatRoot::open(default).ok())
        .flatten()
}

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
