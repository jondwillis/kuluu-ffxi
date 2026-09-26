//! Raw hex dump of a byte range inside one event's bytecode, for reading a
//! multi-record opcode by hand.
//!
//! `cargo run -p ffxi-event --example zz-da-hex -- <zone> <event id> <start> <end>`

use ffxi_dat::event_dat::EventDat;
use ffxi_dat::DatRoot;

fn main() {
    let mut args = std::env::args().skip(1);
    let zone: u16 = args.next().and_then(|s| s.parse().ok()).expect("zone id");
    let event_id: u16 = args.next().and_then(|s| s.parse().ok()).expect("event id");
    let start: usize = args.next().and_then(|s| s.parse().ok()).expect("start");
    let end: usize = args.next().and_then(|s| s.parse().ok()).expect("end");
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
        let data = &block.event_data;
        println!("--- block 0x{:08X} len {} ---", block.actor, data.len());
        let end = end.min(data.len());
        for off in (start..end).step_by(16) {
            let row = &data[off..off + 16.min(end - off)];
            let hex: String = row.iter().map(|b| format!("{b:02X} ")).collect();
            let ascii: String = row
                .iter()
                .map(|b| {
                    if b.is_ascii_graphic() {
                        *b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            println!("{off:6}: {hex:<48} {ascii}");
        }
    }
}
