//! Print-only census for the Bastok Mines AH terrace walk-off report of
//! 2026-10-26 (Garfield at xz(-24.15,+59.26) y=+3.00, panel showed floor +3.000
//! under the feet and only a phantom canopy +4.167 from 1.00 ahead): every raw
//! triangle whose world AABB touches the reported box, a compass of probe
//! columns in eight headings from the stand point, and a ground-height map of
//! the region. This test asserts nothing - it separates "the DAT has geometry
//! there and kuluu's bake/query drops it" from "the DAT genuinely has no floor
//! or wall faces past that line" before any code changes.

use bevy::math::{Vec2, Vec3};

use ffxi_dat::mzb;
use kuluu_render::dat_mzb::{
    build_collision_geometry, load_mzb_placed, MzbCollisionGeometry, ZONE_SLOT_MAIN,
};

/// Bastok Mines -> DAT 334 (zone id under 256 plus one hundred).
const BASTOK_ZONE: u16 = 234;

/// The reported stand point and the probe columns walked from it.
const REPORT_X: f32 = -24.15;
const REPORT_Z: f32 = 59.26;
const FEET_Y: f32 = 3.0;

/// Box to dump raw triangles in: the report point ± several yalms, generous
/// enough that if any floor/wall exists past the walk-off line it is inside.
const BOX_X0: f32 = -34.0;
const BOX_X1: f32 = -15.0;
const BOX_Z0: f32 = 50.0;
const BOX_Z1: f32 = 68.0;

#[test]
fn bastok_terrace_edge_raw_triangle_dump() {
    let Some(root) = ffxi_dat::archive::open_test_install() else {
        return;
    };
    bevy::tasks::AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
    let file_id = ffxi_dat::zone_dat::zone_id_to_mzb_file_id(BASTOK_ZONE).unwrap();
    let (submeshes, instances) = load_mzb_placed(&root, file_id, None).unwrap();
    let geom = MzbCollisionGeometry::from_block(build_collision_geometry(
        &submeshes,
        &instances,
        Some(file_id),
    ));
    let block = geom.block(ZONE_SLOT_MAIN);

    // tri id -> instance index: build_collision_geometry appends triangles per
    // instance in order, skipping instances whose submesh failed to resolve.
    let mut inst_of_tri: Vec<usize> = Vec::with_capacity(block.indices.len() / 3);
    for (ii, inst) in instances.iter().enumerate() {
        if let Some(sub) = submeshes.get(inst.submesh_idx) {
            inst_of_tri.resize(inst_of_tri.len() + sub.indices.len() / 3, ii);
        }
    }

    println!(
        "== zone {} DAT {} submeshes={} instances={} baked_tris={}",
        BASTOK_ZONE,
        file_id,
        submeshes.len(),
        instances.len(),
        block.indices.len() / 3
    );

    let mut dumped = 0usize;
    let mut proptris = 0usize;
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
        if max_x < BOX_X0 || min_x > BOX_X1 || max_z < BOX_Z0 || min_z > BOX_Z1 {
            continue;
        }
        dumped += 1;
        if block.tri_prop_blocker[tri_id] {
            proptris += 1;
        }
        let min_y = v[0].y.min(v[1].y).min(v[2].y);
        let max_y = v[0].y.max(v[1].y).max(v[2].y);
        let n = block.tri_normals[tri_id];
        let inst_id = *inst_of_tri.get(tri_id).unwrap_or(&usize::MAX);
        let sub_idx = instances
            .get(inst_id)
            .map(|i| i.submesh_idx)
            .unwrap_or(usize::MAX);
        let flags = submeshes.get(sub_idx).map(|s| s.flags).unwrap_or(0);
        println!(
            "tri {:6} inst{:5} sub{:4} flags={flags:#06x} camskip={} terrain={:?} prop={} \
             n=({:+.3},{:+.3},{:+.3}) y[{min_y:+8.3}..{max_y:+8.3}] \
             x[{min_x:+7.2}..{max_x:+7.2}] z[{min_z:+7.2}..{max_z:+7.2}] sub_area={}",
            tri_id,
            inst_id,
            sub_idx,
            block.camera_skip[tri_id],
            mzb::TerrainType::from_nibble(block.tri_terrain[tri_id]),
            block.tri_prop_blocker[tri_id],
            n.x,
            n.y,
            n.z,
            block.tri_sub_area[tri_id],
        );
    }
    println!(
        "== tris inside box x[{BOX_X0}..{BOX_X1}] z[{BOX_Z0}..{BOX_Z1}]: {dumped} (prop-class {proptris})"
    );

    // Compass: probe columns in eight headings, because the reported divergence
    // was along the player's heading, whose direction is not recorded.
    println!("== compass at xz({REPORT_X},{REPORT_Z}) feet y={FEET_Y}: per-heading first column with no walkable support within one follow word above/below, and where hits change");
    let diag = std::f32::consts::FRAC_1_SQRT_2;
    const DIRS: [(&str, f32, f32); 4] = [
        ("+x", 1.0, 0.0),
        ("-x", -1.0, 0.0),
        ("+z", 0.0, 1.0),
        ("-z", 0.0, -1.0),
    ];
    let dirs = DIRS
        .iter()
        .copied()
        .chain(std::iter::once(("+x+z", diag, diag)))
        .chain(std::iter::once(("+x-z", diag, -diag)))
        .chain(std::iter::once(("-x+z", -diag, diag)))
        .chain(std::iter::once(("-x-z", -diag, -diag)));
    for (name, dx, dz) in dirs {
        print!("dir {name:5} :");
        let mut prev: Option<String> = None;
        for k in 0..=32i32 {
            let along = 0.25 * k as f32;
            let xz = Vec2::new(REPORT_X + dx * along, REPORT_Z + dz * along);
            let walkable = geom.ground_walkable(xz, FEET_Y);
            let mut hits: Vec<(f32, Vec3)> = geom.ground_raycast_all(xz);
            hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            let cur = match walkable {
                Some(y) => format!("{y:+.3}"),
                None => "none".to_string(),
            };
            if prev.as_deref() != Some(cur.as_str()) {
                let all = hits
                    .iter()
                    .map(|(y, n)| format!("{y:+.3}@{:.1}", n.y))
                    .collect::<Vec<_>>()
                    .join(",");
                print!(" @{along:.2}[{cur}|{all}]");
                prev = Some(cur);
            }
        }
        println!();
    }

    // Fine walk-off scan along each heading that lost support, printing every hit
    // including its full normal, so the construction (treads / risers / slab) is visible.
    println!("== fine columns at z={REPORT_Z} walking -x, 0.1 pitch: all hits (y@nx,ny,nz)");
    for k in 0..=60i32 {
        let along = 0.1 * k as f32;
        let xz = Vec2::new(REPORT_X - along, REPORT_Z);
        let mut hits: Vec<(f32, Vec3)> = geom.ground_raycast_all(xz);
        hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        println!(
            "-{along:4.1} x={:>7.2} : {}",
            xz.x,
            hits.iter()
                .map(|(y, n)| format!("{y:+.3}@({:+.2},{:+.2},{:+.2})", n.x, n.y, n.z))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }

    println!("== fine columns from x={REPORT_X} walking +z, 0.1 pitch: all hits (y@nx,ny,nz)");
    for k in 0..=60i32 {
        let along = 0.1 * k as f32;
        let xz = Vec2::new(REPORT_X, REPORT_Z + along);
        let mut hits: Vec<(f32, Vec3)> = geom.ground_raycast_all(xz);
        hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        println!(
            "+{along:4.1} z={:>7.2} : {}",
            xz.y,
            hits.iter()
                .map(|(y, n)| format!("{y:+.3}@({:+.2},{:+.2},{:+.2})", n.x, n.y, n.z))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }

    // Face census inside the tight walk-off box: classify every face by its authored normal.
    println!("== faces with AABB inside x[-27.5..-19] z[56..63], classified by authored normal.y");
    let mut rows: Vec<String> = Vec::new();
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
        if max_x < -27.5 || min_x > -19.0 || max_z < 56.0 || min_z > 63.0 {
            continue;
        }
        let min_y = v[0].y.min(v[1].y).min(v[2].y);
        let max_y = v[0].y.max(v[1].y).max(v[2].y);
        let n = block.tri_normals[tri_id];
        let ny = n.y;
        let cls = if n.y > 0.7 {
            "tread"
        } else if n.y >= 0.5 {
            "slope"
        } else if n.y > -0.5 {
            "wall"
        } else {
            "ceil"
        };
        rows.push(format!(
            "{cls:5} ny={ny:+.2} y[{min_y:+7.3}..{max_y:+7.3}] x[{min_x:+6.2}..{max_x:+6.2}] z[{min_z:+6.2}..{max_z:+6.2}] prop={}",
            block.tri_prop_blocker[tri_id],
        ));
    }
    rows.sort();
    for r in &rows {
        println!("{r}");
    }
    println!("== tight-box faces: {}", rows.len());

    // Ground map: what the walker's selection answers on a grid around the point.
    println!("== ground_walkable(xz,{FEET_Y}) map, x -34..-15 step 1, z 50..68 step 1 ('.'=no support answer, else height)");
    println!(
        "     {}",
        (BOX_X0 as i32..=BOX_X1 as i32)
            .map(|x| format!("{: >4}", x % 10))
            .collect::<Vec<_>>()
            .join("")
    );
    for z in BOX_Z0 as i32..=BOX_Z1 as i32 {
        let mut row = String::new();
        for x in BOX_X0 as i32..=BOX_X1 as i32 {
            let h = geom.ground_walkable(Vec2::new(x as f32, z as f32), FEET_Y);
            match h {
                Some(y) => row.push_str(&format!("{:>4}", y.round() as i32)),
                None => row.push_str("   ."),
            }
        }
        println!("z{z:4} {row}");
    }

    // Column answers along +x for comparison with the panel report.
    println!("== columns along +x at z={REPORT_Z}, feet y={FEET_Y}: ground_walkable + all hits + nearest_wall_contact(r=1)");
    for k in 0..=40i32 {
        let along = 0.3 * k as f32;
        let x = REPORT_X + along;
        let xz = Vec2::new(x, REPORT_Z);
        let walkable = geom.ground_walkable(xz, FEET_Y);
        let mut hits: Vec<(f32, Vec3)> = geom.ground_raycast_all(xz);
        hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let wall =
            geom.nearest_wall_contact(Vec3::new(x, FEET_Y + 0.5, REPORT_Z), 1.0, FEET_Y, 0.0);
        println!(
            "{along:5.2} x={x:+8.2} gw={} wall={wall:?} hits[{}] : {}",
            match walkable {
                Some(y) => format!("{y:+.4}"),
                None => "none".to_string(),
            },
            hits.len(),
            hits.iter()
                .map(|(y, n)| format!("{y:+.4}@{:.2}", n.y))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
}
