//! Print-only flag census for USER's 2026-10-28 "stuck on this rounded edge" report at
//! Bastok Mines xz(-52.15,+19.15): the walker mounts a ~+0.55 pipe-cap floor through
//! terrain side faces (t24632/t24633/t27828/t27829, y[0..2]) via the riser exemption,
//! and stands wedged at +0.581 above the street. A real stair riser and this pipe side
//! are geometrically identical under kuluu's normal-only class rules, so before any law
//! changes this dumps the one authored bit that could tell them apart: the MZB triangle
//! second-word `0x4000` (`ffxi_dat::mzb::TRI_SECOND_WORD_FLAG`, parsed as `is_invalid`).
//! research/xi-tools/docs/zone/collision.md "Technical notes" claims player movement
//! keys off it; ffxi-dat measured the bit unset across Lower Jeuno, Port Jeuno, Southern San
//! d'Oria and West Ronfaure. Bastok Mines (DAT 334) had never been checked.
//!
//! Prints, per zone: total triangles with the bit set, every flagged face in a body-
//! height band over the reported pipe box (Bastok) and the shipped stair pin box (San
//! d'Oria DAT 331), and the flags of the exact blocking face ids from the STAIR DEBUG
//! panel. Asserts nothing.

use bevy::math::Vec3;

use kuluu_render::dat_mzb::{build_collision_geometry, load_mzb_placed, MzbCollisionGeometry};

struct Box2 {
    label: &'static str,
    zone: u16,
    x0: f32,
    x1: f32,
    z0: f32,
    z1: f32,
}

fn dump_zone(b: &Box2, known_ids: &[usize]) {
    // install-gated, print-only: a checkout without a client install (CI) has no shipped
    // triangles to census, and that is not a defect.
    let Some(root) = ffxi_dat::archive::open_test_install() else {
        return;
    };
    bevy::tasks::AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
    let file_id = ffxi_dat::zone_dat::zone_id_to_mzb_file_id(b.zone).unwrap();
    let (submeshes, instances) = load_mzb_placed(&root, file_id, None).unwrap();
    let geom: MzbCollisionGeometry = MzbCollisionGeometry::from_block(build_collision_geometry(
        &submeshes,
        &instances,
        Some(file_id),
    ));
    let blk = geom.block(kuluu_render::dat_mzb::ZONE_SLOT_MAIN);
    let total = blk.indices.len() / 3;
    let flagged: Vec<usize> = (0..total)
        .filter(|&t| blk.tri_invalid.get(t).copied().unwrap_or(false))
        .collect();
    println!(
        "DIAG flags {} zone {} slot0: tris={total} invalid={} (tri_invalid len={})",
        b.label,
        file_id,
        flagged.len(),
        blk.tri_invalid.len()
    );

    let in_band = |ymin: f32, ymax: f32| ymin <= 3.0 && ymax >= -1.0;
    let mut shown_flagged = 0usize;
    for &t in &flagged {
        let base = t * 3;
        let (a, bb, c) = (
            blk.positions[blk.indices[base] as usize],
            blk.positions[blk.indices[base + 1] as usize],
            blk.positions[blk.indices[base + 2] as usize],
        );
        let lo = a.min(bb.min(c));
        let hi = a.max(bb.max(c));
        if !(lo.x >= b.x0 && hi.x <= b.x1 && lo.z >= b.z0 && hi.z <= b.z1) {
            continue;
        }
        shown_flagged += 1;
        if shown_flagged <= 60 && in_band(lo.y, hi.y) {
            let n = blk.tri_normals.get(t).copied().unwrap_or(Vec3::ZERO);
            println!(
                "DIAG inv t{t} n({:.2},{:.2},{:.2}) y[{:.2}..{:.2}] xz[({:.2},{:.2})..({:.2},{:.2})] terrain{:?}",
                n.x, n.y, n.z, lo.y, hi.y, lo.x, lo.z, hi.x, hi.z,
                blk.tri_terrain.get(t).copied().unwrap_or(0xFF),
            );
        }
    }
    println!("DIAG flags {} flagged-in-box={shown_flagged}", b.label);

    for &t in known_ids {
        let base = t * 3;
        if base + 2 >= blk.indices.len() {
            continue;
        }
        let (a, bb, c) = (
            blk.positions[blk.indices[base] as usize],
            blk.positions[blk.indices[base + 1] as usize],
            blk.positions[blk.indices[base + 2] as usize],
        );
        let lo = a.min(bb.min(c));
        let hi = a.max(bb.max(c));
        let n = blk.tri_normals.get(t).copied().unwrap_or(Vec3::ZERO);
        println!(
            "DIAG known {} t{t} invalid={} n({:.2},{:.2},{:.2}) y[{:.2}..{:.2}] xz[({:.2},{:.2})..({:.2},{:.2})]",
            b.label,
            blk.tri_invalid.get(t).copied().unwrap_or(false),
            n.x, n.y, n.z, lo.y, hi.y, lo.x, lo.z, hi.x, hi.z,
        );
    }

    // Standable cap faces in the box: do THEY carry the bit? (A flagged floor would let
    // a "skip invalid for movement" rule reject them outright.)
    let mut caps = 0usize;
    let mut caps_invalid = 0usize;
    for t in 0..total {
        let n = blk.tri_normals.get(t).copied().unwrap_or(Vec3::ZERO);
        if n.y < 0.99 {
            continue;
        }
        let base = t * 3;
        let (a, bb, c) = (
            blk.positions[blk.indices[base] as usize],
            blk.positions[blk.indices[base + 1] as usize],
            blk.positions[blk.indices[base + 2] as usize],
        );
        let lo = a.min(bb.min(c));
        let hi = a.max(bb.max(c));
        if !(lo.x >= b.x0 && hi.x <= b.x1 && lo.z >= b.z0 && hi.z <= b.z1) {
            continue;
        }
        if !(lo.y > 0.2 && hi.y < 3.0) {
            continue; // body-height cap band over the street
        }
        caps += 1;
        if blk.tri_invalid.get(t).copied().unwrap_or(false) {
            caps_invalid += 1;
        }
    }
    println!(
        "DIAG caps {} flat-up faces in band: {caps}, invalid among them {caps_invalid}",
        b.label
    );
}

#[test]
fn pipe_cap_flags_dump() {
    dump_zone(
        &Box2 {
            label: "bastok_pipe",
            zone: 234,
            x0: -56.5,
            x1: -48.0,
            z0: 15.0,
            z1: 23.0,
        },
        &[23339, 24632, 24633, 27828, 27829],
    );
    dump_zone(
        &Box2 {
            label: "sd_pin_stairs",
            zone: 231,
            x0: -201.5,
            x1: -190.5,
            z0: -84.0,
            z1: -70.0,
        },
        &[],
    );
}
