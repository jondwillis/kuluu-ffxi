use std::collections::HashSet;
use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::mmb::MmbHeader;
use ffxi_dat::{mmb, mzb, walk, ChunkKind, DatRoot};

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

    let mut mmb_names: Vec<(usize, String)> = Vec::new();
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
        mmb_names.push((i, hdr.asset_name_str().trim().to_string()));
    }

    let mzb_chunk = chunks
        .iter()
        .find(|c| c.kind == ChunkKind::Mzb as u8)
        .unwrap();
    let plain = mzb::decrypt(mzb_chunk.data).unwrap();
    let header = mzb::MzbHeader::parse(&plain).unwrap();
    let placements = mzb::parse_mmb_placements(&plain, &header).unwrap();
    let names_only: Vec<String> = mmb_names.iter().map(|(_, n)| n.clone()).collect();
    let zone_prefix = mzb::infer_zone_prefix(&names_only);

    let mut referenced: HashSet<usize> = HashSet::new();
    for p in &placements {
        let trimmed = p.id_str().trim_end_matches('\0').trim_end();
        if let Some(idx) = mzb::resolve_mmb_index(trimmed, &zone_prefix, &names_only) {
            referenced.insert(idx);
        }
    }

    let mut unplaced: Vec<(usize, &String)> = mmb_names
        .iter()
        .enumerate()
        .filter(|(local, _)| !referenced.contains(local))
        .map(|(_local, (chunk_idx, name))| (*chunk_idx, name))
        .collect();
    unplaced.sort_by_key(|(_, name)| name.to_string());

    println!(
        "DAT {file_id}: {} MMBs, {} placements, {} referenced, {} unplaced",
        mmb_names.len(),
        placements.len(),
        referenced.len(),
        unplaced.len()
    );
    println!();
    println!("Unplaced MMBs:");
    for (chunk_idx, name) in &unplaced {
        println!("  chunk[{chunk_idx}]  {name}");
    }

    ExitCode::SUCCESS
}
