//! Parse-and-summarize tool for VertexOs2 chunks. Loads every Os2
//! chunk in the given file_id, runs them through `vos2::parse_vos2`,
//! and prints vertex/triangle counts plus a sanity-check on normal
//! magnitudes. Use this to confirm a parser fix against a real DAT
//! before wiring the spawn path.
//!
//!   FFXI_DAT_PATH=... cargo run -p ffxi-dat --example vos2-dump -- <file_id>
//!
//! Default file_id: 13746 (Kuu Mohzolhil body equip, race 29).

use ffxi_dat::{vos2, walk, ChunkKind, DatRoot};
use std::env;
use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    let file_id: u32 = env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(13746);

    let root = match DatRoot::from_env_or_default() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("DatRoot::from_env_or_default: {e}");
            return ExitCode::from(2);
        }
    };
    let loc = root.resolve(file_id).unwrap();
    let bytes = fs::read(loc.path_under(root.root())).unwrap();

    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
    println!("file_id={file_id}  total_chunks={}", chunks.len());

    for (idx, c) in chunks.iter().enumerate() {
        if ChunkKind::from_u8(c.kind) != Some(ChunkKind::VertexOs2) {
            continue;
        }
        println!(
            "\n=== chunk[{idx}] kind=0x{:02x} (VertexOs2) body_len={} ===",
            c.kind,
            c.data.len()
        );
        match vos2::parse_vos2(c.data) {
            Err(e) => {
                println!("  PARSE FAILED: {e}");
            }
            Ok(mesh) => {
                let total_tris: usize =
                    mesh.groups.iter().map(|g| g.triangles.len()).sum();
                println!(
                    "  header: ver={:#x} type={:#x} flip={} off_vertex={:#x} off_poly={:#x} lod2={}",
                    mesh.header.version,
                    mesh.header.kind_type,
                    mesh.header.flip,
                    mesh.header.off_vertex_bytes,
                    mesh.header.off_poly_bytes,
                    mesh.header.poly_lod2_count,
                );
                println!(
                    "  vertices: {}  groups: {}  total_triangles: {}",
                    mesh.vertices.len(),
                    mesh.groups.len(),
                    total_tris,
                );

                // Sanity-check normal magnitudes for the 1-bone vertices.
                let mut unit_count = 0;
                let mut off_count = 0;
                for v in &mesh.vertices {
                    let m =
                        (v.normal[0] * v.normal[0] + v.normal[1] * v.normal[1] + v.normal[2] * v.normal[2])
                            .sqrt();
                    if (m - 1.0).abs() < 0.05 {
                        unit_count += 1;
                    } else {
                        off_count += 1;
                    }
                }
                println!(
                    "  normal mag check: {unit_count} unit-ish, {off_count} off (off-vs-unit ratio = {:.2})",
                    off_count as f32 / mesh.vertices.len().max(1) as f32,
                );

                for (gi, g) in mesh.groups.iter().enumerate().take(4) {
                    println!(
                        "  group[{gi}]: tex='{}' triangles={}",
                        g.texture_name,
                        g.triangles.len()
                    );
                    if !g.triangles.is_empty() {
                        let t = g.triangles[0];
                        println!(
                            "    first tri: idx=({},{},{}) uv0=({:.3},{:.3})",
                            t.indices[0], t.indices[1], t.indices[2], t.uvs[0][0], t.uvs[0][1],
                        );
                    }
                }
                if mesh.groups.len() > 4 {
                    println!("  ...{} more groups", mesh.groups.len() - 4);
                }
            }
        }
    }

    ExitCode::SUCCESS
}
