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

        let has_vertex_color = meshes.iter().any(|m| {
            m.meshes.iter().any(|b| {
                b.vertices
                    .iter()
                    .any(|v| v.color != [0x80, 0x80, 0x80, 0x80])
            })
        });

        let (mut ymin, mut ymax) = (f32::INFINITY, f32::NEG_INFINITY);
        for bb in &skel.bounding_boxes {
            ymin = ymin.min(bb.y_min);
            ymax = ymax.max(bb.y_max);
        }
        let imgs = count_img(&walk_tree(&bytes));

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
