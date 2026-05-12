//! Probe an MZB chunk's grid placements: how many cells are populated,
//! how many total placements, how many unique mesh templates.
//!
//! Usage:
//!   FFXI_DAT_PATH=... cargo run -p ffxi-dat --example dat-mzb-placements -- <file_id> [chunk_idx]

use std::collections::HashMap;
use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::{mzb, walk, ChunkKind, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "usage: FFXI_DAT_PATH=... {} <file_id> [chunk_idx]",
            args.first().map(String::as_str).unwrap_or("dat-mzb-placements")
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
            eprintln!("chunk_idx {i} out of range ({} chunks)", chunks.len());
            return ExitCode::from(1);
        }
        None => match chunks.iter().enumerate().find(|(_, c)| c.kind == ChunkKind::Mzb as u8) {
            Some((i, c)) => (i, c),
            None => {
                eprintln!("no MZB chunk in file_id {file_id}");
                return ExitCode::from(1);
            }
        },
    };

    println!("file_id     {file_id}");
    println!("path        {}", path.display());
    println!("chunk[{idx}]   kind=0x{:02X} body_len={}", chunk.kind, chunk.data.len());
    println!();

    let plain = match mzb::decrypt(chunk.data) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("MZB decrypt failed: {e}");
            return ExitCode::from(1);
        }
    };
    let header = match mzb::MzbHeader::parse(&plain) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("MZB header parse failed: {e}");
            return ExitCode::from(1);
        }
    };
    println!(
        "header      grid={}x{} (×10 cells = {}x{}={} cells)  mesh_table=0x{:X}",
        header.grid_width,
        header.grid_height,
        header.grid_width as usize * 10,
        header.grid_height as usize * 10,
        (header.grid_width as usize * 10) * (header.grid_height as usize * 10),
        header.mesh_table_offset,
    );

    let placements = match mzb::parse_placements(&plain, &header) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("MZB parse_placements failed: {e}");
            return ExitCode::from(1);
        }
    };
    println!();
    println!("placements  total={}", placements.len());
    let mut by_geo: HashMap<u32, usize> = HashMap::new();
    let mut populated_cells: HashMap<(u16, u16), usize> = HashMap::new();
    let mut flip_count = 0usize;
    let mut nolos = 0usize;
    for p in &placements {
        *by_geo.entry(p.geometry_offset).or_default() += 1;
        *populated_cells.entry((p.grid_x, p.grid_y)).or_default() += 1;
        if p.flip_winding {
            flip_count += 1;
        }
        if p.doesnt_block_los {
            nolos += 1;
        }
    }
    println!(
        "            unique_meshes={}  populated_cells={}  flip_winding={}  doesnt_block_los={}",
        by_geo.len(),
        populated_cells.len(),
        flip_count,
        nolos,
    );

    for (i, p) in placements.iter().take(4).enumerate() {
        let t = &p.transform;
        println!(
            "  [{i}]  cell=({:2},{:2})  geo=0x{:X}  trans=({:>8.1},{:>8.1},{:>8.1})  flip={}  noLoS={}",
            p.grid_x, p.grid_y, p.geometry_offset, t[12], t[13], t[14],
            p.flip_winding, p.doesnt_block_los,
        );
    }
    if placements.len() > 4 {
        println!("  ... +{} more", placements.len() - 4);
    }

    ExitCode::SUCCESS
}
