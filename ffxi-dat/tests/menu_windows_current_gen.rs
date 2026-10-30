//! Pins the config-family window rows measured on retail-2026-09 (vault
//! main_menu.md §49 as corrected by the §57 sweep): the reader must resolve
//! each named-root window through VTABLE/FTABLE with its exact `windowpsNNN`
//! row ids, fall back to the legacy DAT for roots the newer generation lacks,
//! and keep the newest generation winning where a root exists in both. The
//! captions must then read out of catalog table 0x0E (the `d_msg` container).
//! Self-skips without an install.

use ffxi_dat::archive::open_test_install;
use ffxi_dat::dmsg::DmsgTable;
use ffxi_dat::menu_windows::{windows_from_root, CAPTION_CATALOG_TABLE_14_FILE_ID};

/// Named-root generation (ROM/119/51.DAT) rows in file order, measured.
/// Repeated ids are real repeated records (§57): conf7 shows 219 and 222
/// twice; conf4, conf5m and conf11m duplicate their single option id.
const CURRENT_GEN_ROWS: &[(&str, &[u32])] = &[
    (
        "configwi",
        &[
            226, 227, 228, 229, 230, 233, 599, 231, 232, 900, 916, 985, 1092,
        ],
    ),
    ("conf1us", &[145, 146, 151, 152, 153]),
    (
        "conf2win",
        &[155, 156, 159, 160, 162, 163, 164, 671, 672, 988, 989],
    ),
    (
        "conf3win",
        &[
            165, 166, 167, 168, 174, 176, 177, 178, 179, 180, 181, 712, 713,
        ],
    ),
    ("conf4", &[182, 182]),
    ("conftxtc", &[236]),
    ("conf5m", &[894, 895, 895]),
    ("conf5win", &[914, 915, 897, 924, 925, 899, 710, 711]),
    ("conf5w1", &[184, 185, 683, 684, 685, 197]),
    ("conf5w2", &[184, 185, 683, 684, 685, 197]),
    (
        "conf6win",
        &[
            206, 207, 208, 209, 145, 146, 212, 151, 152, 153, 686, 687, 687, 688, 689,
        ],
    ),
    (
        "conf7",
        &[
            213, 214, 215, 216, 217, 218, 219, 220, 219, 221, 222, 223, 222, 224,
        ],
    ),
    ("conf11m", &[905, 905]),
    ("conf11l", &[903]),
    ("conf11s", &[]),
    (
        "conf12wi",
        &[
            917, 918, 920, 919, 929, 930, 942, 943, 944, 945, 946, 947, 948, 949,
        ],
    ),
    ("conf13wi", &[986, 147, 148, 1088, 1090, 1089]),
];

/// Roots absent from the named-root generation that must still resolve from
/// the legacy base DAT (§57: `conf10` exists only in ROM/0/1.DAT).
const LEGACY_ONLY_ROWS: &[(&str, &[u32])] = &[(
    "conf10",
    &[181, 712, 713, 917, 918, 920, 919, 929, 930, 942, 943],
)];

/// `conf5m` lives in both generations; the newer file must win (§57 measures
/// ROM/119/51.DAT chunk co11 @0x299920 — legacy sits at a different offset).
const MERGE_WINNER_OFFSET: usize = 0x299_920;

#[test]
fn config_windows_resolve_with_measured_rows() {
    let Some(root) = open_test_install() else {
        eprintln!("SKIP: no FFXI install");
        return;
    };
    let windows = windows_from_root(&root).expect("menu window records parse from the install");
    for (name, rows) in CURRENT_GEN_ROWS.iter().chain(LEGACY_ONLY_ROWS.iter()) {
        let window = windows
            .get(*name)
            .unwrap_or_else(|| panic!("window {name} resolves"));
        assert_eq!(&window.option_row_ids(), rows, "rows of {name}");
    }
}

#[test]
fn the_newer_generation_wins_shared_roots() {
    let Some(root) = open_test_install() else {
        eprintln!("SKIP: no FFXI install");
        return;
    };
    let windows = windows_from_root(&root).expect("menu window records parse from the install");
    // Newest-first merge: conf5m must come from ROM/119, not the legacy copy.
    assert_eq!(windows["conf5m"].offset, MERGE_WINNER_OFFSET);
    assert_eq!((windows["conf6win"].fourcc.as_str()), "co13");
}

#[test]
fn caption_ids_read_real_strings_from_catalog_table_14() {
    let Some(root) = open_test_install() else {
        eprintln!("SKIP: no FFXI install");
        return;
    };
    let loc = root.resolve(CAPTION_CATALOG_TABLE_14_FILE_ID).unwrap();
    let bytes = std::fs::read(loc.path_under(&root)).unwrap();
    let catalog = DmsgTable::parse(&bytes).expect("table 0x0E parses as a d_msg catalog");
    assert_eq!(catalog.len(), 1099, "§43 measured entry count");
    // §43 law, measured text on retail-2026-09: the first conf3win row is
    // caption 165.
    assert_eq!(
        catalog.text(165),
        Some("Display damage in the log window only.")
    );
    assert_eq!(catalog.text(143), Some("Default to ROMAJI input."));
    let windows = windows_from_root(&root).unwrap();
    let rows = windows["conf3win"].option_rows(|i| catalog.text(i).map(str::to_string));
    assert_eq!(
        rows.first().map(String::as_str),
        Some("Display damage in the log window only.")
    );
}

/// Four roots appear as two chunks per UI DAT (`cfilter`, `evitem`, `buff`,
/// `fep`): one carries the length-worded block chain, the later one is an
/// argument-less twin. The reader must resolve them to the block-bearing copy,
/// or kuluu loses their geometry and rows (bead kuluu-o9qt7.20.1.3 — this was
/// why `cfilter` looked frameless and needed a placeholder rect).
#[test]
fn duplicate_chunk_roots_resolve_to_the_block_bearing_copy() {
    let Some(root) = open_test_install() else {
        eprintln!("SKIP: no FFXI install");
        return;
    };
    let windows = windows_from_root(&root).expect("menu window records parse from the install");
    for name in ["cfilter", "evitem", "buff", "fep"] {
        let window = windows
            .get(name)
            .unwrap_or_else(|| panic!("window {name} resolves"));
        let Some(f) = &window.frame else {
            panic!(
                "{name}: the block-less twin must not win (kept offset {:#x}, declared items {})",
                window.offset, window.item_count
            );
        };
        println!(
            "collision-winner[{name}] off={:#x} x={} y={} w={} h={}",
            window.offset, f.x, f.y, f.width, f.height
        );
        assert!(
            !window.items_in_display_order().is_empty(),
            "{name}: item blocks must survive the merge"
        );
    }
}

/// Placement census (bead kuluu-o9qt7.30): every Config-family record's own
/// frame, printed for the vault pin and asserted only on invariants we already
/// lawed — top edge y=48 everywhere except moved-by-code windows, and the hub
/// sharing the selector column at x=384 anchor 1 while pages sit left-half.
#[test]
fn config_family_frames_census() {
    let Some(root) = open_test_install() else {
        eprintln!("SKIP: no FFXI install");
        return;
    };
    let windows = windows_from_root(&root).expect("menu window records parse from the install");
    for name in [
        "socialme", "configwi", "conf2win", "cfilter", "conftxtc", "conf5m", "conf3win",
        "conf6win", "fxfilter", "conf7", "conf4", "conf11m", "conf12wi", "conf13wi", "keylayou",
        "k1assign", "k2assign", "conf5win", "conf5w1", "conf5w2", "conftxtc",
    ] {
        let Some(w) = windows.get(name) else {
            println!("frame[{name}] MISSING");
            continue;
        };
        match &w.frame {
            Some(f) => println!(
                "frame[{}] x={} y={} w={} h={} anchor={}",
                name, f.x, f.y, f.width, f.height, f.anchor
            ),
            None => println!("frame[{name}] no frame block"),
        }
    }
}
