//! Survey all sub-record tags inside every MMB chunk of a DAT.
//! Prints, per-MMB, the list of (tag, variant, count, body_len) tuples.
//! Aggregates a histogram of tags at the end.
//!
//!   FFXI_DAT_PATH=... cargo run -p ffxi-dat --example dat-mmb-tag-survey -- <file_id>

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::mmb::{MmbHeader, MmbSubRecord};
use ffxi_dat::{mmb, walk, ChunkKind, DatRoot};

fn ascii(b: &[u8]) -> String {
    b.iter()
        .map(|&c| {
            if c.is_ascii_graphic() || c == b' ' {
                c as char
            } else {
                '.'
            }
        })
        .collect()
}

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

    let mut tag_hist: BTreeMap<String, (u32, u64)> = BTreeMap::new();
    let mut total_mmb_chunks = 0u32;
    let mut total_sub_records = 0u32;
    let mut total_filtered_out = 0u32;

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
        total_mmb_chunks += 1;
        let records = MmbSubRecord::find_all(header.payload);
        if records.is_empty() {
            continue;
        }
        println!(
            "chunk[{chunk_idx}] asset={:?} sub_records={}",
            header.asset_name_str().trim(),
            records.len()
        );
        for r in &records {
            let tag_str = ascii(r.tag);
            let variant_str = ascii(r.variant_name);
            println!(
                "  tag=\"{tag_str}\" variant=\"{variant_str}\" count={} body_len={}",
                r.count,
                r.body.len()
            );
            total_sub_records += 1;
            let entry = tag_hist.entry(tag_str.clone()).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += r.body.len() as u64;
            if !r.tag.starts_with(b"model") {
                total_filtered_out += 1;
            }
        }
    }

    println!();
    println!("=== Tag histogram across {total_mmb_chunks} MMB chunks, {total_sub_records} sub-records ===");
    for (tag, (n, total_body)) in &tag_hist {
        println!("  {n:5}× \"{tag}\"   total_body_bytes={total_body}");
    }
    println!();
    println!("filtered_out_by_model_only_check = {total_filtered_out}");

    ExitCode::SUCCESS
}
