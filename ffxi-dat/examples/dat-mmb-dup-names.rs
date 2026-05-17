//! Find MMB asset names that appear on MORE THAN ONE chunk in a DAT.
//! Important for placement-resolution: if two MMB chunks share an
//! asset_name, the current resolver picks only the first; placements
//! that intended the second instance silently lose their geometry.
//!
//!   FFXI_DAT_PATH=... cargo run -p ffxi-dat --example dat-mmb-dup-names -- <file_id>

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::mmb::MmbHeader;
use ffxi_dat::{mmb, walk, ChunkKind, DatRoot};

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

    let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, c) in chunks.iter().enumerate() {
        if ChunkKind::from_u8(c.kind) != Some(ChunkKind::Mmb) {
            continue;
        }
        let Ok(dec) = mmb::decrypt(c.data) else {
            continue;
        };
        let Ok(hdr) = MmbHeader::parse(&dec) else {
            continue;
        };
        let name = hdr.asset_name_str().trim().to_string();
        by_name.entry(name).or_default().push(i);
    }

    let mut dup_count = 0u32;
    let mut total_extra = 0u32;
    println!("Duplicate MMB asset names in DAT {file_id}:");
    for (name, chunk_idxs) in &by_name {
        if chunk_idxs.len() > 1 {
            dup_count += 1;
            total_extra += (chunk_idxs.len() - 1) as u32;
            println!("  {} -> chunks {:?}", name, chunk_idxs);
        }
    }
    println!();
    println!("{dup_count} asset names appear on >1 chunk");
    println!("{total_extra} extra chunks lost to first-match resolution");
    println!("{} unique names total", by_name.len());
    ExitCode::SUCCESS
}
