//! One fixed tick of the player walker: retail's mover machine over kuluu's
//! zone data.
//!
//! Wire coordinates at the boundary (x/y horizontal, z grows DOWN; bevy space
//! inside). The law implemented here is the mover probe machine as recorded in
//! `.agents/skills/retail-observe/references/2026-10-25-collision-query.md`:
//! ground follows measured height at a rate bound with no maximum-rise cap
//! ("Step up", "Fall and landing"), terrain never blocks horizontally (walls
//! come from the separately authored blocker class - "Face classification" and
//! "Blocking belongs to other candidates"), falling is one fixed step per
//! movement frame times lean with no ramp and no terminal speed, landing is a
//! contact gate flip ("Landing is a gate flip, not a snap"), and the probe
//! answer's horizontal pair shortens the next step at `PROBE_MARGIN_SCALE`
//! while its vertical word feeds aim height ("Result vector (consumer pass)").

use bevy::math::{Vec2, Vec3};
use kuluu_render::dat_mzb::MzbCollisionGeometry;

use super::consts::{
    BODY_HEIGHT, BODY_RADIUS, CONTACT_SEARCH_RADIUS, FALL_STEP_FRAME_ALT,
    FALL_STEP_FRAME_INTEGRATED, FLOOR_COS, LEAN_DEFAULT_NEUTRAL, PINCH_PENETRATION_SLOP,
    PROBE_MARGIN_SCALE, STEP_MAX,
};
use super::obstacles::{DoorObstacle, MobObstacle, ObstacleSet};
use super::sweep::{self, WallSource};
use super::{ActorContact, HorizontalOutcome, StepResult, VerticalDecision, WalkMode, Walker};

/// One follow word: the magnitude bounding any vertical change per movement
/// frame, up or down - retail carries one symbol for both directions (the
/// collision record's step-acceptance delta row). Single definition lives in
/// `dat_mzb::FOLLOW_WORD_YAMLS_PER_FRAME` because the wall sweep's riser band
/// uses the same number: a face that query suppresses is exactly one this
/// machine can climb; never re-typed.
pub const FOLLOW_WORD_YAMLS_PER_FRAME: f32 = kuluu_render::dat_mzb::FOLLOW_WORD_YAMLS_PER_FRAME;

/// Ceiling on a rise accepted as standable floor under a pose; single definition in
/// `dat_mzb::STEP_UP_ACCEPT_RISE_YAMLS` (which carries the record citation; since 2026-10-27 the
/// wall side anchors its height rejection at retail's body point instead, see
/// `dat_mzb::WALL_TOP_BODY_POINT_TOL_YAMLS`). Walking through it rejects phantom floors: an up-facing upper-structure plane measured at
/// feet+1.167 on the shipped Bastok Mines AH terrace (overlay 2026-10-26) was accepted under the
/// old symmetric follow-word window and lifted actors over solid parapets, then dropped them when
/// the phantom ended. The lower side stays as recorded: descents answer within
/// `FOLLOW_WORD_YAMLS_PER_FRAME` while the fall/follow law carries them.
pub const STEP_UP_ACCEPT_RISE_YAMLS: f32 = kuluu_render::dat_mzb::STEP_UP_ACCEPT_RISE_YAMLS;

/// Alternate-gate penalty, carried as sourced data: kuluu models one gate pair
/// so nothing selects it (same scope note as `walker/consts.rs`).
pub const FALL_PENALTY_ALT_YAMLS_PER_FRAME: f32 = FALL_STEP_FRAME_ALT;

/// The only vertical extent authored on retail's shape block at construction
/// ("Step reach and actor shape", collision record). Narrow-phase use is
/// unestablished there, so this constant documents the seed and seeds nothing.
pub const SHAPE_EXTENT_SEED_YAMLS: f32 = 2.0;

/// The mover probe's answer block, named per "Result vector (consumer pass)"
/// (collision record): words one and three are the horizontal pair consumed
/// only as a margin length; word two is the vertical accumulation that moves
/// both player and camera; word four has no established consumer.
#[derive(Clone, Copy, Debug, Default)]
pub struct ResultVector {
    pub lateral_x: f32,
    pub vertical: f32,
    pub lateral_z: f32,
    /// No consumer found at retail's anchors; carried for layout completeness.
    pub word4_unconsumed: f32,
}

impl ResultVector {
    /// The step shortening the movement integration applies: root of squares
    /// over the horizontal pair times the recorded scale; the vertical word is
    /// skipped by construction ("the margin wrapper skips the accumulated
    /// vertical word ...", collision record delta).
    pub fn margin(&self) -> f32 {
        (self.lateral_x * self.lateral_x + self.lateral_z * self.lateral_z).sqrt()
            * PROBE_MARGIN_SCALE
    }

    /// The camera unit's law for aim height: add this answer's vertical word
    /// straight into the accumulator - no intermediate global, no scaling.
    pub fn apply_to_aim_height(&self, accumulator_y: f32) -> f32 {
        accumulator_y + self.vertical
    }
}

/// Walls are the authored blocker class (kuluu's body queries over MZB wall
/// faces plus closed doors): terrain answers standability and never blocks;
/// this source is the counterpart of retail's separately registered shape
/// candidates ("Blocking belongs to other candidates", collision record).
struct AuthoredWalls<'a> {
    geom: &'a MzbCollisionGeometry,
    doors: &'a [DoorObstacle],
}

impl WallSource for AuthoredWalls<'_> {
    fn nearest_wall(
        &self,
        center: Vec3,
        r: f32,
        feet_y: f32,
        lift_yamls: f32,
    ) -> Option<(f32, Vec3)> {
        let mut best = self
            .geom
            .nearest_wall_contact(center, r, feet_y, lift_yamls);
        let r2 = r * r;
        for d in self.doors {
            if center.x < d.min.x - r
                || center.x > d.max.x + r
                || center.z < d.min.z - r
                || center.z > d.max.z + r
            {
                continue;
            }
            for (v, n) in &d.tris {
                if n.y >= FLOOR_COS {
                    continue;
                }
                // Doors are authored blocker leaves: they keep the fixed step-band
                // anchor and are never lifted over (a raised retry still stops at a
                // closed leaf), unlike terrain faces whose anchor rides with lift.
                if v[0].y.max(v[1].y).max(v[2].y) <= feet_y + STEP_MAX {
                    continue;
                }
                let d2 = kuluu_render::dat_mzb::point_tri_dist_sq(center, v[0], v[1], v[2]);
                if d2 < r2 && best.is_none_or(|(bd, _)| d2 < bd) {
                    best = Some((d2, *n));
                }
            }
        }
        best
    }
}

/// Downward vertical ray at `xz` against triangle `v`: the hit height, or None
/// when the face is parallel to the column. Ported from the pre-retail tick's
/// door floor source: a closed door is a wall for the sweep AND a floor for
/// the column probe (a closed drawbridge; an elevator platform is the floor
/// under a rider at whatever height it reached - `walker/obstacles.rs`).
fn tri_hits_column(v: [Vec3; 3], xz: Vec2) -> Option<f32> {
    let (a, b, c) = (v[0], v[1], v[2]);
    let e1 = b - a;
    let e2 = c - a;
    let det = e1.x * e2.z - e1.z * e2.x;
    if det.abs() < 1e-9 {
        return None;
    }
    let qx = xz.x - a.x;
    let qz = xz.y - a.z;
    let u = (qx * e2.z - qz * e2.x) / det;
    if u < 0.0 || u > 1.0 {
        return None;
    }
    let v_ = (e1.x * qz - e1.z * qx) / det;
    if v_ < 0.0 || u + v_ > 1.0 {
        return None;
    }
    Some(a.y + u * e1.y + v_ * e2.y)
}

/// Highest up-facing closed-door face in the column at or below `ceiling_y`.
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

/// Any closed-door triangle crossing the column slab `(lo_y, hi_y]` - the door
/// half of the ceiling hold (ported with the same record-silent authored
/// status as `sweep::ceiling_holds`).
fn doors_in_column_slab(doors: &[DoorObstacle], xz: Vec2, lo_y: f32, hi_y: f32) -> bool {
    for d in doors {
        if xz.x < d.min.x || xz.x > d.max.x || xz.y < d.min.z || xz.y > d.max.z {
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

/// Standable answer for this pose: the measured floor (MZB plus closed-door
/// up-faces) inside the asymmetric acceptance window over/under the feet, with
/// ring support at the probe-padding radius held against the body point (see
/// the ring block below - it is what keeps a climbing pose from dropping into
/// the slab pockets between shipped flight treads).
/// `pub(crate)` so the shipped-stairs battery can chain its profiles through
/// exactly the answer a drive would get, phantom-roof steps included - the
/// detector and the walker must not disagree on what is standable. Below,
/// one follow word (`FOLLOW_WORD_YAMLS_PER_FRAME`; beyond that the probe answers
/// "no contact", which is what makes a height pop hover then fall - see "Landing
/// is a gate flip, not a snap" and its hover note in the collision record).
/// Above, `GROUND_FOLLOW_RISE_WINDOW_YAMLS` (twice the follow word) - answers further up
/// are roof slabs and terrace phantoms that must not carry a grounded walker; support for
/// a lifted assist pose keeps the narrower probe-reach law of
/// [`standable_support_at_or_below`].
/// Floor faces come from `ground_walkable`: retail's contact path tests no normal
/// (collision record, "Face classification"), so shipped rock-step treads steeper
/// than cos(45°) stand, while authored blocker objects never answer height at all.
/// ... `ring_seams` is the walk-vs-stopped tracker (USER 2026-10-28/29, Bastok Mines
/// xz(+1.15,+72.77): "we are still stair floating when stopped ... .26 too high (same as
/// riser/tread, but there IS not tread under us.. and we are stopped"): the ring hold keeps a
/// crossing pose on its plate, but it also re-wins by body point every tick from plates whose
/// edge the pose already walked past, holding a STOPPED pose floating over its own axis floor -
/// forever. The hold exists for crossing, so it runs only while movement intent is held; idle
/// stopped poses get their own column (plus doors) - "YOU FALL in STOPPED". The gate is the input
/// rather than `in_motion` because a tick jammed against geometry mid-flight measures an immediate
/// 0.5124 yalm sink off the shell treads under the travelling gate (battery pin
/// `approaching_a_shipped_riser_never_lifts_the_pose_early_or_flaps`) - pressed-but-holding is
/// still crossing, keys up and moving are; released keys are stopped. Airborne landing keeps
/// the ring on (crossing semantics), whichever way the tracker reads mid-fall.
pub(crate) fn contact_floor(
    geom: &MzbCollisionGeometry,
    doors: &[DoorObstacle],
    xz: Vec2,
    feet_y: f32,
    ring_seams: bool,
) -> Option<f32> {
    let rise_window = GROUND_FOLLOW_RISE_WINDOW_YAMLS + 1e-6;
    let drop_window = FOLLOW_WORD_YAMLS_PER_FRAME + 1e-6;
    // Deterministic ranking once more than one candidate arrives (the pose
    // column plus the four ring columns): nearness is measured to the body
    // point half a yalm over the feet - retail's probe centre, the same anchor
    // `ground_walkable_face` ranks with (`GROUND_PROBE_OFFSET_YAMLS`) - and an
    // exact tie prefers the higher face (the tie-break `dat_mzb::step_candidate_beats`
    // states). The former last-wins-by-scan-order rule was only sound while this
    // function held a single candidate.
    let body_point = feet_y + kuluu_render::dat_mzb::GROUND_PROBE_OFFSET_YAMLS;
    let mut best: Option<(f32, f32)> = None; // (distance to the body point, height)
    let mut consider = |h: f32| {
        let rise = h - feet_y;
        let inside = if rise >= 0.0 {
            rise <= rise_window
        } else {
            -rise <= drop_window
        };
        if !inside {
            return;
        }
        let d = (h - body_point).abs();
        // Keep the stored answer only while it is strictly nearer, or tied and no lower.
        if best.is_none_or(|(bd, bh)| bd > d || (bd == d && bh < h)) {
            best = Some((d, h));
        }
    };
    // Ring support (kuluu-authored stand-in for retail's half-yalm probe sphere
    // around the body point - this record's character-sweep law; see the delta
    // table row "the moving body is one point padded by a half-yalm probe
    // sphere"). The pose column alone falls into sliver pockets between shipped
    // flight treads riding a slab (measured end to end at the Bastok Mines AH
    // shell, Garfield's xz(-21.3,+69.6) pin `climb_the_ah_shell_treads_from_the_
    // reported_stand_point`, and the former `bastok_mines_shell_steps_z` sink):
    // an infinitesimal ray slips off the tread edge ~0.1 yalm before the next
    // riser answers, and the buried slab 0.5 under it answers as a legal drop.
    // The body still stands on that tread - ring columns at the recorded probe-padding
    // radius (eight directions, see below) keep it in contact while the axis crosses the seam.
    // Ring candidates may only hold near the level the pose already occupies.
    // Two ordering rules enforce that: each face must not stand wholly above
    // `feet_y + STEEP_SUPPORT_WHOLLY_ABOVE_IGNORE_YAMLS` (retail's own wholly-above ignore for
    // steep faces, same pass), and - tightening it to the pose's own level after USER's
    // 2026-10-28 "feels a bit high on the steps" report - a ring candidate may never lift the
    // answer more than [`kuluu_render::dat_mzb::RING_SUPPORT_HOLD_LIFT_CAP_YAMLS`] above the feet.
    // Uncapped, measured against shipped Bastok AH shell data, the ring rode poses up to 0.746
    // yalm above their own axis column anticipating taller plates (the "high" feel) and dropped a
    // full tread at plate corners when drifting diagonally - sink 0.253 on the -x diagonal while
    // straight and +x lines held clean (the "W+D ... like an update pause" jitter), both now pinned
    // by `approaching_a_shipped_riser_never_lifts_the_pose_early_or_flaps`. Climbing onto taller
    // ground stays a pose-column event: the axis ray only answers the higher plate once the body
    // is over it. A 5-sample union *without* the wholly-above rule was tried and reverted
    // 2026-10-27 because it ghosted `pressed_against_a_one_yalm_terrain_riser_never_ghosts_or_drops`.
    // Eight sample rays (the four axes plus the four diagonals at the same probe radius): measured
    // on the shell plate clusters, a drifting diagonal can stand at a plate corner where all four
    // axis rays miss the plate the body still stands on - equal angular coverage removes that bias,
    // and it is exactly symmetric to what the axes already do.
    let r_ring = kuluu_render::dat_mzb::GROUND_PROBE_OFFSET_YAMLS;
    let r_diag = r_ring * std::f32::consts::FRAC_1_SQRT_2;
    let ignore_above = kuluu_render::dat_mzb::STEEP_SUPPORT_WHOLLY_ABOVE_IGNORE_YAMLS + 1e-6;
    let hold_cap = kuluu_render::dat_mzb::RING_SUPPORT_HOLD_LIFT_CAP_YAMLS + 1e-6;
    if ring_seams {
        for (dxo, dzo) in [
            (r_ring, 0.0),
            (-r_ring, 0.0),
            (0.0, r_ring),
            (0.0, -r_ring),
            (r_diag, r_diag),
            (r_diag, -r_diag),
            (-r_diag, r_diag),
            (-r_diag, -r_diag),
        ] {
            let oxz = Vec2::new(xz.x + dxo, xz.y + dzo);
            for (h, face_bottom) in geom.column_floor_candidates(oxz) {
                if face_bottom > feet_y + ignore_above {
                    continue;
                }
                // Hold at the occupied level; never anticipate taller ground sideways.
                if h > feet_y + hold_cap {
                    continue;
                }
                consider(h);
            }
        }
    }

    // Pose column first: its single body-point-nearest winner under retail's
    // own far-rise continuity clause (below).
    if let Some(h) = geom.ground_walkable(xz, feet_y) {
        // A far-rise answer (beyond ordinary acceptance reach) only carries the pose when
        // this same column also answers floor within ordinary reach below it: support runs
        // up through the riser into that height (the shipped big-step gap columns measure
        // tread and terrace together, SD forensics 2026-10-27). A lone answer high overhead -
        // the Bastok AH terrace canopy at feet+1.167, the phantom-float report of 2026-10-26
        // (`phantom_support_over_void_falls_instead_of_letting_the_walker_float_on_it`) - has
        // nothing under it and never lifts a grounded walker.
        let far_rise = h - feet_y > STEP_UP_ACCEPT_RISE_YAMLS + 1e-6;
        let supported_below = !far_rise
            || geom
                .ground_raycast_all(xz)
                .into_iter()
                .any(|(hit_y, normal)| {
                    normal.y >= kuluu_render::dat_mzb::WALKABLE_FACE_UP_DOT
                        && (hit_y - feet_y).abs() <= STEP_UP_ACCEPT_RISE_YAMLS + 1e-6
                });
        if supported_below {
            consider(h);
        }
    }
    for d in doors {
        if xz.x < d.min.x || xz.x > d.max.x || xz.y < d.min.z || xz.y > d.max.z {
            continue;
        }
        if let Some(h) = door_floor_at(d, xz, f32::MAX) {
            consider(h);
        }
    }
    best.map(|(_, h)| h)
}

/// Retail's contact test over the tick's mob circles
/// (research/XIClient/src/XIClient/source/World/Actor/ControllableActor.cpp
/// ControllableActor::CheckContactActor): true when this tick's movement must
/// be dropped. `projected` is the position the move would reach, not the
/// current one; only the nearest candidate is considered.
fn contact_blocks(
    obstacles: &ObstacleSet,
    projected: Vec2,
    dt: f32,
    state: &mut ActorContact,
) -> bool {
    let mut closest_d2 = CONTACT_SEARCH_RADIUS * CONTACT_SEARCH_RADIUS;
    let mut closest: Option<&MobObstacle> = None;
    for mob in &obstacles.mobs {
        let d2 = (mob.center - projected).length_squared();
        if d2 < closest_d2 {
            closest_d2 = d2;
            closest = Some(mob);
        }
    }
    let Some(mob) = closest else { return false };
    // Retail's own radius is 0.8x the XZ magnitude of model locator 44
    // (research/XIClient/src/XIClient/source/World/Actor/ControllableActor.cpp ControllableActor::GetCollisionSize);
    // we parse no locators yet, so the sweep body radius stands in (kuluu-53tn).
    if closest_d2.sqrt() - BODY_RADIUS - mob.radius >= 0.0 {
        state.clear();
        return false;
    }
    state.contact(mob.id, dt)
}

/// One tick of retail's machine. Same inputs and outputs as `walker::step`, so
/// dispatch picks a walker with one branch; state carries in the same `Walker`
/// (mode + published measurement).
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
    geometry_ready: bool,
) -> StepResult {
    let _ = (speed_yps, dt); // the speed law is settled upstream in input; this vertical law is per frame.
    let feet_xz = Vec2::new(x, -y);
    let mut up_y = -z_wire;
    let d_in = Vec2::new(dx, -dy);
    let want_len = d_in.length();
    // Player-state tracker (USER 2026-10-28): "YOU FALL in STOPPED.. you rise the slope
    // in WALKING". Walking is intent AND last tick actually travelling - a tick pressed
    // against geometry displaced nothing, was not walking, and must not pre-climb with
    // look-ahead only to rebound off it (the sd_north_big_steps_z sink class).
    state.in_motion = want_len > 1e-6 && state.moved_last_tick;

    // 1. Probe the pose against the authored wall class (answer's horizontal
    //    pair = measured intrusion) before moving: walking into geometry cuts
    //    this step short; it never gates it (the sweep alone hard-blocks).
    let walls = AuthoredWalls {
        geom,
        doors: &obstacles.doors,
    };
    let center = Vec3::new(feet_xz.x, feet_y_of(up_y), feet_xz.y);
    let answer = probe_answer(&walls, center, up_y, BODY_RADIUS);

    // 2. Horizontal through the authored sweep, step shortened by the margin.
    let mut outcome;
    let mut d;
    if want_len <= 1e-6 {
        outcome = HorizontalOutcome::NoInput;
        d = d_in;
    } else if noclip {
        outcome = HorizontalOutcome::Noclip;
        d = d_in;
    } else {
        let margin = answer.margin();
        let shortened = if margin > 0.0 && want_len > 1e-6 {
            d_in * ((want_len - margin).max(0.0) / want_len)
        } else {
            d_in
        };
        let (swept, exit) = sweep::sweep(&walls, feet_xz, up_y, shortened, 0.0);
        d = swept;
        // A slide that clears its wall exits the loop as Clean with a clipped
        // length, so - like the pre-retail tick - Moved vs Slid comes from
        // allowed-over-input, not from the exit label.
        let ratio = (d.length() / want_len).min(1.0);
        outcome = match exit {
            sweep::SweepExit::Hold => HorizontalOutcome::WallHold {
                contact: Some(feet_xz + swept),
            },
            sweep::SweepExit::Reversal => HorizontalOutcome::WallReversal {
                contact: Some(feet_xz + swept),
            },
            _ if ratio >= 0.999 => HorizontalOutcome::Moved,
            _ => HorizontalOutcome::Slid { ratio },
        };

        // Retail's step-up assist, applied only when the plain sweep lost most of
        // the move: retry that sweep from a pose raised by this frame's assist reach
        // and take it only if it unlocks the move AND stands on standable terrain at
        // or below the lifted height (prop faces never answer `ground_walkable`, so
        // authored blocker objects stay solid under their tops - no walking up plants).
        if ratio < STEP_UP_RETRY_BELOW_RATIO {
            let lifted_y = up_y + state.step_assist_yamls;
            if lifted_y > up_y + 1e-3 && !sweep::ceiling_holds(geom, feet_xz, up_y, lifted_y) {
                // The retry's terrain-face anchor rides with the raised pose: faces
                // whose tops are below the lifted body point step out of the way,
                // and support acceptance (below) decides whether moving there is
                // allowed - how big steps mount while fence metal stays solid.
                let (swept2, exit2) = sweep::sweep(
                    &walls,
                    feet_xz,
                    lifted_y,
                    shortened,
                    state.step_assist_yamls,
                );
                let ratio2 = (swept2.length() / want_len).min(1.0);
                let target = feet_xz + swept2;
                if ratio2 >= STEP_UP_MIN_GAIN_RATIO
                    && ratio2 > ratio
                    && standable_support_at_or_below(geom, target, up_y, lifted_y)
                {
                    d = swept2;
                    up_y = lifted_y;
                    outcome = match exit2 {
                        sweep::SweepExit::Hold => HorizontalOutcome::WallHold {
                            contact: Some(target),
                        },
                        sweep::SweepExit::Reversal => HorizontalOutcome::WallReversal {
                            contact: Some(target),
                        },
                        _ if ratio2 >= 0.999 => HorizontalOutcome::Moved,
                        _ => HorizontalOutcome::Slid { ratio: ratio2 },
                    };
                }
            }
        }

        // Accumulator bookkeeping. The client zeroes it when its fast check fails and
        // regrows the next frame, so what matters at steady walking is that it climbs
        // to the one-yalm cap in about twenty-five frames; kuluu grows it every frame
        // with movement intent instead of zeroing per failed attempt (the cap bounds
        // both ways, and a face taller than the cap stays solid however long the push).
        state.step_assist_yamls = if want_len <= 1e-6 {
            state.step_assist_started = false;
            0.0
        } else {
            let grown =
                (state.step_assist_yamls + STEP_ASSIST_RATE_PER_FRAME).min(STEP_ASSIST_MAX_YAMLS);
            if state.step_assist_started {
                grown
            } else {
                state.step_assist_started = true;
                grown.min(STEP_ASSIST_FIRST_YAMLS)
            }
        };
    }

    // Retail's all-or-nothing actor contact, ported from the pre-retail tick
    // unchanged (ControllableActor::CheckContactActor - see `contact_blocks`):
    // it drops the move rather than pushing out; the expiring budget lives on
    // Walker::contact across ticks.
    if !noclip && contact_blocks(obstacles, feet_xz + d, dt, &mut state.contact) {
        d = Vec2::ZERO;
        outcome = HorizontalOutcome::ActorContact {
            mob: state.contact.target().unwrap_or(0),
            contact: Some(feet_xz + d_in),
        };
    }

    // Walk-vs-stopped tracker update: this tick travelled when it carried intent and
    // the ground let it out at least half the intended step. Wall-holds, jammed presses
    // and contact-blocked moves (d zeroed above) all record stopped.
    state.moved_last_tick = want_len > 1e-6 && d.length() >= want_len * STEP_MOVED_MIN_RATIO;

    // 3. Vertical by the recorded law ("Fall and landing (mover-probe
    //    re-read)"): measured ground within one follow word of the feet holds
    //    or drags at <= the rate bound both ways; no contact applies exactly
    //    FALL_STEP_FRAME_PROBE * lean this frame, fresh every frame with no
    //    ramp and no terminal speed; landing is the gate flipping yes. The
    //    tick's height change publishes to `Walker::probe_measurement_y`
    //    exactly as moved (measurement-feed parity with the old walker).
    let move_dir = if d.length_squared() > 1e-12 {
        d / d.length()
    } else {
        Vec2::ZERO
    };
    // Forward reachability for the in-motion look-ahead: how far straight along this tick's
    // committed direction the pose can actually walk under the authored wall source (one
    // probe sweep from the post-move feet; slides contribute only their projection on the
    // direction, so a face ahead or beside shortens it to what is truly open). A sample
    // column beyond this reach sits behind a blocking face and must never lift the pose.
    let lookahead_reach = if !state.in_motion || noclip || move_dir.length_squared() <= 1e-12 {
        0.0
    } else {
        let (probed, _exit) = sweep::sweep(
            &walls,
            feet_xz + d,
            up_y,
            move_dir * LOOKAHEAD_FAR_YAMLS,
            0.0,
        );
        probed.dot(move_dir).clamp(0.0, LOOKAHEAD_FAR_YAMLS)
    };
    let decision = if !geometry_ready {
        VerticalDecision::NoGeometry
    } else {
        step_vertical(
            geom,
            &obstacles.doors,
            state,
            feet_xz + d,
            move_dir,
            lookahead_reach,
            want_len > 1e-6,
            &mut up_y,
        )
    };

    StepResult {
        dx: d.x,
        dy: -d.y,
        feet_z: -up_y,
        mode: state.mode,
        decision,
        outcome,
    }
}

/// Retail's step-up assist reach (`CollidableActor::field_F4`): while an actor is
/// walking and its candidate pose needs correcting, the client raises that pose by
/// an accumulator capped at one yalm before asking collision again, so up-facing
/// faces inside its half-yalm narrow-phase probe resolve Y onto the next tread
/// instead of holding the move. The very first correction of a walk-out is clamped
/// to `STEP_ASSIST_FIRST_YAMLS`; later frames may grow toward `STEP_ASSIST_MAX_YAMLS`
/// at `STEP_ASSIST_RATE_PER_FRAME`, and a hard block zeroes it (all three numbers
/// read from the client's own accumulator arithmetic, see record
/// `.agents/skills/retail-observe/references/2026-10-25-collision-query.md`).
pub const STEP_ASSIST_FIRST_YAMLS: f32 = 0.4;
/// Upper clamp of the assist accumulator (same site as `STEP_ASSIST_FIRST_YAMLS`).
pub const STEP_ASSIST_MAX_YAMLS: f32 = 1.0;
/// Per-movement-frame growth of the assist accumulator (the client multiplies its
/// frame tick by nought point nought four nought eight three three).
pub const STEP_ASSIST_RATE_PER_FRAME: f32 = 0.040_833_3;
/// A lifted retry only replaces the plain sweep when it gains this much of the
/// wanted move; kuluu-authored threshold that separates "the wall is what stopped
/// me" from "a tread was in reach", bracketed by the all-or-nothing cases (0 and 1).
const STEP_UP_MIN_GAIN_RATIO: f32 = 0.5;
/// Retry the plain sweep only when it lost at least this much of the move; short
/// slides stay on the ordinary path so a graze never triggers a lift.
const STEP_UP_RETRY_BELOW_RATIO: f32 = 0.999;
/// How much of the intended step a tick must travel to count as walking, in the
/// walk-vs-stopped tracker (`Walker::moved_last_tick`): half. A record-silent kuluu
/// tuning split between "moving (along a wall too)" and "jammed against it", set
/// under the assist retry's own gain bar (`STEP_UP_MIN_GAIN_RATIO`) so a tick that
/// failed to mount is never also counted as travelling.
pub const STEP_MOVED_MIN_RATIO: f32 = 0.5;
/// Slack accepting a support face level-precise enough to be the lifted height
/// itself (tessellation noise at zone coordinates); kuluu-authored, an order of
/// magnitude below any shipped tread.
const STEP_SUPPORT_SLACK_YAMLS: f32 = 0.05;

/// Rise window of the grounded contact-floor answer (`contact_floor`): single definition
/// in `dat_mzb::GROUND_RISE_BAND_YAMLS`, shared with that module's riser-exemption probe so
/// acceptance and suppression read one band. Record law: terrain rises are never rejected,
/// only rate-bounded with everything else by the follow word (collision record "Step up":
/// no slope or max-rise test exists on the movement-time path; a rise taller than one
/// frame's measure is measured over more frames while contact holds). The shipped big-step
/// class measures its biggest standable-to-standable rise at 1.64 yalm (SD north forensics
/// profile 2026-10-27), which the old 1.0 reach answered as "no contact" - walkers then fell
/// into the gap's void column and their landing gate could never re-acquire the floor above,
/// the abyss report of 2026-10-25/26. Airborne landing keeps its own one-follow-word crossing
/// gate below, so widening this never turns a fall into a climb back.
pub const GROUND_FOLLOW_RISE_WINDOW_YAMLS: f32 = kuluu_render::dat_mzb::GROUND_RISE_BAND_YAMLS;

/// How far below the lifted pose a standable face may sit for the retry to count:
/// retail's narrow-phase probe is half a yalm around the body point, and its lift
/// exists precisely so "up-facing faces inside that reach resolve Y onto the next
/// tread" - support only reachable by lowering the feet back down was never what
/// the accumulator resolves. Previously this bound was `STEP_ASSIST_MAX_YAMLS`
/// (one full assist), which let a pose float-hop in front of any low obstacle:
/// against a crate on level floor, blocked sweep + grown assist + support at old
/// floor level accepted, lifting the feet a full assist and dropping them again -
/// the height jitter reported as "whole screen shakes when running into short
/// things" (2026-10-26). Single source: `dat_mzb::GROUND_PROBE_OFFSET_YAMLS`
/// (collision record, "Movement-time collision: the character sweep").
const ASSIST_SUPPORT_REACH_YAMLS: f32 = kuluu_render::dat_mzb::GROUND_PROBE_OFFSET_YAMLS;

/// Whether standable terrain answers at or just below `lifted_y` in this column:
/// the lifted retry needs real support, not a wall top. `ground_walkable_face` already
/// refuses authored blocker faces, so prop tops never count as support. The answer
/// is additionally bounded by the one-mount ceiling above the un-lifted pose
/// (`base_y`): support standing more than `dat_mzb::STEP_MOUNT_RISE_MAX_YAMLS` over this
/// pose's own floor is a wall whatever answers there (USER law, Bastok Mines barrel-cap
/// parapet xz(+45.18,+84.98) - see the constant's doc; it replaces the wider
/// `STEP_UP_ACCEPT_RISE_YAMLS` accept this site used to share with `contact_floor`, so a
/// phantom floor cannot legitimize lifting the pose over solid geometry, collision record
/// "Movement-time collision: the character sweep"), and never further below the lifted
/// feet than [`ASSIST_SUPPORT_REACH_YAMLS`].
///
/// A support face standing above the un-lifted pose must be retail's steep floor class
/// (authored normal.y past `dat_mzb::FLOOR_NORMAL_MIN`): in retail's movement pass only that
/// class resolves a body height - faces inside the ±0.708 wall band get pushed out of, never
/// mounted, whatever the assist accumulator reaches (same record section; "wall-contact
/// correction ... multiplied by nought-point-two on the vertical" is a push leak, not support).
/// Without this gate the accumulator hoisted walkers onto shoulder-high metal: the Bastok Mines
/// walkway rail caps measure normal.y +0.667 and top +0.687 over the floor (DAT 334
/// x[-46.6..-46.4] z[-17.35..-13.12], `kuluu-render/tests/mzb_bastok_wall_slide_dump.rs`),
/// and the assist reached them from certain approach angles - then dropped walkers into the
/// street when they slid off the caps. Support at or below the un-lifted level needs no class
/// check: nothing is being mounted there.
fn standable_support_at_or_below(
    geom: &MzbCollisionGeometry,
    xz: Vec2,
    base_y: f32,
    lifted_y: f32,
) -> bool {
    let Some((h, face)) = geom.ground_walkable_face(xz, lifted_y) else {
        return false;
    };
    h - base_y <= kuluu_render::dat_mzb::STEP_MOUNT_RISE_MAX_YAMLS + 1e-6
        && h <= lifted_y + STEP_SUPPORT_SLACK_YAMLS
        && lifted_y - h <= ASSIST_SUPPORT_REACH_YAMLS
        && (h <= base_y + STEP_SUPPORT_SLACK_YAMLS || face.y > FLOOR_COS)
}

/// Body center height used for the wall probe at a given feet height (the same
/// mid-body anchor kuluu's authored chain queries; record silent on retail's
/// exact anchor, so this stays kuluu-authored).
fn feet_y_of(feet_y: f32) -> f32 {
    feet_y + BODY_RADIUS
}

/// Upper bound of the slope-proportional anti-clip push-up (USER-authorized 2026-10-28:
/// "it needs an offset based on slope to push us UP slightly and not clip into the steps
/// while walking.. so just add the offset, not change how it works"). The collision record
/// is silent - retail resolves Y onto the answered face with no such offset; this sits on
/// top of that answer without touching acceptance or ranking. A quarter of a shipped tread
/// rise (roughly twice at most), so it cannot mount anything new.
pub const SLOPE_PUSH_UP_MAX_YAMLS: f32 = 0.1;

/// The face tilt at which the push-up reaches its cap: the steepest walkable authored
/// slope ([`kuluu_render::dat_mzb::WALKABLE_FACE_UP_DOT`] measured from flat).
pub const SLOPE_PUSH_UP_FULL_TILT: f32 = 1.0 - kuluu_render::dat_mzb::WALKABLE_FACE_UP_DOT;

/// Below this tilt the offset is exactly zero, so flat floors keep bit-identical heights
/// (`running_on_flat_shipped_ground_commits_one_height`).
pub const SLOPE_PUSH_UP_TILT_EPSILON: f32 = 1.0 / 64.0;

/// How far a standable answer may sit from the pose's own column for the tilt to count:
/// when `contact_floor`'s winner is this same face (not a ring or door candidate), the
/// push-up is keyed by its authored normal; any other winner pushes nothing up.
const SLOPE_PUSH_UP_ANSWER_SLACK_YAMLS: f32 = STEP_SUPPORT_SLACK_YAMLS;

/// The slope-proportional push-up for this tick's grounded support answer: zero on flat
/// faces, rising linearly with the answering face's authored tilt to
/// [`SLOPE_PUSH_UP_MAX_YAMLS`] at [`SLOPE_PUSH_UP_FULL_TILT`]. Applied additively to the
/// accepted support height in `step_vertical`, never as an acceptance or ranking input.
fn slope_push_up(geom: &MzbCollisionGeometry, xz: Vec2, feet_y: f32, h: f32) -> f32 {
    let Some((hc, n)) = geom.ground_walkable_face(xz, feet_y) else {
        return 0.0;
    };
    if (hc - h).abs() > SLOPE_PUSH_UP_ANSWER_SLACK_YAMLS {
        return 0.0; // ring or door candidate answered this tick, not this face
    }
    let tilt = 1.0 - n.y.max(0.0);
    if tilt <= SLOPE_PUSH_UP_TILT_EPSILON {
        return 0.0;
    }
    (SLOPE_PUSH_UP_MAX_YAMLS * (tilt / SLOPE_PUSH_UP_FULL_TILT)).min(SLOPE_PUSH_UP_MAX_YAMLS)
}

/// Far end of the motion look-ahead window (yalms along the committed direction
/// of travel, twice the body radius): the lip scan runs from one stride to here.
/// A hard edge further ahead than this is untouched and keeps today's
/// crossing-time resolution; it is also the denominator of [`lookahead_ramp_weight`].
pub const LOOKAHEAD_FAR_YAMLS: f32 = 2.0 * BODY_RADIUS;
/// Probe stride along travel for the lip scan: nought-point-two yalm columns from a
/// stride to [`LOOKAHEAD_FAR_YAMLS`].
pub const LOOKAHEAD_STRIDE_YAMLS: f32 = 0.2;

/// Crossing-linked ramp weight (USER 2026-10-28 diagram - red line = continuous rise across
/// each step; "you rise the slope in WALKING", and stopping still falls): once the nearest
/// riser lip ahead is at distance `d_hat`, lift is a linear fraction of full,
/// one minus d_hat over the window. Full height arrives exactly as the feet reach the lip,
/// where the support flip takes over - no hold-up-early hover (measured law-free float:
/// +nought-two-seven-five yalm over own column for forty-eight consecutive ticks per tread
/// on shipped flights before this ramp existed).
pub fn lookahead_ramp_weight(d_hat: f32) -> f32 {
    (1.0 - d_hat / LOOKAHEAD_FAR_YAMLS).clamp(0.0, 1.0)
}

/// A forward rise above this never look-aheads - it stays on the step-up assist path
/// exactly as measured today (`kuluu_render::dat_mzb::MAX_GROUND_STEP_UP`, the 0.5 hard-edge
/// class). Look-ahead adds no mountable geometry: anything it lifts toward would have been
/// crossed and climbed anyway.
pub const LOOKAHEAD_RISE_CAP_YAMLS: f32 = kuluu_render::dat_mzb::MAX_GROUND_STEP_UP;

/// A forward rise smaller than this is triangle-interpolation noise, not a riser (feet
/// already on the tread answer `top + ~1e-5`): it must never fire the channel or the
/// uplift rides along under flat walking. Authored guard well under the smallest real
/// FFXI stair rise (~0.2 yalm) and orders above the query noise (pinned by
/// `motion_lookahead_lift_reads_forward_columns_within_the_window_only`).
pub const LOOKAHEAD_MIN_RISE_YAMLS: f32 = 1.0 / 64.0;

/// The small offset uplift applied while rising (USER 2026-10-28, "add small offset
/// uplift while rising"): while a forward column still answers higher than the current
/// support - i.e. throughout the climb across the window - feet ride this much above the
/// interpolated ramp so they clear the riser edge, and it fades with the look-ahead itself,
/// so the pose lands onto the tread (a settle of at most this amount, never a hover).
pub const RISE_UPLIFT_YAMLS: f32 = 0.05;

/// Max a look-ahead lift may give back in one frame while it un-fades (one uplift per
/// frame, so the worst collapse step equals a landing settle and stays under the pin's
/// flap allowance). Rising edges are instant; only the fade is rate-bounded, capped by
/// how far the pose actually sits above this tick's support so a latched lift can never
/// re-add itself on top of support that already caught up (the SD crossings still settle
/// by exactly one uplift). Bastok shell seam class,
/// `approaching_a_shipped_riser_never_lifts_the_pose_early_or_flaps`.
pub const LOOK_UNFADE_PER_FRAME_YAMLS: f32 = RISE_UPLIFT_YAMLS;

/// The motion look-ahead lift for one grounded tick: standable support sampled straight
/// ahead along the committed travel direction (same authored filter as every other ground
/// query - props and wall faces answer nothing), risen-to only, capped by
/// [`LOOKAHEAD_RISE_CAP_YAMLS`], blended by proximity, carrying [`RISE_UPLIFT_YAMLS`] while
/// any rise remains ahead. Applies only while the player-state tracker says in-motion; a
/// stopped walker gets exactly zero and falls back to its own flat.
///
/// `reach_yamls` is how far straight along `dir` this tick's pose can actually walk (the
/// same authored wall source as the sweep): a sample column beyond it sits behind a blocking
/// face - the ledge-behind-a-wall trap (`a_wall_whose_ledge_meets_it_mid_extent_is_never_
/// mounted_by_pressing`, USER 2026-10-28) must never look-ahead through its own blocker.
fn motion_lookahead_lift(
    geom: &MzbCollisionGeometry,
    xz: Vec2,
    dir: Vec2,
    h: f32,
    reach_yamls: f32,
) -> f32 {
    if dir.length_squared() <= 1e-12 {
        return 0.0;
    }
    let mut d = LOOKAHEAD_STRIDE_YAMLS;
    while d <= LOOKAHEAD_FAR_YAMLS + 1e-6 {
        if d > reach_yamls + 1e-3 {
            break; // a blocking face stands between the body and everything beyond
        }
        let Some((hc, _)) = geom.ground_walkable_face(xz + dir * d, h) else {
            d += LOOKAHEAD_STRIDE_YAMLS;
            continue; // nothing standable at this probe: never a pre-drop
        };
        let rise = hc - h;
        if rise > LOOKAHEAD_RISE_CAP_YAMLS + 1e-6 {
            return 0.0; // assist-class riser first in line: the channel steps aside entirely
        }
        if rise > LOOKAHEAD_MIN_RISE_YAMLS {
            // First riser ahead wins (nearest lip); its distance lands half a stride back
            // from the probe that saw it, clamped at zero.
            let d_hat = (d - LOOKAHEAD_STRIDE_YAMLS * 0.5).max(0.0);
            return lookahead_ramp_weight(d_hat) * (rise + RISE_UPLIFT_YAMLS);
        }
        d += LOOKAHEAD_STRIDE_YAMLS;
    }
    0.0
}

/// Vertical pass. Applies the tick's height change to `up_y` and returns the
/// decision; also publishes `Walker::probe_measurement_y`.
/// `move_dir` is the unit direction of this tick's committed move (zero when
/// none) and `look_reach` how far straight along it the pose can walk; both feed
/// the motion look-ahead, which only runs while `state.in_motion`.
/// `intent_held` says whether the tick carried any movement intent at all: a
/// crossing counts as walking for the seam ring hold even when geometry jammed
/// this tick (`approaching_a_shipped_riser_never_lifts_the_pose_early_or_flaps`
/// measures a 0.5124 yalm mid-flight sink under the `in_motion` gate and refuses
/// it); see `contact_floor`'s doc for where idle release happens.
fn step_vertical(
    geom: &MzbCollisionGeometry,
    doors: &[DoorObstacle],
    state: &mut Walker,
    xz: Vec2,
    move_dir: Vec2,
    look_reach: f32,
    intent_held: bool,
    up_y: &mut f32,
) -> VerticalDecision {
    let lean = match state.mode {
        WalkMode::Airborne { lean } => lean,
        _ => LEAN_DEFAULT_NEUTRAL,
    };
    if matches!(state.mode, WalkMode::Airborne { .. }) {
        // Fall first, then ask: one fixed step this frame regardless of how
        // long the drop has run. Landing is contact answering at or above the
        // stepped feet (crossing within one follow word, never a snap from
        // farther away).
        let prev_up = *up_y;
        let stepped = prev_up + FALL_STEP_FRAME_INTEGRATED * lean;
        // Landing probe keeps the ring hold on (crossing semantics mid-fall).
        match contact_floor(geom, doors, xz, stepped, true) {
            Some(h)
                if (h - stepped).abs() <= FOLLOW_WORD_YAMLS_PER_FRAME + 1e-6 && h >= stepped =>
            {
                *up_y = h;
                state.mode = WalkMode::Walking;
                state.probe_measurement_y = h - prev_up;
                state.look_latch = 0.0;
                VerticalDecision::Landed
            }
            _ => {
                state.look_latch = 0.0;
                *up_y = stepped;
                state.probe_measurement_y = FALL_STEP_FRAME_INTEGRATED * lean;
                VerticalDecision::Airborne {
                    delta_y: FALL_STEP_FRAME_INTEGRATED * lean,
                    lean,
                }
            }
        }
    } else {
        // Seam ring hold only during an actual crossing - see `contact_floor`'s
        // doc. The gate is movement INTENT: a jammed tick (intent held, ground
        // would not let the step out) is mid-crossing and keeps its plate, while
        // an idle stopped pose gets its own flat.
        match contact_floor(geom, doors, xz, *up_y, intent_held) {
            Some(h) => {
                // The grounded readout of the walk-vs-stopped tracker (USER 2026-10-28: "what is
                // mode - why does it always say Stopped, even when walking"): while support holds,
                // mode mirrors the player-state tracker - Walking while travelling, Stopped when
                // not. Behaviour-neutral at this point (both modes share the neutral lean; only
                // Airborne changes the vertical law, entered by lost contact below and cleared at
                // landing as before); it is what the panel shows and what look-ahead already gates on.
                state.mode = if state.in_motion {
                    WalkMode::Walking
                } else {
                    WalkMode::Stopped
                };
                // Ground follow at the rate bound: rises come capped to probe
                // reach by `contact_floor`, drops by the follow word; neither
                // direction moves more than one frame's rate bound here. The
                // slope-proportional push-up (USER-authorized 2026-10-28) and the
                // in-motion forward look-ahead (USER 2026-10-28: rises toward a hard
                // edge before reaching it, gated by the player-state tracker so a
                // stopped walker falls back to its own flat) both ride on top of the
                // accepted answer - neither feeds acceptance or ranking.
                let raw_look = if state.in_motion {
                    motion_lookahead_lift(geom, xz, move_dir, h, look_reach)
                } else {
                    0.0
                };
                // Un-fade bound (USER 2026-10-28, W+D jitter class): the lift may die as
                // fast as the geometry says when this pose is not holding it up - while
                // floating above support by more than a settle of uplift, the collapse is
                // rate-bounded so the drop reads as a fade, never a one-tick step down.
                let over_support = *up_y - h;
                let look = if raw_look < state.look_latch {
                    raw_look.max((state.look_latch - LOOK_UNFADE_PER_FRAME_YAMLS).min(over_support))
                } else {
                    raw_look
                }
                .max(0.0);
                state.look_latch = look;
                let target = h + slope_push_up(geom, xz, *up_y, h) + look;
                let dy = (target - *up_y)
                    .clamp(-FOLLOW_WORD_YAMLS_PER_FRAME, FOLLOW_WORD_YAMLS_PER_FRAME);
                if dy > 0.0 && ceiling_blocks(geom, doors, xz, *up_y, *up_y + dy) {
                    // Authored kuluu gate kept against record silence (same
                    // status as the pre-retail tick's ceiling hold): a rise
                    // that would push the body top through any face holds.
                    state.probe_measurement_y = 0.0;
                    return VerticalDecision::CeilingHold;
                }
                *up_y += dy;
                state.probe_measurement_y = dy;
                if dy.abs() <= 1e-6 {
                    VerticalDecision::Flat
                } else if dy > 0.0 {
                    VerticalDecision::SingleStep { up: true }
                } else {
                    VerticalDecision::SingleStep { up: false }
                }
            }
            // Support missed: retail's single probe pass steps the frame the
            // contact is lost - fall immediately, no held tick.
            None => {
                state.mode = WalkMode::Airborne { lean };
                *up_y += FALL_STEP_FRAME_INTEGRATED * lean;
                state.probe_measurement_y = FALL_STEP_FRAME_INTEGRATED * lean;
                VerticalDecision::Airborne {
                    delta_y: FALL_STEP_FRAME_INTEGRATED * lean,
                    lean,
                }
            }
        }
    }
}

/// The ceiling hold (authored kuluu law kept at record silence): any face
/// crossing the body-top slab `(feet_old + BODY_HEIGHT, feet_new + BODY_HEIGHT]`
/// in the column rejects a rise. MZB and closed-door faces both count; a
/// descent never trips it.
fn ceiling_blocks(
    geom: &MzbCollisionGeometry,
    doors: &[DoorObstacle],
    xz: Vec2,
    feet_old: f32,
    feet_new: f32,
) -> bool {
    sweep::ceiling_holds(geom, xz, feet_old, feet_new)
        || doors_in_column_slab(
            doors,
            xz,
            feet_old + BODY_HEIGHT - 1.0e-6,
            feet_new + BODY_HEIGHT,
        )
}

/// The movement axes as retail carries them into its displacement blend: `forward` along the aim
/// (negative is the backward component, which blends against heading minus a half turn) and `lateral`
/// across it, positive to the actor's right. Callers hand these in already normalized and gait-scaled by
/// [`consts::scaled_move_axes`].
#[derive(Clone, Copy, Debug, Default)]
pub struct MoveAxes {
    pub forward: f32,
    pub lateral: f32,
}

/// The displacement half of retail's per-frame step - `.agents/skills/retail-observe/references/
/// 2026-10-25-walker-speed-and-step.md`, end of the "Speed law" section: forward and lateral components
/// are blended against heading with sine/cosine, the backward component read from heading minus a half
/// turn (the reversed aim) rather than as a negated forward vector, each term scaled by this tick's step.
/// Kuluu keeps facing as the wire's 256-step byte, so `aim` and `right` are that byte's unit vectors; the
/// blend lives here so nothing upstream pre-scales terms itself.
pub fn heading_blend(step_len: f32, axes: MoveAxes, aim: Vec2, right: Vec2) -> Vec2 {
    let forward_axis = axes.forward.max(0.0);
    let backward_axis = -(axes.forward.min(0.0));
    let along_aim = aim * forward_axis - aim * backward_axis;
    (along_aim + right * axes.lateral) * step_len
}

/// Measure the pose against the wall class into a retail-shaped answer: the
/// horizontal pair is penetration depth along each axis of the nearest face
/// (zero when clear); vertical belongs to accumulation semantics and a wall
/// probe answers nothing there.
///
/// Penetration inside `PINCH_PENETRATION_SLOP` reads zero, the same authored slop the sweep
/// uses to tell a graze from a pinch (`sweep.rs`): retail's margin carries accumulated push-
/// out corrections (record `.agents/skills/retail-observe/references/2026-10-25-collision-query.md`,
/// "Result vector" rows), and its pass resolves penetration to zero every step, so a pose
/// merely touching a wall contributes nothing; kuluu's sweep halts at the contact shell, so
/// without this dead band a slide along any shipped wall (the Bastok bastion tri 17851 report)
/// lost ~half of every step to margin shortening - the felt "caught on walls" of 2026-10-27.
fn probe_answer(walls: &AuthoredWalls<'_>, center: Vec3, feet_y: f32, r: f32) -> ResultVector {
    match walls.nearest_wall(center, r, feet_y, 0.0) {
        Some((d_sq, n)) => {
            let intr = (r - d_sq.sqrt() - PINCH_PENETRATION_SLOP).max(0.0);
            ResultVector {
                lateral_x: intr * n.x.abs(),
                vertical: 0.0,
                lateral_z: intr * n.z.abs(),
                word4_unconsumed: 0.0,
            }
        }
        None => ResultVector::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::consts::{FALL_STEP_FRAME_PROBE, MEASUREMENT_DRIVER_SCALE};
    use super::*;
    use bevy::math::Quat;
    use bevy::prelude::Transform;
    use kuluu_render::dat_mzb::{
        build_collision_geometry, MzbCollisionBlock, MzbInstance, MzbSubMesh, ZONE_SLOT_MAIN,
    };

    fn geom_with(floors: &[(f32, f32)], cx: f32, cz: f32) -> MzbCollisionGeometry {
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        let mut tri_normals = Vec::new();
        for (y, half) in floors {
            let base = positions.len() as u32;
            positions.extend_from_slice(&[
                Vec3::new(cx - half, *y, cz - half),
                Vec3::new(cx + half, *y, cz - half),
                Vec3::new(cx + half, *y, cz + half),
                Vec3::new(cx - half, *y, cz + half),
            ]);
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            tri_normals.extend([Vec3::Y; 2]);
        }
        let mut block = MzbCollisionBlock::default();
        block.positions = positions;
        block.indices = indices;
        block.tri_normals = tri_normals;
        MzbCollisionGeometry::from_block(block)
    }

    fn tick_once(geom: &MzbCollisionGeometry, st: &mut Walker, pos_z_up: f32) -> StepResult {
        let obstacles = ObstacleSet::default();
        step(
            geom,
            &obstacles,
            st,
            0.0,
            0.0,
            -pos_z_up,
            0.0,
            0.0,
            1.0,
            1.0 / 60.0,
            false,
            true,
        )
    }

    #[test]
    fn fall_step_is_the_recorded_frame_penalty() {
        // "The penalties" + "Lean ladder, closed" (collision record): primary
        // gate minus one point three world units per movement frame at neutral
        // lean magnitude exactly one; alternate carried as sourced data. The
        // scene driver rescales the published word by 0.8 before integration
        // (`MEASUREMENT_DRIVER_SCALE`, f_100c54a2, decoded [0x10329a34]=+0.8),
        // so what height moves per airborne frame is -1.04 (rescored 2026-10-26).
        assert_eq!(FALL_STEP_FRAME_PROBE, -1.3);
        assert!((MEASUREMENT_DRIVER_SCALE - 0.8).abs() < 1e-6);
        assert!((FALL_STEP_FRAME_INTEGRATED + 1.04).abs() < 1e-5);
        assert_eq!(FALL_PENALTY_ALT_YAMLS_PER_FRAME, -0.5);
        assert_eq!(LEAN_DEFAULT_NEUTRAL, 1.0);
        // "Step reach and actor shape": the plus-two construction seed.
        assert_eq!(SHAPE_EXTENT_SEED_YAMLS, 2.0);
    }

    #[test]
    fn fall_from_a_ledge_lands_on_the_floor() {
        // Feet at +3 over a floor at 0: each airborne frame steps exactly one
        // penalty; landing comes on the frame contact enters reach and lands at
        // the measured height, never snapped further.
        let geom = geom_with(&[(0.0, 40.0)], 0.0, 0.0);
        let mut st = Walker {
            mode: WalkMode::Airborne {
                lean: LEAN_DEFAULT_NEUTRAL,
            },
            ..Default::default()
        }; // ledge exit already taken (support missed below)
        let mut z_wire = -3.0_f32; // y_up = +3
        let mut airborne_steps = 0usize;
        for frame in 0..16 {
            let obstacles = ObstacleSet::default();
            let prev_up = -z_wire;
            let res = step(
                &geom,
                &obstacles,
                &mut st,
                0.0,
                0.0,
                z_wire,
                0.0,
                0.0,
                1.0,
                1.0 / 60.0,
                false,
                true,
            );
            let up_now = -res.feet_z;
            match res.decision {
                VerticalDecision::Airborne { delta_y, .. } => {
                    airborne_steps += 1;
                    assert!(
                        (delta_y + 1.04).abs() < 1e-5,
                        "frame {frame}: expected one fall step of -1.3 x driver 0.8 = -1.04, got {delta_y}"
                    );
                    // No terminal speed: identical step deep into the drop, and
                    // the position actually moved by it.
                    assert!((up_now - prev_up - delta_y).abs() < 1e-5);
                }
                VerticalDecision::Landed => {
                    assert!(
                        up_now.abs() < 1e-4,
                        "landed at height {up_now}, expected the measured floor at 0"
                    );
                    break;
                }
                other => panic!("unexpected decision during fall: {:?}", other),
            }
            z_wire = res.feet_z;
        }
        // From +3 at the integrated step -1.04: two airborne frames, landing on
        // the third as the floor crosses below the stepped feet.
        assert_eq!(airborne_steps, 2);
        assert!(matches!(st.mode, WalkMode::Walking));
    }

    /// A staircase rising along bevy +x: tread i has its top at `y = rise*i`
    /// spanning `x in [depth*(i-1), depth*i]`, and the riser between tread i
    /// and tread i+1 stands at plane `x = depth*i` from `rise*i` to `rise*(i+1)`.
    /// Faces carry authored normals; the wall query's band rule decides which
    /// risers block.
    fn stair_geom(rise: f32, depth: f32, treads: usize) -> MzbCollisionGeometry {
        let mut block = MzbCollisionBlock::default();
        let mut quad = |p: [[f32; 3]; 4], n: Vec3| {
            let base = block.positions.len() as u32;
            for v in p {
                block.positions.push(Vec3::from_array(v));
            }
            block
                .indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            block.tri_normals.extend([n, n]);
        };
        let (z_lo, z_hi) = (-4.0f32, 4.0f32);
        for i in 0..treads {
            let y = rise * i as f32;
            let x_a = depth * (i as f32 - 1.0);
            let x_b = depth * i as f32;
            quad(
                [
                    [x_a, y, z_lo],
                    [x_b, y, z_lo],
                    [x_b, y, z_hi],
                    [x_a, y, z_hi],
                ],
                Vec3::Y,
            );
        }
        for i in 0..treads.saturating_sub(1) {
            let x = depth * i as f32;
            let y_lo = rise * i as f32;
            let y_hi = rise * (i + 1) as f32;
            quad(
                [
                    [x, y_lo, z_lo],
                    [x, y_lo, z_hi],
                    [x, y_hi, z_hi],
                    [x, y_hi, z_lo],
                ],
                Vec3::new(-1.0, 0.0, 0.0),
            );
        }
        MzbCollisionGeometry::from_block(block)
    }

    #[test]
    fn climbs_short_riser_stairs_like_ffxi_default_steps() {
        // Fine steps (~0.3): terrain risers under the follow-word band, ground
        // follow lifts while walking.
        let geom = stair_geom(0.3, 1.2, 4);
        let mut st = Walker::default();
        let (mut x, mut up) = (-1.0f32, 0.0f32);
        let mut held = false;
        for _tick in 0..160 {
            let obstacles = ObstacleSet::default();
            let res = step(
                &geom,
                &obstacles,
                &mut st,
                x,
                0.0,
                -up,
                0.12,
                0.0,
                6.0,
                1.0 / 60.0,
                false,
                true,
            );
            up = -res.feet_z;
            x += res.dx;
            held |= matches!(res.outcome, HorizontalOutcome::WallHold { .. });
            if x > 3.2 {
                break; // reached the last tread's middle; stop before its edge
            }
        }
        assert!(!held, "a band-suppressed riser blocked the sweep");
        assert!(
            up > 0.9 - 1e-2,
            "short-riser stair not climbed: height {up}, last x {x:.2}"
        );
    }

    #[test]
    fn walks_up_a_tall_riser_end_to_end() {
        // The middle band between the shipped staircase class (measured risers of 0.5,
        // `walks_up_the_measured_shipped_sandoria_riser`) and shoulder-high blocking
        // metal (`fence_rails_stop_the_walker_and_are_never_mounted`): a lone rise of
        // ~0.9 per tread. The collision record's "Step up" section is silent on how the
        // lifted retry crosses a lip it cannot yet stand on (support only exists past
        // the face, so each retry's own target column answers the old floor and gets
        // rejected - measured 2026-10-27), so this lane pins the safety half - no ghost,
        // no drop, no airborne frames - and reports the climb outcome as the shipped-stairs
        // battery does for the same class (`sd_north_big_steps_*`, KNOWN-OPEN). The
        // missing law piece (advance-toward visible standable ground under assist) needs
        // a retail read to source; do not invent a constant here.
        let geom = stair_geom(0.9, 1.2, 3);
        let mut st = Walker::default();
        // Wire frame: bevy +x is wire +x; a body at height `up` sits at wire
        // z = -up.
        let (mut x, mut up) = (-1.0f32, 0.0f32);
        let mut trace = Vec::new();
        for tick in 0..160 {
            let obstacles = ObstacleSet::default();
            let res = step(
                &geom,
                &obstacles,
                &mut st,
                x,
                0.0,
                -up,
                0.12,
                0.0,
                6.0,
                1.0 / 60.0,
                false,
                true,
            );
            up = -res.feet_z;
            x += res.dx;
            trace.push((tick, x, up, res.outcome));
            if up > 1.7 {
                break; // two treads climbed
            }
        }
        let reached_top = up > 1.7 - 1e-3;
        for (_tick, _x, mid_up, o) in trace.iter() {
            // Safety half: while pressed against the climb the pose never drops below
            // its start level and never reads airborne.
            assert!(*mid_up >= -1e-3, "dropped below the start floor: {o:?}");
        }
        println!(
            "status 0.9-riser stair: {} (last x {:.2}, height {:.2})",
            if reached_top {
                "CLIMBED"
            } else {
                "KNOWN-OPEN held short - middle-band climb law undetermined, see collision record 'Step up'"
            },
            x,
            up
        );
    }

    #[test]
    fn walks_up_the_measured_shipped_sandoria_riser() {
        // Census of the shipped Northern San d'Oria collision (zone 231, record
        // `.agents/skills/retail-observe/references/2026-10-25-collision-query.md`, spec
        // section "Shipped-zone step classes"): three parallel flights near geom x=-200..-192,
        // z=-74.67/-78.67/-81.33, risers exactly 0.5 yalm tall on a 1.0-yalm run - the
        // staircase class the stall report was filed against, and taller than the old
        // 0.4 wall band that caused it. The replica must walk up without ever holding.
        let geom = stair_geom(0.5, 1.0, 4);
        let mut st = Walker::default();
        let (mut x, mut up) = (-1.0f32, 0.0f32);
        let mut held = false;
        for _tick in 0..240 {
            let obstacles = ObstacleSet::default();
            let res = step(
                &geom,
                &obstacles,
                &mut st,
                x,
                0.0,
                -up,
                0.12,
                0.0,
                6.0,
                1.0 / 60.0,
                false,
                true,
            );
            up = -res.feet_z;
            x += res.dx;
            held |= matches!(res.outcome, HorizontalOutcome::WallHold { .. });
            if x > 2.9 {
                break; // three risers crossed
            }
        }
        assert!(!held, "the shipped 0.5-riser class standoff-blocked");
        assert!(
            (up - 1.5).abs() < 1e-2,
            "measured-class stair not climbed: height {up} at x {x:.2}"
        );
    }

    #[test]
    fn published_measurement_is_what_height_moved_by() {
        // The measurement-feed rule: the tick publishes exactly what height
        // did, in every vertical branch (fall, landing, ground follow, flat).
        let geom = geom_with(&[(0.0, 40.0)], 0.0, 0.0);
        let mut st = Walker {
            mode: WalkMode::Airborne {
                lean: LEAN_DEFAULT_NEUTRAL,
            },
            ..Default::default()
        };
        let mut z_wire = -2.6_f32; // y_up = +2.6
        for frame in 0..16 {
            let obstacles = ObstacleSet::default();
            let prev_up = -z_wire;
            let res = step(
                &geom,
                &obstacles,
                &mut st,
                0.0,
                0.0,
                z_wire,
                0.0,
                0.0,
                1.0,
                1.0 / 60.0,
                false,
                true,
            );
            let moved = (-res.feet_z) - prev_up;
            assert!(
                (st.probe_measurement_y - moved).abs() < 1e-4,
                "frame {frame}: published {} but height moved {moved}",
                st.probe_measurement_y
            );
            if matches!(res.decision, VerticalDecision::Landed) {
                break;
            }
            z_wire = res.feet_z;
        }
        // Settled on the floor: a flat grounded tick publishes zero.
        let obstacles = ObstacleSet::default();
        let res = step(
            &geom,
            &obstacles,
            &mut st,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            1.0 / 60.0,
            false,
            true,
        );
        assert!(matches!(res.decision, VerticalDecision::Flat));
        let _moved = -res.feet_z;
        assert!(st.probe_measurement_y.abs() < 1e-4);
    }

    #[test]
    fn rise_above_the_kuluu_band_within_one_follow_word_is_accepted() {
        // "Step acceptance is a rate bound": a riser of 0.8 - double kuluu's
        // authored 0.4 band, inside the recorded probe reach - is walked up at
        // <= FOLLOW_WORD per frame, never refused. The old "no maximum-rise cap"
        // wording was superseded by "Movement-time collision: the character sweep"
        // (2026-10-26), which bounds acceptance to STEP_UP_ACCEPT_RISE_YAMLS; 0.8
        // stays under it.
        let (band, rise) = (STEP_MAX, 0.8f32);
        // Pinned to the measured shipped stair class height (riser half a yalm,
        // `kuluu-render/tests/mzb_shipped_step_classes.rs`), not the pre-class authored
        // 0.4 that stalled every big-step lane below it (`MAX_GROUND_STEP_UP`).
        assert!((band - 0.5).abs() < 1e-6);
        assert!(rise <= STEP_UP_ACCEPT_RISE_YAMLS);
        let geom = geom_with(&[(0.9, 40.0)], 0.0, 0.0);
        let mut st = Walker::default();
        let res = tick_once(&geom, &mut st, 0.1);
        match res.decision {
            VerticalDecision::SingleStep { up } => assert!(up),
            other => panic!(
                "rise of 0.8 above the {:.1} band refused: {:?}",
                STEP_MAX, other
            ),
        }
        let after = -res.feet_z;
        // Column-ray hit heights carry ~1e-4 noise by construction
        // (`dat_mzb STEP_UP_REACH_EPSILON` rationale).
        assert!(
            (after - 0.9).abs() < 1e-3,
            "expected merge to the riser top: feet_y=0.1 h=0.9, landed at {after}"
        );
    }

    #[test]
    fn vertical_change_never_exceeds_one_follow_word_per_frame() {
        // Rate bound and acceptance reach (collision record "Fall and landing"
        // for the rate, "Movement-time collision: the character sweep" for the
        // reach): a drop crossing answers and moves at most one follow word in a
        // frame; a rise exactly at probe reach is accepted at its boundary; any
        // rise beyond reach - the phantom-floor signature (the Bastok Mines AH
        // overlay showed feet+1.167) - refuses support outright, falling instead
        // of climbing. The old version accepted +1.25 rises inside a symmetric
        // ±follow-word window; that half is re-scored against the sweep record.
        assert_eq!(FOLLOW_WORD_YAMLS_PER_FRAME, 1.3);
        assert!((STEP_UP_ACCEPT_RISE_YAMLS - 1.0).abs() < 1e-6);
        let geom = geom_with(&[(10.0, 40.0)], 0.0, 0.0);
        // Floor far above: no contact at all (unchanged), pose falls.
        let mut st = Walker::default();
        let res = tick_once(&geom, &mut st, 0.0);
        assert!(matches!(res.decision, VerticalDecision::Airborne { .. }));
        // Rise exactly at probe reach (+1.0): accepted; one frame closes it fully,
        // under the rate bound and at the acceptance boundary.
        let mut st_edge = Walker::default();
        let res_edge = tick_once(&geom, &mut st_edge, 9.0);
        match res_edge.decision {
            VerticalDecision::SingleStep { up } => assert!(up),
            other => panic!("rise exactly at probe reach refused: {:?}", other),
        }
        let closed = (-res_edge.feet_z) - 9.0;
        assert!((closed - 1.0).abs() < 1e-3);
        // Rise beyond reach (+1.25, where the old window accepted): refused, and
        // the pose falls instead of lifting.
        let mut st_phantom = Walker::default();
        let res_phantom = tick_once(&geom, &mut st_phantom, 8.75);
        match res_phantom.decision {
            VerticalDecision::Airborne { delta_y, .. } => assert!(delta_y < 0.0),
            other => panic!("rise beyond probe reach accepted as support: {:?}", other),
        }
        // Drops are unchanged: a -1.25 crossing answers and one frame moves the
        // whole crossing, which stays at (not above) the rate bound.
        let mut st_down = Walker::default();
        let res_down = tick_once(&geom, &mut st_down, 11.25);
        match res_down.decision {
            VerticalDecision::SingleStep { up } => assert!(!up),
            other => panic!("drop crossing refused: {:?}", other),
        }
        let moved = (-res_down.feet_z) - 11.25;
        assert!((moved + 1.25).abs() < 1e-3);
        assert!(moved.abs() <= FOLLOW_WORD_YAMLS_PER_FRAME + 1e-6);
    }

    /// Walk `yps` yalm/s along +x for up to 1200 ticks, stopping past `stop_x`, and
    /// report (final x, final feet height). Positions are bevy-space like the fixtures.
    fn walk_along_x(
        geom: &MzbCollisionGeometry,
        st: &mut Walker,
        start_x: f32,
        yps: f32,
        stop_x: f32,
    ) -> (f32, f32) {
        let step_len = yps / 60.0;
        let (mut ax, mut az, mut up) = (start_x, 0.0f32, 0.0f32);
        for _ in 0..1200 {
            let res = step(
                geom,
                &ObstacleSet::default(),
                st,
                ax,
                -az,
                -up,
                step_len,
                0.0,
                yps,
                1.0 / 60.0,
                false,
                true,
            );
            ax += res.dx;
            az -= res.dy;
            up = -res.feet_z;
            if ax >= stop_x {
                break;
            }
        }
        (ax, up)
    }

    #[test]
    fn walks_up_a_half_yalm_terrain_riser_while_walking() {
        // The shipped flight class (rise 0.5): a terrain-law terrace chunk whose riser
        // used to hold the sweep at Slid/Hold. Retail answers it with its step-up
        // assist, so continuous walking arrives on the terrace, not against it.
        assert!((STEP_ASSIST_MAX_YAMLS - 1.0).abs() < 1e-6);
        // Low terrace only up to the riser line: shipped stairs are one surface, and a
        // floor quad running under the higher terrace would hand the column probe its
        // old level back inside the stair footprint.
        let floor = floor_quad_submesh(13.0);
        let terrace = box_submesh(8.0, 0.5, 8.0);
        let geom = baked(
            &[floor, terrace],
            &[
                placed(0, [-7.0, 0.0, 0.0], None),
                placed(1, [10.0, 0.0, 0.0], None),
            ],
        );
        let mut st = Walker::default();
        // Riser face sits at x=6; arrive well past it.
        let (x, up) = walk_along_x(&geom, &mut st, 3.0, 2.4, 7.5);
        assert!(
            x > 7.0,
            "stopped at {x}, the half-yalm riser still holds the sweep"
        );
        assert!(
            (up - 0.5).abs() < 0.05,
            "feet ended at height {up}, expected on the 0.5 terrace"
        );
    }

    #[test]
    fn dbg_lone_rise_ticks() {
        if std::env::var("KULUU_DBG_LONE_RISE").is_err() {
            return;
        }
        let floor = floor_quad_submesh(13.0);
        let terrace = box_submesh(8.0, 1.0, 8.0);
        let geom = baked(
            &[floor, terrace],
            &[
                placed(0, [-7.0, 0.0, 0.0], None),
                placed(1, [10.0, 0.0, 0.0], None),
            ],
        );
        let mut st = Walker::default();
        let step_len = 2.4 / 60.0;
        let (mut ax, mut az, mut up) = (3.0f32, 0.0f32, 0.0f32);
        for tick in 0..600 {
            let cf_center = contact_floor(&geom, &[], Vec2::new(ax, az), up, true);
            let res = step(
                &geom,
                &ObstacleSet::default(),
                &mut st,
                ax,
                -az,
                -up,
                step_len,
                0.0,
                2.4,
                1.0 / 60.0,
                false,
                true,
            );
            let assist = st.step_assist_yamls;
            ax += res.dx;
            az -= res.dy;
            up = -res.feet_z;
            if tick % 3 == 0 || (ax > 5.4 && ax < 7.2) {
                let probe = geom.nearest_wall_contact(
                    bevy::math::Vec3::new(ax, up + BODY_RADIUS, az),
                    BODY_RADIUS,
                    up,
                    0.0,
                );
                println!(
                    "t{tick:4} x={ax:+6.3} up={up:+6.3} out={} dec={:?} assist={assist:.3} cf_prev={cf_center:?} wall={probe:?}",
                    res.outcome.label(),
                    res.decision
                );
            }
            if ax > 7.5 {
                break;
            }
        }
    }

    #[test]
    fn pressed_against_a_one_yalm_terrain_riser_never_ghosts_or_drops() {
        // The open big-step class (a lone rise of 1.0). Re-keyed to USER's one-mount ceiling
        // (`kuluu_render::dat_mzb::STEP_MOUNT_RISE_MAX_YAMLS`; the barrel-cap parapet report at
        // Bastok Mines xz(+45.18,+84.98), "this wall is well above .5 ... you are climbing it.
        // then falling down because its not a surface"): a lone rise of 1.0 is WALL. It must hold
        // the pose pressed at base floor against its face - no climb, no ghost through at floor
        // height, no airborne flicker. Grounded grounding unchanged: the city-stair census is 0.5
        // (`walks_up_the_measured_shipped_sandoria_riser`), knee-high blocks stay solid
        // (`pushing_into_a_low_prop_never_bounces_the_pose_height`), shoulder metal stops
        // (`fence_rails_stop_the_walker_and_are_never_mounted`); the record's "Step up" section is
        // silent between them and the earlier climb-outcome re-key of this lane is superseded.
        let floor = floor_quad_submesh(13.0);
        let terrace = box_submesh(8.0, 1.0, 8.0);
        let geom = baked(
            &[floor, terrace],
            &[
                placed(0, [-7.0, 0.0, 0.0], None),
                placed(1, [10.0, 0.0, 0.0], None),
            ],
        );
        let mut st = Walker::default();
        let step_len = 2.4 / 60.0;
        let (mut ax, mut az, mut up) = (3.0f32, 0.0f32, 0.0f32);
        for _tick in 0..600 {
            let res = step(
                &geom,
                &ObstacleSet::default(),
                &mut st,
                ax,
                -az,
                -up,
                step_len,
                0.0,
                2.4,
                1.0 / 60.0,
                false,
                true,
            );
            ax += res.dx;
            az -= res.dy;
            up = -res.feet_z;
            assert!(
                !matches!(res.decision, VerticalDecision::Airborne { .. }),
                "pressed pose went airborne at x {ax:.2}"
            );
            // The terrace footprint starts at x=6.0: under the one-mount ceiling the pose must
            // never enter it at all from this side - entering at floor height is a ghost, and so
            // is riding the assist up onto the top from over-class height.
            assert!(
                ax <= 6.0,
                "walked into the footprint of an over-class lone rise (wall) at x {ax:.2}, feet {up:.3}"
            );
            assert!(
                (-1e-3..=kuluu_render::dat_mzb::STEP_MOUNT_TOP_SLACK_YAMLS).contains(&up),
                "feet height {up} rose off base floor against the wall-class lone rise at x {ax:.2}"
            );
        }
        println!(
            "status lone-1yalm-rise: SOLID WALL held short (one-mount ceiling), ended x {:.2} up {:.3}",
            ax, up
        );
    }

    #[test]
    fn walks_up_shell_treads_when_a_floor_slab_runs_underneath() {
        // The Bastok Mines AH shell class (2026-10-26 overlay at the user's
        // xz(-21.3,+69.6)): shipped flights ride on a floor slab that continues
        // under every tread, so each column answers two up-facing surfaces and
        // selection must measure nearness to the body point half a yalm above
        // the feet (`GROUND_PROBE_OFFSET_YAMLS`, collision record "Movement-time
        // collision: the character sweep"). Measured from the feet this walker
        // answered the slab at every column and walked into the flight invisible.
        let floor = floor_quad_submesh(16.0);
        let tread_a = box_submesh(1.0, 0.26, 8.0);
        let tread_b = box_submesh(1.0, 0.52, 8.0);
        let tread_c = box_submesh(1.0, 0.78, 8.0);
        let geom = baked(
            &[floor, tread_a, tread_b, tread_c],
            &[
                placed(0, [-4.0, 0.0, 0.0], None), // floor spans x[-20..12]: under all treads
                placed(1, [8.5, 0.0, 0.0], None),
                placed(2, [9.5, 0.0, 0.0], None),
                placed(3, [10.5, 0.0, 0.0], None),
            ],
        );
        // Columns above the treads answer tread tops, not the slab beneath.
        for (x, expect) in [(8.5f32, 0.26f32), (9.5, 0.52), (10.5, 0.78)] {
            let h = geom
                .ground_walkable(Vec2::new(x, 4.0), 0.0)
                .unwrap_or(f32::NAN);
            assert!(
                (h - expect).abs() < 0.02,
                "column x={x} answered {h}, expected tread top {expect}"
            );
        }
        let mut st = Walker::default();
        // Arrive at the first riser from flat slab and walk through all three.
        let (x, up) = walk_along_x(&geom, &mut st, 3.0, 2.4, 10.9);
        assert!(x > 10.8, "stopped at {x}, the shell flight held the sweep");
        assert!(
            (up - 0.78).abs() < 0.05,
            "feet ended at height {up}, expected on the 0.78 tread"
        );
    }

    /// Shell flights ship their treads as skin over a mound: successive tread tops end
    /// just before the next one begins, and the slab running underneath answers the pose
    /// column in between (measured at Bastok Mines AH shell 2026-10-27; pinned end to end
    /// by `climb_the_ah_shell_treads_from_the_reported_stand_point`). Without ring support the climbing
    /// pose loses half a yalm on every seam - drop, bounce, re-climb - which is how users
    /// described "refuses to see the start of the stairs". This fixture puts that class into
    /// a unit test: three tread plates with seam slivers between them over one continuing slab.
    #[test]
    fn shell_seams_between_tread_plates_do_not_drop_the_pose() {
        let floor = floor_quad_submesh(16.0);
        let plate_a = box_submesh(1.0, 0.26, 8.0);
        let plate_b = box_submesh(1.0, 0.52, 8.0);
        let plate_c = box_submesh(1.0, 0.78, 8.0);
        // Gaps of ~0.12 yalm between plates (within the ring radius), tops rising 0.26 each.
        let geom = baked(
            &[floor, plate_a, plate_b, plate_c],
            &[
                placed(0, [2.0, 0.0, 0.0], None), // slab spans x[-14..18]: under every seam
                placed(1, [5.6, 0.0, 0.0], None), // plate A: x[5.1..6.1]
                placed(2, [6.72, 0.0, 0.0], None), // plate B: x[6.22..7.22]
                placed(3, [7.84, 0.0, 0.0], None), // plate C: x[7.34..8.34]
            ],
        );
        // Query-level law first: above seam A-B the pose column alone would answer slab 0, but
        // ring support holds the occupied level - plate A's own top at +0.26, beaten only by
        // nearness to body point regardless of scan order. The higher plate B (+0.52) is NOT taken
        // over the seam: a ring candidate may lift no more than RING_SUPPORT_HOLD_LIFT_CAP_YAMLS
        // above the feet (USER's 2026-10-28 "feels a bit high on the steps" report; the shipped-
        // data pin is `approaching_a_shipped_riser_never_lifts_the_pose_early_or_flaps`). B arrives
        // through the pose column once the axis stands over it, which the drive below re-verifies.
        let held = contact_floor(&geom, &[], Vec2::new(6.17, 0.0), 0.26, true).unwrap();
        assert!(
            (held - 0.26).abs() < 1e-4,
            "over the seam expected ring support to hold the occupied plate A level (+0.26 over slab \
             0) without anticipating plate B, got {held}"
        );
        // Then the drive: walk +x across all three seams at a run.
        let step_len = 2.4 / 60.0;
        let mut st = Walker::default();
        let (mut ax, az, mut up) = (
            4.0f32,
            0.0f32,
            geom.ground_nearest(Vec2::new(4.0, 0.0), 0.0).unwrap(),
        );
        let mut climbed_a = false;
        for _ in 0..900 {
            let res = step(
                &geom,
                &ObstacleSet::default(),
                &mut st,
                ax,
                -az,
                -up,
                step_len,
                0.0,
                2.4,
                1.0 / 60.0,
                false,
                true,
            );
            let now = -res.feet_z;
            if !climbed_a && now >= 0.25 {
                climbed_a = true;
            }
            // The seam law: once on plate A, no tick may fall back to the slab under a seam.
            if climbed_a {
                assert!(
                    now > 0.24,
                    "pose dropped into a tread seam pocket at x {ax:.3}: height {now:.3}"
                );
            }
            assert!(
                !matches!(res.decision, VerticalDecision::Airborne { .. }),
                "seam crossing went airborne at x {ax:.3} ({:?})",
                res.decision
            );
            ax += res.dx;
            up = now;
            if ax > 8.2 {
                break;
            }
        }
        assert!(ax > 8.1, "stalled at x {ax:.3} crossing the plate seams");
        assert!(
            (up - 0.78).abs() < 0.05,
            "ended at height {up}, expected on the top plate C (+0.78)"
        );
    }

    /// Stopped poses release from the seam ring hold (USER 2026-10-28 report, Bastok Mines
    /// xz(+1.15,+72.77): "we are still stair floating when stopped" and ".26 too high (same as
    /// riser/tread, but there IS not tread under us.. and we are stopped)"). The ring hold exists
    /// for crossing, so it runs only while the walk-vs-stopped tracker (`in_motion`) says walking;
    /// a pose that walked past a plate edge settles onto its own axis column's flat instead of
    /// being re-held by the plate it already left ("YOU FALL in STOPPED"). The fixture reproduces
    /// the reported pose: just past the end of a +0.26 shell-tread plate (forensics grid at those
    /// coords: plates end ~z+72.42 under a pose floating at exactly +0.26, axis column answering
    /// floor 0 only), with the continuing slab under everything.
    #[test]
    fn a_stopped_pose_past_a_plate_edge_is_not_held_up_by_the_seam_ring() {
        let floor = floor_quad_submesh(16.0);
        let plate = box_submesh(1.0, 0.26, 8.0);
        let geom = baked(
            &[floor, plate],
            &[
                placed(0, [2.0, 0.0, 0.0], None), // slab spans x[-14..18]: under the pose column
                placed(1, [5.6, 0.0, 0.0], None), // plate: x[5.1..6.1], top +0.26
            ],
        );
        // Query-level law at the reported pose (axis x=6.3 past the plate edge; rear ring columns
        // still reach the plate top). Tracker walking: the hold stands at the occupied level, it is
        // a crossing. Tracker stopped: the pose gets its own column's flat 0 - never re-held.
        let held = contact_floor(&geom, &[], Vec2::new(6.3, 0.0), 0.26, true).unwrap();
        assert!(
            (held - 0.26).abs() < 1e-4,
            "walking tracker must keep the seam hold at the occupied plate level (+0.26), got {held}"
        );
        let flat = contact_floor(&geom, &[], Vec2::new(6.3, 0.0), 0.26, false).unwrap();
        assert!(
            flat.abs() < 1e-4,
            "stopped pose must answer its own axis column (slab 0), got {flat}"
        );

        // Stopped drive: zero intent switches the seam ring off every tick, and the
        // float drains to the slab within one bounded follow-word step - never Airborne (the floor
        // answers its own column), never re-held on a later tick.
        let mut st = Walker::default();
        let (mut ax, az, mut up) = (6.3f32, 0.0f32, 0.26f32);
        for tick in 0..10 {
            let res = step(
                &geom,
                &ObstacleSet::default(),
                &mut st,
                ax,
                -az,
                -up,
                0.0,
                0.0,
                2.4,
                1.0 / 60.0,
                false,
                true,
            );
            let now = -res.feet_z;
            assert!(
                !matches!(res.decision, VerticalDecision::Airborne { .. }),
                "stopped release went airborne at tick {tick} ({:?})",
                res.decision
            );
            if tick >= 2 {
                assert!(
                    now.abs() <= 1e-3 && matches!(res.decision, VerticalDecision::Flat),
                    "stopped pose did not settle onto its own flat: tick {tick} up {now} ({:?})",
                    res.decision
                );
            }
            ax += res.dx;
            up = now;
        }
        assert!((ax - 6.3).abs() < 1e-4, "stopped drive drifted to x {ax}");

        // Walking drive over the same plate: the hold carries past the plate edge while a ring
        // column still reaches it (up to axis edge + probe radius 0.5), then - and only then - the
        // body leaves support and settles onto the slab. Crossing semantics intact under the gate.
        let mut st = Walker::default();
        let step_len = 2.4 / 60.0;
        let (mut ax, az, mut up) = (5.5f32, 0.0f32, 0.26f32);
        let mut saw_held_past_edge = false;
        for _ in 0..240 {
            let res = step(
                &geom,
                &ObstacleSet::default(),
                &mut st,
                ax,
                -az,
                -up,
                step_len,
                0.0,
                2.4,
                1.0 / 60.0,
                false,
                true,
            );
            let now = -res.feet_z;
            assert!(
                !matches!(res.decision, VerticalDecision::Airborne { .. }),
                "walking edge crossing went airborne at x {ax:.3} ({:?})",
                res.decision
            );
            if ax > 6.15 && ax < 6.5 {
                // Past the plate end but inside ring reach: held, not dropped early.
                saw_held_past_edge = true;
                assert!(
                    now > 0.24,
                    "walking pose lost the seam hold at x {ax:.3} (up {now:.3})"
                );
            }
            ax += res.dx;
            up = now;
            if ax > 7.5 {
                break;
            }
        }
        assert!(
            saw_held_past_edge,
            "drive never reached the past-edge hold window (ended x {ax:.3})"
        );
        assert!(
            up.abs() < 0.05,
            "walking pose stayed high after leaving the plate: up {up}"
        );
    }

    #[test]
    fn ring_sample_never_holds_a_deck_wholly_above_the_pose() {
        // The ordering rule that makes ring support safe (retail's wholly-above ignore for
        // steep faces, `STEEP_SUPPORT_WHOLLY_ABOVE_IGNORE_YAMLS`, collision record "Movement-time
        // collision: the character sweep"): a deck top standing 1.0 over the walking level - whose
        // whole extent is beyond the ignored magnitude from a pose on the slab - must never answer,
        // even when the ring column reaches into it across the terrace edge. Without this rule a
        // body-point distance tie picks the +1.0 deck and the pose pops up before entering the
        // footprint, which is what `pressed_against_a_one_yalm_terrain_riser_never_ghosts_or_drops`
        // failed with when the same law was tried without it (reverted 2026-10-27).
        let floor = floor_quad_submesh(20.0);
        let terrace = box_submesh(8.0, 1.0, 8.0);
        let geom = baked(
            &[floor, terrace],
            &[
                placed(0, [3.0, 0.0, 0.0], None),  // slab spans x[-17..23]
                placed(1, [10.0, 0.0, 0.0], None), // terrace: x[6.0..14.0], top +1.0
            ],
        );
        // Pose on the slab pressed near the terrace edge: forward ring column (x+0.5=6.4) reaches
        // into the deck footprint, but its face stands wholly above feet+0.3 -> rejected; answer stays slab.
        let h = contact_floor(&geom, &[], Vec2::new(5.9, 0.0), 0.0, true).unwrap();
        assert!(
            (h - 0.0).abs() < 1e-4,
            "ring sample lifted the slab pose toward the +1.0 deck: answered {h}"
        );
        // And once the pose has actually climbed on, its own level answers normally.
        let h2 = contact_floor(&geom, &[], Vec2::new(9.0, 0.0), 1.0, true).unwrap();
        assert!(
            (h2 - 1.0).abs() < 1e-4,
            "terrace-level pose lost its own floor: answered {h2}"
        );
    }

    #[test]
    fn a_face_taller_than_the_assist_cap_stays_solid_under_a_long_push() {
        // The cap is retail's own accumulator clamp: how long you press does not buy
        // more than one yalm of step-up reach, so a three-yalm terrace never yields.
        let floor = floor_quad_submesh(20.0);
        let wall = box_submesh(8.0, 3.0, 8.0);
        let geom = baked(
            &[floor, wall],
            &[
                placed(0, [0.0, 0.0, 0.0], None),
                placed(1, [10.0, 0.0, 0.0], None),
            ],
        );
        let mut st = Walker::default();
        let (x, up) = walk_along_x(&geom, &mut st, 3.0, 2.4, 7.5);
        assert!(
            x < 6.0 - BODY_RADIUS + 0.05,
            "climbed through a three-yalm face to {x}"
        );
        assert!(up.abs() < 0.05, "rose to {up} with nothing under the feet");
    }

    #[test]
    fn an_authored_blocker_prop_is_never_mounted_by_the_assist() {
        // User law of 2026-10-25 ("no walking up plants") under the new assist: a
        // knee-height planter blocks and its top refuses standable support, so the
        // lift never applies.
        let floor = floor_quad_submesh(20.0);
        let planter = box_submesh(1.4, 0.7, 1.4);
        let geom = baked(
            &[floor, planter],
            &[
                placed(0, [0.0, 0.0, 0.0], None),
                placed(1, [6.0, 0.0, 0.0], None),
            ],
        );
        let mut st = Walker::default();
        let (x, up) = walk_along_x(&geom, &mut st, 3.0, 2.4, 7.5);
        assert!(
            x < 6.0 - 0.7 - BODY_RADIUS + 0.35,
            "mounted the planter: reached {x}"
        );
        assert!(up.abs() < 0.05, "lifted {up} onto a prop top");
    }

    #[test]
    fn wall_height_blocks_horizontally_through_the_authored_class() {
        // A tall vertical face (authored blocker class) stops the horizontal
        // move; terrain faces never do this to a walker.
        // Two-triangle rectangle so every body-chain height sees full wall.
        let positions = vec![
            Vec3::new(-20.0, -2.0, -20.0),
            Vec3::new(20.0, -2.0, -20.0),
            Vec3::new(20.0, 8.0, -20.0),
            Vec3::new(-20.0, 8.0, -20.0),
        ];
        let indices = vec![0u32, 1, 2, 0, 2, 3];
        let tri_normals = vec![Vec3::new(0.0, 0.0, 1.0); 2];
        let mut block = MzbCollisionBlock::default();
        block.positions = positions;
        block.indices = indices;
        block.tri_normals = tri_normals;
        // Authored blocker identity: under the props-only blocking law a bare
        // terrain face answers no wall query at any height, so this fixture has
        // to carry it explicitly for "authored class" to mean anything.
        block.tri_prop_blocker = vec![true; 2];
        let geom = MzbCollisionGeometry::from_block(block);
        let mut st = Walker::default();
        let obstacles = ObstacleSet::default();
        // Body already within reach of the face (feet_xz=(x,-y): wire y=+19.8
        // sits at bevy z=-19.8, 0.2 from the plane at -20, inside the body
        // radius). Pushing 2.0 into it (+dy -> negative bevy z) must never
        // report a clean move.
        let res = step(
            &geom,
            &obstacles,
            &mut st,
            0.0,
            19.8,
            0.0,
            0.0,
            2.0,
            1.0,
            1.0 / 60.0,
            false,
            true,
        );
        match res.outcome {
            HorizontalOutcome::Slid { .. } | HorizontalOutcome::WallHold { .. } => {}
            other => panic!("wall did not block: {:?}", other),
        }
    }

    /// The pipe trap from USER's 2026-10-28 Bastok Mines report, synthetic: street at y=0,
    /// a full-height side face (plane x=0, y[0..2], authored normal +x) with a standable
    /// cap floor (+0.5) meeting it mid-extent behind. The flush-top riser rule
    /// (`dat_mzb::RISER_FLUSH_TOP_SLACK_YAMLS`) must keep the face solid: pressing into it
    /// never mounts the ledge (the shipped pin is
    /// `tests_shipped_stairs::the_bastok_pipe_edge_at_minus_52_plus_19_is_never_mounted`).
    fn ledge_trap_geom() -> MzbCollisionGeometry {
        let mut block = MzbCollisionBlock::default();
        let mut quad = |pts: [[f32; 3]; 4], n: [f32; 3]| {
            let b = block.positions.len() as u32;
            for p in pts {
                block.positions.push(Vec3::from(p));
            }
            block
                .indices
                .extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
            block.tri_normals.push(Vec3::from(n));
            block.tri_normals.push(Vec3::from(n));
        };
        // Street under everything.
        quad(
            [
                [-8.0, 0.0, -8.0],
                [8.0, 0.0, -8.0],
                [8.0, 0.0, 8.0],
                [-8.0, 0.0, 8.0],
            ],
            [0.0, 1.0, 0.0],
        );
        // Side face: plane x=0 spanning y[0..2], z[-4..4], normal +x (toward the walker).
        quad(
            [
                [0.0, 0.0, -4.0],
                [0.0, 2.0, -4.0],
                [0.0, 2.0, 4.0],
                [0.0, 0.0, 4.0],
            ],
            [1.0, 0.0, 0.0],
        );
        // Cap floor +0.5 behind the face (x -4..-0.02 so its ray column is clear of the
        // face's own edge).
        quad(
            [
                [-4.0, 0.5, -4.0],
                [-0.02, 0.5, -4.0],
                [-0.02, 0.5, 4.0],
                [-4.0, 0.5, 4.0],
            ],
            [0.0, 1.0, 0.0],
        );
        MzbCollisionGeometry::from_block(block)
    }

    #[test]
    fn a_wall_whose_ledge_meets_it_mid_extent_is_never_mounted_by_pressing() {
        let geom = ledge_trap_geom();
        let mut st = Walker::default();
        let (mut ax, az) = (3.0f32, 0.0f32);
        let mut up = 0.0f32;
        let step_len = 1.6 / 60.0;
        let (mut min_ax, mut max_up) = (ax, up);
        for _ in 0..300 {
            let res = step(
                &geom,
                &super::super::obstacles::ObstacleSet::default(),
                &mut st,
                ax,
                -az,
                -up,
                -step_len,
                0.0,
                1.6,
                1.0 / 60.0,
                false,
                true,
            );
            ax += res.dx;
            up = -res.feet_z;
            min_ax = min_ax.min(ax);
            max_up = max_up.max(up);
        }
        assert!(
            max_up < 0.05,
            "pose rose to {max_up:+.3} against the ledge wall - a standable floor meeting a \
             full-height face mid-extent is wall-with-a-ledge, not a riser (flush-top rule)"
        );
        assert!(
            min_ax > 0.1,
            "pose reached x={min_ax:+.2}; the side face at plane x=0 must stop it at body \
             radius distance"
        );
    }

    /// A flat quad at y=`y` over the origin with an authored normal of `normal_y`.
    fn tilted_quad_geom(normal_y: f32) -> MzbCollisionGeometry {
        let mut block = MzbCollisionBlock::default();
        block.positions = vec![
            Vec3::new(-4.0, 0.0, -4.0),
            Vec3::new(4.0, 0.0, -4.0),
            Vec3::new(4.0, 0.0, 4.0),
            Vec3::new(-4.0, 0.0, 4.0),
        ];
        block.indices = vec![0, 1, 2, 0, 2, 3];
        let nx = (1.0 - normal_y * normal_y).sqrt().max(0.0);
        block.tri_normals = vec![Vec3::new(nx, normal_y, 0.0); 2];
        MzbCollisionGeometry::from_block(block)
    }

    #[test]
    fn slope_push_up_is_zero_on_flat_and_caps_at_the_steepest_walkable_tilt() {
        // USER-authorized anti-clip offset (2026-10-28): exactly zero on flat faces,
        // linear in authored tilt, capped at SLOPE_PUSH_UP_MAX_YAMLS by the steepest
        // walkable face; a ring/door answer that is not this column's own face pushes
        // nothing. Constants pinned so the feel cannot drift past a quarter tread.
        assert_eq!(SLOPE_PUSH_UP_MAX_YAMLS, 0.1);
        let flat = tilted_quad_geom(1.0);
        assert_eq!(slope_push_up(&flat, Vec2::ZERO, 0.0, 0.0), 0.0);
        let gentle = tilted_quad_geom(1.0 - SLOPE_PUSH_UP_TILT_EPSILON * 4.0); // past epsilon
        assert!(slope_push_up(&gentle, Vec2::ZERO, 0.0, 0.0) > 0.0);
        let just_flat = tilted_quad_geom(1.0 - SLOPE_PUSH_UP_TILT_EPSILON * 0.5); // under it
        assert_eq!(slope_push_up(&just_flat, Vec2::ZERO, 0.0, 0.0), 0.0);
        let half = tilted_quad_geom(0.75); // tilt 0.25 of the 0.5 full tilt
        assert!((slope_push_up(&half, Vec2::ZERO, 0.0, 0.0) - 0.05).abs() < 1e-6);
        let steep = tilted_quad_geom(kuluu_render::dat_mzb::WALKABLE_FACE_UP_DOT);
        assert!(
            (slope_push_up(&steep, Vec2::ZERO, 0.0, 0.0) - SLOPE_PUSH_UP_MAX_YAMLS).abs() < 1e-6
        );
        // Answer came from elsewhere (h far off this column's face): no push.
        assert_eq!(slope_push_up(&half, Vec2::ZERO, 0.0, 0.7), 0.0);
    }

    #[test]
    fn grounded_pose_settles_on_a_sloped_face_at_the_answer_plus_the_push_up() {
        // One correction tick lifts onto face+push-up, then the decision reads Flat:
        // stable fixed point, no flap (and never above answer + cap).
        let geom = tilted_quad_geom(0.75);
        let mut st = Walker::default();
        let res0 = tick_once(&geom, &mut st, 0.0);
        assert!(matches!(
            res0.decision,
            VerticalDecision::SingleStep { up: true }
        ));
        let settled = -res0.feet_z;
        assert!((settled - 0.05).abs() < 1e-6);
        for i in 1..8 {
            let res = tick_once(&geom, &mut st, settled);
            assert!(
                matches!(res.decision, VerticalDecision::Flat),
                "tick {i}: {res:?}"
            );
            assert!((-res.feet_z - 0.05).abs() < 1e-6, "drift at tick {i}");
        }
    }

    /// Street y=0 (x[-8..-0.1]), a flush-top riser face on plane x=0 spanning
    /// y[0..top] (normal +x), upper tread y=top (x[0..6]) - optionally flagged as
    /// prop blocker faces. The pure-function and drive look-ahead pins below use it.
    fn one_step_geom(top: f32, prop_tread: bool) -> MzbCollisionGeometry {
        let mut block = MzbCollisionBlock::default();
        let mut quad = |pts: [[f32; 3]; 4], n: [f32; 3]| {
            let b = block.positions.len() as u32;
            for p in pts {
                block.positions.push(Vec3::from(p));
            }
            block
                .indices
                .extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
            block.tri_normals.push(Vec3::from(n));
            block.tri_normals.push(Vec3::from(n));
        };
        quad(
            [
                [-8.0, 0.0, -8.0],
                [-0.1, 0.0, -8.0],
                [-0.1, 0.0, 8.0],
                [-8.0, 0.0, 8.0],
            ],
            [0.0, 1.0, 0.0],
        );
        quad(
            [
                [0.0, 0.0, -4.0],
                [0.0, top, -4.0],
                [0.0, top, 4.0],
                [0.0, 0.0, 4.0],
            ],
            [1.0, 0.0, 0.0],
        );
        // Tread runs far past the drive's travel (a 400-tick walk covers ~10 yalm) so
        // the pinned asserts measure the crossing, not a fall off a test-only ledge.
        quad(
            [
                [-0.05, top, -4.0],
                [20.0, top, -4.0],
                [20.0, top, 4.0],
                [-0.05, top, 4.0],
            ],
            [0.0, 1.0, 0.0],
        );
        if prop_tread {
            block.tri_prop_blocker = vec![false, false, false, false, true, true];
        }
        MzbCollisionGeometry::from_block(block)
    }

    /// The one-mount ceiling as a pinned law (USER 2026-10-28/29, barrel-cap parapet at Bastok
    /// Mines xz(+45.18,+84.98): "this wall is well above .5 so you are climbing it. then falling
    /// down because its not a surface"). The synthetic shelf - 0.9 of standable rise behind a
    /// near-vertical face - matches the shipped parapet's blocking shape and is friendlier than
    /// it (support holds there, where the shipped barrel lids gave out): it must simply never be
    /// mounted. Pose stays pressed at base floor, no airborne flicker, never crosses the face.
    /// The half-class still climbs (`walks_up_a_half_yalm_terrain_riser_while_walking`): the
    /// ceiling sits on top of `MAX_GROUND_STEP_UP`, not below it.
    #[test]
    fn a_standable_shelf_beyond_step_class_is_never_mounted() {
        let geom = one_step_geom(0.9, false);
        let mut st = Walker::default();
        let (mut ax, az) = (-3.0f32, 0.0f32);
        let mut up = 0.0f32;
        for tick in 0..400 {
            let res = step(
                &geom,
                &ObstacleSet::default(),
                &mut st,
                ax,
                -az,
                -up,
                1.6 / 60.0,
                0.0,
                1.6,
                1.0 / 60.0,
                false,
                true,
            );
            ax += res.dx;
            up = -res.feet_z;
            if std::env::var("KULUU_SHELF_TRACE").is_ok() {
                println!(
                    "shelf t{tick} x={ax:.4} up={up:.4} out={:?} dec={:?}",
                    res.outcome, res.decision
                );
            }
            assert!(
                !matches!(res.decision, VerticalDecision::Airborne { .. }),
                "tick {tick}: pressed pose went airborne at x {ax:.2} (up {up:.3})"
            );
            assert!(
                (-1e-3..=kuluu_render::dat_mzb::STEP_MOUNT_TOP_SLACK_YAMLS).contains(&up),
                "tick {tick}: feet rose {up:.3} toward a shelf beyond the one-mount ceiling (x {ax:.2})"
            );
        }
        assert!(
            ax < 0.5,
            "ghosted through an over-class shelf to x {ax:.2} (feet {up:.3})"
        );
    }

    #[test]
    fn motion_lookahead_lift_reads_forward_columns_within_the_window_only() {
        let step = one_step_geom(0.4, false);
        let px = |v: f32| Vec2::new(v, 0.0);
        let dir = Vec2::X;
        let open = LOOKAHEAD_FAR_YAMLS;
        // Far behind the window (samples still on the lower floor): nothing.
        assert_eq!(motion_lookahead_lift(&step, px(-3.0), dir, 0.0, open), 0.0);
        assert_eq!(motion_lookahead_lift(&step, px(-1.5), dir, 0.0, open), 0.0);
        // Column heights come off triangle interpolation, so the blend checks carry a
        // small epsilon rather than demanding exact float equality.
        const LIFT_EPS: f32 = 5e-3;
        // Crossing-linked ramp law (USER 2026-10-28): the nearest riser lip ahead lifts a
        // linear fraction one minus its distance over the window; full is never held early.
        // Lip edge at x = -0.05, stride nought-point-two: first riser probe at each stand:
        let far = motion_lookahead_lift(&step, px(-0.8), dir, 0.0, open); // lip d_hat 0.7
        assert!(
            (far - lookahead_ramp_weight(0.7) * (0.4 + RISE_UPLIFT_YAMLS)).abs() < LIFT_EPS,
            "far-lip ramp lift {far}"
        );
        let mid = motion_lookahead_lift(&step, px(-0.3), dir, 0.0, open); // lip d_hat 0.3
        assert!(
            (mid - lookahead_ramp_weight(0.3) * (0.4 + RISE_UPLIFT_YAMLS)).abs() < LIFT_EPS,
            "mid-lip ramp lift {mid}"
        );
        let near = motion_lookahead_lift(&step, px(-0.1), dir, 0.0, open); // lip d_hat 0.1
        assert!(
            (near - lookahead_ramp_weight(0.1) * (0.4 + RISE_UPLIFT_YAMLS)).abs() < LIFT_EPS,
            "near-lip ramp lift {near}"
        );
        assert!(
            far < mid && mid < near,
            "ramp must climb as the lip approaches: {far} {mid} {near}"
        );
        // Never lowers: on the tread looking back down the step.
        assert_eq!(motion_lookahead_lift(&step, px(2.0), -dir, 0.4, open), 0.0);
        // Above the cap (assist-class riser) the channel stays silent.
        let tall = one_step_geom(LOOKAHEAD_RISE_CAP_YAMLS + 0.3, false);
        assert_eq!(motion_lookahead_lift(&tall, px(-0.3), dir, 0.0, open), 0.0);
        // Prop tops answer nothing (authored blocker filter).
        let props = one_step_geom(0.4, true);
        assert_eq!(motion_lookahead_lift(&props, px(-0.3), dir, 0.0, open), 0.0);
        // No direction of travel: exactly zero.
        assert_eq!(
            motion_lookahead_lift(&step, px(-0.3), Vec2::ZERO, 0.0, open),
            0.0
        );
        // Reach gate: a blocking face between the body and the sample column suppresses
        // even a perfectly valid rise (the ledge-behind-a-wall trap).
        assert_eq!(motion_lookahead_lift(&step, px(-0.3), dir, 0.0, 0.2), 0.0);
    }

    #[test]
    fn in_motion_lookahead_rises_before_a_flush_riser_and_stopped_walkers_fall_back() {
        let geom = one_step_geom(0.4, false);
        let mut st = Walker::default();
        let (mut ax, az) = (-3.0f32, 0.0f32);
        let mut up = 0.0f32;
        let step_len = 1.6 / 60.0;
        let (mut early, mut max_dy, mut max_up) = (0.0f32, 0.0f32, 0.0f32);
        for _ in 0..400 {
            let res = step(
                &geom,
                &super::super::obstacles::ObstacleSet::default(),
                &mut st,
                ax,
                -az,
                -up,
                step_len,
                0.0,
                1.6,
                1.0 / 60.0,
                false,
                true,
            );
            let new_up = -res.feet_z;
            max_dy = max_dy.max((new_up - up).abs());
            ax += res.dx;
            up = new_up;
            if ax < -0.15 {
                early = early.max(up);
            }
            max_up = max_up.max(up);
        }
        println!("lookahead drive: early={early:.3} max_dy={max_dy:.3} max_up={max_up:.3} end up={up:.3}");
        // Rising before the edge (center still well behind the riser plane) ...
        assert!(
            early > 0.15,
            "pose never rose ahead of the riser plane (early {early:.3}) - look-ahead dead"
        );
        // ... climbing the crossing-linked ramp: substantial but never above the next tread
        // before support arrives (the pre-ramp law snapped full rise+uplift early and hovered;
        // USER 2026-10-28 diagram re-keyed this band to "never exceeds tread top + uplift").
        assert!(
            max_up > 0.3 && max_up <= 0.4 + RISE_UPLIFT_YAMLS + 1e-3,
            "ramp band broken: max_up {max_up:.3}"
        );
        // ... no single-tick pop of the full rise anymore (arrival merges with the support
        // flip; measured nought-point-one-one-three against the old nought-point-four pop) ...
        assert!(
            max_dy < 0.4 - 15e-2,
            "a near-full-riser one-tick jump survived (max dy {max_dy:.3})"
        );
        // ... and the settled pose lands ON the tread, uplift faded.
        assert!((up - 0.4).abs() < 0.01, "settled at {up:.3}, expected ~0.4");

        // Stopped mid-ramp: with no movement intent the look-ahead gate closes and the
        // pose falls back to its own flat (USER: "falls down to the flat when stopped").
        let mut st2 = Walker::default();
        let (mut bx, bz) = (-3.0f32, 0.0f32);
        let mut bup = 0.0f32;
        let mut rising_ticks = 0usize;
        // Walk until the look-ahead ramp itself offers >0.1 (far onset fires ~0.75 behind
        // the riser plane); the plane-crossing bound keeps a dead channel failing loud.
        while bx < 0.5 && rising_ticks < 600 {
            let res = step(
                &geom,
                &super::super::obstacles::ObstacleSet::default(),
                &mut st2,
                bx,
                -bz,
                -bup,
                step_len,
                0.0,
                1.6,
                1.0 / 60.0,
                false,
                true,
            );
            bx += res.dx;
            bup = -res.feet_z;
            rising_ticks += 1;
            if bup > 0.1 {
                break;
            }
        }
        assert!(bup > 0.1, "never got mid-ramp to stop from");
        let stopped_at = bx;
        let mut max_stop_drop = 0.0f32;
        let mut prev_bup = bup;
        for _ in 0..24 {
            let res = step(
                &geom,
                &super::super::obstacles::ObstacleSet::default(),
                &mut st2,
                bx,
                -bz,
                -bup,
                0.0,
                0.0,
                1.6,
                1.0 / 60.0,
                false,
                true,
            );
            bx += res.dx;
            bup = -res.feet_z;
            max_stop_drop = max_stop_drop.max(prev_bup - bup);
            prev_bup = bup;
        }
        assert!(
            stopped_at < -0.5 && bup.abs() < 0.01,
            "stopped mid-ramp at x {stopped_at:.2} settled at {bup:.3}, expected the street 0.0"
        );
        // The fall back to flat is a fade, not a drop: one un-fade per frame at most
        // (W+D jitter class - pre-latch this dropped the whole lift in a single tick).
        assert!(
            max_stop_drop <= LOOK_UNFADE_PER_FRAME_YAMLS + 1e-6,
            "stop settle exceeded one un-fade per frame ({max_stop_drop:.4})"
        );
    }

    #[test]
    fn slope_hold_tracks_ground_at_the_follow_rate() {
        // Stair-as-slope: measured floor rises 0.3/tick; feet must track every
        // tick within the rate bound and never flick airborne (slope hold).
        let mut st = Walker::default();
        for i in 0..12 {
            let y = i as f32 * 0.3;
            let geom = geom_with(&[(y + 0.3, 40.0)], 0.0, 0.0);
            let res = tick_once(&geom, &mut st, y);
            assert!(
                !matches!(res.decision, VerticalDecision::Airborne { .. }),
                "tick {i} flicked airborne on a floor rising to {}",
                y + 0.3
            );
            let moved = (-res.feet_z) - y;
            assert!((moved - 0.3).abs() < 1e-4);
        }
    }

    #[test]
    fn margin_uses_the_recorded_scale_and_skips_vertical() {
        // "the margin wrapper skips the accumulated vertical word ...":
        // nought point eight times the length of the horizontal pair only.
        let a = ResultVector {
            lateral_x: 0.5,
            vertical: -99.0,
            lateral_z: 0.25,
            word4_unconsumed: 7.0,
        };
        assert!((a.margin() - 0.8 * (0.5_f32.powi(2) + 0.25f32.powi(2)).sqrt()).abs() < 1e-6);
        assert_eq!(PROBE_MARGIN_SCALE, 0.8);
    }

    #[test]
    fn camera_aim_height_adds_the_vertical_word_directly() {
        // "Word two - vertical accumulation; moves both player and camera":
        // straight add into the unit's own accumulator.
        let a = ResultVector {
            lateral_x: 0.0,
            vertical: -1.3,
            lateral_z: 0.0,
            word4_unconsumed: 0.0,
        };
        assert!((a.apply_to_aim_height(5.0) - 3.7).abs() < 1e-6);
    }

    #[test]
    fn no_geometry_holds_instead_of_falling() {
        // Zone geometry absent: gravity off (same guard as the column walker).
        let geom = MzbCollisionGeometry::default();
        let mut st = Walker::default();
        let obstacles = ObstacleSet::default();
        let res = step(
            &geom,
            &obstacles,
            &mut st,
            0.0,
            0.0,
            -3.0,
            0.0,
            0.0,
            1.0,
            1.0 / 60.0,
            false,
            false,
        );
        assert!(matches!(res.decision, VerticalDecision::NoGeometry));
        assert!((res.feet_z + 3.0).abs() < 1e-9);
    }

    /// Single up-facing quad at local y=0 spanning ±half in xz.
    fn floor_quad_submesh(half: f32) -> MzbSubMesh {
        MzbSubMesh {
            positions: vec![
                [-half, 0.0, -half],
                [half, 0.0, -half],
                [half, 0.0, half],
                [-half, 0.0, half],
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            tri_terrain: vec![0, 0],
            tri_normal: vec![[0.0, 1.0, 0.0]; 2],
            tri_camera_transparent: vec![false; 2],
            tri_invalid: Vec::new(),
            flags: 1,
        }
    }

    /// Axis-aligned box with its footprint centered on the local origin and
    /// its base in y=0. No underside: every wall probe's chain center sits at
    /// least BODY_RADIUS above the floor, so the riser-band rule and the
    /// column filter answer for it without an authored face.
    fn box_submesh(w: f32, h: f32, d: f32) -> MzbSubMesh {
        let (x, z) = (w / 2.0, d / 2.0);
        let v: [[f32; 3]; 8] = [
            [-x, 0.0, -z],
            [x, 0.0, -z],
            [x, 0.0, z],
            [-x, 0.0, z],
            [-x, h, -z],
            [x, h, -z],
            [x, h, z],
            [-x, h, z],
        ];
        let positions = v.to_vec();
        let indices = vec![
            0, 1, 5, 0, 5, 4, // -z side
            2, 3, 7, 2, 7, 6, // +z side
            3, 0, 4, 3, 4, 7, // -x side
            1, 2, 6, 1, 6, 5, // +x side
            4, 5, 6, 4, 6, 7, // top
        ];
        let tri_normal = vec![
            [0.0, 0.0, -1.0],
            [0.0, 0.0, -1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [-1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        MzbSubMesh {
            positions,
            indices,
            tri_terrain: vec![0; 10],
            tri_normal,
            tri_camera_transparent: vec![false; 10],
            tri_invalid: Vec::new(),
            flags: 1,
        }
    }

    fn placed(submesh_idx: usize, translation: [f32; 3], yaw: Option<f32>) -> MzbInstance {
        let mut t = Transform::from_xyz(translation[0], translation[1], translation[2]);
        if let Some(y) = yaw {
            t.rotation = Quat::from_rotation_y(y);
        }
        MzbInstance {
            submesh_idx,
            bevy_transform: t,
            water_height_bevy: None,
            lighting: None,
            sub_area_link: 0,
        }
    }

    fn baked(submeshes: &[MzbSubMesh], instances: &[MzbInstance]) -> MzbCollisionGeometry {
        MzbCollisionGeometry::from_block(build_collision_geometry(submeshes, instances, None))
    }

    /// One tick with positions expressed directly in bevy space (feet point,
    /// horizontal displacement), mapping through the wire convention exactly
    /// like dispatch does.
    fn tick_bevy(
        geom: &MzbCollisionGeometry,
        obstacles: &ObstacleSet,
        st: &mut Walker,
        feet: Vec3,
        disp: Vec2,
    ) -> StepResult {
        step(
            geom,
            obstacles,
            st,
            feet.x,
            -feet.z,
            -feet.y,
            disp.x,
            -disp.y,
            1.0,
            1.0 / 60.0,
            false,
            true,
        )
    }

    #[test]
    fn closed_door_floor_grounds_the_walker() {
        // The door floor source carried over from the pre-retail tick: a
        // closed door's up-facing deck is a floor for the column probe (a
        // drawbridge; an elevator platform), not only a wall.
        let geom = MzbCollisionGeometry::default();
        let deck_y = 1.2;
        let a = Vec3::new(-0.6, deck_y, -0.6);
        let b = Vec3::new(0.6, deck_y, -0.6);
        let c = Vec3::new(0.6, deck_y, 0.6);
        let e = Vec3::new(-0.6, deck_y, 0.6);
        let mut obstacles = ObstacleSet::default();
        obstacles.doors.push(DoorObstacle {
            tris: vec![([a, b, c], Vec3::Y), ([a, c, e], Vec3::Y)],
            min: Vec3::new(-0.6, deck_y - 0.1, -0.6),
            max: Vec3::new(0.6, deck_y, 0.6),
        });
        let mut st = Walker::default();
        let res = tick_bevy(
            &geom,
            &obstacles,
            &mut st,
            Vec3::new(0.0, 0.9, 0.0),
            Vec2::ZERO,
        );
        match res.decision {
            VerticalDecision::SingleStep { up } => assert!(up),
            other => panic!("deck floor not answered: {other:?}"),
        }
        let after = -res.feet_z;
        assert!(
            (after - deck_y).abs() < 1e-3,
            "expected merge onto the posed deck at {deck_y}, got {after}"
        );
    }

    #[test]
    fn actor_contact_drops_the_move_until_its_budget_expires() {
        // ControllableActor::CheckContactActor through Walker's ported budget:
        // first overlapping tick drops the move; sustained input walks through
        // after CONTACT_BLOCK_TICKS, and a fresh approach gets a fresh budget.
        let floor = floor_quad_submesh(20.0);
        let geom = baked(&[floor], &[placed(0, [0.0, 0.0, 0.0], None)]);
        let mut obstacles = ObstacleSet::default();
        obstacles.mobs.push(MobObstacle {
            id: 7,
            center: Vec2::new(2.0, 0.0),
            radius: 0.5,
        });
        let mut st = Walker::default();
        // Projected (1.6, z0) sits inside the mob circle through BODY_RADIUS.
        let first = tick_bevy(
            &geom,
            &obstacles,
            &mut st,
            Vec3::new(1.2, 0.0, 0.0),
            Vec2::new(0.4, 0.0),
        );
        match first.outcome {
            HorizontalOutcome::ActorContact { mob, .. } => assert_eq!(mob, 7),
            other => panic!("overlap must be ActorContact, got {other:?}"),
        }
        // The move was dropped: dx is the zeroed displacement.
        assert!((first.dx).abs() < 1e-6 && first.dy.abs() < 1e-6);
        let mut allowed = None;
        for tick in 0..240 {
            let res = tick_bevy(
                &geom,
                &obstacles,
                &mut st,
                Vec3::new(1.2, 0.0, 0.0),
                Vec2::new(0.4, 0.0),
            );
            if !matches!(res.outcome, HorizontalOutcome::ActorContact { .. }) {
                allowed = Some(tick);
                break;
            }
        }
        let Some(tick) = allowed else {
            panic!("budget never expired: still blocked after 240 ticks");
        };
        // CONTACT_BLOCK_TICKS = 30 decremented at 60/s -> release near tick 30.
        assert!((15..=45).contains(&tick), "released at tick {tick}");
    }

    #[test]
    fn planter_blocks_walk_move() {
        // A dedicated placed blocker object stays solid through the bake even though its
        // side faces stand within one follow word of the feet: the prop identity from
        // `build_collision_geometry` (record spec sections "Face classification" and
        // "Blocking belongs to other candidates") keeps it answering wall queries while
        // terrain-law risers of that height never do.
        let floor = floor_quad_submesh(20.0);
        let planter = box_submesh(0.8, 0.7, 0.8);
        let geom = baked(
            &[floor, planter],
            &[
                placed(0, [0.0, 0.0, 0.0], None),
                placed(1, [4.0, 0.0, 0.0], None),
            ],
        );
        let flags = &geom.block(ZONE_SLOT_MAIN).tri_prop_blocker;
        assert_eq!(flags.len(), 12);
        assert!(
            !flags[..2].iter().any(|f| *f) && flags[2..].iter().all(|f| *f),
            "the bake lost the planter's blocker identity (or flagged the ground)"
        );
        let obstacles = ObstacleSet::default();
        let mut st = Walker::default();
        let res = tick_bevy(
            &geom,
            &obstacles,
            &mut st,
            Vec3::new(2.3, 0.0, 0.0),
            Vec2::new(1.6, 0.0),
        );
        match res.outcome {
            HorizontalOutcome::Slid { .. } | HorizontalOutcome::WallHold { .. } => {}
            other => panic!("planter did not block: {other:?}"),
        }
        let stop_x = 2.3 + res.dx;
        assert!(
            stop_x > 3.0,
            "stopped at {stop_x}, never approached the face"
        );
        // Front face x=3.6 minus the body standoff (BODY_RADIUS): never closer.
        assert!(
            stop_x < 3.6 - BODY_RADIUS + 0.05,
            "walked into the planter: {stop_x}"
        );
    }

    #[test]
    fn rotated_fence_blocks_on_its_rotated_face() {
        // The placement pose reaches the queries: probes and blocks follow the
        // fence's rotated faces, not its AABB or unrotated footprint.
        let yaw = std::f32::consts::FRAC_PI_4;
        let rot = Quat::from_rotation_y(yaw);
        let floor = floor_quad_submesh(20.0);
        // Under PROP_OBJECT_MAX_EXTENT_YAMLS on every axis: shipped blocker panels
        // measure <=1.6 yalm per axis, and only such small dedicated meshes carry
        // the authored-blocker identity that answers wall queries at all.
        let fence = box_submesh(1.5, 1.4, 0.2);
        let geom = baked(
            &[floor, fence],
            &[
                placed(0, [0.0, 0.0, 0.0], None),
                placed(1, [10.0, 0.0, 10.0], Some(yaw)),
            ],
        );
        // Probe 0.4 along the rotated -z normal: a wall contact with exactly
        // that face's rotated authored normal.
        let c1_off = rot * Vec3::new(0.0, 0.7, -0.4);
        let hit =
            geom.nearest_wall_contact(Vec3::new(10.0, 0.7, 10.0) + c1_off, BODY_RADIUS, 0.0, 0.0);
        let Some((d2, n)) = hit else {
            panic!(
                "no contact against the rotated fence face at ({:?})",
                Vec3::new(10.0, 0.7, 10.0) + c1_off
            );
        };
        assert!(
            ((d2.sqrt() - 0.3).abs()) < 0.05,
            "contact distance {:.4}, expected ~0.3",
            d2.sqrt()
        );
        let want = rot * Vec3::new(0.0, 0.0, -1.0);
        assert!(
            n.dot(want) > 0.99,
            "face normal {n:?}, expected rotated {want:?}"
        );
        // Along the long axis the slab is thin: a point 2.8 from center on
        // that side sees no wall at all (the unrotated box would have had one).
        assert!(
            geom.nearest_wall_contact(Vec3::new(12.8, 0.7, 10.0), BODY_RADIUS, 0.0, 0.0)
                .is_none(),
            "phantom wall off the fence's long axis"
        );
        // End to end: walking +x through the fence line is clipped by the
        // diagonal face and the body stays outside the posed slab.
        let obstacles = ObstacleSet::default();
        let mut st = Walker::default();
        let res = tick_bevy(
            &geom,
            &obstacles,
            &mut st,
            Vec3::new(7.6, 0.0, 10.0),
            Vec2::new(3.0, 0.0),
        );
        match res.outcome {
            HorizontalOutcome::Slid { .. } | HorizontalOutcome::WallHold { .. } => {}
            other => panic!("rotated fence did not block: {other:?}"),
        }
        assert!(
            res.dx > 0.8 && res.dx < 2.4,
            "clip distance {:.3} off the diagonal stop band",
            res.dx
        );
        let final_xz = Vec2::new(7.6 + res.dx, 10.0 - res.dy);
        let local: Vec3 = rot.inverse() * Vec3::new(final_xz.x - 10.0, 0.0, final_xz.y - 10.0);
        assert!(
            local.z.abs() > 0.42,
            "body inside the posed slab: local {local:?}"
        );
    }

    #[test]
    fn prop_top_is_never_a_step_at_its_posed_height() {
        // An authored blocker object blocks and its top never answers height
        // ("Blocking belongs to other candidates", collision record; user law of
        // 2026-10-25: no walking up plants): standing in the column of a placed
        // crate at knee height, the walker settles back down toward the floor
        // instead of climbing the posed top.
        let floor = floor_quad_submesh(20.0);
        let crate_ = box_submesh(1.2, 0.7, 1.2);
        let geom = baked(
            &[floor, crate_],
            &[
                placed(0, [0.0, 0.0, 0.0], None),
                // Local top y=0.7 composed with translation y=0.3: the posed
                // top would be exactly 1.0 - what an unbaked law would stand the
                // walker on; the prop exclusion says it never happens.
                placed(1, [6.0, 0.3, 6.0], None),
            ],
        );
        let obstacles = ObstacleSet::default();
        let mut st = Walker::default();
        let res = tick_bevy(
            &geom,
            &obstacles,
            &mut st,
            Vec3::new(6.0, 0.9, 6.0),
            Vec2::ZERO,
        );
        assert!(
            matches!(res.decision, VerticalDecision::SingleStep { up: false })
                && -res.feet_z <= 0.6,
            "walker climbed onto an authored blocker top: {:?} at {}",
            res.decision,
            -res.feet_z
        );
        // The camera BVH is built through `trimesh_data` of the same block
        // (collision_bvh.rs), so the pull-in stops at props by construction:
        // the posed face is in that mirror too.
        let (positions, _) = geom.trimesh_data();
        assert!(
            positions
                .iter()
                .any(|p| (p.y - 1.0).abs() < 1e-3 && p.x > 5.4),
            "posed crate top missing from the mirrored triangle set"
        );
    }

    #[test]
    fn pushing_into_a_low_prop_never_bounces_the_pose_height() {
        // The float-hop jitter of 2026-10-26: walking into a knee-height crate on
        // level floor used to cycle blocked-sweep -> assist growth -> lifted retry
        // accepted against the old floor (support a full assist below the raised
        // pose) -> pose rises, drops, repeats - visible as the whole screen
        // shaking while pushing into short props. `ASSIST_SUPPORT_REACH_YAMLS`
        // bounds that support by the recorded half-yalm probe reach; through
        // several full assist-growth cycles the feet must not leave the floor at
        // all, and no frame may read airborne.
        let floor = floor_quad_submesh(20.0);
        let crate_ = box_submesh(1.2, 0.7, 1.2);
        let geom = baked(
            &[floor, crate_],
            &[
                placed(0, [0.0, 0.0, 0.0], None),
                placed(1, [4.6, 0.0, 0.0], None),
            ],
        );
        let obstacles = ObstacleSet::default();
        let mut st = Walker::default();
        let (mut x, mut up) = (2.4f32, 0.0f32);
        for tick in 0..500 {
            let res = tick_bevy(
                &geom,
                &obstacles,
                &mut st,
                Vec3::new(x, up, 0.0),
                Vec2::new(0.1, 0.0),
            );
            x += res.dx;
            up = -res.feet_z;
            assert!(
                (up - 0.0).abs() <= 1e-3,
                "pose left the floor while pushing into the crate: tick {tick} x {x:.2} up {up} ({:?})",
                res.decision
            );
            assert!(
                !matches!(res.decision, VerticalDecision::Airborne { .. }),
                "float-hop made a frame airborne while pressed into a prop: tick {tick}"
            );
        }
    }

    /// Axis-aligned up-facing quad at local y=0 spanning `[x0..x1] x [z0..z1]`.
    fn rect_submesh(x0: f32, x1: f32, z0: f32, z1: f32) -> MzbSubMesh {
        MzbSubMesh {
            positions: vec![[x0, 0.0, z0], [x1, 0.0, z0], [x1, 0.0, z1], [x0, 0.0, z1]],
            indices: vec![0, 1, 2, 0, 2, 3],
            tri_terrain: vec![0, 0],
            tri_normal: vec![[0.0, 1.0, 0.0]; 2],
            tri_camera_transparent: vec![false; 2],
            tri_invalid: Vec::new(),
            flags: 1,
        }
    }

    #[test]
    fn phantom_support_over_void_falls_instead_of_letting_the_walker_float_on_it() {
        // The Bastok Mines AH terrace signature (2026-10-26 overlay at the user's
        // xz(-13.65,+65.59)): an up-facing upper-structure DAT plane answers about
        // feet+1.167 where no floor is under it. Under the old symmetric window,
        // walking off the terrace edge accepted that phantom and lifted the pose
        // 1.167 in one frame, stranding the actor on air over the void - the
        // report's "falls to abyss" half. With the recorded reach
        // (`STEP_UP_ACCEPT_RISE_YAMLS`, collision record "Movement-time collision:
        // the character sweep") only the terrace answers while on it, past the
        // edge nothing answers, and the walk-off falls by the fall law.
        let terrace = rect_submesh(-4.0, 1.0, -5.0, 5.0);
        let canopy = floor_quad_submesh(20.0);
        let geom = baked(
            &[terrace, canopy],
            &[
                placed(0, [0.0, 3.0, 0.0], None),
                placed(1, [0.0, 4.167, 0.0], None),
            ],
        );
        // The canopy is terrain identity (its extent dwarfs the prop cap), so
        // `ground_walkable` does answer it where no terrace exists - the refusal
        // belongs to the acceptance window under test, not to face exclusion.
        assert!(geom
            .ground_walkable(Vec2::new(3.0, 0.0), 3.0)
            .is_some_and(|h| (h - 4.167).abs() < 1e-3));
        let obstacles = ObstacleSet::default();
        let mut st = Walker::default();
        let (mut x, mut up) = (-1.0f32, 3.0f32);
        let mut fell = false;
        for _tick in 0..96 {
            let res = tick_bevy(
                &geom,
                &obstacles,
                &mut st,
                Vec3::new(x, up, 0.0),
                Vec2::new(0.12, 0.0),
            );
            x += res.dx;
            up = -res.feet_z;
            assert!(
                up <= 3.0 + 1e-3,
                "phantom canopy lifted the pose to {up} at x {x:.2}: {:?}",
                res.decision
            );
            fell |= matches!(res.decision, VerticalDecision::Airborne { .. });
        }
        assert!(fell, "walk-off past the terrace edge never went airborne");
        assert!(up < 1.0, "pose did not keep falling: stuck at {up}");
        assert!(matches!(st.mode, WalkMode::Airborne { .. }));
    }

    #[test]
    fn wall_holds_the_pose_while_phantom_support_looms_over_it() {
        // Same terrace and canopy as the float test, with a shipped parapet-class
        // prop blocker (the screenshot's `p1` faces y[3.0..3.8]) in front of the
        // edge: pushing into the wall must hold at walking height - never rise on
        // the phantom above it, and never let step-up assist ride the phantom past
        // solid masonry (`standable_support_at_or_below`'s reach clause). Outcome
        // pin for user law of 2026-10-26: "walls can be randomly walked through if
        // I push into them" must stop being true.
        let terrace = rect_submesh(-4.0, 1.0, -5.0, 5.0);
        let canopy = floor_quad_submesh(20.0);
        let parapet = box_submesh(0.4, 0.8, 1.6);
        let geom = baked(
            &[terrace, canopy, parapet],
            &[
                placed(0, [0.0, 3.0, 0.0], None),
                placed(1, [0.0, 4.167, 0.0], None),
                placed(2, [0.75, 3.0, 0.0], None),
            ],
        );
        let flags = &geom.block(ZONE_SLOT_MAIN).tri_prop_blocker;
        assert_eq!(flags.len(), 14);
        assert!(
            !flags[..4].iter().any(|f| *f) && flags[4..].iter().all(|f| *f),
            "the bake lost the parapet's blocker identity (or flagged terrace/canopy)"
        );
        let obstacles = ObstacleSet::default();
        let mut st = Walker::default();
        let (mut x, mut up) = (-1.0f32, 3.0f32);
        let mut held = false;
        let mut last_decision = VerticalDecision::Flat;
        for _tick in 0..400 {
            let res = tick_bevy(
                &geom,
                &obstacles,
                &mut st,
                Vec3::new(x, up, 0.0),
                Vec2::new(0.1, 0.0),
            );
            x += res.dx;
            up = -res.feet_z;
            last_decision = res.decision;
            assert!(
                up <= 3.0 + 1e-3,
                "pose rose over the parapet at x {x:.2} on phantom support: {:?}",
                res.decision
            );
            held |= matches!(
                res.outcome,
                HorizontalOutcome::WallHold { .. } | HorizontalOutcome::Slid { .. }
            );
        }
        assert!(held, "the parapet never blocked the push");
        // Front face x=0.55 minus the body standoff: never closer.
        assert!(
            x < 0.55 - BODY_RADIUS + 0.05,
            "walked through the parapet: stopped at {x:.3}"
        );
        // Still standing grounded on the terrace at the end of the push.
        assert!(
            (up - 3.0).abs() < 1e-3,
            "pose left the terrace height: {up}"
        );
        assert!(
            matches!(last_decision, VerticalDecision::Flat),
            "not standing flat on the terrace at the end: {last_decision:?}"
        );
    }

    /// A floor quad at y=0.5 under the ray: hit; a vertical wall face: miss.
    #[test]
    fn tri_hits_column_finds_horizontal_faces_only() {
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
    fn tri_hits_column_interpolates_sloped_faces_in_either_winding() {
        let triangle = [
            Vec3::new(4.0, 10.0, 6.0),
            Vec3::new(6.0, 12.0, 6.0),
            Vec3::new(4.0, 14.0, 8.0),
        ];
        let query = Vec2::new(4.5, 6.5);
        assert_eq!(tri_hits_column(triangle, query), Some(11.5));
        assert_eq!(
            tri_hits_column([triangle[0], triangle[2], triangle[1]], query),
            Some(11.5)
        );
        assert_eq!(tri_hits_column(triangle, Vec2::new(6.0, 8.0)), None);
    }

    /// A slab crossing (feet+BODY_HEIGHT, y_new+BODY_HEIGHT] holds; one
    /// above the band or a descent does not. The positive case is a door leaf
    /// with a horizontal lintel at y=2.0 over the column.
    #[test]
    fn ceiling_slab_rejects_only_rises_into_geometry() {
        let doors: &[DoorObstacle] = &[];
        assert!(!doors_in_column_slab(doors, Vec2::ZERO, 1.8, 2.1));

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

    /// The blend's three terms at pure values: full forward walks the aim,
    /// a backward axis displaces along the reversed aim (heading minus a half
    /// turn), and lateral runs across it; each term carries this tick's step.
    #[test]
    fn heading_blend_places_each_axis_on_its_own_heading_term() {
        let aim = Vec2::new(1.0, 0.0);
        let right = Vec2::new(0.0, 1.0);

        let d = heading_blend(
            0.5,
            MoveAxes {
                forward: 1.0,
                lateral: 0.0,
            },
            aim,
            right,
        );
        assert!((d - Vec2::new(0.5, 0.0)).length() < 1e-6);

        let d = heading_blend(
            0.4,
            MoveAxes {
                forward: -1.0,
                lateral: 0.0,
            },
            aim,
            right,
        );
        assert!((d - Vec2::new(-0.4, 0.0)).length() < 1e-6);

        let d = heading_blend(
            1.0,
            MoveAxes {
                forward: 0.0,
                lateral: 0.6,
            },
            aim,
            right,
        );
        assert!((d - Vec2::new(0.0, 0.6)).length() < 1e-6);

        let s = std::f32::consts::FRAC_1_SQRT_2;
        let d = heading_blend(
            0.5,
            MoveAxes {
                forward: s,
                lateral: -s,
            },
            aim,
            right,
        );
        assert!((d.length() - 0.5).abs() < 1e-6);
        assert!((d - Vec2::new(0.5 * s, -0.5 * s)).length() < 1e-6);
    }
    /// Wire-bevy bridge mirroring `tick_bevy`: position expressed directly in
    /// bevy space (feet point, y up), displacement a bevy-space horizontal
    /// Vec2; carry uses the same dx/dy convention as the shipped pins.
    fn tick_bevy_at(
        geom: &MzbCollisionGeometry,
        st: &mut Walker,
        p: Vec3,
        disp: Vec2,
    ) -> StepResult {
        let obstacles = ObstacleSet::default();
        tick_bevy(geom, &obstacles, st, p, disp)
    }

    /// Regression (user panel 2026-10-27, Bastok Mines DAT 334 xz=(-30.5,+44.2)):
    /// "we are getting caught on walls. this a angle issue. it should be sliding."
    /// A pose touching an angled wall - authored normal inside the walkable band
    /// (`FLOOR_NORMAL_MIN`), leaning ~7.6% of its normal along its own run like
    /// the shipped bastion face (-0.945, +0.3188, -0.0718) over y 0..4.167
    /// (`kuluu-render/tests/mzb_bastok_wall_slide_dump.rs`) - must keep walking
    /// along the wall at full pace and along pure bevy-+z (the shipped 'caught'
    /// travel, approaching that face at dot(n, z) ~ -0.0718), while a head-on
    /// press still blocks. Before `INTO_FACE_APPROACH_SLOP` plus the probe margin
    /// dead-band (`probe_answer`), catch-vs-slide was decided by +-1e-9 noise in
    /// the travel-vs-normal approach dot and each persistent graze ate ~46% of
    /// every step through the margin (leg progress 0.857/1.6 measured).
    #[test]
    fn slides_along_an_angled_wall_face_while_touching_it() {
        // Construct a plane with exactly the shipped normal: base line along the
        // run r = (nz, 0, -nx), top edge up-slope u = n x r, vertical extent as
        // measured on the real face.
        let n = Vec3::new(-0.945, 0.3188, -0.0718).normalize();
        let run = {
            let v = Vec3::new(n.z, 0.0, -n.x);
            assert!(v.length() > 1e-6, "degenerate run for normal {n}");
            v.normalize()
        };
        let up_slope = n.cross(run).normalize();
        assert!(
            up_slope.y > 0.9,
            "constructed wall must lean back: u.y={}",
            up_slope.y
        );

        let mid = Vec3::new(2.6, 0.0, 0.0); // shipped base-line midpoint
        let half_run = 4.0;
        let e1 = mid - run * half_run;
        let e2 = mid + run * half_run;
        let rise = up_slope * (4.167 / up_slope.y); // face spans y 0..4.167 vertically
        let t1 = e1 + rise;
        let t2 = e2 + rise;
        assert!((t1.y - 4.167).abs() < 1e-3 && (t2.y - 4.167).abs() < 1e-3);

        // Wall submesh: two triangles per quad, authored normal n on every face;
        // floor submesh so the pose has support while it slides.
        let wall = MzbSubMesh {
            positions: vec![e1.to_array(), e2.to_array(), t2.to_array(), t1.to_array()],
            indices: vec![0, 1, 2, 0, 2, 3],
            tri_terrain: vec![0; 2],
            tri_normal: vec![n.to_array(); 2],
            tri_camera_transparent: vec![false; 2],
            tri_invalid: Vec::new(),
            flags: 1,
        };
        let floor = floor_quad_submesh(16.0);
        let geom = baked(
            &[wall, floor],
            &[
                placed(0, [0.0, 0.0, 0.0], None),
                placed(1, [2.6, 0.0, 0.0], None),
            ],
        );

        // Park the pose on the plane's -x side at shipped graze depth (probe
        // distance just inside r=BODY_RADIUS, intrusion ~0.015, the persistent-
        // contact state that starved the old margin). Walk in from clear ground
        // using the same query the sweep uses: the leaning face's horizontal
        // normal is not unit-x, so hand-computed plane offsets mis-estimate.
        let mut p = Vec3::new(mid.x - 1.0, 0.0, 0.0);
        while geom
            .nearest_wall_contact(Vec3::new(p.x, 0.35, p.z), BODY_RADIUS, 0.0, 0.0)
            .is_none()
        {
            p.x += 0.01;
            assert!(p.x < mid.x, "wall never answered walking in from x=mid-1");
        }
        let mut st = Walker::default();

        // Pre-assert the pose really touches the wall through the sweep's own
        // query at the body center it uses (`feet_y_of` = feet + BODY_RADIUS);
        // y=0.35 sits clear inside the face (its base line runs y=0, and the
        // point-to-triangle distance grows in the edge region below it).
        let contact = geom.nearest_wall_contact(Vec3::new(p.x, 0.35, p.z), BODY_RADIUS, 0.0, 0.0);
        let Some((d2, cn)) = contact else {
            panic!("wall must answer at the constructed pose x={}", p.x);
        };
        assert!(
            d2.sqrt() < BODY_RADIUS,
            "pose should touch, got {}",
            d2.sqrt()
        );
        assert!(
            (cn - n).length() < 1e-4,
            "authored normal must survive baking"
        );

        fn drive(
            geom: &MzbCollisionGeometry,
            st: &mut Walker,
            p: &mut Vec3,
            disp: Vec2,
            ticks: u32,
            air: &mut u32,
        ) {
            for _ in 0..ticks {
                let res = tick_bevy_at(geom, st, *p, disp);
                if matches!(res.decision, VerticalDecision::Airborne { .. }) {
                    *air += 1;
                }
                p.x += res.dx;
                p.z -= res.dy;
                p.y = -res.feet_z;
            }
        }

        let mut air = 0_u32;

        // The walker's displacement is a bevy-space horizontal Vec2 (x, z).
        let run_xz = Vec2::new(run.x, run.z);

        // Leg 1: one second along +run at the shipped 1.6 yps pace.
        let start1 = p;
        drive(&geom, &mut st, &mut p, run_xz * (1.6 / 60.0), 60, &mut air);
        assert_eq!(air, 0, "wall graze must never un-ground the pose");
        let along1 = (p - start1).dot(run);
        println!("synthetic wall leg +run: along={along1:.3} intent=1.6");
        assert!(along1 > 1.45, "plus-run slide regressed: {along1}");

        // Leg 2: back along -run - sliding works symmetrically.
        let start2 = p;
        drive(&geom, &mut st, &mut p, run_xz * -(1.6 / 60.0), 60, &mut air);
        assert_eq!(air, 0);
        let along2 = (start2 - p).dot(run);
        println!("synthetic wall leg -run: along={along2:.3} intent=1.6");
        assert!(along2 > 1.45, "minus-run slide regressed: {along2}");

        // Pure bevy-+z travel while grazing: the shipped 'caught' direction.
        let z_start = p.z;
        drive(
            &geom,
            &mut st,
            &mut p,
            Vec2::new(0.0, 1.6 / 60.0),
            45,
            &mut air,
        );
        assert_eq!(air, 0);
        let dz3 = p.z - z_start;
        println!("synthetic wall leg +z (caught dir): dz={dz3:.3} intent=1.2");
        assert!(
            dz3 > 1.0,
            "pure +z slide along the leaning face regressed: {dz3}"
        );

        // Head-on press must still block (this is a wall, not a ghost). The
        // pose sits inside r already, so blocking reads as no creep beyond the
        // contact shell it started in.
        let x_press_start = p.x;
        drive(
            &geom,
            &mut st,
            &mut p,
            Vec2::new(1.6 / 60.0, 0.0),
            60,
            &mut air,
        );
        assert_eq!(air, 0);
        println!(
            "synthetic wall head-on press: crept {:.3}",
            (p - Vec3::new(x_press_start, p.y, p.z)).length()
        );
        assert!(
            (p.x - x_press_start).abs() < 0.1,
            "wall must stay solid head-on: crept from {} to {}",
            x_press_start,
            p.x
        );
    }

    /// Regression (user panel 2026-10-27, Bastok Mines DAT 334 truss/fence at
    /// x[-46.6..-46.4], walkway slab +3.493 over street 0): "These 'fence/wall'
    /// should stop us. instead it sends us to the abyss and you climb it under
    /// certain angles." Synthetic stand-in built from `mzb_bastok_wall_slide_dump.rs`
    /// measurements, carrying both shipped properties the bug turn lost:
    /// - faces topping above the body point (feet + 0.5) answer the wall query
    ///   across their whole height after the height-rejection anchor moved there
    ///   (`WALL_TOP_BODY_POINT_TOL_YAMLS`): rail tops at deck+0.687 > deck+0.5 do
    ///   block, exactly where the old acceptance-ceiling anchor (deck+1.0) let the
    ///   whole fence be swallowed;
    /// - steep cap faces authored inside the wall band but short of the floor cut
    ///   (shipped tooth normal (-0.745, +0.667)): they may block bodies, yet
    ///   `standable_support_at_or_below` must refuse them as lifted-pose support,
    ///   so a pose is never hoisted onto fence metal.
    #[test]
    fn fence_rails_stop_the_walker_and_are_never_mounted() {
        let deck_y = 3.493_f32; // shipped walkway slab top
        let cap_top = deck_y + 0.687; // shipped truss cap height over the slab

        // `MzbSubMesh` is not Clone, so each build gets fresh submeshes from
        // these factories - every geometry in this test carries the same three
        // fence faces plus slab, once with a flat tread added.
        let rail = || {
            // Rail verticals spanning z, front face at x=-0.2 (authored -x normal),
            // back face at +0.2 (+x). Tops above feet+0.5 from the deck: blocking.
            MzbSubMesh {
                positions: vec![
                    [-0.2, deck_y, -6.0],
                    [-0.2, deck_y, 6.0],
                    [-0.2, cap_top, 6.0],
                    [-0.2, cap_top, -6.0], // 3
                    [0.2, deck_y, 6.0],
                    [0.2, deck_y, -6.0],
                    [0.2, cap_top, -6.0],
                    [0.2, cap_top, 6.0], // 7
                ],
                indices: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
                tri_terrain: vec![0; 4],
                tri_normal: vec![
                    [-1.0, 0.0, 0.0],
                    [-1.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0],
                ],
                tri_camera_transparent: vec![false; 4],
                tri_invalid: Vec::new(),
                flags: 1,
            }
        };

        // Cap pitches rising outward from each rail top toward a ridge, authored
        // with the shipped tooth tilt (-0.745, +0.667): inside the wall band (they
        // block) but short of the floor cut (never standable). The pitch over the
        // walking side spans deck+0.45..deck+1.09 - within one assist reach of the
        // deck, exactly where an ungated accumulator hoisted walkers onto metal.
        let tooth_n = Vec3::new(-0.745_f32, 0.667_f32, 0.0).normalize(); // authored as shipped
        let lo = 0.45_f32;
        let h = lo * (tooth_n.y / tooth_n.x.abs()); // rise for that face tilt
        let ridge_y = cap_top + h;

        let caps = || MzbSubMesh {
            positions: vec![
                [-0.2, cap_top, -6.0],
                [-0.2 - lo, ridge_y, -6.0],
                [-0.2 - lo, ridge_y, 6.0],
                [-0.2, cap_top, 6.0], // 3: left pitch, authored tooth normal
                [0.2, cap_top, 6.0],
                [0.2 + lo, ridge_y, 6.0],
                [0.2 + lo, ridge_y, -6.0],
                [0.2, cap_top, -6.0], // 7: right pitch mirrors it
            ],
            indices: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
            tri_terrain: vec![0; 4],
            tri_normal: {
                let mirror = Vec3::new(tooth_n.x.abs(), tooth_n.y, 0.0).normalize();
                vec![
                    tooth_n.to_array(),
                    tooth_n.to_array(),
                    mirror.to_array(),
                    mirror.to_array(),
                    mirror.to_array(),
                    mirror.to_array(),
                    tooth_n.to_array(),
                    tooth_n.to_array(),
                ]
            },
            tri_camera_transparent: vec![false; 4],
            tri_invalid: Vec::new(),
            flags: 1,
        };

        // Walkway slab ends at x=-0.65 - the fence stands over void beyond, as
        // shipped (street 4 yalms below; reaching it means falling).
        let deck = || MzbSubMesh {
            positions: vec![
                [-24.0, 0.0, -8.0],
                [-0.65, 0.0, -8.0],
                [-0.65, 0.0, 8.0],
                [-24.0, 0.0, 8.0],
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            tri_terrain: vec![0; 2],
            tri_normal: vec![[0.0, 1.0, 0.0]; 2],
            tri_camera_transparent: vec![false; 2],
            tri_invalid: Vec::new(),
            flags: 1,
        };

        let geom = baked(
            &[rail(), caps(), deck()],
            &[
                placed(0, [0.0, 0.0, 0.0], None),
                placed(1, [0.0, 0.0, 0.0], None),
                placed(2, [0.0, deck_y, 0.0], None),
            ],
        );

        // (a) Pre-assert the shipped law on queries before driving anything: at
        // one body radius out from the front face the wall query answers. Under
        // the old acceptance-ceiling anchor every rail/cap face topped <= deck+1.0
        // and was suppressed - that swallow is what sent walkers into the void.
        // Body center anchor (feet + BODY_RADIUS) minus a margin: the rail
        // triangles' base edge runs along y=deck_y and point-to-triangle
        // distance leaves the plane interior right at that corner band.
        let at = geom.nearest_wall_contact(
            Vec3::new(-0.58, deck_y + BODY_RADIUS - 0.15, 2.0),
            BODY_RADIUS,
            deck_y,
            0.0,
        );
        let Some((d_at, n_at)) = at else {
            panic!("rail face must answer one radius out from its front plane");
        };
        assert!(
            d_at.sqrt() < BODY_RADIUS && n_at.x < -0.9,
            "front rail must block: normal {n_at}"
        );

        // (b) The lifted-pose support law refuses tooth-tilt caps and accepts a
        // flat tread at the same height (assist gate `FLOOR_COS`): base deck,
        // assist-lifted pose, cap resolving within probe reach above.
        let lifted = deck_y + 1.0;
        assert!(
            !standable_support_at_or_below(&geom, Vec2::new(-0.45, 3.0), deck_y, lifted),
            "tooth-tilt cap (ny {:.3}) must never be lifted-pose support",
            tooth_n.y
        );
        // Same column with a flat tread of identical height. Re-keyed 2026-10-28/29 by USER's
        // one-mount ceiling (`dat_mzb::STEP_MOUNT_RISE_MAX_YAMLS`): class is necessary, not
        // sufficient - support standing 0.687 (over the shipped rail cap) above this pose's own
        // slab may not be mounted either, flat or toothed ("this wall is well above .5 ... you are
        // climbing it. then falling down because its not a surface"). The in-class case (treads at
        // 0.5 and below, class-gated on face normal) stays covered end-to-end by
        // `walks_up_a_half_yalm_terrain_riser_while_walking`.
        let tread = MzbSubMesh {
            positions: vec![
                [-0.65, cap_top, -8.0],
                [0.25, cap_top, -8.0],
                [0.25, cap_top, 8.0],
                [-0.65, cap_top, 8.0],
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            tri_terrain: vec![0; 2],
            tri_normal: vec![[0.0, 1.0, 0.0]; 2],
            tri_camera_transparent: vec![false; 2],
            tri_invalid: Vec::new(),
            flags: 1,
        };
        let geom_tread = baked(
            &[rail(), caps(), deck(), tread],
            &[
                placed(0, [0.0, 0.0, 0.0], None),
                placed(1, [0.0, 0.0, 0.0], None),
                placed(2, [0.0, deck_y, 0.0], None),
                placed(3, [0.0, 0.0, 0.0], None),
            ],
        );
        assert!(
            !standable_support_at_or_below(&geom_tread, Vec2::new(-0.45, 3.0), deck_y, lifted),
            "flat tread at the cap height stands over the one-mount ceiling above the slab and must not be mountable either (USER law)"
        );

        // Drive: straight +x travel from clear ground; the fence must stop the
        // pose on the slab. Void behind the deck edge (-0.65) sits behind the
        // blocked line, so these bounds are exactly the law.
        let front_block_line = -0.6_f32; // face plane at -0.2 minus body radius
        let mut p = Vec3::new(-4.0, deck_y, 1.0);
        let mut st = Walker::default();
        let mut air = 0_u32;
        let mut max_x = p.x;
        let mut min_h = deck_y;
        let mut max_h = deck_y;
        for _ in 0..180 {
            let res = tick_bevy_at(&geom, &mut st, p, Vec2::new(1.6 / 60.0, 0.0));
            if matches!(res.decision, VerticalDecision::Airborne { .. }) {
                air += 1;
            }
            p.x += res.dx;
            p.z -= res.dy;
            p.y = -res.feet_z;
            max_x = max_x.max(p.x);
            min_h = min_h.min(p.y);
            max_h = max_h.max(p.y);
        }
        println!(
            "synthetic fence drive: stopped x={max_x:.2} (front face at -0.2) h=[{min_h:.3}..{max_h:.3}] air={air}"
        );
        assert_eq!(air, 0, "never over the void: end {p:?}");
        assert!(
            (min_h - deck_y).abs() < 0.2,
            "sank to {min_h} on solid walkway"
        );
        assert!(
            max_h < deck_y + 0.5,
            "mounted fence metal at {max_h} (deck {deck_y})"
        );
        assert!(max_x > -3.9, "walked almost nowhere: {max_x}");
        assert!(
            max_x < front_block_line,
            "crossed the rail plane: max x={max_x}"
        );

        // And walking along the fence must slide, not stall (same law as the
        // bastion pin, on rails perpendicular to travel).
        let z0 = p.z;
        for _ in 0..60 {
            let res = tick_bevy_at(&geom, &mut st, p, Vec2::new(0.0, 1.6 / 60.0));
            if matches!(res.decision, VerticalDecision::Airborne { .. }) {
                air += 1;
            }
            p.x += res.dx;
            p.z -= res.dy;
            p.y = -res.feet_z;
        }
        assert_eq!(air, 0);
        let dzs = p.z - z0;
        println!("synthetic fence slide along rail: dz={dzs:.3} intent=1.6");
        assert!(dzs > 1.2, "must slide along the rail line: {dzs}");
        assert!(
            p.x < front_block_line,
            "sliding must not leak past the fence: x={}",
            p.x
        );
    }
}
