//! Which side of the doorway each leaf of a `_`/`@` door group swings to, measured
//! through the same `ZoneDoorLeaf::posed_transform` the renderer uses.

use bevy::math::{Vec3, Vec4Swizzles};
use ffxi_dat::chunk::{walk_tree, ChunkNode};
use ffxi_dat::kind::ChunkKind;
use ffxi_dat::mzb;
use ffxi_dat::scheduler::{Scheduler, StageKind};
use ffxi_dat::DatRoot;
use kuluu_render::zone_doors::{DoorPose, ZoneDoorLeaf};
use std::collections::HashMap;

fn door_routines(bytes: &[u8]) -> HashMap<u32, Vec<Scheduler>> {
    fn walk(node: &ChunkNode<'_>, out: &mut HashMap<u32, Vec<Scheduler>>) {
        for child in &node.children {
            if child.children.is_empty() {
                continue;
            }
            let mut routines = Vec::new();
            for entry in &child.children {
                let c = &entry.chunk;
                if ChunkKind::from_u8(c.kind) == Some(ChunkKind::Scheduler) {
                    if let Ok(s) = Scheduler::parse_in_dir(child.chunk.name, c.name, c.data) {
                        routines.push(s);
                    }
                }
            }
            if routines
                .iter()
                .any(|s| &s.name == b"open" || &s.name == b"clos")
            {
                out.insert(u32::from_le_bytes(child.chunk.name), routines);
            }
            walk(child, out);
        }
    }
    let mut out = HashMap::new();
    walk(&walk_tree(bytes), &mut out);
    out
}

/// Each door panel reaches from its own hinge toward its partner's, so the
/// hinge-to-hinge axis stands in for the free edge and its perpendicular is
/// the doorway's through-direction.
fn main() {
    let root = DatRoot::from_env_or_default().expect("FFXI_DAT_PATH");
    let only: Option<u32> = std::env::args().nth(1).and_then(|a| a.parse().ok());
    let zones: Vec<u32> = match only {
        Some(id) => vec![id],
        None => (0u16..=299)
            .filter_map(ffxi_dat::zone_dat::zone_id_to_mzb_file_id)
            .collect(),
    };

    let mut opposite_sides = 0usize;
    let mut same_side = 0usize;

    for file_id in zones {
        let Ok(loc) = root.resolve(file_id) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            continue;
        };
        let routines = door_routines(&bytes);
        if routines.is_empty() {
            continue;
        }
        let Some(chunk) = ffxi_dat::walk(&bytes)
            .filter_map(Result::ok)
            .find(|c| c.kind == ChunkKind::Mzb as u8)
        else {
            continue;
        };
        let Ok(plain) = mzb::decrypt(chunk.data) else {
            continue;
        };
        let Ok(header) = mzb::MzbHeader::parse(&plain) else {
            continue;
        };
        let Ok(placements) = mzb::parse_mmb_placements(&plain, &header) else {
            continue;
        };

        for group in mzb::underscore_at_groups(&placements) {
            if group.subchunks.len() != 2 {
                continue;
            }
            let Some(open) = routines
                .get(&group.four_cc)
                .and_then(|rs| rs.iter().find(|s| &s.name == b"open"))
            else {
                continue;
            };
            let mut delta: HashMap<u32, Vec3> = HashMap::new();
            for st in &open.stages {
                if st.stage.kind != StageKind::ModelRotation {
                    continue;
                }
                if let Some(m) = st.stage.model_transform {
                    delta.insert(m.subchunk, Vec3::from_array(m.final_value));
                }
            }
            if delta.len() != 2 {
                continue;
            }

            let leaves: Vec<ZoneDoorLeaf> = group
                .subchunks
                .iter()
                .enumerate()
                .map(|(slot, &idx)| ZoneDoorLeaf::new(slot as u32, &placements[idx]))
                .collect();
            let hinges: Vec<Vec3> = leaves
                .iter()
                .map(|l| l.posed_transform(DoorPose::default()).w_axis.xyz())
                .collect();

            let axis = (hinges[1] - hinges[0]).normalize_or_zero();
            if axis == Vec3::ZERO {
                continue;
            }
            let through = axis.cross(Vec3::Y).normalize_or_zero();

            let mut push = [0.0f32; 2];
            for (slot, leaf) in leaves.iter().enumerate() {
                let toward_partner = if slot == 0 { axis } else { -axis };
                let edge = hinges[slot] + toward_partner;
                let swung = leaf
                    .posed_transform(DoorPose {
                        rotation: delta[&(slot as u32)],
                        ..Default::default()
                    })
                    .transform_point3(
                        leaf.posed_transform(DoorPose::default())
                            .inverse()
                            .transform_point3(edge),
                    );
                push[slot] = (swung - edge).dot(through);
            }

            let opposite = push[0] * push[1] < 0.0;
            if opposite {
                opposite_sides += 1;
            } else {
                same_side += 1;
            }
            let authored: Vec<f32> = (0..2).map(|s| delta[&s].y.to_degrees()).collect();
            let scales: Vec<[f32; 3]> = group
                .subchunks
                .iter()
                .map(|&i| placements[i].scale)
                .collect();
            println!(
                "dat {file_id} {} open.y_deg {:?} scale {:?} push {:+.2} {:+.2} -> {}",
                String::from_utf8_lossy(&group.four_cc_bytes()),
                authored,
                scales,
                push[0],
                push[1],
                if opposite {
                    "OPPOSITE SIDES (one in, one out)"
                } else {
                    "same side (both swing out together)"
                }
            );
        }
    }

    println!("\ntotals: same_side {same_side} opposite_sides {opposite_sides}");
}
