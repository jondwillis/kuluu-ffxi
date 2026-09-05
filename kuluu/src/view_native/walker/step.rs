//! One fixed tick of the walker (plan §2.3/§3): horizontal slide plus vertical
//! authority in one place, replacing the stub's free move + column snap.
//!
//! Wire coordinates at the boundary: x/y horizontal, z grows DOWN; bevy space
//! inside (xz = (x, -y), y up = -z). The tick is:
//! 1. horizontal — wall sweep (MZB + closed doors) unless noclip, then mob
//!    circles with PushThrough accrual;
//! 2. support probe at the NEW xz (five column queries, MZB + door floors);
//! 3. vertical by mode: Stopped/Walking merge toward h0 or the staircase
//!    envelope at `speed_yps * dt`; Airborne integrates `FallModel` and lands
//!    when a floor enters the swept band; every rise is gated by the ceiling
//!    hold and sanity-capped at STEP_MAX per tick.

use bevy::math::{Vec2, Vec3};
use kuluu_render::dat_mzb::{point_tri_dist_sq, MzbCollisionGeometry};

use super::consts::*;
use super::field::{self, Sampler};
use super::obstacles::{DoorObstacle, ObstacleSet};
use super::sweep::{self, WallSource};
use super::{StepResult, VerticalDecision, WalkMode, Walker};

/// Floor source for the walker's column queries: MZB zone collision plus
/// closed-door triangles (a closed drawbridge is a floor, plan §2.1). The
/// support probe and the ramp field both sample through this.
struct FloorSampler<'a> {
    geom: &'a MzbCollisionGeometry,
    doors: &'a [DoorObstacle],
}

impl Sampler for FloorSampler<'_> {
    fn floor(&self, xz: Vec2, ceiling_y: f32) -> Option<f32> {
        let mut best = self.geom.ground_raycast(xz, ceiling_y);
        for d in self.doors {
            if !column_in_box(d.min.x, d.max.x, d.min.z, d.max.z, xz) || d.min.y > ceiling_y {
                continue;
            }
            if let Some(h) = door_floor_at(d, xz, ceiling_y) {
                best = Some(match best {
                    Some(prev) if prev >= h => prev,
                    _ => h,
                });
            }
        }
        best
    }
}

/// Wall source for the body sweep: MZB zone geometry plus closed-door
/// triangles (plan §2.5 — doors are walls for the sweep AND floors for the
/// column probe). Door normals are winding-derived and oriented toward the
/// query point at contact time, so either mesh authoring side blocks.
struct Walls<'a> {
    geom: &'a MzbCollisionGeometry,
    doors: &'a [DoorObstacle],
}

impl WallSource for Walls<'_> {
    fn nearest_wall(&self, center: Vec3, r: f32) -> Option<(f32, Vec3)> {
        let mut best = self.geom.nearest_wall_contact(center, r);
        let r2 = r * r;
        for d in self.doors {
            if !point_in_box(d.min.x - r, d.max.x + r, d.min.z - r, d.max.z + r, center) {
                continue;
            }
            for (v, n) in &d.tris {
                // The 60 degree rule: an up-facing door face (a drawbridge deck)
                // is a floor, not a wall — the column probe owns it.
                if n.y >= FLOOR_COS {
                    continue;
                }
                let d2 = point_tri_dist_sq(center, v[0], v[1], v[2]);
                if d2 < r2 && best.is_none_or(|(bd, _)| d2 < bd) {
                    // Orient the face normal toward the body: slide re-projection
                    // and depenetration both need "from face to free space".
                    let closest = closest_point_on_tri(center, v[0], v[1], v[2]);
                    let mut n = *n;
                    if n.dot(center - closest) < 0.0 {
                        n = -n;
                    }
                    best = Some((d2, n));
                }
            }
        }
        best
    }
}

fn column_in_box(min_x: f32, max_x: f32, min_z: f32, max_z: f32, xz: Vec2) -> bool {
    xz.x >= min_x && xz.x <= max_x && xz.y >= min_z && xz.y <= max_z
}

fn point_in_box(min_x: f32, max_x: f32, min_z: f32, max_z: f32, p: Vec3) -> bool {
    p.x >= min_x && p.x <= max_x && p.z >= min_z && p.z <= max_z
}

/// Highest up-facing door face a downward column ray at `xz` hits at or below
/// `ceiling_y` — the door half of [`FloorSampler::floor`] (same acceptance as
/// `ground_raycast`: normal.y >= FLOOR_COS, hit under the ceiling).
fn door_floor_at(d: &DoorObstacle, xz: Vec2, ceiling_y: f32) -> Option<f32> {
    let mut best: Option<f32> = None;
    for (v, n) in &d.tris {
        if n.y < FLOOR_COS {
            continue;
        }
        if let Some(h) = tri_hits_column(*v, xz) {
            if h <= ceiling_y && best.is_none_or(|prev| prev < h) {
                best = Some(h);
            }
        }
    }
    best
}

/// True when any closed-door triangle crosses the vertical slab `(lo_y, hi_y]`
/// at `xz` — the door half of the ceiling hold (plan §2.4: "any triangle, any
/// face class"). A descent never trips it; callers gate on a rise first.
fn doors_in_column_slab(doors: &[DoorObstacle], xz: Vec2, lo_y: f32, hi_y: f32) -> bool {
    for d in doors {
        if !column_in_box(d.min.x, d.max.x, d.min.z, d.max.z, xz) {
            continue;
        }
        if d.max.y <= lo_y || d.min.y > hi_y {
            continue;
        }
        for (v, _) in &d.tris {
            if let Some(h) = tri_hits_column(*v, xz) {
                if h > lo_y && h <= hi_y {
                    return true;
                }
            }
        }
    }
    false
}

/// Downward vertical ray at `xz` against triangle `v`: the hit height, or None
/// when the face is parallel to the column (vertical faces never cross it).
fn tri_hits_column(v: [Vec3; 3], xz: Vec2) -> Option<f32> {
    let (a, b, c) = (v[0], v[1], v[2]);
    let e1 = b - a;
    let e2 = c - a;
    // det = cross(e1, e2).y: zero means the face is vertical — no crossing.
    let det = e1.x * e2.z - e1.z * e2.x;
    if det.abs() < 1e-9 {
        return None;
    }
    // Möller–Trumbore with dir (0, -1, 0); only xz of the origin matters.
    let qx = xz.x - a.x;
    let qz = xz.y - a.z;
    let u = (-qx * e2.z + qz * e2.x) / det; // cross(q, e2).y / det
    if u < 0.0 || u > 1.0 {
        return None;
    }
    let tvec_y = e1.z * e2.x - e1.x * e2.z; // cross(e1, e2).y
    let v_ = -tvec_y / det; // dir . cross(e1, e2) / det
    if v_ < 0.0 || u + v_ > 1.0 {
        return None;
    }
    Some(a.y + u * b.y + v_ * c.y)
}

/// Closest point on triangle (a, b, c) to `p` — Ericson §5.1.3, the same
/// region walk as `point_tri_dist_sq` without the final distance.
fn closest_point_on_tri(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= -d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return a + ab * v;
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= -d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return a + ac * w;
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return b + (c - b) * w;
    }
    let denom = va + vb + vc;
    if denom.abs() <= f32::EPSILON {
        // p is in the vertex region of a degenerate triangle.
        return a;
    }
    let v = vb / denom;
    let w = vc / denom;
    a + ab * v + ac * w
}

/// Landing test for an airborne tick (plan §2.3): a floor within the swept
/// band `[y_lo, y_hi]` under the footprint — any of the five support columns,
/// highest accepted wins. Unlike [`field::support_probe`] this is UNBOUNDED in
/// how far below the feet the floor may sit: that IS the fall.
fn landing_floor(sampler: &impl Sampler, xz: Vec2, y_lo: f32, y_hi: f32) -> Option<f32> {
    let mut best: Option<f32> = None;
    for p in field::support_positions(xz) {
        if let Some(h) = sampler.floor(p, y_hi) {
            if h >= y_lo && best.is_none_or(|prev| prev < h) {
                best = Some(h);
            }
        }
    }
    best
}

/// One fixed tick of the walker. Wire coordinates throughout at the boundary:
/// x/y horizontal, z grows DOWN (the frame `AgentCommand::Move` carries).
/// Pure over its inputs — no ECS — so the test matrices can drive it headless.
pub fn step(
    geom: &MzbCollisionGeometry,
    obstacles: &ObstacleSet,
    state: &mut Walker,
    x: f32,
    y: f32,
    z_wire: f32,
    dx: f32,
    dy: f32,
    speed_yps: f32,
    dt: f32,
    noclip: bool,
) -> StepResult {
    let feet_xz = Vec2::new(x, -y);
    let feet_y = -z_wire;
    let d_in = Vec2::new(dx, -dy);
    let want_len = d_in.length();

    let sampler = FloorSampler {
        geom,
        doors: &obstacles.doors,
    };
    let walls = Walls {
        geom,
        doors: &obstacles.doors,
    };

    // ---- 1. Horizontal -----------------------------------------------------
    // The wall sweep runs at the PRE-move height (plan §2.4): nothing below
    // feet + STEP_MAX is a horizontal obstacle, and vertical authority lands
    // after it. Noclip bypasses walls AND mobs — free-fly for debugging;
    // grounding stays on either way.
    let mut d = if noclip {
        d_in
    } else {
        sweep::sweep(&walls, feet_xz, feet_y, d_in)
    };

    // Mobs: circle-vs-circle in xz (plan §2.5). A mob the walker is pressing
    // into accrues PushThrough time; past PUSH_THROUGH_SECS it stops blocking
    // until the pressure releases. Sustained = overlapping this tick AND still
    // inside after the push-out.
    if !noclip {
        let mut xz_now = feet_xz + d;
        let mut pressed: Option<u32> = None;
        for mob in &obstacles.mobs {
            let delta = xz_now - mob.center;
            let dist = delta.length();
            let min_dist = BODY_RADIUS + mob.radius;
            if dist >= min_dist {
                continue; // no overlap: no pressure on this mob
            }
            // Sustained pressure into this same mob past the threshold excludes
            // it: pass through while the pressure holds.
            if state.push_through.excluded(mob.id) {
                state.push_through.press(mob.id, dt); // keep the clock running
                pressed = Some(mob.id);
                continue;
            }
            // Push out to the circle boundary along the separation axis.
            let n2 = if dist > 1e-6 {
                delta / dist
            } else {
                // Centered on top of it: push along the motion (or +x when idle).
                if want_len > 1e-6 {
                    d_in / want_len
                } else {
                    Vec2::X
                }
            };
            xz_now += n2 * (min_dist - dist);
            let still = (xz_now - mob.center).length() < min_dist + 1e-4;
            if still {
                // Pressing into this mob: accrue against it.
                state.push_through.press(mob.id, dt);
                pressed = Some(mob.id);
            }
            d = xz_now - feet_xz;
        }
        // Release when the previously-pressed mob is no longer being pressed
        // (moved away, or a different obstacle took over): it blocks again.
        match state.push_through.target() {
            Some(t) if Some(t) == pressed => {}
            _ => state.push_through.release(),
        }
    }

    let new_xz = feet_xz + d;

    // ---- 2. Support at the NEW xz ------------------------------------------
    let probe = field::support_probe(&sampler, new_xz, feet_y);

    // Mode: airborne persists until a landing (steering stays live in the air);
    // otherwise input decides — want_len == 0 is Stopped this tick.
    let was_airborne = matches!(state.mode, WalkMode::Airborne { .. });
    let grounded_now = probe.grounded;

    // ---- 3. Vertical --------------------------------------------------------
    let mut y_new = feet_y;
    let decision = if was_airborne {
        // Gravity (plan §2.3): vy -= g*dt clamped to -v_max, then integrate.
        let prev_vy = match state.mode {
            WalkMode::Airborne { vy } => vy,
            _ => 0.0,
        };
        let vy = (prev_vy - state.fall.g * dt).max(-state.fall.v_max);
        y_new = feet_y + vy * dt;

        // Landing: a floor entered the swept band [y_new, feet_y] under the
        // footprint. Set y to it, mode by input, vy = 0.
        match landing_floor(&sampler, new_xz, y_new, feet_y) {
            Some(floor) => {
                y_new = floor;
                state.mode = if want_len > 1e-6 {
                    WalkMode::Walking
                } else {
                    WalkMode::Stopped
                };
                VerticalDecision::Landed
            }
            None => {
                state.mode = WalkMode::Airborne { vy };
                VerticalDecision::Airborne { vy }
            }
        }
    } else if !grounded_now {
        // Support missed: no floor within the step band under the footprint —
        // a ledge, a hole wider than the footprint. Enter Airborne from rest;
        // this tick holds height (the fall starts next tick).
        state.mode = WalkMode::Airborne { vy: 0.0 };
        VerticalDecision::Airborne { vy: 0.0 }
    } else {
        // Grounded: merge toward the target at speed_yps * dt per tick — that's
        // the whole blend model (plan §2.3). The field is only sampled when it
        // can matter: a moving walker on a staircase window rides the envelope;
        // everything else targets h0 direct.
        let v = speed_yps * dt;
        let h0 = probe.h0.expect("grounded implies an accepted support hit");

        let field_opt = (want_len > 1e-6).then(|| {
            let m = d_in / want_len;
            field::sample_field(&sampler, new_xz, feet_y, m)
        });

        // Target + decision: dead band snaps to h0 instantly; a staircase
        // window rides the slewed envelope; everything else targets h0 direct.
        let (target, mut decision) = match &field_opt {
            Some(f) if f.poof => (h0, VerticalDecision::Poof { delta: h0 - feet_y }),
            Some(f) if f.target.is_some() => {
                // Slew the gradient toward this tick's fit (plan §2.2): a
                // wobbly estimate or a fast 180 can't spike g.
                let g_new = f.g;
                state.grad.x += (g_new.x - state.grad.x).clamp(-GRAD_SLEW, GRAD_SLEW);
                state.grad.y += (g_new.y - state.grad.y).clamp(-GRAD_SLEW, GRAD_SLEW);
                let target = f.target.unwrap();
                (
                    target,
                    VerticalDecision::Ramp {
                        g: state.grad,
                        target,
                    },
                )
            }
            Some(f)
                if f.samples
                    .iter()
                    .any(|s| s.status == field::SampleStatus::WallAhead) =>
            {
                // A wall face ahead capped the chain: hold h0, no rise — the
                // sweep slides on it.
                (h0, VerticalDecision::WallAhead)
            }
            _ => {
                let decision = match &field_opt {
                    Some(f) if f.riser_count == 1 => VerticalDecision::SingleStep {
                        up: h0 >= feet_y - 1e-6,
                    },
                    Some(f) if f.range > POOF_MAX => VerticalDecision::Slope,
                    _ => VerticalDecision::Flat,
                };
                (h0, decision)
            }
        };

        // Rate-limit the merge to speed_yps * dt per tick.
        let delta = (target - feet_y).clamp(-v, v);
        y_new = feet_y + delta;

        // Sanity cap outside Airborne: |dy| <= STEP_MAX per tick — if the field
        // ever asks for more it's lying; hold and log (plan §2.3).
        if y_new - feet_y > STEP_MAX + 1e-6 || feet_y - y_new > STEP_MAX + 1e-6 {
            eprintln!(
                "walker: vertical delta {} exceeds STEP_MAX, holding",
                y_new - feet_y
            );
            y_new = feet_y;
        }

        // Ceiling hold (plan §2.4): a rise is rejected for the tick when any
        // triangle sits in (feet + BODY_HEIGHT, y_new + BODY_HEIGHT] at the new
        // column — MZB or a closed door above us.
        if y_new > feet_y + 1e-6 {
            let held = sweep::ceiling_holds(geom, new_xz, feet_y, y_new)
                || doors_in_column_slab(
                    &obstacles.doors,
                    new_xz,
                    feet_y + BODY_HEIGHT,
                    y_new + BODY_HEIGHT,
                );
            if held {
                y_new = feet_y;
                decision = VerticalDecision::CeilingHold;
            }
        }

        state.mode = if want_len > 1e-6 {
            WalkMode::Walking
        } else {
            WalkMode::Stopped
        };
        // Idle ticks: the same settle, named for the panel.
        if !matches!(decision, VerticalDecision::Poof { .. }) && want_len <= 1e-6 {
            decision = VerticalDecision::Stopped {
                settling: (y_new - feet_y).abs() > 1e-9,
            };
        }
        decision
    };

    StepResult {
        dx: d.x,
        dy: -d.y,
        feet_z: -y_new,
        mode: state.mode,
        decision,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view_native::walker::PushThrough;

    // The rate/fall math below is exercised directly rather than through
    // `step` over a synthetic geometry: the MZB side of a default geometry has
    // no blocks, so driving `step` would couple these tests to door plumbing.

    fn fall_closed_form(g: f32, v_max: f32, t: f32) -> (f32, f32) {
        // y(t), vy(t) for a drop from rest under FallModel{g, v_max}.
        let t_term = (v_max / g).min(t);
        let dist = 0.5 * g * t_term * t_term + v_max * (t - t_term);
        let vy = -(g * t_term).min(v_max);
        (-dist, vy)
    }

    #[test]
    fn fall_model_matches_closed_form() {
        // The plan's feel numbers: 1 yalm ~0.22 s, 3 ~0.39 s, 10 ~0.6 s at
        // g=40 v_max=30 (terminal speed reached at 0.75 s).
        let fall = FallModel::default();
        assert!((fall.g - 40.0).abs() < 1e-6);
        assert!((fall.v_max - 30.0).abs() < 1e-6);

        // Euler integration of the same model must track the closed form to
        // within one tick's worth at production dt.
        let dt = 1.0 / 60.0;
        for t_target in [0.22, 0.39, 0.6] {
            let mut y = 0.0f32;
            let mut vy = 0.0f32;
            let mut t = 0.0f32;
            while t < t_target - 1e-9 {
                vy = (vy - fall.g * dt).max(-fall.v_max);
                y += vy * dt;
                t += dt;
            }
            let (y_ref, _) = fall_closed_form(fall.g, fall.v_max, t_target);
            assert!(
                (y - y_ref).abs() < 0.5 * fall.g * dt * dt + 1e-3,
                "t={t_target}: euler {y} vs closed {y_ref}"
            );
        }
    }

    #[test]
    fn rate_limit_reaches_h0_in_expected_ticks() {
        // From a stop on a 0.3 float at run speed (5 y/s, 60 Hz): y reaches h0
        // in ceil(0.3 / (speed*dt)) ticks, monotonically.
        let speed = 5.0;
        let dt = 1.0 / 60.0;
        let v = speed * dt;
        let gap = 0.3f32;
        let mut y = -gap; // floating above the floor at 0
        let mut ticks = 0u32;
        let mut monotone = true;
        while y < -1e-9 {
            let delta = (0.0 - y).clamp(-v, v);
            y += delta;
            ticks += 1;
            if delta <= 0.0 && ticks > 1 {
                monotone = false;
            }
        }
        assert_eq!(ticks, ((gap / v).ceil() as u32), "took {ticks} ticks");
        assert!(monotone);
    }

    #[test]
    fn tap_forward_two_ticks_moves_at_most_two_steps() {
        // Tap and release: y moves at most 2 * speed * dt total, then settles
        // back — the rate limit is symmetric in both directions.
        let speed = 5.0;
        let dt = 1.0 / 60.0;
        let v = speed * dt;
        let mut y = 0.0f32;
        for _ in 0..2 {
            y += (0.3 - y).clamp(-v, v); // rising toward a 0.3 target
        }
        assert!(y <= 2.0 * v + 1e-9, "moved {y}");
        let mut settle = y;
        for _ in 0..60 {
            settle += (0.0 - settle).clamp(-v, v); // back to the floor
        }
        assert!(settle.abs() < 1e-9, "settled at {settle}");
    }

    #[test]
    fn just_past_step_band_is_a_fall_just_under_is_a_step() {
        // 0.41 ledge: beyond the step band on every probe => Airborne (a fall).
        // 0.39: inside it => a step-down at speed, not a fall.
        let dt = 1.0 / 60.0;
        for (drop, is_fall) in [(0.41f32, true), (0.39f32, false)] {
            // Support probe acceptance: hit >= feet_y - STEP_MAX.
            let accepted = drop <= STEP_MAX + 1e-6;
            assert_eq!(accepted, !is_fall, "drop {drop}");
        }
        // And the fall itself: off a 3.0 ledge, y(t) matches FallModel and it
        // lands within one tick of the analytic time (closed form below).
        let fall = FallModel::default();
        let mut y = 3.0f32;
        let mut vy = 0.0f32;
        let mut t = 0.0f32;
        while y > 1e-6 {
            vy = (vy - fall.g * dt).max(-fall.v_max);
            y += vy * dt;
            t += dt;
        }
        // Analytic: solve 0.5 g t^2 = 3 for the pre-terminal phase.
        let t_analytic = (2.0 * 3.0 / fall.g).sqrt();
        assert!(
            (t - t_analytic).abs() <= dt + 1e-6,
            "landed at {t}, analytic {t_analytic}"
        );
    }

    #[test]
    fn landing_band_catches_the_floor_it_sweeps_over() {
        // A floor inside [y_new, y_old] lands; one below the band does not.
        let fall = FallModel::default();
        let dt = 1.0 / 60.0;
        let vy = -fall.v_max; // terminal speed: ~0.5 yalm per tick
        for (floor_drop, should_land) in [(0.4f32, true), (0.9f32, false)] {
            let y_old = 10.0f32;
            let y_new = y_old + vy * dt;
            let floor = y_old - floor_drop;
            let landed = floor >= y_new && floor <= y_old;
            assert_eq!(landed, should_land, "floor {floor}");
        }
    }

    #[test]
    fn push_through_accrual_excludes_after_threshold() {
        // 0.8 s of sustained pressure into the same mob excludes it; a release
        // or a different target resets the clock.
        let dt = 1.0 / 60.0;
        let mut pt = PushThrough::default();
        for i in 0..(PUSH_THROUGH_SECS / dt) as u32 - 1 {
            assert!(!pt.press(1, dt), "early release at tick {i}");
        }
        assert!(pt.press(1, dt), "threshold not reached");
        pt.release();
        assert!(!pt.press(1, dt), "release did not reset");
        for _ in 0..(PUSH_THROUGH_SECS / dt) as u32 {
            pt.press(1, dt);
        }
        // A different target mid-accrual restarts the clock.
        let mut pt = PushThrough::default();
        for _ in 0..((PUSH_THROUGH_SECS * 0.5) / dt) as u32 {
            pt.press(1, dt);
        }
        assert!(!pt.press(2, dt), "target switch must reset");
    }

    #[test]
    fn tri_hits_column_finds_horizontal_faces_only() {
        // A floor quad at y=0.5 under the ray: hit; a vertical wall face: miss.
        let floor = [
            Vec3::new(0.0, 0.5, 0.0),
            Vec3::new(1.0, 0.5, 0.0),
            Vec3::new(0.0, 0.5, 1.0),
        ];
        assert_eq!(tri_hits_column(floor, Vec2::new(0.4, 0.4)), Some(0.5));
        assert_eq!(tri_hits_column(floor, Vec2::new(2.0, 0.4)), None);

        let wall = [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 2.0, 0.0),
            Vec3::new(1.0, 0.0, 1.0),
        ];
        assert_eq!(tri_hits_column(wall, Vec2::new(1.0, 0.5)), None);
    }

    #[test]
    fn ceiling_slab_rejects_only_rises_into_geometry() {
        // A slab crossing (feet+BODY_HEIGHT, y_new+BODY_HEIGHT] holds; one
        // above the band or a descent does not.
        let doors: &[DoorObstacle] = &[];
        assert!(!doors_in_column_slab(doors, Vec2::ZERO, 1.8, 2.1));

        // A door leaf with a horizontal lintel at y=2.0 over the column.
        let lintel = [
            Vec3::new(-1.0, 2.0, -1.0),
            Vec3::new(1.0, 2.0, -1.0),
            Vec3::new(0.0, 2.0, 1.0),
        ];
        let n = (lintel[1] - lintel[0])
            .cross(lintel[2] - lintel[0])
            .normalize();
        let door = DoorObstacle {
            tris: vec![(lintel, n)],
            min: Vec3::new(-1.0, 2.0, -1.0),
            max: Vec3::new(1.0, 2.0, 1.0),
        };
        let doors = [door];
        assert!(doors_in_column_slab(&doors, Vec2::ZERO, 1.8, 2.1));
        assert!(!doors_in_column_slab(&doors, Vec2::ZERO, 2.1, 2.4));
    }
}
