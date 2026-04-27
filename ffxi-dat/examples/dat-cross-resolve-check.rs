use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;

use ffxi_dat::mmb::{self, MmbHeader};
use ffxi_dat::{mzb, walk, ChunkKind, DatRoot};

fn load_index(path: &str) -> HashMap<String, Vec<(u32, u32)>> {
    let text = fs::read_to_string(path).expect("read mmb_index.json");

    let i = text.find("\"index\"").expect("no index field");
    let body_start = text[i..].find('{').unwrap() + i + 1;
    let body = &text[body_start..];
    let mut out: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i] != b'"' {
            if bytes[i] == b'}' {
                return out;
            }
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        i += 1;
        let name_start = i;
        while i < bytes.len() && bytes[i] != b'"' {
            i += 1;
        }
        let name = String::from_utf8_lossy(&bytes[name_start..i]).into_owned();
        i += 1;

        while i < bytes.len() && bytes[i] != b'[' {
            i += 1;
        }
        i += 1;

        let mut pairs = Vec::new();
        loop {
            while i < bytes.len() && bytes[i] != b'[' && bytes[i] != b']' {
                i += 1;
            }
            if i >= bytes.len() || bytes[i] == b']' {
                if i < bytes.len() {
                    i += 1;
                }
                break;
            }

            i += 1;
            let n1s = i;
            while i < bytes.len() && bytes[i] != b',' {
                i += 1;
            }
            let n1: u32 = std::str::from_utf8(&bytes[n1s..i])
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            i += 1;
            let n2s = i;
            while i < bytes.len() && bytes[i] != b']' {
                i += 1;
            }
            let n2: u32 = std::str::from_utf8(&bytes[n2s..i])
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            i += 1;
            pairs.push((n1, n2));
        }
        out.insert(name, pairs);
    }
    out
}

fn collect_zone(
    root: &DatRoot,
    file_id: u32,
) -> Option<(String, Vec<String>, Vec<mzb::MmbPlacement>)> {
    let loc = root.resolve(file_id).ok()?;
    let bytes = fs::read(loc.path_under(root.root())).ok()?;
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
    let zone_tag = chunks
        .iter()
        .find(|c| c.kind == 0x01)
        .map(|c| c.name_str())
        .unwrap_or_else(|| "?".to_string());
    let mzb_chunk = chunks.iter().find(|c| c.kind == ChunkKind::Mzb as u8)?;
    let mut mmb_names = Vec::new();
    for c in &chunks {
        if c.kind == 0x2E {
            if let Ok(d) = mmb::decrypt(c.data) {
                if let Ok(h) = MmbHeader::parse(&d) {
                    mmb_names.push(h.asset_name_str().trim_end().to_string());
                }
            }
        }
    }
    let plain = mzb::decrypt(mzb_chunk.data).ok()?;
    let header = mzb::MzbHeader::parse(&plain).ok()?;
    let placements = mzb::parse_mmb_placements(&plain, &header).ok()?;
    Some((zone_tag, mmb_names, placements))
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let (zone_ids, index_path) = {
        let mut zones: Vec<u32> = Vec::new();
        let mut path = "/tmp/mmb_index.json".to_string();
        for a in &args[1..] {
            if let Ok(n) = a.parse::<u32>() {
                zones.push(n);
            } else {
                path = a.clone();
            }
        }
        if zones.is_empty() {
            zones = vec![202, 330, 200];
        }
        (zones, path)
    };

    println!("loading global index from {}", index_path);
    let index = load_index(&index_path);
    println!("  → {} unique names\n", index.len());

    let root = DatRoot::from_env().expect("FFXI_DAT_PATH not set");

    for fid in zone_ids {
        let Some((tag, mmbs, placements)) = collect_zone(&root, fid) else {
            println!("file {fid}: no MZB / cannot read");
            continue;
        };
        let prefix = mzb::infer_zone_prefix(&mmbs);
        let total = placements.len();

        let mut local = 0u32;
        let mut cross = 0u32;
        let mut miss = 0u32;
        let mut cross_dist: BTreeMap<u32, u32> = BTreeMap::new();
        let mut miss_samples: Vec<String> = Vec::new();

        let try_global = |id: &str| -> Option<&Vec<(u32, u32)>> {
            if let Some(v) = index.get(id) {
                return Some(v);
            }
            let mut prefixed = String::with_capacity(prefix.len() + id.len());
            prefixed.push_str(&prefix);
            prefixed.push_str(id);
            if prefixed.len() > 16 {
                prefixed.truncate(16);
            }
            if let Some(v) = index.get(&prefixed) {
                return Some(v);
            }

            None
        };

        let try_local_vendor = |id: &str| -> bool {
            if id.is_empty() {
                return false;
            }
            let id8: String = id.chars().take(8).collect();
            mmbs.iter().any(|n| {
                let t = n.trim_end();
                t.len() >= 8 && t[t.len() - 8..] == id8
            })
        };
        let mut local_vendor = 0u32;
        for p in &placements {
            let id = p.id_str().trim_end();
            if id.is_empty() {
                miss += 1;
                continue;
            }
            if mzb::resolve_mmb_index(id, &prefix, &mmbs).is_some() {
                local += 1;
            } else if try_local_vendor(id) {
                local_vendor += 1;
            } else if let Some(locs) = try_global(id) {
                cross += 1;

                let mut seen = std::collections::HashSet::new();
                for (file_id, _) in locs {
                    if seen.insert(*file_id) {
                        *cross_dist.entry(*file_id).or_insert(0) += 1;
                    }
                }
            } else {
                miss += 1;
                if miss_samples.len() < 10 {
                    miss_samples.push(id.to_string());
                }
            }
        }

        println!(
            "file {fid:>4}  tag={tag:6}  local_mmbs={:>5}  placements={total}",
            mmbs.len()
        );
        println!("  prefix      = {:?}", prefix);
        println!("  local       = {local}");
        println!("  local_vendor= {local_vendor}");
        println!("  cross       = {cross}");
        println!("  miss        = {miss}");
        let mut dist: Vec<_> = cross_dist.into_iter().collect();
        dist.sort_by_key(|x| std::cmp::Reverse(x.1));
        println!(
            "  cross_top10 = {:?}",
            dist.iter().take(10).collect::<Vec<_>>()
        );
        println!("  cross_files = {}", dist.len());
        println!("  miss_sample = {:?}", miss_samples);
        println!();
    }
}
