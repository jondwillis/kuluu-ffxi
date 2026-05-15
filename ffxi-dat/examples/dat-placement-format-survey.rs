//! For each zone DAT file_id in a range, report:
//! - chunk-1 name tag (zone identifier)
//! - first 3 placement IDs (from SMZBBlock100 records)
//! - first 3 MMB asset names
//! - how many placements resolve to MMBs via `resolve_mmb_index`.
//!
//! Quickly classifies which zones use the Bastok-style direct-or-
//! prefix-match placement convention vs the San d'Oria-style
//! indirect-reference convention.
//!
//! Usage:
//!   FFXI_DAT_PATH=... cargo run --release -p ffxi-dat --example dat-placement-format-survey -- 200 215

use std::env;
use std::fs;

use ffxi_dat::mmb::{self, MmbHeader};
use ffxi_dat::{mzb, walk, ChunkKind, DatRoot};

fn main() {
    let args: Vec<String> = env::args().collect();
    let start: u32 = args[1].parse().unwrap();
    let end: u32 = args[2].parse().unwrap();
    let root = DatRoot::from_env().unwrap();

    for file_id in start..=end {
        let Ok(loc) = root.resolve(file_id) else {
            continue;
        };
        let Ok(bytes) = fs::read(loc.path_under(root.root())) else {
            continue;
        };
        let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
        if chunks.is_empty() {
            continue;
        }
        // Zone tag = first 0x01 chunk name (e.g. "t_ba", "t_sa").
        let zone_tag = chunks
            .iter()
            .find(|c| c.kind == 0x01)
            .map(|c| c.name_str())
            .unwrap_or_else(|| "?".to_string());
        // Skip non-zone files: must have at least one MZB chunk.
        let Some(mzb_chunk) = chunks.iter().find(|c| c.kind == ChunkKind::Mzb as u8) else {
            continue;
        };

        // Collect MMB asset names.
        let mut mmb_names: Vec<String> = Vec::new();
        for c in &chunks {
            if c.kind != 0x2E {
                continue;
            }
            if let Ok(d) = mmb::decrypt(c.data) {
                if let Ok(h) = MmbHeader::parse(&d) {
                    mmb_names.push(h.asset_name_str().trim_end().to_string());
                }
            }
        }
        let prefix = mzb::infer_zone_prefix(&mmb_names);

        let Ok(plain) = mzb::decrypt(mzb_chunk.data) else {
            continue;
        };
        let Ok(header) = mzb::MzbHeader::parse(&plain) else {
            continue;
        };
        let Ok(placements) = mzb::parse_mmb_placements(&plain, &header) else {
            continue;
        };

        let mut hits = 0;
        for p in &placements {
            let id = p.id_str().trim_end();
            if mzb::resolve_mmb_index(id, &prefix, &mmb_names).is_some() {
                hits += 1;
            }
        }
        let first_ids: Vec<String> = placements
            .iter()
            .take(3)
            .map(|p| p.id_str().trim_end().to_string())
            .collect();
        let first_mmb: Vec<String> = mmb_names.iter().take(3).cloned().collect();

        println!(
            "file {file_id:>4}  tag={:6}  mmbs={:>4}  placements={:>5}  resolved={:>5}/{:<5}  prefix={prefix:?}  ids={:?}  mmb={:?}",
            zone_tag,
            mmb_names.len(),
            placements.len(),
            hits,
            placements.len(),
            first_ids,
            first_mmb,
        );
    }
}
