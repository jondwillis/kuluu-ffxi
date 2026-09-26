//! Pins [`string_dat_file_id`] against a real install: the ids the formula
//! computes must resolve, through the install's own VTABLE/FTABLE, to files
//! that parse as dialog tables. Self-skips without an install.

use ffxi_dat::archive::{open_test_install, DatRoot};
use ffxi_dat::dmsg::StringDat;
use ffxi_dat::zone_dat::{string_dat_file_id, ZONE_DAT_TABLE};

/// Fewest of the LSB zone set whose dialog DAT must resolve and parse.
/// horizonxi-2023 and retail-2026-09 both reach 294 of 299; the margin
/// absorbs a zone a later patch relocates without hiding a broken offset.
const MIN_PARSED_ZONES: usize = 294;

/// `(zone_id, file_id)` either side of the offset switch, measured on
/// horizonxi-2023 and retail-2026-09.
const PINNED_IDS: &[(u16, u32)] = &[(230, 6650), (256, 85591)];

fn install() -> Option<DatRoot> {
    let root = open_test_install();
    if root.is_none() {
        eprintln!("SKIP: no FFXI install");
    }
    root
}

/// The same install with the content-substitution overlay cleared, so the
/// pin sees the install's own dialog tables, not a Pivot era pack's
/// (ffxi-dat/src/archive.rs discover_overlays).
fn vanilla_install() -> Option<DatRoot> {
    let root = open_test_install()?;
    root.set_overlays(Vec::new());
    Some(root)
}

fn parse(root: &DatRoot, zone: u16) -> Result<StringDat, String> {
    let file_id = string_dat_file_id(zone);
    let loc = root
        .resolve(file_id)
        .map_err(|e| format!("zone {zone} file id {file_id}: {e}"))?;
    let bytes = std::fs::read(loc.path_under(root))
        .map_err(|e| format!("zone {zone} file id {file_id}: {e}"))?;
    StringDat::parse(&bytes).map_err(|e| format!("zone {zone} file id {file_id}: {e}"))
}

#[test]
fn pinned_zones_address_the_measured_file_ids() {
    for &(zone, file_id) in PINNED_IDS {
        assert_eq!(string_dat_file_id(zone), file_id, "zone {zone}");
    }
    let Some(root) = install() else { return };
    for &(zone, _) in PINNED_IDS {
        parse(&root, zone).unwrap_or_else(|e| panic!("{e}"));
    }
}

#[test]
fn the_zone_set_resolves_to_files_that_parse_as_dialog_tables() {
    let Some(root) = vanilla_install() else {
        return;
    };
    let mut parsed = 0usize;
    let mut failures = Vec::new();
    for &(zone, _) in ZONE_DAT_TABLE {
        match parse(&root, zone) {
            Ok(_) => parsed += 1,
            Err(e) => failures.push(e),
        }
    }
    eprintln!(
        "dialog: {parsed}/{} zone dialog DATs parse under {}",
        ZONE_DAT_TABLE.len(),
        root.root().display()
    );
    assert!(
        parsed >= MIN_PARSED_ZONES,
        "{parsed} of {} zone dialog DATs parse, expected at least \
         {MIN_PARSED_ZONES}: {failures:?}",
        ZONE_DAT_TABLE.len()
    );
}

#[test]
fn whitegate_and_feretory_use_the_english_dialog_members() {
    let Some(root) = install() else { return };
    // vendor/server/scripts/zones/Aht_Urhgan_Whitegate/IDs.lua and vendor/server/scripts/zones/Feretory/IDs.lua KEYITEM_OBTAINED.
    const RETAIL_TEXT_PINS: &[(u16, usize)] = &[(50, 235), (285, 6398)];
    for &(zone, index) in RETAIL_TEXT_PINS {
        let dat = parse(&root, zone).expect("English dialog DAT");
        let prefix = "Obtained key item:";
        assert!(
            dat.first_entry_starting_with(prefix).is_some(),
            "zone {zone} is not English dialog"
        );
        if root.profile().name() == "retail-2026-09" {
            assert!(
                dat.text(index).is_some_and(|text| text.starts_with(prefix)),
                "zone {zone} text {index} mismatches LSB"
            );
        }
    }
}
