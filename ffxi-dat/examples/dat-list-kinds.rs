//! Quick one-shot kind listing: print every chunk's index, kind byte,
//! kind name (if known), and body length. Used to find which file
//! contains a particular chunk kind (e.g. Sk2) when the docs guess wrong.
//!
//!   cargo run -p ffxi-dat --example dat-list-kinds -- <file_id>

use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::{walk, ChunkKind, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: {} <file_id>", args[0]);
        return ExitCode::from(2);
    }
    let file_id: u32 = args[1].parse().unwrap();
    let root = DatRoot::from_env_or_default().unwrap();
    let loc = root.resolve(file_id).unwrap();
    let bytes = fs::read(loc.path_under(root.root())).unwrap();
    println!("file_id={file_id} bytes={}", bytes.len());
    for (i, c) in walk(&bytes).filter_map(Result::ok).enumerate() {
        let name = ChunkKind::from_u8(c.kind)
            .map(|k| format!("{k:?}"))
            .unwrap_or_else(|| "?".into());
        println!(
            "  chunk[{i:>3}] kind=0x{:02x} ({name:<10}) body_len={}",
            c.kind,
            c.data.len()
        );
    }
    ExitCode::SUCCESS
}
