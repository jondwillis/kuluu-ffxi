//! Live matrices (plan §4 step 4): drive [`step`] headless against real
//! `MzbCollisionGeometry` built from synthetic blocks — no App, no physics
//! world. The builders are the ones that lived in dat_mzb's
//! `wall_collision_tests`; the assertions are re-pinned for the single-
//! authority walker: final height, zero dy sign reversals on a flight, and a
//! bounded second difference of y (the same two numbers FieldDebug shows).

use bevy::math::{Vec2, Vec3};
use ffxi_dat::mzb::NO_SUB_AREA_LINK;
use kuluu_render::dat_mzb::{MzbCollisionBlock, MzbCollisionGeometry};

use super::consts::{FallModel, STEP_MAX};
use super::obstacles::{DoorObstacle, MobObstacle, ObstacleSet};
use super::step::step;
use super::{VerticalDecision, Walker};

// ---------------------------------------------------------------------------
// Geometry builders (ported from dat_mzb's wall_collision_tests)
// ---------------------------------------------------------------------------

fn quad(b: &mut MzbCollisionBlock, v: [Vec3; 4], n: Vec3, link: u32) {
    let i0 = b.positions.len() as u32;
    b.positions.extend_from_slice(&v);
    b.indices
        .extend_from_slice(&[i0, i0 + 1, i0 + 2, i0, i0 + 2, i0 + 3]);
    b.tri_normals.extend_from_slice(&[n, n]);
    b.tri_sub_area.extend_from_slice(&[link, link]);
}

fn staircase(steps: usize, d: f32, r: f32, balustrades: bool) -> MzbCollisionGeometry {
    let mut b = MzbCollisionBlock::default();
    quad(
        &mut b,
        [
            Vec3::new(-10.0, 0.0, -3.0),
            Vec3::new(0.0, 0.0, -3.0),
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::new(-10.0, 0.0, 3.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    for i in 0..steps {
        let x0 = i as f32 * d;
        let y0 = i as f32 * r;
        let y1 = y0 + r;
        quad(
            &mut b,
            [
                Vec3::new(x0, y0, -3.0),
                Vec3::new(x0, y0, 3.0),
                Vec3::new(x0, y1, 3.0),
                Vec3::new(x0, y1, -3.0),
            ],
            Vec3::new(-1.0, 0.0, 0.0),
            NO_SUB_AREA_LINK,
        );
        quad(
            &mut b,
            [
                Vec3::new(x0, y1, -3.0),
                Vec3::new(x0 + d, y1, -3.0),
                Vec3::new(x0 + d, y1, 3.0),
                Vec3::new(x0, y1, 3.0),
            ],
            Vec3::Y,
            NO_SUB_AREA_LINK,
        );
    }
    let xt = steps as f32 * d;
    let yt = steps as f32 * r;
    quad(
        &mut b,
        [
            Vec3::new(xt, yt, -3.0),
            Vec3::new(xt + 10.0, yt, -3.0),
            Vec3::new(xt + 10.0, yt, 3.0),
            Vec3::new(xt, yt, 3.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    if balustrades {
        let sl = (d * d + r * r).sqrt();
        for zed in [3.0f32, -3.0] {
            let nn = Vec3::new(0.0, 0.0, -zed.signum());
            quad(
                &mut b,
                [
                    Vec3::new(-2.0, 0.0, zed),
                    Vec3::new(xt + 2.0, yt, zed),
                    Vec3::new(xt + 2.0, yt + 1.2, zed),
                    Vec3::new(-2.0, 1.2, zed),
                ],
                nn,
                NO_SUB_AREA_LINK,
            );
            let sn = Vec3::new(-r / sl, d / sl, 0.0);
            quad(
                &mut b,
                [
                    Vec3::new(0.0, 0.0, zed - 0.2 * zed.signum()),
                    Vec3::new(xt, yt, zed - 0.2 * zed.signum()),
                    Vec3::new(xt + 0.3, yt, zed - 0.2 * zed.signum()),
                    Vec3::new(0.3, 0.0, zed - 0.2 * zed.signum()),
                ],
                sn,
                NO_SUB_AREA_LINK,
            );
        }
    }
    MzbCollisionGeometry::from_block(b)
}

fn flat_with_wall(wall_x: f32, height: f32, link: u32) -> MzbCollisionGeometry {
    let mut b = MzbCollisionBlock::default();
    quad(
        &mut b,
        [
            Vec3::new(-10.0, 0.0, -10.0),
            Vec3::new(10.0, 0.0, -10.0),
            Vec3::new(10.0, 0.0, 10.0),
            Vec3::new(-10.0, 0.0, 10.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    quad(
        &mut b,
        [
            Vec3::new(wall_x, 0.0, -10.0),
            Vec3::new(wall_x, 0.0, 10.0),
            Vec3::new(wall_x, height, 10.0),
            Vec3::new(wall_x, height, -10.0),
        ],
        Vec3::new(-1.0, 0.0, 0.0),
        link,
    );
    MzbCollisionGeometry::from_block(b)
}

fn parapet_platform(wall_x: f32, wall_h: f32, plat_y: f32) -> MzbCollisionGeometry {
    let mut b = MzbCollisionBlock::default();
    quad(
        &mut b,
        [
            Vec3::new(-10.0, 0.0, -10.0),
            Vec3::new(wall_x, 0.0, -10.0),
            Vec3::new(wall_x, 0.0, 10.0),
            Vec3::new(-10.0, 0.0, 10.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    quad(
        &mut b,
        [
            Vec3::new(wall_x, 0.0, -10.0),
            Vec3::new(wall_x, 0.0, 10.0),
            Vec3::new(wall_x, wall_h, 10.0),
            Vec3::new(wall_x, wall_h, -10.0),
        ],
        Vec3::new(-1.0, 0.0, 0.0),
        NO_SUB_AREA_LINK,
    );
    quad(
        &mut b,
        [
            Vec3::new(wall_x, plat_y, -10.0),
            Vec3::new(10.0, plat_y, -10.0),
            Vec3::new(10.0, plat_y, 10.0),
            Vec3::new(wall_x, plat_y, 10.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    MzbCollisionGeometry::from_block(b)
}

fn ramp(from_x: f32, to_x: f32, top_y: f32) -> MzbCollisionGeometry {
    let mut b = MzbCollisionBlock::default();
    quad(
        &mut b,
        [
            Vec3::new(-10.0, 0.0, -6.0),
            Vec3::new(from_x, 0.0, -6.0),
            Vec3::new(from_x, 0.0, 6.0),
            Vec3::new(-10.0, 0.0, 6.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    let run = to_x - from_x;
    let l = (run * run + top_y * top_y).sqrt();
    let n = Vec3::new(-top_y / l, run / l, 0.0);
    quad(
        &mut b,
        [
            Vec3::new(from_x, 0.0, -6.0),
            Vec3::new(to_x, top_y, -6.0),
            Vec3::new(to_x, top_y, 6.0),
            Vec3::new(from_x, 0.0, 6.0),
        ],
        n,
        NO_SUB_AREA_LINK,
    );
    quad(
        &mut b,
        [
            Vec3::new(to_x, top_y, -6.0),
            Vec3::new(to_x + 10.0, top_y, -6.0),
            Vec3::new(to_x + 10.0, top_y, 6.0),
            Vec3::new(to_x, top_y, 6.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    MzbCollisionGeometry::from_block(b)
}

fn corridor(gap: f32) -> MzbCollisionGeometry {
    let mut b = MzbCollisionBlock::default();
    quad(
        &mut b,
        [
            Vec3::new(-10.0, 0.0, -10.0),
            Vec3::new(10.0, 0.0, -10.0),
            Vec3::new(10.0, 0.0, 10.0),
            Vec3::new(-10.0, 0.0, 10.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    let h = gap / 2.0;
    quad(
        &mut b,
        [
            Vec3::new(2.0, 0.0, h),
            Vec3::new(3.0, 0.0, h),
            Vec3::new(3.0, 2.5, h),
            Vec3::new(2.0, 2.5, h),
        ],
        Vec3::new(0.0, 0.0, -1.0),
        NO_SUB_AREA_LINK,
    );
    quad(
        &mut b,
        [
            Vec3::new(2.0, 0.0, -h),
            Vec3::new(3.0, 0.0, -h),
            Vec3::new(3.0, 2.5, -h),
            Vec3::new(2.0, 2.5, -h),
        ],
        Vec3::new(0.0, 0.0, 1.0),
        NO_SUB_AREA_LINK,
    );
    MzbCollisionGeometry::from_block(b)
}

/// The Bastok Mines stair as measured 2026-08-23: a level-0 terrace slab that
/// continues UNDER the treads (the "stuff" the old walker clipped through).
fn stair_with_ground_under(steps: usize, d: f32, r: f32) -> MzbCollisionGeometry {
    let mut b = MzbCollisionBlock::default();
    quad(
        &mut b,
        [
            Vec3::new(-10.0, 0.0, -3.0),
            Vec3::new(steps as f32 * d + 5.0, 0.0, -3.0),
            Vec3::new(steps as f32 * d + 5.0, 0.0, 3.0),
            Vec3::new(-10.0, 0.0, 3.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    for i in 0..steps {
        let x0 = i as f32 * d;
        let y0 = i as f32 * r;
        let y1 = y0 + r;
        quad(
            &mut b,
            [
                Vec3::new(x0, y0, -3.0),
                Vec3::new(x0, y0, 3.0),
                Vec3::new(x0, y1, 3.0),
                Vec3::new(x0, y1, -3.0),
            ],
            Vec3::new(-1.0, 0.0, 0.0),
            NO_SUB_AREA_LINK,
        );
        quad(
            &mut b,
            [
                Vec3::new(x0, y1, -3.0),
                Vec3::new(x0 + d, y1, -3.0),
                Vec3::new(x0 + d, y1, 3.0),
                Vec3::new(x0, y1, 3.0),
            ],
            Vec3::Y,
            NO_SUB_AREA_LINK,
        );
    }
    let xt = steps as f32 * d;
    let yt = steps as f32 * r;
    quad(
        &mut b,
        [
            Vec3::new(xt, yt, -3.0),
            Vec3::new(xt + 10.0, yt, -3.0),
            Vec3::new(xt + 10.0, yt, 3.0),
            Vec3::new(xt, yt, 3.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    MzbCollisionGeometry::from_block(b)
}

// ---------------------------------------------------------------------------
// New builders (plan §4): hole / ledge / nosing
// ---------------------------------------------------------------------------

/// Flat floor at y=0 with a vertical drop of `drop` at x = edge_x.
fn ledge(edge_x: f32, drop: f32) -> MzbCollisionGeometry {
    let mut b = MzbCollisionBlock::default();
    quad(
        &mut b,
        [
            Vec3::new(-10.0, 0.0, -6.0),
            Vec3::new(edge_x, 0.0, -6.0),
            Vec3::new(edge_x, 0.0, 6.0),
            Vec3::new(-10.0, 0.0, 6.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    quad(
        &mut b,
        [
            Vec3::new(edge_x, -drop, -6.0),
            Vec3::new(10.0, -drop, -6.0),
            Vec3::new(10.0, -drop, 6.0),
            Vec3::new(edge_x, -drop, 6.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    quad(
        &mut b,
        [
            Vec3::new(edge_x, -drop, -6.0),
            Vec3::new(edge_x, 0.0, -6.0),
            Vec3::new(edge_x, 0.0, 6.0),
            Vec3::new(edge_x, -drop, 6.0),
        ],
        Vec3::new(1.0, 0.0, 0.0),
        NO_SUB_AREA_LINK,
    );
    MzbCollisionGeometry::from_block(b)
}

/// Flat floor at y=0 with a gap of `width` centered on x = center_x; an
/// optional lower floor `depth` below the gap (None = open hole).
fn hole(center_x: f32, width: f32, depth: Option<f32>) -> MzbCollisionGeometry {
    let lo = center_x - width / 2.0;
    let hi = center_x + width / 2.0;
    let mut b = MzbCollisionBlock::default();
    quad(
        &mut b,
        [
            Vec3::new(-10.0, 0.0, -6.0),
            Vec3::new(lo, 0.0, -6.0),
            Vec3::new(lo, 0.0, 6.0),
            Vec3::new(-10.0, 0.0, 6.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    quad(
        &mut b,
        [
            Vec3::new(hi, 0.0, -6.0),
            Vec3::new(10.0, 0.0, -6.0),
            Vec3::new(10.0, 0.0, 6.0),
            Vec3::new(hi, 0.0, 6.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    if let Some(depth) = depth {
        quad(
            &mut b,
            [
                Vec3::new(lo, -depth, -6.0),
                Vec3::new(hi, -depth, -6.0),
                Vec3::new(hi, -depth, 6.0),
                Vec3::new(lo, -depth, 6.0),
            ],
            Vec3::Y,
            NO_SUB_AREA_LINK,
        );
    }
    MzbCollisionGeometry::from_block(b)
}

/// A flight with a 0.15-wide x 0.05-tall lip on every tread edge: the live
/// version of the nosing case (the lip filter must ignore it).
fn nosing_flight(steps: usize, d: f32, r: f32) -> MzbCollisionGeometry {
    let mut b = MzbCollisionBlock::default();
    quad(
        &mut b,
        [
            Vec3::new(-10.0, 0.0, -3.0),
            Vec3::new(0.0, 0.0, -3.0),
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::new(-10.0, 0.0, 3.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    for i in 0..steps {
        let x0 = i as f32 * d;
        let y0 = i as f32 * r;
        let y1 = y0 + r;
        quad(
            &mut b,
            [
                Vec3::new(x0, y0, -3.0),
                Vec3::new(x0, y0, 3.0),
                Vec3::new(x0, y1, 3.0),
                Vec3::new(x0, y1, -3.0),
            ],
            Vec3::new(-1.0, 0.0, 0.0),
            NO_SUB_AREA_LINK,
        );
        quad(
            &mut b,
            [
                Vec3::new(x0, y1, -3.0),
                Vec3::new(x0 + d, y1, -3.0),
                Vec3::new(x0 + d, y1, 3.0),
                Vec3::new(x0, y1, 3.0),
            ],
            Vec3::Y,
            NO_SUB_AREA_LINK,
        );
        // Nosing: a raised shelf on the first 0.15 of the tread.
        quad(
            &mut b,
            [
                Vec3::new(x0, y1 + 0.05, -3.0),
                Vec3::new(x0 + 0.15, y1 + 0.05, -3.0),
                Vec3::new(x0 + 0.15, y1 + 0.05, 3.0),
                Vec3::new(x0, y1 + 0.05, 3.0),
            ],
            Vec3::Y,
            NO_SUB_AREA_LINK,
        );
    }
    let xt = steps as f32 * d;
    let yt = steps as f32 * r;
    quad(
        &mut b,
        [
            Vec3::new(xt, yt, -3.0),
            Vec3::new(xt + 10.0, yt, -3.0),
            Vec3::new(xt + 10.0, yt, 3.0),
            Vec3::new(xt, yt, 3.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    MzbCollisionGeometry::from_block(b)
}

// ---------------------------------------------------------------------------
// Obstacle builders (plan §4): door_leaf / mob_circle
// ---------------------------------------------------------------------------

fn door_tri(v: [Vec3; 4]) -> ([Vec3; 3], Vec3) {
    let n = (v[1] - v[0]).cross(v[2] - v[0]).normalize();
    ([v[0], v[1], v[2]], n)
}

fn door_bounds(tris: &[([Vec3; 3], Vec3)]) -> (Vec3, Vec3) {
    let mut min = Vec3::INFINITY;
    let mut max = Vec3::NEG_INFINITY;
    for (v, _) in tris {
        for p in v.iter() {
            min = min.min(*p);
            max = max.max(*p);
        }
    }
    (min, max)
}

/// A closed door leaf standing at x = wall_x: a wall for the sweep.
fn door_wall(wall_x: f32) -> ObstacleSet {
    let v = [
        Vec3::new(wall_x, 0.0, -1.0),
        Vec3::new(wall_x, 0.0, 1.0),
        Vec3::new(wall_x, 3.0, 1.0),
        Vec3::new(wall_x, 3.0, -1.0),
    ];
    let tris = vec![door_tri(v)];
    let (min, max) = door_bounds(&tris);
    ObstacleSet {
        doors: vec![DoorObstacle { tris, min, max }],
        mobs: Vec::new(),
    }
}

/// A closed drawbridge deck at y = deck_y spanning x in [x0, x1]: a floor for
/// the column probe (up-facing door faces are never walls).
fn drawbridge(x0: f32, x1: f32, deck_y: f32) -> ObstacleSet {
    let v = [
        Vec3::new(x0, deck_y, 3.0),
        Vec3::new(x1, deck_y, 3.0),
        Vec3::new(x1, deck_y, -3.0),
        Vec3::new(x0, deck_y, -3.0),
    ];
    let tris = vec![door_tri(v)];
    let (min, max) = door_bounds(&tris);
    ObstacleSet {
        doors: vec![DoorObstacle { tris, min, max }],
        mobs: Vec::new(),
    }
}

fn mob_circle(id: u32, cx: f32, cz: f32, radius: f32) -> ObstacleSet {
    ObstacleSet {
        doors: Vec::new(),
        mobs: vec![MobObstacle {
            id,
            center: Vec2::new(cx, cz),
            radius,
        }],
    }
}

// ---------------------------------------------------------------------------
// Walk harness (wire coordinates at the boundary, like dispatch)
// ---------------------------------------------------------------------------

/// One headless walk through [`step`]: `secs` of input in unit wire direction
/// `dir` at `speed_yps`, fixed-rate `hz`. Records the wire position and the
/// vertical decision after every tick.
fn walk(
    geom: &MzbCollisionGeometry,
    obstacles: &ObstacleSet,
    start: (f32, f32, f32),
    dir: (f32, f32),
    secs: f32,
    hz: f32,
    speed_yps: f32,
) -> Vec<((f32, f32, f32), VerticalDecision)> {
    let dt = 1.0 / hz;
    let ticks = (secs * hz).round() as usize;
    let mut state = Walker::default();
    let (mut x, mut y, mut z) = start;
    let mut out = Vec::with_capacity(ticks);
    for _ in 0..ticks {
        let res = step(
            geom,
            obstacles,
            &mut state,
            x,
            y,
            z,
            dir.0 * speed_yps * dt,
            dir.1 * speed_yps * dt,
            speed_yps,
            dt,
            false,
        );
        x += res.dx;
        y += res.dy;
        z = res.feet_z;
        out.push(((x, y, z), res.decision));
    }
    out
}

/// [`walk`] with stop phases: each segment is (unit wire dir, move secs, stop
/// secs). Stop ticks run the vertical pass with zero input — exactly what
/// dispatch does on idle.
fn walk_with_stops(
    geom: &MzbCollisionGeometry,
    obstacles: &ObstacleSet,
    start: (f32, f32, f32),
    segments: &[(Vec2, f32, f32)],
    hz: f32,
    speed_yps: f32,
) -> Vec<((f32, f32, f32), VerticalDecision)> {
    let dt = 1.0 / hz;
    let mut state = Walker::default();
    let (mut x, mut y, mut z) = start;
    let mut out = Vec::new();
    for &(dir, move_secs, stop_secs) in segments {
        let ticks = |secs: f32| (secs * hz).round() as usize;
        for _ in 0..ticks(move_secs + stop_secs) {
            let moving = out.len() % ticks(move_secs + stop_secs) < ticks(move_secs);
            let (dx, dy) = if moving {
                (dir.x * speed_yps * dt, dir.y * speed_yps * dt)
            } else {
                (0.0, 0.0)
            };
            let res = step(
                geom, obstacles, &mut state, x, y, z, dx, dy, speed_yps, dt, false,
            );
            x += res.dx;
            y += res.dy;
            z = res.feet_z;
            out.push(((x, y, z), res.decision));
        }
    }
    out
}

fn ys(trace: &[((f32, f32, f32), VerticalDecision)]) -> Vec<f32> {
    trace.iter().map(|&((_, _, z), _)| -z).collect()
}

/// Sign flips of consecutive nonzero dy (FieldDebug::reversals' definition).
fn reversals(ys: &[f32]) -> u32 {
    let mut flips = 0u32;
    let mut last_sign = 0i8;
    for w in ys.windows(2) {
        let dy = w[1] - w[0];
        if dy.abs() < 1e-6 {
            continue;
        }
        let s = if dy > 0.0 { 1 } else { -1 };
        if last_sign != 0 && s != last_sign {
            flips += 1;
        }
        last_sign = s;
    }
    flips
}

/// Max |second difference| of y (FieldDebug::max_d2y's definition).
fn max_d2(ys: &[f32]) -> f32 {
    ys.windows(3)
        .map(|w| (w[2] - 2.0 * w[1] + w[0]).abs())
        .fold(0.0f32, f32::max)
}

/// The d2 bound for a flight: merges are straight lines (~0); a fall is a
/// parabola whose Euler second difference is exactly g*dt^2 per tick.
fn d2_bound(r: f32, hz: f32) -> f32 {
    if r > STEP_MAX + 1e-6 {
        FallModel::default().g / (hz * hz) + 1e-3
    } else {
        0.02
    }
}

const RUN: f32 =
    kuluu_session::state::move_speed_yps(kuluu_session::state::BASE_PACKET_SPEED, false); // 5.0 y/s — the production run speed

// ---------------------------------------------------------------------------
// Staircase matrices (plan §4 live)
// ---------------------------------------------------------------------------

#[test]
fn stairs_ascend_matrix() {
    for hz in [60.0f32, 30.0] {
        for (r, d) in [
            (0.2, 0.25),
            (0.25, 0.3),
            (0.3, 0.4),
            (0.35, 0.5),
            (0.4, 0.4),
            (0.3, 0.9),
        ] {
            for bal in [false, true] {
                let g = staircase(12, d, r, bal);
                let t = walk(
                    &g,
                    &ObstacleSet::default(),
                    (-2.0, 0.0, 0.0),
                    (1.0, 0.0),
                    18.0,
                    hz,
                    RUN,
                );
                let (x, _y, z) = t.last().unwrap().0;
                let ys = ys(&t);
                assert!(
                    (-z - 12.0 * r).abs() < 0.05 && x > 12.0 * d,
                    "stuck ascending r={r} d={d} bal={bal} hz={hz}: x={x:.2} h={:.2}",
                    -z
                );
                assert_eq!(reversals(&ys), 0, "zig on ascent r={r} d={d} hz={hz}");
                assert!(
                    max_d2(&ys) < d2_bound(r, hz),
                    "jerk on ascent r={r} d={d} hz={hz}: {}",
                    max_d2(&ys)
                );
            }
        }
    }
}

#[test]
fn stairs_descend_matrix() {
    for hz in [60.0f32, 30.0] {
        for (r, d) in [(0.2, 0.25), (0.3, 0.4), (0.35, 0.5), (0.5, 0.5), (0.3, 0.9)] {
            for bal in [false, true] {
                let g = staircase(12, d, r, bal);
                let top = (12.0 * d + 3.0, 0.0, -(12.0 * r));
                let t = walk(&g, &ObstacleSet::default(), top, (-1.0, 0.0), 18.0, hz, RUN);
                let (x, _y, z) = t.last().unwrap().0;
                let ys = ys(&t);
                assert!(
                    (-z).abs() < 0.05 && x < -1.0,
                    "stuck descending r={r} d={d} bal={bal} hz={hz}: x={x:.2} h={:.2}",
                    -z
                );
                assert_eq!(reversals(&ys), 0, "zig on descent r={r} d={d} hz={hz}");
                assert!(
                    max_d2(&ys) < d2_bound(r, hz),
                    "jerk on descent r={r} d={d} hz={hz}: {}",
                    max_d2(&ys)
                );
            }
        }
    }
}

/// A flight whose risers exceed STEP_MAX cannot be climbed: the walker stops at
/// the foot of the first riser and never gains height.
#[test]
fn riser_above_step_height_blocks_ascent() {
    let g = staircase(12, 0.5, 0.5, false);
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (-2.0, 0.0, 0.0),
        (1.0, 0.0),
        8.0,
        60.0,
        RUN,
    );
    let (x, _y, z) = t.last().unwrap().0;
    assert!(
        x < -0.2 && z.abs() < 0.05,
        "blocked at the foot of the 0.5-riser flight: x={x:.3} h={z:.3}"
    );
}

#[test]
fn stair_with_ground_under_climbs_and_never_dips_to_slab() {
    for (r, d) in [(0.26, 0.48), (0.35, 0.5)] {
        let g = stair_with_ground_under(8, d, r);
        // Start flush against the first riser — his spawn was ~0.35 yalms off it.
        let start_x = -super::consts::BODY_RADIUS + 0.05;
        let t = walk(
            &g,
            &ObstacleSet::default(),
            (start_x, 0.0, 0.0),
            (1.0, 0.0),
            18.0,
            60.0,
            RUN,
        );
        let ys = ys(&t);
        assert!(ys.iter().all(|&h| h >= -0.05), "dipped to the slab: {ys:?}");
        let (x, _y, z) = t.last().unwrap().0;
        assert!(
            (-z - 8.0 * r).abs() < 0.05 && x > 8.0 * d,
            "pinned at foot of buried stair r={r} d={d}: x={x:.2} h={:.3}",
            -z
        );
    }
}

#[test]
fn stair_with_ground_under_descends() {
    let (r, d) = (0.26f32, 0.48);
    let g = stair_with_ground_under(8, d, r);
    let top = (8.0 * d + 3.0, 0.0, -(8.0 * r));
    let t = walk(
        &g,
        &ObstacleSet::default(),
        top,
        (-1.0, 0.0),
        18.0,
        60.0,
        RUN,
    );
    let (x, _y, z) = t.last().unwrap().0;
    assert!(
        (-z).abs() < 0.05 && x < -1.0,
        "stuck descending buried stair: x={x:.2} h={:.3}",
        -z
    );
}

#[test]
fn nosing_flight_climbs_like_a_clean_one() {
    let g = nosing_flight(8, 0.4, 0.3);
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (-2.0, 0.0, 0.0),
        (1.0, 0.0),
        15.0,
        60.0,
        RUN,
    );
    let ys = ys(&t);
    assert_eq!(reversals(&ys), 0, "nosing zig: {ys:?}");
    let (x, _y, z) = t.last().unwrap().0;
    assert!(
        (-z - 8.0 * 0.3).abs() < 0.05 && x > 8.0 * 0.4,
        "nosing flight: x={x:.2} h={:.2}",
        -z
    );
}

// ---------------------------------------------------------------------------
// Walls / corners / corridors (plan §4 live)
// ---------------------------------------------------------------------------

#[test]
fn tall_wall_blocks_with_standoff() {
    let g = flat_with_wall(2.0, 3.0, NO_SUB_AREA_LINK);
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (-2.0, 0.0, 0.0),
        (1.0, 0.0),
        8.0,
        60.0,
        RUN,
    );
    let x = t.last().unwrap().0 .0;
    assert!(x < 2.0 - 0.35 && x > 2.0 - 0.65, "standoff: x={x:.3}");
}

#[test]
fn fence_with_level_ground_behind_blocks() {
    let g = flat_with_wall(2.0, 0.7, NO_SUB_AREA_LINK);
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (-2.0, 0.0, 0.0),
        (1.0, 0.0),
        8.0,
        60.0,
        RUN,
    );
    let (x, _y, z) = t.last().unwrap().0;
    assert!(x < 2.0, "fence held: x={x:.3}");
    assert!((-z).abs() < 0.01, "no phantom lift: h={:.3}", -z);
}

#[test]
fn parapet_lip_fronting_platform_blocks() {
    let g = parapet_platform(2.0, 1.0, 0.8);
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (-2.0, 0.0, 0.0),
        (1.0, 0.0),
        8.0,
        60.0,
        RUN,
    );
    let (x, _y, z) = t.last().unwrap().0;
    assert!(x < 2.0, "lip held: x={x:.3}");
    assert!((-z).abs() < 0.01, "stayed low: h={:.3}", -z);
}

/// A flush riser at or below STEP_MAX is a step: climb it.
#[test]
fn flush_step_onto_platform_works() {
    let g = parapet_platform(2.0, 0.35, 0.35);
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (-2.0, 0.0, 0.0),
        (1.0, 0.0),
        8.0,
        60.0,
        RUN,
    );
    let (x, _y, z) = t.last().unwrap().0;
    assert!(
        x > 3.0 && (-z - 0.35).abs() < 0.05,
        "stepped up: x={x:.2} h={:.2}",
        -z
    );
}

/// A flush riser ABOVE STEP_MAX is a wall, not a step: standoff, no climb.
#[test]
fn flush_riser_above_step_height_blocks() {
    let g = parapet_platform(2.0, 0.8, 0.8);
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (-2.0, 0.0, 0.0),
        (1.0, 0.0),
        8.0,
        60.0,
        RUN,
    );
    let (x, _y, z) = t.last().unwrap().0;
    assert!(
        x < 2.0 - 0.35 && x > 2.0 - 0.65 && z.abs() < 0.05,
        "standoff at the tall riser: x={x:.3} h={z:.3}"
    );
}

#[test]
fn corridor_pass_and_block() {
    let t = walk(
        &corridor(1.2),
        &ObstacleSet::default(),
        (-2.0, 0.0, 0.0),
        (1.0, 0.0),
        8.0,
        60.0,
        RUN,
    );
    assert!(
        t.last().unwrap().0 .0 > 4.0,
        "1.2 gap passes: x={:.2}",
        t.last().unwrap().0 .0
    );

    let t = walk(
        &corridor(0.7),
        &ObstacleSet::default(),
        (-2.0, 0.0, 0.0),
        (1.0, 0.0),
        8.0,
        60.0,
        RUN,
    );
    assert!(
        t.last().unwrap().0 .0 < 2.0,
        "0.7 gap blocks: x={:.2}",
        t.last().unwrap().0 .0
    );
}

#[test]
fn corner_blocks_the_body() {
    let mut b = MzbCollisionBlock::default();
    quad(
        &mut b,
        [
            Vec3::new(-10.0, 0.0, -10.0),
            Vec3::new(10.0, 0.0, -10.0),
            Vec3::new(10.0, 0.0, 10.0),
            Vec3::new(-10.0, 0.0, 10.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    quad(
        &mut b,
        [
            Vec3::new(2.0, 0.0, 0.1),
            Vec3::new(2.0, 0.0, 10.0),
            Vec3::new(2.0, 3.0, 10.0),
            Vec3::new(2.0, 3.0, 0.1),
        ],
        Vec3::new(-1.0, 0.0, 0.0),
        NO_SUB_AREA_LINK,
    );
    let g = MzbCollisionGeometry::from_block(b);
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (-2.0, 0.0, 0.0),
        (1.0, 0.0),
        6.0,
        60.0,
        RUN,
    );
    assert!(
        t.last().unwrap().0 .0 < 2.0 - 0.2,
        "corner caught body: x={:.2}",
        t.last().unwrap().0 .0
    );
}

#[test]
fn embedded_start_recovers_and_never_tunnels() {
    let g = flat_with_wall(2.0, 3.0, NO_SUB_AREA_LINK);
    // Start embedded in the wall (body radius 0.4, face at x=2).
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (1.75, 0.0, 0.0),
        (1.0, 0.0),
        4.0,
        60.0,
        RUN,
    );
    assert!(
        t.last().unwrap().0 .0 < 2.0,
        "never crossed: x={:.3}",
        t.last().unwrap().0 .0
    );

    let t = walk(
        &g,
        &ObstacleSet::default(),
        (1.75, 0.0, 0.0),
        (-1.0, 0.0),
        2.0,
        60.0,
        RUN,
    );
    assert!(
        t.last().unwrap().0 .0 < 1.0,
        "walking away works: x={:.2}",
        t.last().unwrap().0 .0
    );
}

#[test]
fn suppressed_shell_is_walk_through() {
    let mut g = flat_with_wall(2.0, 3.0, 7);
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (-2.0, 0.0, 0.0),
        (1.0, 0.0),
        4.0,
        60.0,
        RUN,
    );
    assert!(
        t.last().unwrap().0 .0 < 2.0,
        "blocks while unsuppressed: x={:.2}",
        t.last().unwrap().0 .0
    );

    g.set_suppressed(Some(7));
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (-2.0, 0.0, 0.0),
        (1.0, 0.0),
        4.0,
        60.0,
        RUN,
    );
    assert!(
        t.last().unwrap().0 .0 > 4.0,
        "suppressed shell passes: x={:.2}",
        t.last().unwrap().0 .0
    );
}

// ---------------------------------------------------------------------------
// Ramps / slopes (plan §4 live)
// ---------------------------------------------------------------------------

#[test]
fn ramp_40_degrees_walks_up_free() {
    let top = 4.0 * (40f32.to_radians().tan());
    let g = ramp(2.0, 6.0, top);
    for hz in [60.0f32, 30.0] {
        let t = walk(
            &g,
            &ObstacleSet::default(),
            (-2.0, 0.0, 0.0),
            (1.0, 0.0),
            15.0,
            hz,
            RUN,
        );
        let ys = ys(&t);
        assert_eq!(reversals(&ys), 0, "zig on ramp hz={hz}");
        let (x, _y, z) = t.last().unwrap().0;
        assert!(
            x > 6.5 && (-z - top).abs() < 0.05,
            "ramp free hz={hz}: x={x:.2} h={:.2}",
            -z
        );
    }
}

/// A 65 degree face is a wall under the 60 degree rule: it blocks, and the
/// walker never climbs it.
#[test]
fn steep_65_degree_face_blocks() {
    let top = 4.0 * (65f32.to_radians().tan());
    let g = ramp(2.0, 6.0, top);
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (-2.0, 0.0, 0.0),
        (1.0, 0.0),
        8.0,
        60.0,
        RUN,
    );
    let (x, _y, z) = t.last().unwrap().0;
    assert!(x < 2.5, "steep held: x={x:.2}");
    assert!(-z < 0.5, "did not climb: h={:.2}", -z);
}

/// A 30 degree oblique wall: the slide keeps full speed along it — no stall,
/// and the walk covers the same ground as a straight one in the same time.
#[test]
fn oblique_wall_slide_keeps_full_speed() {
    let mut b = MzbCollisionBlock::default();
    quad(
        &mut b,
        [
            Vec3::new(-10.0, 0.0, -10.0),
            Vec3::new(10.0, 0.0, -10.0),
            Vec3::new(10.0, 0.0, 10.0),
            Vec3::new(-10.0, 0.0, 10.0),
        ],
        Vec3::Y,
        NO_SUB_AREA_LINK,
    );
    // A wall from (4,-6) to (8,6): 30 degrees off the x axis.
    let a = Vec3::new(4.0, 0.0, -6.0);
    let c = Vec3::new(8.0, 0.0, 6.0);
    quad(
        &mut b,
        [a, c, c + Vec3::Y * 3.0, a + Vec3::Y * 3.0],
        (c - a).cross(Vec3::Y).normalize(),
        NO_SUB_AREA_LINK,
    );
    let g = MzbCollisionGeometry::from_block(b);
    // Walk straight at the wall's middle: it slides along, never stalls.
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (-4.0, 0.0, -3.0),
        (1.0, 0.0),
        6.0,
        60.0,
        RUN,
    );
    let ys = ys(&t);
    assert_eq!(reversals(&ys), 0);
    // The x advance over the slide must stay a large fraction of free speed:
    // sliding along a 30 degree wall keeps cos(30) ~ 0.87 of the x component.
    let (x, _y, z) = t.last().unwrap().0;
    assert!(x > -4.0 + 0.6 * RUN * 6.0, "slide stalled: x={x:.2}");
    assert!((-z).abs() < 0.01, "no lift on the slide: h={:.3}", -z);
}

// ---------------------------------------------------------------------------
// Falls / ledges / holes (plan §4 live)
// ---------------------------------------------------------------------------

/// Walk off a ledge: Airborne under FallModel, lands on the lower floor within
/// one tick of the analytic time.
#[test]
fn walk_off_a_ledge_falls_and_lands() {
    let g = ledge(2.0, 3.0);
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (0.5, 0.0, 0.0),
        (1.0, 0.0),
        6.0,
        60.0,
        RUN,
    );
    let ys = ys(&t);
    // The fall starts at the lip: find the first tick below the upper floor.
    let fall_start = ys.iter().position(|&h| h < -0.05).expect("never fell");
    assert!(fall_start > 0, "fell before reaching the lip");
    let dt = 1.0 / 60.0;
    // Analytic landing: 0.5 g t^2 = 3 (pre-terminal at this height).
    let t_analytic = (2.0 * 3.0 / FallModel::default().g).sqrt();
    let landed_at = ys[fall_start..]
        .iter()
        .position(|&h| (-h - 3.0).abs() < 0.05)
        .expect("never landed");
    let t_landed = (landed_at as f32 + 1.0) * dt;
    assert!(
        (t_landed - t_analytic).abs() <= dt + 1e-6,
        "landed at {t_landed:.3}s, analytic {t_analytic:.3}s"
    );
    // vy resets: after landing the height is flat.
    let tail = &ys[fall_start + landed_at..];
    assert!(
        tail.windows(2).all(|w| (w[1] - w[0]).abs() < 1e-6),
        "still moving after landing"
    );
}

/// A drop just past the step band is a fall; just under it is a step-down at
/// speed — pinned live, not just on the probe rule.
#[test]
fn ledge_just_past_step_band_falls_just_under_steps() {
    let g = ledge(2.0, 0.41);
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (0.5, 0.0, 0.0),
        (1.0, 0.0),
        3.0,
        60.0,
        RUN,
    );
    assert!(
        t.iter()
            .any(|(_, d)| matches!(d, VerticalDecision::Airborne { .. })),
        "0.41 drop must go airborne"
    );

    let g = ledge(2.0, 0.39);
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (0.5, 0.0, 0.0),
        (1.0, 0.0),
        3.0,
        60.0,
        RUN,
    );
    assert!(
        !t.iter()
            .any(|(_, d)| matches!(d, VerticalDecision::Airborne { .. })),
        "0.39 drop must stay grounded"
    );
    let (x, _y, z) = t.last().unwrap().0;
    assert!(
        x > 4.0 && (-z - 0.39).abs() < 0.05,
        "stepped down: x={x:.2} h={:.2}",
        -z
    );
}

/// A hole wider than the footprint: fall through it and land on the lower
/// floor; a hole narrower than the footprint is bridged by the ring probes.
#[test]
fn wide_hole_falls_narrow_hole_bridges() {
    // 0.8 wide > footprint diameter 0.5: falls.
    let g = hole(2.0, 0.8, Some(1.0));
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (0.5, 0.0, 0.0),
        (1.0, 0.0),
        4.0,
        60.0,
        RUN,
    );
    assert!(
        t.iter()
            .any(|(_, d)| matches!(d, VerticalDecision::Airborne { .. })),
        "wide hole must go airborne"
    );
    let (x, _y, z) = t.last().unwrap().0;
    assert!(
        x > 3.5 && (-z - 1.0).abs() < 0.05,
        "landed on the lower floor: x={x:.2} h={:.2}",
        -z
    );

    // 0.4 wide < footprint: bridged, no fall at all.
    let g = hole(2.0, 0.4, None);
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (0.5, 0.0, 0.0),
        (1.0, 0.0),
        4.0,
        60.0,
        RUN,
    );
    assert!(
        !t.iter()
            .any(|(_, d)| matches!(d, VerticalDecision::Airborne { .. })),
        "narrow hole must be bridged"
    );
}

/// A cliff descent is a fall, never a wall: the walker walks off and lands.
#[test]
fn cliff_descent_is_never_a_wall() {
    let g = ledge(2.0, 2.0);
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (0.5, 0.0, 0.0),
        (1.0, 0.0),
        6.0,
        60.0,
        RUN,
    );
    let (x, _y, z) = t.last().unwrap().0;
    assert!(
        x > 4.0 && (-z - 2.0).abs() < 0.05,
        "walked off and landed: x={x:.2} h={:.2}",
        -z
    );
}

// ---------------------------------------------------------------------------
// Stop / resume / settle (plan §4 live)
// ---------------------------------------------------------------------------

/// Stop mid-flight: the walker settles onto the tread at speed, then resumes
/// climbing smoothly — no dip below the current tread, no zig.
#[test]
fn stop_mid_flight_settles_then_resumes() {
    let g = staircase(8, 0.5, 0.3, false);
    // Climb ~2 treads, stop for a second on the flight, then finish.
    let t = walk_with_stops(
        &g,
        &ObstacleSet::default(),
        (-2.0, 0.0, 0.0),
        &[
            (Vec2::new(1.0, 0.0), 1.5, 1.0),
            (Vec2::new(1.0, 0.0), 12.0, 0.0),
        ],
        60.0,
        RUN,
    );
    let ys = ys(&t);
    assert_eq!(reversals(&ys), 0, "stop/resume zig: {ys:?}");
    let (x, _y, z) = t.last().unwrap().0;
    assert!(
        (-z - 8.0 * 0.3).abs() < 0.05 && x > 8.0 * 0.5,
        "did not finish the flight: x={x:.2} h={:.2}",
        -z
    );
}

/// A 0.1 sill (dead band) snaps in one tick — no ramp, no rate-limited crawl.
#[test]
fn sill_snaps_in_one_tick() {
    let g = parapet_platform(2.0, 0.1, 0.1);
    // Walk up to the lip and stop just past it: the first grounded tick on the
    // platform must already be at full height (poof snaps instantly).
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (-2.0, 0.0, 0.0),
        (1.0, 0.0),
        4.0,
        60.0,
        RUN,
    );
    let on_platform = t
        .iter()
        .position(|&((x, _, _), _)| x > 2.3)
        .expect("never crossed");
    let (_, d) = t[on_platform];
    assert!(
        matches!(d, VerticalDecision::Poof { .. }),
        "sill must poof, got {d:?}"
    );
}

/// Landing at the top of a flight never overshoots: y <= top + 0.01.
#[test]
fn landing_at_top_never_overshoots() {
    let g = staircase(8, 0.5, 0.3, false);
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (-2.0, 0.0, 0.0),
        (1.0, 0.0),
        15.0,
        60.0,
        RUN,
    );
    let top = 8.0 * 0.3;
    assert!(
        ys(&t).iter().all(|&h| h <= top + 0.01),
        "overshot the top: {:?}",
        ys(&t)
            .iter()
            .filter(|&&h| h > top + 0.01)
            .take(4)
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Doors / mobs (plan §4 live)
// ---------------------------------------------------------------------------

/// A closed door leaf blocks the sweep like a wall...
#[test]
fn door_leaf_blocks() {
    // Backstop wall far ahead: the DOOR is what must stop the walk.
    let g = flat_with_wall(12.0, 3.0, NO_SUB_AREA_LINK);
    let doors = door_wall(5.0);
    let t = walk(&g, &doors, (-2.0, 0.0, 0.0), (1.0, 0.0), 8.0, 60.0, RUN);
    assert!(
        t.last().unwrap().0 .0 < 5.0 - 0.3,
        "door held: x={:.2}",
        t.last().unwrap().0 .0
    );
}

/// ...and a closed drawbridge deck is a floor: the walker crosses it without
/// falling where an open gap would drop it.
#[test]
fn door_leaf_is_a_floor() {
    let g = hole(2.0, 4.0, None); // no MZB floor over x in [0, 4]
                                  // Without the leaf: a fall through the gap.
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (-3.0, 0.0, 0.0),
        (1.0, 0.0),
        6.0,
        60.0,
        RUN,
    );
    assert!(
        t.iter()
            .any(|(_, d)| matches!(d, VerticalDecision::Airborne { .. })),
        "open gap must drop the walker"
    );
    // With it: straight across at floor level.
    let deck = drawbridge(0.0, 4.0, 0.0);
    let t = walk(&g, &deck, (-3.0, 0.0, 0.0), (1.0, 0.0), 6.0, 60.0, RUN);
    assert!(
        !t.iter()
            .any(|(_, d)| matches!(d, VerticalDecision::Airborne { .. })),
        "deck must hold the walker up"
    );
    let (x, _y, z) = t.last().unwrap().0;
    assert!(
        x > 5.0 && (-z).abs() < 0.05,
        "crossed the deck: x={x:.2} h={:.2}",
        -z
    );
}

/// A mob circle soft-blocks at standoff...
#[test]
fn mob_circle_soft_blocks_then_push_through() {
    // Backstop wall far ahead: the MOB is what must stop phase 1.
    let g = flat_with_wall(30.0, 3.0, NO_SUB_AREA_LINK);
    let mobs = mob_circle(1, 4.0, 0.0, 0.5);
    // Phase 1: press into it for just under the push-through threshold.
    let t = walk(&g, &mobs, (-2.0, 0.0, 0.0), (1.0, 0.0), 3.0, 60.0, RUN);
    assert!(
        t.last().unwrap().0 .0 < 4.0 - 0.85,
        "mob held: x={:.2}",
        t.last().unwrap().0 .0
    );
    // Phase 2: keep pressing past the threshold — it stops blocking and we pass.
    let t = walk(&g, &mobs, (-2.0, 0.0, 0.0), (1.0, 0.0), 6.0, 60.0, RUN);
    assert!(
        t.last().unwrap().0 .0 > 4.5,
        "push-through passed the mob: x={:.2}",
        t.last().unwrap().0 .0
    );
}

// ---------------------------------------------------------------------------
// Diagonals / strafes (plan §4 live)
// ---------------------------------------------------------------------------

/// Diagonal ascent at 30/45/60 degrees to the flight: the plane fit carries it.
#[test]
fn diagonal_ascent() {
    for deg in [30.0f32, 45.0, 60.0] {
        let a = deg.to_radians();
        let dir = (a.cos(), -a.sin()); // wire: +x with a -y component (bevy +z)
        let g = staircase(10, 0.5, 0.3, false);
        let t = walk(
            &g,
            &ObstacleSet::default(),
            (-2.0, 4.0, 0.0),
            dir,
            20.0,
            60.0,
            RUN,
        );
        // The flight is only 6 wide in z: a diagonal walk exits its side before
        // the top — assert it climbed at least partway and never zigged.
        let z = t.last().unwrap().0 .2;
        let ys = ys(&t);
        assert_eq!(reversals(&ys), 0, "zig on diagonal {deg}");
        assert!(-z > 1.5, "barely climbed at {deg}: h={:.2}", -z);
    }
}

/// Strafe along a tread edge: the lateral gradient keeps the target continuous.
#[test]
fn strafe_along_tread_edge() {
    let g = staircase(8, 0.5, 0.3, false);
    // Walk +x hugging the flight's near side (z = -2.6 in bevy = wire y +2.6).
    let t = walk(
        &g,
        &ObstacleSet::default(),
        (-2.0, 2.6, 0.0),
        (1.0, 0.0),
        15.0,
        60.0,
        RUN,
    );
    let ys = ys(&t);
    assert_eq!(reversals(&ys), 0, "strafe zig: {ys:?}");
    let (x, _y, z) = t.last().unwrap().0;
    assert!(
        (-z - 8.0 * 0.3).abs() < 0.05 && x > 8.0 * 0.5,
        "strafed up the flight: x={x:.2} h={:.2}",
        -z
    );
}
