//! Body sweep: horizontal slide against MZB wall triangles (plan §2.4).
//!
//! Ported from the legacy `dat_mzb::wall_clip_wire` core, minus its step lift,
//! face_top validation, pending_floor and 45 degree wall test — vertical
//! authority lives in `step.rs`, and a wall is anything with normal.y <
//! FLOOR_COS (the 60 degree rule the geometry side already applies). Bevy xz
//! throughout; y up.

use bevy::math::{Vec2, Vec3};
use kuluu_render::dat_mzb::MzbCollisionGeometry;

use super::consts::*;

/// A source of wall-class contacts for the body sweep (plan §2.4/§2.5). MZB
/// zone geometry is one; closed door leaves add their triangles on top.
pub trait WallSource {
    /// Nearest wall-class triangle within `r` of `center`: `(dist_sq, normal)`
    /// for the slide re-projection (the 60 degree floor/wall rule lives in the
    /// source — MZB's authored normals, door winding normals).
    fn nearest_wall(&self, center: Vec3, r: f32) -> Option<(f32, Vec3)>;
}

impl WallSource for MzbCollisionGeometry {
    fn nearest_wall(&self, center: Vec3, r: f32) -> Option<(f32, Vec3)> {
        self.nearest_wall_contact(center, r)
    }
}

/// Nearest wall contact for the two-sphere body standing at `xz` with feet at
/// `feet_y`, across every loaded block. Nothing below feet + STEP_MAX is ever
/// a horizontal obstacle: the lower sphere bottoms out exactly there.
fn body_contact(src: &impl WallSource, xz: Vec2, feet_y: f32, r: f32) -> Option<(f32, Vec3)> {
    let lower = Vec3::new(xz.x, feet_y + LOWER_CENTER_OFFSET, xz.y);
    let upper = Vec3::new(xz.x, feet_y + UPPER_CENTER_OFFSET, xz.y);
    let mut best: Option<(f32, Vec3)> = None;
    for center in [lower, upper] {
        if let Some(c) = src.nearest_wall(center, r) {
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
fn body_sweep(src: &impl WallSource, xz: Vec2, feet_y: f32, d: Vec2) -> (f32, Option<(f32, Vec3)>) {
    let len = d.length();
    if len < 1e-6 {
        return (1.0, None);
    }
    let r_eff = BODY_RADIUS - 1e-4;
    let blocked = |t: f32| -> Option<(f32, Vec3)> {
        let c = body_contact(src, xz + d * t, feet_y, r_eff)?;
        // A face we are NOT moving into cannot block. The walker rests at
        // standoff ~R beside a wall; sliding parallel used to re-detect the
        // same wall at t≈0 every iteration (triangle-seam distance dips), which
        // is the stuck-on-walls / walking-in-place bug: only an opposing face
        // stops the sweep now — grazes and partings pass through. Two faces
        // pinching a gap narrower than the body still stop it (PINCH slop).
        let n2 = Vec2::new(c.1.x, c.1.z);
        let into_face = d.dot(n2) < 0.0;
        let pinched = r_eff - c.0.sqrt() > PINCH_PENETRATION_SLOP;
        if !into_face && !pinched {
            return None;
        }
        Some(c)
    };
    // March no more than ~1/8 radius per probe: the old half-radius cap meant a
    // normal in-game tick got ONE check at the segment end — by the time
    // "blocked" registered the body was already embedded.
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
fn depenetrate(src: &impl WallSource, mut xz: Vec2, feet_y: f32) -> Vec2 {
    let mut pushed = 0.0f32;
    for _ in 0..DEPEN_ITERATIONS {
        if pushed >= DEPEN_MAX_PUSH - 1e-6 {
            break;
        }
        let Some(c) = body_contact(src, xz, feet_y, BODY_RADIUS - DEPEN_SLOP) else {
            return xz;
        };
        let n2 = Vec2::new(c.1.x, c.1.z);
        let n2l = n2.length();
        if n2l < 1e-4 {
            return xz;
        }
        let push = ((BODY_RADIUS - c.0.sqrt()) + 0.01).min(DEPEN_MAX_PUSH - pushed);
        xz += (n2 / n2l) * push;
        pushed += push;
    }
    xz
}

/// Clamp a horizontal move against walls (MZB + closed doors): depenetrate an
/// embedded start, then sweep + slide (Quake III `PM_SlideMove` clip planes).
/// A slide against a face keeps full speed along it. Returns the allowed
/// displacement in bevy xz; `feet_y` is the pre-move height (the body's
/// vertical position does not change inside the loop — `step.rs` owns that
/// after the sweep).
pub fn sweep(src: &impl WallSource, xz: Vec2, feet_y: f32, d_in: Vec2) -> Vec2 {
    let mut p = depenetrate(src, xz, feet_y);
    let mut d = d_in;
    let want_len = d.length();
    // The original desired direction. A slide that ends up pointing more than
    // 90° away from this means we're boxed in — stop rather than crab
    // backwards.
    let want_dir = if want_len > 1e-6 {
        d / want_len
    } else {
        Vec2::ZERO
    };

    // Clip planes accumulated across slide iterations (PM_SlideMove /
    // PM_ClipVelocity). Each wall we touch adds its normal; the remaining
    // velocity is clipped so it never points into any plane we've hit. On a
    // single flat wall this is a plain slide; at an inside corner it rides the
    // crease; only a true reversal stops us.
    let mut normals: [Vec2; 4] = [Vec2::ZERO; 4];
    let mut n_count: usize = 0;

    for _ in 0..SLIDE_ITERATIONS {
        if d.length() < 1e-6 {
            break;
        }
        let (t, hit) = body_sweep(src, p, feet_y, d);
        p += d * t;
        let Some(hit) = hit else {
            return p - xz;
        };
        let rem = d * (1.0 - t);
        let rem_len = rem.length();
        if rem_len < 1e-6 {
            break; // stopped exactly at the face
        }

        let n2_raw = Vec2::new(hit.1.x, hit.1.z);
        let n2l = n2_raw.length();
        if n2l < 1e-4 {
            break; // no usable slide direction — hold this tick's position
        }
        let n2 = n2_raw / n2l;

        // Skip a plane we already have (same wall grazed again). A tiny angular
        // tolerance keeps numerical twins on a flat wall from filling the clip
        // set and confusing the crease logic below.
        let is_duplicate = normals[..n_count].iter().any(|prev| prev.dot(n2) > 0.98);
        if !is_duplicate && n_count < normals.len() {
            normals[n_count] = n2;
            n_count += 1;
        }

        // Clip `rem` against each plane: remove the component pointing into the
        // plane. Do it for every accumulated plane; if clipping against a later
        // plane re-introduces motion into an earlier one, clip to the CREASE
        // (slide along the shared edge).
        let rem_len2 = rem.length();
        let mut vel = rem;
        'planes: for i in 0..n_count {
            // Only clip if we're actually heading into this plane.
            if vel.dot(normals[i]) >= 0.0 {
                continue;
            }
            // Slide along this plane.
            let mut v = vel - normals[i] * vel.dot(normals[i]);
            // Does the new velocity dig into any OTHER plane?
            for j in 0..n_count {
                if j == i {
                    continue;
                }
                if v.dot(normals[j]) < 0.0 {
                    // Two planes at once — slide along their crease (the
                    // direction perpendicular to both). In 2D that's the
                    // perpendicular of one normal, oriented downstream.
                    let crease = Vec2::new(-normals[i].y, normals[i].x);
                    let crease = if crease.dot(rem) < 0.0 {
                        -crease
                    } else {
                        crease
                    };
                    // If the crease still points into plane j, it's a real
                    // dead-end pocket. Otherwise ride it.
                    if crease.dot(normals[j]) < -1e-3 {
                        vel = Vec2::ZERO;
                        break 'planes;
                    }
                    v = crease * rem_len2;
                }
            }
            vel = v;
        }

        // Stop only on a genuine reversal: the slide points back more than 90°
        // from where we wanted to go. A flat wall hit at any oblique angle stays
        // forward and keeps sliding; a true box-in reverses.
        if want_dir != Vec2::ZERO && vel.length() > 1e-6 {
            let vd = vel / vel.length();
            if vd.dot(want_dir) < -0.01 {
                break;
            }
        }
        d = vel;
    }

    p - xz
}

/// Ceiling hold (plan §2.4): before applying a rise from `feet_old` to
/// `feet_new`, reject it when any triangle (any face class) sits in the slab
/// `(feet_old + BODY_HEIGHT, feet_new + BODY_HEIGHT]` at the feet column —
/// the body top would push into geometry. A descent never trips this.
pub fn ceiling_holds(geom: &MzbCollisionGeometry, xz: Vec2, feet_old: f32, feet_new: f32) -> bool {
    if feet_new <= feet_old + 1e-6 {
        return false;
    }
    geom.any_tri_in_column_slab(xz, feet_old + BODY_HEIGHT, feet_new + BODY_HEIGHT)
}
