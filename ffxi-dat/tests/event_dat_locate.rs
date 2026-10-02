//! Pins [`event_dat_file_id`] against a real install: the ids it computes must
//! resolve, through the install's own VTABLE/FTABLE, to the event DATs a
//! hand-materialised zone -> ROM-path table names. Self-skips without an
//! install.

use std::path::{Path, PathBuf};

use ffxi_dat::archive::{open_test_install, DatRoot};
use ffxi_dat::event_dat::EventDat;
use ffxi_dat::event_locate::{event_dat_file_id, event_dat_zones, EVENT_DAT_ZONE_ID_MAX};

/// The one zone id inside `0..=EVENT_DAT_ZONE_ID_MAX` that VTABLE marks missing
/// on both installs, so it has no event DAT to resolve.
const EVENT_ZONE_ID_ABSENT: u16 = 286;

/// `(zone_id, install-relative DAT path)` carried over from the retired table,
/// spanning both offsets and both ends of the zone-id space.
const PINNED_PATHS: &[(u16, &str)] = &[
    (0, "ROM3/0/66.DAT"),
    (230, "ROM/21/39.DAT"),
    (256, "ROM9/5/53.DAT"),
    (299, "ROM/378/101.DAT"),
];

fn install() -> Option<DatRoot> {
    let root = open_test_install();
    if root.is_none() {
        eprintln!("SKIP: no FFXI install");
    }
    root
}

fn relative_path(root: &DatRoot, zone: u16) -> PathBuf {
    root.resolve(event_dat_file_id(zone))
        .unwrap_or_else(|e| panic!("zone {zone} event DAT id does not resolve: {e}"))
        .join_under(Path::new(""))
}

#[test]
fn pinned_zones_resolve_to_the_paths_the_table_named() {
    let Some(root) = install() else { return };
    for &(zone, expected) in PINNED_PATHS {
        assert_eq!(
            relative_path(&root, zone),
            PathBuf::from(expected),
            "zone {zone} event DAT moved"
        );
    }
}

#[test]
fn every_event_zone_resolves_to_a_file_that_parses() {
    let Some(root) = install() else { return };
    let zones = event_dat_zones(&root);
    let unresolved: Vec<u16> = (0..=EVENT_DAT_ZONE_ID_MAX)
        .filter(|zone| !zones.contains(zone))
        .collect();
    let mut missing = Vec::new();
    let mut unparsed = Vec::new();

    for &zone in &zones {
        let loc = root
            .resolve(event_dat_file_id(zone))
            .expect("listed zone locates");
        let path = loc.path_under(&root);
        match std::fs::read(&path) {
            Ok(bytes) if EventDat::parse(&bytes).is_ok() => {}
            Ok(_) => unparsed.push(zone),
            Err(_) => missing.push((zone, path)),
        }
    }

    assert_eq!(
        unresolved,
        vec![EVENT_ZONE_ID_ABSENT],
        "event DAT ids that no longer resolve"
    );
    assert!(
        root.resolve(event_dat_file_id(EVENT_DAT_ZONE_ID_MAX + 1))
            .is_err(),
        "this install carries event DATs above EVENT_DAT_ZONE_ID_MAX"
    );
    assert!(
        missing.is_empty(),
        "event DATs resolved but absent: {missing:?}"
    );
    assert!(
        unparsed.is_empty(),
        "event DATs that do not parse: {unparsed:?}"
    );
}
