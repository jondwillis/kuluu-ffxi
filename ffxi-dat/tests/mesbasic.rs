//! Composes battle-log lines out of a real install's basic-message table, so
//! the wording is pinned to the client's own data rather than to English
//! literals in our source. The two measured rows hold 1024 index-stable entries
//! that differ in wording at eleven of them, so every expectation is keyed by
//! KNOWN_CLIENTS row. Self-skips without an install.

use ffxi_dat::client_profile::KNOWN_CLIENTS;
use ffxi_dat::sysmes::{MesBasicDat, MesBasicResource, MesBasicResourceRef, SysMesParams};
use ffxi_dat::sysmes::{MES_BASIC_FILE_ID, PARAM_SLOTS};
use ffxi_dat::DatRoot;

/// Entry count measured on both rows — the wording moved between eras, the
/// indexing did not.
const ENTRIES: usize = 1024;

/// Message-parameter slots a battle packet fills, mirroring what the session
/// puts in them: the action id, then the result's value.
const ACTION_ID: usize = 0;
const MAIN_VALUE: usize = 1;

const CASTER: &str = "Daisy";
const TARGET: &str = "Rock Lizard";

const HORIZON: &str = "horizonxi-2023";
const RETAIL: &str = "retail-2026-09";

struct Case {
    /// The battle packet's `MesNo`.
    index: usize,
    numbers: [i64; PARAM_SLOTS],
    /// Name the caller resolves for whatever resource the entry asks for.
    resource: &'static str,
    /// Composed plain text per KNOWN_CLIENTS row.
    expected: &'static [(&'static str, &'static str)],
}

fn numbers(pairs: &[(usize, i64)]) -> [i64; PARAM_SLOTS] {
    let mut out = [0; PARAM_SLOTS];
    for &(slot, value) in pairs {
        out[slot] = value;
    }
    out
}

/// Four ids the session carries hand-pinned wording for, and the four
/// era-divergent indices whose wording the two rows disagree on; the skill-up
/// tenths diverge because the horizon table has no fractional skill-up and
/// spells them behind a literal "0.".
fn cases() -> Vec<Case> {
    vec![
        Case {
            index: 100,
            numbers: numbers(&[(ACTION_ID, 39)]),
            resource: "Boost",
            expected: &[
                (HORIZON, "Daisy uses Boost."),
                (RETAIL, "Daisy uses Boost."),
            ],
        },
        Case {
            index: 136,
            numbers: numbers(&[(ACTION_ID, 39)]),
            resource: "Charm",
            expected: &[
                (
                    HORIZON,
                    "Daisy uses Charm.\nThe Rock Lizard is now under Daisy's control.",
                ),
                (
                    RETAIL,
                    "Daisy uses Charm.\nThe Rock Lizard is now under Daisy's control.",
                ),
            ],
        },
        Case {
            index: 317,
            numbers: numbers(&[(ACTION_ID, 39), (MAIN_VALUE, 42)]),
            resource: "Boost",
            expected: &[
                (
                    HORIZON,
                    "Daisy uses Boost.\nThe Rock Lizard takes 42 points of damage.",
                ),
                (
                    RETAIL,
                    "Daisy uses Boost.\nThe Rock Lizard takes 42 points of damage.",
                ),
            ],
        },
        Case {
            index: 565,
            numbers: numbers(&[(ACTION_ID, 1200)]),
            resource: "",
            expected: &[
                (HORIZON, "Rock Lizard obtains 1,200 gil."),
                (RETAIL, "Rock Lizard obtains 1,200 gil."),
            ],
        },
        Case {
            index: 38,
            numbers: numbers(&[(ACTION_ID, 2), (MAIN_VALUE, 15)]),
            resource: "Dagger",
            expected: &[
                (HORIZON, "The Rock Lizard's Dagger skill rises 0.15 points."),
                (RETAIL, "The Rock Lizard's Dagger skill rises 1.5 points."),
            ],
        },
        Case {
            index: 537,
            numbers: numbers(&[(MAIN_VALUE, 100)]),
            resource: "",
            expected: &[
                (HORIZON, "The Rock Lizard's TP is increased to 100%."),
                (RETAIL, "The Rock Lizard's TP is increased to 100."),
            ],
        },
        Case {
            index: 679,
            numbers: numbers(&[(MAIN_VALUE, 5)]),
            resource: "",
            expected: &[
                (HORIZON, "Daisy will return to the Feretory in: 5."),
                (RETAIL, "Daisy will be returned to the entrance in 5."),
            ],
        },
        Case {
            index: 795,
            numbers: numbers(&[(ACTION_ID, 1), (MAIN_VALUE, 7)]),
            resource: "",
            expected: &[
                (HORIZON, "You receive 1 deeds of heroism, for a total of 7!"),
                (RETAIL, "You receive 1 deed of heroism, for a total of 7!"),
            ],
        },
    ]
}

fn install() -> Option<DatRoot> {
    let root = ffxi_dat::archive::open_test_install();
    if root.is_none() {
        eprintln!("SKIP: no FFXI install");
    }
    root
}

fn open() -> Option<(DatRoot, MesBasicDat)> {
    let root = install()?;
    match MesBasicDat::open(&root) {
        Some(dat) => Some((root, dat)),
        None => {
            eprintln!(
                "SKIP: install has no usable basic-message table at file id {MES_BASIC_FILE_ID}"
            );
            None
        }
    }
}

fn compose(dat: &MesBasicDat, case: &Case) -> Option<String> {
    let refs: Vec<MesBasicResourceRef> = dat.resource_refs(case.index);
    let mut names = [None; PARAM_SLOTS];
    for r in &refs {
        names[r.slot] = Some(case.resource);
    }
    let params = SysMesParams {
        numbers: case.numbers,
        names,
        caster_name: Some(CASTER),
        caster_article: false,
        target_name: Some(TARGET),
        target_article: true,
        ..Default::default()
    };
    dat.message(case.index, &params).map(|l| l.to_plain())
}

#[test]
fn composed_lines_match_the_installs_own_wording() {
    let Some((root, dat)) = open() else { return };
    let profile = root.profile().name();
    for (name, _) in cases().iter().flat_map(|c| c.expected) {
        assert!(
            KNOWN_CLIENTS.iter().any(|k| k.name == *name),
            "expectation names {name}, which is not a KNOWN_CLIENTS row"
        );
    }
    for case in cases() {
        let composed = compose(&dat, &case)
            .unwrap_or_else(|| panic!("entry {} composes on {profile}", case.index));
        match case.expected.iter().find(|(row, _)| *row == profile) {
            Some((_, expected)) => assert_eq!(&composed, expected, "entry {}", case.index),
            None => eprintln!(
                "SKIP pin entry {}: unmeasured row {profile} -> {composed:?}",
                case.index
            ),
        }
    }
}

#[test]
fn the_table_is_index_stable_across_eras() {
    let Some((root, dat)) = open() else { return };
    assert_eq!(
        dat.len(),
        ENTRIES,
        "{}: basic-message entry count moved, so every pinned index is suspect",
        root.profile().name()
    );
}

/// The resource an entry names comes from the entry, not from a table in our
/// source: a "readies" line reads the id out of the value slot, the matching
/// "uses" line out of the action slot.
#[test]
fn resource_refs_report_the_slot_the_entry_reads() {
    let Some((_, dat)) = open() else { return };
    assert_eq!(
        dat.resource_refs(43),
        vec![MesBasicResourceRef {
            kind: MesBasicResource::WeaponSkill,
            slot: MAIN_VALUE,
        }],
        "entry 43 readies a weapon skill"
    );
    assert_eq!(
        dat.resource_refs(185),
        vec![MesBasicResourceRef {
            kind: MesBasicResource::WeaponSkill,
            slot: ACTION_ID,
        }],
        "entry 185 uses one"
    );
    assert_eq!(
        dat.resource_refs(2),
        vec![MesBasicResourceRef {
            kind: MesBasicResource::Spell,
            slot: ACTION_ID,
        }],
        "entry 2 casts a spell"
    );
    assert_eq!(
        dat.resource_refs(100),
        vec![MesBasicResourceRef {
            kind: MesBasicResource::JobAbility,
            slot: ACTION_ID,
        }],
        "entry 100 uses a job ability"
    );
}

/// An entry the composer cannot fully render must report nothing rather than a
/// line with a hole in it, so a caller with a second wording source can use it.
#[test]
fn an_unrenderable_entry_composes_to_nothing() {
    let Some((_, dat)) = open() else { return };
    let params = SysMesParams {
        caster_name: Some(CASTER),
        target_name: Some(TARGET),
        ..Default::default()
    };
    assert!(
        dat.message(100, &params).is_none(),
        "entry 100 names an ability the caller did not resolve"
    );
    assert!(
        dat.message(ENTRIES - 1, &params).is_none(),
        "the table's trailing entries are empty"
    );
}
