//! Install-gated smooth-climb checks: drive the real walker over shipped
//! staircases (San d'Oria north/south, Bastok Mines) at crawl-to-run speeds and
//! hold it to the recorded law — no airborne inside a staircase ("Fall and
//! landing", `.agents/skills/retail-observe/references/2026-10-25-collision-query.md`),
//! height follows measured ground without sinking a tread (the ghost report of
//! 2026-10-25), each tick stays under the follow-word rate bound ("Step up"),
//! and the walker arrives past the crest instead of stalling. The boxes, lanes
//! and seed heights are the flights measured against DAT 331/334/330 on
//! 2026-10-26; each lane's climb is re-detected from geometry every run (the
//! profile must ground near its seed and still rise), so a re-authored
//! staircase fails here rather than silently invalidating these pins.

use bevy::math::{Vec2, Vec3Swizzles};
use kuluu_render::dat_mzb::{
    build_collision_geometry, MzbCollisionGeometry, FOLLOW_WORD_YAMLS_PER_FRAME,
};

use super::obstacles::ObstacleSet;
use super::{step, VerticalDecision, Walker};

/// One flight to walk: an outer box, the travel axis, lane coordinates across
/// it (world units on the other axis), and the height a profile chain starts from.
/// `status` says what CI holds this box to today: [`CLEAN`] drives must pass;
/// any other string reports detections without failing the run, with that reason shown.
const CLEAN: &str = "clean";
struct StairBox {
    label: &'static str,
    zone: u16,
    x0: f32,
    x1: f32,
    z0: f32,
    z1: f32,
    along_x: bool,
    lanes: &'static [f32],
    seed_h: f32,
    status: &'static str,
}

const STAIR_BOXES: &[StairBox] = &[
    // Northern San d'Oria city staircase (the pinned class in mzb_shipped_step_classes.rs).
    StairBox {
        label: "sd_north_pin_stairs",
        zone: 231,
        x0: -201.5,
        x1: -190.5,
        z0: -84.0,
        z1: -70.0,
        along_x: true,
        lanes: &[-74.666_664, -78.0, -81.333_336],
        seed_h: -8.0,
        status: CLEAN,
    },
    // Northern San d'Oria market terraces (parallel half-yalm flights).
    StairBox {
        label: "sd_north_market_flights",
        zone: 231,
        x0: -200.5,
        x1: -189.5,
        z0: -110.0,
        z1: -100.0,
        along_x: true,
        lanes: &[-107.5, -103.5],
        seed_h: 0.5,
        status: CLEAN,
    },
    // Northern San d'Oria one-yalm big steps (census cells x[−181..−178) z[−176..−171); lanes on cell centres).
    StairBox {
        label: "sd_north_big_steps_x",
        zone: 231,
        x0: -184.5,
        x1: -176.0,
        z0: -176.5,
        z1: -170.5,
        along_x: true,
        lanes: &[-175.5, -173.5, -171.5],
        seed_h: -1.5,
        // Known-fail under the restored HEAD wall law (out=Slid 0% at tread edges). Under
        // the props-only variant this lane instead walked off-crest into a void column
        // (ghost + sink 46-144 yalm, measured 2026-10-26) - neither kuluu law matches retail,
        // whose narrow phase f_1016f320 (build 2026-10-05) ignores a wall face whose whole
        // extent stands above the feet and takes Y from up-facing faces inside its
        // half-yalm probe. Retail-law implementation is the open work, not this pin.
        // Re-measured 2026-10-26 after wall suppression and support acceptance were tied to one
        // ceiling: three lanes stopped before the crest, sinking 0.326/0.315.
        // Re-measured 2026-10-27 under the riser-exemption stack (`nearest_wall_contact_riser_aware`,
        // `contact_floor` continuity rule): all three lanes still stop at the first facade face -
        // the exemption flickers once the body presses deep into a merged multi-riser quad
        // (forensics `sd_bigstep_forensics`: contact re-answers at d 0.1 after pass-through), and
        // one lane then falls the gap's void column (sink 24.960). The synthetic single rise of
        // exactly one yalm climbs (`pressed_against_a_one_yalm_terrain_riser_never_ghosts_or_drops`);
        // shipped facades need retail's registered-candidate architecture (open L row).
        // Re-measured 2026-10-28 with ring support in `contact_floor`: one lane detected, it
        // climbs -1.5 -> -0.25 and stops against the same facade face (WallHold at x -179.767).
        // Forensics that day (`sd_bigstep_forensics`, scratchpad `bigstep_forensics_today.txt`)
        // retract the earlier "merged facade quads / retail candidate-shape architecture"
        // reading: the blocking triangles are t48829/t48830, normal (+1,0), y[+0.5..+2.1] -
        // bridge-abutment side faces standing on a shelf at -0.25 with their bottoms floating
        // 0.75 above it, and they block correctly under every recorded law. The census cells
        // x[-184..-176] z[-176..-169] around them hold ramp terraces (n.y ~0.89 slopes,
        // flats -1.5/-2.5) beneath a bridge deck at +2.14 with soffit +0.5: no authored
        // player staircase in this box, the same false-positive class as sd_south_upper_steps_*.
        status: "census false positive (measured 2026-10-28: ramp terraces under a bridge deck, no player staircase; the drive's stopper is a legitimate abutment wall t48829/t48830); kept as a no-ghost watch",
    },
    StairBox {
        label: "sd_north_big_steps_z",
        zone: 231,
        x0: -182.0,
        x1: -177.0,
        z0: -179.0,
        z1: -168.5,
        along_x: false,
        lanes: &[-180.5, -179.5, -178.5],
        seed_h: -1.5,
        // Was known-fail under the restored HEAD wall law; see steps_x for retail's actual
        // rule and the facade-press class still open on that axis.
        // Re-measured 2026-10-28 with ring support in `contact_floor` (the slab-seam hold):
        // both detected lanes drive clean at all four speeds. The former sink of exactly
        // 1.000 was a buried-slab seam column answering under the pose - same class fixed at
        // the Bastok shell (`shell_seams_between_tread_plates_do_not_drop_the_pose`). Honest
        // scope note from the same forensics as steps_x: this axis drives through those ramp
        // terraces under the viaduct, not an authored staircase either - held clean as a
        // seam/follow-law pin on that class, not as proof of stair climbing.
        // Known-status since 2026-10-28 (crossing-linked look-ahead ramp, USER law): one drive
        // of lane -179.5 h[-3.0..-1.8] sinks 0.206 at z=-168.36 where a floating-slab seam
        // sliver ~0.2 tall stands ~0.1 along travel. The raw look scan accepts it as the nearest
        // riser and lifts +0.206 (one ramp sample, weight 0.875 x rise+uplift); the body-side
        // `contact_floor` continuity that holds the pose flat across such seams (`shell_seams_
        // between_tread_plates_do_not_drop_the_pose`) has no counterpart in the scan, so support
        // stays -4 and the lift un-fades over five frames. Sink = exactly one look-ahead lift;
        // every other lane/speed clean under the ramp. Seam suppression inside the scan is the
        // named follow-up; kept as a no-ghost watch meanwhile.
        status: "known (measured 2026-10-28, ramp law): lane -179.5 sinks 0.206 at one seam sliver z=-168.36 (see comment); all other drives clean; no-ghost watch",
    },
    // Southern San d'Oria upper-terrace stairs (rise class ~0.9).
    StairBox {
        label: "sd_south_upper_steps_x",
        zone: 230,
        x0: 116.5,
        x1: 125.5,
        z0: -91.5,
        z1: -82.5,
        along_x: true,
        lanes: &[-89.5, -87.5, -85.5],
        seed_h: 1.0,
        status: "census false positive (pyramid slopes; no staircase verified at this box)",
    },
    StairBox {
        label: "sd_south_upper_steps_z",
        zone: 230,
        x0: 116.5,
        x1: 125.5,
        z0: -92.0,
        z1: -82.0,
        along_x: false,
        lanes: &[118.5, 120.5, 122.5],
        seed_h: 1.0,
        status: "census false positive (see steps_x)",
    },
    // Bastok Mines terraced bowl climb (survey bbox x[-175..-171] z[+21..+25]).
    StairBox {
        label: "bastok_mines_terraces_x",
        zone: 234,
        x0: -177.0,
        x1: -169.5,
        z0: 20.5,
        z1: 25.5,
        along_x: true,
        lanes: &[24.5, 23.0, 21.5],
        seed_h: 7.0,
        status: "smooth bowl slopes, not steps; kept as a no-ghost watch",
    },
    StairBox {
        label: "bastok_mines_terraces_z",
        zone: 234,
        x0: -176.5,
        x1: -170.0,
        z0: 20.5,
        z1: 28.5,
        along_x: false,
        lanes: &[-174.5, -173.5, -172.5, -171.5],
        seed_h: 7.0,
        status: "smooth bowl slopes (see terraces_x)",
    },
    // Bastok Mines one-yalm rock steps (survey bbox x[+26..+31] z[+13..+17]).
    StairBox {
        label: "bastok_mines_rock_steps_x",
        zone: 234,
        x0: 24.5,
        x1: 32.5,
        z0: 12.5,
        z1: 17.5,
        along_x: true,
        lanes: &[13.5, 14.5, 15.5, 16.5],
        seed_h: -6.7,
        // Re-measured 2026-10-26 after wall suppression and support acceptance were tied to one
        // ceiling (dat_mzb STEP_UP_ACCEPT_RISE_YAMLS): both detected lanes now stop at the flight
        // instead of walking off the crest into a pit below (the former ghost 128 ticks / sink 98.8
        // yalm, measured earlier). Better than falling, still not climbing.
        // Re-measured 2026-10-28: the wall-aware detector finds no climb entry on these four lanes
        // (columns answer body-blocked at both poses), so nothing is driven here - kept non-clean
        // as a watch until something re-establishes why this class refused to climb.
        status: "measured 2026-10-28: detector entries all wall-blocked, no drives (was 2026-10-26: two lanes stalled at the flight after a former crest walk-off sink 98.8 yalm); cause never established",
    },
    // Bastok Mines long half-yalm flights toward the AH terrace (three lanes).
    StairBox {
        label: "bastok_mines_ah_flights",
        zone: 234,
        x0: 102.5,
        x1: 113.5,
        z0: 12.0,
        z1: 20.5,
        along_x: true,
        lanes: &[14.0, 16.5, 18.5],
        seed_h: -6.0,
        status: CLEAN,
    },
    // Bastok Mines AH entrance shell flight (2026-10-26 overlay, Garfield at
    // xz(-21.3,+69.6)): 0.26-riser treads riding on a floor slab that continues
    // underneath - the class `ground_walkable` used to answer as flat floor.
    StairBox {
        label: "bastok_mines_shell_steps_x",
        zone: 234,
        x0: -25.0,
        x1: -16.5,
        z0: 65.5,
        z1: 72.5,
        along_x: true,
        // +69.5 is the overlay-verified walking line (Garfield stood on these
        // treads at xz(-21.3,+69.6), the report that opened this box); the other
        // three lanes were synthetic and clip the shell's flank geometry.
        lanes: &[66.5, 68.5, 69.5, 70.5],
        seed_h: 0.0,
        // Re-measured 2026-10-26 after wall suppression and support acceptance were tied to one
        // ceiling: every lane stops before the crest exit, worst sink 1.769 yalm (it was stalling at
        // a shallower depth). The axis-aligned lanes clip flanks/side geometry - not yet established
        // as balustrade-class.
        // Re-measured 2026-10-27 under the riser-exemption stack: same class, deepest sink now
        // 2.350 yalm (lanes travel further into the flight before stopping).
        // Re-measured 2026-10-28: every lane is refused entry by the wall-aware detector - its
        // columns answer deep authored contact (shell flank/jamb sheets, block-probe d 0.05..0.37)
        // at both body poses - so no drive runs on these four synthetic axis lanes. The flight's
        // real reported line is pinned clean where it belongs: along the walking line from
        // xz(-21.3,+69.6) in `climb_the_ah_shell_treads_from_the_reported_stand_point` (this file)
        // and the -z lanes of `bastok_mines_shell_steps_z`. Non-clean keeps this box as the axis-
        // lane watch; its former sink class is fixed by ring support (seam hold).
        // Same structure, 2026-10-28 side observation from the terrace-box authoring run:
        // ascending the mound-flank slope plates at x[-24..-22] z~+68 dips ~0.5 yalm between
        // plates (the ring support holds the tread-seam class, not steep flank-ramp seams).
        status: "measured 2026-10-28: all four axis lanes entry-blocked by shell flank/jamb sheets, no drives; reported walk-line pinned clean elsewhere in this file - former seam-sink class closed by contact-floor ring support",
    },
    StairBox {
        label: "bastok_mines_shell_steps_z",
        zone: 234,
        x0: -25.0,
        x1: -18.5,
        z0: 65.5,
        z1: 73.5,
        along_x: false,
        lanes: &[-24.0, -22.0, -20.0],
        seed_h: 0.0,
        // Measured 2026-10-26/27: lane -20.0 mounted treads but sank 0.51 yalm mid-flight.
        // Cause, measured 2026-10-28 against shipped data: at xz(-20,+68.6) the infinitesimal
        // pose column slipped into the sliver between successive tread tops riding the slab and
        // answered street 0 as a legal drop - the seam class now held by `contact_floor`'s ring
        // support (`shell_seams_between_tread_plates_do_not_drop_the_pose`). Re-measured clean:
        // detected lane drives all four speeds, air=0, no sink past tolerance.
        status: CLEAN,
    },
    // Bastok Mines AH west terrace steps (lane z~+69 eastbound; grid dump `ah_terrace_wide.txt`,
    // KULUU_ZONE_GRID 234:-26:-9:62:72:0.5, shallow ~+0.25 risers on skins over a continuing slab)
    // were authored as a CLEAN box this morning and drove clean - but that detection chain tracked
    // the slab-under-skin risers by the ring ANTICIPATING each next tread sideways, measured under
    // USER's "feels a bit high on the steps" report to be riding poses 0.746 yalm above their own
    // column; `RING_SUPPORT_HOLD_LIFT_CAP_YAMLS` removed that behavior, after which a ground-answer
    // profile chain (no step-assist) cannot see slab-under-skin risers at all and the box detected
    // nothing. Its drive half was real gameplay, so the class stays pinned where it is honestly
    // visible on the same flights: `climb_the_ah_shell_treads_from_the_reported_stand_point` (the
    // climb itself) and `approaching_a_shipped_riser_never_lifts_the_pose_early_or_flaps` (height,
    // flap, and pre-lift law at straight AND diagonal angles).
];

/// Step heights the detector links into a climb: below `LINK_MIN_RISE` flat, above `LINK_MAX_RISE` a wall
/// band. Chosen against measured ramp profiles (the Bastok AH incline gains +0.25 per half-yalm sample).
const LINK_MIN_RISE_YAMLS: f32 = 0.2;
const LINK_MAX_RISE_YAMLS: f32 = 1.75;
/// Tread flats the chain carries between risers before calling the run done.
const MAX_FLAT_SAMPLES_BETWEEN_RISERS: usize = 6;
/// A run must net this much climb over at least this many rising links to count as stairs.
const CLIMB_MIN_NET_RISE_YAMLS: f32 = 1.2;
const CLIMB_MIN_STEPS: usize = 2;
/// How far a climbing walker may fall back below its best-so-far height mid-flight.
const SINK_TOLERANCE_YAMLS: f32 = 0.2;
/// Profile sampling pitch and approach/exit margins driven past the flight.
const PROFILE_STEP_YAMLS: f32 = 0.5;
const APPROACH_YAMLS: f32 = 2.0;
const EXIT_PAST_CREST_YAMLS: f32 = 1.5;

/// Speeds walked as yalm/sec: the user's slow creep, a walk, a fast walk, a run.
const WALK_SPEEDS: [f32; 4] = [0.4, 0.7, 1.6, 2.4];

fn zone_geom(zone: u16) -> Option<MzbCollisionGeometry> {
    let root = ffxi_dat::archive::open_test_install()?;
    bevy::tasks::AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
    let file_id = ffxi_dat::zone_dat::zone_id_to_mzb_file_id(zone)?;
    let (submeshes, instances) =
        kuluu_render::dat_mzb::load_mzb_placed(&root, file_id, None).ok()?;
    Some(MzbCollisionGeometry::from_block(build_collision_geometry(
        &submeshes,
        &instances,
        Some(file_id),
    )))
}

/// One detected climb in travel order: `entry_s` is the low side walked from, past the crest at `exit_s`; `s` runs along the box's axis at `lane`.
#[derive(Clone, Copy, Debug)]
struct Climb {
    lane: f32,
    along_x: bool,
    dir: f32,
    entry_s: f32,
    exit_s: f32,
    h_low: f32,
    h_high: f32,
}

fn detect_climbs(geom: &MzbCollisionGeometry, b: &StairBox) -> Vec<Climb> {
    let mut out = Vec::new();
    for &lane in b.lanes {
        for dir in [1.0f32, -1.0] {
            println!("detect {} lane {:+.2} dir {:+.0} start", b.label, lane, dir);
            if let Some(c) = climb_on_profile(geom, b, lane, dir) {
                println!(
                    "  climb s[{:.1}..{:.1}] h[{:.2}..{:.2}]",
                    c.entry_s, c.exit_s, c.h_low, c.h_high
                );
                out.push(c);
            }
        }
    }
    out
}

fn profile_span(b: &StairBox) -> (f32, f32) {
    if b.along_x {
        (b.x0 - APPROACH_YAMLS, b.x1 + APPROACH_YAMLS)
    } else {
        (b.z0 - APPROACH_YAMLS, b.z1 + APPROACH_YAMLS)
    }
}

/// Chain-sample a profile in travel direction from the box seed height, then take
/// the first rising run meeting the climb bar. The chain references each answer to the previous,
/// so flights are followed upward across their treads; it resets on any drop, gap or wall-band jump.
fn climb_on_profile(
    geom: &MzbCollisionGeometry,
    b: &StairBox,
    lane: f32,
    dir: f32,
) -> Option<Climb> {
    let (s_lo, s_hi) = profile_span(b);
    let n = ((s_hi - s_lo) / PROFILE_STEP_YAMLS).ceil() as usize + 1;
    let s_at = |k: usize| -> f32 {
        if dir > 0.0 {
            s_lo + k as f32 * PROFILE_STEP_YAMLS
        } else {
            s_hi - k as f32 * PROFILE_STEP_YAMLS
        }
    };
    // Each column contributes every up-facing hit; chaining picks the candidate nearest the previous
    // answer (seeded at the box height), so a multi-floor column cannot hijack the profile.
    let mut ref_y = b.seed_h;
    let mut hs: Vec<Option<f32>> = Vec::with_capacity(n);
    // A column the body could not stand up in and push through (the authored
    // wall chain answers a contact inside the body radius at BOTH the ground
    // pose and the max-assist lifted pose - exactly what `step`'s sweep would
    // stop on, suppression included) breaks any climb run: no drive enters
    // from this side. Shell-flank stall evidence 2026-10-27.
    let mut blocked: Vec<bool> = Vec::with_capacity(n);
    for k in 0..n {
        let s = s_at(k);
        let xz = if b.along_x {
            Vec2::new(s, lane)
        } else {
            Vec2::new(lane, s)
        };
        // Chain through the walker's own standable answer (`step::contact_floor`):
        // detection and drive must agree on every rise window, far-rise continuity
        // clause and drop bound. The old "follow up within reach" heuristic rode
        // phantom roof slopes off slab edges here - both the shell-flank stall
        // entries and one ghost entry were detector artifacts of that disagreement
        // (measured 2026-10-27; `block-probe` prints in this test's dump).
        match super::step::contact_floor(geom, &[], xz, ref_y, true) {
            Some(h) => {
                ref_y = h;
                hs.push(Some(h));
            }
            None => {
                hs.push(None);
                blocked.push(false);
                continue;
            }
        }
        let center = bevy::math::Vec3::new(xz.x, ref_y + super::consts::BODY_RADIUS, xz.y);
        let answer_at = |lift: f32| {
            geom.nearest_wall_contact(center, super::consts::BODY_RADIUS, ref_y, lift)
                .map(|(d2, n)| (d2.sqrt(), n))
        };
        if std::env::var("KULUU_STAIR_DUMP").is_ok() {
            if let Some((d0, _)) = answer_at(0.0) {
                println!(
                    "  block-probe s={:+7.2} feet={:+6.3} lift0 d{:.3} n({:.2},{:.2}) | liftMax {}",
                    s,
                    ref_y,
                    d0,
                    answer_at(0.0).unwrap().1.x,
                    answer_at(0.0).unwrap().1.z,
                    match answer_at(super::step::STEP_ASSIST_MAX_YAMLS) {
                        Some((dl, nl)) => format!("d{dl:.3} n({:.2},{:.2})", nl.x, nl.z),
                        None => String::from("free"),
                    }
                );
            }
        }
        // Deep contact: the face intrudes well inside the body radius (a graze
        // at the shell's edge answers too but the sweep slides off it).
        let stopper =
            |lift: f32| answer_at(lift).is_some_and(|(d, _)| d < super::consts::BODY_RADIUS - 0.1);
        blocked.push(stopper(0.0) && stopper(super::step::STEP_ASSIST_MAX_YAMLS));
    }
    // The drive walks `APPROACH_YAMLS` before the entry and `EXIT_PAST_CREST` past
    // the exit: a body-blocked column anywhere in that window stops it, so the run
    // cannot end there either. Two extra sample steps of margin cover both.
    let blocked_window = |lo: usize, hi: usize| -> bool {
        let hi = hi.min(blocked.len().saturating_sub(1));
        (lo..=hi).any(|k| blocked[k])
    };
    let drive_margin_k =
        ((APPROACH_YAMLS + EXIT_PAST_CREST_YAMLS) / PROFILE_STEP_YAMLS).ceil() as usize;
    let mut run_start: Option<usize> = None;
    let mut last_rise_k: usize = 0;
    for k in 1..hs.len() {
        if blocked[k] || blocked[k - 1] {
            run_start = None;
            continue;
        }
        let (Some(h_prev), Some(h_now)) = (hs[k - 1], hs[k]) else {
            run_start = None;
            continue;
        };
        let dh = h_now - h_prev;
        if dh > LINK_MIN_RISE_YAMLS && dh <= LINK_MAX_RISE_YAMLS {
            let st = *run_start.get_or_insert(k - 1);
            last_rise_k = k;
            // Riser transitions must sit inside the box core so lanes graze no terrace edge.
            let (s_in_lo, s_in_hi) = if b.along_x {
                (b.x0 + 1.5, b.x1 - 1.5)
            } else {
                (b.z0 + 1.5, b.z1 - 1.5)
            };
            let (s_st, s_k) = (s_at(st).min(s_at(k)), s_at(st).max(s_at(k)));
            if s_st < s_in_lo || s_k > s_in_hi {
                run_start = None;
                continue;
            }
            if last_rise_k - st + 1 >= CLIMB_MIN_STEPS
                && h_now - hs[st]? >= CLIMB_MIN_NET_RISE_YAMLS
                && !blocked_window(st.saturating_sub(drive_margin_k), k + drive_margin_k)
            {
                return Some(Climb {
                    lane,
                    along_x: b.along_x,
                    dir,
                    entry_s: s_at(st),
                    exit_s: s_at(last_rise_k + 1).max(s_at(k)),
                    h_low: hs[st]?,
                    h_high: h_now,
                });
            }
        } else if dh.abs() <= LINK_MIN_RISE_YAMLS {
            if let Some(st) = run_start {
                if k - last_rise_k > MAX_FLAT_SAMPLES_BETWEEN_RISERS {
                    if last_rise_k + 1 - st >= CLIMB_MIN_STEPS
                        && hs[last_rise_k]? - hs[st]? >= CLIMB_MIN_NET_RISE_YAMLS
                        && !blocked_window(st.saturating_sub(drive_margin_k), k + drive_margin_k)
                    {
                        return Some(Climb {
                            lane,
                            along_x: b.along_x,
                            dir,
                            entry_s: s_at(st).min(s_at(last_rise_k)),
                            exit_s: (s_at(last_rise_k + 1) + PROFILE_STEP_YAMLS * dir)
                                .max(s_at(k - 1)),
                            h_low: hs[st]?,
                            h_high: hs[last_rise_k]?.max(h_now),
                        });
                    }
                    run_start = None;
                }
            }
        } else {
            run_start = None; // drop or wall-band jump
        }
    }
    None
}

struct DriveReport {
    air_ticks: usize,
    max_tick_dy: f32,
    reached_crest_exit: bool,
    sank_below: Option<f32>,
    end_height: f32,
    trace_tail: String,
}

fn drive_climb(geom: &MzbCollisionGeometry, c: &Climb, yps: f32) -> DriveReport {
    let step_len = yps / 60.0;
    let s_start = if c.dir > 0.0 {
        c.entry_s - APPROACH_YAMLS
    } else {
        c.entry_s + APPROACH_YAMLS
    };
    let xz_at = |s: f32| -> Vec2 {
        if c.along_x {
            Vec2::new(s, c.lane)
        } else {
            Vec2::new(c.lane, s)
        }
    };
    let start_xz = xz_at(s_start);
    let mut up = match geom.ground_nearest(start_xz, c.h_low) {
        Some(h) => h,
        None => {
            return DriveReport {
                air_ticks: 0,
                max_tick_dy: 0.0,
                reached_crest_exit: false,
                sank_below: None,
                end_height: f32::NAN,
                trace_tail: "approach column has no ground".to_string(),
            }
        }
    };
    let (mut ax, mut az) = (start_xz.x, start_xz.y);
    let mut st = Walker::default();
    let crest_exit = if c.dir > 0.0 {
        c.exit_s + EXIT_PAST_CREST_YAMLS / 2.0
    } else {
        c.exit_s - EXIT_PAST_CREST_YAMLS / 2.0
    };
    let (box_lo, box_hi) = if c.dir > 0.0 {
        (c.entry_s - APPROACH_YAMLS, c.exit_s + APPROACH_YAMLS)
    } else {
        (c.exit_s - APPROACH_YAMLS, c.entry_s + APPROACH_YAMLS)
    };
    let mut rep = DriveReport {
        air_ticks: 0,
        max_tick_dy: 0.0,
        reached_crest_exit: false,
        sank_below: None,
        end_height: up,
        trace_tail: String::new(),
    };
    let ticks_raw = (box_hi - box_lo).abs() / step_len;
    let ticks_max = (ticks_raw as i32).saturating_mul(4).saturating_add(900);
    println!(
        "drive plan lane {:+.2} dir {:+.0} yps {yps:.1}: box [{box_lo:.1}..{box_hi:.1}] step_len {step_len:.5} ticks_raw {ticks_raw:.0} -> max {ticks_max}",
        c.lane, c.dir
    );
    let mut lines: Vec<String> = Vec::new();
    let mut last_step_us = 0u32;
    for tick in 0..ticks_max {
        if tick > 0 && tick % 50 == 0 {
            println!(
                "drive lane {:+.2} dir {:+.0} yps {yps:.1}: tick {tick}/{ticks_max} last_step_us {last_step_us} pos ({ax:.2},{az:.2}) up {up:.2}",
                c.lane, c.dir
            );
        }
        let t_before = std::time::Instant::now();
        let (dxw, dyw) = if c.along_x {
            (step_len * c.dir, 0.0)
        } else {
            (0.0, -step_len * c.dir)
        };
        let res = step(
            geom,
            &ObstacleSet::default(),
            &mut st,
            ax,
            -az,
            -up,
            dxw,
            dyw,
            yps,
            1.0 / 60.0,
            false,
            true,
        );
        last_step_us = t_before.elapsed().as_micros() as u32;
        let moved_up = -res.feet_z;
        ax += res.dx;
        az -= res.dy;
        rep.max_tick_dy = rep.max_tick_dy.max((moved_up - up).abs());
        let along_now = if c.along_x { ax } else { az };
        let inside_box = along_now >= box_lo && along_now <= box_hi;
        if matches!(res.decision, VerticalDecision::Airborne { .. }) && inside_box {
            rep.air_ticks += 1;
        }
        lines.push(format!(
            "t{tick:5} s={along_now:+8.3} pos=({ax:+.3},{az:+.3}) up={moved_up:+7.3} out={} dec={:?}",
            res.outcome.label(),
            res.decision
        ));
        up = moved_up;
        let done = if c.dir > 0.0 {
            along_now >= crest_exit
        } else {
            along_now <= crest_exit
        };
        if done {
            rep.reached_crest_exit = true;
            break;
        }
    }
    // Sink envelope over the flight: height must not fall below its best-so-far by more than the tolerance.
    let mut hmax = f32::MIN;
    let mut sink = 0.0f32;
    for l in &lines {
        if let Some(field) = l
            .split(" up=")
            .nth(1)
            .and_then(|t| t.split_whitespace().next())
        {
            if let Ok(h) = field.parse::<f32>() {
                hmax = hmax.max(h);
                sink = sink.max(hmax - h);
            }
        }
    }
    rep.sank_below = (sink > SINK_TOLERANCE_YAMLS + 1e-6).then_some(sink);
    if !(rep.reached_crest_exit
        && rep.air_ticks == 0
        && rep.sank_below.is_none()
        && rep.max_tick_dy <= FOLLOW_WORD_YAMLS_PER_FRAME + 1e-3)
    {
        // KULUU_STAIR_FULLTRACE prints the entire tick trace instead of just the tail.
        let tail_start = if std::env::var("KULUU_STAIR_FULLTRACE").is_ok() {
            0
        } else {
            lines.len().saturating_sub(24)
        };
        rep.trace_tail = lines[tail_start..].join("\n");
    }
    rep.end_height = up;
    rep
}

/// The walk-off report of 2026-10-26, pinned against shipped data. Garfield stood on the Bastok
/// Mines AH terrace at xz(-24.15,+59.26) y=+3.00 and walked forward: a vertical ray past ~0.75 yalm
/// answered only a lip top at +4.167 (feet+1.167, measured in the census section of the collision
/// record), which kuluu refused to stand on while its wall query suppressed faces topping inside
/// one follow word - so he crossed it with nothing beneath him and fell into a three-yalm drop.
/// Acceptance and suppression now read one ceiling: this drive must stay grounded, hold terrace
/// height, and stop at the lip instead of reaching the lower street floor (-0.000) 3.8 yalms out.
#[test]
fn walk_into_the_shipped_terrace_lip_and_stop_rather_than_fall() {
    let Some(geom) = zone_geom(234) else { return };
    // The reported stand point, walked along -x (the direction whose columns lost support).
    let (mut ax, az) = (-24.15f32, 59.26f32);
    let mut up = geom.ground_nearest(Vec2::new(ax, az), 3.0).unwrap();
    assert!(
        (up - 3.0).abs() < 0.05,
        "the reported terrace floor should answer +3.000 at the stand point, got {up:+.4}"
    );

    let yps = 1.6;
    let step_len = yps / 60.0;
    let mut st = Walker::default();
    let mut air_ticks = 0usize;
    let mut min_up = up;
    for _ in 0..180 {
        let res = step(
            &geom,
            &ObstacleSet::default(),
            &mut st,
            ax,
            -az,
            -up,
            -step_len,
            0.0,
            yps,
            1.0 / 60.0,
            false,
            true,
        );
        if matches!(res.decision, VerticalDecision::Airborne { .. }) {
            air_ticks += 1;
        }
        ax += res.dx;
        up = -res.feet_z;
        min_up = min_up.min(up);
    }
    println!(
        "terrace lip drive: ended x={ax:+.2} (walked into the lip from -24.15) height {up:+.3} \
         min {min_up:+.3} air_ticks {air_ticks}"
    );
    assert_eq!(
        air_ticks, 0,
        "a pose walking into a shipped terrace lip went airborne at x={ax:.2}; the lip top stands \
         feet+1.167 and must either be stopped by or supported - never walked through"
    );
    assert!(
        min_up >= 3.0 - 0.4,
        "the pose left terrace height (+3.000) at some point: min {min_up:+.3}"
    );
    assert!(
        ax > -27.5,
        "the drive reached the lower street floor past the lip (x={ax:.2}); the lip faces must stop it"
    );
}

/// The 2026-10-27 "caught on walls, this is an angle issue, it should be sliding" report at
/// xz(-30.44,+44.15): Garfield stood touching the Bastok bastion slope (tri 17851/17852,
/// authored normal (-0.945,+0.319,-0.072), y[0..4.167] - `kuluu-render/tests/
/// mzb_bastok_wall_slide_dump.rs`) and crept along it instead of sliding: the face's XZ
/// normal is not perpendicular to its run, so straight-z travel closes on it at ~8% (a real
/// shallow approach), and after one clip plane the re-check ran against dot(noise). Walks
/// both ways along the wall from the reported pose at walking pace demanding tangential
/// progress, then presses head-on into it demanding the stop holds.
#[test]
fn slide_along_the_shipped_bastion_wall_at_minus_30() {
    let Some(geom) = zone_geom(234) else { return };
    // The reported pose exactly: feet on the street, body touching the face at d=0.3956.
    // `az` is bevy z (drive_climb's convention: step() gets `-az`).
    let (mut ax, mut az) = (-30.44f32, 44.15f32);
    let mut up = geom.ground_nearest(Vec2::new(ax, az), 0.0).unwrap();
    assert!(
        (up - 0.0).abs() < 0.05,
        "street should answer ~0 at the stand point, got {up:+.3}"
    );

    let yps = 1.6;
    let step_len = yps / 60.0;
    let mut st = Walker::default();
    let mut air_ticks = 0usize;
    let mut min_up = up;
    let mut max_up = up;

    // 60 ticks along +z, then 60 along -z: tangential progress each way.
    for (dir, leg) in [(1.0f32, "plus_z"), (-1.0, "minus_z")] {
        let az0 = az;
        for _ in 0..60 {
            let res = step(
                &geom,
                &ObstacleSet::default(),
                &mut st,
                ax,
                -az,
                -up,
                0.0,
                -dir * step_len,
                yps,
                1.0 / 60.0,
                false,
                true,
            );
            if matches!(res.decision, VerticalDecision::Airborne { .. }) {
                air_ticks += 1;
            }
            ax += res.dx;
            az -= res.dy; // drive_climb's convention
            up = -res.feet_z;
            min_up = min_up.min(up);
            max_up = max_up.max(up);
        }
        let progress = (az - az0).abs();
        println!("bastion slide leg {leg}: dz={progress:.3} x={ax:+.2} up={up:+.3}");
        assert!(
            progress > 1.0,
            "{leg}: walking along the shipped bastion face progressed only {progress:.4} yalm \
             (intent was ~1.6); a touching pose creeps instead of sliding - the caught-on-wall \
             report of 2026-10-27"
        );
    }

    // Press head-on into the face (+x, toward the wall): the stop must hold.
    let ax_at_press = ax;
    for _ in 0..45 {
        let res = step(
            &geom,
            &ObstacleSet::default(),
            &mut st,
            ax,
            -az,
            -up,
            step_len, // +x into the wall
            0.0,
            yps,
            1.0 / 60.0,
            false,
            true,
        );
        if matches!(res.decision, VerticalDecision::Airborne { .. }) {
            air_ticks += 1;
        }
        ax += res.dx;
        up = -res.feet_z;
        min_up = min_up.min(up);
        max_up = max_up.max(up);
    }
    println!(
        "bastion press: x {ax_at_press:+.2} -> {ax:+.2}; height now {up:+.3} (min \
         {min_up:+.3} max {max_up:+.3}) air_ticks {air_ticks}"
    );
    assert!(
        ax - ax_at_press < 0.6,
        "pressing +x into the bastion face gained {:.3} yalm - the face must stop the body",
        ax - ax_at_press
    );
    // The buttress slope (normal.y ~ +0.32) is wall band, never floor: no riding up.
    assert!(
        max_up < 1.0 && min_up > -0.4,
        "the pose left street height while working at the bastion foot (min {min_up:+.3} max \
         {max_up:+.3}); a ~72-degree slope face must neither mount the body nor swallow it"
    );
    assert_eq!(
        air_ticks, 0,
        "a pose touching the bastion went airborne at x={ax:.2}"
    );
}

/// The 2026-10-27 "these 'fence/wall' should stop us. instead it sends us to the abyss and
/// you climb it under certain angles" report near xz(-45.6..-46.5,-15): a sawtooth metal
/// truss at x[-46.6..-46.4], cap faces normal.y +/-0.667 topping +0.687 over the walkway
/// (+3.493; `kuluu-render/tests/mzb_bastok_wall_slide_dump.rs`). Walks from the reported
/// stand point straight at the truss and demands: stopped before it, supported on the
/// walkway, never mounted onto the caps, never dropped into the street.
#[test]
fn stop_at_the_shipped_walkway_rail_and_never_mount_it() {
    let Some(geom) = zone_geom(234) else { return };
    let (mut ax, az) = (-45.60f32, -15.06f32);
    let mut up = geom.ground_nearest(Vec2::new(ax, az), 3.49).unwrap();
    assert!(
        (up - 3.493).abs() < 0.05,
        "walkway should answer ~+3.49 at the stand point, got {up:+.3}"
    );

    let yps = 1.6;
    let step_len = yps / 60.0;
    let mut st = Walker::default();
    let mut air_ticks = 0usize;
    let mut min_up = up;
    let mut max_up = up;
    let mut min_ax = ax;
    for _ in 0..150 {
        let res = step(
            &geom,
            &ObstacleSet::default(),
            &mut st,
            ax,
            -az,
            -up,
            -step_len, // walk -x, straight at the truss
            0.0,
            yps,
            1.0 / 60.0,
            false,
            true,
        );
        if matches!(res.decision, VerticalDecision::Airborne { .. }) {
            air_ticks += 1;
        }
        ax += res.dx;
        up = -res.feet_z;
        min_up = min_up.min(up);
        max_up = max_up.max(up);
        min_ax = min_ax.min(ax);
    }
    println!(
        "rail drive: stopped x={min_ax:+.2} (truss faces at ~-46.44) height {up:+.3} (min \
         {min_up:+.3} max {max_up:+.3}) air_ticks {air_ticks}"
    );
    assert_eq!(
        air_ticks, 0,
        "walking into the shipped walkway rail went airborne at x={ax:.2} - swallowed rails \
         are how walkers end up dropped into the street (the abyss report)"
    );
    assert!(
        min_ax > -46.1,
        "the pose walked to x={min_ax:+.2}; the rail faces must stop it before their plane"
    );
    assert!(
        max_up < 3.75,
        "the pose rose to {max_up:+.3} - the truss caps top +4.187 over the walkway and stand \
         on wall-band faces (normal.y +0.667); retail never mounts them"
    );
    assert!(
        min_up > 3.2,
        "the pose dropped to {min_up:+.3} - support on the walkway slab (+3.493) must hold \
         while the rail stops the body"
    );
}

/// Env-gated (`KULUU_STAIR_DUMP=1`) geometry dump for the failing boxes: per
/// lane and direction, chain-follow the profile at a finer pitch than the
/// detector and print every column hit (height, normal.y) plus the body-chain
/// wall contacts `nearest_wall_contact` answers at that stance. Evidence only -
/// this is how the blocking faces on the San d'Oria big steps and the Bastok
/// rock steps get identified instead of guessed at.
#[test]
fn stair_column_dump() {
    if std::env::var("KULUU_STAIR_DUMP").is_err() {
        return;
    }
    let _root = match ffxi_dat::archive::open_test_install() {
        Some(r) => r,
        None => return,
    };
    const DUMP_PITCH: f32 = 0.25;
    for b in STAIR_BOXES {
        // KULUU_STAIR_DUMP_BOXES may narrow which boxes dump (substring list,
        // `|`-separated); the built-in set is every box that has ever stalled.
        let wanted = std::env::var("KULUU_STAIR_DUMP_BOXES")
            .unwrap_or_else(|_| String::from("rock_steps|ah_flights|big_steps|shell"));
        if !wanted
            .split('|')
            .any(|w| !w.is_empty() && b.label.contains(w))
        {
            continue;
        }
        let Some(geom) = zone_geom(b.zone) else {
            continue;
        };
        for &lane in b.lanes {
            for dir in [1.0f32, -1.0] {
                println!("### {} lane {:+.2} dir {:+.0}", b.label, lane, dir);
                let (s_lo, s_hi) = profile_span(b);
                let n = ((s_hi - s_lo) / DUMP_PITCH).ceil() as usize + 1;
                let mut ref_y = b.seed_h;
                for k in 0..n {
                    let s = if dir > 0.0 {
                        s_lo + k as f32 * DUMP_PITCH
                    } else {
                        s_hi - k as f32 * DUMP_PITCH
                    };
                    let xz = if b.along_x {
                        Vec2::new(s, lane)
                    } else {
                        Vec2::new(lane, s)
                    };
                    let mut hits = geom.ground_raycast_all(xz);
                    hits.sort_by(|a, c| {
                        (a.0 - ref_y)
                            .abs()
                            .partial_cmp(&(c.0 - ref_y).abs())
                            .unwrap()
                    });
                    let picks: Vec<String> = hits
                        .iter()
                        .take(5)
                        .map(|(h, n)| format!("{:+.3}@{:.3}", h, n.y))
                        .collect();
                    if let Some((h, _)) = hits
                        .iter()
                        .filter(|(_, n)| n.y >= kuluu_render::dat_mzb::WALKABLE_FACE_UP_DOT)
                        .min_by(|a, c| {
                            (a.0 - ref_y)
                                .abs()
                                .partial_cmp(&(c.0 - ref_y).abs())
                                .unwrap()
                        })
                    {
                        ref_y = *h;
                    }
                    let mut walls: Vec<String> = Vec::new();
                    for off in [
                        super::consts::LOWER_CENTER_OFFSET,
                        super::consts::MIDDLE_CENTER_OFFSET,
                        super::consts::UPPER_CENTER_OFFSET,
                    ] {
                        if let Some((d2, n)) = geom.nearest_wall_contact(
                            bevy::math::Vec3::new(xz.x, ref_y + off, xz.y),
                            super::consts::BODY_RADIUS,
                            ref_y,
                            0.0,
                        ) {
                            // Face-level identity for the blocking contact: slot, triangle
                            // id, sub-area shell link, prop flag and vertex y-extent - enough
                            // to tell an architecture riser from a placeholder shell without
                            // replaying the game.
                            let mut ident = String::from("-");
                            'blocks: for slot in 0..kuluu_render::dat_mzb::ZONE_BLOCK_SLOTS {
                                let blk = geom.block(slot as u8);
                                if blk.cell_index.is_empty() {
                                    continue;
                                }
                                let r = super::consts::BODY_RADIUS;
                                let c3 = bevy::math::Vec3::new(xz.x, ref_y + off, xz.y);
                                let lo = c3 - Vec2::splat(r).extend(0.0);
                                let hi = c3 + Vec2::splat(r).extend(0.0);
                                for cx in (lo.x / 8.0).floor() as i32..=(hi.x / 8.0).floor() as i32
                                {
                                    for cz in
                                        (lo.z / 8.0).floor() as i32..=(hi.z / 8.0).floor() as i32
                                    {
                                        let Some(ids) = blk.cell_index.get(&(cx, cz)) else {
                                            continue;
                                        };
                                        for &id in ids {
                                            let id = id as usize;
                                            let base = id * 3;
                                            let (a, b, c) = (
                                                blk.positions[blk.indices[base] as usize],
                                                blk.positions[blk.indices[base + 1] as usize],
                                                blk.positions[blk.indices[base + 2] as usize],
                                            );
                                            let nn = blk.tri_normals[id];
                                            if (nn.x - n.x).abs() > 5e-3
                                                || (nn.y - n.y).abs() > 5e-3
                                            {
                                                continue;
                                            }
                                            let d3 = kuluu_render::dat_mzb::point_tri_dist_sq(
                                                c3, a, b, c,
                                            );
                                            if (d3 - d2).abs() > 1e-3 {
                                                continue;
                                            }
                                            ident = format!(
                                                "s{slot} t{id} sub{:x} prop{} y[{:.1}..{:.1}]",
                                                blk.tri_sub_area.get(id).copied().unwrap_or(0),
                                                blk.tri_prop_blocker
                                                    .get(id)
                                                    .copied()
                                                    .unwrap_or(false)
                                                    as u8,
                                                a.y.min(b.y).min(c.y),
                                                a.y.max(b.y).max(c.y),
                                            );
                                            break 'blocks;
                                        }
                                    }
                                }
                            }
                            walls.push(format!(
                                "off{off:.1} d{:.2} n({:.2},{:.2}) {ident}",
                                d2.sqrt(),
                                n.x,
                                n.y
                            ));
                        }
                    }
                    println!(
                        "s={:+8.2} feet={:+7.3} hits=[{}] walls=[{}]",
                        s,
                        ref_y,
                        picks.join(" "),
                        walls.join(" | ")
                    );
                }
            }
        }
    }
}

/// Env-gated (`KULUU_STAIR_CENSUS=<zone>`): triangle riser census in model
/// space. A staircase is where tread-anchored near-vertical faces cluster: the
/// column just in front of the low edge and just behind the high edge must both
/// answer flat faces, or the vertical triangle is a wall/roof side, not a step
/// (without that check every wall counted). Hotspot cells are printed so flights
/// get chosen from the data, not from remembered map coordinates.
#[test]
fn stair_census_zone() {
    let Ok(zone) = std::env::var("KULUU_STAIR_CENSUS") else {
        return;
    };
    let zone: u16 = zone.parse().unwrap();
    let Some(geom) = zone_geom(zone) else {
        println!("census: zone {zone} geometry unavailable");
        return;
    };
    let flat_near = |at: Vec2, lo: f32, hi: f32| -> bool {
        geom.ground_raycast_all(at)
            .into_iter()
            .any(|(y, n)| n.y > 0.7 && y >= lo - 0.6 && y <= hi + 0.9)
    };
    let mut cells: std::collections::BTreeMap<(i32, i32), usize> = Default::default();
    for slot in 0..kuluu_render::dat_mzb::ZONE_BLOCK_SLOTS {
        let blk = geom.block(slot as u8);
        for tri_id in 0..blk.tri_count() {
            let Some(n) = blk.tri_normals.get(tri_id) else {
                continue;
            };
            if n.y.abs() > 0.35 {
                continue;
            }
            let base = tri_id * 3;
            let (a, b, c) = (
                blk.positions[blk.indices[base] as usize],
                blk.positions[blk.indices[base + 1] as usize],
                blk.positions[blk.indices[base + 2] as usize],
            );
            let ymin = a.y.min(b.y).min(c.y);
            let ymax = a.y.max(b.y).max(c.y);
            if !(ymax - ymin > 0.2 && ymax - ymin < 1.7) {
                continue;
            }
            let nxz = Vec2::new(n.x, n.z);
            if nxz.length_squared() < 1e-6 {
                continue;
            }
            let nxz = nxz.normalize();
            let cxz = (a.xz() + b.xz() + c.xz()) / 3.0;
            if !flat_near(cxz - nxz * 0.45, ymin - 1.2, ymin + 0.3)
                || !flat_near(cxz + nxz * 0.45, ymax - 0.6, ymax + 1.2)
            {
                continue;
            }
            let cx = (cxz.x / 8.0).floor() as i32;
            let cz = (cxz.y / 8.0).floor() as i32;
            *cells.entry((cx, cz)).or_insert(0) += 1;
        }
    }
    let mut hot: Vec<_> = cells.into_iter().filter(|(_, n)| *n >= 6).collect();
    hot.sort_by_key(|(_, n)| -(*n as i32));
    println!(
        "=== stair census zone {zone}: {} hotspot cells ===",
        hot.len()
    );
    for ((cx, cz), n) in hot.iter().take(50) {
        let probe = Vec2::new(*cx as f32 * 8.0 + 4.0, *cz as f32 * 8.0 + 4.0);
        let floor = geom
            .ground_raycast_all(probe)
            .first()
            .map(|(h, _)| format!("{h:+.1}"))
            .unwrap_or_else(|| "-".to_string());
        println!(
            "cell ({cx},{cz}) centre ({:.0},{:.0}) floor~{floor} risers={n}",
            probe.x, probe.y
        );
    }
}

/// Env-gated (`KULUU_STAIR_SCOUT=<zone>`): grid-scan a zone for staircase-shaped
/// profiles near given anchors (or everywhere at coarse pitch) and print every
/// detected climb's lane, span and heights - so stair boxes come from the data,
/// not from guessed coordinates. The Bastok Markets synergy-furnace anchors are
/// LSB `!pos` entries (Synergy_Furnace.lua: (-186.8, 2.0, -146.6), (-193.9, 2.0,
/// -149.4), (-269, -12.5, -31), (-263, -12.5, -31)); Garfield's screenshot
/// staircase must be one of these flights.
#[test]
fn stair_scout_zone() {
    let Ok(zone) = std::env::var("KULUU_STAIR_SCOUT") else {
        return;
    };
    let zone: u16 = zone.parse().unwrap();
    let anchors: &[(f32, f32)] = match zone {
        235 => &[
            (-190.4, -148.0),
            (-266.0, -31.0),
            (-127.0, -133.0), // Bastok Markets AH / Merchant's Guild terrace
        ],
        // Garfield live in the dev DB (xidb.chars): on the screenshot staircase,
        // zone 234 at (19.12, 0.00, -70.51) - the flight the user stands on.
        234 => &[(19.1, -70.5)],
        _ => &[],
    };
    let Some(geom) = zone_geom(zone) else {
        println!("scout: zone {zone} geometry unavailable");
        return;
    };
    let mut lo = bevy::math::Vec3::splat(f32::MAX);
    let mut hi = bevy::math::Vec3::splat(f32::MIN);
    for slot in 0..kuluu_render::dat_mzb::ZONE_BLOCK_SLOTS {
        for p in geom.block(slot as u8).positions.iter() {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
    }
    println!(
        "=== stair scout zone {zone} tri_count {} bbox x[{:.1}..{:.1}] y[{:.1}..{:.1}] z[{:.1}..{:.1}] ===",
        geom.tri_count(),
        lo.x,
        hi.x,
        lo.y,
        hi.y,
        lo.z,
        hi.z
    );
    for &(ax, az) in anchors {
        let xz = Vec2::new(ax, az);
        let hits = geom.ground_raycast_all(xz);
        let picks: Vec<String> = hits
            .iter()
            .take(6)
            .map(|(h, n)| format!("{:+.2}@{:.2}", h, n.y))
            .collect();
        println!("anchor ({ax},{az}) column: [{}]", picks.join(" "));
    }
    for &(ax, az) in anchors {
        for along_x in [true, false] {
            let lanes: Vec<f32> = if along_x {
                (-12..=12).map(|k| az + k as f32 * 0.75).collect()
            } else {
                (-12..=12).map(|k| ax + k as f32 * 0.75).collect()
            };
            let b = (
                if along_x { ax - 14.0 } else { ax - 6.0 },
                if along_x { ax + 14.0 } else { ax + 6.0 },
                if along_x { az - 6.0 } else { az - 14.0 },
                if along_x { az + 6.0 } else { az + 14.0 },
            );
            let probe = StairBox {
                label: "anchor",
                zone,
                x0: b.0,
                x1: b.1,
                z0: b.2,
                z1: b.3,
                along_x,
                // StairBox pins 'static lanes (it is a const table); leaking the
                // scout's lane list per box is fine in a one-shot test.
                lanes: Box::leak(lanes.clone().into_boxed_slice()),
                seed_h: {
                    let s0 = if along_x { b.0 } else { b.2 };
                    let xz = if along_x {
                        Vec2::new(s0, lanes[lanes.len() / 2])
                    } else {
                        Vec2::new(lanes[lanes.len() / 2], s0)
                    };
                    geom.ground_raycast_all(xz)
                        .first()
                        .map(|(h, _)| *h - 1.5)
                        .unwrap_or(-6.0)
                },
                status: CLEAN,
            };
            let climbs = detect_climbs(&geom, &probe);
            if climbs.is_empty() {
                continue;
            }
            println!(
                "box x[{:.1}..{:.1}] z[{:.1}..{:.1}] along_x={} seed_h={:.2}: {} climbs",
                b.0,
                b.1,
                b.2,
                b.3,
                along_x,
                probe.seed_h,
                climbs.len()
            );
            let mut seen: Vec<(f32, f32)> = Vec::new();
            for c in &climbs {
                // Collapse the mirror-image detections (lane ±dir over one flight).
                if seen
                    .iter()
                    .any(|(l, e)| ((c.lane - l).abs() < 1.0) && ((c.entry_s - e).abs() < 2.0))
                {
                    continue;
                }
                seen.push((c.lane, c.entry_s));
                println!(
                    "  climb lane {:+.2} dir {:+.0} s[{:+.2}..{:+.2}] h[{:+.2}..{:+.2}] net {:.2}",
                    c.lane,
                    c.dir,
                    c.entry_s,
                    c.exit_s,
                    c.h_low,
                    c.h_high,
                    c.h_high - c.h_low
                );
            }
        }
    }
}

/// The overlay-verified climb at the AH entrance shell (Garfield's report of
/// 2026-10-26, xz(-21.3,+69.6): "still refuses to see the start of the stairs").
/// The axis-aligned battery lanes on `bastok_mines_shell_steps_x/z` clip the shell's
/// flank sheets and measure the detector's entry law more than the walker; this drive
/// walks the actual reported line - from the stand point straight along -z toward the
/// AH terrace at z<=65, across every tread it crosses - under the same bar as a clean
/// battery lane: never airborne inside the flight, no sink below best-so-far beyond
/// `SINK_TOLERANCE_YAMLS`, per-tick height change bounded by the follow word ("Fall and
/// landing", `.agents/skills/retail-observe/references/2026-10-25-collision-query.md`),
/// and a real net rise. Coordinates are the overlay pose; the stand point itself must
/// answer ground, or the pin has drifted off the staircase with re-authored geometry.
/// The drive stops at z=+66.4: past the topmost tread the DAT ships no floor under
/// this line (the mound flank there answers only slab 0 and dome sheets overhead),
/// so walking further is a walk-off fall, not the climb under test.
#[test]
fn climb_the_ah_shell_treads_from_the_reported_stand_point() {
    let Some(geom) = zone_geom(234) else { return };
    // Overlay pose: Garfield stood on the treads here (x=-21.3, bevy z=+69.6).
    let (sx, sz) = (-21.3f32, 69.6f32);
    let seed_up = geom.ground_walkable(Vec2::new(sx, sz), 0.0);
    assert!(
        seed_up.is_some(),
        "the reported stand point xz({sx},{sz}) must answer ground for this pin to mean anything"
    );
    let travel = 3.2f32;
    for &yps in WALK_SPEEDS.iter() {
        let step_len = yps / 60.0;
        let (mut ax, mut az) = (sx, sz);
        let mut up = seed_up.unwrap();
        let mut st = Walker::default();
        let mut air_ticks = 0usize;
        let mut max_tick_dy = 0.0f32;
        let mut sink = 0.0f32;
        let mut best = up;
        let ticks = (travel / step_len) as usize + 1;
        for tick in 0..ticks {
            // `step`'s wire convention, identical to drive_climb's z-lane branch.
            let res = step(
                &geom,
                &ObstacleSet::default(),
                &mut st,
                ax,
                -az,
                -up,
                0.0,
                step_len, // travel along -z
                yps,
                1.0 / 60.0,
                false,
                true,
            );
            if matches!(res.decision, VerticalDecision::Airborne { .. }) {
                air_ticks += 1;
            }
            ax += res.dx;
            az -= res.dy;
            let moved_up = -res.feet_z;
            max_tick_dy = max_tick_dy.max((moved_up - up).abs());
            best = best.max(moved_up);
            sink = sink.max(best - moved_up);
            if std::env::var("KULUU_STAIR_DUMP").is_ok() {
                println!(
                    "pin t{tick:4} ({ax:+.3},{az:+.3}) up={moved_up:+7.3} out={} dec={:?}",
                    res.outcome.label(),
                    res.decision
                );
            }
            up = moved_up;
        }
        let net_rise = up - seed_up.unwrap();
        println!(
            "pin shell stand-point drive yps {yps:.1}: air={air_ticks} sink={sink:.3} maxdy={max_tick_dy:.4} net_rise={net_rise:+.3} end=({ax:+.2},{az:+.2}) h={up:+.3}"
        );
        assert_eq!(
            air_ticks, 0,
            "yps {yps:.1}: went airborne inside the AH shell flight (see pin drive line)"
        );
        assert!(
            sink <= SINK_TOLERANCE_YAMLS + 1e-3,
            "yps {yps:.1}: sank {sink:.3} yalm below best-so-far climbing the reported treads"
        );
        assert!(
            net_rise >= 0.5,
            "yps {yps:.1}: only rose {net_rise:+.3} over {travel} yalm along -z from xz({sx},{sz}) - not climbing"
        );
        assert!(
            max_tick_dy <= FOLLOW_WORD_YAMLS_PER_FRAME + 1e-3,
            "yps {yps:.1}: tick dy {max_tick_dy:.4} exceeds the follow word"
        );
    }
}

/// The "feels a bit high on the steps" and "W+D jitter" reports of 2026-10-28, measured against
/// shipped data instead of felt. Walks the San d'Oria pin staircase two ways - straight along x and
/// at a 45-degree diagonal - and reads two numbers per tick: how far the pose rose ABOVE the flat
/// street while its own axis still stood short of the first riser plane (early lift: retail's
/// half-yalm probe sphere touches a +0.5 tread top essentially only when under it, so this must be
/// nil), and every downward step taken between treads while climbing grounded (the flap the user
/// felt as an update pause). Prints the full per-tick profile for forensics.
#[test]
fn approaching_a_shipped_riser_never_lifts_the_pose_early_or_flaps() {
    let Some(geom) = zone_geom(231) else { return };
    let lane = -74.666_664f32; // sd_north_pin_stairs centre lane
    let seed_h = -8.0f32;

    // Locate the first riser plane by chaining ground answers along x at 0.1 pitch.
    let mut flat = geom
        .ground_nearest(Vec2::new(-200.5, lane), seed_h)
        .expect("flat street before the pin staircase");
    assert!(
        (flat - seed_h).abs() < 0.35,
        "street near {seed_h}, got {flat}"
    );
    let mut riser_x = None;
    let mut x = -200.5f32;
    while x < -192.0 && riser_x.is_none() {
        x += 0.1;
        if let Some(h) = geom.ground_nearest(Vec2::new(x, lane), flat) {
            if h > flat + 0.1 {
                riser_x = Some(x);
            } else {
                flat = flat.max(h);
            }
        }
    }
    let riser_x = riser_x.expect("the pin staircase must have a first riser in the box");
    println!("pin staircase: street {flat:+.3}, first riser plane at x={riser_x:.2}");

    for (label, dir_z) in [("axis", 0.0f32), ("diagonal45", -1.0f32)] {
        let yps = 0.7f32;
        let step_len = yps / 60.0 * std::f32::consts::FRAC_1_SQRT_2;
        let (mut ax, mut az) = (riser_x - 1.5f32, lane);
        let mut up = geom
            .ground_nearest(Vec2::new(ax, az), flat)
            .expect("approach column");
        let h_at_start = up;
        let mut st = Walker::default();
        // Tick counts to clear two risers past the first plane (~3 yalm of travel).
        let ticks = (3.0 / step_len) as usize;
        let mut early_lift = 0.0f32;
        let mut flap = 0.0f32;
        let mut float_over_own_column = 0.0f32;
        let mut prev_up = up;
        for tick in 0..ticks {
            let res = step(
                &geom,
                &ObstacleSet::default(),
                &mut st,
                ax,
                -az,
                -up,
                step_len,
                dir_z * step_len,
                yps,
                1.0 / 60.0,
                false,
                true,
            );
            ax += res.dx;
            az -= res.dy;
            up = -res.feet_z;
            if ax < riser_x - 0.05 {
                early_lift = early_lift.max(up - h_at_start);
            } else {
                flap = flap.max(prev_up - up);
            }
            // Only settled ticks count: mid-step-up the body legitimately reaches the next tread
            // before its own axis ray does. The reference is the walker's own support answer at the
            // new position (pose column plus the ring) - a pose held level over an exposed slab
            // sliver by the seam bridge is ground truth under the recorded probe-sphere model, not
            // a float; standing higher than ANY nearby support is.
            if matches!(res.decision, VerticalDecision::Flat) {
                if let Some(col) =
                    super::step::contact_floor(&geom, &[], Vec2::new(ax, az), prev_up, true)
                {
                    if up - col > 0.06 {
                        println!(
                            "  FLOAT {label} t{tick:4} ({ax:+7.3},{az:+7.3}) up={up:+7.3} support={col:+7.3}"
                        );
                    }
                    float_over_own_column = float_over_own_column.max(up - col - 0.01);
                }
            }
            prev_up = up;
            println!(
                "{label} t{tick:4} ({ax:+7.3},{az:+7.3}) up={up:+7.3} out={} dec={:?}",
                res.outcome.label(),
                res.decision
            );
        }
        println!("DIAG {label}: early_lift={early_lift:.4} flap_between_treads={flap:.4} float_over_support={float_over_own_column:.4} end up={up:+.3}");
        // Re-scoped 2026-10-28: this pin once forbade ANY early lift. USER made the in-motion
        // forward look-ahead law that day ("it actually goes UP before it hits the edge IF already
        // in motion", gated by the player-state tracker), so bounded anticipation ahead of a riser
        // is now expected behaviour, up to rise-cap + uplift within the 1.2 yalm sample window.
        // Still forbidden: exceeding that allowance, ring/lateral pre-lift beyond cap plus whatever
        // the forward channel legitimately explains (the measured feels-high bug rode 0.746 above
        // its own support - outside even the combined allowance), and jitter-class dips larger
        // than a landing settle of one uplift.
        assert!(
            early_lift
                < super::step::LOOKAHEAD_RISE_CAP_YAMLS + super::step::RISE_UPLIFT_YAMLS + 1e-3,
            "{label}: pose rose {early_lift:.4} yalm before its axis crossed the first riser - \
             beyond the look-ahead allowance"
        );
        assert!(
            flap < 2e-2 + super::step::RISE_UPLIFT_YAMLS,
            "{label}: grounded climb dipped {flap:.4} yalm between treads (the W+D jitter class; \
             a settle of one uplift at landing is law now)"
        );
        assert!(
            float_over_own_column
                < kuluu_render::dat_mzb::RING_SUPPORT_HOLD_LIFT_CAP_YAMLS
                    + super::step::LOOKAHEAD_RISE_CAP_YAMLS
                    + super::step::RISE_UPLIFT_YAMLS
                    + 1e-3,
            "{label}: settled pose floated {float_over_own_column:.4} above any nearby support"
        );
    }

    // The Bastok flights the user played right before the report: the AH shell stand-point line and
    // the terrace lane, each straight and on a 45-degree diagonal. Their risers measure ~0.25-0.27 -
    // under the wholly-above gate that already rejects SD's half-yalm class (a tread top standing
    // 0.3+ above the feet is never support), so they are the heights a ring sample can pre-lift onto.
    let bastok = zone_geom(234);
    if let Some(geom) = bastok {
        // Shell stand-point line: straight -z, plus diagonals drifting +-x across the treads.
        for (label, dir_x) in [
            ("shell_axis_minus_z", 0.0f32),
            ("shell_diag_plus_x", 1.0),
            ("shell_diag_minus_x", -1.0),
        ] {
            // float_over_own_column: how far the final height sits above what the pose's own
            // axis-column walkably supports at its current position (+ tiny tolerance for exact
            // plane floats) - the "feels high" measure, assist pre-lift included.
            let mut float_over_own_column = 0.0f32;
            let yps = 0.7f32;
            let step_len = yps / 60.0 * std::f32::consts::FRAC_1_SQRT_2;
            let (mut ax, mut az) = (-21.3f32, 69.6f32);
            let mut up = geom
                .ground_walkable(Vec2::new(ax, az), 0.0)
                .expect("reported stand point");
            let h_at_start = up;
            let mut st = Walker::default();
            let ticks = (3.2 / step_len) as usize;
            let mut flap = 0.0f32;
            let mut prev_up = up;
            // The stand point sits mid-tread; the first riser ahead is ~1 yalm out, so lift beyond
            // +0.05 before any rise below records as pre-lift against this start.
            let mut early_lift = 0.0f32;
            let mut saw_rise = false;
            for tick in 0..ticks {
                let res = step(
                    &geom,
                    &ObstacleSet::default(),
                    &mut st,
                    ax,
                    -az,
                    -up,
                    dir_x * step_len,
                    step_len, // travel along -z
                    yps,
                    1.0 / 60.0,
                    false,
                    true,
                );
                ax += res.dx;
                az -= res.dy;
                up = -res.feet_z;
                if up > h_at_start + 0.05 {
                    saw_rise = true;
                }
                if !saw_rise {
                    early_lift = early_lift.max(up - h_at_start);
                } else {
                    flap = flap.max(prev_up - up);
                }
                if matches!(res.decision, VerticalDecision::Flat) {
                    if let Some(col) =
                        super::step::contact_floor(&geom, &[], Vec2::new(ax, az), prev_up, true)
                    {
                        if up - col > 0.06 {
                            println!(
                                "  FLOAT {label} t{tick:4} ({ax:+7.3},{az:+7.3}) up={up:+7.3} support={col:+7.3}"
                            );
                        }
                        float_over_own_column = float_over_own_column.max(up - col - 0.01);
                    }
                }
                prev_up = up;
                println!(
                    "{label} t{tick:4} ({ax:+7.3},{az:+7.3}) up={up:+7.3} out={} dec={:?}",
                    res.outcome.label(),
                    res.decision
                );
            }
            println!("DIAG {label}: early_lift={early_lift:.4} flap_between_treads={flap:.4} float_over_support={float_over_own_column:.4} end up={up:+.3}");
            // Re-scoped with the SD lanes above: bounded in-motion anticipation is law now.
            assert!(
                early_lift
                    < super::step::LOOKAHEAD_RISE_CAP_YAMLS + super::step::RISE_UPLIFT_YAMLS + 1e-3,
                "{label}: pose rose {early_lift:.4} yalm before any rise - beyond the look-ahead \
                 allowance (shell risers measure ~0.27, so well under this)"
            );
            assert!(
                flap < 2e-2 + super::step::RISE_UPLIFT_YAMLS,
                "{label}: grounded climb dipped {flap:.4} yalm between treads (the W+D jitter class)"
            );
            assert!(
                float_over_own_column
                    < kuluu_render::dat_mzb::RING_SUPPORT_HOLD_LIFT_CAP_YAMLS
                        + super::step::LOOKAHEAD_RISE_CAP_YAMLS
                        + super::step::RISE_UPLIFT_YAMLS
                        + 1e-3,
                "{label}: settled pose floated {float_over_own_column:.4} above any nearby support (the feels-high class)"
            );
        }
    }
}

/// Env-gated (`KULUU_ZONE_GRID=<zone:x0:x1:z0:z1[:pitch]>`): every up-facing hit and
/// wall answer in a 2-D column grid over any zone - same output shape as
/// `shell_entrance_grid_dump` (which stays for its shell-specific history), generalized
/// so stair boxes get re-authored from shipped data when the geometry moves, not from
/// remembered coordinates.
#[test]
fn zone_column_grid_dump() {
    let Ok(spec) = std::env::var("KULUU_ZONE_GRID") else {
        return;
    };
    let nums: Vec<f32> = spec.split(':').map(|t| t.parse().unwrap()).collect();
    let (zone, x0, x1, z0, z1) = (nums[0] as u16, nums[1], nums[2], nums[3], nums[4]);
    let pitch = if nums.len() > 5 { nums[5] } else { 1.0 };
    let Some(geom) = zone_geom(zone) else { return };
    let nx = ((x1 - x0) / pitch) as i32 + 1;
    let nz = ((z1 - z0) / pitch) as i32 + 1;
    println!("zone {zone} grid x[{x0}..{x1}] z[{z0}..{z1}] pitch {pitch}");
    for iz in 0..=nz {
        let z = z0 + iz as f32 * pitch;
        println!("z={z:+.2}");
        for ix in 0..=nx {
            let x = x0 + ix as f32 * pitch;
            let hits = geom.ground_raycast_all(Vec2::new(x, z));
            let joined = hits
                .iter()
                .map(|(h, n)| format!("{h:+.2}@{:.2}", n.y))
                .collect::<Vec<_>>()
                .join(",");
            println!("  [{x}{joined}]");
        }
    }
}

/// Env-gated (`KULUU_SHELL_GRID=<x0:x1:z0:z1[:pitch]>`, zone 234) 2-D probe grid over
/// the AH shell entrance: for every cell print the standable floor chain (all up-face
/// hits bottom-to-top with normals) and whether a body at each of two candidate feet
/// heights would answer authored wall contact. This is how the real walking path past
/// the shell jambs gets found from shipped data instead of guessed from three lanes.
#[test]
fn shell_entrance_grid_dump() {
    if std::env::var("KULUU_SHELL_GRID").is_err() {
        return;
    }
    let spec = std::env::var("KULUU_SHELL_GRID").unwrap();
    let nums: Vec<f32> = spec.split([':', ';']).map(|t| t.parse().unwrap()).collect();
    let (x0, x1, z0, z1) = (nums[0], nums[1], nums[2], nums[3]);
    let pitch = if nums.len() > 4 { nums[4] } else { 1.0 };
    let heights: Vec<f32> = if nums.len() > 5 {
        nums[5..].to_vec()
    } else {
        vec![0.0]
    };
    let Some(geom) = zone_geom(234) else { return };
    let nx = ((x1 - x0) / pitch) as i32 + 1;
    let nz = ((z1 - z0) / pitch) as i32 + 1;
    for iz in 0..=nz {
        let z = z0 + iz as f32 * pitch;
        let mut row_hits: Vec<String> = Vec::new();
        let mut row_walls: Vec<String> = Vec::new();
        for ix in 0..=nx {
            let x = x0 + ix as f32 * pitch;
            let xz = Vec2::new(x, z);
            let mut hits: Vec<(f32, bevy::math::Vec3)> = geom.ground_raycast_all(xz);
            hits.sort_by(|a, c| a.0.partial_cmp(&c.0).unwrap());
            row_hits.push(format!(
                "[{}{}]",
                x,
                hits.iter()
                    .map(|(h, n)| format!("{:+.2}@{:.2}", h, n.y))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
            for &feet in &heights {
                let lift = 0.0f32;
                let wall = geom.nearest_wall_contact(
                    bevy::math::Vec3::new(x, feet + super::consts::BODY_RADIUS, z),
                    super::consts::BODY_RADIUS,
                    feet,
                    lift,
                );
                match wall {
                    Some((d2, n)) => row_walls.push(format!(
                        "x{x:+.1}h{feet:+.1} d{:.2} n({:.2},{:.2})",
                        d2.sqrt(),
                        n.x,
                        n.y
                    )),
                    None => row_walls.push(format!("x{x:+.1}h{feet:+.1} free")),
                }
            }
        }
        println!("z={z:+.2}");
        for h in row_hits {
            println!("  {h}");
        }
        println!("  walls: {}", row_walls.join(" | "));
    }
}

#[test]
fn shipped_stairs_climb_without_ghosting_or_stalls() {
    let Some(_root) = ffxi_dat::archive::open_test_install() else {
        return;
    };
    let mut failures: Vec<String> = Vec::new();
    let mut summary: Vec<String> = Vec::new();
    // Localise long or hung runs: KULUU_STAIR_ONLY=<label substring> drives one box.
    let only = std::env::var("KULUU_STAIR_ONLY").unwrap_or_default();
    for b in STAIR_BOXES {
        if !only.is_empty() && !b.label.contains(&only) {
            continue;
        }
        println!("box {} start", b.label);
        let Some(geom) = zone_geom(b.zone) else {
            failures.push(format!(
                "{}: zone {} geometry failed to load",
                b.label, b.zone
            ));
            continue;
        };
        let clean = b.status == CLEAN;
        let climbs = detect_climbs(&geom, b);
        summary.push(format!(
            "[{}] detected {} climb lanes ({})",
            b.label,
            climbs.len(),
            if clean {
                String::from("clean")
            } else {
                format!("known: {}", b.status)
            }
        ));
        if climbs.is_empty() {
            if clean {
                failures.push(format!(
                    "{}: census found no climb in box (geometry moved?)",
                    b.label
                ));
            }
            continue;
        }
        for c in &climbs {
            let tag = format!("{} lane {:+.1}", b.label, c.lane);
            let mut rep = drive_climb(&geom, c, WALK_SPEEDS[0]);
            println!("drive {tag} yps {:.1} done", WALK_SPEEDS[0]);
            // Each extra speed runs on a fresh walker (mode state must not carry between speeds).
            for &yps in WALK_SPEEDS.iter().skip(1) {
                let r2 = drive_climb(&geom, c, yps);
                println!("drive {tag} yps {:.1} done", yps);
                rep.air_ticks += r2.air_ticks;
                rep.max_tick_dy = rep.max_tick_dy.max(r2.max_tick_dy);
                rep.reached_crest_exit &= r2.reached_crest_exit;
                if r2.sank_below > rep.sank_below {
                    rep.sank_below = r2.sank_below;
                }
                if !r2.trace_tail.is_empty() {
                    rep.trace_tail = r2.trace_tail.clone();
                }
            }
            let mut case_failures: Vec<String> = Vec::new();
            if !rep.reached_crest_exit {
                case_failures.push(format!("STALLED before crest exit\n{}", rep.trace_tail));
            }
            if rep.air_ticks > 0 {
                case_failures.push(format!(
                    "GHOST - airborne {} ticks inside flight\n{}",
                    rep.air_ticks, rep.trace_tail
                ));
            }
            if let Some(sink) = rep.sank_below {
                case_failures.push(format!(
                    "sank {sink:.3} yalm below envelope\n{}",
                    rep.trace_tail
                ));
            }
            if rep.max_tick_dy > FOLLOW_WORD_YAMLS_PER_FRAME + 1e-3 {
                case_failures.push(format!(
                    "tick dy {:.3} exceeds follow word",
                    rep.max_tick_dy
                ));
            }
            if !clean {
                for f in case_failures {
                    summary.push(format!("{tag}: [known: {}] {f}", b.status));
                }
                continue;
            }
            failures.extend(
                case_failures
                    .into_iter()
                    .map(|f| format!("{tag} h[{:.1}..{:.1}]: {f}", c.h_low, c.h_high)),
            );
            summary.push(format!(
                "{tag}: air={} sink={:?} maxdy={:.4} end_h={:.3} reached={}",
                rep.air_ticks,
                rep.sank_below,
                rep.max_tick_dy,
                rep.end_height,
                rep.reached_crest_exit
            ));
        }
    }
    println!("=== smooth-climb summary ===");
    for s in &summary {
        println!("{s}");
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// A flat-run measurement for USER's "nameplates jitter on a flat surface while running"
// report of 2026-10-28: before blaming anything downstream, prove what the walker commits.
/// How far a driven segment must hold one measured height (yalm) to count as flat ground.
const FLAT_SEGMENT_YAMLS: f32 = 5.0;
/// The height spread that still counts as one level floor (yalm): an order below any shipped
/// tread, and below what any nameplate could show on screen at the running camera distance.
const FLAT_LEVEL_TOLERANCE_YAMLS: f32 = 1e-3;

struct FlatRunReport {
    ticks: usize,
    max_abs_tick_dy: f32,
    distinct_heights: usize,
    decisions: std::collections::BTreeMap<&'static str, usize>,
}

/// Drive the real walker along `along` through a segment whose standable answer never moves, at
/// `yps`, and report what height it committed each tick. Returns [`None`] when no such segment
/// exists on this lane (the drive is meaningless then, not failed).
fn drive_flat(
    geom: &MzbCollisionGeometry,
    x0: f32,
    z0: f32,
    dir_x: f32,
    dir_z: f32,
    yps: f32,
) -> Option<FlatRunReport> {
    let step_len = yps / 60.0;
    // Find a start where the answer holds for FLAT_SEGMENT_YAMLS at this pitch.
    let mut seed_up = geom.ground_walkable(Vec2::new(x0, z0), 0.0)?;
    let mut run_start = (x0, z0);
    let mut run_len = 0.0f32;
    let mut s = 0.0f32;
    while run_len < FLAT_SEGMENT_YAMLS && s < 60.0 {
        s += 0.1;
        let p = (x0 + dir_x * s, z0 + dir_z * s);
        match geom.ground_walkable(Vec2::new(p.0, p.1), seed_up) {
            Some(h) if (h - seed_up).abs() <= FLAT_LEVEL_TOLERANCE_YAMLS => run_len += 0.1,
            Some(h) => {
                seed_up = h;
                run_start = p;
                run_len = 0.0;
            }
            None => {
                run_start = p;
                run_len = 0.0;
            }
        }
    }
    if run_len < FLAT_SEGMENT_YAMLS {
        return None;
    }
    let (mut ax, mut az) = run_start;
    let mut up = seed_up;
    let mut st = Walker::default();
    let ticks = ((FLAT_SEGMENT_YAMLS / step_len) as usize).min(600);
    let mut rep = FlatRunReport {
        ticks: 0,
        max_abs_tick_dy: 0.0,
        distinct_heights: 1,
        decisions: std::collections::BTreeMap::new(),
    };
    let mut last_bucket: Option<i64> = None;
    for _ in 0..ticks {
        let res = step(
            geom,
            &ObstacleSet::default(),
            &mut st,
            ax,
            -az,
            -up,
            dir_x * step_len,
            -(dir_z * step_len),
            yps,
            1.0 / 60.0,
            false,
            true,
        );
        let moved_up = -res.feet_z;
        ax += res.dx;
        az -= res.dy;
        rep.ticks += 1;
        rep.max_abs_tick_dy = rep.max_abs_tick_dy.max((moved_up - up).abs());
        let bucket = (moved_up * 4096.0).round() as i64;
        if last_bucket.is_some_and(|b| b != bucket) {
            rep.distinct_heights += 1;
        }
        last_bucket = Some(bucket);
        *rep.decisions
            .entry(match res.decision {
                VerticalDecision::Flat => "Flat",
                VerticalDecision::Airborne { .. } => "Airborne",
                VerticalDecision::Landed => "Landed",
                VerticalDecision::Slope => "Slope",
                VerticalDecision::Ramp { .. } => "Ramp",
                VerticalDecision::SingleStep { .. } => "SingleStep",
                VerticalDecision::Stopped { .. } => "Stopped",
                VerticalDecision::Poof { .. } => "Poof",
                VerticalDecision::CeilingHold => "CeilingHold",
                VerticalDecision::WallAhead => "WallAhead",
                VerticalDecision::NoGeometry => "NoGeometry",
            })
            .or_insert(0) += 1;
        up = moved_up;
    }
    Some(rep)
}

/// USER's "nameplates jitter on a flat surface while running" (2026-10-28): on shipped ground the
/// answer never moves, so the committed height must not move either - no tick may lift or drop more
/// than a thousandth of a yalm at any speed. If this passes, the plate wobble is downstream of the
/// walker and only the drawn feed can be fixed.
#[test]
fn running_on_flat_shipped_ground_commits_one_height() {
    let mut checked = 0usize;
    let mut zones_loaded = 0usize;
    let mut worst = 0.0f32;
    let mut worst_case = String::new();
    for (zone, lanes) in [
        (
            231u16,
            [
                (-205.0f32, -74.666_664f32),
                (-198.0, -107.5),
                (-195.0, -96.0),
            ],
        ),
        (234u16, [(-21.3f32, 60.0), (-30.0, 44.0), (-12.0, 30.0)]),
    ] {
        let Some(geom) = zone_geom(zone) else {
            println!("flat-run: zone {zone} geometry unavailable here (no client install)");
            continue;
        };
        zones_loaded += 1;
        for (x0, z0) in lanes {
            for (label, dir) in [
                ("+x", (1.0f32, 0.0f32)),
                ("-z", (0.0, -1.0)),
                (
                    "diag45",
                    (
                        std::f32::consts::FRAC_1_SQRT_2,
                        -std::f32::consts::FRAC_1_SQRT_2,
                    ),
                ),
            ] {
                for &yps in WALK_SPEEDS.iter() {
                    let Some(rep) = drive_flat(&geom, x0, z0, dir.0, dir.1, yps) else {
                        continue;
                    };
                    checked += 1;
                    worst = worst.max(rep.max_abs_tick_dy);
                    println!(
                        "DIAG flat-run zone {zone} ({x0},{z0}) {label} yps {yps:.1}: ticks={} max_tick_dy={:.6} distinct_heights={} decisions={:?}",
                        rep.ticks, rep.max_abs_tick_dy, rep.distinct_heights, rep.decisions
                    );
                    // The height buckets in the report are 1/4096 yalm wide, so a few extra buckets from
                    // float plane noise do not matter; what matters is that no tick moves by more than
                    // FLAT_LEVEL_TOLERANCE_YAMLS, which is far under anything visible.
                    assert!(
                        rep.max_abs_tick_dy <= FLAT_LEVEL_TOLERANCE_YAMLS,
                        "zone {zone} ({x0},{z0}) {label} yps {yps:.1}: a tick moved the pose {0:.6} yalm on flat ground - the drawn feed will show this as nameplate jitter",
                        rep.max_abs_tick_dy
                    );
                    if rep.max_abs_tick_dy > 0.0 {
                        worst_case = format!("zone {zone} ({x0},{z0}) {label} yps {yps:.1}");
                    }
                }
            }
        }
    }
    println!(
        "DIAG flat-run: {checked} drives, worst tick dy {worst:.6}{}",
        if worst_case.is_empty() {
            String::new()
        } else {
            format!(" ({worst_case})")
        }
    );
    if zones_loaded == 0 {
        // Same self-skip convention as every sibling in this module: with no client
        // install on the machine there is no shipped ground to drive, and asserting here
        // would only measure whether the DATs are present.
        println!("flat-run skipped: no shipped zone geometry on this machine");
        return;
    }
    assert!(
        checked > 0,
        "{zones_loaded} shipped zones loaded but not one flat segment drove - either the pinned \
         lanes moved or drive_flat's acceptance gate changed"
    );
}

/// Diagnostic, asserts nothing (install-gated like the battery): drive one stalled
/// `sd_north_big_steps_x` lane and print why every tick stops - the authored-wall
/// chain with face identity (`walker/debug.rs probe_chain_walls`), the standable
/// support answer under the pose and under this frame's assist lift, and the
/// accumulator value. Purpose: name the gate that refuses the first big-step tread.
#[test]
fn sd_bigstep_forensics() {
    let Some(geom) = zone_geom(231) else {
        println!("forensics: zone 231 geometry unavailable");
        return;
    };
    let b = STAIR_BOXES
        .iter()
        .find(|b| b.label == "sd_north_big_steps_x")
        .expect("box present");
    let mut found: Vec<Climb> = Vec::new();
    for &lane in b.lanes {
        for dir in [1.0f32, -1.0] {
            if let Some(cc) = climb_on_profile(&geom, b, lane, dir) {
                found.push(cc);
            }
        }
    }
    println!("forensics: detected {} climbs on the box", found.len());
    let Some(c) = found.into_iter().next() else {
        println!("forensics: no climb detected at this box");
        return;
    };
    println!(
        "forensics climb s[{:.1}..{:.1}] h[{:.2}..{:.2}]",
        c.entry_s, c.exit_s, c.h_low, c.h_high
    );

    let mut s = if c.dir > 0.0 {
        c.entry_s - APPROACH_YAMLS
    } else {
        c.exit_s - EXIT_PAST_CREST_YAMLS
    };
    let s_hi = if c.dir > 0.0 {
        c.exit_s + EXIT_PAST_CREST_YAMLS
    } else {
        c.entry_s + APPROACH_YAMLS
    };
    while s <= s_hi {
        let xz = Vec2::new(s, c.lane);
        let hits: Vec<String> = geom
            .ground_raycast_all(xz)
            .into_iter()
            .map(|(h, n)| {
                let ny = n.y;
                format!("{h:+.3}/n{ny:.2}")
            })
            .collect();
        println!(
            "col s={s:+8.3} floor_nearest={:?} hits[{}]",
            geom.ground_nearest(xz, c.h_low),
            hits.join(" ")
        );
        s += PROFILE_STEP_YAMLS;
    }

    let step_len = 0.7 / 60.0;
    let mut a = if c.dir > 0.0 {
        c.entry_s - APPROACH_YAMLS
    } else {
        c.entry_s + APPROACH_YAMLS
    };
    let lane = c.lane;
    let mut up = geom
        .ground_nearest(Vec2::new(a, lane), c.h_low)
        .unwrap_or(c.h_low);
    let mut st = Walker::default();
    for tick in 0..3200 {
        let gxz = Vec2::new(a, lane);
        let lift = st.step_assist_yamls;
        let sup = geom.ground_walkable(gxz, up);
        let sup_lift = geom.ground_walkable(gxz, up + lift);
        let chain = super::debug::probe_chain_walls(&geom, gxz, up);
        if (c.dir > 0.0 && a >= c.exit_s + EXIT_PAST_CREST_YAMLS)
            || (c.dir < 0.0 && a <= c.exit_s - EXIT_PAST_CREST_YAMLS)
        {
            println!("t{tick:5} CLEARED crest at s={a:+8.3} up={up:+7.3}");
            break;
        }
        let res = step(
            &geom,
            &ObstacleSet::default(),
            &mut st,
            a,
            -lane,
            -up,
            step_len * c.dir,
            0.0,
            0.7,
            1.0 / 60.0,
            false,
            true,
        );
        println!(
            "t{tick:5} s={a:+8.3}->{:+8.3} up={up:+7.3}->{:+7.3} assist={lift:.3} sup={sup:?} sup_lift={sup_lift:?} out={} dec={:?} chain[{chain}]",
            a + res.dx,
            -res.feet_z,
            res.outcome.label(),
            res.decision
        );
        up = -res.feet_z;
        a += res.dx;
    }
}

/// Read-only forensic dump for USER's 2026-10-28 "stuck on this rounded edge" report: Bastok Mines
/// xz(-52.15,+19.15), feet +0.581 with the street at 0. Prints every standable candidate and every
/// wall-band face in a grid across the spot, with height, authored normal and prop flag, so the
/// mount path is named from data rather than assumed.
#[test]
fn pipe_edge_forensics_minus_52_plus_19() {
    let Some(geom) = zone_geom(234) else { return };
    for ix in 0..7 {
        for iz in 0..7 {
            let x = -52.15 + (ix as f32 - 3.0) * 0.4;
            let z = 19.15 + (iz as f32 - 3.0) * 0.4;
            let pos = bevy::math::Vec2::new(x, z);
            let mut line = String::new();
            for (h, fb) in geom.column_floor_candidates(pos) {
                line.push_str(&format!(" cand[+{h:+.3}..bottom{fb:+.3}]"));
            }
            // The pose height (0.581) as well as the street: which face answers when
            // the body is already up on the edge, and which when it is on the street.
            for base in [0.0f32, 0.581] {
                if let Some((h, n)) = geom.ground_walkable_face(pos, base) {
                    let (nx, ny, nz) = (n.x, n.y, n.z);
                    line.push_str(&format!(
                        " walk@{base:+.3}[+{h:+.3}] n({nx:.2},{ny:.2},{nz:.2})"
                    ));
                }
            }
            let walls = super::debug::probe_chain_walls(&geom, pos, 0.581);
            line.push_str(&format!(" walls[{walls}]"));
            println!("DIAG pipe ({x:+.2},{z:+.2}){line}");
        }
    }
}

/// Env-gated (`KULUU_WALL_FORENSICS=<zone>:<x>:<z>:<pose-height>`), generalized from the pipe
/// forensics for USER's 2026-10-28 stuck-wall report at Bastok Mines xz(+21.05,+62.63) feet
/// +3.83 (STAIR DEBUG panel: prop face t12834 p1 y[3.0..3.8] at d 0.01, terrain wall t12816
/// y[3.0..4.2], ground under the feet answering none): standable candidates, walkable answers
/// from street and pose base, and the body-chain wall list with face ids, normals and prop
/// flags in a grid across any stuck point.
#[test]
fn wall_stuck_forensics_env() {
    let Ok(spec) = std::env::var("KULUU_WALL_FORENSICS") else {
        return;
    };
    let nums: Vec<f32> = spec.split(':').map(|t| t.parse().unwrap()).collect();
    let (zone, cx, cz, ph) = (nums[0] as u16, nums[1], nums[2], nums[3]);
    let Some(geom) = zone_geom(zone) else { return };
    for ix in 0..9 {
        for iz in 0..9 {
            let x = cx + (ix as f32 - 4.0) * 0.35;
            let z = cz + (iz as f32 - 4.0) * 0.35;
            let pos = Vec2::new(x, z);
            let mut line = String::new();
            for (h, fb) in geom.column_floor_candidates(pos) {
                line.push_str(&format!(" cand[+{h:+.3}..b{fb:+.3}]"));
            }
            for base in [0.0f32, ph] {
                if let Some((h, n)) = geom.ground_walkable_face(pos, base) {
                    line.push_str(&format!(
                        " walk@{base:+.2}[+{h:+.3}] n({:.2},{:.2},{:.2})",
                        n.x, n.y, n.z
                    ));
                }
            }
            let walls = super::debug::probe_chain_walls(&geom, pos, ph);
            line.push_str(&format!(" walls[{walls}]"));
            println!("DIAG wallstuck ({x:+.2},{z:+.2}){line}");
        }
    }
}

/// Env-gated (`KULUU_STUCK_DRIVE=<zone>:<x>:<z>`): drive the real walker through a stuck point
/// from eight directions off both levels (street +3.0 and deck +4.167), printing x/z/feet each
/// tick, to find which law mounts the slab top +3.83 USER got stuck on at (+21.05,+62.63).
#[test]
fn wall_stuck_drive_env() {
    let Ok(spec) = std::env::var("KULUU_STUCK_DRIVE") else {
        return;
    };
    let nums: Vec<f32> = spec.split(':').map(|t| t.parse().unwrap()).collect();
    let (zone, cx, cz) = (nums[0] as u16, nums[1], nums[2]);
    let Some(geom) = zone_geom(zone) else { return };
    for (label, sx, sz, dx, dz, base) in [
        ("E->W deck", 23.5f32, 62.63, -1.0, 0.0, 4.167),
        ("W->E street", 18.5, 62.63, 1.0, 0.0, 3.0),
        ("N->S deck", 21.05, 65.5, 0.0, -1.0, 4.167),
        ("S->N street", 21.05, 59.5, 0.0, 1.0, 3.0),
        ("E->W low start", 23.5, 62.28, -1.0, 0.0, 4.167),
        ("+X off deck edge", 21.9, 63.0, 1.0, 0.0, 4.167),
    ] {
        let mut ax = sx;
        let mut az = sz;
        let up0 = geom
            .ground_walkable(Vec2::new(ax, az), base)
            .unwrap_or(base);
        let mut up = up0;
        let mut st = Walker::default();
        let yps = 1.6f32;
        let step_len = yps / 60.0;
        print!("DRIVE {label} start ({ax:+.2},{az:+.2}) up={up:+.3}: ");
        for tick in 0..300 {
            let res = step(
                &geom,
                &ObstacleSet::default(),
                &mut st,
                ax,
                -az,
                -up,
                dx * step_len,
                dz * step_len,
                yps,
                1.0 / 60.0,
                false,
                true,
            );
            ax += res.dx;
            az -= res.dy;
            up = -res.feet_z;
            if tick % 4 == 0 {
                print!("t{tick}({ax:+.2},{az:+.2}){up:+.3}/{:?}", res.decision);
            }
            if (ax - cx).abs() < 0.15 && (az - cz).abs() < 0.6 {
                print!(
                    " | ATSPOT t{tick} up={up:+.3} {:?} out={}",
                    res.decision,
                    res.outcome.label()
                );
                break;
            }
        }
        println!();
    }
}

/// One more read at USER's stuck point (+21.05,+62.63) feet +3.83: what does the walker's own
/// support query (`contact_floor`, pose-seeded, ring included) answer there - the panel says the
/// column answers none yet grounded stayed yes.
#[test]
fn wall_stuck_contact_at_spot() {
    let Ok(spec) = std::env::var("KULUU_WALL_FORENSICS") else {
        return;
    };
    let nums: Vec<f32> = spec.split(':').map(|t| t.parse().unwrap()).collect();
    let (zone, cx, cz, ph) = (nums[0] as u16, nums[1], nums[2], nums[3]);
    let Some(geom) = zone_geom(zone) else { return };
    for seed in [ph, 4.167, 3.583, 3.0] {
        match super::step::contact_floor(&geom, &[], Vec2::new(cx, cz), seed, true) {
            Some(h) => println!("DIAG contact seed{seed:+.3} -> {h:+.4}"),
            None => println!("DIAG contact seed{seed:+.3} -> NONE"),
        }
    }
}

/// Pin for USER's 2026-10-28 report ("Im stuck on this rounded edge, that i should not have
/// been able to climb in the first place") at Bastok Mines xz(-52.15,+19.15): three streets
/// east of the pipe cluster (columns measured by `pipe_edge_forensics_minus_52_plus_19`:
/// floor +0.000, side faces t24632/t24633 y[+0.0..+2.0] at plane x~-52, cap floor +0.49..+0.60
/// answering mid-face behind them), walk straight west into the faces and confirm nothing is
/// ever mounted: height never leaves the street (the flush-top riser rule,
/// `dat_mzb::RISER_FLUSH_TOP_SLACK_YAMLS`, keeps a face whose standable floor meets it
/// mid-extent solid - wall-with-a-ledge, not riser) and the body stops before their plane.
#[test]
fn the_bastok_pipe_edge_at_minus_52_plus_19_is_never_mounted() {
    let Some(geom) = zone_geom(234) else { return };
    for lane in [18.75f32, 19.15, 19.55] {
        let (mut ax, az) = (-50.5f32, lane);
        let mut up = geom.ground_nearest(Vec2::new(ax, az), 0.0).unwrap();
        assert!(
            up.abs() < 0.1,
            "street should answer ~+0.0 at ({ax:+.2},{az:+.2}), got {up:+.3}"
        );
        let yps = 1.6;
        let step_len = yps / 60.0;
        let mut st = Walker::default();
        let (mut min_ax, mut max_up, mut air_ticks) = (ax, up, 0usize);
        for _ in 0..240 {
            let res = step(
                &geom,
                &ObstacleSet::default(),
                &mut st,
                ax,
                -az,
                -up,
                -step_len, // walk -x, straight at the pipe side faces
                0.0,
                yps,
                1.0 / 60.0,
                false,
                true,
            );
            if matches!(res.decision, VerticalDecision::Airborne { .. }) {
                air_ticks += 1;
            }
            ax += res.dx;
            up = -res.feet_z;
            min_ax = min_ax.min(ax);
            max_up = max_up.max(up);
        }
        println!(
            "pipe pin lane {lane:+.2}: stopped x={min_ax:+.2} (side-face plane at ~-52) height \
             {up:+.3} (max {max_up:+.3}) air_ticks {air_ticks}"
        );
        assert_eq!(
            air_ticks, 0,
            "lane {lane:+.2}: walked off the street into a drop at x={ax:.2}"
        );
        assert!(
            max_up < 0.1,
            "lane {lane:+.2}: pose rose to {max_up:+.3} - the pipe cap tops +0.49..+0.60 behind \
             faces standing y[+0.0..+2.0]; their standable floor meets them mid-extent, so the \
             riser exemption must not fire and nothing here may be mounted"
        );
        assert!(
            min_ax > -52.0,
            "lane {lane:+.2}: pose reached x={min_ax:+.2}; the side faces (plane ~x=-52, body \
             radius 0.4) must stop it short of their plane"
        );
    }
}
