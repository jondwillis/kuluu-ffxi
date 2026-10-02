//! The effect-DAT instruction census (bead kuluu-q6ui). Walks the weaponskill / spell /
//! job-ability / mob-skill / pet-ability effect DATs an install actually ships, through the same
//! `action_dat_file_id` dispatcher the runtime uses, and prints what resolves and which
//! instructions the parsers understand. Self-skips without an install
//! (`DatRoot::from_env_or_default`), so point `FFXI_DAT_PATH` at each install in turn.
//!
//! The handled sets are derived rather than listed here: scheduler opcodes by probing
//! `StageKind::from_stage` over the opcode/length space, generator opcodes by the
//! decoded/dropped outcome the parsers report per block.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ffxi_dat::main_dll::MainDll;
use ffxi_dat::particle_gen::{GeneratorOpcodeOutcome, GeneratorSection};
use ffxi_dat::scheduler::{is_structural_opcode, StageKind, STAGE_WORDS_RANGE};
use kuluu_render::look_resolver::PC_LOOK_RACES;
use kuluu_render::scheduler_runtime::{action_dat_file_id, parse_action_bytes_reporting};

/// vendor/server/src/map/enums/action/category.h `ActionCategory`: the finish categories that
/// carry a completed skill, which is what `action_dat_file_id` keys the effect DAT by.
const CATEGORY_WEAPON_SKILL: u8 = 3;
const CATEGORY_MAGIC: u8 = 4;
const CATEGORY_JOB_ABILITY: u8 = 6;
const CATEGORY_MOB_SKILL: u8 = 11;
const CATEGORY_PET_SKILL: u8 = 13;

/// A table's failing rows are listed to make one concrete, not to enumerate thousands of them;
/// the counts above the list carry the magnitude.
const MAX_EXAMPLES: usize = 12;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Resolution {
    Ok,
    NoFileId,
    Unresolvable,
    Unreadable,
    NoRoutines,
    /// Parsed, but no routine named `main`: `apply_action_dispatch` falls back to the first
    /// scheduler in the file, so this is a degraded hit rather than a failure.
    NoMain,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum StageBucket {
    Handled,
    /// The parser acts on the opcode structurally (end of section, random block, control flow);
    /// `StageKind::Unknown` there is the design, not a gap.
    Structural,
    /// `from_stage` knows the opcode at some longer stage length, but this stage is too short —
    /// including the `stage_bytes >= 12` id gate in `Scheduler::parse`.
    ShortStageSuppressed,
    Unhandled,
}

#[derive(Default)]
struct OpcodeTally {
    occurrences: usize,
    files: BTreeSet<u32>,
    example: Option<(u32, [u8; 4])>,
}

impl OpcodeTally {
    fn record(&mut self, file_id: u32, name: [u8; 4]) {
        self.occurrences += 1;
        self.files.insert(file_id);
        self.example.get_or_insert((file_id, name));
    }
}

fn fourcc(name: [u8; 4]) -> String {
    name.iter()
        .map(|&b| if b.is_ascii_graphic() { b as char } else { '.' })
        .collect()
}

/// Every (opcode, length) the classifier answers with a real kind. This is the handled set,
/// read out of the match arms by asking them, so it stays in step with the classifier.
fn probe_handled() -> BTreeMap<u8, BTreeSet<usize>> {
    let mut handled: BTreeMap<u8, BTreeSet<usize>> = BTreeMap::new();
    for raw in 0..=u8::MAX {
        for words in STAGE_WORDS_RANGE {
            if StageKind::from_stage(raw, words) != StageKind::Unknown {
                handled.entry(raw).or_default().insert(words);
            }
        }
    }
    handled
}

/// (category, action id, look race where the id is race-keyed, resolved effect DAT file id)
type CorpusRow = (u8, u32, Option<u8>, Option<u32>);

struct Corpus {
    rows: Vec<CorpusRow>,
}

/// Categories 11 and 13 share one table and one resolver, so the mob-skill rows
/// stand for both; a pet skill resolves to the same file its animation index names.
fn build_corpus(dll: &MainDll) -> Corpus {
    let mut rows = Vec::new();
    for &(spell_id, animation) in ffxi_vocab::action_anim::SPELL_ANIMATION {
        rows.push((
            CATEGORY_MAGIC,
            u32::from(spell_id),
            None,
            action_dat_file_id(
                u32::from(spell_id),
                Some(animation),
                CATEGORY_MAGIC,
                None,
                Some(dll),
            ),
        ));
    }
    for &(ability_id, animation) in ffxi_vocab::action_anim::ABILITY_ANIMATION {
        rows.push((
            CATEGORY_JOB_ABILITY,
            u32::from(ability_id),
            None,
            action_dat_file_id(
                u32::from(ability_id),
                Some(animation),
                CATEGORY_JOB_ABILITY,
                None,
                Some(dll),
            ),
        ));
    }
    for &(skill_id, animation) in ffxi_vocab::action_anim::MOB_SKILL_ANIMATION {
        for category in [CATEGORY_MOB_SKILL, CATEGORY_PET_SKILL] {
            rows.push((
                category,
                u32::from(skill_id),
                None,
                action_dat_file_id(
                    u32::from(skill_id),
                    Some(animation),
                    category,
                    None,
                    Some(dll),
                ),
            ));
        }
    }
    for &(ws_id, animation) in ffxi_vocab::action_anim::WEAPON_SKILL_ANIMATION {
        for race in PC_LOOK_RACES {
            rows.push((
                CATEGORY_WEAPON_SKILL,
                u32::from(ws_id),
                Some(race),
                action_dat_file_id(
                    u32::from(ws_id),
                    Some(animation),
                    CATEGORY_WEAPON_SKILL,
                    Some(race),
                    Some(dll),
                ),
            ));
        }
    }
    Corpus { rows }
}

#[test]
fn effect_dat_instruction_census() {
    let Some(root) = ffxi_dat::archive::open_test_install() else {
        return;
    };
    let Ok(dll) = MainDll::load(root.root()) else {
        eprintln!(
            "effect census: {} has no readable FFXiMain.dll; skipping",
            root.root().display()
        );
        return;
    };

    let corpus = build_corpus(&dll);
    assert!(
        !corpus.rows.is_empty(),
        "the scraped action tables are empty"
    );

    println!("=== EFFECT-DAT INSTRUCTION CENSUS ===");
    println!("install: {}", root.root().display());
    println!("corpus rows: {}", corpus.rows.len());

    let mut per_file: HashMap<u32, Resolution> = HashMap::new();
    let mut stage_tallies: BTreeMap<(StageBucket, u8), OpcodeTally> = BTreeMap::new();
    let mut generator_tallies: BTreeMap<
        (GeneratorOpcodeOutcome, GeneratorSection, u8),
        OpcodeTally,
    > = BTreeMap::new();
    let mut observed_stage_opcodes: BTreeSet<u8> = BTreeSet::new();
    let handled = probe_handled();

    for &(_, _, _, file_id) in &corpus.rows {
        let Some(file_id) = file_id else { continue };
        if per_file.contains_key(&file_id) {
            continue;
        }
        let Ok(loc) = root.resolve(file_id) else {
            per_file.insert(file_id, Resolution::Unresolvable);
            continue;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            per_file.insert(file_id, Resolution::Unreadable);
            continue;
        };
        let (schedulers, _, report, _) = parse_action_bytes_reporting(&bytes);
        per_file.insert(
            file_id,
            if schedulers.is_empty() {
                Resolution::NoRoutines
            } else if schedulers.iter().any(|s| &s.name == b"main") {
                Resolution::Ok
            } else {
                Resolution::NoMain
            },
        );

        for scheduler in &schedulers {
            for timed in &scheduler.stages {
                let stage = timed.stage;
                observed_stage_opcodes.insert(stage.raw_type);
                let bucket = if stage.kind != StageKind::Unknown {
                    StageBucket::Handled
                } else if is_structural_opcode(stage.raw_type) {
                    StageBucket::Structural
                } else if handled.contains_key(&stage.raw_type) {
                    StageBucket::ShortStageSuppressed
                } else {
                    StageBucket::Unhandled
                };
                stage_tallies
                    .entry((bucket, stage.raw_type))
                    .or_default()
                    .record(file_id, scheduler.name);
            }
        }
        for &(chunk, section, opcode, outcome) in &report.generator_opcodes {
            generator_tallies
                .entry((outcome, section, opcode))
                .or_default()
                .record(file_id, chunk);
        }
    }

    print_resolution(&corpus, &per_file);
    print_stage_coverage(&stage_tallies, &handled, &observed_stage_opcodes);
    print_generator_coverage(&generator_tallies);

    let parsed = per_file.values().filter(|r| **r != Resolution::Ok).count();
    assert!(
        parsed < per_file.len(),
        "no corpus file resolved and parsed: the census measured nothing"
    );
    assert!(
        stage_tallies
            .keys()
            .any(|(bucket, _)| *bucket == StageBucket::Handled),
        "no handled stage opcode occurred: the corpus walk is broken, not the coverage"
    );
}

fn print_resolution(corpus: &Corpus, per_file: &HashMap<u32, Resolution>) {
    println!("\n--- RESOLUTION (rows by category, outcome of the row's file) ---");
    let mut by_category: BTreeMap<u8, BTreeMap<Resolution, Vec<CorpusRow>>> = BTreeMap::new();
    for &(category, action_id, race, file_id) in &corpus.rows {
        let outcome = match file_id {
            None => Resolution::NoFileId,
            Some(id) => per_file[&id],
        };
        by_category
            .entry(category)
            .or_default()
            .entry(outcome)
            .or_default()
            .push((category, action_id, race, file_id));
    }
    for (category, outcomes) in &by_category {
        let total: usize = outcomes.values().map(Vec::len).sum();
        println!("category {category}: {total} rows");
        for (outcome, rows) in outcomes {
            println!("  {outcome:?}: {} rows", rows.len());
            if *outcome == Resolution::Ok {
                continue;
            }
            for (_, action_id, race, file_id) in rows.iter().take(MAX_EXAMPLES) {
                println!(
                    "    action {action_id} race {race:?} file {}",
                    file_id.map_or("none".to_string(), |f| format!("{f} ({f:#x})"))
                );
            }
            if rows.len() > MAX_EXAMPLES {
                println!("    ... and {} more", rows.len() - MAX_EXAMPLES);
            }
        }
    }
    let mut distinct: BTreeMap<Resolution, usize> = BTreeMap::new();
    for outcome in per_file.values() {
        *distinct.entry(*outcome).or_default() += 1;
    }
    println!("distinct files: {} total", per_file.len());
    for (outcome, count) in &distinct {
        println!("  {outcome:?}: {count}");
    }
}

fn print_stage_coverage(
    tallies: &BTreeMap<(StageBucket, u8), OpcodeTally>,
    handled: &BTreeMap<u8, BTreeSet<usize>>,
    observed: &BTreeSet<u8>,
) {
    println!("\n--- INSTRUCTION COVERAGE: scheduler stage opcodes ---");
    for bucket in [
        StageBucket::Unhandled,
        StageBucket::ShortStageSuppressed,
        StageBucket::Handled,
        StageBucket::Structural,
    ] {
        let mut rows: Vec<_> = tallies
            .iter()
            .filter(|((b, _), _)| *b == bucket)
            .map(|((_, opcode), tally)| (*opcode, tally))
            .collect();
        rows.sort_by_key(|(opcode, tally)| (std::cmp::Reverse(tally.occurrences), *opcode));
        let total: usize = rows.iter().map(|(_, t)| t.occurrences).sum();
        println!("{bucket:?}: {} opcodes, {total} occurrences", rows.len());
        for (opcode, tally) in rows {
            let (file_id, name) = tally.example.expect("a tallied opcode has an example");
            println!(
                "  {opcode:#04x}  n={:<6} files={:<5} kind={:?}  e.g. file {file_id} ({file_id:#x}) routine {}",
                tally.occurrences,
                tally.files.len(),
                StageKind::from_stage(opcode, *STAGE_WORDS_RANGE.end()),
                fourcc(name),
            );
        }
    }
    let never_seen: Vec<u8> = handled
        .keys()
        .copied()
        .filter(|op| !observed.contains(op))
        .collect();
    println!(
        "probe: from_stage answers for {} opcodes; {} of them never occur in this corpus: {}",
        handled.len(),
        never_seen.len(),
        never_seen
            .iter()
            .map(|op| format!("{op:#04x}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
}

fn print_generator_coverage(
    tallies: &BTreeMap<(GeneratorOpcodeOutcome, GeneratorSection, u8), OpcodeTally>,
) {
    println!("\n--- INSTRUCTION COVERAGE: generator section opcodes ---");
    for outcome in [
        GeneratorOpcodeOutcome::Dropped,
        GeneratorOpcodeOutcome::Decoded,
    ] {
        let mut rows: Vec<_> = tallies
            .iter()
            .filter(|((o, ..), _)| *o == outcome)
            .map(|((_, section, opcode), tally)| (*section, *opcode, tally))
            .collect();
        rows.sort_by_key(|(section, opcode, tally)| {
            (std::cmp::Reverse(tally.occurrences), *section, *opcode)
        });
        let total: usize = rows.iter().map(|(.., t)| t.occurrences).sum();
        println!("{outcome:?}: {} codes, {total} occurrences", rows.len());
        for (section, opcode, tally) in rows {
            let (file_id, name) = tally.example.expect("a tallied opcode has an example");
            println!(
                "  {section:?} {opcode:#04x}  n={:<6} files={:<5} e.g. file {file_id} ({file_id:#x}) chunk {}",
                tally.occurrences,
                tally.files.len(),
                fourcc(name),
            );
        }
    }
}
