//! Dump a single chunk's bytes (hex + ASCII) for byte-level RE.
//!
//! Usage:
//!   FFXI_DAT_PATH=... cargo run -p ffxi-dat --example dat-chunk-dump -- <file_id> <chunk_idx> [byte_limit]

use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::{walk, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "usage: FFXI_DAT_PATH=... {} <file_id> <chunk_idx> [byte_limit]",
            args[0]
        );
        return ExitCode::from(2);
    }
    let file_id: u32 = args[1].parse().unwrap();
    let chunk_idx: usize = args[2].parse().unwrap();
    let byte_limit: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(512);

    let root = DatRoot::from_env().unwrap();
    let location = root.resolve(file_id).unwrap();
    let bytes = fs::read(location.path_under(root.root())).unwrap();
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
    let c = &chunks[chunk_idx];

    println!(
        "file_id={file_id} idx={chunk_idx} kind=0x{:02X} name={:?} body_len={}",
        c.kind,
        c.name_str(),
        c.data.len()
    );
    let end = byte_limit.min(c.data.len());
    for (i, line) in c.data[..end].chunks(16).enumerate() {
        let off = i * 16;
        let hex: Vec<String> = line.iter().map(|b| format!("{:02x}", b)).collect();
        let ascii: String = line
            .iter()
            .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
            .collect();
        println!("  {:06x}  {:<48}  {}", off, hex.join(" "), ascii);
    }
    if end < c.data.len() {
        println!("  ... +{} bytes", c.data.len() - end);
    }
    ExitCode::SUCCESS
}
