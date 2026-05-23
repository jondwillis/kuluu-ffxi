//! Print a VertexOs2 chunk's bone palette and the first N
//! per-vertex bone assignments. Used to sanity-check the bone_table
//! + bone_indices parser against real DAT files.
//!
//!   cargo run -p ffxi-dat --example dat-vos2-bones -- <file_id> <chunk_idx> [n]
//!
//! Known-good target: 13746 chunk[4] (Kuu Mohzolhil body, high-LOD).

use std::env;
use std::fs;
use std::process::ExitCode;

use ffxi_dat::{vos2, walk, DatRoot};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: {} <file_id> <chunk_idx> [n]", args[0]);
        return ExitCode::from(2);
    }
    let file_id: u32 = args[1].parse().unwrap();
    let want_idx: usize = args[2].parse().unwrap();
    let n_show: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(8);

    let root = DatRoot::from_env_or_default().unwrap();
    let loc = root.resolve(file_id).unwrap();
    let bytes = fs::read(loc.path_under(root.root())).unwrap();
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
    let chunk = match chunks.get(want_idx) {
        Some(c) if c.kind == 0x2A => c,
        Some(c) => {
            eprintln!(
                "chunk[{want_idx}] is kind 0x{:02x}, not VertexOs2 (0x2A)",
                c.kind
            );
            return ExitCode::from(1);
        }
        None => {
            eprintln!("file has no chunk[{want_idx}]");
            return ExitCode::from(1);
        }
    };
    let mesh = match vos2::parse_vos2(chunk.data) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("parse failed: {e}");
            return ExitCode::from(1);
        }
    };
    let h = &mesh.header;
    println!(
        "file_id={file_id} chunk[{want_idx}] body_len={}\n\
         version={} kind_type=0x{:04x} use_bone_table={} flip=0x{:04x}\n\
         off_bone_table=0x{:x} bone_table_count={}\n\
         off_bone=0x{:x} bone_indices_count={}\n\
         vertices={} groups={}",
        chunk.data.len(),
        h.version,
        h.kind_type,
        h.use_bone_table(),
        h.flip,
        h.off_bone_table_bytes,
        h.bone_table_count,
        h.off_bone_bytes,
        h.bone_indices_count,
        mesh.vertices.len(),
        mesh.groups.len(),
    );

    if !mesh.bone_table.is_empty() {
        let head: Vec<String> = mesh
            .bone_table
            .iter()
            .take(16)
            .map(|b| b.to_string())
            .collect();
        println!(
            "bone_table[{}] head: [{}{}]",
            mesh.bone_table.len(),
            head.join(", "),
            if mesh.bone_table.len() > 16 {
                ", ..."
            } else {
                ""
            }
        );
    } else {
        println!("bone_table: (empty)");
    }

    let v_show = mesh.vertices.len().min(n_show);
    println!("\nfirst {v_show} vertices: (raw idx → skeleton idx)");
    for i in 0..v_show {
        let raw = mesh.raw_bone_index_for(i);
        let skel = mesh.skeleton_bone_for(i);
        let pos = mesh.vertices[i].pos;
        println!(
            "  vert[{i:>3}] raw={:?} skel={:?} pos=({:.3}, {:.3}, {:.3})",
            raw, skel, pos[0], pos[1], pos[2]
        );
    }
    ExitCode::SUCCESS
}
