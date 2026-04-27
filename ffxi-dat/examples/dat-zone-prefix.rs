use std::env;
use std::fs;

use ffxi_dat::mmb::{self, MmbHeader};
use ffxi_dat::{mzb, walk, ChunkKind, DatRoot};

fn main() {
    let args: Vec<String> = env::args().collect();
    let file_id: u32 = args[1].parse().unwrap();
    let root = DatRoot::from_env().unwrap();
    let bytes = fs::read(root.resolve(file_id).unwrap().path_under(root.root())).unwrap();
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();

    let mut names: Vec<String> = Vec::new();
    for c in &chunks {
        if c.kind != 0x2E {
            continue;
        }
        if let Ok(d) = mmb::decrypt(c.data) {
            if let Ok(h) = MmbHeader::parse(&d) {
                names.push(h.asset_name_str().trim_end().to_string());
            }
        }
    }
    let prefix = mzb::infer_zone_prefix(&names);
    println!("file_id={file_id}");
    println!("mmb_count={}", names.len());
    println!("inferred zone_prefix={prefix:?}");
    println!("first 6 MMB names:");
    for n in names.iter().take(6) {
        println!("  {n:?}");
    }

    let mzb_chunk = chunks
        .iter()
        .find(|c| c.kind == ChunkKind::Mzb as u8)
        .unwrap();
    let plain = mzb::decrypt(mzb_chunk.data).unwrap();
    let header = mzb::MzbHeader::parse(&plain).unwrap();
    println!("mzb node_count={}", header.node_count);
    let placements = mzb::parse_mmb_placements(&plain, &header).unwrap();
    println!("placements: {}", placements.len());
    println!("first 6 placement IDs:");
    for p in placements.iter().take(6) {
        println!("  {:?}", p.id_str().trim_end());
    }

    let mut hits = 0;
    for p in &placements {
        let id = p.id_str().trim_end();
        if mzb::resolve_mmb_index(id, &prefix, &names).is_some() {
            hits += 1;
        }
    }
    println!("resolved: {hits}/{}", placements.len());
}
