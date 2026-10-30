//! One event's bytecode, thread by thread, with the same width rules the VM
//! uses (`OPCODE_META` fixed size overridden by `sub_size(op, byte@+1)`).
//! A thread walk stops at OP_END/OP_EXECEND so a block holding several entry
//! threads cannot be misread as one linear stream.

use std::process::ExitCode;

use ffxi_dat::event_dat::{EventDat, EVENT_ID_PLACEHOLDER};
use ffxi_dat::{event_locate, DatRoot};
use ffxi_event::opcode_meta::{sub_size, OPCODE_META};

fn fourcc(v: u32) -> Option<String> {
    let b = v.to_le_bytes();
    if b.iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
        Some(String::from_utf8_lossy(&b).to_string())
    } else {
        None
    }
}

fn width(data: &[u8], o: usize, fixed: u8) -> u8 {
    data.get(o + 1)
        .and_then(|&sub| sub_size(data[o], sub))
        .unwrap_or(fixed)
}

/// Walk one thread from `off`, stopping at an END/EXECEND byte or the stop.
fn disasm_thread(block: &ffxi_dat::event_dat::EventBlock, off: usize, stop: usize) {
    let data = &block.event_data;
    let mut o = off;
    while o < stop.min(data.len()) {
        let op = data[o];
        if op as usize >= OPCODE_META.len() {
            println!("{o:5}: {op:#04x} outside the opcode table");
            return;
        }
        let meta = OPCODE_META[op as usize];
        if !meta.valid {
            println!("{o:5}: {op:#04x} INVALID (size {})", meta.size);
            return;
        }
        if op == 0x00 || op == 0x21 {
            let w = width(data, o, meta.size);
            println!("{o:5}: {:02x} size={:<2} END-of-thread marker", op, w);
            return;
        }
        let size = width(data, o, meta.size) as usize;
        if size == 0 {
            println!("{o:5}: {op:#04x} zero-width");
            return;
        }
        let end = (o + size).min(stop).min(data.len());
        let mut note = String::new();
        for chunk in data[o + 1..end].chunks(4) {
            let mut b = [0u8; 4];
            b[..chunk.len()].copy_from_slice(chunk);
            let v = u32::from_le_bytes(b);
            match fourcc(v) {
                Some(s) => note.push_str(&format!(" +{:02X}={v:10} '{s}'", o + 1)),
                None => note.push_str(&format!(" +{v}")),
            }
        }
        println!(
            "{o:5}: {:02x} size={:<2}{}{}{}",
            op,
            size,
            if meta.jumps { " J" } else { "" },
            if meta.sets_ret { " R" } else { "" },
            note
        );
        o += size;
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: zz-cs-disasm <zone_id> <event_id> [actor_hex start_dec stop_dec]");
        return ExitCode::from(2);
    }
    let Ok(zone) = args[0].parse::<u16>() else {
        return ExitCode::from(2);
    };
    let Ok(want) = args[1].parse::<u16>() else {
        return ExitCode::from(2);
    };

    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => return fail(&format!("dat root: {e}")),
    };
    let file_id = event_locate::event_dat_file_id(zone);
    let Ok(loc) = root.resolve(file_id) else {
        return fail(&format!("zone {zone} event DAT {file_id}: unresolvable"));
    };
    println!(
        "zone {zone} event {want} -> file_id {file_id} {}",
        loc.path_under(&root).display()
    );
    let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
        return fail("read event DAT");
    };
    let Ok(ev) = EventDat::parse(&bytes) else {
        return fail("parse event DAT");
    };

    let mut found = 0;
    for block in &ev.blocks {
        // Every entry (event id, offset), including repeats of `want`.
        let mut entries: Vec<(usize, u16)> = block
            .event_ids
            .iter()
            .enumerate()
            .filter(|(_, id)| **id != EVENT_ID_PLACEHOLDER)
            .map(|(i, id)| (block.event_offsets[i] as usize, *id))
            .collect();
        entries.sort();
        let hits: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, (off, id))| {
                let _ = off;
                *id == want
            })
            .map(|(n, (_, _))| n)
            .collect();
        for n in hits {
            let (off, _) = entries[n];
            found += 1;
            let stop = entries
                .get(n + 1)
                .map(|(o, _)| *o)
                .unwrap_or(block.event_data.len());
            println!(
                "\n##### block actor 0x{:08X} ({}) id {want}: entry @{off}..{stop} of {}",
                block.actor,
                block.actor,
                block.event_data.len()
            );
            disasm_thread(block, off, stop);
            // Optional raw thread dump [actor_hex start stop] for jump targets
            // that live past an EXECEND marker inside the same entry range.
            if args.len() == 5 {
                let want_actor = u32::from_str_radix(&args[2], 16).unwrap_or(0);
                let start: usize = args[3].parse().unwrap_or(usize::MAX);
                let stop_at: usize = args[4].parse().unwrap_or(block.event_data.len());
                if block.actor == want_actor {
                    println!("-- raw thread @{start} --");
                    disasm_thread(block, start, stop_at.min(block.event_data.len()));
                }
            }
        }
    }
    if found == 0 {
        println!("no event {want} in this zone's DAT");
    }
    ExitCode::SUCCESS
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("{msg}");
    ExitCode::from(1)
}
