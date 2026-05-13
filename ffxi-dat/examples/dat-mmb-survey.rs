//! Survey: walk every MMB chunk in a DAT, decrypt header, print asset_name +
//! body length + sub-record variants. Used to characterize whether MMB is the
//! visual zone-mesh format.
//!
//!   FFXI_DAT_PATH=... cargo run -p ffxi-dat --example dat-mmb-survey -- <file_id>

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::mmb::{MmbHeader, MmbSubRecord};
use ffxi_dat::{mmb, walk, DatRoot};

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

    let mut total_mmb = 0usize;
    let mut total_body = 0usize;
    let mut variant_counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (idx, c) in chunks.iter().enumerate() {
        if c.kind != 0x2E {
            continue;
        }
        total_mmb += 1;
        total_body += c.data.len();
        let decrypted = match mmb::decrypt(c.data) {
            Ok(d) => d,
            Err(e) => {
                println!("[{idx}] decrypt error: {e}");
                continue;
            }
        };
        let header = match MmbHeader::parse(&decrypted) {
            Ok(h) => h,
            Err(e) => {
                println!("[{idx}] header parse error: {e}");
                continue;
            }
        };
        let records = MmbSubRecord::find_all(header.payload);
        let variants: Vec<String> = records
            .iter()
            .map(|r| {
                let v = r.variant_name_str();
                let v = v.trim_end_matches('\0').trim();
                format!("{}x{}", v, r.count)
            })
            .collect();
        for r in &records {
            let v = r
                .variant_name_str()
                .trim_end_matches('\0')
                .trim()
                .to_string();
            let entry = variant_counts.entry(v).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += r.body.len();
        }
        println!(
            "[{idx:>4}] body={:>7}  ver={}  key={:#04x}  asset={:?}  variants=[{}]",
            c.data.len(),
            header.version,
            header.key_index,
            header.asset_name_str(),
            variants.join(",")
        );
    }
    println!();
    println!("--- summary ---");
    println!("MMB chunks: {total_mmb}");
    println!("total body bytes: {total_body}");
    println!("variant frequency across all MMB chunks:");
    for (v, (count, bytes)) in &variant_counts {
        println!("  {v:>8}  records={count}  total_body={bytes}");
    }
    ExitCode::SUCCESS
}
