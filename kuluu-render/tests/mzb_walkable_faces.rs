//! The walker's floor acceptance, pinned: rock-step treads steeper than cos(45°)
//! stand (the Bastok Mines ghost report), authored blocker objects never answer
//! height (their tops are not stairs). Law prose:
//! `.agents/skills/retail-observe/references/2026-10-25-collision-query.md` spec
//! sections "Face classification" and "Step up"; the shipped measurements behind
//! [`WALKABLE_FACE_UP_DOT`] are cited at the constant.

use bevy::math::{Vec2, Vec3};
use kuluu_render::dat_mzb::{
    build_collision_geometry, load_mzb_placed, MzbCollisionBlock, MzbCollisionGeometry,
};

fn geom_of(block: MzbCollisionBlock) -> MzbCollisionGeometry {
    MzbCollisionGeometry::from_block(block)
}

/// A floor quad `y0..y1` rising along +x with the given normal, optionally flagged as an
/// authored blocker object (which only [`build_collision_geometry`] would ever do).
fn sloped_floor(normal: Vec3, y_lo: f32, y_hi: f32, prop: bool) -> MzbCollisionGeometry {
    let mut block = MzbCollisionBlock::default();
    block.positions = vec![
        Vec3::new(-2.0, y_lo, -2.0),
        Vec3::new(2.0, y_hi, -2.0),
        Vec3::new(2.0, y_hi, 2.0),
        Vec3::new(-2.0, y_lo, 2.0),
    ];
    block.indices = vec![0, 1, 2, 0, 2, 3];
    block.tri_normals = vec![normal, normal];
    if prop {
        block.tri_prop_blocker = vec![true, true];
    }
    geom_of(block)
}

#[test]
fn rock_steeper_than_cos45_is_walkable_for_the_walker_but_not_a_floor_query() {
    // Bastok Mines rock-step treads measured on the shipped DAT at face-normal dot +0.56;
    // `ground_nearest`'s cos(45°)-cut view drops them (the ghost report), the walker's does not.
    let normal = Vec3::new(0.0, 0.56, -0.828).normalize();
    let g = sloped_floor(normal, 0.0, 1.0, false);
    assert!(
        g.ground_nearest(Vec2::ZERO, -0.5).is_none(),
        "ground_nearest must keep its authored cos(45°) floor cut (entity placement view)"
    );
    let h = g.ground_walkable(Vec2::ZERO, -0.5);
    assert!(
        h.is_some_and(|h| (0.2..0.8).contains(&h)),
        "rock step at n·y=0.56 must answer the walker a floor height on its ramp, got {h:?}"
    );
}

#[test]
fn authored_blocker_faces_never_answer_a_walkable_floor() {
    // A crate/planter top is flat enough for every floor query, but it was baked from an
    // authored blocker object: its height may not be stood on. Props block; their tops are
    // not stairs.
    let g = sloped_floor(Vec3::Y, 0.9, 0.9, true);
    // The column ray starts a thousand yalm up, so hits carry ~1e-4 quantization
    // (`STEP_UP_REACH_EPSILON` in `dat_mzb`).
    assert!(g
        .ground_nearest(Vec2::ZERO, 0.0)
        .is_some_and(|h| (h - 0.9).abs() < 1e-3));
    assert_eq!(g.ground_walkable(Vec2::ZERO, 0.0), None);
}

/// The Bastok Mines rock steps that motivated the rule (census box x[+26..+31] z[+13..+17]):
/// sampled columns there must answer `ground_walkable` heights the cos(45°) floor view disagrees with,
/// or a client patch moved the steps out from under this file and the pin's coordinates.
#[test]
fn shipped_bastok_rock_steps_answer_walkable_floor() {
    let Some(root) = ffxi_dat::archive::open_test_install() else {
        return;
    };
    bevy::tasks::AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
    let file_id = ffxi_dat::zone_dat::zone_id_to_mzb_file_id(234).expect("bastok mines mzb");
    let (submeshes, instances) = load_mzb_placed(&root, file_id, None).expect("load 334");
    let block = build_collision_geometry(&submeshes, &instances, Some(file_id));
    assert!(!block.indices.is_empty());
    let g = geom_of(block);

    let mut disagree = 0usize;
    for ix in 25..=32 {
        for iz in 13..=17i32 {
            let xz = Vec2::new(ix as f32 + 0.5, iz as f32 + 0.5);
            let old = g.ground_nearest(xz, -6.0);
            let walker = g.ground_walkable(xz, -6.0);
            if old != walker {
                disagree += 1;
                println!("rock column ({xz:?}): floor={old:?} walkable={walker:?}");
            }
        }
    }
    assert!(
        disagree >= 8,
        "only {disagree} of 40 sampled Bastok rock-step columns answer the walker a different \
         height than the cos(45°) floor view; the rock steps are not where the 2026-10-26 census \
         measured them"
    );
}
