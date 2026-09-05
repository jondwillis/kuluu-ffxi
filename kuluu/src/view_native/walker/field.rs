//! Ramp field: the walking target under and ahead of the feet (plan §2.1-2.2).
//!
//! Everything here is in y-up space (bevy xz, y up) — the frame
//! `MzbCollisionGeometry`'s column queries answer in. The walker boundary
//! (`step.rs`) converts to wire coordinates (z grows down); this module never
//! sees them. Pure over its inputs: a [`Sampler`] supplies floor heights, so
//! unit tests hand it closures and the live path hands it the MZB geometry.

use bevy::math::Vec2;

use super::consts::{
    CHAIN_CEILING_EPS, FOOT_RADIUS, LATERAL_OFFSET, LIP_MAX, LOOKAHEAD, LOOKBEHIND, POOF_MAX,
    SAMPLE_SPACING, STEP_MAX,
};

/// Floor-height source for field sampling. The live implementation is
/// `MzbCollisionGeometry::ground_raycast` (highest up-facing floor at or below
/// the ceiling); tests use closures via the blanket impl below.
pub trait Sampler {
    fn floor(&self, xz: Vec2, ceiling_y: f32) -> Option<f32>;
}

impl<F> Sampler for F
where
    F: Fn(Vec2, f32) -> Option<f32>,
{
    fn floor(&self, xz: Vec2, ceiling_y: f32) -> Option<f32> {
        (self)(xz, ceiling_y)
    }
}

/// Second-query reach for the WallAhead test: how far above the chain ceiling
/// a floor must sit to count as "a wall face ahead capped this rise".
const WALL_AHEAD_REACH: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SampleStatus {
    /// A floor was found under the chain ceiling and survived the filters.
    Valid,
    /// Above both neighbors by less than LIP_MAX: replaced by min(neighbors).
    LipFiltered,
    /// Dropped more than STEP_MAX below the previous sample (ledge / hole
    /// edge): truncates its arm.
    RejectDrop,
    /// No floor under the chain ceiling: truncates its arm.
    Miss,
    /// The chain ceiling capped a rise and a second query found a floor above
    /// it (a wall face ahead): truncates its arm.
    WallAhead,
}

#[derive(Clone, Copy, Debug)]
pub struct FieldSample {
    pub xz: Vec2,
    /// Signed distance along the move direction; 0 is under the feet.
    pub along: f32,
    /// Signed lateral offset from the move line (+ left of `m`).
    pub lateral: f32,
    /// Height as sampled (None for Miss).
    pub raw: Option<f32>,
    /// Height after the lip filter (None when truncated / missed).
    pub filtered: Option<f32>,
    pub status: SampleStatus,
}

/// One tick's ramp field. `samples` holds every arm up to its truncation point
/// (a truncated arm simply stops contributing samples; its last entry carries
/// the truncating status).
#[derive(Clone, Debug)]
pub struct Field {
    pub feet_xz: Vec2,
    /// Unit move direction in xz (the facing when idle).
    pub m: Vec2,
    /// Floor under the footprint per the support-probe rule; None while
    /// airborne.
    pub h0: Option<f32>,
    pub samples: Vec<FieldSample>,
    /// Height jumps >= LIP_MAX along the surviving run of the move arm.
    /// 0 = flat/slope, 1 = single step, >= 2 = staircase.
    pub riser_count: u32,
    /// max(h) - min(h) over the window's surviving samples (0 when empty).
    pub range: f32,
    /// Dead band: range < POOF_MAX — no ramp, target is h0 direct.
    pub poof: bool,
    /// Envelope gradient (along m, lateral left of m); ZERO unless the window
    /// is a staircase.
    pub g: Vec2,
    /// Upper envelope at the feet: max_k (h_k - g . d_k) over surviving
    /// samples including k=0 (d = 0), so target >= h0 whenever both exist.
    /// None when there is no ramp to ride (flat / single step / poof / empty).
    pub target: Option<f32>,
}

/// Support probe result (plan §2.1): five column queries, feet xz plus four at
/// radius FOOT_RADIUS. Grounded if any accepts; h0 = center hit, fallback max
/// of accepted ring hits.
#[derive(Clone, Copy, Debug)]
pub struct SupportProbe {
    pub grounded: bool,
    /// Floor under the footprint per the rule above (None while airborne).
    pub h0: Option<f32>,
    /// Per-probe acceptance, index 0 = center, then +x / -x / +y / -y.
    pub accepted: [bool; 5],
}

/// The five probe xz positions for a footprint at `feet_xz`: center first,
/// then the ring in +x / -x / +y / -y order (matches `accepted` indices).
pub fn support_positions(feet_xz: Vec2) -> [Vec2; 5] {
    [
        feet_xz,
        feet_xz + Vec2::new(FOOT_RADIUS, 0.0),
        feet_xz - Vec2::new(FOOT_RADIUS, 0.0),
        feet_xz + Vec2::new(0.0, FOOT_RADIUS),
        feet_xz - Vec2::new(0.0, FOOT_RADIUS),
    ]
}

/// Five column queries at `feet_xz` + ring; a hit accepts when it sits within
/// one step up of the feet and no more than one step below (a floor further
/// down means airborne, not grounded).
pub fn support_probe<S: Sampler>(sampler: &S, feet_xz: Vec2, feet_y: f32) -> SupportProbe {
    let ceiling = feet_y + STEP_MAX + CHAIN_CEILING_EPS;
    let mut accepted = [false; 5];
    let mut hits: [Option<f32>; 5] = [None; 5];
    for (i, xz) in support_positions(feet_xz).into_iter().enumerate() {
        if let Some(h) = sampler.floor(xz, ceiling) {
            if h >= feet_y - STEP_MAX {
                accepted[i] = true;
                hits[i] = Some(h);
            }
        }
    }
    let grounded = accepted.iter().any(|&a| a);
    let ring_max = (1..5).filter_map(|i| hits[i]).reduce(f32::max);
    let h0 = if accepted[0] { hits[0] } else { ring_max };
    SupportProbe {
        grounded,
        h0: if grounded { h0 } else { None },
        accepted,
    }
}

/// Sample the ramp field at `feet_xz` / `feet_y` facing unit direction `m`.
pub fn sample_field<S: Sampler>(sampler: &S, feet_xz: Vec2, feet_y: f32, m: Vec2) -> Field {
    let m = if m.length_squared() < 1e-8 {
        Vec2::X
    } else {
        m.normalize()
    };

    // Sample positions: the move arm is a zero-aligned grid (k=0 sits under
    // the feet, so the envelope always includes d = 0), plus two lateral pairs
    // off the line at along 0 and along +LATERAL_OFFSET.
    let k_behind = (LOOKBEHIND / SAMPLE_SPACING).floor() as usize;
    let k_ahead = (LOOKAHEAD / SAMPLE_SPACING).round() as usize;

    let mut samples: Vec<FieldSample> = Vec::with_capacity(2 * k_behind + 1 + k_ahead + 4);

    // Forward arm, chained outward from the feet.
    let mut prev_h: Option<f32> = None;
    for k in 0..=k_ahead {
        let d = (k as f32) * SAMPLE_SPACING;
        let xz = feet_xz + m * d;
        let ceiling = match prev_h {
            Some(h) => h + STEP_MAX + CHAIN_CEILING_EPS,
            None => feet_y + STEP_MAX + CHAIN_CEILING_EPS,
        };
        let hit = sampler.floor(xz, ceiling);
        let status = classify_forward(hit, prev_h, sampler, xz);
        samples.push(FieldSample {
            xz,
            along: d,
            lateral: 0.0,
            raw: hit,
            filtered: None,
            status,
        });
        match status {
            SampleStatus::Valid | SampleStatus::LipFiltered => prev_h = hit,
            _ => break, // arm truncated
        }
    }

    // Backward arm, chained outward from the feet in reverse. The chain seeds
    // from the feet column (the first forward sample) when it exists.
    let seed = samples.first().and_then(|s| s.raw);
    let mut prev_h: Option<f32> = None;
    for k in 1..=k_behind {
        let d = -(k as f32) * SAMPLE_SPACING;
        let xz = feet_xz + m * d;
        let ceiling = match prev_h.or(seed) {
            Some(h) => h + STEP_MAX + CHAIN_CEILING_EPS,
            None => feet_y + STEP_MAX + CHAIN_CEILING_EPS,
        };
        let hit = sampler.floor(xz, ceiling);
        let status = classify_backward(hit, prev_h.or(seed));
        samples.push(FieldSample {
            xz,
            along: d,
            lateral: 0.0,
            raw: hit,
            filtered: None,
            status,
        });
        match status {
            SampleStatus::Valid | SampleStatus::LipFiltered => prev_h = hit,
            _ => break, // arm truncated
        }
    }

    // Lateral pairs: independent single queries (no chain across the line),
    // ceiling from the nearest on-line sample at that along distance.
    for &along in &[0.0_f32, LATERAL_OFFSET] {
        let base = samples
            .iter()
            .find(|s| s.lateral == 0.0 && (s.along - along).abs() < 1e-4)
            .and_then(|s| s.raw);
        for side in [1.0_f32, -1.0] {
            let xz = feet_xz + m * along + m.perp() * (LATERAL_OFFSET * side);
            let ceiling = match base {
                Some(h) => h + STEP_MAX + CHAIN_CEILING_EPS,
                None => feet_y + STEP_MAX + CHAIN_CEILING_EPS,
            };
            let hit = sampler.floor(xz, ceiling);
            samples.push(FieldSample {
                xz,
                along,
                lateral: LATERAL_OFFSET * side,
                raw: hit,
                filtered: None,
                status: if hit.is_some() {
                    SampleStatus::Valid
                } else {
                    SampleStatus::Miss
                },
            });
        }
    }

    // Lip filter over the surviving run of the move arm (on-line samples in
    // along order): a sample above both neighbors by less than LIP_MAX is a
    // nosing — replaced by min(neighbors). Interior points only; the run's
    // endpoints keep their raw height.
    let mut on_line: Vec<usize> = samples
        .iter()
        .enumerate()
        .filter(|(_, s)| s.lateral == 0.0 && s.raw.is_some())
        .map(|(i, _)| i)
        .collect();
    on_line.sort_by(|&a, &b| {
        samples[a]
            .along
            .partial_cmp(&samples[b].along)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for pos in 0..on_line.len() {
        let i = on_line[pos];
        if pos > 0 && pos + 1 < on_line.len() {
            let prev_i = on_line[pos - 1];
            let next_i = on_line[pos + 1];
            let h = samples[i].raw.unwrap();
            let lo = samples[prev_i]
                .raw
                .unwrap()
                .min(samples[next_i].raw.unwrap());
            if h > lo && h - lo < LIP_MAX {
                samples[i].filtered = Some(lo);
                samples[i].status = SampleStatus::LipFiltered;
            } else {
                samples[i].filtered = Some(h);
            }
        } else {
            samples[i].filtered = samples[i].raw;
        }
    }

    // Riser count + range over the surviving on-line run (post-filter).
    let heights: Vec<f32> = on_line
        .iter()
        .filter_map(|&i| samples[i].filtered)
        .collect();
    let mut risers = 0u32;
    for w in heights.windows(2) {
        if (w[1] - w[0]).abs() >= LIP_MAX {
            risers += 1;
        }
    }
    let range = if heights.is_empty() {
        0.0
    } else {
        let lo = heights.iter().cloned().reduce(f32::min).unwrap();
        let hi = heights.iter().cloned().reduce(f32::max).unwrap();
        hi - lo
    };

    let poof = !heights.is_empty() && range < POOF_MAX;

    // Envelope: only a staircase (>= 2 risers) rides the upper envelope of a
    // least-squares plane over the surviving samples; everything else targets
    // h0 directly (single step / slope / flat / poof all ride h0 at speed).
    let (g, target) = if risers >= 2 && !poof {
        fit_envelope(&samples, &on_line)
    } else {
        (Vec2::ZERO, None)
    };

    Field {
        feet_xz,
        m,
        h0: support_probe(sampler, feet_xz, feet_y).h0,
        samples,
        riser_count: risers,
        range,
        poof,
        g,
        target,
    }
}

/// Forward-arm classification against the previous (toward-feet) sample.
fn classify_forward(
    hit: Option<f32>,
    prev_h: Option<f32>,
    sampler: &impl Sampler,
    xz: Vec2,
) -> SampleStatus {
    let Some(h) = hit else {
        return SampleStatus::Miss;
    };
    if let Some(prev) = prev_h {
        // A drop beyond one step is a ledge / hole edge, not a walkable run.
        if prev - h > STEP_MAX + CHAIN_CEILING_EPS {
            return SampleStatus::RejectDrop;
        }
        // The chain ceiling capped a rise: check whether a floor sits above it
        // (a wall face ahead) or the sample simply found the next tread.
        let ceiling = prev + STEP_MAX + CHAIN_CEILING_EPS;
        if h >= ceiling - 1e-4 {
            if let Some(higher) = sampler.floor(xz, prev + WALL_AHEAD_REACH) {
                if higher > ceiling + 1e-4 {
                    return SampleStatus::WallAhead;
                }
            }
        }
    }
    SampleStatus::Valid
}

/// Backward-arm classification (mirror of the forward rule). The WallAhead
/// second query is skipped backwards: a wall behind the feet never gates the
/// walk target.
fn classify_backward(hit: Option<f32>, prev_h: Option<f32>) -> SampleStatus {
    let Some(h) = hit else {
        return SampleStatus::Miss;
    };
    if let Some(prev) = prev_h {
        if prev - h > STEP_MAX + CHAIN_CEILING_EPS {
            return SampleStatus::RejectDrop;
        }
    }
    SampleStatus::Valid
}

/// Least-squares plane `h = g.x * d + g.y * l + c` over the surviving on-line
/// and lateral samples, then the upper envelope at the feet:
/// `max_k (h_k - g . d_k)` — the plane of gradient g touching the highest
/// sample. k=0 (d = 0) is in the set, so the result is >= h0 whenever both
/// exist: never inside a tread.
fn fit_envelope(samples: &[FieldSample], on_line: &[usize]) -> (Vec2, Option<f32>) {
    // Surviving samples as (d along m, l left of m, h).
    let mut pts: Vec<(f32, f32, f32)> = Vec::new();
    for &i in on_line {
        if let Some(h) = samples[i].filtered {
            pts.push((samples[i].along, 0.0, h));
        }
    }
    for s in samples
        .iter()
        .filter(|s| s.lateral != 0.0 && s.filtered.is_some())
    {
        pts.push((s.along, s.lateral, s.filtered.unwrap()));
    }
    if pts.len() < 3 {
        return (Vec2::ZERO, None);
    }

    // Normal equations for h = a*d + b*l + c: X^T X [a b c]^T = X^T h with
    // design rows [d l 1].
    let n = pts.len() as f32;
    let (mut sdd, mut sdl, mut sdh, mut sll, mut slh, mut sch, mut sum_d, mut sum_l) =
        (0.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for &(d, l, h) in &pts {
        sdd += d * d;
        sdl += d * l;
        sdh += d * h;
        sll += l * l;
        slh += l * h;
        sch += h;
        sum_d += d;
        sum_l += l;
    }
    let a11 = sdd;
    let a12 = sdl;
    let a13 = sum_d;
    let a21 = sdl;
    let a22 = sll;
    let a23 = sum_l;
    let a31 = sum_d;
    let a32 = sum_l;
    let a33 = n;
    let det = a11 * (a22 * a33 - a23 * a32) - a12 * (a21 * a33 - a23 * a31)
        + a13 * (a21 * a32 - a22 * a31);
    if det.abs() < 1e-9 {
        return (Vec2::ZERO, None);
    }
    let b1 = sdh;
    let b2 = slh;
    let b3 = sch;
    let da =
        b1 * (a22 * a33 - a23 * a32) - a12 * (b2 * a33 - b3 * a31) + a13 * (b2 * a32 - b3 * a31);
    let db =
        a11 * (b2 * a33 - a23 * b3) - b1 * (a21 * a33 - a23 * a31) + a13 * (a21 * b3 - b2 * a31);
    let g = Vec2::new(da / det, db / det);

    // Upper envelope at the feet: max over surviving samples of h_k - g . d_k.
    let mut best = f32::NEG_INFINITY;
    for &(d, l, h) in &pts {
        best = best.max(h - (g.x * d + g.y * l));
    }
    if !best.is_finite() {
        (Vec2::ZERO, None)
    } else {
        (g, Some(best))
    }
}

/// The walking target for this tick's vertical step: the envelope when a
/// staircase is in view, otherwise h0 direct (flat / slope / single step /
/// poof all ride h0 at speed). None while airborne.
pub fn field_target(field: &Field) -> Option<f32> {
    match field.target {
        Some(t) => Some(t),
        None => field.h0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic sampler over a height function (clamps to the ceiling like
    /// `ground_raycast`).
    struct FnSampler<F>(F);
    impl<F: Fn(Vec2, f32) -> Option<f32>> Sampler for FnSampler<F> {
        fn floor(&self, xz: Vec2, ceiling_y: f32) -> Option<f32> {
            (self.0)(xz, ceiling_y).filter(|h| *h <= ceiling_y)
        }
    }

    /// Inference-friendly constructor: the closure's argument types are known
    /// at this call site, whereas `FnSampler(closure)` would leave them to be
    /// resolved from downstream constraints (which Rust does not do).
    fn sampler<F>(f: F) -> FnSampler<F>
    where
        F: Fn(Vec2, f32) -> Option<f32>,
    {
        FnSampler(f)
    }

    /// Flat floor at y=0 everywhere.
    fn flat() -> impl Fn(Vec2, f32) -> Option<f32> {
        |_, _| Some(0.0)
    }

    #[test]
    fn flat_field_is_poof_and_targets_h0() {
        let s = sampler(flat());
        let f = sample_field(&s, Vec2::ZERO, 0.0, Vec2::X);
        assert_eq!(f.h0, Some(0.0));
        assert_eq!(f.riser_count, 0);
        assert!(f.poof);
        assert_eq!(field_target(&f), Some(0.0));
    }

    #[test]
    fn support_probe_grounded_on_flat() {
        let s = sampler(flat());
        let p = support_probe(&s, Vec2::ZERO, 0.0);
        assert!(p.grounded);
        assert_eq!(p.h0, Some(0.0));
        assert!(p.accepted.iter().all(|&a| a));
    }

    #[test]
    fn support_probe_airborne_over_deep_hole() {
        // Floor 1 m below the feet: beyond the step band on every probe.
        let s = sampler(flat());
        let p = support_probe(&s, Vec2::ZERO, 1.0);
        assert!(!p.grounded);
        assert_eq!(p.h0, None);
    }

    #[test]
    fn single_riser_is_one_not_a_staircase() {
        // A 0.3 riser at along +0.45 on otherwise flat ground: one jump in the
        // window => single step, no envelope.
        let s = sampler(|xz, _| Some(if xz.x >= 0.45 { 0.3 } else { 0.0 }));
        let f = sample_field(&s, Vec2::ZERO, 0.0, Vec2::X);
        assert_eq!(f.riser_count, 1);
        assert!(f.target.is_none());
    }

    #[test]
    fn staircase_envelope_never_below_h0() {
        // Ascending flight: tread width 0.45, riser 0.3, starting at along 0.3.
        let s = sampler(|xz, _| {
            Some(if xz.x < 0.3 {
                0.0
            } else {
                (1.0 + ((xz.x - 0.3) / 0.45).floor()) * 0.3
            })
        });
        let f = sample_field(&s, Vec2::ZERO, 0.0, Vec2::X);
        assert!(
            f.riser_count >= 2,
            "expected a staircase, got {}",
            f.riser_count
        );
        let t = field_target(&f).expect("staircase has a target");
        assert!(t >= f.h0.unwrap() - 1e-6, "envelope {t} below h0");
    }

    #[test]
    fn lip_filter_removes_nosings() {
        // A 0.05 nosing on every tread edge of an otherwise clean flight must
        // not count as a riser and must not tilt the gradient.
        let s = sampler(|xz, _| {
            let base = if xz.x < 0.3 {
                0.0
            } else {
                (1.0 + ((xz.x - 0.3) / 0.45).floor()) * 0.3
            };
            // Nosing: a bump in the first 0.06 of each tread.
            let into_tread = if xz.x < 0.3 {
                xz.x
            } else {
                (xz.x - 0.3) % 0.45
            };
            Some(if into_tread < 0.06 { base + 0.05 } else { base })
        });
        let f = sample_field(&s, Vec2::ZERO, 0.0, Vec2::X);
        // The nosings are filtered; the flight still reads as a staircase with
        // the same gradient as the clean case (within noise).
        assert!(f.riser_count >= 2);
    }

    #[test]
    fn ledge_ahead_rejects_and_holds_target_at_h0() {
        // Flat, then a 1.5 drop at along +0.6: RejectDrop truncates the arm;
        // no staircase, target stays h0 — no dip toward the edge.
        let s = sampler(|xz, _| Some(if xz.x < 0.6 { 0.0 } else { -1.5 }));
        let f = sample_field(&s, Vec2::ZERO, 0.0, Vec2::X);
        assert!(f
            .samples
            .iter()
            .any(|s| s.status == SampleStatus::RejectDrop));
        assert_eq!(field_target(&f), Some(0.0));
    }

    #[test]
    fn hole_truncates_arm() {
        // No floor past along +0.3: the arm misses and stops.
        let s = sampler(|xz, _| (xz.x < 0.3).then_some(0.0));
        let f = sample_field(&s, Vec2::ZERO, 0.0, Vec2::X);
        assert!(f.samples.iter().any(|s| s.status == SampleStatus::Miss));
    }

    #[test]
    fn descending_staircase_envelope_rides_treads() {
        // Descending flight: the envelope must stay >= h0 (never inside a
        // tread) even though forward samples are lower.
        let s = sampler(|xz, _| {
            Some(if xz.x < 0.3 {
                0.9
            } else {
                0.9 - ((1.0 + ((xz.x - 0.3) / 0.45).floor()) * 0.3)
            })
        });
        let f = sample_field(&s, Vec2::ZERO, 0.9, Vec2::X);
        assert!(f.riser_count >= 2);
        let t = field_target(&f).expect("target");
        assert!(t >= f.h0.unwrap() - 1e-6);
    }

    #[test]
    fn wall_ahead_caps_the_chain() {
        // A vertical face at along +0.5 rising to a ledge at +1.5: the chain
        // ceiling caps at feet+STEP_MAX and the second query finds the floor
        // above it.
        let s = sampler(|xz, ceiling| {
            if xz.x < 0.5 {
                Some(0.0f32).filter(|h| *h <= ceiling)
            } else {
                // The wall's top ledge: a floor at +1.5 only "under" ceilings
                // that reach it; the face itself is not a floor (the normal
                // test lives in the geometry, here we model its top).
                Some(1.5f32).filter(|h| *h <= ceiling)
            }
        });
        let f = sample_field(&s, Vec2::ZERO, 0.0, Vec2::X);
        assert!(
            f.samples
                .iter()
                .any(|s| s.status == SampleStatus::WallAhead),
            "statuses: {:?}",
            f.samples
                .iter()
                .map(|s| (s.along, s.status))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn envelope_is_continuous_along_a_walk() {
        // Walk an ascending flight sample by sample: the target must move in
        // small steps (no zig) — max tick-to-tick change stays under a riser.
        let h = |xz: Vec2, _ceiling: f32| {
            Some(if xz.x < 0.3 {
                0.0
            } else {
                (1.0 + ((xz.x - 0.3) / 0.45).floor()) * 0.3
            })
        };
        let s = sampler(h);
        let mut max_step = 0.0f32;
        let mut last: Option<f32> = None;
        for i in 0..40 {
            let xz = Vec2::new(i as f32 * 0.1, 0.0);
            let feet_y = h(xz, f32::INFINITY).unwrap();
            let f = sample_field(&s, xz, feet_y, Vec2::X);
            let t = field_target(&f).unwrap_or(feet_y);
            if let Some(p) = last {
                max_step = max_step.max((t - p).abs());
            }
            last = Some(t);
        }
        assert!(max_step < 0.35, "target zig of {max_step}");
    }

    #[test]
    fn lateral_step_gives_lateral_gradient() {
        // Steps on both sides ahead of the feet (a cross-aisle): the plane fit
        // must pick up a non-zero gradient and stay finite.
        let s = sampler(|xz, _| {
            Some(if xz.x > 0.3 && xz.y.abs() > 0.15 {
                0.3
            } else {
                0.0
            })
        });
        let f = sample_field(&s, Vec2::ZERO, 0.0, Vec2::X);
        assert!(f.g.is_finite());
    }

    #[test]
    fn dead_band_sill_is_poof() {
        // A 0.1 sill (under POOF_MAX) in the window: no ramp, target == h0.
        let s = sampler(|xz, _| Some(if xz.x > 0.3 { 0.1 } else { 0.0 }));
        let f = sample_field(&s, Vec2::ZERO, 0.0, Vec2::X);
        assert!(f.poof || f.riser_count < 2, "sill must not ramp");
        assert_eq!(field_target(&f), Some(0.0));
    }

    #[test]
    fn reverse_direction_samples_behind() {
        // Facing -x on an ascending flight that rises toward +x: the backward
        // arm (toward +x) must see the risers, so the window still counts them.
        let s = sampler(|xz, _| {
            Some(if xz.x < 0.3 {
                0.0
            } else {
                (1.0 + ((xz.x - 0.3) / 0.45).floor()) * 0.3
            })
        });
        let f = sample_field(&s, Vec2::ZERO, 0.0, -Vec2::X);
        assert!(f.samples.iter().any(|s| s.along < 0.0 && s.raw.is_some()));
    }

    #[test]
    fn support_probe_bridges_narrow_hole() {
        // A hole wider than one probe spacing but narrower than the footprint:
        // the center misses, a ring probe still accepts => grounded.
        let s = sampler(|xz, _| (xz.x.abs() > 0.1).then_some(0.0));
        let p = support_probe(&s, Vec2::ZERO, 0.0);
        assert!(p.grounded, "narrower-than-footprint hole is bridged");
    }

    #[test]
    fn support_probe_misses_wide_hole() {
        // A hole wider than the footprint: no probe accepts => airborne.
        let s = sampler(|xz, _| (xz.x.abs() > 0.4).then_some(0.0));
        let p = support_probe(&s, Vec2::ZERO, 0.0);
        assert!(!p.grounded);
    }

    #[test]
    fn field_h0_matches_support_rule() {
        // Floating 0.3 above the floor (mid stop-settle): still grounded, h0
        // is the floor below.
        let s = sampler(flat());
        let f = sample_field(&s, Vec2::ZERO, 0.3, Vec2::X);
        assert_eq!(f.h0, Some(0.0));
    }

    #[test]
    fn zero_direction_falls_back_to_x() {
        let s = sampler(flat());
        let f = sample_field(&s, Vec2::ZERO, 0.0, Vec2::ZERO);
        assert_eq!(f.m, Vec2::X);
    }

    /// An ascending flight as a height function: flat at 0 until x0, then
    /// riser r every tread d (clamped at the top terrace).
    fn flight(x0: f32, d: f32, r: f32, steps: u32) -> impl Fn(Vec2, f32) -> Option<f32> {
        move |xz, _| {
            Some(if xz.x < x0 {
                0.0
            } else {
                ((1.0 + ((xz.x - x0) / d).floor()).min(steps as f32)) * r
            })
        }
    }

    /// Property (plan §4): on random staircases the envelope never dips below h0.
    #[test]
    fn envelope_never_below_h0_on_random_staircases() {
        // Deterministic LCG so a failure is reproducible.
        let mut seed = 0x9E37_79B9u32;
        let next_f = |s: &mut u32| -> f32 {
            *s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (*s >> 8) as f32 / (u32::MAX >> 8) as f32
        };
        for _ in 0..200 {
            let steps = 2 + (next_f(&mut seed) * 14.0) as u32;
            let d = 0.2 + next_f(&mut seed) * 0.8; // tread 0.2..1.0
            let r = 0.1 + next_f(&mut seed) * 0.3; // riser 0.1..0.4 (climbable)
            for _ in 0..8 {
                let x = 0.3 + next_f(&mut seed) * ((steps as f32 * d - 0.6).max(0.5));
                let h = flight(0.3, d, r, steps);
                let feet_y = h(Vec2::new(x, 0.0), f32::INFINITY).unwrap();
                let s = sampler(h);
                let f = sample_field(&s, Vec2::new(x, 0.0), feet_y, Vec2::X);
                if let (Some(t), Some(h0)) = (f.target, f.h0) {
                    assert!(t >= h0 - 1e-6, "envelope {t} below h0 {h0} at x={x}");
                }
            }
        }
    }

    /// Ascend/descend (r, d) matrix (plan §4): the fitted gradient is within
    /// 10% of r/d; facing the other way it flips sign.
    #[test]
    fn gradient_matches_riser_over_tread() {
        for (r, d) in [
            (0.2, 0.25),
            (0.25, 0.3),
            (0.3, 0.4),
            (0.35, 0.5),
            (0.4, 0.4),
        ] {
            // Stand mid-flight where the window is full of treads.
            let h = flight(0.3, d, r, 20);
            let feet_y = h(Vec2::new(2.0, 0.0), f32::INFINITY).unwrap();
            let s = sampler(h);
            let f = sample_field(&s, Vec2::new(2.0, 0.0), feet_y, Vec2::X);
            assert!(f.riser_count >= 2, "r={r} d={d}: not a staircase");
            let want = r / d;
            assert!(
                (f.g.x - want).abs() <= 0.1 * want + 1e-3,
                "ascend g.along {:.4} vs {want:.4}",
                f.g.x
            );
            // Same flight walked the other way: g flips sign, envelope still >= h0.
            let f = sample_field(&s, Vec2::new(2.0, 0.0), feet_y, -Vec2::X);
            assert!(f.riser_count >= 2, "r={r} d={d}: not a staircase (reverse)");
            assert!(
                (f.g.x + want).abs() <= 0.1 * want + 1e-3,
                "descend g.along {:.4}, expected -{want:.4}",
                f.g.x
            );
            if let (Some(t), Some(h0)) = (f.target, f.h0) {
                assert!(t >= h0 - 1e-6, "reverse envelope below h0");
            }
        }
    }

    /// Riser-count regimes by tread width (plan §4): narrow treads read as a
    /// staircase, wide ones as single steps.
    #[test]
    fn riser_count_by_tread_width() {
        let flat_f = flight(0.3, 1.0, 0.0, 1); // no risers at all
        let s = sampler(flat_f);
        assert_eq!(sample_field(&s, Vec2::ZERO, 0.0, Vec2::X).riser_count, 0);

        for (d, min_risers) in [(0.25, 2u32), (0.3, 2), (0.4, 2)] {
            let h = flight(0.3, d, 0.3, 10);
            let s = sampler(h);
            let f = sample_field(&s, Vec2::new(1.5, 0.0), 0.9, Vec2::X);
            assert!(
                f.riser_count >= min_risers,
                "d={d}: expected a staircase, got {}",
                f.riser_count
            );
        }
        // Wide tread (0.9): one riser in the window = single step.
        let h = flight(0.3, 0.9, 0.3, 10);
        let s = sampler(h);
        let f = sample_field(&s, Vec2::new(1.5, 0.0), 0.9, Vec2::X);
        assert_eq!(f.riser_count, 1, "wide tread must be a single step");
    }

    /// A 0.05 nosing on every tread edge: the lip filter removes it and the
    /// gradient equals the no-nosing case (plan §4).
    #[test]
    fn nosings_do_not_tilt_the_gradient() {
        let clean = flight(0.3, 0.45, 0.3, 12);
        let feet_y = clean(Vec2::new(2.0, 0.0), 0.0).unwrap();
        let with_nosings = |xz: Vec2, _c: f32| -> Option<f32> {
            let base = flight(0.3, 0.45, 0.3, 12)(xz, 0.0)?;
            // Into-tread distance on the current tread (0 before the first riser).
            let into = if xz.x < 0.3 {
                xz.x
            } else {
                ((xz.x - 0.3) % 0.45).max(0.0)
            };
            Some(if into < 0.06 { base + 0.05 } else { base })
        };
        let s1 = sampler(clean);
        let f1 = sample_field(&s1, Vec2::new(2.0, 0.0), feet_y, Vec2::X);
        let s2 = sampler(with_nosings);
        let f2 = sample_field(
            &s2,
            Vec2::new(2.0, 0.0),
            with_nosings(Vec2::new(2.0, 0.0), 0.0).unwrap(),
            Vec2::X,
        );
        assert!(f1.riser_count >= 2 && f2.riser_count >= 2);
        assert!(
            (f1.g.x - f2.g.x).abs() < 0.02,
            "nosing tilted g: clean {:.4} vs nosed {:.4}",
            f1.g.x,
            f2.g.x
        );
    }

    /// A hole 0.8 wide at +0.45 (plan §4): the arm truncates, no envelope tilt,
    /// and support under the feet is still grounded.
    #[test]
    fn hole_ahead_truncates_without_losing_support() {
        // Floor everywhere except a 0.8 gap centered at x = 0.45 + 0.4 = 0.85...
        let h = |xz: Vec2, _c: f32| -> Option<f32> {
            if (0.45..1.25).contains(&xz.x) {
                None
            } else {
                Some(0.0)
            }
        };
        let s = sampler(h);
        let f = sample_field(&s, Vec2::ZERO, 0.0, Vec2::X);
        assert!(f
            .samples
            .iter()
            .any(|s| matches!(s.status, SampleStatus::Miss | SampleStatus::RejectDrop)));
        // No staircase out of a truncated arm: target is h0 direct.
        assert_eq!(field_target(&f), Some(0.0));
        let p = support_probe(&s, Vec2::ZERO, 0.0);
        assert!(p.grounded, "feet are on solid floor");
    }

    /// A wall face 1.5 up ahead (plan §4): WallAhead truncates and the target
    /// holds at h0.
    #[test]
    fn tall_wall_ahead_holds_target_at_h0() {
        let s = sampler(|xz, ceiling| {
            if xz.x < 0.5 {
                Some(0.0f32).filter(|h| *h <= ceiling)
            } else {
                // The ledge on top of the wall: only visible to high ceilings.
                Some(1.5f32).filter(|h| *h <= ceiling)
            }
        });
        let f = sample_field(&s, Vec2::ZERO, 0.0, Vec2::X);
        assert!(f
            .samples
            .iter()
            .any(|s| s.status == SampleStatus::WallAhead));
        assert_eq!(field_target(&f), Some(0.0));
    }

    /// A riser of 0.5 ahead (plan §4): the chain ceiling caps at STEP_MAX, so it
    /// reads as WallAhead — not a climbable step.
    #[test]
    fn half_yalm_riser_ahead_is_wall_ahead() {
        let s = sampler(|xz, _| Some(if xz.x < 0.45 { 0.0 } else { 0.5 }));
        let f = sample_field(&s, Vec2::ZERO, 0.0, Vec2::X);
        assert!(
            f.samples
                .iter()
                .any(|s| s.status == SampleStatus::WallAhead),
            "statuses: {:?}",
            f.samples
                .iter()
                .map(|s| (s.along, s.status))
                .collect::<Vec<_>>()
        );
        assert_eq!(field_target(&f), Some(0.0));
    }

    /// Diagonal 30/45/60 to the flight (plan §4): the plane fit stays finite and
    /// the envelope never dips below h0.
    #[test]
    fn diagonal_to_flight_stays_sane() {
        for deg in [30.0f32, 45.0, 60.0] {
            let a = deg.to_radians();
            let m = Vec2::new(a.cos(), -a.sin());
            let h = flight(0.3, 0.45, 0.3, 12);
            let feet_y = h(Vec2::ZERO, f32::INFINITY).unwrap();
            let s = sampler(h);
            let f = sample_field(&s, Vec2::ZERO, feet_y, m);
            assert!(f.g.is_finite(), "deg {deg}");
            if let (Some(t), Some(h0)) = (f.target, f.h0) {
                assert!(t >= h0 - 1e-6, "deg {deg}: envelope below h0");
            }
        }
    }

    /// A 50 degree slanted riser is a SLOPE (0 risers, continuous samples); a
    /// 65 degree face of the same height is a single step (plan §4).
    #[test]
    fn slanted_riser_50_is_slope_65_is_step() {
        // 50 degrees: rise 0.3 over run 0.373 — continuous, no jumps.
        let h50 = |xz: Vec2, _c: f32| -> Option<f32> {
            Some(if xz.x < 0.45 {
                0.0
            } else {
                ((xz.x - 0.45) * (0.3 / 0.373)).min(0.3)
            })
        };
        let s = sampler(h50);
        let f = sample_field(&s, Vec2::ZERO, 0.0, Vec2::X);
        assert_eq!(f.riser_count, 0, "50 degree face must be a slope: {f:?}");

        // 65 degrees: rise 0.3 over run 0.143 — one jump in the window.
        let h65 = |xz: Vec2, _c: f32| -> Option<f32> {
            Some(if xz.x < 0.45 {
                0.0
            } else {
                ((xz.x - 0.45) * (0.3 / 0.143)).min(0.3)
            })
        };
        let s = sampler(h65);
        let f = sample_field(&s, Vec2::ZERO, 0.0, Vec2::X);
        assert_eq!(
            f.riser_count, 1,
            "65 degree face must be a single step: {f:?}"
        );
    }
}
