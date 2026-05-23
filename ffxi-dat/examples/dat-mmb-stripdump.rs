//! Dump the raw triangle-strip u16 stream for one submesh of one MMB.
//! Prints the strip-length header, then every subsequent u16, with
//! out-of-range flags so we can see where real strip data ends and
//! trailing padding begins.
//!
//!   FFXI_DAT_PATH=... cargo run -p ffxi-dat --example dat-mmb-stripdump -- <file_id> <chunk_idx> <sub_idx>

use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::mmb::{MmbHeader, MmbSubRecord};
use ffxi_dat::{mmb, walk, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 4 {
        eprintln!(
            "usage: FFXI_DAT_PATH=... {} <file_id> <chunk_idx> <sub_idx>",
            args[0]
        );
        return ExitCode::from(2);
    }
    let file_id: u32 = args[1].parse().unwrap();
    let chunk_idx: usize = args[2].parse().unwrap();
    let sub_idx: usize = args[3].parse().unwrap();

    let root = DatRoot::from_env().unwrap();
    let location = root.resolve(file_id).unwrap();
    let bytes = fs::read(location.path_under(root.root())).unwrap();
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
    let decrypted = mmb::decrypt(chunks[chunk_idx].data).unwrap();
    let header = MmbHeader::parse(&decrypted).unwrap();
    let records = MmbSubRecord::find_all(header.payload);
    let r = &records[sub_idx];

    let vc = r.count as usize;
    let vert_bytes = vc * 36;
    println!(
        "asset={:?} variant={:?} verts={} body_len={} vert_bytes={} leftover={}",
        header.asset_name_str(),
        r.variant_name_str(),
        vc,
        r.body.len(),
        vert_bytes,
        r.body.len().saturating_sub(vert_bytes)
    );

    let strip = r.parse_triangle_strip();
    println!("u16 stream ({} u16s, first is header):", strip.len());
    for (i, v) in strip.iter().enumerate() {
        let oor = if *v as usize >= vc { " OOR" } else { "" };
        let tag = if i == 0 { " <header>" } else { "" };
        println!("  [{:3}] = {:5}  (0x{:04x}){tag}{oor}", i, v, v);
    }

    let tris = r.parse_triangle_list();
    println!("\ncurrent parser emits {} tris:", tris.len());
    for (i, t) in tris.iter().enumerate() {
        println!("  tri[{i}] = ({}, {}, {})", t[0], t[1], t[2]);
    }
    ExitCode::SUCCESS
}
