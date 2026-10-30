//! Dump every entry thread of one event block in a zone's event DAT: the
//! TagOffset table (which is what REQSET indexes by tag byte), then a thread
//! walk per distinct offset. Each opcode line is annotated against kuluu's own
//! VM source: `x` = arm only advances it by width (skipped behaviour), `s` =
//! handled only through the scene path, blank = has an ordinary arm. Lines for
//! opcodes with no constant anywhere in vm.rs/scene.rs are marked `-`.
//!
//! `cargo run -q --release -p ffxi-event --example zz-cs-threads -- <zone> <actor_hex>`

use std::process::ExitCode;

use ffxi_dat::event_dat::{EventDat, EVENT_ID_PLACEHOLDER};
use ffxi_dat::{event_locate, DatRoot};
use ffxi_event::cue::SCHEDULER_FADE_DAT_ID;
use ffxi_event::opcode_meta::{sub_size, OPCODE_META};

fn fourcc_str(b: &[u8]) -> String {
    b.iter()
        .map(|&c| {
            if c.is_ascii_graphic() || c == b' ' {
                c as char
            } else {
                '.'
            }
        })
        .collect()
}

fn fourcc(v: u32) -> Option<String> {
    let b = v.to_le_bytes();
    if b.iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
        Some(String::from_utf8_lossy(&b).to_string())
    } else {
        None
    }
}

/// Opcodes kuluu's VM arms but only advances by width (behaviour skipped).
const SKIPPED_BY_WIDTH: &[u8] = &[0x63, 0x6E, 0x99, 0x8D, 0xB8, 0x69, 0x6A, 0x8E, 0x8F];
/// Opcodes handled in the scene path (vm/scene.rs handles_scene_opcode).
const SCENE_OPS: &[u8] = &[
    0x27, 0x28, 0x29, 0x2A, 0x32, 0x3B, 0x1F, 0x5A, 0x31, 0x36, 0x37, 0x39, 0x4A, 0x5E, 0x6B, 0x79,
    0x1E, 0x47,
];

/// Parse the OP_* constants and their arms straight from kuluu's VM sources.
fn coverage(vm_src: &str) -> Vec<String> {
    let mut named: Vec<(String, u8)> = Vec::new();
    for line in vm_src.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("const OP_") {
            // OP_NAME: u8 = 0xNN;   or OP_NAME: u8 = 0xNN,
            if let Some(eq) = rest.find(": u8 = ") {
                let name = rest[..eq].to_string();
                let tail = &rest[eq + 7..];
                let hex_end = tail
                    .find(|c: char| !c.is_ascii_hexdigit())
                    .unwrap_or(tail.len());
                if let Ok(v) = u8::from_str_radix(&tail[..hex_end], 16) {
                    named.push((format!("OP_{name}"), v));
                }
            }
        }
    }
    named.sort_by_key(|(_, v)| *v);
    named.dedup_by_key(|(_, v)| *v);
    let mut out = Vec::new();
    for (name, op) in named {
        // An arm: the constant appears followed by ` =>` or as `| OP_XX`.
        let armed = vm_src.contains(&format!("{name} =>"))
            || vm_src.contains(&format!("| {name}"))
            || vm_src.contains(&format!("\n{name} |"));
        out.push(format!(
            "{op:#04x}:{name}:{}",
            if armed { "arm" } else { "constonly" }
        ));
    }
    out
}

fn mark(op: u8) -> &'static str {
    if SKIPPED_BY_WIDTH.contains(&op) {
        "x"
    } else if SCENE_OPS.contains(&op) {
        "s"
    } else {
        "*"
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 || args.len() > 3 {
        eprintln!("usage: zz-cs-threads <zone_id> <actor_hex> [raw]");
        return ExitCode::from(2);
    }
    let raw_mode = args.len() >= 3 && (args[2] == "raw" || args[2].starts_with("raw-from:"));
    let raw_from: Option<usize> = args
        .get(2)
        .and_then(|a| a.strip_prefix("raw-from:"))
        .and_then(|v| v.parse().ok());
    let Ok(zone) = args[0].parse::<u16>() else {
        return ExitCode::from(2);
    };
    let Ok(actor) = u32::from_str_radix(&args[1], 16) else {
        return ExitCode::from(2);
    };

    let vm_src = match std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/vm.rs")) {
        Ok(s) => s,
        Err(e) => return fail(&format!("read vm.rs for coverage: {e}")),
    };
    println!("# opcode coverage table (from vm.rs): op constant arm-state");
    for line in coverage(&vm_src) {
        println!("#  {line}");
    }

    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => return fail(&format!("dat root: {e}")),
    };
    let file_id = event_locate::event_dat_file_id(zone);
    let loc = match root.resolve(file_id) {
        Ok(l) => l,
        Err(e) => return fail(&format!("resolve: {e}")),
    };
    println!("# {} {}", file_id, loc.path_under(&root).display());
    let bytes = match std::fs::read(loc.path_under(&root)) {
        Ok(b) => b,
        Err(e) => return fail(&format!("read: {e}")),
    };
    let ev = match EventDat::parse(&bytes) {
        Ok(e) => e,
        Err(e) => return fail(&format!("parse: {e}")),
    };
    let Some(block) = ev.blocks.iter().find(|b| b.actor == actor) else {
        return fail(&format!("no block for actor 0x{actor:08X} in zone {zone}"));
    };
    println!(
        "# block actor 0x{:08X}: {} entries, data {}",
        block.actor,
        block.event_ids.len(),
        block.event_data.len()
    );
    if raw_from.is_some() {
        let refs = &block.references;
        println!("# references ({})", refs.len());
        for chunk_start in (0..refs.len()).step_by(8) {
            let mut line = String::new();
            for r in &refs[chunk_start..(chunk_start + 8).min(refs.len())] {
                line.push_str(&format!(" {r}(0x{r:08X})"));
            }
            println!(
                "# [{chunk_start}:{}]{}",
                (chunk_start + 8).min(refs.len()),
                line
            );
        }
    }

    println!("# tag-index | event_id (755 = placeholder) | offset");
    for i in 0..block.event_ids.len() {
        let id = block.event_ids[i];
        if id == EVENT_ID_PLACEHOLDER && block.event_offsets[i] == 0 {
            continue;
        }
        println!("  [ {:>3} ] {} @{}", i, id, block.event_offsets[i]);
    }

    if let Some(start) = raw_from {
        println!("\n##### raw thread @{start}");
        walk_thread(block, start, true);
        return ExitCode::SUCCESS;
    }

    // `walk-all`: every tag-index entry, placeholders included. REQSET-driven
    // scenes live in placeholder threads, so a dump that skips them cannot
    // reconstruct the beat graph.
    if args.get(2).map(|a| a.as_str()) == Some("walk-all") {
        for i in 0..block.event_ids.len() {
            let off = block.event_offsets[i] as usize;
            println!("\n##### tag {i} id {} @ {off}", block.event_ids[i]);
            walk_thread(block, off, false);
        }
        return ExitCode::SUCCESS;
    }

    let mut seen: Vec<usize> = Vec::new();
    for i in 0..block.event_ids.len() {
        if block.event_ids[i] == EVENT_ID_PLACEHOLDER {
            continue;
        }
        let off = block.event_offsets[i] as usize;
        if seen.contains(&off) {
            continue;
        }
        seen.push(off);
        let id = block.event_ids[i];
        println!("\n##### entry [tag {i}] id {id} @ {off}");
        // `[raw]` walks past the EXECEND markers, so the continuation threads a
        // scene keeps after its first end-of-thread show up too - `raw-from:`
        // does the same thing from a caller-picked offset.
        walk_thread(block, off, raw_mode);
    }
    ExitCode::SUCCESS
}

/// One annotated thread walk from `start` to end of block data. With
/// `past_execend`, EXECEND markers do not stop the walk.
fn walk_thread(block: &ffxi_dat::event_dat::EventBlock, start: usize, past_execend: bool) {
    let stop = block.event_data.len();
    let mut o = start;
    loop {
        if o >= stop {
            println!("{o:5}: <end of data>");
            break;
        }
        let op = block.event_data[o];
        if op as usize >= OPCODE_META.len() || !OPCODE_META[op as usize].valid {
            println!("{o:5}: {op:#04x} outside/invalid -> stop");
            break;
        }
        let meta = OPCODE_META[op as usize];
        let fixed = meta.size as usize;
        let size = block
            .event_data
            .get(o + 1)
            .and_then(|&sub| sub_size(op, sub))
            .unwrap_or(meta.size) as usize;
        if op == 0x00 || op == 0x21 {
            let m = mark(op);
            println!("{o:5}: {op:02x} {m} END-of-thread marker (size {fixed})");
            if past_execend && op == 0x21 {
                // Keep walking past EXECEND into the continuation threads.
                o += fixed;
                continue;
            }
            break;
        }
        let end = (o + size).min(stop);
        let mut note = String::new();
        for chunk in block.event_data[o + 1..end].chunks(4) {
            let mut b = [0u8; 4];
            b[..chunk.len()].copy_from_slice(chunk);
            let v = u32::from_le_bytes(b);
            match fourcc(v) {
                Some(s) => note.push_str(&format!(" +{v} '{s}'")),
                None => note.push_str(&format!(" +{v} ({v:#010X})")),
            }
        }
        let rd = |off: usize, n: usize| -> u32 {
            let mut acc = 0u32;
            for k in 0..n {
                if let Some(&byte) = block.event_data.get(o + off + k) {
                    acc |= (byte as u32) << (8 * k);
                }
            }
            acc
        };
        match op {
            // 0x45 family: file u16@+1, actor1 u32@+3, actor2 u32@+7,
            // tag @+11, duration u16@+15.
            0x45 | 0x62 | 0x9F | 0xBB | 0xC5 | 0xCD | 0xD0 | 0xD5 => {
                let file = rd(1, 2);
                note.push_str(&format!(
                    "   >> file={file} dat_id~{} actor1=0x{:X} actor2=0x{:X} tag='{}' dur={}",
                    SCHEDULER_FADE_DAT_ID + file,
                    rd(3, 4),
                    rd(7, 4),
                    fourcc_str(
                        &block.event_data[(o + 11).min(stop)..(o + 15).min(stop.max(o + 12))]
                    )
                    .as_str(),
                    rd(15, 2)
                ));
            }
            // 0x2C SCHEDULOR: actor1@+1, actor2@+5, key @+9.
            0x2C => {
                note.push_str(&format!(
                    "   >> actor1=0x{:X} actor2=0x{:X} key='{}'",
                    rd(1, 4),
                    rd(5, 4),
                    fourcc_str(
                        &block.event_data[(o + 9).min(stop)..(o + 13).min(stop.max(o + 10))]
                    )
                    .as_str()
                ));
            }
            // 0x53 WAIT: actor1@+1, key @+9.
            0x53 => {
                note.push_str(&format!(
                    "   >> actor1=0x{:X} key='{}'",
                    rd(1, 4),
                    fourcc_str(
                        &block.event_data[(o + 9).min(stop)..(o + 13).min(stop.max(o + 10))]
                    )
                    .as_str()
                ));
            }
            // REQ family: priority byte@1, actor u32@+2, tag byte@+6.
            0x27..=0x29 => {
                note.push_str(&format!(
                    "   >> pri={} actor=0x{:X} tag={}",
                    rd(1, 1),
                    rd(2, 4),
                    rd(6, 1)
                ));
            }
            0x2A => {
                note.push_str(&format!("   >> pri={} actor=0x{:X}", rd(1, 1), rd(2, 4)));
            }
            // 0x7E: case byte@1, target u32@+2.
            0x7E => {
                note.push_str(&format!("   >> case={} target=0x{:X}", rd(1, 1), rd(2, 4)));
            }
            _ => {}
        }
        println!(
            "{o:5}: {:02x} {} size={:<2}{}{}{}",
            op,
            mark(op),
            size,
            if meta.jumps { " J" } else { "" },
            if meta.sets_ret { " R" } else { "" },
            note
        );
        o += size;
    }
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("{msg}");
    ExitCode::from(1)
}
