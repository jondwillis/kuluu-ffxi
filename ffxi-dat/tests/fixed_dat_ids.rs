//! Pins the file ids this crate reads by id — item, spell, emote, system
//! message and UI sheet — to the install-relative path each resolves to, so a
//! patch that re-homes one fails here instead of silently reading a stale copy.
//! Self-skips without an install.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ffxi_dat::archive::{open_test_install, DatLocation, DatRoot};
use ffxi_dat::dmsg::EMOTE_TEXT_FILE_ID;
use ffxi_dat::item_dat::{
    ITEM_DAT_ARMOR, ITEM_DAT_ARMOR_EXPANSION, ITEM_DAT_CURRENCY, ITEM_DAT_GENERAL,
    ITEM_DAT_INSTINCT, ITEM_DAT_ITEMS_EXPANSION, ITEM_DAT_MONIPULATOR, ITEM_DAT_PUPPET,
    ITEM_DAT_USABLE, ITEM_DAT_VOUCHERS_AND_SLIPS, ITEM_DAT_WEAPON,
};
use ffxi_dat::spell_info::SPELL_LIST_FILE_ID;
use ffxi_dat::sysmes::{MES_BASIC_FILE_ID, SYS_MES_FILE_ID};
use ffxi_dat::ui_element::UI_SHEET_FILE_ID;

/// Measured identical on every install under `vendor/game-files/targets/`.
const PINS: &[(u32, &str)] = &[
    (ITEM_DAT_GENERAL, "ROM/118/106.DAT"),
    (ITEM_DAT_USABLE, "ROM/118/107.DAT"),
    (ITEM_DAT_WEAPON, "ROM/118/108.DAT"),
    (ITEM_DAT_ARMOR, "ROM/118/109.DAT"),
    (ITEM_DAT_PUPPET, "ROM/118/110.DAT"),
    (ITEM_DAT_VOUCHERS_AND_SLIPS, "ROM/217/21.DAT"),
    (ITEM_DAT_MONIPULATOR, "ROM/288/67.DAT"),
    (ITEM_DAT_INSTINCT, "ROM/288/80.DAT"),
    (ffxi_dat::key_item::KEY_ITEM_TEXT_FILE_ID, "ROM/118/115.DAT"),
    (ITEM_DAT_CURRENCY, "ROM/174/48.DAT"),
    (ITEM_DAT_ARMOR_EXPANSION, "ROM/286/73.DAT"),
    (ITEM_DAT_ITEMS_EXPANSION, "ROM/301/115.DAT"),
    (SPELL_LIST_FILE_ID, "ROM/118/114.DAT"),
    (EMOTE_TEXT_FILE_ID, "ROM/27/70.DAT"),
    (MES_BASIC_FILE_ID, "ROM/27/72.DAT"),
    (SYS_MES_FILE_ID, "ROM/27/76.DAT"),
    (UI_SHEET_FILE_ID, "ROM/119/51.DAT"),
];

/// [`KNOWN_CLIENTS`](ffxi_dat::client_profile::KNOWN_CLIENTS) rows [`PINS`] was
/// measured on; another row still has to resolve every id to a readable file,
/// but its paths are reported rather than pinned.
const MEASURED_ROWS: [&str; 2] = ["horizonxi-2023", "retail-2026-09"];

fn install() -> Option<DatRoot> {
    let root = open_test_install();
    if root.is_none() {
        eprintln!("SKIP: no FFXI install");
    }
    root
}

fn relative_path(root: &DatRoot, file_id: u32) -> PathBuf {
    root.resolve(file_id)
        .unwrap_or_else(|e| panic!("file id {file_id} does not resolve: {e}"))
        .join_under(Path::new(""))
}

#[test]
fn every_fixed_id_resolves_to_the_path_it_was_measured_at() {
    let Some(root) = install() else { return };
    let client = root.profile().name();
    let measured = MEASURED_ROWS.contains(&client);
    if !measured {
        eprintln!("client profile {client} is not a measured row: reporting paths, not pinning");
    }
    for &(file_id, expected) in PINS {
        let actual = relative_path(&root, file_id);
        if measured {
            assert_eq!(
                actual,
                PathBuf::from(expected),
                "{client}: file id {file_id} moved"
            );
        } else {
            eprintln!("{client}: file id {file_id} -> {}", actual.display());
        }
    }
}

#[test]
fn every_fixed_id_resolves_to_a_file_that_exists() {
    let Some(root) = install() else { return };
    let missing: Vec<PathBuf> = PINS
        .iter()
        .map(|&(file_id, _)| {
            root.resolve(file_id)
                .unwrap_or_else(|e| panic!("file id {file_id} does not resolve: {e}"))
                .path_under(&root)
        })
        .filter(|path| !path.is_file())
        .collect();
    assert!(missing.is_empty(), "resolved but absent: {missing:#?}");
}

/// A re-homed file leaves its old copy behind, so a second id claiming a pinned
/// location is the shape that would let a stale read pass unnoticed.
#[test]
fn no_other_file_id_claims_a_fixed_location() {
    let Some(root) = install() else { return };
    let key = |loc: &DatLocation| (loc.rom_dir.clone(), loc.sub_path.dir, loc.sub_path.file);
    let mut claimants: HashMap<(String, u16, u8), Vec<u32>> = HashMap::new();
    for id in 0..root.file_id_count() {
        if let Ok(loc) = root.resolve(id) {
            claimants.entry(key(&loc)).or_default().push(id);
        }
    }
    for &(file_id, _) in PINS {
        let loc = root
            .resolve(file_id)
            .unwrap_or_else(|e| panic!("file id {file_id} does not resolve: {e}"));
        assert_eq!(
            claimants.get(&key(&loc)).cloned().unwrap_or_default(),
            vec![file_id],
            "{}: {}/{}/{}.DAT is claimed by more than file id {file_id}",
            root.profile().name(),
            loc.rom_dir,
            loc.sub_path.dir,
            loc.sub_path.file
        );
    }
}
