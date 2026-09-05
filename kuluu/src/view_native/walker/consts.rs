//! Walker constants, one place (plan §2.7). Everything else in the walker
//! derives from these; tuning knobs are marked.

/// Tallest rise accepted between two column samples and by the step band.
/// Single definition in `dat_mzb::MAX_GROUND_STEP_UP`, re-exported here.
pub const STEP_MAX: f32 = kuluu_render::dat_mzb::MAX_GROUND_STEP_UP;

/// 60 degree floor/wall rule: a face is FLOOR if its normal.y >= FLOOR_COS,
/// otherwise it's a WALL. Single definition in `dat_mzb::FLOOR_NORMAL_MIN`.
pub const FLOOR_COS: f32 = kuluu_render::dat_mzb::FLOOR_NORMAL_MIN;

/// Total height range under which the window carries no ramp at all (poof):
/// target is h0 direct, no envelope. Tuning knob (plan §6).
pub const POOF_MAX: f32 = 0.12;

/// A sample above both neighbors by less than this is a nosing/trim/sill, not
/// a riser: replaced by min(neighbors) before anything reads heights.
pub const LIP_MAX: f32 = 0.10;

/// Staircase vs single-step cutoff: >= 2 risers within two treads of this
/// width is a staircase; one riser in the window is a single step climbed at
/// speed when the footprint reaches it (plan §6).
pub const STAIR_TREAD_MAX: f32 = 0.4;

/// Sample spacing along the move direction.
pub const SAMPLE_SPACING: f32 = 0.15;

/// Forward reach of the sample window, derived from the tread cutoff (plan §0
/// Q5): two treads plus one spacing.
pub const LOOKAHEAD: f32 = 2.0 * STAIR_TREAD_MAX + SAMPLE_SPACING;

/// Backward reach behind the feet.
pub const LOOKBEHIND: f32 = 0.5;

/// Lateral sample offset from the move line (both sides). The second lateral
/// pair also sits this far ahead of the feet along m (plan §2.2).
pub const LATERAL_OFFSET: f32 = 0.3;

/// Footprint probe ring radius (inside the body radius): a hole wider than
/// the footprint means airborne, narrower is bridged.
pub const FOOT_RADIUS: f32 = 0.25;

/// Body sweep sphere radius. Two spheres of this radius cover feet+STEP_MAX to
/// feet+BODY_HEIGHT; nothing below feet + STEP_MAX is ever a horizontal
/// obstacle (plan §2.4).
pub const BODY_RADIUS: f32 = 0.4;

/// Body height for the ceiling hold: the top of the sweep coverage.
pub const BODY_HEIGHT: f32 = 1.8;

/// Lower body-sphere center, above the feet: STEP_MAX + BODY_RADIUS (the
/// sphere bottoms out exactly at the step band).
pub const LOWER_CENTER_OFFSET: f32 = STEP_MAX + BODY_RADIUS;

/// Upper body-sphere center, above the feet (plan §2.4: coverage 0.4 to 1.7).
pub const UPPER_CENTER_OFFSET: f32 = 1.3;

/// Per-tick slew limit on the envelope gradient components: a wobbly estimate
/// or a fast 180 can't spike g (plan §2.2, tuning knob per plan §6).
pub const GRAD_SLEW: f32 = 0.15;

/// f32 slack on chain ceilings and support-probe bounds. The column ray is
/// cast from a fixed high origin, so reported hit heights carry ~1e-4-yalm
/// noise at zone coordinate magnitudes — the same rationale as
/// `dat_mzb::STEP_UP_REACH_EPSILON`.
pub const CHAIN_CEILING_EPS: f32 = 5e-4;

// ---------------------------------------------------------------------------
// Sweep (plan §2.4)
// ---------------------------------------------------------------------------

/// Slide re-projection passes per tick (wall, then crease).
pub const SLIDE_ITERATIONS: usize = 3;

/// Penetration depth at which a face we are NOT moving into still blocks the
/// sweep. Two faces pinching a gap narrower than the body each read "parallel"
/// to the motion individually, but together they must stop us (corridor pass
/// 0.9 / block 0.7). Stuck-on-walls sliding only ever re-detects its wall at
/// ~1e-4 triangle-seam dip — far below this slop, so parallel slides pass.
pub const PINCH_PENETRATION_SLOP: f32 = 0.02;

/// Depenetration passes for a move that STARTS embedded (bad server seed, mid
/// zone-swap). Resting at sweep standoff (~R) must not fight the approach, so
/// only penetration past DEPEN_SLOP pushes out.
pub const DEPEN_ITERATIONS: usize = 3;
pub const DEPEN_SLOP: f32 = 0.02;

/// Cap on depenetration's total kick per tick — ~two in-game ticks of walking.
/// An uncapped push teleports along a riser face that passes through the body,
/// which reads as the "pop" and sideways drift on descent.
pub const DEPEN_MAX_PUSH: f32 = 0.15;

// ---------------------------------------------------------------------------
// Dynamic obstacles (plan §2.5)
// ---------------------------------------------------------------------------

/// Sustained pressure into the same mob before it stops blocking (PushThrough).
pub const PUSH_THROUGH_SECS: f32 = 0.8;

// ---------------------------------------------------------------------------
// Falling (plan §0 Q3)
// ---------------------------------------------------------------------------

/// Fall feel: fast and smooth, tuned by walking off ledges — swap for the real
/// constant if the XiClient source ever turns up one. A 1 yalm drop takes
/// ~0.22 s, 3 yalms ~0.39 s, 10 yalms ~0.6 s.
#[derive(Clone, Copy, Debug)]
pub struct FallModel {
    /// Downward acceleration, yalms/s^2.
    pub g: f32,
    /// Terminal fall speed, yalms/s (down).
    pub v_max: f32,
}

impl Default for FallModel {
    fn default() -> Self {
        Self {
            g: 40.0,
            v_max: 30.0,
        }
    }
}
