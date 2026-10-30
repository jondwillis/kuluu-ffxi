//! Walker constants, one place. Everything else in the walker
//! derives from these; tuning knobs are marked.

/// Tallest rise accepted between two column samples and by the step band.
/// Single definition in `dat_mzb::MAX_GROUND_STEP_UP`, re-exported here.
pub const STEP_MAX: f32 = kuluu_render::dat_mzb::MAX_GROUND_STEP_UP;

/// Retail's 45 degree floor/wall rule: a face is FLOOR if its normal.y >=
/// FLOOR_COS, otherwise it's a WALL. Single definition in
/// `dat_mzb::FLOOR_NORMAL_MIN`.
pub const FLOOR_COS: f32 = kuluu_render::dat_mzb::FLOOR_NORMAL_MIN;

/// Body sweep sphere radius. A chain of spheres this wide stands from the feet
/// to BODY_HEIGHT; faces inside the step band are excluded by the riser rule
/// in `dat_mzb::nearest_wall_contact`, not by a coverage gap — a gap under a
/// tall face lets the body walk into wedges and fall out beneath them.
pub const BODY_RADIUS: f32 = 0.4;

/// Body height for the ceiling hold: the top of the sweep coverage.
pub const BODY_HEIGHT: f32 = 1.8;

/// Lowest body-sphere center, above the feet: BODY_RADIUS (the chain bottoms
/// out at the feet themselves).
pub const LOWER_CENTER_OFFSET: f32 = BODY_RADIUS;

/// Middle body-sphere center: half the body height, overlapping both neighbours.
pub const MIDDLE_CENTER_OFFSET: f32 = 0.5 * BODY_HEIGHT;

/// Upper body-sphere center: BODY_HEIGHT - BODY_RADIUS (the chain tops out at
/// BODY_HEIGHT exactly).
pub const UPPER_CENTER_OFFSET: f32 = BODY_HEIGHT - BODY_RADIUS;

/// Sweep: slide re-projection passes per tick (wall, then crease).
pub const SLIDE_ITERATIONS: usize = 3;

/// Minimum approach cosine for a contact to count as moving INTO the face in
/// `sweep::body_sweep`. Kuluu-authored numerical guard, not a retail number: the sweep's own
/// invariant is that only an opposing face stops it - "grazes and partings pass through"
/// (sweep.rs). After one clip plane the tangential velocity carries dot(noise) ~1e-9 against
/// the same face; at exactly-parallel travel a wall whose authored XZ-normal is not perpendicular
/// to travel direction (every non-axis-aligned wall: the shipped Bastok bastion faces
/// n=(-0.945,+0.319,-0.072) lean 0.076 of the normal along the wall's run - DAT 334 tri 17851,
/// `kuluu-render/tests/mzb_bastok_wall_slide_dump.rs`) decides catch-vs-slide by float sign: a
/// resting pose touching a face at standoff sits inside r_eff, so an unscaled approach test
/// fires on noise alone, the bisection collapses to t~0 every tick, and the body creeps along
/// at ~1e-5/tick ("caught on walls, should be sliding", 2026-10-27 report). Below this approach
/// fraction a contact passes; real shallow approaches still stop via the pinch term, whose
/// penetration slop accumulates as the body closes. One part in a thousand of head-on:
/// shallower than ~0.06 degrees, i.e., grazing only.
pub const INTO_FACE_APPROACH_SLOP: f32 = 1e-3;

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

// Actor contact, from research/XIClient/src/XIClient/source/World/Actor/ControllableActor.cpp
// ControllableActor::CheckContactActor.

/// Dynamic obstacles: search radius for the contact candidate: retail seeds
/// its nearest-actor scan with a squared distance of 64, so only actors
/// within 8 yalms of the projected position are ever candidates.
pub const CONTACT_SEARCH_RADIUS: f32 = 8.0;

/// Retail's contact block budget, decremented each tick by
/// research/XIClient/src/XIClient/source/Game/GameManager.cpp
/// GameManager::CheckTick, which returns `60 / EffectiveFramerate` -- so the
/// unit is a sixtieth of a second and the window holds at ~0.5 s on any
/// framerate.
pub const CONTACT_BLOCK_TICKS: f32 = 30.0;

/// The tick unit `CONTACT_BLOCK_TICKS` counts in, imported from the movement
/// side rather than re-typed.
pub const CONTACT_TICKS_PER_SEC: f32 = crate::view_native::input::RETAIL_MOVE_TICKS_PER_SEC;

/// Falling: retail's law, from the mover contact probe -
/// `.agents/skills/retail-observe/references/2026-10-25-collision-query.md`,
/// spec section "Terrain height and gravity, per frame (contact-probe pass)".
/// Its contact test answers zero when it touches nothing, and only on that
/// answer does the client accumulate a fixed vertical step into the frame's
/// answer vector: one world-unit constant times one lean/posture float, with no
/// acceleration ramp and no terminal speed inside that unit. On contact nothing
/// is accumulated at all - height is held, never snapped.
///
/// Sign convention: one axis only. The probe body accumulates a NEGATIVE
/// vertical penalty (minus 1.3 / minus 0.5) times one POSITIVE lean float to
/// descend, so its answer vector is up-positive - the same sign this walker
/// uses internally (`feet_y = -z_wire` in `step`), where the value is added
/// straight onto feet height. The wire axis (z grows down) exists only at the
/// module boundary, so nothing here carries a second negation: the constants
/// read as-is and their product is what moves Y.
///
/// The fall step's magnitude per movement frame at neutral posture: 1.3 world
/// units, carried with the client's sign (negative = down) so the product can
/// be added straight onto feet height - client constant `FALL_STEP_FRAME_PROBE`.
/// Derived from the canonical positive bound in `dat_mzb::FOLLOW_WORD_YAMLS_PER_FRAME`,
/// which also bands the wall sweep's riser rule, keeping fall step and climb reach one
/// recorded magnitude (collision record "Step up" and "Fall and landing" sections).
pub const FALL_STEP_FRAME_PROBE: f32 = -kuluu_render::dat_mzb::FOLLOW_WORD_YAMLS_PER_FRAME;

/// The alternate branch's step magnitude per movement frame (a different gate
/// pair plus its own entity predicate): 0.5 world units, same sign convention
/// as `FALL_STEP_FRAME_PROBE` - client constant `FALL_STEP_FRAME_ALT`.
pub const FALL_STEP_FRAME_ALT: f32 = -0.5;

/// The lean/posture float that scales the fall step, at neutral posture. The
/// record describes one positive float reset to one at mover mode transitions,
/// so an actor with no posture measurement yet multiplies the penalty by
/// exactly one (`LEAN_DEFAULT_NEUTRAL`); together with `FALL_STEP_FRAME_PROBE`
/// that is 1.3 world units down on every frame of a drop, ramp-free.
pub const LEAN_DEFAULT_NEUTRAL: f32 = 1.0;

/// The margin wrapper's constant: the record states it as 0.8 times the length
/// of the answer block's measured pair.
pub const PROBE_MARGIN_SCALE: f32 = 0.8;

/// Scene-driver scale on the published measurement word, applied before
/// integration adds it into height. The driver unit re-reads the one global
/// measurement float `[0x10456da8]`, multiplies it by dword `[0x10329a34]` -
/// decoded from the unpacked 2026-10-05 image as exactly +0.8, the same dword
/// behind [`PROBE_MARGIN_SCALE`] - and stores it back (`f_100c549c..0x100c54a8`
/// in `research/xi-on-anything/generated/all`), gated on a mover-state getter
/// answering 5 or 6 (calls @`0x100c548b/0x100c5492`, compares @`0x100c548b..0x100c549a`);
/// whether the gate covers other mover states is unresolved, so kuluu applies
/// this scale to the no-contact (fall) feed only - on contact the record states
/// no penalty word enters that channel at all (collision record
/// `.agents/skills/retail-observe/references/2026-10-25-collision-query.md`, "Result vector
/// (consumer pass)", rescored 2026-10-26: kuluu formerly integrated unscaled).
pub const MEASUREMENT_DRIVER_SCALE: f32 = PROBE_MARGIN_SCALE;

/// The fall step as kuluu integrates it - the probe constant after the scene
/// driver's scale. This is what height moves per airborne frame and exactly what
/// every vertical branch publishes, keeping measurement parity by construction.
pub const FALL_STEP_FRAME_INTEGRATED: f32 = FALL_STEP_FRAME_PROBE * MEASUREMENT_DRIVER_SCALE;

// Gait scale, from the movement-time gait helper - record
// `.agents/skills/retail-observe/references/2026-10-25-walker-speed-and-step.md` spec section
// "Gait rate selection": the two axis values are normalized against their own
// length and then rescaled in place by one of three constants chosen from that
// pre-normalization length, and the same scale is what a mid-range intent
// crosses ground with.

/// The intent length above which the gait scale is full (`0.9`).
pub const GAIT_FULL_CUTOFF: f32 = 0.9;

/// The intent length below which the pair reads as no intent at all and the
/// scale is zero (`1/20` of a unit).
pub const GAIT_DEAD_ZONE: f32 = 1.0 / 20.0;

/// A per-actor gait override raises a scale no larger than this to `GAIT_WALK`
/// (one third - the value the helper compares against, not a scale of its own).
pub const GAIT_OVERRIDE_RAISE_LIMIT: f32 = 1.0 / 3.0;

/// The full-press gait scale.
pub const GAIT_RUN: f32 = 1.0;

/// The mid-range gait scale (two thirds), also what an override raises to.
pub const GAIT_WALK: f32 = 2.0 / 3.0;

/// A pair no longer than this reads as zero and is multiplied out rather than
/// normalized (about one millionth of a unit).
pub const INTENT_ZERO_LENGTH: f32 = 1e-6;

/// The gait scale for an intent `magnitude`: full above [`GAIT_FULL_CUTOFF`],
/// [`GAIT_WALK`] down to [`GAIT_DEAD_ZONE`], zero below it. A per-actor override
/// (`gait_override`) lifts a zero-scale pair back up to [`GAIT_WALK`] - that is
/// how a deliberate walk with no axis input still moves and animates.
pub fn gait_scale(magnitude: f32, gait_override: bool) -> f32 {
    let scale = if magnitude > GAIT_FULL_CUTOFF {
        GAIT_RUN
    } else if magnitude >= GAIT_DEAD_ZONE {
        GAIT_WALK
    } else {
        0.0
    };
    if gait_override && scale <= GAIT_OVERRIDE_RAISE_LIMIT {
        GAIT_WALK
    } else {
        scale
    }
}

/// The helper's whole in-place edit, returned by value: normalize the axis pair
/// against its own length and rescale both components by [`gait_scale`] of that
/// same pre-normalization length. A pair under [`INTENT_ZERO_LENGTH`] is
/// multiplied out to zero rather than divided; the scale still comes back for
/// the caller that feeds animation.
pub fn scaled_move_axes(forward: f32, lateral: f32, gait_override: bool) -> (f32, f32, f32) {
    let magnitude = (forward * forward + lateral * lateral).sqrt();
    let scale = gait_scale(magnitude, gait_override);
    if magnitude <= INTENT_ZERO_LENGTH || scale == 0.0 {
        return (0.0, 0.0, scale);
    }
    (
        forward / magnitude * scale,
        lateral / magnitude * scale,
        scale,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three cutoffs of the gait helper, each at its exact boundary:
    /// full only strictly above `GAIT_FULL_CUTOFF`, walk down to and including
    /// `GAIT_DEAD_ZONE`, zero below it.
    #[test]
    fn gait_scale_reads_its_three_bands_at_the_boundaries() {
        assert_eq!(gait_scale(1.0, false), GAIT_RUN);
        assert_eq!(gait_scale(GAIT_FULL_CUTOFF + 0.05, false), GAIT_RUN);
        assert_eq!(gait_scale(GAIT_FULL_CUTOFF, false), GAIT_WALK);
        assert_eq!(gait_scale(0.5, false), GAIT_WALK);
        assert_eq!(gait_scale(GAIT_DEAD_ZONE, false), GAIT_WALK);
        assert_eq!(gait_scale(GAIT_DEAD_ZONE - 0.01, false), 0.0);
    }

    /// The per-actor override raises only a scale at or below
    /// `GAIT_OVERRIDE_RAISE_LIMIT`, and never past `GAIT_WALK`.
    #[test]
    fn gait_override_raises_only_a_sub_third_scale_to_walk() {
        assert_eq!(gait_scale(0.0, true), GAIT_WALK);
        assert_eq!(gait_scale(GAIT_DEAD_ZONE - 0.01, true), GAIT_WALK);
        assert_eq!(gait_scale(0.5, true), GAIT_WALK);
        assert_eq!(gait_scale(1.0, true), GAIT_RUN);
    }

    /// The helper normalizes against the pair's own length before rescaling,
    /// so a full diagonal crosses at unit speed on its heading, and a half
    /// press at two thirds of one axis; a zero pair is multiplied out to zero
    /// rather than divided.
    #[test]
    fn scaled_move_axes_normalizes_before_rescaling() {
        let (f, l, s) = scaled_move_axes(1.0, 1.0, false);
        assert_eq!(s, GAIT_RUN);
        assert!((f - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6 && f == l);

        let (f, l, s) = scaled_move_axes(-0.5, 0.0, false);
        assert_eq!(s, GAIT_WALK);
        assert!((f + GAIT_WALK).abs() < 1e-6 && l == 0.0);

        let (f, l, s) = scaled_move_axes(0.0, 0.0, false);
        assert_eq!((f, l, s), (0.0, 0.0, 0.0));
        let (_f, _l, s) = scaled_move_axes(0.0, 0.0, true);
        assert_eq!(s, GAIT_WALK);
    }
}
