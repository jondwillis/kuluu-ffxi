//! Throwaway scan: probe NPC model ids for skeleton + mesh content, report
//! joint count, mesh-buffer count, occlude types, whether any buffer carries
//! a vertex-color (untextured) opcode, and the skeleton bounding-box Y extent.
//! Used to locate creature NPCs for Phase 3 visual verification: a bee-type
//! (textured wings, e.g. modelid 272 / NPC 1572) and a worm-type (low, sits
//! near floor). NOTE: a `vcolor=true` buffer with `tex=[]` is NOT a bee — it's
//! a particle-emitter model (e.g. the elementals at modelid 8..15, whose body
//! is a particle effect with sub-millimeter emitter triangles), which the
//! skinned-mesh path renders near-blank by design.
//!
//! Usage: cargo run -p ffxi-viewer-core --example actor-scan -- <lo_modelid> <hi_modelid>

use ffxi_dat::resource_dir::ResourceDir;
use ffxi_dat::{walk_tree, ChunkKind, ChunkNode, DatRoot};
use ffxi_viewer_core::look_resolver::npc_dat_id;
use std::env;
use std::fs;

fn count_img(node: &ChunkNode<'_>) -> usize {
    let mut n = 0;
    if ChunkKind::from_u8(node.chunk.kind) == Some(ChunkKind::Img) {
        n += 1;
    }
    for c in &node.children {
        n += count_img(c);
    }
    n
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let lo: u16 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(900);
    let hi: u16 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1000);

    let root = DatRoot::from_env_or_default().expect("DatRoot");

    for modelid in lo..=hi {
        let file_id = npc_dat_id(modelid);
        let Ok(loc) = root.resolve(file_id) else {
            continue;
        };
        let Ok(bytes) = fs::read(loc.path_under(root.root())) else {
            continue;
        };
        let dir = ResourceDir::from_bytes(bytes.clone());
        let skels = dir.collect_skeletons();
        let meshes = dir.collect_skel_meshes();
        if skels.is_empty() || meshes.is_empty() {
            continue;
        }
        let skel = &skels[0];
        let total_buffers: usize = meshes.iter().map(|m| m.meshes.len()).sum();
        let occlude: Vec<u8> = meshes.iter().map(|m| m.occlude_type).collect();
        // Vertex-color opcode -> a buffer whose every vertex shares a non-half
        // color (0x80 default). Detect any buffer carrying a non-0x80 color.
        let has_vertex_color = meshes.iter().any(|m| {
            m.meshes.iter().any(|b| {
                b.vertices
                    .iter()
                    .any(|v| v.color != [0x80, 0x80, 0x80, 0x80])
            })
        });
        // Skeleton bounding-box Y extent (post-axis space).
        let (mut ymin, mut ymax) = (f32::INFINITY, f32::NEG_INFINITY);
        for bb in &skel.bounding_boxes {
            ymin = ymin.min(bb.y_min);
            ymax = ymax.max(bb.y_max);
        }
        let imgs = count_img(&walk_tree(&bytes));
        // Sample a few unique texture names to recognize creature families.
        let mut names: Vec<String> = meshes
            .iter()
            .flat_map(|m| m.meshes.iter())
            .map(|b| b.texture_name.trim_end_matches(['\0', ' ']).to_string())
            .filter(|s| !s.is_empty())
            .collect();
        names.sort();
        names.dedup();
        names.truncate(4);
        println!(
            "modelid={modelid:>4} file={file_id:>6} joints={:>3} buffers={total_buffers:>3} imgs={imgs:>2} vcolor={has_vertex_color} occlude={occlude:?} tex={names:?}",
            skel.joints.len(),
        );
        let _ = (ymin, ymax);
    }
}
