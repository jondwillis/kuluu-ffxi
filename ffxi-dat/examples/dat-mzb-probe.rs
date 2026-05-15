//! Probe an MZB chunk in a DAT: decrypt, parse header, summarize meshes.
//!
//! Usage:
//!   FFXI_DAT_PATH=... cargo run -p ffxi-dat --example dat-mzb-probe -- <file_id> [chunk_idx]
//!
//! If `chunk_idx` is omitted, scans the file for the first kind=0x1C
//! (MZB) chunk and uses that.

use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::{mzb, walk, ChunkKind, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "usage: FFXI_DAT_PATH=... {} <file_id> [chunk_idx]",
            args.first().map(String::as_str).unwrap_or("dat-mzb-probe")
        );
        return ExitCode::from(2);
    }

    let file_id: u32 = match args[1].parse() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("bad file_id: {e}");
            return ExitCode::from(2);
        }
    };
    let forced_idx: Option<usize> = args.get(2).and_then(|s| s.parse().ok());

    let root = match DatRoot::from_env() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("DatRoot::from_env failed: {e}");
            return ExitCode::from(2);
        }
    };
    let location = match root.resolve(file_id) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("resolve({file_id}) failed: {e}");
            return ExitCode::from(1);
        }
    };
    let path = location.path_under(root.root());
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("read {} failed: {e}", path.display());
            return ExitCode::from(1);
        }
    };

    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();

    let (idx, chunk) = match forced_idx {
        Some(i) if i < chunks.len() => (i, &chunks[i]),
        Some(i) => {
            eprintln!(
                "chunk_idx {i} out of range (file has {} chunks)",
                chunks.len()
            );
            return ExitCode::from(1);
        }
        None => {
            // Find the first MZB.
            match chunks
                .iter()
                .enumerate()
                .find(|(_, c)| c.kind == ChunkKind::Mzb as u8)
            {
                Some((i, c)) => (i, c),
                None => {
                    eprintln!(
                        "no MZB chunk (kind 0x1C) found in file_id {file_id} ({} chunks scanned). \
                         Try a different file_id (zone-bundle dats).",
                        chunks.len()
                    );
                    return ExitCode::from(1);
                }
            }
        }
    };

    println!("file_id     {file_id}");
    println!("path        {}", path.display());
    println!(
        "chunk[{idx}]   name={:?} kind=0x{:02X} body_len={}",
        chunk.name_str(),
        chunk.kind,
        chunk.data.len()
    );
    println!();

    let (header, meshes) = match mzb::parse_all(chunk.data) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("MZB parse failed: {e}");
            return ExitCode::from(1);
        }
    };

    println!(
        "header      version=0x{:02X}  key_index=0x{:02X}  decode_length={}  node_count={}",
        header.version, header.key_index, header.decode_length, header.node_count
    );
    println!(
        "            grid={}x{}  mesh_table_offset=0x{:X}  quadtree_offset=0x{:X}",
        header.grid_width, header.grid_height, header.mesh_table_offset, header.quadtree_offset
    );
    println!(
        "            maplist_offset=0x{:X}  maplist_count={}",
        header.maplist_offset, header.maplist_count
    );
    println!();

    println!("meshes      count={}", meshes.len());
    let total_verts: usize = meshes.iter().map(|m| m.vertices.len()).sum();
    let total_norms: usize = meshes.iter().map(|m| m.normals.len()).sum();
    let total_tris: usize = meshes.iter().map(|m| m.triangles.len()).sum();
    println!("            total_vertices={total_verts}  total_normals={total_norms}  total_triangles={total_tris}");
    println!();

    for (i, m) in meshes.iter().enumerate().take(4) {
        println!(
            "mesh[{i}]   verts={}  norms={}  tris={}  flags=0x{:04X}",
            m.vertices.len(),
            m.normals.len(),
            m.triangles.len(),
            m.flags
        );
        for (j, v) in m.vertices.iter().take(4).enumerate() {
            println!(
                "    v[{j}]   ({:>10.3}, {:>10.3}, {:>10.3})",
                v.pos[0], v.pos[1], v.pos[2]
            );
        }
        for (j, t) in m.triangles.iter().take(4).enumerate() {
            println!(
                "    t[{j}]   [{}, {}, {}]  n={}",
                t[0], t[1], t[2], m.triangle_normals[j]
            );
        }
    }
    if meshes.len() > 4 {
        println!("... +{} more meshes", meshes.len() - 4);
    }

    ExitCode::SUCCESS
}
