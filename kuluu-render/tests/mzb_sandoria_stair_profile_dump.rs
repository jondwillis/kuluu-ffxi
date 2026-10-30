//! Print-only profile of the Northern San d'Oria city staircase (the pinned
//! flight class in `mzb_shipped_step_classes.rs`) for the 2026-10-27 "player
//! jacks up and down on stairs while retail slopes" report: per corridor slab,
//! every face in authored form - normal, extents, prop flag - so a step-across
//! can be reconstructed as the retail machine (record
//! `.agents/skills/retail-observe/references/2026-10-25-collision-query.md`,
//! "Movement-time collision: the character sweep") would see it, beside kuluu's
//! walkable-floor answers at every half-yalm column. Asserts nothing; needs the
//! retail DAT install (returns silently without one).

use bevy::math::Vec2;

use kuluu_render::dat_mzb::{build_collision_geometry, load_mzb_placed, MzbCollisionGeometry};

/// Northern San d'Oria -> DAT 331.
const SANDORIA_ZONE: u16 = 231;

/// Corridor centre lines from `mzb_shipped_step_classes.rs` (STAIR_CORRIDOR_Z).
const CORRIDORS_Z: [f32; 3] = [-74.666_664, -78.0, -81.333_336];

/// Slab half-width around each corridor centre line (the census band), and the
/// sampled x span from below the foot tread to past the crest.
const SLAB_HALF_WIDTH_Z: f32 = 0.5;
const X_FOOT: f32 = -203.0;
const X_TOP: f32 = -190.0;
/// Column sample pitch along the climb, in yalm.
const SAMPLE_PITCH_XAMLS: f32 = 0.5;

#[test]
fn sandoria_city_stair_profile_dump() {
    let Some(root) = ffxi_dat::archive::open_test_install() else {
        return;
    };
    bevy::tasks::AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
    let file_id = ffxi_dat::zone_dat::zone_id_to_mzb_file_id(SANDORIA_ZONE).unwrap();
    let (submeshes, instances) = load_mzb_placed(&root, file_id, None).unwrap();
    let block = build_collision_geometry(&submeshes, &instances, Some(file_id));
    println!(
        "== zone {SANDORIA_ZONE} DAT {file_id} submeshes={} instances={} baked_tris={}",
        submeshes.len(),
        instances.len(),
        block.indices.len() / 3,
    );

    for (ci, z_c) in CORRIDORS_Z.iter().enumerate() {
        println!(
            "== corridor {ci} slab x[{X_FOOT}..{X_TOP}] z[{:.2}..{:.2}]",
            *z_c - SLAB_HALF_WIDTH_Z,
            *z_c + SLAB_HALF_WIDTH_Z
        );
        let mut faces = 0usize;
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
            if max_x < X_FOOT
                || min_x > X_TOP
                || max_z < *z_c - SLAB_HALF_WIDTH_Z
                || min_z > *z_c + SLAB_HALF_WIDTH_Z
            {
                continue;
            }
            faces += 1;
            let n = block.tri_normals[tri_id];
            println!(
                "face x[{min_x:+7.2}..{max_x:+7.2}] y[{lo_y:+8.3}..{hi_y:+8.3}] \
                 z[{min_z:+7.2}..{max_z:+7.2}] n=({nx:+.4},{ny:+.4},{nz:+.4}) prop={prop}",
                lo_y = v[0].y.min(v[1].y).min(v[2].y),
                hi_y = v[0].y.max(v[1].y).max(v[2].y),
                nx = n.x,
                ny = n.y,
                nz = n.z,
                prop = block.tri_prop_blocker[tri_id],
            );
        }
        println!("== corridor {ci}: {faces} faces");

        // Half-yalm walkable-floor answers along the climb. `ground_walkable`
        // is height-anchored per call (the caller's current floor), so print
        // one sweep anchored from a plausible ascending feet line: start at the
        // westmost column's answer and carry it east, rising only to an answer
        // within the recorded acceptance window - kuluu facts, no new logic.
        let geom = MzbCollisionGeometry::from_block(block.clone());
        println!(
            "== corridor {ci} columns z={z_c:.3}: x -> walkable(x|feet), delta vs carried feet"
        );
        let mut feet: Option<f32> = None;
        let mut x = X_FOOT;
        while x <= X_TOP + 1e-3 {
            let probe_from = feet.unwrap_or(-8.0);
            let answer = geom.ground_walkable(Vec2::new(x, *z_c), probe_from);
            match (answer, feet) {
                // Carry the ascending line: accept a floor at or above the
                // carried one; hold it otherwise (print-only descent tracking).
                (Some(h), prev) => {
                    let new_feet = h.max(prev.unwrap_or(f32::MIN));
                    println!(
                        "col x{x:+7.2} feet{probe_from:+8.3} -> walkable {h:+8.3} (dh {dh:+6.3})",
                        dh = h - probe_from,
                    );
                    let _ = new_feet; // explicit: never lower a carried climb line here
                }
                (None, Some(p)) => println!("col x{x:+7.2} feet{p:+8.3} -> none"),
                (None, None) => println!("col x{x:+7.2} no answer from -8"),
            }
            if let Some(h) = answer {
                if feet.is_none() || h > feet.unwrap_or(f32::MIN) {
                    feet = Some(h);
                }
            }
            x += SAMPLE_PITCH_XAMLS;
        }
    }
}
