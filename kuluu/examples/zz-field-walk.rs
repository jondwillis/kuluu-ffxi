//! Drives the single-authority walker (`view_native::walker::step`) headless
//! along a straight line through real MZB collision geometry, printing every
//! tick's wire position, feet height, mode, and vertical decision — so a walk
//! that misbehaves in game can be replayed without a live session.
//!
//! Usage: zz-field-walk <zone_id> <x0> <y0> <z0> <x1> <y1> [stride]
//! (ffxi coordinates; z grows down, like the wire frame. `stride` prints every
//! Nth tick, default 1.)

use bevy::tasks::AsyncComputeTaskPool;
use kuluu::view_native::walker::{obstacles::ObstacleSet, step, WalkMode, Walker};
use kuluu_render::dat_mzb::{build_collision_geometry, load_mzb_placed, MzbCollisionGeometry};

/// The in-game fixed tick rate (Bevy's default `Time<Fixed>`).
const HZ: f32 = 60.0;

fn mode_label(m: &WalkMode) -> String {
    match m {
        WalkMode::Stopped => "stopped".into(),
        WalkMode::Walking => "walking".into(),
        WalkMode::Airborne { vy } => format!("air vy={vy:+.2}"),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut next = |d: f32| args.next().and_then(|s| s.parse().ok()).unwrap_or(d);
    let zone_id = next(245.0) as u16;
    let x0 = next(0.0);
    let y0 = next(0.0);
    let z0 = next(0.0);
    let x1 = next(0.0);
    let y1 = next(0.0);
    let stride = (next(1.0) as usize).max(1);

    AsyncComputeTaskPool::get_or_init(Default::default);

    let file_id = ffxi_dat::zone_dat::effective_zone_dat_file_id(Some(zone_id), None)
        .expect("zone -> mzb file id");
    let (submeshes, instances) = load_mzb_placed(file_id, None).expect("load_mzb_placed");

    let geom: MzbCollisionGeometry = MzbCollisionGeometry::from_block(build_collision_geometry(
        &submeshes,
        &instances,
        Some(file_id),
    ));

    // Horizontal direction in wire units (z is not part of the walk).
    let dx01 = x1 - x0;
    let dy01 = y1 - y0;
    let len = (dx01 * dx01 + dy01 * dy01).sqrt();
    if len < 1e-6 {
        eprintln!("start and end are the same point");
        std::process::exit(1);
    }
    let dir = (dx01 / len, dy01 / len);

    // The production run speed: what dispatch feeds the walker on foot.
    let speed =
        kuluu_session::state::move_speed_yps(kuluu_session::state::BASE_PACKET_SPEED, false);
    let dt = 1.0 / HZ;
    let ticks = (len / speed * HZ).ceil() as usize;

    println!(
        "zone {zone_id} (DAT {file_id}): ({x0:.2},{y0:.2}) -> ({x1:.2},{y1:.2}), \
         {len:.2} units at {speed:.1} y/s, {ticks} ticks @ {HZ:.0} Hz"
    );
    println!("start wire z={z0:.3}\n");

    let obstacles = ObstacleSet::default();
    let mut state = Walker::default();
    let (mut x, mut y, mut z) = (x0, y0, z0);
    for i in 0..ticks {
        let res = step(
            &geom,
            &obstacles,
            &mut state,
            x,
            y,
            z,
            dir.0 * speed * dt,
            dir.1 * speed * dt,
            speed,
            dt,
            false,
        );
        x += res.dx;
        y += res.dy;
        z = res.feet_z;
        if i % stride == 0 {
            println!(
                "tick {i:5}: wire=({x:8.3},{y:8.3}) feet_z={z:+7.3}  {:<12} {}",
                mode_label(&res.mode),
                res.decision.label()
            );
        }
    }
    println!("\nend wire=({x:.3},{y:.3}) feet_z={z:.3}");
}
