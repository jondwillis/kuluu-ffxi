//! Corpus check for jump-target addressing in retail event bytecode: walks
//! every entry of a zone's event blocks, collects instruction boundaries, and
//! verifies that GOTO/JUMP targets and IF taken-branch targets (val3 read as
//! an absolute offset into EventData) all land on one. The REL column reports
//! how many IFs would miss under the `ExecPointer += val3` reading printed in
//! research/XiEvents/OpCodes/0x0002.md's case tables; it stays nonzero across
//! every zone, which is why EventVm::op_if reads the target absolute.
//!
//! A walk that runs past an event's END into trailing block data can report a
//! few phantom misses there; real code regions are clean.
//!
//! `cargo run -p ffxi-event --example zz-jump-check -- <zone> [event id]`

use ffxi_dat::event_dat::{EventBlock, EventDat};
use ffxi_dat::DatRoot;
use ffxi_event::opcode_meta::{sub_size, OPCODE_META};

type DecodedBlock = (Vec<usize>, Vec<(usize, u8, usize, usize)>);

/// research/XiEvents/OpCodes/0x0001.md: the unconditional jump.
const OP_GOTO: u8 = 0x01;
/// research/XiEvents/OpCodes/0x0002.md: the conditional jump.
const OP_IF: u8 = 0x02;
/// research/XiEvents/OpCodes/0x001A.md: the long jump.
const OP_JUMP: u8 = 0x1A;

/// ffxi-dat/src/event_dat.rs: walk every entry offset directly and collect
/// instruction start positions (absolute into event_data), including the
/// EVENT_ID_PLACEHOLDER-owned shared code regions that event_entry_exact
/// refuses to hand out; also record jump opcodes with their absolute position.
/// The walk stops at an opcode byte past the table or one the table marks
/// undefined.
fn decode_block(block: &EventBlock) -> DecodedBlock {
    let mut boundaries: std::collections::BTreeSet<usize> = Default::default();
    let mut jumps: Vec<(usize, u8, usize, usize)> = vec![];

    for &start in block.event_offsets.iter() {
        let start = start as usize;
        let data = &block.event_data[start..];
        let mut pos = 0usize;
        while pos < data.len() {
            let abs = start + pos;
            boundaries.insert(abs);
            let op = data[pos];
            let Some(meta) = OPCODE_META.get(op as usize) else {
                break;
            };
            if !meta.valid {
                break;
            }
            let sub = *data.get(pos + 1).unwrap_or(&0);
            let width = sub_size(op, sub).unwrap_or(meta.size) as usize;
            if width == 0 || pos + width > data.len() {
                break;
            }
            match op {
                OP_GOTO | OP_JUMP => {
                    let target = u16::from_le_bytes([data[pos + 1], data[pos + 2]]) as usize;
                    jumps.push((abs, op, target, target));
                }
                OP_IF => {
                    let val3 = u16::from_le_bytes([data[pos + 6], data[pos + 7]]);
                    let abs_target = val3 as usize;
                    let rel_target = (abs as u16).wrapping_add(val3) as usize;
                    jumps.push((abs, op, abs_target, rel_target));
                }
                _ => {}
            }
            pos += width;
        }
    }
    (boundaries.into_iter().collect(), jumps)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let zone: u16 = args.next().and_then(|s| s.parse().ok()).expect("zone id");
    let only: Option<u16> = args.next().as_deref().and_then(|s| s.parse().ok());

    let root = DatRoot::from_env_or_default().expect("DatRoot");
    let loc = root
        .resolve(ffxi_dat::event_locate::event_dat_file_id(zone))
        .expect("event DAT mapping");
    let bytes = std::fs::read(loc.path_under(&root)).expect("read event DAT");
    let dat = EventDat::parse(&bytes).expect("parse event DAT");

    for block in &dat.blocks {
        if only.is_some_and(|o| !block.event_ids.contains(&o)) {
            continue;
        }
        let (boundaries, jumps) = decode_block(block);
        let mut abs_miss = 0usize;
        let mut rel_miss = 0usize;
        for (abs, op, abs_target, rel_target) in &jumps {
            let kind = match *op {
                0x01 => "GOTO",
                0x1A => "JUMP",
                _ => "IF  ",
            };
            if !boundaries.contains(abs_target) {
                abs_miss += 1;
                println!(
                    "  MISS-ABS {kind} @abs {abs}: target {abs_target} is not an instruction boundary"
                );
            }
            if *op == OP_IF && !boundaries.contains(rel_target) {
                rel_miss += 1;
                println!(
                    "  MISS-REL IF   @abs {abs}: abs+val3 = {rel_target} is not an instruction boundary"
                );
            }
        }
        let n_ifs = jumps.iter().filter(|(_, op, _, _)| *op == OP_IF).count();
        println!(
            "block 0x{:08X}: {} boundaries, {} jump opcodes ({} IFs): ABS misses {abs_miss}, REL misses {rel_miss}",
            block.actor,
            boundaries.len(),
            jumps.len(),
            n_ifs
        );
    }
}
