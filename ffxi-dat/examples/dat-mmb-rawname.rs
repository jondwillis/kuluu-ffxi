//! Dump the raw 16-byte `asset_name` field (and a few bytes around it)
//! for each MMB chunk in a DAT — hex + ASCII. Goal: see whether
//! the "duplicate" chunks per `dat-mmb-dup-names` truly share identical
//! asset_name bytes, or differ in some subtle way (NUL placement,
//! extension into the trailing padding, etc).
//!
//!   FFXI_DAT_PATH=... cargo run -p ffxi-dat --example dat-mmb-rawname -- <file_id> [name_substr]

use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::mmb::MmbHeader;
use ffxi_dat::{mmb, walk, ChunkKind, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: FFXI_DAT_PATH=... {} <file_id> [substr]", args[0]);
        return ExitCode::from(2);
    }
    let file_id: u32 = args[1].parse().unwrap();
    let needle = args.get(2).map(|s| s.as_str()).unwrap_or("");

    let root = DatRoot::from_env().unwrap();
    let location = root.resolve(file_id).unwrap();
    let bytes = fs::read(location.path_under(root.root())).unwrap();
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();

    println!("DAT {file_id}: dumping MMB asset_name + neighbors");
    for (i, c) in chunks.iter().enumerate() {
        if ChunkKind::from_u8(c.kind) != Some(ChunkKind::Mmb) {
            continue;
        }
        let Ok(dec) = mmb::decrypt(c.data) else {
            continue;
        };
        if dec.len() < 64 {
            continue;
        }
        let Ok(hdr) = MmbHeader::parse(&dec) else {
            continue;
        };
        let name = hdr.asset_name_str();
        let trimmed = name.trim().to_string();
        if !needle.is_empty() && !trimmed.contains(needle) {
            continue;
        }

        // bytes 0..64 of the decrypted body
        let head = &dec[..64.min(dec.len())];
        print!("  chunk {i:5}  asset='{:18}' [v={} k=0x{:02x} f=0x{:04x}]  ",
            trimmed, hdr.version, hdr.key_index, hdr.feature_flags);
        // hex of 8..32 (asset_name + trailing padding)
        for b in &dec[8..32.min(dec.len())] {
            print!("{:02x}", b);
        }
        print!("  '");
        for &b in &dec[8..32.min(dec.len())] {
            let c = if (32..127).contains(&b) { b as char } else { '.' };
            print!("{c}");
        }
        println!("'");
        let _ = head;
    }
    ExitCode::SUCCESS
}
