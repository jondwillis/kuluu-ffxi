//! Install conformance: what `scripts/checks.sh install` runs once per client
//! install with `FFXI_DAT_PATH` set. Every test self-skips with a printed
//! reason when no install is present and hard-fails on a violation; a value
//! pinned per KNOWN_CLIENTS row prints instead of failing on an unmeasured row.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use ffxi_dat::archive::{open_test_install, BASE_ROM_INDEX, DAT_PATH_ENV};
use ffxi_dat::client_profile::{ItemBlockLayout, KNOWN_CLIENTS};
use ffxi_dat::dmsg::{EmoteTextDat, StringDat, MARKER_KEY_ITEM};
use ffxi_dat::event_dat::EventDat;
use ffxi_dat::event_locate::{event_dat_file_id, event_dat_zones};
use ffxi_dat::ftable::FTABLE_BYTES_PER_FILE_ID;
use ffxi_dat::item_dat::{ItemTable, ITEM_DAT_FILE_IDS};
use ffxi_dat::main_dll::MainDll;
use ffxi_dat::sysmes::{MesBasicDat, SysMesDat};
use ffxi_dat::ui_element::{find_ui_element_group, UI_SHEET_FILE_ID};
use ffxi_dat::vtable::VTable;
use ffxi_dat::zone_dat::{
    moghouse_model_to_mzb_file_id, string_dat_file_id, ZONE_DAT_TABLE, ZONE_DAT_THRESHOLD,
};
use ffxi_dat::{ChunkKind, DatRoot};
use ffxi_proto::fishing_messages::{kind, offset_text, FISHING_ZONE_OFFSET};
use ffxi_proto::login::{
    compare_client_ver_era, expansion_display, lobby_accepts_client_ver, VerLock, LSB_CLIENT_VER,
    LSB_DEFAULT_VER_LOCK,
};
use kuluu_session::event_dialog::{find_fishing_block, DialogSession, MAX_ERA_SKEW};

/// The install under test with its overlay search path cleared: conformance
/// measures the install's own files, not the Pivot overlays a private server
/// ships beside it.
fn install() -> Option<DatRoot> {
    let root = open_test_install()?;
    refuse_fallback(&root);
    root.set_overlays(Vec::new());
    eprintln!("install: {} [{}]", root.root().display(), root.profile());
    Some(root)
}

/// A per-install gate measures the install it was pointed at and nothing
/// else; this pins that `open_test_install` honoured `FFXI_DAT_PATH`.
fn refuse_fallback(root: &DatRoot) {
    let Some(requested) = std::env::var_os(DAT_PATH_ENV).map(PathBuf::from) else {
        return;
    };
    let same = matches!(
        (requested.canonicalize(), root.root().canonicalize()),
        (Ok(a), Ok(b)) if a == b
    );
    assert!(
        same,
        "{DAT_PATH_ENV} names {} but the opened install is {}",
        requested.display(),
        root.root().display()
    );
}

/// The value measured for this install's KNOWN_CLIENTS row, or `None` (with a
/// printed reason) on a row nobody has measured. A pin naming a row that no
/// longer exists is a silent skip waiting to happen, so every name is checked.
fn row_pin<T: Copy + std::fmt::Debug>(root: &DatRoot, what: &str, pins: &[(&str, T)]) -> Option<T> {
    for (name, _) in pins {
        assert!(
            KNOWN_CLIENTS.iter().any(|k| k.name == *name),
            "{what}: pin names {name}, which is not a KNOWN_CLIENTS row"
        );
    }
    let profile = root.profile().name();
    let pin = pins
        .iter()
        .find(|(name, _)| *name == profile)
        .map(|(_, v)| *v);
    if pin.is_none() {
        eprintln!("SKIP pin {what}: no measured value for client profile {profile}");
    }
    pin
}

fn read_file_id(root: &DatRoot, file_id: u32) -> Result<Vec<u8>, String> {
    let loc = root
        .resolve(file_id)
        .map_err(|e| format!("file id {file_id}: {e}"))?;
    let path = loc.path_under(root);
    std::fs::read(&path).map_err(|e| format!("file id {file_id} at {}: {e}", path.display()))
}

fn has_chunk(bytes: &[u8], kind: ChunkKind) -> bool {
    ffxi_dat::walk(bytes)
        .flatten()
        .any(|c| c.kind == kind as u8)
}

fn parse_string_dat(root: &DatRoot, zone: u16) -> Result<StringDat, String> {
    let file_id = string_dat_file_id(zone);
    let bytes = read_file_id(root, file_id)?;
    StringDat::parse(&bytes).map_err(|e| format!("zone {zone} file id {file_id}: {e}"))
}

#[test]
fn profile_is_a_measured_known_client_row() {
    let Some(root) = install() else {
        return;
    };
    let profile = root.profile();
    let Some(row) = profile.known else {
        panic!(
            "install is not a KNOWN_CLIENTS row; add one for FFXiMain.dll sha256 {} ({} bytes), \
             patch {}, item layout {}",
            profile.ffximain_sha256.as_deref().unwrap_or("missing"),
            profile.ffximain_len.unwrap_or(0),
            profile.patch_version.as_deref().unwrap_or("none"),
            profile
                .item_layout
                .map_or("unprobed", ItemBlockLayout::name),
        );
    };
    assert_eq!(
        profile.item_layout,
        Some(row.item_layout),
        "{}: probed item layout differs from the row",
        row.name
    );
    // The stamp is checkable only when the install carries one: the
    // patch.cfg stamp is the PlayOnline patch session's manifest
    // (ffxi-install/src/manifest.rs MANIFEST_FILE); SE's own patcher
    // stamps patch.txt, so a stamped-row install can lack it.
    if profile.patch_version.is_some() {
        assert_eq!(
            profile.patch_version.as_deref(),
            row.patch_version,
            "{}: patch stamp differs from the row",
            row.name
        );
    }
    for file_id in ITEM_DAT_FILE_IDS {
        let Ok(loc) = root.resolve(file_id) else {
            continue;
        };
        let path = loc.path_under(&root);
        if !path.is_file() {
            continue;
        }
        assert_eq!(
            ItemBlockLayout::probe_file(&path),
            Some(row.item_layout),
            "{}: file id {file_id} probes to a different layout than the row",
            row.name
        );
    }
}

// vendor/server/sql/item_basic.sql fire_crystal: @FLAG_MYSTERY_BOX | @FLAG_CANUSE.
const FIRE_CRYSTAL: u16 = 4096;
const FIRE_CRYSTAL_NAME: &str = "Fire Crystal";
const FIRE_CRYSTAL_FLAGS: u16 = 0x0204;
/// The DAT's own type byte for a crystal; LSB's @USABLE_TYPE lumps crystals
/// with medicines, so the two type spaces are not the same.
const CRYSTAL_ITEM_TYPE: u8 = 8;
// vendor/server/sql/item_basic.sql potion.
const POTION: u16 = 4112;
const USABLE_ITEM_TYPE: u8 = 7;
// vendor/server/sql/item_equipment.sql defending_ring (level, slots, jobs).
const DEFENDING_RING: u16 = 13566;
const DEFENDING_RING_LEVEL: u8 = 70;
const DEFENDING_RING_SLOTS: u16 = 0x6000;
/// Every playable race bit; the DAT's bit 0 is unused.
const ALL_RACES: u16 = 0x1FE;
/// LSB's `item_equipment.jobs` bit is `job - 1` while the DAT's bit is the
/// job id itself, so the DAT mask is LSB's shifted up by one.
const LSB_JOBS_TO_DAT_SHIFT: u32 = 1;
const DEFENDING_RING_JOBS: u32 = 4_194_303 << LSB_JOBS_TO_DAT_SHIFT;
// vendor/server/sql/item_equipment.sql cesti: level 1, slot 1, jobs 263667.
const CESTI: u16 = 16385;
const CESTI_LEVEL: u8 = 1;
const MAIN_SLOT: u16 = 1;
const CESTI_JOBS: u32 = 263_667 << LSB_JOBS_TO_DAT_SHIFT;
// vendor/server/sql/item_basic.sql gil.
const GIL: u16 = 0xFFFF;
const GIL_NAME: &str = "Gil";
/// Item icons are 32x32 on both KNOWN_CLIENTS rows (the icon's own Graphic header carries the size).
const ICON_SIDE: u32 = 32;

#[test]
fn items_decode_on_the_row_layout() {
    let Some(root) = install() else {
        return;
    };
    let Some(row) = root.profile().known else {
        eprintln!("SKIP items: unknown client profile");
        return;
    };
    let table = ItemTable::open_from_root(&root);
    assert!(
        table.skipped().is_empty(),
        "item DATs skipped: {:?}",
        table.skipped()
    );
    assert_eq!(table.layouts(), vec![row.item_layout], "{}", row.name);

    let fire = table.lookup(FIRE_CRYSTAL).expect("fire crystal block");
    assert_eq!(fire.name, FIRE_CRYSTAL_NAME);
    assert_eq!(fire.item_type, CRYSTAL_ITEM_TYPE, "fire crystal type");
    assert_eq!(fire.flags, FIRE_CRYSTAL_FLAGS, "fire crystal flags");

    let potion = table.lookup(POTION).expect("potion block");
    assert_eq!(potion.item_type, USABLE_ITEM_TYPE, "potion type");

    let ring = table.lookup(DEFENDING_RING).expect("defending ring block");
    assert_eq!(ring.level, DEFENDING_RING_LEVEL, "defending ring level");
    assert_eq!(ring.slot_mask, DEFENDING_RING_SLOTS, "defending ring slots");
    assert_eq!(ring.races_mask, ALL_RACES, "defending ring races");
    assert_eq!(ring.jobs_mask, DEFENDING_RING_JOBS, "defending ring jobs");

    let cesti = table.lookup(CESTI).expect("cesti block");
    assert_eq!(cesti.level, CESTI_LEVEL, "cesti level");
    assert_eq!(cesti.slot_mask, MAIN_SLOT, "cesti slot");
    assert_eq!(cesti.jobs_mask, CESTI_JOBS, "cesti jobs");

    let gil = table.lookup(GIL).expect("gil block");
    assert_eq!(gil.name, GIL_NAME);

    let icon = table.icon(FIRE_CRYSTAL).expect("fire crystal icon");
    assert_eq!((icon.width, icon.height), (ICON_SIDE, ICON_SIDE));
}

/// The eight playable look-race bytes, HumeM=1 through Galka=8
/// (research/xim/src/jsMain/kotlin/xim/poc/Model.kt RaceGenderConfig): the rows
/// every per-race DLL table must carry.
const PLAYABLE_RACES: std::ops::RangeInclusive<u8> = 1..=8;
/// Zone-keyed rows of the zone-map table, measured on both KNOWN_CLIENTS rows.
const MIN_MAPPED_ZONES: usize = 231;

#[test]
fn ffximain_dll_tables_resolve_for_every_playable_race_and_zone() {
    let Some(root) = install() else {
        return;
    };
    let dll = MainDll::load(root.root()).expect("FFXiMain.dll tables located");
    for race in PLAYABLE_RACES {
        let bases: [(&str, Option<u16>); 6] = [
            ("weapon", dll.base_weapon_skill_index(race)),
            ("dance", dll.base_dance_skill_index(race)),
            ("emote", dll.base_emote_index(race)),
            ("race_config", dll.base_race_config_index(race)),
            ("action_anim", dll.base_action_animation_index(race)),
            ("battle_anim", dll.base_battle_animation_index(race)),
        ];
        for (name, base) in bases {
            assert!(
                matches!(base, Some(b) if b != 0),
                "race {race}: base_{name} is {base:?}"
            );
        }
    }
    let counts = dll.zone_map_counts();
    assert!(
        counts.len() >= MIN_MAPPED_ZONES,
        "zone-map table keys {} zones, expected at least {MIN_MAPPED_ZONES}",
        counts.len()
    );
    let mut unresolved = Vec::new();
    for &zone in counts.keys() {
        for rec in dll.zone_maps(zone) {
            if root.resolve(rec.file_id).is_err() {
                unresolved.push((zone, rec.sub_zone_id, rec.file_id));
            }
        }
    }
    assert!(
        unresolved.is_empty(),
        "zone-map file ids that do not resolve: {unresolved:?}"
    );
    eprintln!("dll: {} zones carry maps", counts.len());
}

/// The base ROM plus ROM2..ROM9 ship with every install; a tenth (ROM10) is
/// what tells horizonxi-2023 apart from retail-2026-09.
const MIN_APPS: usize = 9;
/// File ids claimed by more than one ROM: horizonxi-2023's ROM10 re-ships 72
/// of the base ROM's files; retail-2026-09 has no ROM10 and no overlap.
const MULTI_CLAIM_PINS: &[(&str, usize)] = &[("horizonxi-2023", 72), ("retail-2026-09", 0)];

/// The per-ROM table pair as the install lays it out (`VTABLE.DAT`/`FTABLE.DAT`
/// for the base ROM, `ROMn/VTABLEn.DAT` otherwise), read independently of the
/// merge so the merge has something to be checked against.
fn app_vtable(root: &DatRoot, rom_dir: &str) -> (u8, VTable) {
    let (index, path) = match rom_dir.strip_prefix("ROM") {
        Some("") => (BASE_ROM_INDEX, root.root().join("VTABLE.DAT")),
        Some(n) => {
            let index: u8 = n.parse().expect("ROMn suffix is a number");
            (
                index,
                root.root().join(rom_dir).join(format!("VTABLE{n}.DAT")),
            )
        }
        None => panic!("unexpected app dir {rom_dir}"),
    };
    (index, VTable::load(&path).expect("app VTABLE loads"))
}

#[test]
fn archive_tables_agree_and_the_highest_rom_wins() {
    let Some(root) = install() else {
        return;
    };
    assert!(
        root.skipped_tables().is_empty(),
        "ROMs rejected at open: {:?}",
        root.skipped_tables()
    );
    let apps = root.app_summary();
    assert!(apps.len() >= MIN_APPS, "only {} apps: {apps:?}", apps.len());
    let base_len = root.file_id_count();
    for (rom_dir, vtable_len, ftable_len) in &apps {
        assert_eq!(*vtable_len, base_len, "{rom_dir}: VTABLE length");
        assert_eq!(
            *ftable_len, *vtable_len,
            "{rom_dir}: FTABLE entry count differs from the VTABLE's ({FTABLE_BYTES_PER_FILE_ID} bytes per id)"
        );
    }

    let tables: Vec<(u8, VTable)> = apps.iter().map(|(d, _, _)| app_vtable(&root, d)).collect();
    let mut multi = 0usize;
    let mut wrong_owner = Vec::new();
    for file_id in 0..base_len {
        let claims: Vec<u8> = tables
            .iter()
            .filter(|(index, v)| v.contains(file_id, *index))
            .map(|(index, _)| *index)
            .collect();
        if claims.len() < 2 {
            continue;
        }
        multi += 1;
        let highest = *claims.iter().max().expect("non-empty");
        let loc = root.resolve(file_id).expect("a claimed id resolves");
        let (owner, _) = app_vtable(&root, &loc.rom_dir);
        if owner != highest {
            wrong_owner.push((file_id, claims, loc.rom_dir));
        }
    }
    assert!(
        wrong_owner.is_empty(),
        "multi-claimed ids not owned by the highest ROM: {wrong_owner:?}"
    );
    eprintln!("archive: {} apps, {multi} multi-claimed ids", apps.len());
    if let Some(expected) = row_pin(&root, "multi-claim count", MULTI_CLAIM_PINS) {
        assert_eq!(multi, expected, "multi-claimed id count");
    }
}

/// vendor/server/sql/zone_settings.sql zone 286: "Crashes the client if
/// enabled", a placeholder with no zone DAT behind it.
const PLACEHOLDER_ZONE: u16 = 286;
/// Zone-DAT table shape on the vendored LSB pin: 255 zone ids below the
/// high-branch threshold, 44 at or above it, one of which is the placeholder.
const ZONES_BELOW_THRESHOLD: usize = 255;
const ZONES_AT_OR_ABOVE_THRESHOLD: usize = 44;
/// Mog House interior models with a verified MZB file id.
const MOGHOUSE_MODELS: usize = 16;

#[test]
fn zone_dats_resolve_with_an_mzb() {
    let Some(root) = install() else {
        return;
    };
    let mut below = 0usize;
    let mut at_or_above = 0usize;
    let mut failures = Vec::new();
    for &(zone, file_id) in ZONE_DAT_TABLE {
        if zone == PLACEHOLDER_ZONE {
            continue;
        }
        match read_file_id(&root, file_id) {
            Ok(bytes) if has_chunk(&bytes, ChunkKind::Mzb) => {
                if zone < ZONE_DAT_THRESHOLD {
                    below += 1;
                } else {
                    at_or_above += 1;
                }
            }
            Ok(_) => failures.push(format!("zone {zone} file id {file_id}: no MZB chunk")),
            Err(e) => failures.push(format!("zone {zone}: {e}")),
        }
    }
    assert!(failures.is_empty(), "zone DATs: {failures:#?}");
    assert!(
        below >= ZONES_BELOW_THRESHOLD,
        "{below} zones below {ZONE_DAT_THRESHOLD} carry an MZB, expected {ZONES_BELOW_THRESHOLD}"
    );
    assert!(
        at_or_above >= ZONES_AT_OR_ABOVE_THRESHOLD - 1,
        "{at_or_above} zones at or above {ZONE_DAT_THRESHOLD} carry an MZB, expected \
         {} (all but the placeholder)",
        ZONES_AT_OR_ABOVE_THRESHOLD - 1
    );
    eprintln!("zones: {below} below and {at_or_above} at/above the threshold carry an MZB");

    let moghouse: Vec<u32> = (0..=u16::MAX)
        .filter_map(moghouse_model_to_mzb_file_id)
        .collect();
    assert_eq!(moghouse.len(), MOGHOUSE_MODELS, "Mog House model table");
    for file_id in moghouse {
        let bytes = read_file_id(&root, file_id).expect("Mog House DAT");
        assert!(
            has_chunk(&bytes, ChunkKind::Mzb),
            "Mog House file id {file_id}: no MZB chunk"
        );
    }
}

/// Event DATs of the vendored zone set that must parse; measured on both rows
/// with a margin for a zone a patch relocates.
const MIN_PARSED_EVENT_ZONES: usize = 295;
const PASHHOW_MARSHLANDS: u16 = 109;
// vendor/server/sql/npc_list.sql Tahmasp (17224336), the outpost vendor.
const PASHHOW_TAHMASP: u32 = 0x0106_D290;
/// vendor/server/sql/npc_list.sql Conquest_Banner, the unique_no adjacent to
/// Tahmasp's: a renumbered HorizonXI npc_list sends the vendor event against
/// it, and the DAT has no block for it, so the event resolves on the sole owner.
const PASHHOW_CONQUEST_BANNER: u32 = 0x0106_D291;
// vendor/server/scripts/zones/Pashhow_Marshlands/npcs/Tahmasp.lua vendorEvent.
const OUTPOST_VENDOR_EVENT: u16 = 32756;

#[test]
fn event_dats_parse_and_pashhow_scripts_the_vendor_on_tahmasp() {
    let Some(root) = install() else {
        return;
    };
    let mut parsed = 0usize;
    let mut missing = Vec::new();
    let mut unparsed = Vec::new();
    let zones = event_dat_zones(&root);
    for &zone in &zones {
        let loc = root
            .resolve(event_dat_file_id(zone))
            .expect("listed zone locates");
        let path = loc.path_under(&root);
        let Ok(bytes) = std::fs::read(&path) else {
            missing.push((zone, path));
            continue;
        };
        match EventDat::parse(&bytes) {
            Ok(_) => parsed += 1,
            Err(e) => unparsed.push((zone, e)),
        }
    }
    assert!(missing.is_empty(), "event DATs missing: {missing:?}");
    assert!(
        parsed >= MIN_PARSED_EVENT_ZONES,
        "{parsed} of {} event DATs parse, expected at least {MIN_PARSED_EVENT_ZONES}: {unparsed:?}",
        zones.len()
    );
    eprintln!("event: {parsed}/{} zone event DATs parse", zones.len());

    let loc = root
        .resolve(event_dat_file_id(PASHHOW_MARSHLANDS))
        .expect("Pashhow event DAT");
    let bytes = std::fs::read(loc.path_under(&root)).expect("Pashhow event DAT readable");
    let dat = EventDat::parse(&bytes).expect("Pashhow event DAT parses");
    let tahmasp = dat
        .block_for_actor(PASHHOW_TAHMASP)
        .expect("Tahmasp has an event block");
    assert!(
        tahmasp.event_entry_exact(OUTPOST_VENDOR_EVENT).is_some(),
        "Tahmasp's block does not script the outpost vendor event"
    );
    assert!(
        dat.block_for_actor(PASHHOW_CONQUEST_BANNER).is_none(),
        "the conquest banner has a block of its own"
    );
}

/// Zone dialog tables of the LSB zone set that must parse; measured on
/// both rows with a margin for a zone a patch relocates.
const MIN_PARSED_STRING_ZONES: usize = 294;
const SOUTHERN_SAN_DORIA: u16 = 230;
const KEYITEM_OBTAINED_PREFIX: &str = "Obtained key item:";
/// Southern San d'Oria's KEYITEM_OBTAINED entry per row. LSB text ids are
/// identity DAT indexes for the client era LSB was synced to: the vendored
/// vendor/server/scripts/zones/Southern_San_dOria/IDs.lua (CLIENT_VER
/// 30260904_1) pins 6442, which retail-2026-09 matches exactly;
/// horizonxi-2023 sits 5 below it.
const KEYITEM_OBTAINED_PINS: &[(&str, usize)] =
    &[("horizonxi-2023", 6437), ("retail-2026-09", 6442)];
/// The US sheet's frame group; the JP sheet's is `menu    frames  `. Measured
/// on both rows.
const FRAMES_US: &str = "menu    framesus";

#[test]
fn dialog_tables_parse_and_the_fixed_tables_open() {
    let Some(root) = install() else {
        return;
    };
    let mut parsed = 0usize;
    let mut failures = Vec::new();
    for &(zone, _) in ZONE_DAT_TABLE {
        match parse_string_dat(&root, zone) {
            Ok(_) => parsed += 1,
            Err(e) => failures.push(e),
        }
    }
    assert!(
        parsed >= MIN_PARSED_STRING_ZONES,
        "{parsed} of {} zone dialog DATs parse, expected at least \
         {MIN_PARSED_STRING_ZONES}: {failures:?}",
        ZONE_DAT_TABLE.len()
    );
    eprintln!(
        "dialog: {parsed}/{} zone dialog DATs parse",
        ZONE_DAT_TABLE.len()
    );

    let dat = parse_string_dat(&root, SOUTHERN_SAN_DORIA).expect("Southern San d'Oria dialog");
    let index = dat
        .first_entry_starting_with(KEYITEM_OBTAINED_PREFIX)
        .unwrap_or_else(|| {
            panic!(
                "no {KEYITEM_OBTAINED_PREFIX:?} entry in {} entries",
                dat.len()
            )
        });
    let text = dat.text(index).expect("entry decodes");
    let marker = format!("{{{MARKER_KEY_ITEM}:0}}");
    assert!(text.contains(&marker), "expected {marker} in {text:?}");
    eprintln!("dialog: zone {SOUTHERN_SAN_DORIA} KEYITEM_OBTAINED at {index}");
    if let Some(expected) = row_pin(&root, "KEYITEM_OBTAINED index", KEYITEM_OBTAINED_PINS) {
        assert_eq!(
            index, expected,
            "zone {SOUTHERN_SAN_DORIA} KEYITEM_OBTAINED index"
        );
    }

    assert!(SysMesDat::open(&root).is_some(), "system-message table");
    assert!(
        MesBasicDat::open(&root).is_some(),
        "basic-message table - without it every battle line falls back to the scraped msg_basic wording"
    );
    assert!(EmoteTextDat::open(&root).is_some(), "emote text table");
    let sheet = read_file_id(&root, UI_SHEET_FILE_ID).expect("UI element sheet");
    assert!(
        find_ui_element_group(&sheet, FRAMES_US).is_some(),
        "{FRAMES_US:?} group in file id {UI_SHEET_FILE_ID}"
    );
}

const PORT_SAN_DORIA: u16 = 232;
/// Port San d'Oria's installed fishing base per row; the vendored LSB pin's
/// `FISHING_MESSAGE_OFFSET` for the zone is 7264.
const PORT_SAN_DORIA_FISHING_BASE_PINS: &[(&str, u16)] =
    &[("horizonxi-2023", 7255), ("retail-2026-09", 7268)];
/// Zones whose NOROD line is read back through the session's chat path:
/// West Ronfaure, Lower Jeuno, Mog Garden (vendor/server/sql/zone_settings.sql).
const NOROD_CHAT_ZONES: [u16; 3] = [100, 245, 280];
const CONFORMANCE_PLAYER_NAME: &str = "Conformance";

/// The shared block's stacked-item line, verbatim up to its first parameter.
/// LSB sends it as `ITEM_OBTAINED + 9`
/// (vendor/server/scripts/globals/npc_util.lua giveItem).
const ITEM_OBTAINED_PLURAL_PREFIX: &str = "You obtain {Num:1} {Item:0}";
/// `{Item:0}` + `{Num:1}` as `StringDat::param_slots` reports them.
const ITEM_AND_COUNT_SLOTS: u32 = (1 << 0) | (1 << 1);
/// The dispense that exposed the skew: a hatchling shield handing over a stack
/// of sairui-ran (vendor/server/sql/item_basic.sql).
const SAIRUI_RAN_ITEM_NO: i32 = 1188;
const SAIRUI_RAN_DISPENSED: i32 = 8;

#[test]
fn fishing_blocks_sit_within_the_era_skew_of_the_lsb_pin() {
    let Some(root) = install() else {
        return;
    };
    let root = Arc::new(root);
    let mut bases: BTreeMap<u16, u16> = BTreeMap::new();
    let mut failures = Vec::new();
    for &(zone, pin) in FISHING_ZONE_OFFSET {
        let dat = match parse_string_dat(&root, zone) {
            Ok(dat) => dat,
            Err(e) => {
                eprintln!("era: zone {zone} skipped: {e}");
                continue;
            }
        };
        let Some(base) = find_fishing_block(&dat) else {
            failures.push(format!(
                "zone {zone}: no fishing block landmarks (pin {pin})"
            ));
            continue;
        };
        let skew = i32::from(base) - i32::from(pin);
        eprintln!("era: zone {zone} pin {pin} install {base} skew {skew:+}");
        if base.abs_diff(pin) > MAX_ERA_SKEW {
            failures.push(format!(
                "zone {zone}: installed base {base} is {skew:+} from pin {pin}, beyond {MAX_ERA_SKEW}"
            ));
        }
        bases.insert(zone, base);
    }
    assert!(failures.is_empty(), "era: {failures:#?}");
    assert!(
        !bases.is_empty(),
        "no fishing zone had a parseable dialog DAT"
    );

    let port = bases
        .get(&PORT_SAN_DORIA)
        .copied()
        .expect("Port San d'Oria fishing block");
    if let Some(expected) = row_pin(
        &root,
        "Port San d'Oria fishing base",
        PORT_SAN_DORIA_FISHING_BASE_PINS,
    ) {
        assert_eq!(port, expected, "zone {PORT_SAN_DORIA} fishing base");
    }

    let norod = offset_text(kind::NOROD).expect("NOROD text scraped");
    let mut session = DialogSession::new(Some(Arc::clone(&root)), CONFORMANCE_PLAYER_NAME.into());
    for zone in NOROD_CHAT_ZONES {
        let base = bases
            .get(&zone)
            .copied()
            .unwrap_or_else(|| panic!("zone {zone} has no fishing block"));
        let index = usize::from(base) + usize::from(kind::NOROD);
        let line = session
            .zone_chat_text(zone, index as u16, &[])
            .unwrap_or_else(|| panic!("zone {zone} entry {index} is not a chat line"));
        assert!(
            line.starts_with(norod),
            "zone {zone} entry {index}: {line:?} is not the NOROD line"
        );
    }
}

/// The shared system-message block's item-obtained line reads the item id and
/// the stack count; the lines it sits between read nothing. That contrast is
/// what lets a zone message name its own line when the server numbers the
/// dialog table from a different client era, so it has to hold on the real DAT
/// — the decoder has to see through the `{Auto:N}` terminators and inline tags
/// these entries carry. The server's own stack message for a sairui-ran
/// landed on the parameterless neighbour on both installs in hand, so feeding
/// one back in has to resolve to the obtained line rather than print the
/// neighbour.
#[test]
fn the_item_obtained_line_is_the_only_shape_its_neighbours_are_not() {
    let Some(root) = install() else {
        return;
    };
    let root = Arc::new(root);
    for zone in NOROD_CHAT_ZONES {
        let dat = match parse_string_dat(&root, zone) {
            Ok(dat) => dat,
            Err(e) => {
                eprintln!("shape: zone {zone} skipped: {e}");
                continue;
            }
        };
        let obtain = (0..dat.len())
            .find(|&i| {
                dat.text(i)
                    .is_some_and(|t| t.starts_with(ITEM_OBTAINED_PLURAL_PREFIX))
            })
            .unwrap_or_else(|| panic!("zone {zone} has no {ITEM_OBTAINED_PLURAL_PREFIX:?} line"));
        assert_eq!(
            dat.param_slots(obtain),
            Some(ITEM_AND_COUNT_SLOTS),
            "zone {zone} entry {obtain}: {:?}",
            dat.text(obtain)
        );

        let wire = (obtain + 1..obtain + 1 + usize::from(MAX_ERA_SKEW))
            .find(|&i| dat.param_slots(i) == Some(0) && dat.menu(i).is_none())
            .unwrap_or_else(|| panic!("zone {zone}: no parameterless line after {obtain}"));
        let mut session =
            DialogSession::new(Some(Arc::clone(&root)), CONFORMANCE_PLAYER_NAME.into());
        let line = session
            .zone_chat_text(
                zone,
                wire as u16,
                &[SAIRUI_RAN_ITEM_NO, SAIRUI_RAN_DISPENSED],
            )
            .unwrap_or_else(|| panic!("zone {zone} entry {wire} resolved to nothing"));
        assert!(
            line.starts_with(ITEM_OBTAINED_PLURAL_PREFIX),
            "zone {zone}: wire index {wire} printed {line:?}, not the obtained line at {obtain}"
        );
    }
}

/// Print-only: whether the vendored LSB pin's lobby would admit this install's
/// patch stamp. A mismatch is a dev-stack configuration fact, not an install
/// defect.
#[test]
fn lsb_pin_era_against_the_install_patch_stamp() {
    let Some(root) = install() else {
        return;
    };
    let Some(stamp) = root.profile().patch_version.clone() else {
        eprintln!("WARN lsb-era: install has no patch stamp; LSB pins CLIENT_VER {LSB_CLIENT_VER}");
        return;
    };
    let lock = VerLock::from_setting(LSB_DEFAULT_VER_LOCK);
    let era = compare_client_ver_era(&stamp, LSB_CLIENT_VER);
    let accepted = lobby_accepts_client_ver(&stamp, LSB_CLIENT_VER, lock);
    eprintln!(
        "WARN lsb-era: install {stamp} is {era:?} the LSB pin {LSB_CLIENT_VER}; \
         lobby under {lock:?} {}",
        if accepted { "accepts it" } else { "rejects it" }
    );
}

/// The C2S 0x26 excode_client this install's ROM inventory yields
/// (research/XiPackets/lobby/C2S_0x0026_RequestLobbyLogin.md excode_client).
/// Pinned only over the bits vendor/server/src/login/login_helpers.h
/// EXPANSION_DISPLAY names: a private server's extra ROM sets a bit above them
/// that is reported, not judged.
#[test]
fn excode_client_matches_the_rom_inventory_pinned_for_this_row() {
    let Some(root) = install() else {
        return;
    };
    let Some(pinned) = row_pin(
        &root,
        "excode_client",
        &[
            ("horizonxi-2023", expansion_display::ALL_KNOWN),
            ("retail-2019-base", expansion_display::ALL_KNOWN),
            ("retail-2026-09", expansion_display::ALL_KNOWN),
        ],
    ) else {
        return;
    };
    let derived = root.excode_client();
    assert_eq!(
        derived & expansion_display::ALL_KNOWN,
        pinned,
        "{}: derived {derived:#06x}",
        root.root().display()
    );
    let extra = derived & !expansion_display::ALL_KNOWN;
    if extra != 0 {
        eprintln!(
            "WARN excode_client: {} sets {extra:#06x} above the expansions LSB names",
            root.root().display()
        );
    }
}
