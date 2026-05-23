//! Dump MZB header + first few placement records for a DAT. Helps
//! diagnose whether `node_count` covers the full visual placement set
//! or only part of it.
//!
//!   FFXI_DAT_PATH=... cargo run -p ffxi-dat --example dat-mzb-header-dump -- <file_id>

use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::{mzb, walk, ChunkKind, DatRoot};

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

    let (chunk_idx, mzb_chunk) = chunks
        .iter()
        .enumerate()
        .find(|(_, c)| c.kind == ChunkKind::Mzb as u8)
        .expect("no MZB chunk");
    let plain = mzb::decrypt(mzb_chunk.data).unwrap();
    let header = mzb::MzbHeader::parse(&plain).unwrap();

    println!(
        "DAT {file_id} MZB at chunk[{chunk_idx}] body_len={}",
        plain.len()
    );
    println!("  decode_length      {}", header.decode_length);
    println!("  node_count         {}", header.node_count);
    println!("  version            {}", header.version);
    println!("  key_index          {}", header.key_index);
    println!("  grid_width         {}", header.grid_width);
    println!("  grid_height        {}", header.grid_height);
    println!("  mesh_table_offset  0x{:08x}", header.mesh_table_offset);
    println!("  quadtree_offset    0x{:08x}", header.quadtree_offset);
    println!("  maplist_offset     0x{:08x}", header.maplist_offset);
    println!("  maplist_count      {}", header.maplist_count);
    println!();
    let placements_size = (header.node_count as usize) * 100;
    let placements_end = 0x20 + placements_size;
    println!(
        "placement table: 0x20..0x{:x}  ({} bytes, stride 100)",
        placements_end, placements_size
    );
    println!(
        "byte gap to mesh_table: {} bytes",
        (header.mesh_table_offset as i64) - (placements_end as i64)
    );
    println!(
        "byte gap to quadtree:   {} bytes",
        (header.quadtree_offset as i64) - (placements_end as i64)
    );
    println!(
        "byte gap to maplist:    {} bytes",
        (header.maplist_offset as i64) - (placements_end as i64)
    );
    println!();

    // Dump the bytes immediately after the placement table to see if
    // there's a hidden second placement block.
    if placements_end < plain.len() {
        let peek_len = 256.min(plain.len() - placements_end);
        println!(
            "first {peek_len} bytes after placement table (offset 0x{:x}):",
            placements_end
        );
        for chunk in plain[placements_end..placements_end + peek_len].chunks(16) {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
            let ascii: String = chunk
                .iter()
                .map(|&b| {
                    if b.is_ascii_graphic() || b == b' ' {
                        b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            println!("  {}  |{}|", hex.join(" "), ascii);
        }
    }

    ExitCode::SUCCESS
}
