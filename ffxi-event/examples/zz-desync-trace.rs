//! Where does a desynced event lose alignment? Walks the event body
//! sequentially with the VM's own width logic (OPCODE_META + sub_size, the
//! same widths the VM advances by) and reports the first instruction whose
//! byte is not a known opcode: the instruction before it in the walk is the
//! width-table suspect, because its wrong width is what lands the pointer in
//! data.
//!
//! `cargo run -p ffxi-event --example zz-desync-trace -- <zone> <event id> [<zone> <event id> ...]`

use std::collections::BTreeMap;

use ffxi_dat::event_dat::EventDat;
use ffxi_dat::DatRoot;
use ffxi_event::opcode_meta::{sub_size, OPCODE_META};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut pairs: Vec<(u16, u16)> = Vec::new();
    let mut i = 0;
    while i + 1 < args.len() {
        let zone: u16 = args[i].parse().expect("zone id");
        let event_id: u16 = args[i + 1].parse().expect("event id");
        pairs.push((zone, event_id));
        i += 2;
    }
    if pairs.is_empty() {
        panic!("usage: zz-desync-trace <zone> <event id> [...]");
    }
    let verbose = pairs.len() == 1;

    let root = DatRoot::from_env_or_default().expect("DatRoot");
    let mut dat_cache: BTreeMap<u16, EventDat> = BTreeMap::new();
    let mut suspects: BTreeMap<String, u64> = BTreeMap::new();

    for (zone, event_id) in &pairs {
        let dat = match dat_cache.get(zone) {
            Some(d) => d.clone(),
            None => {
                let loc = root
                    .resolve(ffxi_dat::event_locate::event_dat_file_id(*zone))
                    .unwrap_or_else(|e| panic!("resolve event DAT for zone {zone}: {e}"));
                let bytes = std::fs::read(loc.path_under(&root)).expect("read event DAT");
                let dat = EventDat::parse(&bytes).expect("parse event DAT");
                dat_cache.insert(*zone, dat.clone());
                dat
            }
        };

        let owners: Vec<&ffxi_dat::event_dat::EventBlock> = dat
            .blocks
            .iter()
            .filter(|b| b.event_entry(*event_id).is_some())
            .collect();
        if owners.is_empty() {
            println!("z{zone} e{event_id}: no block owns it");
            continue;
        }

        for block in &owners {
            // The sequential body of this event inside the block.
            let entry = block.event_entry(*event_id).unwrap();
            let data = &block.event_data;
            let mut off = entry;
            let mut trace: Vec<(usize, u8, usize)> = Vec::new();
            let mut first_bad: Option<(usize, u8)> = None;
            while off < data.len() {
                let op = data[off];
                if op == 0x00 || op == 0x21 {
                    trace.push((off, op, 1));
                    break;
                }
                let known = OPCODE_META.get(op as usize).is_some();
                if !known && first_bad.is_none() {
                    first_bad = Some((off, op));
                }
                let fixed = OPCODE_META.get(op as usize).map(|m| m.size).unwrap_or(1);
                let width = sub_size(op, data.get(off + 1).copied().unwrap_or(0))
                    .unwrap_or(fixed)
                    .max(1) as usize;
                trace.push((off, op, width));
                off += width;
                if off <= entry {
                    break; // a width of 0 or a backward step: stop
                }
            }

            if verbose {
                println!(
                "zone {zone} event {event_id} block 0x{:08X}: {} instructions walked from entry {entry}",
                block.actor,
                trace.len()
            );
            }
            match first_bad {
                Some((bad_off, bad_op)) => {
                    let idx = trace
                        .iter()
                        .position(|(o, _, _)| *o == bad_off)
                        .unwrap_or(trace.len());
                    if idx == 0 {
                        println!(
                            "z{zone} e{event_id} (0x{:08X}): first byte 0x{bad_op:02X} unknown",
                            block.actor
                        );
                        continue;
                    }
                    let (s_off, s_op, s_w) = trace[idx - 1];
                    let s_sub = data.get(s_off + 1).copied().unwrap_or(0);
                    let key = format!("0x{s_op:02X} sub 0x{s_sub:02X} (walk w{s_w})");
                    *suspects.entry(key).or_default() += 1;
                    if verbose {
                        println!(
                            "\nfirst unknown opcode byte 0x{bad_op:02X} at block offset {bad_off};"
                        );
                        let lo = idx.saturating_sub(12);
                        println!(
                            "last {n} instructions before it (offset op width -> next):",
                            n = idx - lo
                        );
                        for (i, (o, op, w)) in trace[lo..idx].iter().enumerate() {
                            let marker = if i + lo == idx - 1 {
                                "  <-- suspect width"
                            } else {
                                ""
                            };
                            println!(
                                "    {o:>8}  0x{op:02X}  w{w}  -> {next}{marker}",
                                next = o + w
                            );
                        }
                        let lo2 = bad_off.saturating_sub(16);
                        let hi2 = (bad_off + 32).min(data.len());
                        let hex: Vec<String> = data[lo2..hi2]
                            .iter()
                            .enumerate()
                            .map(|(i, b)| {
                                if lo2 + i == bad_off {
                                    format!("[{b:02X}]")
                                } else {
                                    format!("{b:02X}")
                                }
                            })
                            .collect();
                        println!("bytes around it: {}", hex.join(" "));
                    }
                }
                None => {
                    if verbose {
                        println!("walk stayed on known opcodes to the end");
                    }
                }
            }
        }
    }

    if !verbose {
        println!("suspect instructions (op sub -> count of events desynced after them):");
        let mut by_count: Vec<(&String, &u64)> = suspects.iter().collect();
        by_count.sort_by_key(|&(_, n)| std::cmp::Reverse(*n));
        for (k, n) in by_count {
            println!("  {k} x{n}");
        }
    }
}
