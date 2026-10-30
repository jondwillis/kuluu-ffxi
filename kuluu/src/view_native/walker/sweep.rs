//! Body sweep: horizontal slide against MZB wall triangles.
//!
//! Ported from the legacy `dat_mzb::wall_clip_wire` core, minus its step lift,
//! face_top validation and pending_floor — vertical authority lives in
//! `step.rs`, and a wall is anything with normal.y < FLOOR_COS (the 45 degree
//! rule the geometry side already applies) except faces standing entirely
//! inside the step band, which are risers, not walls. Bevy xz throughout; y up.

use bevy::math::{Vec2, Vec3};
use kuluu_render::dat_mzb::MzbCollisionGeometry;

use super::consts::*;

/// A source of wall-class contacts for the body sweep. MZB
/// zone geometry is one; closed door leaves add their triangles on top.
pub trait WallSource {
    /// Nearest wall-class triangle within `r` of `center`: `(dist_sq, normal)`
    /// for the slide re-projection (the 45 degree floor/wall rule lives in the
    /// source — MZB's authored normals, door winding normals). Terrain faces
    /// whose top edge meets the body point are step-band risers and do not
    /// answer; `lift_yamls` raises that anchor for a step-up assist retry's
    /// raised pose (`kuluu_render::dat_mzb::nearest_wall_contact`; pass 0.0 for
    /// every query from a pose on the ground). Authored blocker faces answer at
    /// any height.
    fn nearest_wall(
        &self,
        center: Vec3,
        r: f32,
        feet_y: f32,
        lift_yamls: f32,
    ) -> Option<(f32, Vec3)>;
}

impl WallSource for MzbCollisionGeometry {
    fn nearest_wall(
        &self,
        center: Vec3,
        r: f32,
        feet_y: f32,
        lift_yamls: f32,
    ) -> Option<(f32, Vec3)> {
        self.nearest_wall_contact(center, r, feet_y, lift_yamls)
    }
}

/// Nearest wall contact for the sphere-chain body standing at `xz` with feet
/// at `feet_y`, across every loaded block. The chain covers feet to BODY_HEIGHT
/// without gaps; low geometry is suppressed by the step-band riser rule in the
/// source, not by leaving the body's shins unmeasured (a coverage gap under a
/// tall face let the body walk into wedges and fall out underneath them).
fn body_contact(
    src: &impl WallSource,
    xz: Vec2,
    feet_y: f32,
    r: f32,
    lift_yamls: f32,
) -> Option<(f32, Vec3)> {
    let lower = Vec3::new(xz.x, feet_y + LOWER_CENTER_OFFSET, xz.y);
    let middle = Vec3::new(xz.x, feet_y + MIDDLE_CENTER_OFFSET, xz.y);
    let upper = Vec3::new(xz.x, feet_y + UPPER_CENTER_OFFSET, xz.y);
    let mut best: Option<(f32, Vec3)> = None;
    for center in [lower, middle, upper] {
        if let Some(c) = src.nearest_wall(center, r, feet_y, lift_yamls) {
            if best.is_none_or(|(bd, _)| c.0 < bd) {
                best = Some(c);
            }
        }
    }
    best
}

/// Sweep the body from `xz` along horizontal `d`. Returns the clear fraction
/// of `d` and the blocking contact at the stop (None when the whole sweep is
/// clear). Coarse march by 1/8 radius then bisect.
///
/// A face the body is not moving into cannot block: the walker rests at
/// standoff ~R beside a wall, and a parallel slide re-detecting that same
/// wall at t≈0 every iteration (triangle-seam distance dips) is the
/// stuck-on-walls / walking-in-place bug — only an opposing face stops the
/// sweep; grazes and partings pass through. Two faces pinching a gap narrower
/// than the body still stop it (PINCH slop).
///
/// The march cap of ~1/8 radius per probe keeps the check dense: one check
/// at the segment end per in-game tick let the body embed before "blocked"
/// registered.
fn body_sweep(
    src: &impl WallSource,
    xz: Vec2,
    feet_y: f32,
    d: Vec2,
    lift_yamls: f32,
) -> (f32, Option<(f32, Vec3)>) {
    let len = d.length();
    if len < 1e-6 {
        return (1.0, None);
    }
    let r_eff = BODY_RADIUS - 1e-4;
    let blocked = |t: f32| -> Option<(f32, Vec3)> {
        let c = body_contact(src, xz + d * t, feet_y, r_eff, lift_yamls)?;
        // Approach test against the unit XZ normal scaled by segment length (see
        // INTO_FACE_APPROACH_SLOP): contacts under this approach fraction pass, and only real
        // closing motion - or accumulated pinch penetration - can stop a sample pose. An
        // unscaled `< 0` here turns post-clip dot(noise) into a wall stop: the bisection then
        // collapses toward t=0 from a resting standoff contact, every slide iteration re-blocks
        // on its own clipped velocity, and travel along any face whose normal is not exactly
        // perpendicular to it creeps at ~1e-5/tick instead of sliding.
        let n2 = Vec2::new(c.1.x, c.1.z);
        let into_face = if n2.length() > 1e-6 {
            d.dot(n2 / n2.length()) < -INTO_FACE_APPROACH_SLOP * len
        } else {
            false
        };
        let pinched = r_eff - c.0.sqrt() > PINCH_PENETRATION_SLOP;
        if !into_face && !pinched {
            return None;
        }
        Some(c)
    };
    let step = ((BODY_RADIUS * 0.125) / len).min(1.0);
    let mut t_clear = 0.0f32;
    let mut t_hit: Option<f32> = None;
    let mut t = 0.0f32;
    loop {
        t = (t + step).min(1.0);
        if blocked(t).is_some() {
            t_hit = Some(t);
            break;
        }
        t_clear = t;
        if t >= 1.0 {
            break;
        }
    }
    let Some(mut hi) = t_hit else {
        return (1.0, None);
    };
    let mut lo = t_clear;
    for _ in 0..8 {
        let mid = (lo + hi) / 2.0;
        if blocked(mid).is_some() {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    (lo, blocked(hi))
}

/// Push the body out of any wall it starts embedded in (past DEPEN_SLOP), with
/// the total kick capped at DEPEN_MAX_PUSH per tick. This path exists for bad
/// server seeds and mid zone-swap embeds, not for normal walking: an uncapped
/// push teleports along a riser face that passes through the body, which reads
/// as the "pop" and sideways drift on descent.
///
/// The escape is oriented by measurement, not by the authored normal: shipped meshes
/// carry wall normals on both sides of the body (winding varies per mesh), and the old
/// blind push along the raw normal shoved walkers THROUGH the very faces they pressed on:
/// the tick after a slide left contact, an inward-facing parapet normal kicked the pose
/// the full DEPEN cap into the wall (measured 0.15 yalm at Bastok Mines xz(+45.18,+84.98),
/// USER's "walk though it.. nothing stops you" / barrel-cap mount trace). Both sides are
/// probed; whichever increases the chain distance is taken, and if neither helps (a thin
/// sheet with contact both ways) the pose holds instead of being squeezed.
fn depenetrate(src: &impl WallSource, mut xz: Vec2, feet_y: f32, lift_yamls: f32) -> Vec2 {
    let probe_r = BODY_RADIUS - DEPEN_SLOP;
    // Beyond the chain radius distance stops resolving; saturating there (instead of
    // INFINITY) keeps ties falling back to the raw normal for genuinely deep embeds, where
    // neither side measures a gradient anyway.
    let dist_at = |xz: Vec2| -> f32 {
        body_contact(src, xz, feet_y, probe_r + 1e-6, lift_yamls)
            .map_or(probe_r + 1.0, |(d2, _)| d2.sqrt())
    };
    let mut pushed = 0.0f32;
    for _ in 0..DEPEN_ITERATIONS {
        if pushed >= DEPEN_MAX_PUSH - 1e-6 {
            break;
        }
        let Some(c) = body_contact(src, xz, feet_y, probe_r, lift_yamls) else {
            return xz;
        };
        let n2 = Vec2::new(c.1.x, c.1.z);
        let n2l = n2.length();
        if n2l < 1e-4 {
            return xz;
        }
        let push = ((BODY_RADIUS - c.0.sqrt()) + 0.01).min(DEPEN_MAX_PUSH - pushed);
        let dir = n2 / n2l;
        let d_now = dist_at(xz);
        let d_plus = dist_at(xz + dir * push);
        let d_minus = dist_at(xz - dir * push);
        let (plus_ok, minus_ok) = (d_plus > d_now + 1e-6, d_minus > d_now + 1e-6);
        match (plus_ok, minus_ok) {
            (true, true) if d_plus >= d_minus => xz += dir * push,
            (true, false) => xz += dir * push,
            (false, true) => xz -= dir * push,
            // Neither side measures farther from the face than here: hold. The old blind
            // shove along the authored normal is what walked users through their walls.
            _ => return xz,
        }
        pushed += push;
    }
    xz
}

/// How the slide loop ended. `step` maps this onto the tick's
/// `HorizontalOutcome` together with the returned displacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SweepExit {
    /// No wall stopped the move: either the whole input was clear, or the
    /// body ran exactly into a face and the displacement is the full input.
    Clean,
    /// A wall clipped the move and the body slid along it; the displacement is
    /// nonzero and shorter than (or rotated from) the input.
    Slid,
    /// Stopped at the contact with no usable slide direction: a degenerate
    /// face normal, or a head-on hit whose clip consumed the remainder.
    Hold,
    /// Boxed in: the slide points back more than 90 degrees from the input, or
    /// two planes' crease dead-ends.
    Reversal,
}

/// Clamp a horizontal move against walls (MZB + closed doors): depenetrate an
/// embedded start, then sweep + slide over the sphere chain (Quake III
/// `PM_SlideMove` clip planes).
/// A slide against a face keeps full speed along it. Returns the allowed
/// displacement in bevy xz and which exit of the slide loop produced it;
/// `feet_y` is the pre-move height (the body's vertical position does not
/// change inside the loop — `step.rs` owns that after the sweep).
///
/// The wanted direction is the original input; a slide that ends up pointing
/// more than 90° away from it means the body is boxed in — stop rather than
/// crab backwards. Clip planes accumulate across slide iterations: each wall
/// touched adds its normal, and the remaining velocity is clipped so it does
/// not point into any plane hit. On a single flat wall this is a plain slide;
/// at an inside corner it rides the crease; only a true reversal stops the
/// body. A grazed wall already in the set is skipped, with a small angular
/// tolerance keeping numerical twins on a flat wall from filling the set and
/// confusing the crease logic. Clipping against a later plane that
/// re-introduces motion into an earlier one follows the crease — the
/// direction perpendicular to both, oriented downstream; a crease still
/// pointing into the second plane is a dead-end pocket (Reversal), otherwise
/// the body rides it. A zero-length velocity exits with the exit the clip
/// that produced it already decided; a stop exactly at the face and a
/// degenerate face normal end the loop at the held position. A head-on hit
/// leaves no slide direction after the clip (Hold).
/// `lift_yamls` is how far above the ground the pose this sweep runs for stands
/// (0.0 from a ground pose; `step.rs` passes its step-up assist accumulator on
/// the retry), raising the height-rejection anchor of terrain faces.
pub fn sweep(
    src: &impl WallSource,
    xz: Vec2,
    feet_y: f32,
    d_in: Vec2,
    lift_yamls: f32,
) -> (Vec2, SweepExit) {
    let mut p = depenetrate(src, xz, feet_y, lift_yamls);
    let mut d = d_in;
    let want_len = d.length();
    let want_dir = if want_len > 1e-6 {
        d / want_len
    } else {
        Vec2::ZERO
    };

    let mut normals: [Vec2; 4] = [Vec2::ZERO; 4];
    let mut n_count: usize = 0;
    let mut exit = SweepExit::Clean;

    for _ in 0..SLIDE_ITERATIONS {
        if d.length() < 1e-6 {
            break;
        }
        let (t, hit) = body_sweep(src, p, feet_y, d, lift_yamls);
        p += d * t;
        let Some(hit) = hit else {
            return (p - xz, SweepExit::Clean);
        };
        let rem = d * (1.0 - t);
        let rem_len = rem.length();
        if rem_len < 1e-6 {
            break;
        }

        let n2_raw = Vec2::new(hit.1.x, hit.1.z);
        let n2l = n2_raw.length();
        if n2l < 1e-4 {
            break;
        }
        let n2 = n2_raw / n2l;

        let is_duplicate = normals[..n_count].iter().any(|prev| prev.dot(n2) > 0.98);
        if !is_duplicate && n_count < normals.len() {
            normals[n_count] = n2;
            n_count += 1;
        }

        let rem_len2 = rem.length();
        let mut vel = rem;
        'planes: for i in 0..n_count {
            if vel.dot(normals[i]) >= 0.0 {
                continue;
            }
            let mut v = vel - normals[i] * vel.dot(normals[i]);
            for j in 0..n_count {
                if j == i {
                    continue;
                }
                if v.dot(normals[j]) < 0.0 {
                    let crease = Vec2::new(-normals[i].y, normals[i].x);
                    let crease = if crease.dot(rem) < 0.0 {
                        -crease
                    } else {
                        crease
                    };
                    if crease.dot(normals[j]) < -1e-3 {
                        vel = Vec2::ZERO;
                        exit = SweepExit::Reversal;
                        break 'planes;
                    }
                    v = crease * rem_len2;
                }
            }
            vel = v;
        }

        if want_dir != Vec2::ZERO && vel.length() > 1e-6 {
            let vd = vel / vel.length();
            if vd.dot(want_dir) < -0.01 {
                exit = SweepExit::Reversal;
                break;
            }
        }
        if vel.length() < 1e-6 {
            exit = SweepExit::Hold;
            break;
        }
        d = vel;
    }

    (p - xz, exit)
}

/// Ceiling hold: before applying a rise from `feet_old` to
/// `feet_new`, reject it when any triangle (any face class) sits in the slab
/// `(feet_old + BODY_HEIGHT, feet_new + BODY_HEIGHT]` at the feet column —
/// the body top would push into geometry. A descent does not trip this.
pub fn ceiling_holds(geom: &MzbCollisionGeometry, xz: Vec2, feet_old: f32, feet_new: f32) -> bool {
    if feet_new <= feet_old + 1e-6 {
        return false;
    }
    geom.any_tri_in_column_slab(xz, feet_old + BODY_HEIGHT, feet_new + BODY_HEIGHT)
}
