//! Inventory all chunk kinds + names in a single DAT. Used to discover
//! where MMB-placement records live in a zone DAT.
//!
//! Usage:
//!   FFXI_DAT_PATH=... cargo run -p ffxi-dat --example dat-chunk-kinds -- <file_id>

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::{walk, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: FFXI_DAT_PATH=... {} <file_id>", args[0]);
        return ExitCode::from(2);
    }
    let file_id: u32 = args[1].parse().unwrap();
    let root = DatRoot::from_env().unwrap();
    let location = root.resolve(file_id).unwrap();
    let bytes = fs::read(location.path_under(root.root())).unwrap();
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();

    println!("file_id={file_id} chunk_count={}", chunks.len());

    let mut by_kind: BTreeMap<u8, (usize, usize)> = BTreeMap::new();
    for c in &chunks {
        let e = by_kind.entry(c.kind).or_insert((0, 0));
        e.0 += 1;
        e.1 += c.data.len();
    }
    println!();
    println!("kind summary:");
    for (k, (n, b)) in &by_kind {
        println!("  0x{k:02X}  count={n:>5}  total_body={b}");
    }
    println!();

    // Print every non-MMB, non-IMG chunk in full (kind, name, size) so we can
    // spot a placement-table chunk.
    println!("non-MMB(0x2E) non-MZB(0x1C) chunks (incl. IMG 0x20):");
    for (i, c) in chunks.iter().enumerate() {
        if matches!(c.kind, 0x2E | 0x1C) {
            continue;
        }
        println!(
            "  [{i:>4}] kind=0x{:02X} name={:?} body_len={}",
            c.kind,
            c.name_str(),
            c.data.len()
        );
    }
    ExitCode::SUCCESS
}
