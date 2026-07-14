use bevy::math::{Vec2, Vec3};
use ffxi_viewer_core::dat_mzb::{load_mzb_placed, MzbCollisionGeometry};

/// Pins the Mog House grounding geometry against the real Windurst MH DAT
/// (391): at the server spawn column (0,0) the interior floor sits near
/// y=0 and the model also carries a roof plane at y=5 — `ground_nearest`
/// with the wire spawn Y must pick the floor, never the roof. (The roof was
/// exactly where entities ended up when a stale previous-zone collision set
/// inflated their reference Y mid-transition.) Also documents that MH floor
/// triangles arrive with downward-facing normals, so the |normal.y| floor
/// filter is load-bearing. Skips without a retail DAT install.
#[test]
fn mh_391_spawn_column_grounds_to_interior_floor_not_roof() {
    if std::env::var("FFXI_DAT_PATH").is_err() {
        eprintln!("FFXI_DAT_PATH unset; skipping");
        return;
    }
    bevy::tasks::AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
    let (submeshes, instances) = load_mzb_placed(391, None).expect("load DAT 391");
    let mut geom = MzbCollisionGeometry::default();
    for inst in &instances {
        let sub = &submeshes[inst.submesh_idx];
        if sub.flags & 1 != 0 {
            continue;
        }
        let base = geom.positions.len() as u32;
        for v in &sub.positions {
            geom.positions.push(
                inst.bevy_transform
                    .transform_point(Vec3::new(v[0], v[1], v[2])),
            );
        }
        for &i in &sub.indices {
            geom.indices.push(i + base);
        }
    }
    assert!(geom.tri_count() > 1000, "MH collision unexpectedly small");

    let spawn = Vec2::new(0.0, 0.0);
    let hits = geom.ground_raycast_all(spawn);
    let top = hits.first().expect("no surfaces at spawn column").0;
    assert!(
        top > 4.0,
        "expected a roof plane above the interior (got top {top})"
    );

    let grounded = geom
        .ground_nearest(spawn, 0.0)
        .expect("no floor at spawn column");
    assert!(
        grounded.abs() < 0.5,
        "wire spawn Y=0 must ground to the interior floor, got {grounded}"
    );

    let stale_ref = geom.ground_nearest(spawn, 40.0).unwrap_or(f32::NAN);
    assert!(
        stale_ref > 3.0,
        "a stale high reference Y sticks to the roof band ({stale_ref}) — \
         which is why the collision set must be cleared on zone transition"
    );
}
