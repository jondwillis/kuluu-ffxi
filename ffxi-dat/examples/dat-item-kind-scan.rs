//! Probe: which record byte is retail's furniture "kind", and what does it read for furniture?
//!
//! The myroom rebuild keeps an item only when `byte[obj + 4]` is 10, 12 or 14 (FFXiMain.dll
//! retail-2026-09 `.text 0x1D2C3C`, the kind read at `.text 0x1D2C31` from whatever the item-info
//! lookup `.text 0x15BC40` returns). This histograms [`ffxi_dat::item_dat::ItemStatic`] fields over
//! every record and prints names, so a field/value pair can be matched to that predicate.
//!
//! Measured on the retail-2026-09 install: `item_type` is that byte — it reads 10 for 479 records
//! (beds, tables, chests, desks), 12 for the 10 flowerpots and 14 for the 8 mannequins. Nothing else
//! in the record range produces those three values, which is why the ingest asks the DAT instead of
//! keeping a kind table.

use std::collections::BTreeMap;
use std::env;
use std::process::ExitCode;

use ffxi_dat::item_dat::ItemTable;
use ffxi_dat::DatRoot;

fn main() -> ExitCode {
    let keywords: Vec<String> = env::args().skip(1).collect();
    let root = match DatRoot::from_env_or_default() {
        Ok(root) => root,
        Err(e) => {
            eprintln!("no install: {e}");
            return ExitCode::from(1);
        }
    };
    let table = ItemTable::open_from_root(&root);
    if table.is_empty() {
        eprintln!("no item DATs");
        return ExitCode::from(1);
    }

    let mut by_type: BTreeMap<u8, usize> = BTreeMap::new();
    let mut samples: BTreeMap<u8, Vec<String>> = BTreeMap::new();
    let mut found = 0usize;
    for id in 0..=u16::MAX {
        let Some(rec) = table.lookup(id) else {
            continue;
        };
        found += 1;
        *by_type.entry(rec.item_type).or_default() += 1;
        if keywords.is_empty() || keywords.iter().any(|k| rec.name.contains(k)) {
            let v = samples.entry(rec.item_type).or_default();
            if v.len() < 8 {
                v.push(format!("{id:#06x} flags={:#06x} {}", rec.flags, rec.name));
            }
        }
    }
    println!("records={found} layouts={:?}", table.layouts());
    println!("--- item_type histogram ---");
    for (k, n) in by_type {
        println!("type {:>3} (0x{k:02X}): {n}", k);
    }
    let mut keys: Vec<u8> = samples.keys().copied().collect();
    keys.sort();
    for k in keys {
        println!("--- item_type {k} samples ---");
        for s in &samples[&k] {
            println!("{s}");
        }
    }
    ExitCode::SUCCESS
}
