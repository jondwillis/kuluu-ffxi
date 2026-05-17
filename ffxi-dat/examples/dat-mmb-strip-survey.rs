//! For each `model` sub-record in a DAT, check whether the bytes after
//! the first strip's declared length contain more strip data (indicating
//! multi-strip layout we currently discard).
//!
//!   FFXI_DAT_PATH=... cargo run -p ffxi-dat --example dat-mmb-strip-survey -- <file_id>

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

    let mut total_model = 0u32;
    let mut multi_strip_candidates = 0u32;
    let mut extra_u16_total: u64 = 0;
    let mut extra_u16_max: usize = 0;
    let mut examples_printed = 0u32;
    let mut parse_vertices_fails: u32 = 0;
    let mut parse_strip_empty: u32 = 0;
    let mut tris_after_filter_zero: u32 = 0;

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
            total_model += 1;
            if r.parse_vertices().is_none() {
                parse_vertices_fails += 1;
            }
            let tris = r.parse_triangle_list();
            if tris.is_empty() {
                parse_strip_empty += 1;
            } else {
                let n = r.count;
                let kept = tris
                    .iter()
                    .filter(|t| (t[0] as u32) < n && (t[1] as u32) < n && (t[2] as u32) < n)
                    .count();
                if kept == 0 {
                    tris_after_filter_zero += 1;
                }
            }
            let vert_bytes = r.count as usize * 36;
            if vert_bytes >= r.body.len() {
                continue;
            }
            let leftover = &r.body[vert_bytes..];
            let leftover_u16: Vec<u16> = leftover
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            if leftover_u16.is_empty() {
                continue;
            }
            let declared = leftover_u16[0] as usize;
            let avail = leftover_u16.len() - 1;
            let extra = avail.saturating_sub(declared);
            if extra > 0 {
                multi_strip_candidates += 1;
                extra_u16_total += extra as u64;
                if extra > extra_u16_max {
                    extra_u16_max = extra;
                }
                if examples_printed < 10 {
                    examples_printed += 1;
                    let asset = header.asset_name_str().trim().to_string();
                    let variant = std::str::from_utf8(r.variant_name)
                        .unwrap_or("?")
                        .trim_end_matches('\0')
                        .to_string();
                    // Peek the first few u16s past the declared strip:
                    // is the next u16 a plausible "strip-length" (small
                    // count) or noise?
                    let peek_start = 1 + declared;
                    let peek: Vec<u16> = leftover_u16
                        .iter()
                        .skip(peek_start)
                        .take(8)
                        .copied()
                        .collect();
                    println!(
                        "chunk[{chunk_idx}] {asset}/{variant} verts={} declared={declared} avail={avail} extra={extra}  peek_past_strip={peek:?}",
                        r.count
                    );
                }
            }
        }
    }

    println!();
    println!("model sub-records scanned: {total_model}");
    println!("  parse_vertices None: {parse_vertices_fails}");
    println!("  parse_triangle_list empty: {parse_strip_empty}");
    println!("  triangles after bounds filter = 0: {tris_after_filter_zero}");
    println!("with extra u16 past first strip: {multi_strip_candidates}");
    println!("total extra u16: {extra_u16_total}");
    println!("max extra u16 in a single record: {extra_u16_max}");

    ExitCode::SUCCESS
}
