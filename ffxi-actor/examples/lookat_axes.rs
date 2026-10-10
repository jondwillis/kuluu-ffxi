//! What frame a shipped rig actually hands the look-at bend.
//!
//! `apply_look_bends` takes each bend bone's own pose rotation and reads forward off its local X, up
//! off its local -Y, and treats those plus their cross product as the yaw/pitch/roll axes. That is an
//! assumption about the authored joint frames; this prints it against real skeletons — bind pose, no
//! clip — so it can be confirmed or broken instead of asserted: for each file id given, the model-space
//! orientation of the bones named by the bend reference slots, and the exact axes `apply_look_bends`
//! derives from them.
//!
//! Retail never reads bone-local axes here (`FFXiMain.dll retail-2026-09`: the ellipse clamp at
//! RVA 0x2B140 first builds a basis through RVA 0x2D8141 from the reference frames fetched for slots 3
//! and 4 (RVA 0x2AC69 / RVA 0x2AC8E, both through RVA 0x2A750), then multiplies its point into that
//! basis by RVA 0x282C0 before dividing by the third component). So this is also the measurement
//! needed to judge whether a per-bone frame can match retail's at all.
//!
//! Usage: cargo run -p ffxi-actor --example lookat_axes -- <install root> <file_id ...>

use std::path::Path;
use std::process::ExitCode;

use ffxi_actor::look_bend::BEND_RECORDS_MAX;
use ffxi_actor::skeleton_instance::{pose_world, RootTransform};
use ffxi_dat::datid::DatId;
use ffxi_dat::skel::{standard_position, Skeleton};
use ffxi_dat::{walk, ChunkKind, DatRoot};
use glam::Vec3;

/// The reference slot whose point retail feeds the clamp as its second argument (`FFXiMain.dll
/// retail-2026-09` RVA 0x2AC8E fetches it and passes it at RVA 0x2AF0B). XIM names it EID_LOOK_AT;
/// kuluu has no constant for it yet because only the nameplate path uses attach slots by name.
const LOOK_AT_ATTACH_SLOT: usize = 4;

/// Reference slots worth printing: the two bend bones and the clamp's point source.
const SLOTS: [usize; 3] = [
    standard_position::NECK,
    standard_position::CHEST,
    LOOK_AT_ATTACH_SLOT,
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(install) = args.get(1) else {
        eprintln!("usage: lookat_axes <install root> <file_id ...>");
        return ExitCode::from(2);
    };
    let ids: Vec<u32> = args[2..].iter().filter_map(|a| a.parse().ok()).collect();
    if ids.is_empty() {
        eprintln!("no file ids given");
        return ExitCode::from(2);
    }
    let root = match DatRoot::open(install) {
        Ok(root) => root,
        Err(e) => {
            eprintln!("cannot open install: {e}");
            return ExitCode::from(2);
        }
    };

    let mut unresolved = 0;
    for &id in &ids {
        let Ok(loc) = root.resolve(id) else {
            println!("{id}: unresolved");
            unresolved += 1;
            continue;
        };
        let path = loc.path_under(&root);
        let Ok(bytes) = std::fs::read(&path) else {
            println!("{id}: cannot read {}", path.display());
            unresolved += 1;
            continue;
        };
        for skel in skeletons_in(&bytes) {
            dump(id, &path, &root, &skel);
        }
    }
    if unresolved == ids.len() {
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn skeletons_in(bytes: &[u8]) -> Vec<Skeleton> {
    walk(bytes)
        .flatten()
        .filter(|chunk| chunk.kind == ChunkKind::Bone as u8)
        .map(|chunk| ffxi_dat::skel::parse(DatId::from_str(&chunk.name_str()), chunk.data))
        .collect()
}

fn name_string(skel: &Skeleton) -> String {
    skel.id
        .0
        .iter()
        .map(|b| {
            if *b == 0 || *b == b' ' {
                '.'
            } else {
                *b as char
            }
        })
        .collect()
}

/// One `[f32; 3]` on one line, two decimals.
fn xyz_arr(v: [f32; 3]) -> String {
    xyz_vec(Vec3::new(v[0], v[1], v[2]))
}

fn xyz_vec(v: Vec3) -> String {
    format!("{:>7.2}{:>7.2}{:>7.2}", v.x, v.y, v.z)
}

fn xyz_precise(v: Vec3) -> String {
    format!("{:>8.3}{:>8.3}{:>8.3}", v.x, v.y, v.z)
}

fn dump(id: u32, path: &Path, root: &DatRoot, skel: &Skeleton) {
    let rel = path.strip_prefix(root.root()).unwrap_or(path);
    println!(
        "== file_id {id} {} skeleton {:<8} ({} joints)",
        rel.display(),
        name_string(skel),
        skel.joints.len()
    );

    for slot in SLOTS {
        match skel.reference_at(slot) {
            Some(r) => println!(
                "  ref slot {slot}: joint {:>3} rotation {} offset {}",
                r.index,
                xyz_arr(r.rotation),
                xyz_arr(r.position_offset),
            ),
            None => println!("  ref slot {slot}: (no such reference)"),
        }
    }

    for record in 0..BEND_RECORDS_MAX {
        let Some(limit) = skel.look_at_limit(record) else {
            continue;
        };
        println!(
            "  bend record {record}: limits ({:.3}, {:.3}) scale {:.3}{}",
            limit.x_limit,
            limit.y_limit,
            limit.scale,
            if limit.x_limit <= 0.0 || limit.y_limit <= 0.0 {
                " -> skipped, that bone does not bend"
            } else {
                ""
            }
        );
    }

    let bind = pose_world(skel, |_| None, RootTransform::identity(), &[]);
    for slot in [standard_position::NECK, standard_position::CHEST] {
        let Some(joint) = skel.reference_at(slot).map(|r| r.index) else {
            continue;
        };
        let Some(mat) = bind.get(joint).copied() else {
            continue;
        };
        println!("  slot {slot} -> joint {joint}:");
        let pivot = mat.w_axis.truncate();
        println!("    pivot {}", xyz_vec(pivot));
        for (tag, axis) in [
            ("X", mat.x_axis.truncate()),
            ("Y", mat.y_axis.truncate()),
            ("Z", mat.z_axis.truncate()),
        ] {
            println!("    bone {tag} axis  {}", xyz_precise(axis));
        }
        let forward = mat.x_axis.truncate();
        let up = -mat.y_axis.truncate();
        println!(
            "    apply_look_bends: fwd {} up {} right {}",
            xyz_vec(forward),
            xyz_vec(up),
            xyz_vec(up.cross(forward)),
        );
        if let Some(child) = (0..skel.joints.len()).find(|&j| skel.joints[j].parent == Some(joint))
        {
            println!(
                "    toward child {child} {}",
                xyz_vec(bind[child].w_axis.truncate() - pivot)
            );
        }
        let mut chain = Vec::new();
        let mut cur = Some(joint);
        while let Some(j) = cur {
            chain.push(format!(
                "{j}{}",
                skel.joints[j]
                    .parent
                    .map_or("-".to_string(), |p| format!("->{p}"))
            ));
            if chain.len() > 8 {
                break;
            }
            cur = skel.joints.get(j).and_then(|jnt| jnt.parent);
        }
        println!("    parent chain: {}", chain.join(" "));
    }

    for j in 0..skel.joints.len().min(4) {
        let joint = &skel.joints[j];
        println!(
            "  joint {j}: parent={:?} t={:>7.2}{:>7.2}{:>7.2}",
            joint.parent, joint.translation[0], joint.translation[1], joint.translation[2]
        );
    }
}
