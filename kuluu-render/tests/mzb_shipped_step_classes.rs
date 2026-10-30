//! Pinned shipped-data step classes (install-gated).
//!
//! These tests measure the retail DATs directly and pin what the 2026-10-25 census found, so a client patch that
//! moves a class the walker law depends on fails here instead of stranding players mid-staircase. Law-side prose:
//! `.agents/skills/retail-observe/references/2026-10-25-collision-query.md`, spec section "Shipped-zone step
//! classes". The stall report these measurements settle is Northern San d'Oria's city staircase, whose risers
//! exceed kuluu's old column-probe rise bound; the vertical machine (the follow word) and its wall-band suppression
//! (`kuluu-render/src/dat_mzb.rs` `nearest_wall_contact`) are what climb them.

use bevy::math::{Vec2, Vec3};

use kuluu_render::dat_mzb::{
    build_collision_geometry, load_mzb_placed, MzbCollisionBlock, MzbInstance, MzbSubMesh,
    FLOOR_NORMAL_MIN, FOLLOW_WORD_YAMLS_PER_FRAME, PROP_OBJECT_MAX_EXTENT_YAMLS, ZONE_SLOT_MAIN,
};

/// Northern San d'Oria -> DAT 331 (zone id under 256 plus one hundred).
const SANDORIA_ZONE: u16 = 231;
/// Bastok Mines -> DAT 334.
const BASTOK_ZONE: u16 = 234;

/// The three parallel stair corridors of the pinned San d'Oria staircase, world z at each corridor's centre line
/// (the lines column probes were walked along): minus seventy-four-and-two-thirds, minus seventy-eight, minus
/// eighty-one-and-a-third. x runs from minus two hundred to minus one hundred ninety-two; in every corridor the
/// tread tops step up half a yalm per one-yalm move, from top -7.5 near the west end to -4 at the crest.
const STAIR_CORRIDOR_Z: [f32; 3] = [-74.666_664, -78.0, -81.333_336];
/// Half-width of the riser-count band around a corridor centre line (measured 2026-10-25 over x minus two hundred
/// to minus one hundred ninety, z minus eighty-four to minus seventy-two): every corridor's own risers stand on
/// wall lines within two-thirds of its centre - the middle corridor takes them only from one side - and the next
/// terrace line up sits one and two-thirds out, so this band admits no other corridor's nearest line.
const CORRIDOR_HALF_WIDTH_Z: f32 = 1.0;
/// Westmost riser plane to eastmost tread of the stair (inclusive sample span).
const STAIR_X_FOOT: f32 = -200.0;
const STAIR_X_TOP: f32 = -192.0;
/// Tread-top height at the westmost sampled tread and at the crest, measured 2026-10-25 by column probes along
/// each corridor. The sample span starts on the first tread (top -7.5) just east of the westmost riser plane at
/// x minus two hundred; the approach ground below it lies further west and is outside this pin.
const STAIR_FOOT_TREAD_Y: f32 = -7.5;
const STAIR_TOP_TREAD_Y: f32 = -4.0;
/// The pinned riser height of the city stair class, in yalm.
const PINNED_RISER_HEIGHT: f32 = 0.5;

/// Pin tolerance on the measured half-yalm riser and one-yalm tread class. Loose against float noise in the DAT
/// decode, tight enough that a re-authored stair class (different rise) fails loudly.
const RISER_PIN_TOLERANCE: f32 = 0.05;

/// Per-corridor pin: this many half-yalm riser faces inside one corridor band keeps triangulation churn and small
/// edits tolerable while losing whole risers fails. Measured class shows eight-plus per corridor (riser planes at
/// each integer x from minus two hundred to minus one hundred ninety-three, some split into multiple triangles).
const MIN_PINNED_RISERS_PER_CORRIDOR: usize = 8;

fn install_task_pool() {
    bevy::tasks::AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
}

/// Loads a shipped zone's collision submeshes and their placed instances. Requires the retail DAT install; every
/// caller opens it through `ffxi_dat::archive::open_test_install` first, so the expect is unreachable without one.
fn load_zone(root: &ffxi_dat::DatRoot, zone: u16) -> (Vec<MzbSubMesh>, Vec<MzbInstance>) {
    let file_id = zone_file_id(zone);
    load_mzb_placed(root, file_id, None).expect("shipped zone MZB loads")
}

fn zone_file_id(zone: u16) -> u32 {
    ffxi_dat::zone_dat::zone_id_to_mzb_file_id(zone).expect("shipped outdoor zone id")
}

/// Local bounding box of a submesh - the space `build_collision_geometry` judges authored-blocker identity in, so
/// every placed instance of a type carries it.
fn local_bbox(sub: &MzbSubMesh) -> Option<(Vec3, Vec3)> {
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for p in &sub.positions {
        let p = Vec3::from_array(*p);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    if lo.is_finite() && hi.is_finite() {
        Some((lo, hi))
    } else {
        None
    }
}

/// True when the submesh fits `build_collision_geometry`'s authored-blocker bound on every axis - the same test
/// that bakes tri_prop_blocker identity per face there.
fn fits_prop_bound(sub: &MzbSubMesh) -> bool {
    if sub.indices.is_empty() {
        return false;
    }
    local_bbox(sub).is_some_and(|(lo, hi)| (hi - lo).max_element() <= PROP_OBJECT_MAX_EXTENT_YAMLS)
}

/// True when any triangle of the submesh carries a wall-class authored normal (mesh-local), the census's criterion
/// for "this mesh blocks things": neither floor nor ceiling at `FLOOR_NORMAL_MIN`.
fn wall_bearing(sub: &MzbSubMesh) -> bool {
    let tris = sub.indices.len() / 3;
    (0..tris).any(|t| {
        let n = sub
            .tri_normal
            .get(t)
            .copied()
            .map(Vec3::from_array)
            .unwrap_or(Vec3::ZERO);
        n.y < FLOOR_NORMAL_MIN && n.y > -FLOOR_NORMAL_MIN
    })
}

/// Faces baked from bound-fitting submeshes (whether or not those faces are wall-class).
fn flagged_face_total(submeshes: &[MzbSubMesh], instances: &[MzbInstance]) -> usize {
    let fitting: Vec<bool> = submeshes.iter().map(fits_prop_bound).collect();
    instances
        .iter()
        .filter(|i| fitting.get(i.submesh_idx).copied().unwrap_or(false))
        .map(|i| submeshes[i.submesh_idx].indices.len() / 3)
        .sum()
}

/// Placed-instance counts per bound-fitting wall-bearing submesh type.
fn repeated_small_types(
    submeshes: &[MzbSubMesh],
    instances: &[MzbInstance],
) -> Vec<(usize, usize)> {
    let fitting: Vec<bool> = submeshes.iter().map(fits_prop_bound).collect();
    let mut counts: Vec<(usize, usize)> = Vec::new();
    for inst in instances {
        if !fitting.get(inst.submesh_idx).copied().unwrap_or(false) {
            continue;
        }
        if !wall_bearing(&submeshes[inst.submesh_idx]) {
            continue;
        }
        if let Some(entry) = counts.iter_mut().find(|(si, _)| *si == inst.submesh_idx) {
            entry.1 += 1;
        } else {
            counts.push((inst.submesh_idx, 1));
        }
    }
    counts
}

/// Wall-class baked faces inside a corridor band whose height sits within tolerance of the pinned half-yalm rise,
/// as (face index, centroid).
fn pinned_riser_faces(block: &MzbCollisionBlock, z: f32) -> Vec<(usize, Vec3)> {
    let mut out = Vec::new();
    for t in 0..block.indices.len() / 3 {
        let base = t * 3;
        let v = [
            block.positions[block.indices[base] as usize],
            block.positions[block.indices[base + 1] as usize],
            block.positions[block.indices[base + 2] as usize],
        ];
        let c = (v[0] + v[1] + v[2]) / 3.0;
        if c.x < STAIR_X_FOOT - 0.5
            || c.x > STAIR_X_TOP + 0.5
            || (c.z - z).abs() > CORRIDOR_HALF_WIDTH_Z
        {
            continue;
        }
        let n = block.tri_normals.get(t).copied().unwrap_or(Vec3::ZERO);
        if !(n.y < FLOOR_NORMAL_MIN && n.y > -FLOOR_NORMAL_MIN) {
            continue;
        }
        let ymin = v[0].y.min(v[1].y.min(v[2].y));
        let ymax = v[0].y.max(v[1].y.max(v[2].y));
        if (ymax - ymin - PINNED_RISER_HEIGHT).abs() <= RISER_PIN_TOLERANCE {
            out.push((t, c));
        }
    }
    out
}

/// The stall report's staircase, pinned as measured on the retail install: three parallel stair corridors climbing
/// from tread top minus seven-and-a-half to minus four in half-yalm steps on one-yalm treads, nothing between foot
/// and crest taller than one follow word - which is precisely why the vertical machine walks it while a wall band
/// bounded by the old column-probe rise (kuluu's MAX_GROUND_STEP_UP) stalled it.
#[test]
fn shipped_city_stairs_measure_the_pinned_flight_class() {
    let Some(root) = ffxi_dat::archive::open_test_install() else {
        return;
    };
    install_task_pool();
    let (submeshes, instances) = load_zone(&root, SANDORIA_ZONE);
    let geom = kuluu_render::dat_mzb::MzbCollisionGeometry::from_block(build_collision_geometry(
        &submeshes,
        &instances,
        Some(zone_file_id(SANDORIA_ZONE)),
    ));

    // Column probes along each corridor (reference pinned to the foot level; `ground_nearest` then tracks the
    // flight upward through it), sampled one yalm apart: each step up must fit the half-yalm class and always sit
    // inside the follow word.
    for &z in &STAIR_CORRIDOR_Z {
        let mut prev = geom
            .ground_nearest(Vec2::new(STAIR_X_FOOT + 0.5, z), STAIR_FOOT_TREAD_Y)
            .unwrap_or(f32::NAN);
        assert!(
            (prev - STAIR_FOOT_TREAD_Y).abs() < RISER_PIN_TOLERANCE,
            "corridor at z={z} foot tread measured {prev}, pinned minus seven and a half"
        );
        let mut x = STAIR_X_FOOT + 1.5;
        while x <= STAIR_X_TOP {
            let now = geom
                .ground_nearest(Vec2::new(x, z), STAIR_FOOT_TREAD_Y)
                .unwrap_or(f32::NAN);
            assert!(
                (now - prev).abs() <= PINNED_RISER_HEIGHT + RISER_PIN_TOLERANCE,
                    "corridor at z={z} steps {:+.2} between x-{:.1} and x-{x:.1}; the pinned class rises half \
                     a yalm per one-yalm tread (a step taller than one follow word \
                     ({FOLLOW_WORD_YAMLS_PER_FRAME}) is not walkable at all)",
                now - prev,
                x - 1.0
            );
            prev = now;
            x += 1.0;
        }
        assert!(
            (prev - STAIR_TOP_TREAD_Y).abs() < RISER_PIN_TOLERANCE,
            "corridor at z={z} crest measured {prev}, pinned minus four",
        );

        // The riser faces themselves: wall-class quads exactly half a yalm tall inside the corridor band.
        let faces = pinned_riser_faces(geom.block(ZONE_SLOT_MAIN), z);
        assert!(
            faces.len() >= MIN_PINNED_RISERS_PER_CORRIDOR,
            "corridor at z={z} shows only {} half-yalm riser faces; the pinned stair class is gone and this \
             file's numbers need re-measuring",
            faces.len()
        );
    }
}

/// Railing, fence-panel, post and box-prop collision ships as dedicated repeatedly-placed meshes inside the
/// authored-blocker bound - the census that grounded `PROP_OBJECT_MAX_EXTENT_YAMLS`. The pins sit far below
/// measured (San d'Oria: a 0.2-by-1.3-by-0.2 type placed two hundred twenty-four times, near-one-and-a-half types
/// eighty-seven and seventy-one times; Bastok Mines: one-yalm panels fifty-seven times, one-and-a-half-magnitude
/// panels forty-two times, small posts thirty-two and nineteen) so a materially different shipping of small
/// objects trips this, not patch noise.
#[test]
fn small_solid_objects_ship_repeatedly_within_the_prop_bound() {
    let Some(root) = ffxi_dat::archive::open_test_install() else {
        return;
    };
    install_task_pool();

    let (submeshes, instances) = load_zone(&root, SANDORIA_ZONE);
    let types = repeated_small_types(&submeshes, &instances);
    let busiest = types.iter().max_by_key(|(_, n)| *n).expect(
        "San d'Oria ships repeated bound-fitting wall-bearing mesh types (measured: 224x, 87x, 71x, 35x)",
    );
    assert!(
        busiest.1 >= 200,
        "San d'Oria's busiest bound-fitting type is placed {} times",
        busiest.1
    );
    assert!(
        flagged_face_total(&submeshes, &instances) >= 4000,
        "San d'Oria bakes fewer than four thousand authored-blocker candidate faces; measured class was \
         over five thousand"
    );

    let (submeshes, instances) = load_zone(&root, BASTOK_ZONE);
    let types = repeated_small_types(&submeshes, &instances);
    assert!(
        types.iter().any(|(si, n)| {
            *n >= 40 && local_bbox(&submeshes[*si]).is_some_and(|(lo, hi)| hi.y - lo.y > 1.2)
        }),
        "Bastok Mines no longer carries a bound-fitting panel type about one and a half tall repeated \
         forty-plus times"
    );
}

/// The stair class stays on the terrain side of the two-set law: every pinned riser face is unflagged (a chunk
/// fragment, never an authored object) - so `nearest_wall_contact` suppressing it inside the follow-word band is
/// a terrain decision no object identity could have made. Build-side flag counts agree with the bound rule
/// face-for-face.
#[test]
fn stair_riser_faces_are_terrain_law_not_props() {
    let Some(root) = ffxi_dat::archive::open_test_install() else {
        return;
    };
    install_task_pool();
    let (submeshes, instances) = load_zone(&root, SANDORIA_ZONE);
    let file_id = zone_file_id(SANDORIA_ZONE);
    let block = build_collision_geometry(&submeshes, &instances, Some(file_id));

    let mut checked = 0usize;
    for z in STAIR_CORRIDOR_Z {
        for (idx, c) in pinned_riser_faces(&block, z) {
            assert!(
                !block.tri_prop_blocker.get(idx).copied().unwrap_or(false),
                "a pinned stair riser at ({:+.2},{:+.2}) was flagged as an authored object; it would block \
                 the staircase again",
                c.x,
                c.z
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 3 * MIN_PINNED_RISERS_PER_CORRIDOR,
        "only {checked} pinned riser faces found across three corridors of DAT {file_id}"
    );

    let flagged = block.tri_prop_blocker.iter().filter(|b| **b).count();
    assert_eq!(
        flagged,
        flagged_face_total(&submeshes, &instances),
        "authored-blocker identity drifted from the bound rule"
    );
}
