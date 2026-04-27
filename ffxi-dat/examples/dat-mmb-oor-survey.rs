use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::mmb::{MmbHeader, MmbSubRecord};
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

    let mut total_indices: u64 = 0;
    let mut total_oor: u64 = 0;
    let mut oor_value_hist: BTreeMap<u16, u64> = BTreeMap::new();
    let mut records_with_oor = 0u32;
    let mut examples = 0u32;

    for (chunk_idx, chunk) in chunks.iter().enumerate() {
        if ChunkKind::from_u8(chunk.kind) != Some(ChunkKind::Mmb) {
            continue;
        }
        let decrypted = match mmb::decrypt(chunk.data) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let header = match MmbHeader::parse(&decrypted) {
            Ok(h) => h,
            Err(_) => continue,
        };
        for r in MmbSubRecord::find_all(header.payload) {
            if !r.tag.starts_with(b"model") {
                continue;
            }
            let vert_bytes = r.count as usize * 36;
            if vert_bytes >= r.body.len() {
                continue;
            }
            let leftover = &r.body[vert_bytes..];
            let strip_all: Vec<u16> = leftover
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            if strip_all.len() < 4 {
                continue;
            }
            let declared = strip_all[0] as usize;
            let avail = strip_all.len() - 1;
            let n = declared.min(avail);
            let strip = &strip_all[1..1 + n];

            let mut oor_here = 0u64;
            for &idx in strip {
                total_indices += 1;
                if (idx as u32) >= r.count {
                    total_oor += 1;
                    oor_here += 1;
                    *oor_value_hist.entry(idx).or_insert(0) += 1;
                }
            }
            if oor_here > 0 {
                records_with_oor += 1;
                if examples < 10 {
                    examples += 1;
                    let asset = header.asset_name_str().trim().to_string();
                    let variant = std::str::from_utf8(r.variant_name)
                        .unwrap_or("?")
                        .trim_end_matches('\0')
                        .to_string();
                    println!(
                        "chunk[{chunk_idx}] {asset}/{variant} verts={} oor_count={oor_here} declared={declared}",
                        r.count
                    );
                }
            }
        }
    }

    println!();
    println!("total strip indices scanned: {total_indices}");
    println!("out-of-range indices: {total_oor}");
    println!("records with any OOR: {records_with_oor}");
    println!();
    println!("Top 20 OOR values (likely restart sentinels if concentrated):");
    let mut vec: Vec<(u16, u64)> = oor_value_hist.into_iter().collect();
    vec.sort_by_key(|x| std::cmp::Reverse(x.1));
    for (val, n) in vec.iter().take(20) {
        println!("  0x{val:04x} = {val:5}   {n}×");
    }

    ExitCode::SUCCESS
}
