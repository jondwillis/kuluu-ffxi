use std::env;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use ffxi_proto::login::expansion_display;

use crate::client_profile::ClientProfile;
use crate::ftable::{FTable, SubPath, FTABLE_BYTES_PER_FILE_ID};
use crate::vtable::VTable;
use crate::{DatError, Result};

/// research/XIClient/src/XIClient/include/Constants/Values.h INDEX_ROM_MAX:
/// the last ROM index LoadFileTables probes for, and so the last one with a
/// defined excode_client bit.
const RETAIL_INDEX_ROM_MAX: u8 = 13;

/// POLUtils DoFullFileScan (vendor/POLUtils/MassExtractor/Program.cs) probes
/// past [`RETAIL_INDEX_ROM_MAX`] to ROM19, and so does this crate, so a newer
/// install than that client still resolves.
const MAX_ROM_INDEX: u8 = 19;

/// The base install's own tables, the ones with no ROM index in their name.
pub const BASE_ROM_INDEX: u8 = 1;

pub const DAT_PATH_ENV: &str = "FFXI_DAT_PATH";

/// The DAT root's path inside an install directory.
pub const INSTALL_SUBDIR: &str = "SquareEnix/FINAL FANTASY XI";

/// Overlay roots searched before the base install, in order, separated by the
/// platform path separator. A startup override; see [`discover_overlays`] for
/// where the list otherwise comes from.
pub const OVERLAY_ENV: &str = "FFXI_DAT_OVERLAYS";

fn overlays_from_env() -> Option<Vec<PathBuf>> {
    let raw = env::var_os(OVERLAY_ENV)?;
    Some(
        env::split_paths(&raw)
            .filter(|p| !p.as_os_str().is_empty())
            .collect(),
    )
}

/// XI-Pivot's config, relative to the game directory that contains the install.
const PIVOT_INI: &str = "config/pivot/pivot.ini";
/// Where Pivot keeps the overlay directories, relative to the same place.
const PIVOT_DAT_DIR: &str = "polplugins/DATs";

/// The game directory holding Pivot's config and overlays, given a DAT root of
/// `<game>/SquareEnix/FINAL FANTASY XI`. A root that is itself a symlink (an
/// install registered by link) is followed first, since the config sits
/// beside the real tree, not the link.
fn game_dir(install_root: &Path) -> Option<PathBuf> {
    let real = match std::fs::read_link(install_root) {
        Ok(target) => install_root
            .parent()
            .map(|p| p.join(&target))
            .unwrap_or(target),
        Err(_) => install_root.to_path_buf(),
    };
    real.parent()?.parent().map(Path::to_path_buf)
}

/// Overlay directory names from a `pivot.ini`, ordered by their `[overlays]`
/// index.
///
/// Pivot indexes them `0=`, `1=`, … and we search in that order, first match
/// wins. That precedence is NOT confirmed against Pivot's source (none is
/// vendored) and the shipped `pivotSettingsHolder.ini` comment contradicts its
/// own entries; it is unobservable on the horizonxi-2023 install, where no two
/// overlays claim the same path.
fn parse_pivot_ini(ini: &str) -> (Option<PathBuf>, Vec<String>) {
    let mut root_path = None;
    let mut entries: Vec<(u32, String)> = Vec::new();
    let mut in_overlays = false;
    for line in ini.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_overlays = line.eq_ignore_ascii_case("[overlays]");
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if in_overlays {
            if let Ok(index) = key.parse::<u32>() {
                entries.push((index, value.to_string()));
            }
        } else if key.eq_ignore_ascii_case("root_path") {
            root_path = Some(PathBuf::from(value));
        }
    }
    entries.sort_by_key(|(index, _)| *index);
    (
        root_path,
        entries.into_iter().map(|(_, name)| name).collect(),
    )
}

/// Overlay roots for an install, honouring the config a private server already
/// ships: [`OVERLAY_ENV`] first, else XI-Pivot's own `pivot.ini` beside the
/// install. Empty when neither applies, which is the vanilla path.
///
/// `root_path` in a real `pivot.ini` is the Windows path Pivot was configured
/// with (measured: `C:\Program Files (x86)\...\polplugins\DATs`), so it is only
/// honoured when it resolves on this machine; otherwise the overlays are taken
/// from the install's own `polplugins/DATs`.
pub fn discover_overlays(install_root: &Path) -> Vec<PathBuf> {
    if let Some(from_env) = overlays_from_env() {
        return from_env;
    }
    let Some(game_dir) = game_dir(install_root) else {
        return Vec::new();
    };
    let Ok(ini) = std::fs::read_to_string(game_dir.join(PIVOT_INI)) else {
        return Vec::new();
    };
    let (root_path, names) = parse_pivot_ini(&ini);
    let dat_dir = root_path
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| game_dir.join(PIVOT_DAT_DIR));
    names
        .into_iter()
        .map(|name| dat_dir.join(name))
        .filter(|p| p.is_dir())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatLocation {
    pub rom_dir: String,
    pub sub_path: SubPath,
}

impl DatLocation {
    /// Where this file lives, honouring `root`'s overlay search path before the
    /// base install.
    pub fn path_under(&self, root: &DatRoot) -> PathBuf {
        root.path_of(self)
    }

    /// The raw `<root>/<ROMn>/<dir>/<file>.DAT` join, with no overlay search.
    pub fn join_under(&self, root: &Path) -> PathBuf {
        self.join_under_ext(root, "DAT")
    }

    fn join_under_ext(&self, root: &Path, ext: &str) -> PathBuf {
        root.join(&self.rom_dir)
            .join(self.sub_path.dir.to_string())
            .join(format!("{}.{ext}", self.sub_path.file))
    }

    /// First existing spelling of this file under `dir`. Retail runs on
    /// Windows, whose fopen is case-insensitive, so an install (or a
    /// hand-assembled overlay) can mix `.DAT` with `.dat` and the real client
    /// reads both the same; only a case-sensitive filesystem — a Linux user's
    /// wine/launcher-managed install — distinguishes the spellings.
    fn find_under(&self, dir: &Path) -> Option<PathBuf> {
        ["DAT", "dat"]
            .into_iter()
            .map(|ext| self.join_under_ext(dir, ext))
            .find(|p| p.is_file())
    }
}

#[derive(Debug)]
struct AppTables {
    rom_index: u8,
    rom_dir: String,
    vtable: VTable,
    ftable: FTable,
}

/// A ROM left out of the merge because a table is not the size the base
/// tables dictate. LoadFileTables
/// (research/XIClient/src/XIClient/source/System/FileIO/FileIOVirtualFileSystem.cpp)
/// aborts the whole load on this; a user-assembled install is better served by
/// the ROMs that do fit, with the rejected one reported through
/// [`DatRoot::skipped_tables`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSizeMismatch {
    pub rom_dir: String,
    pub path: PathBuf,
    pub len: u64,
    pub expected: u64,
}

impl fmt::Display for TableSizeMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} skipped: {} is {} bytes, the base tables dictate {}",
            self.rom_dir,
            self.path.display(),
            self.len,
            self.expected
        )
    }
}

fn check_table_sizes(
    rom_dir: &str,
    vtable: &VTable,
    ftable: &FTable,
    file_id_count: u32,
) -> std::result::Result<(), TableSizeMismatch> {
    let mismatch = |path: &Path, len: u64, expected: u64| TableSizeMismatch {
        rom_dir: rom_dir.to_string(),
        path: path.to_path_buf(),
        len,
        expected,
    };
    if vtable.len() != file_id_count {
        return Err(mismatch(
            vtable.source(),
            u64::from(vtable.len()),
            u64::from(file_id_count),
        ));
    }
    let ftable_bytes = u64::from(ftable.len()) * FTABLE_BYTES_PER_FILE_ID as u64;
    let expected_ftable_bytes = u64::from(vtable.len()) * FTABLE_BYTES_PER_FILE_ID as u64;
    if ftable_bytes != expected_ftable_bytes {
        return Err(mismatch(
            ftable.source(),
            ftable_bytes,
            expected_ftable_bytes,
        ));
    }
    Ok(())
}

#[derive(Debug)]
pub struct DatRoot {
    root: PathBuf,
    profile: ClientProfile,
    /// Ascending `rom_index`; [`DatRoot::resolve`] walks it backwards.
    apps: Vec<AppTables>,
    skipped: Vec<TableSizeMismatch>,
    /// Behind a lock because the renderer shares one `Arc<DatRoot>`: swapping
    /// overlays must be visible through that handle without rebuilding the root
    /// (which would re-read every VTABLE/FTABLE) or replacing the `Arc` at every
    /// holder. A read per DAT open is nothing against the file I/O that follows.
    overlays: RwLock<Vec<PathBuf>>,
    /// Held while this root is open so an updater refuses to rewrite it;
    /// `None` on a root that cannot take one (read-only media), which only
    /// loses the refusal.
    _lock: Option<crate::install::lock::SharedLock>,
}

impl DatRoot {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let lock = match crate::install::lock::shared(&root) {
            Ok(lock) => Some(lock),
            Err(e) if e.is_held() => {
                return Err(DatError::NoInstall {
                    reason: format!("{} is being updated: {e}", root.display()),
                })
            }
            Err(_) => None,
        };
        let mut apps: Vec<AppTables> = Vec::new();
        let mut skipped = Vec::new();

        for i in 1..=MAX_ROM_INDEX {
            let (rom_dir, vt_path, ft_path) = appid_paths(&root, i);
            if !vt_path.exists() {
                continue;
            }
            let vtable = VTable::load(&vt_path)?;
            let ftable = FTable::load(&ft_path)?;
            let file_id_count = apps.first().map_or(vtable.len(), |base| base.vtable.len());
            if let Err(mismatch) = check_table_sizes(&rom_dir, &vtable, &ftable, file_id_count) {
                skipped.push(mismatch);
                continue;
            }
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

        let overlays = RwLock::new(discover_overlays(&root));
        // probe_in resolves a file id, which needs the assembled tables and the
        // overlay search path (ffxi-dat/src/client_profile.rs probe_in): build the
        // root first, then fill the profile in.
        let mut root = Self {
            root,
            profile: ClientProfile::default(),
            apps,
            skipped,
            overlays,
            _lock: lock,
        };
        root.profile = ClientProfile::probe_in(&root);
        Ok(root)
    }

    /// Replace the overlay search path. Every constructor already seeds it from
    /// [`discover_overlays`]; this is for callers that configure it directly.
    pub fn with_overlays(self, overlays: Vec<PathBuf>) -> Self {
        self.set_overlays(overlays);
        self
    }

    /// Swap the overlay search path on a live root, so a settings change takes
    /// effect without a restart. Callers holding DAT-derived caches must drop
    /// them — this only changes which file a later resolve reads. The
    /// scheduler's zone-scene memo ([`crate::scheduler::clear_zone_scene_cache`])
    /// is cleared here automatically; every other DAT-derived cache is the
    /// holder's to drop.
    pub fn set_overlays(&self, overlays: Vec<PathBuf>) {
        *self
            .overlays
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = overlays;
        crate::scheduler::clear_zone_scene_cache();
    }

    pub fn overlays(&self) -> Vec<PathBuf> {
        self.overlays
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Resolve a location to a real file: each overlay in order, then the base
    /// install under either `.DAT` spelling. The raw `.DAT` join is returned
    /// unconditionally when no spelling exists, so a missing file still
    /// surfaces as a read error at the install path the caller expects.
    pub fn path_of(&self, loc: &DatLocation) -> PathBuf {
        self.overlays
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find_map(|overlay| loc.find_under(overlay))
            .or_else(|| loc.find_under(&self.root))
            .unwrap_or_else(|| loc.join_under(&self.root))
    }

    pub fn from_env() -> Result<Self> {
        let root = env::var_os(DAT_PATH_ENV).ok_or(DatError::EnvMissing)?;
        Self::open(PathBuf::from(root))
    }

    /// The install [`crate::install::resolve`] names: `FFXI_DAT_PATH`, else
    /// the registry's `default` pointer.
    pub fn from_env_or_default() -> Result<Self> {
        let resolved = crate::install::resolve().map_err(|u| DatError::NoInstall {
            reason: u.to_string(),
        })?;
        Self::open(resolved.path)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn profile(&self) -> &ClientProfile {
        &self.profile
    }

    pub fn app_summary(&self) -> Vec<(String, u32, u32)> {
        self.apps
            .iter()
            .map(|a| (a.rom_dir.clone(), a.vtable.len(), a.ftable.len()))
            .collect()
    }

    /// The C2S 0x26 excode_client this install's ROM inventory describes.
    /// research/XiPackets/lobby/C2S_0x0026_RequestLobbyLogin.md excode_client.
    pub fn excode_client(&self) -> u16 {
        let present: Vec<u8> = self.apps.iter().map(|a| a.rom_index).collect();
        excode_client_from_rom_indices(&present)
    }

    /// ROMs whose tables were rejected at open; empty on a well-formed install.
    pub fn skipped_tables(&self) -> &[TableSizeMismatch] {
        &self.skipped
    }

    /// Size of the file-id space: the base VTABLE's length, which every
    /// accepted expansion table matches.
    pub fn file_id_count(&self) -> u32 {
        self.apps.first().map_or(0, |base| base.vtable.len())
    }

    /// The highest ROM claiming `file_id` owns it. LoadFileTables
    /// (research/XIClient/src/XIClient/source/System/FileIO/FileIOVirtualFileSystem.cpp)
    /// merges ROM2..INDEX_ROM_MAX ascending into the base tables and overwrites
    /// the owner on every claim, so a later ROM's copy shadows an earlier one.
    pub fn resolve(&self, file_id: u32) -> Result<DatLocation> {
        for app in self.apps.iter().rev() {
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

/// Test-support entry point, `pub` only so real-DAT guards in sibling crates can
/// share it. Opens the install [`crate::install::resolve`] names; `None` with a
/// printed reason, so a vacuous pass is not mistaken for a real one, when
/// nothing resolves or the install does not open. A set but unusable
/// `FFXI_DAT_PATH` is a skip that says so, not a fallthrough to another
/// install.
#[doc(hidden)]
pub fn open_test_install() -> Option<DatRoot> {
    let resolved = match crate::install::resolve() {
        Ok(r) => r,
        Err(u) => {
            eprintln!("SKIP (real-DAT guard): {u}");
            return None;
        }
    };
    match DatRoot::open(&resolved.path) {
        Ok(root) => Some(root),
        Err(e) => {
            eprintln!(
                "SKIP (real-DAT guard): {} ({}) is not a usable install: {e}",
                resolved.path.display(),
                resolved.source
            );
            None
        }
    }
}

/// One row of retail's ROM-presence-to-expansion fold.
///
/// research/XIClient/src/XIClient/source/System/FileIO/FileIOVirtualFileSystem.cpp
/// FileIOVirtualFileSystem::LoadFileTables computes `bit` arithmetically --
/// `1 << (rom_index - 1)` below ROM9 and `1 << (rom_index + 2)` from ROM9 up --
/// and gates the three add-on ROMs on Rise of the Zilart already being folded
/// in. The shifts are spelled out as the names
/// vendor/server/src/login/login_helpers.h EXPANSION_DISPLAY gives those same
/// bits, and the gate as `requires`, so the rule reads as content rather than
/// as arithmetic.
struct RomExpansion {
    rom_index: u8,
    bit: u16,
    requires: Option<u16>,
}

/// In LoadFileTables fold order, so each `requires` gate sees the bits a
/// lower ROM contributed; rom_expansion_bits_match_the_retail_shift_arithmetic
/// pins the shape.
const ROM_EXPANSION_BITS: &[RomExpansion] = &[
    RomExpansion {
        rom_index: 2,
        bit: expansion_display::RISE_OF_ZILART,
        requires: None,
    },
    RomExpansion {
        rom_index: 3,
        bit: expansion_display::CHAINS_OF_PROMATHIA,
        requires: None,
    },
    RomExpansion {
        rom_index: 4,
        bit: expansion_display::TREASURES_OF_AHT_URGHAN,
        requires: None,
    },
    RomExpansion {
        rom_index: 5,
        bit: expansion_display::WINGS_OF_THE_GODDESS,
        requires: None,
    },
    RomExpansion {
        rom_index: 6,
        bit: expansion_display::A_CRYSTALLINE_PROPHECY,
        requires: Some(expansion_display::RISE_OF_ZILART),
    },
    RomExpansion {
        rom_index: 7,
        bit: expansion_display::A_MOOGLE_KUPOD_ETAT,
        requires: Some(expansion_display::RISE_OF_ZILART),
    },
    RomExpansion {
        rom_index: 8,
        bit: expansion_display::A_SHANTOTTO_ASCENSION,
        requires: Some(expansion_display::RISE_OF_ZILART),
    },
    RomExpansion {
        rom_index: 9,
        bit: expansion_display::SEEKERS_OF_ADOULIN,
        requires: None,
    },
    RomExpansion {
        rom_index: 10,
        bit: expansion_display::UNUSED_EXPANSION_1,
        requires: None,
    },
    RomExpansion {
        rom_index: 11,
        bit: expansion_display::UNUSED_EXPANSION_2,
        requires: None,
    },
    RomExpansion {
        rom_index: 12,
        bit: expansion_display::UNUSED_EXPANSION_3,
        requires: None,
    },
    RomExpansion {
        rom_index: 13,
        bit: expansion_display::UNUSED_EXPANSION_4,
        requires: None,
    },
];

/// The three Abyssea add-ons ship no ROM of their own; LoadFileTables turns
/// them on together once the fold holds both Rise of the Zilart and Wings of
/// the Goddess.
const ABYSSEA_BITS: u16 = expansion_display::VISIONS_OF_ABYSSEA
    | expansion_display::SCARS_OF_ABYSSEA
    | expansion_display::HEROES_OF_ABYSSEA;
const ABYSSEA_REQUIRES: u16 =
    expansion_display::RISE_OF_ZILART | expansion_display::WINGS_OF_THE_GODDESS;

/// The C2S 0x26 excode_client the ROM inventory `present` describes.
///
/// Driven by [`ROM_EXPANSION_BITS`] rather than by the caller's order, so an
/// unsorted inventory cannot break the add-on ROMs' Rise-of-the-Zilart gate.
/// BASE_GAME is unconditional because
/// research/XIClient/src/XIClient/source/Network/Lobby/LoginStateMachine.cpp
/// LoginStateMachine::HandleLogin sends `1 | ClientExpansions`, whatever the
/// fold produced.
fn excode_client_from_rom_indices(present: &[u8]) -> u16 {
    let mut mask = expansion_display::BASE_GAME;
    for entry in ROM_EXPANSION_BITS {
        if !present.contains(&entry.rom_index) {
            continue;
        }
        if entry.requires.is_some_and(|req| mask & req != req) {
            continue;
        }
        mask |= entry.bit;
    }
    if mask & ABYSSEA_REQUIRES == ABYSSEA_REQUIRES {
        mask |= ABYSSEA_BITS;
    }
    mask
}

/// The excode_client for the install at `root`, or `None` when `root` holds no
/// base FTABLE.DAT and so is not an install at all (LoadFileTables fails there
/// too, before it folds a single bit).
///
/// Gated on the same thing retail gates on, each expansion FTABLE existing,
/// which is weaker than the table-size validation [`DatRoot::open`] applies;
/// [`DatRoot::excode_client`] is the same fold over that validated inventory.
pub fn excode_client_at(root: &Path) -> Option<u16> {
    if !appid_paths(root, BASE_ROM_INDEX).2.exists() {
        return None;
    }
    let present: Vec<u8> = (BASE_ROM_INDEX + 1..=RETAIL_INDEX_ROM_MAX)
        .filter(|i| appid_paths(root, *i).2.exists())
        .collect();
    Some(excode_client_from_rom_indices(&present))
}

fn appid_paths(root: &Path, i: u8) -> (String, PathBuf, PathBuf) {
    if i == BASE_ROM_INDEX {
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

    /// Empty FTABLEs at the paths retail probes; the fold tests existence only.
    fn synth_rom_ftables(rom_indices: &[u8]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for i in rom_indices {
            let (_rom_dir, _vt_path, ft_path) = appid_paths(dir.path(), *i);
            if let Some(parent) = ft_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&ft_path, []).unwrap();
        }
        dir
    }

    fn excode_of(rom_indices: &[u8]) -> u16 {
        let dir = synth_rom_ftables(rom_indices);
        excode_client_at(dir.path()).unwrap()
    }

    /// The rows are spelled out so they can carry the LSB names, but retail
    /// derives them: `1 << (rom_index - 1)` below ROM9, `1 << (rom_index + 2)`
    /// from ROM9 up, the three add-on ROMs gated on Rise of the Zilart
    /// (research/XIClient/src/XIClient/source/System/FileIO/FileIOVirtualFileSystem.cpp
    /// LoadFileTables). Without this, a mistyped row is a wrong mask on a
    /// retail lobby and every other test here agrees with it.
    #[test]
    fn rom_expansion_bits_match_the_retail_shift_arithmetic() {
        const FIRST_HIGH_ROM_INDEX: u8 = 9;
        const LOW_ROM_SHIFT_BACK: u8 = 1;
        const HIGH_ROM_SHIFT_FORWARD: u8 = 2;
        const FIRST_ADDON_ROM_INDEX: u8 = 6;
        const LAST_ADDON_ROM_INDEX: u8 = 8;

        let mut expected_index = BASE_ROM_INDEX + 1;
        let mut named = expansion_display::BASE_GAME | ABYSSEA_BITS;
        for entry in ROM_EXPANSION_BITS {
            assert_eq!(
                entry.rom_index, expected_index,
                "the table skips a ROM index retail probes"
            );
            expected_index += 1;
            let shift = if entry.rom_index < FIRST_HIGH_ROM_INDEX {
                entry.rom_index - LOW_ROM_SHIFT_BACK
            } else {
                entry.rom_index + HIGH_ROM_SHIFT_FORWARD
            };
            assert_eq!(entry.bit, 1u16 << shift, "ROM{}", entry.rom_index);
            let gated = (FIRST_ADDON_ROM_INDEX..=LAST_ADDON_ROM_INDEX).contains(&entry.rom_index);
            assert_eq!(
                entry.requires,
                gated.then_some(expansion_display::RISE_OF_ZILART),
                "ROM{}",
                entry.rom_index
            );
            assert_eq!(named & entry.bit, 0, "ROM{} re-uses a bit", entry.rom_index);
            named |= entry.bit;
        }
        assert_eq!(
            expected_index,
            RETAIL_INDEX_ROM_MAX + 1,
            "the table stops before retail's probe does"
        );
        assert_eq!(
            named & expansion_display::ALL_KNOWN,
            expansion_display::ALL_KNOWN,
            "the fold can never produce every expansion LSB names"
        );
    }

    #[test]
    fn excode_client_is_base_only_without_an_expansion_rom() {
        assert_eq!(
            excode_of(&[BASE_ROM_INDEX]),
            expansion_display::BASE_GAME,
            "HandleLogin forces BASE_GAME on and no ROM contributes it"
        );
    }

    #[test]
    fn expansion_roms_set_their_own_bit_and_bundle_abyssea() {
        assert_eq!(
            excode_of(&[BASE_ROM_INDEX, 2, 3, 4, 5]),
            expansion_display::BASE_GAME
                | expansion_display::RISE_OF_ZILART
                | expansion_display::CHAINS_OF_PROMATHIA
                | expansion_display::TREASURES_OF_AHT_URGHAN
                | expansion_display::WINGS_OF_THE_GODDESS
                | expansion_display::VISIONS_OF_ABYSSEA
                | expansion_display::SCARS_OF_ABYSSEA
                | expansion_display::HEROES_OF_ABYSSEA
        );
        assert_eq!(
            excode_of(&[BASE_ROM_INDEX, 2]) & ABYSSEA_BITS,
            0,
            "Abyssea needs Wings of the Goddess as well"
        );
        assert_eq!(
            excode_of(&[BASE_ROM_INDEX, 9]),
            expansion_display::BASE_GAME | expansion_display::SEEKERS_OF_ADOULIN
        );
    }

    #[test]
    fn addon_roms_need_rise_of_zilart_to_count() {
        let addons = expansion_display::A_CRYSTALLINE_PROPHECY
            | expansion_display::A_MOOGLE_KUPOD_ETAT
            | expansion_display::A_SHANTOTTO_ASCENSION;
        assert_eq!(excode_of(&[BASE_ROM_INDEX, 6, 7, 8]) & addons, 0);
        assert_eq!(excode_of(&[BASE_ROM_INDEX, 2, 6, 7, 8]) & addons, addons);
        assert_eq!(
            excode_of(&[8, 7, 6, 2, BASE_ROM_INDEX]) & addons,
            addons,
            "the gate must not depend on the caller's order"
        );
    }

    // ROM directories as shipped by the registered installs: retail stops at
    // ROM9, horizonxi-2023 adds ROM10, a bit LSB has no name for.
    #[test]
    fn known_client_rom_shapes_map_to_their_expansion_masks() {
        let retail_shape: Vec<u8> = (BASE_ROM_INDEX..=9).collect();
        let mut horizon_shape = retail_shape.clone();
        horizon_shape.push(10);
        for (row, shape, expected) in [
            (
                "retail-2019-base",
                &retail_shape,
                expansion_display::ALL_KNOWN,
            ),
            (
                "retail-2026-09",
                &retail_shape,
                expansion_display::ALL_KNOWN,
            ),
            (
                "horizonxi-2023",
                &horizon_shape,
                expansion_display::ALL_KNOWN | expansion_display::UNUSED_EXPANSION_1,
            ),
        ] {
            assert!(
                crate::client_profile::KNOWN_CLIENTS
                    .iter()
                    .any(|k| k.name == row),
                "{row} is not a KNOWN_CLIENTS row"
            );
            assert_eq!(excode_of(shape), expected, "{row}");
        }
    }

    #[test]
    fn rom_indices_past_the_retail_probe_are_ignored() {
        assert_eq!(
            excode_of(&[BASE_ROM_INDEX, 14, MAX_ROM_INDEX]),
            expansion_display::BASE_GAME
        );
    }

    #[test]
    fn dat_root_inventory_and_the_path_probe_agree() {
        let (tmp, root) = synth_root(&[
            SynthApp {
                rom_index: BASE_ROM_INDEX,
                vtable: vec![0, 1],
                ftable_words: vec![0x0000, 0x0080],
            },
            SynthApp {
                rom_index: 2,
                vtable: vec![0, 2],
                ftable_words: vec![0x0000, 0x0100],
            },
            SynthApp {
                rom_index: 5,
                vtable: vec![0, 5],
                ftable_words: vec![0x0000, 0x0180],
            },
        ]);
        assert_eq!(
            root.excode_client(),
            expansion_display::BASE_GAME
                | expansion_display::RISE_OF_ZILART
                | expansion_display::WINGS_OF_THE_GODDESS
                | ABYSSEA_BITS
        );
        assert_eq!(Some(root.excode_client()), excode_client_at(tmp.path()));
    }

    #[test]
    fn excode_client_at_is_none_without_a_base_ftable() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(excode_client_at(dir.path()), None);
        assert_eq!(excode_client_at(&dir.path().join("ROM2")), None);
    }

    #[test]
    fn installed_rom_inventory_covers_every_known_expansion() {
        let Some(root) = open_test_install() else {
            return;
        };
        let derived = root.excode_client();
        assert_eq!(
            derived & expansion_display::ALL_KNOWN,
            expansion_display::ALL_KNOWN,
            "{}: {derived:#06x}",
            root.root().display()
        );
        assert_eq!(excode_client_at(root.root()), Some(derived));
        eprintln!("{}: excode_client {derived:#06x}", root.root().display());
    }

    // Ids 1 and 2 are claimed by the base ROM; ROM2 re-claims id 1 and ROM3
    // re-claims both, each with its own FTABLE entry. The merge in
    // research/XIClient/src/XIClient/source/System/FileIO/FileIOVirtualFileSystem.cpp
    // LoadFileTables walks ROMs ascending, so the highest claimant owns the id.
    #[test]
    fn resolve_picks_highest_appid_that_claims_file_id() {
        let (_tmp, root) = synth_root(&[
            SynthApp {
                rom_index: 1,
                vtable: vec![0, 1, 1, 0, 0],
                ftable_words: vec![0x0000, 0x0080, 0x00FF, 0x0000, 0x0000],
            },
            SynthApp {
                rom_index: 2,
                vtable: vec![0, 2, 0, 2, 0],
                ftable_words: vec![0x0000, 0x0100, 0x0000, 0x0001, 0x0000],
            },
            SynthApp {
                rom_index: 3,
                vtable: vec![0, 3, 3, 0, 3],
                ftable_words: vec![0x0000, 0x0180, 0x0181, 0x0000, 0xFFFF],
            },
        ]);

        let loc1 = root.resolve(1).unwrap();
        assert_eq!(loc1.rom_dir, "ROM3");
        assert_eq!(loc1.sub_path, SubPath { dir: 3, file: 0 });

        let loc2 = root.resolve(2).unwrap();
        assert_eq!(loc2.rom_dir, "ROM3");
        assert_eq!(loc2.sub_path, SubPath { dir: 3, file: 1 });

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
        assert!(root.skipped_tables().is_empty());
        assert_eq!(root.file_id_count(), 5);
    }

    #[test]
    fn expansion_vtable_of_another_length_is_skipped() {
        let (_tmp, root) = synth_root(&[
            SynthApp {
                rom_index: 1,
                vtable: vec![0, 1, 1],
                ftable_words: vec![0x0000, 0x0080, 0x00FF],
            },
            SynthApp {
                rom_index: 2,
                vtable: vec![0, 2, 2, 2],
                ftable_words: vec![0x0000, 0x0100, 0x0101, 0x0102],
            },
            SynthApp {
                rom_index: 3,
                vtable: vec![0, 0, 3],
                ftable_words: vec![0x0000, 0x0000, 0x0180],
            },
        ]);

        let [skipped] = root.skipped_tables() else {
            panic!("exactly ROM2 must be skipped: {:?}", root.skipped_tables());
        };
        assert_eq!(skipped.rom_dir, "ROM2");
        assert_eq!(skipped.path, root.root().join("ROM2").join("VTABLE2.DAT"));
        assert_eq!((skipped.len, skipped.expected), (4, 3));
        assert_eq!(root.file_id_count(), 3);

        assert_eq!(root.resolve(1).unwrap().rom_dir, "ROM");
        assert_eq!(root.resolve(2).unwrap().rom_dir, "ROM3");
        assert!(matches!(
            root.resolve(3),
            Err(DatError::FileNotPresent { file_id: 3 })
        ));
    }

    #[test]
    fn ftable_not_twice_its_vtable_is_skipped() {
        let (_tmp, root) = synth_root(&[
            SynthApp {
                rom_index: 1,
                vtable: vec![0, 1, 1],
                ftable_words: vec![0x0000, 0x0080, 0x00FF],
            },
            SynthApp {
                rom_index: 2,
                vtable: vec![0, 2, 2],
                ftable_words: vec![0x0000, 0x0100],
            },
        ]);

        let [skipped] = root.skipped_tables() else {
            panic!("exactly ROM2 must be skipped: {:?}", root.skipped_tables());
        };
        assert_eq!(skipped.path, root.root().join("ROM2").join("FTABLE2.DAT"));
        assert_eq!((skipped.len, skipped.expected), (4, 6));
        assert_eq!(root.resolve(1).unwrap().rom_dir, "ROM");
        assert_eq!(root.app_summary().len(), 1);
    }

    // Real-install guard: every ROM's tables fit the base id space (no ROM
    // skipped), and wherever more than one ROM claims an id the highest wins.
    // The horizonxi-2023 target ships a ROM10
    // that re-claims base-ROM ids; retail-2026-09 has no multi-claims, where
    // this passes vacuously.
    #[test]
    fn installed_tables_all_fit_and_highest_claim_wins() {
        let Some(root) = open_test_install() else {
            return;
        };
        assert!(
            root.skipped_tables().is_empty(),
            "{:?}",
            root.skipped_tables()
        );
        let summary = root.app_summary();
        let id_count = root.file_id_count();
        assert!(id_count > 0);
        for (rom_dir, vtable_len, ftable_len) in &summary {
            assert_eq!(*vtable_len, id_count, "{rom_dir} VTABLE");
            assert_eq!(*ftable_len, id_count, "{rom_dir} FTABLE");
        }

        let mut multi_claims = 0u32;
        for file_id in 0..id_count {
            let claimants: Vec<&str> = root
                .apps
                .iter()
                .filter(|app| app.vtable.contains(file_id, app.rom_index))
                .map(|app| app.rom_dir.as_str())
                .collect();
            let Some(highest) = claimants.last() else {
                continue;
            };
            if claimants.len() > 1 {
                multi_claims += 1;
            }
            assert_eq!(
                root.resolve(file_id).unwrap().rom_dir,
                *highest,
                "file id {file_id} claimed by {claimants:?}"
            );
        }
        eprintln!(
            "{}: {} ROMs, {id_count} ids, {multi_claims} claimed by more than one ROM",
            root.root().display(),
            summary.len()
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
        let p = loc.path_under(&root);
        assert_eq!(p, tmp.path().join("ROM2").join("511").join("127.DAT"));
    }

    /// Writes `<overlay>/ROM2/511/127.<ext>` — the path file id 2 resolves to in
    /// `overlay_root()` — and returns it.
    fn write_overlay_entry(overlay: &Path, ext: &str, body: &[u8]) -> PathBuf {
        let dir = overlay.join("ROM2").join("511");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("127.{ext}"));
        fs::write(&path, body).unwrap();
        path
    }

    fn overlay_root() -> (tempfile::TempDir, DatRoot) {
        synth_root(&[SynthApp {
            rom_index: 2,
            vtable: vec![0, 0, 2],
            ftable_words: vec![0x0000, 0x0000, 0xFFFF],
        }])
    }

    /// What a reader of file id 2 actually gets. Asserting on bytes rather than
    /// on the path keeps these honest on a case-insensitive filesystem, where
    /// `127.DAT` and `127.dat` name the same file.
    fn served_bytes(root: &DatRoot) -> Vec<u8> {
        fs::read(root.resolve(2).unwrap().path_under(root)).unwrap()
    }

    #[test]
    fn overlays_take_precedence_in_order() {
        let (_tmp, root) = overlay_root();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        write_overlay_entry(first.path(), "DAT", b"first");
        write_overlay_entry(second.path(), "DAT", b"second");

        let root = root.with_overlays(vec![first.path().into(), second.path().into()]);
        assert_eq!(served_bytes(&root), b"first");
    }

    #[test]
    fn an_overlay_that_lacks_the_file_falls_through() {
        let (tmp, root) = overlay_root();
        let empty = tempfile::tempdir().unwrap();
        let backing = tempfile::tempdir().unwrap();
        write_overlay_entry(backing.path(), "DAT", b"backing");

        let with_both = root.with_overlays(vec![empty.path().into(), backing.path().into()]);
        assert_eq!(served_bytes(&with_both), b"backing");

        // Nothing claims it: the base install path, so a missing file still
        // reports against the install the caller expects.
        let only_empty = with_both.with_overlays(vec![empty.path().into()]);
        assert_eq!(
            only_empty.resolve(2).unwrap().path_under(&only_empty),
            tmp.path().join("ROM2").join("511").join("127.DAT")
        );
    }

    // The XI-Pivot overlays of the horizonxi-2023 target
    // mix both spellings, which only matters
    // where the filesystem is case-sensitive.
    #[test]
    fn overlay_matches_a_lowercase_extension() {
        let (_tmp, root) = overlay_root();
        let overlay = tempfile::tempdir().unwrap();
        write_overlay_entry(overlay.path(), "dat", b"lower");

        let root = root.with_overlays(vec![overlay.path().into()]);
        assert_eq!(served_bytes(&root), b"lower");
    }

    #[test]
    fn base_install_matches_a_lowercase_extension() {
        let (tmp, root) = overlay_root();
        write_overlay_entry(tmp.path(), "dat", b"base-lower");
        assert_eq!(served_bytes(&root), b"base-lower");
    }

    /// The shape XI-Pivot actually ships for the horizonxi-2023 client profile
    /// — including the Windows `root_path` that cannot resolve off Windows.
    const REAL_PIVOT_INI: &str = "\
[settings]
root_path=C:\\Program Files (x86)\\HorizonXI\\HorizonXI\\Game\\polplugins\\DATs
debug_log=false
redirect_fopens=true
[overlays]
0=horizonmusic
1=horizonoverrides
2=xiview
";

    #[test]
    fn pivot_ini_parses_in_index_order() {
        let (root_path, names) = parse_pivot_ini(REAL_PIVOT_INI);
        assert_eq!(
            root_path,
            Some(PathBuf::from(
                "C:\\Program Files (x86)\\HorizonXI\\HorizonXI\\Game\\polplugins\\DATs"
            ))
        );
        assert_eq!(names, ["horizonmusic", "horizonoverrides", "xiview"]);
    }

    #[test]
    fn pivot_ini_ignores_comments_and_orders_by_index_not_file_order() {
        let (_, names) = parse_pivot_ini(
            "; a comment\n[overlays]\n2=third\n0=first\n; another\n1=second\n[settings]\nroot_path=x\n",
        );
        assert_eq!(names, ["first", "second", "third"]);
    }

    /// Builds `<game>/SquareEnix/FINAL FANTASY XI` plus the Pivot config and
    /// overlay dirs beside it, and returns the game dir and install root.
    fn synth_pivot_install(ini: &str, overlay_dirs: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let game = tempfile::tempdir().unwrap();
        let install = game.path().join("SquareEnix").join("FINAL FANTASY XI");
        fs::create_dir_all(&install).unwrap();
        let ini_path = game.path().join(PIVOT_INI);
        fs::create_dir_all(ini_path.parent().unwrap()).unwrap();
        fs::write(&ini_path, ini).unwrap();
        for d in overlay_dirs {
            fs::create_dir_all(game.path().join(PIVOT_DAT_DIR).join(d)).unwrap();
        }
        (game, install)
    }

    #[test]
    fn discovery_falls_back_to_the_local_dat_dir_when_root_path_is_a_windows_path() {
        let (game, install) = synth_pivot_install(
            REAL_PIVOT_INI,
            &["horizonmusic", "horizonoverrides", "xiview"],
        );
        let dats = game.path().join(PIVOT_DAT_DIR);
        assert_eq!(
            discover_overlays(&install),
            vec![
                dats.join("horizonmusic"),
                dats.join("horizonoverrides"),
                dats.join("xiview"),
            ]
        );
    }

    // A name in pivot.ini with no directory behind it must not become a search
    // path that silently matches nothing.
    #[test]
    fn discovery_drops_overlays_with_no_directory() {
        let (game, install) = synth_pivot_install(REAL_PIVOT_INI, &["xiview"]);
        assert_eq!(
            discover_overlays(&install),
            vec![game.path().join(PIVOT_DAT_DIR).join("xiview")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn discovery_follows_a_symlinked_install_root_to_the_real_game_dir() {
        let (game, install) = synth_pivot_install(REAL_PIVOT_INI, &["xiview"]);
        let link_home = tempfile::tempdir().unwrap();
        let link_parent = link_home.path().join("SquareEnix");
        fs::create_dir_all(&link_parent).unwrap();
        let link = link_parent.join("FINAL FANTASY XI");
        std::os::unix::fs::symlink(&install, &link).unwrap();
        assert_eq!(
            discover_overlays(&link),
            vec![game.path().join(PIVOT_DAT_DIR).join("xiview")]
        );
    }

    #[test]
    fn discovery_yields_nothing_without_a_pivot_config() {
        let game = tempfile::tempdir().unwrap();
        let install = game.path().join("SquareEnix").join("FINAL FANTASY XI");
        fs::create_dir_all(&install).unwrap();
        assert!(discover_overlays(&install).is_empty());
    }

    #[test]
    fn overlays_can_be_swapped_on_a_live_shared_root() {
        let (_tmp, root) = overlay_root();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        write_overlay_entry(first.path(), "DAT", b"first");
        write_overlay_entry(second.path(), "DAT", b"second");

        let root = std::sync::Arc::new(root.with_overlays(vec![first.path().into()]));
        assert_eq!(served_bytes(&root), b"first");

        // Through the shared handle, with no rebuild and no &mut.
        let shared = std::sync::Arc::clone(&root);
        shared.set_overlays(vec![second.path().into()]);
        assert_eq!(served_bytes(&root), b"second");

        shared.set_overlays(Vec::new());
        assert!(root.overlays().is_empty());
    }

    // Real-install guard: whatever pivot.ini the user's server ships, discovery
    // must return exactly the directories it names that exist, in index order.
    // Self-skips on a vanilla install with no Pivot, and on a shell that has
    // already overridden the list.
    #[test]
    fn discovery_matches_the_installed_pivot_config() {
        if env::var_os(OVERLAY_ENV).is_some() {
            eprintln!("SKIP: {OVERLAY_ENV} is set, which bypasses pivot discovery");
            return;
        }
        let Some(root) = open_test_install() else {
            return;
        };
        let Some(game_dir) = game_dir(root.root()) else {
            return;
        };
        let Ok(ini) = fs::read_to_string(game_dir.join(PIVOT_INI)) else {
            eprintln!("SKIP: no {PIVOT_INI} beside the install (vanilla, not a private server)");
            return;
        };

        let (root_path, names) = parse_pivot_ini(&ini);
        let dat_dir = root_path
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| game_dir.join(PIVOT_DAT_DIR));
        let expected: Vec<PathBuf> = names
            .iter()
            .map(|n| dat_dir.join(n))
            .filter(|p| p.is_dir())
            .collect();

        assert_eq!(discover_overlays(root.root()), expected);
        assert_eq!(
            root.overlays(),
            expected,
            "DatRoot::open must seed overlays from the same discovery"
        );
        assert!(
            !names.is_empty(),
            "a pivot.ini with no [overlays] entries is not a useful fixture"
        );
    }

    #[test]
    fn no_overlays_is_the_base_install() {
        let (tmp, root) = overlay_root();
        assert!(root.overlays().is_empty(), "unset env means vanilla");
        assert_eq!(
            root.resolve(2).unwrap().path_under(&root),
            tmp.path().join("ROM2").join("511").join("127.DAT")
        );
    }

    #[test]
    fn empty_install_errors() {
        let dir = tempfile::tempdir().unwrap();
        let err = DatRoot::open(dir.path()).unwrap_err();
        assert!(matches!(err, DatError::Io { .. }));
    }
}
