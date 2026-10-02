//! B6: distribution of the 0x5B/0x66 actor operands across the retail event
//! DAT corpus (research/XiEvents/OpCodes/0x005B.md, 0x0066.md). The entity Type
//! gate the two motion resource readers apply
//! runs on the entity the opcode names, so the host must know which kind of
//! lookup the corpus
//! actually authors: the event-entity selector, a local-player selector, a
//! party/alliance selector, a literal server id, or a small default-handler
//! fallback.
//!
//! Method: instruction walk. Each block's event entry offsets seed a worklist
//! that advances by the documented opcode width (`opcode_meta::OPCODE_META`,
//! `sub_size` for the variable-width opcodes), records a 0x5B/0x66 site, and
//! follows the absolute u16 targets of GOTO (0x01, +1), IF (0x02, +6) and
//! JUMP (0x1A, +1); every other site falls through by width. END (0x00) and
//! out-of-bounds positions stop a walk; a site is recorded once. A raw byte
//! scan is no use here: 0x5B/0x66 are the ASCII `[`/`f` of the FourCC keys
//! and of banded file-id operands, so the byte scan's literal-id bucket was
//! almost entirely key bytes.
//!
//! Usage: cargo run -p ffxi-event --example zz-5b66-actors -- <install root>

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

use ffxi_dat::{event_dat, event_locate, DatRoot};
use ffxi_event::cue::ActorLookup;
use ffxi_event::opcode_meta::{sub_size, OPCODE_META};

/// research/XiEvents/OpCodes/0x0000.md: the walk's stop opcode.
const OP_END: u8 = 0x00;
/// research/XiEvents/OpCodes/0x005B.md, 0x0066.md: the two event motion
/// resource loaders this probe counts.
const OP_LOADEXTSCHEDULER: u8 = 0x5B;
const OP_LOADEXTSCHEDULER2: u8 = 0x66;

fn categorize(v: u32) -> &'static str {
    let lookup = ActorLookup(v);
    if lookup.is_local_player() {
        "local-player"
    } else if v == ActorLookup::EVENT_ENTITY.0 {
        "event-entity"
    } else if lookup.server_id().is_some() {
        "literal-server-id"
    } else if lookup.is_event_entity() {
        "small-fallback"
    } else {
        "party/alliance"
    }
}

fn u32_at(data: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes(
        data.get(pos..pos + 4)
            .map(|s| [s[0], s[1], s[2], s[3]])
            .unwrap_or([0; 4]),
    )
}

fn u16_at(data: &[u8], pos: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(pos..pos + 2)?.try_into().ok()?))
}

/// One worklist walk: record the 0x5B/0x66 instruction sites reachable from
/// `start` and return them (research/XiEvents/OpCodes/0x005B.md, 0x0066.md).
fn walk_block(data: &[u8], start: usize) -> BTreeSet<usize> {
    let mut sites = BTreeSet::new();
    let mut seen: BTreeSet<usize> = BTreeSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(start);
    while let Some(pos) = queue.pop_front() {
        if !seen.insert(pos) || pos >= data.len() {
            continue;
        }
        let op = data[pos];
        if op == OP_END {
            continue;
        }
        if op == OP_LOADEXTSCHEDULER || op == OP_LOADEXTSCHEDULER2 {
            sites.insert(pos);
        }
        // research/XiEvents/OpCodes: opcodes above 0xD9 are undefined in the meta table;
        // stop the walk there rather than index past the end.
        let Some(meta) = OPCODE_META.get(op as usize) else {
            continue;
        };
        let width = sub_size(op, data.get(pos + 1).copied().unwrap_or(0))
            .unwrap_or(meta.size)
            .max(1) as usize;
        match op {
            0x01 | 0x1A => {
                if let Some(t) = u16_at(data, pos + 1) {
                    queue.push_back(t as usize);
                }
            }
            0x02 => {
                if let Some(t) = u16_at(data, pos + 6) {
                    queue.push_back(t as usize);
                }
            }
            _ => {}
        }
        queue.push_back(pos + width);
    }
    sites
}

fn main() {
    let root_s = std::env::args().nth(1).expect("install root");
    let root = DatRoot::open(Path::new(&root_s)).expect("dat root");

    let mut files_ok = 0usize;
    let mut files_bad = 0usize;
    let mut n5b = 0usize;
    let mut n66 = 0usize;
    let mut key5b = 0usize;
    let mut key66 = 0usize;
    let mut a1_5b: BTreeMap<u32, usize> = BTreeMap::new();
    let mut a1_66: BTreeMap<u32, usize> = BTreeMap::new();
    let mut a2_5b: BTreeMap<u32, usize> = BTreeMap::new();
    let mut a2_66: BTreeMap<u32, usize> = BTreeMap::new();

    for zone in event_locate::event_dat_zones(&root) {
        let Ok(loc) = root.resolve(event_locate::event_dat_file_id(zone)) else {
            files_bad += 1;
            continue;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            files_bad += 1;
            continue;
        };
        let Ok(dat) = event_dat::EventDat::parse(&bytes) else {
            files_bad += 1;
            continue;
        };
        files_ok += 1;
        for block in &dat.blocks {
            let data = &block.event_data;
            let mut sites: BTreeSet<usize> = BTreeSet::new();
            for &entry in &block.event_offsets {
                sites.extend(walk_block(data, entry as usize));
            }
            for &pos in &sites {
                let op = data[pos];
                let actor1 = u32_at(data, pos + 3);
                let actor2 = u32_at(data, pos + 7);
                let key = u32_at(data, pos + 11);
                match op {
                    0x5B => {
                        n5b += 1;
                        if key != 0 {
                            key5b += 1;
                        }
                        *a1_5b.entry(actor1).or_default() += 1;
                        *a2_5b.entry(actor2).or_default() += 1;
                    }
                    _ => {
                        n66 += 1;
                        if key != 0 {
                            key66 += 1;
                        }
                        *a1_66.entry(actor1).or_default() += 1;
                        *a2_66.entry(actor2).or_default() += 1;
                    }
                }
            }
        }
    }

    fn print_summary(label: &str, table: &BTreeMap<u32, usize>) {
        println!("{label}:");
        let mut by_cat: BTreeMap<&str, usize> = BTreeMap::new();
        let mut top: BTreeMap<&str, (u32, usize)> = BTreeMap::new();
        let mut total = 0usize;
        for (v, n) in table {
            let cat = categorize(*v);
            *by_cat.entry(cat).or_default() += n;
            total += n;
            let e = top.entry(cat).or_insert((*v, 0));
            if *n > e.1 {
                *e = (*v, *n);
            }
        }
        for (cat, n) in &by_cat {
            let (v, top_n) = top[cat];
            println!("  {cat:16} x{n:>6}  (most common value {v:#010x} x{top_n})");
        }
        println!("  {total:>16} total");
    }

    println!("event DATs: {files_ok} parsed, {files_bad} unreadable/unparsable");
    println!("0x5B sites: {n5b} (non-empty key: {key5b})");
    println!("0x66 sites: {n66} (non-empty key: {key66})");
    print_summary("0x5B actor1", &a1_5b);
    print_summary("0x5B actor2", &a2_5b);
    print_summary("0x66 actor1", &a1_66);
    print_summary("0x66 actor2", &a2_66);
}
