//! Print-only census for the 2026-10-27 wall reports at Bastok Mines (zone 234,
//! DAT 334), all from live STAIR DEBUG panels:
//! - caught-on-wall: xz(-30.44,+44.15) y=-0.00, panel wall contact `n(-0.95,+0.32)`
//!   tri t17851/t17852 spanning y[0..4.2] - the body stops instead of sliding;
//! - fence beside it: xz(-45.60,-15.06) y=+3.49 and xz(-46.53,-15.16) y=+4.18 -
//!   `walls:` empty on every column while ±0.67-normal faces answer as standing
//!   height, and the pose can stand atop a structure it should never mount.
//!
//! For each spot: every raw triangle whose AABB touches the box (normal, y-range,
//! prop flag), the body-chain wall probe answers at the reported feet heights,
//! and a per-face classification trace showing exactly which branch of
//! `nearest_wall_contact` accepted or rejected each face near the chain. This
//! test asserts nothing; it separates "authored class is wrong" from "the slide
//! law is wrong" before any code changes.

use bevy::math::{Vec2, Vec3};

use kuluu_render::dat_mzb::{
    build_collision_geometry, load_mzb_placed, MzbCollisionGeometry, FLOOR_NORMAL_MIN,
    ZONE_SLOT_MAIN,
};

/// Bastok Mines -> DAT 334.
const BASTOK_ZONE: u16 = 234;

/// Chain centers and radius mirroring `walker/consts.rs` (BODY_RADIUS 0.4 at
/// LOWER/MIDDLE/UPPER offsets 0.4 / 0.9 / 1.4 above the feet) - duplicated here
/// because this crate cannot see the kuluu crate's walker module.
const BODY_R: f32 = 0.4;
const CHAIN_OFFSETS: [f32; 3] = [0.4, 0.9, 1.4];

struct Spot {
    name: &'static str,
    xz: (f32, f32),
    feet_y: f32,
}

fn bastok_block() -> Option<(
    MzbCollisionGeometry,
    Vec<kuluu_render::dat_mzb::MzbSubMesh>,
    Vec<kuluu_render::dat_mzb::MzbInstance>,
)> {
    let Some(root) = ffxi_dat::archive::open_test_install() else {
        return None; // install-gated, print-only: CI has no DATs
    };
    bevy::tasks::AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
    let file_id = ffxi_dat::zone_dat::zone_id_to_mzb_file_id(BASTOK_ZONE).unwrap();
    let (submeshes, instances) = load_mzb_placed(&root, file_id, None).unwrap();
    let geom = MzbCollisionGeometry::from_block(build_collision_geometry(
        &submeshes,
        &instances,
        Some(file_id),
    ));
    Some((geom, submeshes, instances))
}

fn dump_spot(
    geom: &MzbCollisionGeometry,
    submeshes: &[kuluu_render::dat_mzb::MzbSubMesh],
    instances: &[kuluu_render::dat_mzb::MzbInstance],
    spot: &Spot,
) {
    let block = geom.block(ZONE_SLOT_MAIN);
    let (px, pz) = spot.xz;
    let feet = spot.feet_y;
    println!(
        "\n######## {} at xz({px:+.2},{pz:+.2}) feet y={feet:+.2} ########",
        spot.name
    );

    // tri id -> instance index (build appends per instance in order).
    let mut inst_of_tri: Vec<usize> = Vec::with_capacity(block.indices.len() / 3);
    for (ii, inst) in instances.iter().enumerate() {
        if let Some(sub) = submeshes.get(inst.submesh_idx) {
            inst_of_tri.resize(inst_of_tri.len() + sub.indices.len() / 3, ii);
        }
    }

    // ---- 1. raw faces in the box ------------------------------------------
    let (bx0, bx1, bz0, bz1) = (px - 5.0, px + 5.0, pz - 5.0, pz + 5.0);
    println!("-- raw faces in x[{bx0:+.1}..{bx1:+.1}] z[{bz0:+.1}..{bz1:+.1}] --");
    let mut dumped = 0usize;
    for tri_id in 0..block.indices.len() / 3 {
        let base = tri_id * 3;
        let v = [
            block.positions[block.indices[base] as usize],
            block.positions[block.indices[base + 1] as usize],
            block.positions[block.indices[base + 2] as usize],
        ];
        let min_x = v[0].x.min(v[1].x).min(v[2].x);
        let max_x = v[0].x.max(v[1].x).max(v[2].x);
        let min_z = v[0].z.min(v[1].z).min(v[2].z);
        let max_z = v[0].z.max(v[1].z).max(v[2].z);
        if max_x < bx0 || min_x > bx1 || max_z < bz0 || min_z > bz1 {
            continue;
        }
        dumped += 1;
        let min_y = v[0].y.min(v[1].y).min(v[2].y);
        let max_y = v[0].y.max(v[1].y).max(v[2].y);
        let n = block.tri_normals[tri_id];
        let inst_id = *inst_of_tri.get(tri_id).unwrap_or(&usize::MAX);
        let sub_idx = instances
            .get(inst_id)
            .map(|i| i.submesh_idx)
            .unwrap_or(usize::MAX);
        println!(
            "tri {tri_id:6} inst{inst_id:5} sub{sub_idx:4} prop={} n=({:+.3},{:+.3},{:+.3}) \
             y[{min_y:+8.3}..{max_y:+8.3}] x[{min_x:+7.2}..{max_x:+7.2}] z[{min_z:+7.2}..{max_z:+7.2}]",
            block.tri_prop_blocker[tri_id],
            n.x,
            n.y,
            n.z,
        );
    }
    println!("== faces in box: {dumped}");

    // ---- 2. body-chain wall probe at the reported stand point --------------
    println!("-- nearest_wall_contact chain at xz({px:+.2},{pz:+.2}) feet y={feet:+.2} --");
    for off in CHAIN_OFFSETS {
        let center = Vec3::new(px, feet + off, pz);
        match geom.nearest_wall_contact(center, BODY_R, feet, 0.0) {
            Some((d2, n)) => println!(
                "  chain off{off:.1}: d={:.4} n=({:+.3},{:+.3},{:+.3})",
                d2.sqrt(),
                n.x,
                n.y,
                n.z
            ),
            None => println!("  chain off{off:.1}: none"),
        }
    }

    // ---- 3. classification trace for every face near the chain ------------
    // Mirrors `MzbCollisionBlock::nearest_wall_contact`'s branches so a panel
    // line like "walls: empty" is explained per face, not by inference. The
    // STEP_UP_REACH_EPSILON (5e-4) inside dat_mzb is private; inlined here.
    let band = feet + kuluu_render::dat_mzb::STEP_UP_ACCEPT_RISE_YAMLS + 5e-4;
    println!(
        "-- classification trace: faces with point-tri distance < {:.2} below any chain center (band ceiling {band:+.3}) --",
        BODY_R * 2.0
    );
    let mut traced = 0usize;
    for tri_id in 0..block.indices.len() / 3 {
        let base = tri_id * 3;
        let v = [
            block.positions[block.indices[base] as usize],
            block.positions[block.indices[base + 1] as usize],
            block.positions[block.indices[base + 2] as usize],
        ];
        // cheap xz reject against the point + probe reach
        let min_x = v[0].x.min(v[1].x).min(v[2].x);
        let max_x = v[0].x.max(v[1].x).max(v[2].x);
        let min_z = v[0].z.min(v[1].z).min(v[2].z);
        let max_z = v[0].z.max(v[1].z).max(v[2].z);
        if max_x < px - 2.5 || min_x > px + 2.5 || max_z < pz - 2.5 || min_z > pz + 2.5 {
            continue;
        }
        let n = block.tri_normals[tri_id];
        let prop = block.tri_prop_blocker[tri_id];
        let max_y = v[0].y.max(v[1].y).max(v[2].y);
        let min_y = v[0].y.min(v[1].y).min(v[2].y);
        if min_y > feet + 4.5 {
            continue;
        }
        traced += 1;
        let mut answers: Vec<String> = Vec::new();
        for off in CHAIN_OFFSETS {
            let center = Vec3::new(px, feet + off, pz);
            let d2 = kuluu_render::dat_mzb::point_tri_dist_sq(center, v[0], v[1], v[2]);
            if d2 >= BODY_R * BODY_R {
                continue; // out of probe reach for this center
            }
            let why = if n.y >= FLOOR_NORMAL_MIN {
                "NOT-WALL ny>=FLOOR_NORMAL_MIN"
            } else if !prop && max_y <= band {
                "SUPPRESSED top<=feet+accept"
            } else {
                "CANDIDATE"
            };
            answers.push(format!("off{off:.1} d={:.3} {why}", d2.sqrt(),));
        }
        if !answers.is_empty() {
            println!(
                "  tri {tri_id:6} prop={} n=({:+.3},{:+.3},{:+.3}) y[{min_y:+8.3}..{max_y:+8.3}] -> {}",
                prop,
                n.x,
                n.y,
                n.z,
                answers.join(" ; ")
            );
        }
    }
    println!("== faces near point traced: {traced}");

    // ---- 4. ground answers at the spot and along 8 compass rays -----------
    let xz = Vec2::new(px, pz);
    println!(
        "-- support at stand point: ground_walkable -> {:?}; column hits --",
        geom.ground_walkable(xz, feet)
    );
    for (h, ny) in geom.ground_raycast_all(xz) {
        println!("   hit {h:+8.3} @ny{ny:+.2}");
    }
    const DIAG: f32 = std::f32::consts::FRAC_1_SQRT_2;
    const DIRS: [(&str, f32, f32); 8] = [
        ("+x", 1.0, 0.0),
        ("-x", -1.0, 0.0),
        ("+z", 0.0, 1.0),
        ("-z", 0.0, -1.0),
        ("+x+z", DIAG, DIAG),
        ("+x-z", DIAG, -DIAG),
        ("-x+z", -DIAG, DIAG),
        ("-x-z", -DIAG, -DIAG),
    ];
    println!("-- compass: ground_walkable + chain wall answer at 0.5/1.0/1.5 yalms --");
    for (label, dx, dz) in DIRS {
        let mut line = format!("  {label:>4}:");
        for along in [0.5f32, 1.0, 1.5] {
            let p = Vec2::new(px + dx * along, pz + dz * along);
            let g = geom.ground_walkable(p, feet).map(|h| format!("{h:+.2}"));
            let w = CHAIN_OFFSETS.iter().find_map(|off| {
                geom.nearest_wall_contact(Vec3::new(p.x, feet + off, p.y), BODY_R, feet, 0.0)
                    .map(|(_, n)| format!("n({:+.2},{:+.2})", n.x, n.y))
            });
            line.push_str(&format!(
                " {along} g[{}] w[{}]",
                g.unwrap_or_else(|| "-".into()),
                w.unwrap_or_else(|| "-".into())
            ));
        }
        println!("{line}");
    }
}

#[test]
fn bastok_wall_slide_and_fence_census() {
    // The loader owns the install check now, so there is one skip path, not two.
    let Some((geom, submeshes, instances)) = bastok_block() else {
        return; // install-gated, print-only
    };
    let spots = [
        Spot {
            name: "BUG-A caught-on-angled-wall t17851",
            xz: (-30.44, 44.15),
            feet_y: -0.00,
        },
        Spot {
            name: "BUG-B fence beside (walls empty)",
            xz: (-45.60, -15.06),
            feet_y: 3.49,
        },
        Spot {
            name: "BUG-B atop the fence",
            xz: (-46.53, -15.16),
            feet_y: 4.18,
        },
    ];
    for spot in &spots {
        dump_spot(&geom, &submeshes, &instances, spot);
    }

    // Fine approach line toward the fence: where a pose walking -x from the reported walkway
    // stand point loses its slab support (contact-floor window), where the wall chain first
    // answers, and how far each face stands from that stop - the numbers that decide whether
    // a stopped body stays on the walkway or hangs over the edge.
    println!(
        "\n######## approach line z=-15.06 x[-45.3..-47.0] feet 3.493 (contact window: drop <=1.3, rise <=1.0) ########"
    );
    let mut x = -45.3f32;
    while x > -47.0 {
        let p = Vec2::new(x, -15.06);
        let g = geom.ground_walkable(p, 3.493);
        let in_window =
            g.is_some_and(|h| (h - 3.493).abs() <= 1.3 + 1e-6 || h - 3.493 <= 1.0 + 1e-6);
        let w = [0.4f32, 0.9, 1.4].iter().find_map(|off| {
            geom.nearest_wall_contact(Vec3::new(x, 3.493 + off, -15.06), 0.4, 3.493, 0.0)
                .map(|(d2, n)| format!("off{off:.1} d={:.3} n({:+.2},{:+.2})", d2.sqrt(), n.x, n.y))
        });
        println!(
            "  x={x:+.2} g[{}] window_support={} wall[{}]",
            g.map(|h| format!("{h:+.3}")).unwrap_or_else(|| "-".into()),
            in_window,
            w.unwrap_or_else(|| "-".into())
        );
        x -= 0.1;
    }
}
