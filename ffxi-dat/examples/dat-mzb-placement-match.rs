use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::mmb::{self as mmb_lib, MmbHeader};
use ffxi_dat::{mzb, walk, ChunkKind, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let file_id: u32 = args[1].parse().unwrap();
    let root = DatRoot::from_env().unwrap();
    let location = root.resolve(file_id).unwrap();
    let bytes = fs::read(location.path_under(root.root())).unwrap();
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();

    let mut mmb_by_name: HashMap<String, usize> = HashMap::new();
    for (idx, c) in chunks.iter().enumerate() {
        if c.kind != 0x2E {
            continue;
        }
        let dec = match mmb_lib::decrypt(c.data) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let hdr = match MmbHeader::parse(&dec) {
            Ok(h) => h,
            Err(_) => continue,
        };
        mmb_by_name.insert(hdr.asset_name_str().trim_end().to_string(), idx);
    }
    println!("MMBs in file: {}", mmb_by_name.len());

    let (_, mzb_chunk) = chunks
        .iter()
        .enumerate()
        .find(|(_, c)| c.kind == ChunkKind::Mzb as u8)
        .unwrap();
    let body = mzb::decrypt(mzb_chunk.data).unwrap();
    let header = mzb::MzbHeader::parse(&body).unwrap();
    let count = header.node_count as usize;

    let mut placements: BTreeMap<String, usize> = BTreeMap::new();
    let mut sample_unmatched: Vec<(String, [f32; 3])> = Vec::new();
    let mut matched_count = 0usize;

    for i in 0..count {
        let off = 0x20 + i * 100;
        if off + 100 > body.len() {
            break;
        }
        let rec = &body[off..off + 100];
        let name: String = rec[..16]
            .iter()
            .map(|&b| b as char)
            .take_while(|&c| c != '\0' && c != ' ')
            .collect();
        let trans = [
            f32::from_le_bytes([rec[16], rec[17], rec[18], rec[19]]),
            f32::from_le_bytes([rec[20], rec[21], rec[22], rec[23]]),
            f32::from_le_bytes([rec[24], rec[25], rec[26], rec[27]]),
        ];
        *placements.entry(name.clone()).or_insert(0) += 1;

        let prefixed = format!("tshimono{}", name);
        let mut truncated16 = prefixed.clone();
        truncated16.truncate(16);
        if mmb_by_name.contains_key(&name)
            || mmb_by_name.contains_key(&prefixed)
            || mmb_by_name.contains_key(&truncated16)
        {
            matched_count += 1;
        } else if sample_unmatched.len() < 10 {
            sample_unmatched.push((name, trans));
        }
    }
    println!("placements parsed: {count}");
    println!("unique names: {}", placements.len());
    println!("placements matched to MMB by name: {matched_count} / {count}");

    let mut by_freq: Vec<_> = placements.iter().collect();
    by_freq.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    println!();
    println!("top 10 most-instanced placement names:");
    for (n, c) in by_freq.iter().take(10) {
        let mm = mmb_by_name.contains_key(*n);
        println!("  {n:<18}  x{c:<5}  mmb_match={mm}");
    }

    println!();
    println!("sample unmatched placements:");
    for (n, t) in &sample_unmatched {
        println!(
            "  name={:?}  trans=({:>8.2},{:>8.2},{:>8.2})",
            n, t[0], t[1], t[2]
        );
    }

    println!();
    println!("sample MMB names:");
    let mut mmb_names: Vec<_> = mmb_by_name.keys().collect();
    mmb_names.sort();
    for n in mmb_names.iter().take(15) {
        println!("  {n:?}");
    }

    ExitCode::SUCCESS
}
