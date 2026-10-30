//! Corpus-wide event-VM coverage census: every opcode the current install's
//! zone event DATs actually execute, cross-referenced against kuluu's own VM
//! arms (parsed from vm.rs + vm/scene.rs), plus a scheduler stage-tag census of
//! the cutscene carriers so skipped-by-size tags and their bytes land in one
//! table. Analysis probe; not wired into anything.
//!
//! `cargo run -q --release -p ffxi-event --example zz-opcode-gap`
//! flags: `--sched <from>..<to>` widen the scheduler-carrier file-id window.

use std::collections::BTreeMap;
use std::process::ExitCode;

use ffxi_dat::event_dat::{EventDat, EVENT_ID_PLACEHOLDER};
use ffxi_dat::scheduler::StageKind;
use ffxi_dat::{event_locate, resource_dir::ResourceDir, DatRoot};
use ffxi_event::opcode_meta::{sub_size, OPCODE_META};

const MAX_THREAD_STEPS: usize = 200_000;

/// (name, value) OP_* constants declared in a source file.
fn consts_of(src: &str) -> Vec<(String, u8)> {
    let mut out = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        // Visibility-qualified declarations count too: the width-table constants
        // are `pub(crate) const` in opcode_meta.rs, and skipping them made armed
        // ops report as gaps (the reason those rows were trusted before).
        let t = t
            .strip_prefix("pub(crate) ")
            .or_else(|| t.strip_prefix("pub "))
            .map(str::trim_start)
            .unwrap_or(t);
        if let Some(rest) = t.strip_prefix("const OP_") {
            if let Some(eq) = rest.find(": u8 = ") {
                let name = format!("OP_{}", &rest[..eq]);
                // `0xNN`, not "the first run of hex digits": that read only the
                // leading `0` of every constant, so all values collapsed to zero and
                // arm detection silently reported armed opcodes as gaps (measured:
                // 217 rows, arms among them).
                let digits = match rest[eq + 7..].trim_start().strip_prefix("0x") {
                    Some(hex) => hex,
                    None => match rest[eq + 7..].trim_start().strip_prefix("0X") {
                        Some(hex) => hex,
                        None => continue,
                    },
                };
                let hex_end = digits
                    .find(|c: char| !c.is_ascii_hexdigit())
                    .unwrap_or(digits.len());
                if let Ok(v) = u8::from_str_radix(&digits[..hex_end], 16) {
                    out.push((name, v));
                }
            }
        }
    }
    out.sort_by_key(|(_, v)| *v);
    out.dedup_by_key(|(_, v)| *v);
    out
}

/// The production part of a source file: everything before its first test attribute. A
/// `#[cfg(test)] mod tests` declares constants of its own (vm.rs has an `OP_MOVE_TEST`, value 0x1F),
/// and those are neither arms nor evidence about arms — reading them is what hid the real arm for
/// that opcode (`OP_MOVE`, in vm/scene.rs) and made it print as a gap.
fn without_tests(src: &str) -> String {
    let mut kept = String::new();
    for line in src.lines() {
        if line.trim_start().starts_with("#[cfg(test)]")
            || line.trim_start().starts_with("# [test]")
        {
            break;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    kept
}

/// Every `OP_*` word occurrence in `haystack`; identifier-boundary split, no regex dependency.
fn word_count(haystack: &str, name: &str) -> usize {
    haystack
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| *w == name)
        .count()
}

/// Occurrences of `name` that are not its own declaration line — i.e. a real use.
fn uses_beyond_declaration(body: &str, name: &str) -> usize {
    body.lines()
        .filter(|l| {
            let t = l
                .trim_start()
                .strip_prefix("pub ")
                .unwrap_or(l.trim_start());
            let t = t.strip_prefix("crate ").unwrap_or(t).trim_start();
            word_count(t, name) == 0 || !t.starts_with(&format!("const {name}"))
        })
        .map(|l| word_count(l, name))
        .sum()
}

/// Distinct blocks and programs a thread walk saw, per opcode.
#[derive(Default)]
struct Tally {
    total: u64,
    sites: BTreeMap<u16, (u32, u32)>, // zone -> (blocks, programs)
}

/// Walk one thread, counting ops; stops at END/EXECEND, invalid opcodes, or the
/// step budget. Jumps are not followed (they re-enter already-walked offsets in
/// authored code and would double-count).
fn walk_thread(
    data: &[u8],
    start: usize,
    per_op: &mut BTreeMap<u8, u64>,
    sample: &mut Option<OperandSample>,
) -> Option<String> {
    let stop = data.len();
    let mut o = start;
    let mut steps = 0usize;
    loop {
        if o >= stop {
            return None;
        }
        if steps >= MAX_THREAD_STEPS {
            return Some(format!("thread@{start} exceeded step budget"));
        }
        steps += 1;
        let op = data[o];
        if op as usize >= OPCODE_META.len() || !OPCODE_META[op as usize].valid {
            return None;
        }
        *per_op.entry(op).or_insert(0) += 1;
        if op == 0x00 || op == 0x21 {
            return None;
        }
        let size = data
            .get(o + 1)
            .and_then(|&sub| sub_size(op, sub))
            .unwrap_or(OPCODE_META[op as usize].size) as usize;
        if let Some(s) = sample.as_mut() {
            s.record(data, o, op, size);
        }
        o += size;
    }
}

/// `--sample 0x69,0x6A,..`: keep the first few authored records of each named
/// opcode and print them with every 16-bit word read at both odd (+1,+3,..) and
/// even (+2,+4,..) byte offsets from the opcode. A work operand is a reference
/// selector (high bit set, small index) or a small literal, so one of the two
/// rows reads as authored data and the other as mid-selector garbage — which is
/// how an operand-offset convention gets settled against real DAT content.
struct OperandSample {
    want: Vec<u8>,
    per_op: usize,
    lines: Vec<String>,
}

fn word_at(data: &[u8], o: usize) -> Option<u32> {
    Some(u16::from_le_bytes([*data.get(o)?, *data.get(o + 1)?]) as u32)
}

impl OperandSample {
    fn record(&mut self, data: &[u8], o: usize, op: u8, size: usize) {
        if !self.want.contains(&op) || self.lines.len() >= self.want.len() * self.per_op {
            return;
        }
        let end = (o + size).min(data.len());
        let raw: Vec<String> = data[o..end].iter().map(|b| format!("{b:02X}")).collect();
        let mut odd = String::new();
        let mut even = String::new();
        for d in [1usize, 3, 5, 7] {
            match word_at(data, o + d) {
                Some(v) => odd.push_str(&format!(" {v:#06x}")),
                None => odd.push_str("      --"),
            }
            match word_at(data, o + d + 1) {
                Some(v) => even.push_str(&format!(" {v:#06x}")),
                None => even.push_str("      --"),
            }
        }
        let raw = raw.join(" ");
        self.lines.push(format!(
            "  0x{op:02X} size={size:<2} raw {raw} | words at +1,+3,..:{odd} | at +2,+4,..:{even}"
        ));
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut sched_from = 30_820u32;
    let mut sched_to = 31_250u32;
    let mut sample: Option<OperandSample> = None;
    if let Some(pos) = args.iter().position(|a| a == "--sample") {
        let list = args.get(pos + 1).cloned().unwrap_or_default();
        let want: Vec<u8> = list
            .split(',')
            .filter_map(|s| {
                let s = s.trim().trim_start_matches("0x").trim_start_matches("0X");
                u8::from_str_radix(s, 16).ok()
            })
            .collect();
        if want.is_empty() {
            return fail("--sample expects a comma list like 0x69,0x6A");
        }
        sample = Some(OperandSample {
            want,
            per_op: 8,
            lines: Vec::new(),
        });
    }
    if let Some(pos) = args.iter().position(|a| a == "--sched") {
        if let Some(range) = args.get(pos + 1) {
            let Some((a, b)) = range.split_once("..") else {
                return fail("--sched expects <from>..<to>");
            };
            match (a.parse(), b.parse()) {
                (Ok(f), Ok(t)) => {
                    sched_from = f;
                    sched_to = t;
                }
                _ => return fail("--sched range unparsable"),
            }
        }
    }

    let manifest = env!("CARGO_MANIFEST_DIR");
    let vm_src = match std::fs::read_to_string(format!("{manifest}/src/vm.rs")) {
        Ok(s) => s,
        Err(e) => return fail(&format!("read vm.rs: {e}")),
    };
    let scene_src =
        std::fs::read_to_string(format!("{manifest}/src/vm/scene.rs")).unwrap_or_default();

    // `opcode_meta.rs` declares the constants of width-table ops whose arms live
    // only in vm.rs; without reading it here those arms report as gaps forever.
    let meta_src =
        std::fs::read_to_string(format!("{manifest}/src/opcode_meta.rs")).unwrap_or_default();

    // Constants and arms come from the production text only (`without_tests`): vm.rs's test module
    // declares an `OP_MOVE_TEST` at 0x1F, which made that opcode print as a gap while its real arm
    // (scene.rs's `OP_MOVE`) sat unqueried.
    let vm_body = without_tests(&vm_src);
    let scene_body = without_tests(&scene_src);
    let meta_body = without_tests(&meta_src);

    // One opcode can carry several constant names in this crate: the width table declares some in
    // opcode_meta.rs and the dispatch pattern uses another. Collect every alias and treat the opcode
    // as handled when ANY alias is used for more than its own declaration — the question this census
    // answers is "does kuluu's VM mention this opcode at all", not "is whichever name I happened to
    // pick armed" (picking one name was flaw two of the earlier version: it read 0x52/0x55 as gaps,
    // both arms of multi-line patterns whose head line is a bare constant).
    let mut aliases = BTreeMap::<u8, Vec<String>>::new();
    for (name, v) in consts_of(&vm_body)
        .into_iter()
        .chain(consts_of(&scene_body))
        .chain(consts_of(&meta_body))
    {
        let e = aliases.entry(v).or_default();
        if !e.contains(&name) {
            e.push(name);
        }
    }

    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => return fail(&format!("dat root: {e}")),
    };

    // ---- event DAT corpus -------------------------------------------------
    let mut tally = BTreeMap::<u8, Tally>::new();
    let mut zones_seen = 0usize;
    let mut threads_walked = 0usize;
    let mut parse_failed = 0usize;
    // Threads whose walk stopped early (step budget, or a byte the width table cannot step over).
    // Encounters below are what those walks reached, so this count belongs in the header.
    let mut partial_walks = 0usize;
    for zone in ffxi_dat::event_locate::event_dat_zones(&root) {
        let Ok(loc) = root.resolve(event_locate::event_dat_file_id(zone)) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            continue;
        };
        let Ok(ev) = EventDat::parse(&bytes) else {
            parse_failed += 1;
            continue;
        };
        zones_seen += 1;
        for block in &ev.blocks {
            let mut seen: Vec<usize> = Vec::new();
            for i in 0..block.event_ids.len() {
                if block.event_ids[i] == EVENT_ID_PLACEHOLDER && block.event_offsets[i] == 0 {
                    continue;
                }
                let off = block.event_offsets[i] as usize;
                if seen.contains(&off) {
                    continue;
                }
                seen.push(off);
                threads_walked += 1;
                let mut per_op = BTreeMap::<u8, u64>::new();
                let overflow = walk_thread(&block.event_data, off, &mut per_op, &mut sample);
                if overflow.is_some() {
                    // Counted in the header: encounters below only cover what a linear walk reached.
                    partial_walks += 1;
                }
                for (&op, &n) in &per_op {
                    let t = tally.entry(op).or_default();
                    t.total += n;
                    let e = t.sites.entry(zone).or_insert((0, 0));
                    e.1 += 1;
                    if e.0 == 0 {
                        e.0 = 1; // counted once per block below
                    }
                }
            }
        }
    }

    println!(
        "# event DAT corpus: zones={} threads={} (parse-failed zone DATs: {}, walks not reaching thread end: {})",
        zones_seen, threads_walked, parse_failed, partial_walks
    );
    if let Some(s) = &sample {
        println!("# ---- sampled authored records ({}) ----", s.want.len());
        for line in &s.lines {
            println!("{line}");
        }
    }
    println!(
        "# ---- opcode coverage gaps (valid opcode never mentioned in vm.rs or vm/scene.rs outside its own declaration) ----"
    );
    println!("#  op  kuluu constant name(s)            total   zones");
    let mut gap_rows: Vec<(u8, String, u64, usize, bool)> = Vec::new();
    for v in 0u16..=0xD9 {
        let op = v as u8;
        if (op as usize) >= OPCODE_META.len() || !OPCODE_META[op as usize].valid {
            continue;
        }
        let names = aliases.get(&op).cloned().unwrap_or_default();
        let handled = names.iter().any(|n| {
            uses_beyond_declaration(&vm_body, n) + uses_beyond_declaration(&scene_body, n) > 0
        });
        if handled {
            continue;
        }
        // `cw` marks an opcode whose width table answers a width for at least one selector and the
        // fixed record size for another: kuluu named that op and chose a per-selector skip, which is
        // a deliberate posture (the ledger's WIDTH-ONLY), not an oversight. An opcode whose selectors
        // all answer None has only its declared fixed width and does NOT count.
        let widths = (0..=0xFFu8).map(|sub| sub_size(op, sub));
        let case_widths = {
            let mut seen_varied = false;
            let mut first: Option<Option<u8>> = None;
            for w in widths {
                match first {
                    None => first = Some(w),
                    Some(f) if f != w => seen_varied = true,
                    _ => {}
                }
            }
            seen_varied
        };
        let t = tally.get(&op);
        gap_rows.push((
            op,
            if names.is_empty() {
                "-".to_string()
            } else {
                names.join(",")
            },
            t.map(|t| t.total).unwrap_or(0),
            t.map(|t| t.sites.len()).unwrap_or(0),
            case_widths,
        ));
    }
    // corpus-used gaps first, then unused
    gap_rows.sort_by_key(|(op, _, total, _, _)| (std::cmp::Reverse(*total), *op));
    println!("# cw=kuluu's width table has per-selector rows for it (deliberate skip family)");
    for (op, name, total, zones, cw) in &gap_rows {
        println!(
            "  0x{op:02X}    {:<34} {total:>8}  {zones:>5}   cw:{}",
            name.chars().take(34).collect::<String>(),
            if *cw { 'Y' } else { '-' }
        );
    }

    // ---- scheduler carrier census ----------------------------------------
    println!("#");
    println!("# ---- scheduler stage-tag census over file ids {sched_from}..{sched_to} ----");
    type StageTagTally = (u64, BTreeMap<String, u32>, BTreeMap<String, usize>);
    let mut unknown_tags: BTreeMap<u8, StageTagTally> = BTreeMap::new();
    let mut known_tags: BTreeMap<u8, (String, u64)> = BTreeMap::new();
    for file_id in sched_from..=sched_to {
        let Ok(loc) = root.resolve(file_id) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            continue;
        };
        let dir = ResourceDir::from_bytes(bytes);
        let scheds = dir.collect_schedulers();
        if scheds.is_empty() {
            continue;
        }
        for sched in scheds {
            let rname = String::from_utf8_lossy(&sched.name).to_string();
            for t in &sched.stages {
                let raw = t.stage.raw_type;
                if t.stage.kind == StageKind::Unknown {
                    let e = unknown_tags.entry(raw).or_default();
                    e.0 += 1;
                    *e.1.entry(rname.clone()).or_insert(0) += 1;
                    *e.2.entry(rname.clone()).or_insert(0) = usize::from(t.stage.stage_words);
                } else {
                    let e = known_tags.entry(raw).or_default();
                    if e.0.is_empty() {
                        e.0 = format!("{:?}", t.stage.kind);
                    }
                    e.1 += 1;
                }
            }
        }
    }
    println!("# kind=Unknown tags (raw_type, occurrences, routines+word-counts):");
    let mut unk: Vec<_> = unknown_tags.into_iter().collect();
    unk.sort_by_key(|(raw, _)| *raw);
    for (raw, (n, routines, words)) in &unk {
        let mut rs: Vec<_> = routines.iter().collect();
        rs.sort_by_key(|(name, _)| *name);
        print!("  op=0x{raw:02X} n={n:<4}");
        for (rname, c) in rs.into_iter().take(12) {
            let w = words.get(rname).copied().unwrap_or(0);
            print!(" {rname}[{c}/{w}w]");
        }
        println!();
    }
    println!("# known kinds seen in the same window:");
    for (raw, (kind, n)) in &known_tags {
        println!("  op=0x{raw:02X} {:<38} n={n}", kind);
    }
    ExitCode::SUCCESS
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("{msg}");
    ExitCode::from(1)
}
