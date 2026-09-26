//! Linear opcode listing of one event's bytecode, for spotting which staging
//! opcodes a cutscene authors before the VM implements them.
//!
//! `cargo run -p ffxi-event --example zz-event-ops -- <zone> <event id>`

use ffxi_dat::event_dat::EventDat;
use ffxi_dat::DatRoot;
use ffxi_event::opcode_meta::{sub_size, OPCODE_META};

const OP_END: u8 = 0x00;
const OP_EXECEND: u8 = 0x21;

fn main() {
    let mut args = std::env::args().skip(1);
    let zone: u16 = args.next().and_then(|s| s.parse().ok()).expect("zone id");
    let event_id: u16 = args.next().and_then(|s| s.parse().ok()).expect("event id");
    let starts: Vec<usize> = args.filter_map(|s| s.parse().ok()).collect();
    let root = DatRoot::from_env_or_default().expect("DatRoot");
    let loc = root
        .resolve(ffxi_dat::event_locate::event_dat_file_id(zone))
        .expect("resolve event DAT");
    let bytes = std::fs::read(loc.path_under(&root)).expect("read event DAT");
    let dat = EventDat::parse(&bytes).expect("parse event DAT");
    for block in dat
        .blocks
        .iter()
        .filter(|b| b.event_entry_exact(event_id).is_some())
    {
        let entry = block.event_entry_exact(event_id).unwrap();
        println!("--- block 0x{:08X} entry {entry} ---", block.actor);
        let data = &block.event_data;
        if let Some(i) = data.windows(4).position(|w| w == b"bind") {
            println!("  literal 'bind' at byte {i}");
        }
        let mut starts = starts.clone();
        if starts.is_empty() {
            starts.push(entry);
        }
        for start in starts {
            if start >= data.len() {
                continue;
            }
            println!("  -- from {start} --");
            let mut pc = start;
            let mut count = 0;
            while pc < data.len() && count < 400 {
                let op = data[pc];
                let meta = OPCODE_META.get(op as usize).copied();
                let size = sub_size(op, *data.get(pc + 1).unwrap_or(&0))
                    .map(|s| s as usize)
                    .unwrap_or(meta.map(|m| m.size as usize).unwrap_or(1))
                    .max(1);
                let operands: Vec<String> = data[pc + 1..(pc + size).min(data.len())]
                    .iter()
                    .map(|b| format!("{b:02X}"))
                    .collect();
                println!(
                    "  {pc:5}: {op:02X} {}{}{}",
                    operands.join(" "),
                    if meta.is_some_and(|m| m.jumps) {
                        "  (jump)"
                    } else {
                        ""
                    },
                    if meta.is_none() {
                        "  (out of meta range)"
                    } else {
                        ""
                    }
                );
                if op == OP_END || op == OP_EXECEND {
                    break;
                }
                pc += size;
                count += 1;
            }
        }
    }
}
