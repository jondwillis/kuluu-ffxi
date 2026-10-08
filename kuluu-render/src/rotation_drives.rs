//! Retail's ActorRotation drive-task (`CMoActorRotationDriveTask`) turns an actor to an orientation a
//! routine authors: the stage carries three angles in degrees, a mode byte and a duration, and retail's
//! task moves the actor's own angle record from wherever it stood when the stage fired — it lerps toward
//! an absolute target rather than adding a delta (`FFXiMain.dll retail-2026-09` RVA 0x5FA64..0x5FA8B
//! captures the live triple through the orientation accessor at vtable slot byte `0x1C0`, RVA 0x820F0).

use bevy::prelude::{Component, Resource, Vec3};
use ffxi_dat::scheduler::ActorRotation;

/// The degrees→radians factor retail converts each authored angle with in its constructor (`FFXiMain.dll
/// retail-2026-09` RVA 0x5FA95 / 0x5FABB / 0x5FACB, all reading `.rdata 0x32A9F4`). This layer's factor
/// is the approximate π/180; the exact one (`.rdata 0x32A840`) belongs to unrelated code.
const AUTHORED_DEGREES_TO_RADIANS: f32 = 0.017_452_778;

/// The pair a stored angle is re-wrapped with after every write (`FFXiMain.dll retail-2026-09`:
/// `.rdata 0x329D2C` holds the full turn and `0x329D30`/`0x329D28` the limits, component 0 wrapped at
/// RVA 0x5FCF8..0x5FD2F, then 0x5FD32..0x5FCD4). One pass each way — a value already outside ±2π is
/// left alone. The skeleton joint integrator wraps with a different, exact pair, so the two conventions
/// are not interchangeable.
const WRAP_TWO_PI: f32 = 2.0 * WRAP_LIMIT_RADIANS;

/// One bound of the pair that closes [`WRAP_TWO_PI`] (`.rdata 0x329D30`, its negation at `0x329D28`;
/// bits `0x4049_0E56`). The stored float is retail's own approximation, so the full turn is exactly
/// twice this bound and not any multiple of π.
#[allow(clippy::approx_constant)]
const WRAP_LIMIT_RADIANS: f32 = 3.141_5;

/// Which of the three stored angles is the actor heading (`actor+0x48`). Two unrelated consumers in
/// `FFXiMain.dll retail-2026-09` pin it: the walker lerps this component into its facing yaw (RVA
/// 0xA637C..0xA6472, result stored to `actor+0xE8`) and the in-world look-at pass hands its negation to
/// the vector rotator at RVA 0x10027BD0 (call site RVA 0xD5D04). Nothing in this build reads the outer
/// two components as an angle of inclination, so they are carried by [`ActorRotationDrive`] and applied
/// nowhere.
pub const HEADING_COMPONENT: usize = 1;

/// One live ActorRotation task: what the actor's angles were when its stage fired, where the authored
/// record says they should end up, and the countdown between them.
#[derive(Debug, Clone, Copy)]
pub struct ActorRotationDrive {
    /// The stage identity, as `(fire frame, half-open end)` — what retires the drive once no running
    /// routine covers it. Retail tears the task down with its scheduler; a mode-0 task never counts
    /// itself down (its timer is inert), so the interval is the only thing that ends one.
    fire_frame: u32,
    end_frame: u32,
    /// The record's mode byte. Every shipped record carries 0, which retail branches on twice
    /// (`FFXiMain.dll retail-2026-09`): as the gate on the countdown (RVA 0x5FB36) and again to choose
    /// the write order (RVA 0x5FBCF..0x5FBDC).
    mode: u8,
    /// Authored `duration_frames`, in this codebase's routine-clock frames. Retail scales the authored
    /// word by a context factor before truncating it (`FFXiMain.dll retail-2026-09` operand fetch at RVA
    /// 0x1005E590); that factor is not part of any stage kuluu parses, so the countdown inherits the
    /// frame unit every other stage duration uses. Only mode≠0 records read it, and this install has
    /// none.
    duration_frames: f32,
    remaining_frames: f32,
    from_radians: [f32; 3],
    to_radians: [f32; 3],
}

impl ActorRotationDrive {
    /// The drive one fired stage spawns. `live_radians` is the actor's angle record read at that moment
    /// — retail captures it in the constructor, so a re-fired stage starts from the current pose rather
    /// than from where an earlier drive left off.
    pub fn fired_at(
        fire_frame: u32,
        end_frame: u32,
        duration_frames: u16,
        rotation: &ActorRotation,
        live_radians: [f32; 3],
    ) -> Self {
        let duration = duration_frames as f32;
        Self {
            fire_frame,
            end_frame,
            mode: rotation.mode,
            duration_frames: duration,
            remaining_frames: duration,
            from_radians: live_radians,
            to_radians: authored_radians(rotation),
        }
    }

    /// The stage interval this drive was adopted from.
    pub fn stage(&self) -> (u32, u32) {
        (self.fire_frame, self.end_frame)
    }

    /// One tick of retail's update: the countdown only runs when the mode byte is set, progress is
    /// `1 − remaining/duration`, and each angle is re-wrapped on its way out. A mode-0 drive reports the
    /// authored orientation every tick it lives — that repetition is the law, not an accident: retail
    /// writes the record with `k = 1` while its scheduler still holds the task.
    pub fn advance(&mut self, elapsed_frames: f32) -> [f32; 3] {
        let mut progress = 1.0;
        if self.mode != 0 {
            self.remaining_frames -= elapsed_frames;
            if self.remaining_frames > 0.0 && self.duration_frames > 0.0 {
                progress = 1.0 - self.remaining_frames / self.duration_frames;
            }
        }
        let mut applied = [0.0; 3];
        applied.iter_mut().enumerate().for_each(|(index, out)| {
            let from = self.from_radians[index];
            let to = self.to_radians[index];
            *out = wrap_component(from + (to - from) * progress);
        });
        applied
    }
}

/// The authored angles in radians, kept in record order. Only [`HEADING_COMPONENT`] of them has a named
/// meaning in this build.
pub fn authored_radians(rotation: &ActorRotation) -> [f32; 3] {
    let mut radians = [0.0; 3];
    radians
        .iter_mut()
        .zip(rotation.angles_degrees)
        .for_each(|(out, degrees)| *out = degrees * AUTHORED_DEGREES_TO_RADIANS);
    radians
}

/// The heading component of an applied orientation triple — the one number kuluu can act on.
pub fn heading_of(applied: [f32; 3]) -> f32 {
    applied[HEADING_COMPONENT]
}

/// The heading a running drive holds for the local player, written every tick by
/// `scheduler_runtime::tick_actor_rotation_drives` and taken as the walker's base facing while it runs
/// (kuluu/src/view_native/input.rs): taken rather than read, so travel re-aims over it on ticks where
/// the player travels instead of fighting the drive.
#[derive(Resource, Default)]
pub struct SelfAuthoredHeading(pub Option<f32>);

/// Retail's re-wrap of one stored angle into ±π (one pass in each direction, [`WRAP_TWO_PI`]).
fn wrap_component(angle: f32) -> f32 {
    let mut wrapped = angle;
    if wrapped > WRAP_LIMIT_RADIANS {
        wrapped -= WRAP_TWO_PI;
    } else if wrapped < -WRAP_LIMIT_RADIANS {
        wrapped += WRAP_TWO_PI;
    }
    wrapped
}

/// The distance guard the turn stage measures against (`FFXiMain.dll retail-2026-09`: both comparisons
/// at RVA 0x5AFD9 and RVA 0x5AFE6 read `.rdata 0x32A378`, bits `0x3DCC_CCCD`). As written it is not a
/// radius test: only when BOTH the X and Z components of the offset to face are below it does the handler
/// substitute `(1.0, 0.0)` for the offset (stores at RVA 0x5AFF7 / RVA 0x5AFFF), which aims the actor at
/// heading zero.
const TURN_TARGET_GUARD_YALMS: f32 = 0.1;

/// The remaining travel and signed rate one turn stage queued and retail has not finished walking, plus
/// the companion countdowns that keep it consumable. Retail keeps all of it on the actor itself
/// (`FFXiMain.dll retail-2026-09`): remaining magnitude `actor+0x870` and per-frame step
/// `actor+0x874`, both written by the stage handler
/// through setters RVA 0x5E7C0 / RVA 0x5E7D0 (call sites RVA 0x5B08F / RVA 0x5B0B6, and those two setters
/// have no other caller in this build), and the nesting counter `actor+0x86C`, which the handler bumps at
/// RVA 0x5AF7D and each companion task releases in its destructor (RVA 0x60F40, release call at RVA
/// 0x60F5E). One entry of [`PendingTurn::enable_frames`] per companion task, because that is what a bump
/// is.
#[derive(Component, Default, Debug)]
pub struct PendingTurn {
    /// `actor+0x870`: the angle still owed, and a frame that finds it not strictly positive stops
    /// taking any of it.
    remaining_rad: f32,
    /// `actor+0x874`: the authored rate in radians, already carrying the turn's direction.
    step_rad_per_frame: f32,
    /// One countdown per live companion task; the turn is only consumed while this list is non-empty
    /// (`actor+0x86C != 0`).
    enable_frames: Vec<f32>,
    /// Whole retail frames not yet stepped. Retail adds exactly one step per actor update, and the
    /// default client updates at its capped frame rate, so the layer carries the remainder instead
    /// of scaling the step by wall time.
    carry_frames: f32,
}

/// The actor is off the ground, which is retail's second gate in front of its facing integrator
/// (`FFXiMain.dll retail-2026-09`: the byte at `actor+0x102`, set by jump start at RVA 0xAAACE and cleared on
/// the landing test at RVA 0xAAD1D, read as a gate at RVA 0xC66A1). No packet reports an actor leaving the
/// ground, and the only producer this build names for that byte is the knock-back stage (kuluu's
/// `StageKind::Knockback`), so a running knock-back is what holds this flag.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Component, Debug, Default)]
pub struct ActorAirborne;

/// The angle and rate one fired turn stage stores on its actor (`FFXiMain.dll retail-2026-09` handler
/// RVA 0x5AF2C, measurement at RVA 0x5AF79..0x5B083). `current_heading_rad` is what the orientation
/// accessor returns when the stage fires - component 1 of the triple at vtable slot byte `0x1C0` - and
/// both positions come from slot byte `0x1BC`. Retail's angle of an offset is the negated atan2 of its Z
/// over its X (`fpatan` helper), and the difference against the heading is wrapped once before its
/// magnitude is stored.
pub fn measured_turn(
    from: Vec3,
    toward: Vec3,
    current_heading_rad: f32,
    authored_step_degrees: f32,
) -> (f32, f32) {
    let mut delta_x = toward.x - from.x;
    let mut delta_z = toward.z - from.z;
    // Each component is tested on its own, exactly as the two comparisons are laid out in the
    // handler: both under the guard at once replaces the offset.
    if delta_x < TURN_TARGET_GUARD_YALMS && delta_z < TURN_TARGET_GUARD_YALMS {
        delta_x = 1.0;
        delta_z = 0.0;
    }
    let facing = -delta_z.atan2(delta_x);
    let difference = wrap_component(facing - current_heading_rad);
    let step_radians = authored_step_degrees * AUTHORED_DEGREES_TO_RADIANS;
    (
        difference.abs(),
        if difference < 0.0 {
            -step_radians
        } else {
            step_radians
        },
    )
}

impl PendingTurn {
    /// The handler's three writes at stage-fire time: the measured pair onto the actor and one bump of the
    /// nesting counter, whose companion lives as long as the stage's (unrounded) duration.
    pub fn arm(&mut self, from: Vec3, toward: Vec3, current_heading_rad: f32, plan: &PlanTurn) {
        let (remaining_rad, step_rad_per_frame) =
            measured_turn(from, toward, current_heading_rad, plan.step_degrees);
        self.remaining_rad = remaining_rad;
        self.step_rad_per_frame = step_rad_per_frame;
        self.enable_frames.push(plan.duration_frames);
    }

    /// Whether anything still permits consumption (`actor+0x86C != 0`).
    pub fn enabled(&self) -> bool {
        !self.enable_frames.is_empty()
    }

    /// Nothing left to travel, so the queue can be dropped with the routine.
    pub fn is_spent(&self) -> bool {
        self.remaining_rad <= 0.0
    }

    /// One kuluu frame of retail's consumer: each whole [`crate::scheduler_runtime::RETAIL_FPS`] frame adds
    /// one signed step to the heading and shortens the owed angle by `|step|`, and a step that overshoots
    /// backs the heading off again by the leftover so the net travel is exactly what was measured (RVA
    /// 0xC66CA..0xC67DA; the leftover branch re-wraps the same pair). Returns the heading once the turn has
    /// been stepped at least this frame, or `None` when it stayed put. The companion countdowns run whether
    /// or not anything is consumed - a task's own timer never stops (`FFXiMain.dll retail-2026-09` RVA
    /// 0x60FD0..0x60FEF, same shape as the lock tasks) and one ends strictly below zero.
    pub fn advance(&mut self, heading_rad: f32, elapsed_retail_frames: f32) -> Option<f32> {
        for frames in &mut self.enable_frames {
            *frames -= elapsed_retail_frames;
        }
        self.enable_frames.retain(|frames| *frames >= 0.0);

        if !self.enabled() || self.remaining_rad <= 0.0 {
            return None;
        }
        self.carry_frames += elapsed_retail_frames;
        let mut heading = heading_rad;
        let mut stepped = false;
        while self.carry_frames >= 1.0 && self.remaining_rad > 0.0 {
            self.carry_frames -= 1.0;
            heading = wrap_component(heading + self.step_rad_per_frame);
            let leftover = self.remaining_rad - self.step_rad_per_frame.abs();
            if leftover < 0.0 {
                heading = wrap_component(
                    heading
                        + if self.step_rad_per_frame > 0.0 {
                            leftover
                        } else {
                            -leftover
                        },
                );
                self.remaining_rad = 0.0;
            } else {
                self.remaining_rad = leftover;
            }
            stepped = true;
        }
        stepped.then_some(heading)
    }
}

/// The two authored words of a turn stage, as this layer decodes them.
pub struct PlanTurn {
    /// Degrees the actor moves per update (`StageKind::TurnToward`'s payload float).
    pub step_degrees: f32,
    /// `duration_frames`, kept fractional because this stage's fetch skips the rounding every lock task
    /// applies (`FFXiMain.dll retail-2026-09`: no `ftoi_round` call before RVA 0x60F80, unlike RVA 0x5C8E4).
    pub duration_frames: f32,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod actor_rotation_drive_tests {
    use super::*;

    /// The three floats retail reads out of `.rdata` for this task, pinned by bit pattern so a retyped
    /// constant cannot silently shift the layer's wrap or degree unit.
    #[test]
    fn the_constants_are_the_builds_float_bits() {
        assert_eq!(WRAP_TWO_PI.to_bits(), 0x40C9_0E56, ".rdata 0x329D2C");
        assert_eq!(WRAP_LIMIT_RADIANS.to_bits(), 0x4049_0E56, ".rdata 0x329D30");
        assert_eq!(
            AUTHORED_DEGREES_TO_RADIANS.to_bits(),
            0x3C8E_F921,
            ".rdata 0x32A9F4"
        );
    }

    fn rotation(angles_degrees: [f32; 3], mode: u8) -> ActorRotation {
        ActorRotation {
            angles_degrees,
            mode,
        }
    }

    /// Mode 0 — every shipped record — sets the orientation on the spot and does not count down.
    #[test]
    fn mode_zero_applies_the_authored_angle_immediately() {
        let turn = rotation([0.0, -90.0, 0.0], 0);
        let mut drive = ActorRotationDrive::fired_at(10, 70, 60, &turn, [0.0, 0.4, 0.0]);
        let first = drive.advance(1.0);
        assert!(close(
            first[HEADING_COMPONENT],
            authored_radians(&turn)[HEADING_COMPONENT]
        ));

        for _ in 0..5 {
            let again = drive.advance(1.0);
            assert!(close(again[HEADING_COMPONENT], first[HEADING_COMPONENT]));
        }
        assert_eq!(
            drive.remaining_frames, 60.0,
            "mode 0 leaves its timer alone"
        );
    }

    /// A non-zero mode counts down and lands the authored absolute angle at the end of it — measured as
    /// a fraction of the way from the live value, not from zero. The expectations ride the same
    /// approximate degree factor the drive converts with; the exact π/180 lives in unrelated code.
    #[test]
    fn a_nonzero_mode_ramps_from_the_live_angle_to_the_authored_one() {
        let from = [0.0, 0.0, 0.0];
        let turn = rotation([0.0, 90.0, 0.0], 1);
        let to = authored_radians(&turn)[HEADING_COMPONENT];
        let mut drive = ActorRotationDrive::fired_at(0, 100, 100, &turn, from);

        // A quarter through the countdown writes a quarter of the way to the authored angle.
        let quarter = drive.advance(25.0);
        assert!(close(quarter[HEADING_COMPONENT], to * 0.25));

        let rest = drive.advance(75.0);
        assert!(close(rest[HEADING_COMPONENT], to));
        assert!(drive.remaining_frames <= 0.0, "the countdown finished");

        let after = drive.advance(10.0);
        assert!(
            close(after[HEADING_COMPONENT], to),
            "a spent drive holds its target"
        );
    }

    /// The authored angle is absolute: a second stage aimed at the same record starts from wherever the
    /// first left it, because kuluu captures live values per fire.
    #[test]
    fn a_refire_starts_from_the_angle_the_record_holds() {
        let mut first =
            ActorRotationDrive::fired_at(0, 10, 10, &rotation([0.0, 90.0, 0.0], 0), [0.0; 3]);
        let handed_on = heading_of(first.advance(1.0));

        let again = rotation([0.0, -90.0, 0.0], 0);
        let mut second = ActorRotationDrive::fired_at(20, 30, 10, &again, [0.0, handed_on, 0.0]);
        assert!(close(
            heading_of(second.advance(1.0)),
            authored_radians(&again)[HEADING_COMPONENT]
        ));
    }

    /// Wrapping is one pass each way on the same pair retail uses: a target just past `+π` comes back by
    /// the approximate 6.283, and its mirror goes the other way.
    #[test]
    fn stored_angles_are_rewrapped_into_the_layers_pi_pair() {
        let just_over = WRAP_LIMIT_RADIANS + 0.1;
        assert!(close(wrap_component(just_over), just_over - WRAP_TWO_PI));
        assert!(close(
            wrap_component(-(WRAP_LIMIT_RADIANS + 0.1)),
            -(WRAP_LIMIT_RADIANS + 0.1) + WRAP_TWO_PI
        ));

        let mut drive =
            ActorRotationDrive::fired_at(0, 10, 10, &rotation([0.0, 200.0, 0.0], 0), [0.0; 3]);
        let applied = drive.advance(1.0);
        assert!(applied[HEADING_COMPONENT] < WRAP_LIMIT_RADIANS);
        assert!(!close(
            applied[HEADING_COMPONENT],
            200.0 * AUTHORED_DEGREES_TO_RADIANS
        ));
    }

    /// The four angles this install actually authors (one file, `ROM3\\0\\43.DAT`) all turn only the
    /// heading component; the outer two stay zero and are never applied.
    #[test]
    fn shipped_records_turn_only_the_heading() {
        for degrees in [90.0, -90.0, -135.0, 45.0] {
            let authored = authored_radians(&rotation([0.0, degrees, 0.0], 0));
            assert_eq!(authored[0], 0.0);
            assert_eq!(authored[2], 0.0);

            let mut drive = ActorRotationDrive::fired_at(
                0,
                10,
                10,
                &rotation([0.0, degrees, 0.0], 0),
                [0.37, -0.25, 0.11],
            );
            let applied = drive.advance(1.0);
            assert!(close(
                applied[HEADING_COMPONENT],
                authored[HEADING_COMPONENT]
            ));
        }
    }

    /// The guard float, pinned by bit pattern like the other three this layer carries.
    #[test]
    fn the_turn_guard_is_the_builds_float() {
        assert_eq!(
            TURN_TARGET_GUARD_YALMS.to_bits(),
            0x3DCC_CCCD,
            ".rdata 0x32A378"
        );
    }

    /// A turn measures an offset as the negated atan2 of its Z over its X, so a target off `-Z` sits one way
    /// round and the mirror of that offset across `+X` goes the other. Both cases keep their X above the
    /// guard, which is what lets them be measured at all.
    #[test]
    fn a_turn_measures_the_offset_as_a_negated_atan2() {
        let from = Vec3::new(1.0, 0.0, 2.0);

        let (up, step_up) = measured_turn(from, Vec3::new(6.0, 0.0, -3.0), 0.0, 45.0);
        assert!(close(up, std::f32::consts::FRAC_PI_4));
        assert!(step_up > 0.0, "a target off -Z turns that way round");

        let (down, step_down) = measured_turn(from, Vec3::new(6.0, 0.0, 7.0), 0.0, 45.0);
        assert!(close(down.abs(), std::f32::consts::FRAC_PI_4));
        assert!(
            step_down < 0.0,
            "the mirrored offset turns back the other way"
        );
    }

    /// An offset whose X and Z components are both under the guard is replaced by `(1, 0)`, which aims at
    /// heading zero however the actor was facing. The comparisons are on each component rather than on their
    /// length, so a target level with the actor but well behind it falls inside the guard as readily as one
    /// standing almost on top of it.
    #[test]
    fn an_offset_inside_the_guard_aims_at_heading_zero() {
        let from = Vec3::new(12.0, 0.0, -4.0);

        let (near_ahead, _) = measured_turn(from, Vec3::new(12.04, 0.0, -4.0), 2.5, 45.0);
        assert!(close(near_ahead, 2.5), "aimed back at heading zero");

        let (behind, _) = measured_turn(from, Vec3::new(11.9, 0.0, -4.0), 2.5, 45.0);
        assert!(
            close(behind, near_ahead),
            "the guard is not a distance test"
        );
    }

    /// One firing re-stores the measured pair and buys one more companion countdown, rather than adding
    /// to what an earlier firing left.
    #[test]
    fn firing_stores_the_measurement_and_buys_a_companion() {
        let mut turn = PendingTurn::default();
        let plan = PlanTurn {
            step_degrees: 45.0,
            duration_frames: 30.0,
        };
        turn.arm(
            Vec3::new(1.0, 0.0, 2.0),
            Vec3::new(6.0, 0.0, -3.0),
            0.0,
            &plan,
        );
        assert!(close(turn.remaining_rad, std::f32::consts::FRAC_PI_4));
        assert!(close(
            turn.step_rad_per_frame.abs(),
            45.0 * AUTHORED_DEGREES_TO_RADIANS
        ));

        turn.arm(
            Vec3::new(1.0, 0.0, 2.0),
            Vec3::new(6.0, 0.0, -3.0),
            0.5,
            &plan,
        );
        assert_eq!(turn.enable_frames.len(), 2, "each firing buys its own");
        let remeasured = turn.remaining_rad;
        let from_zero = measured_turn(
            Vec3::new(1.0, 0.0, 2.0),
            Vec3::new(6.0, 0.0, -3.0),
            0.5,
            45.0,
        );
        assert!(close(remeasured, from_zero.0), "re-stored, not added on");
    }

    /// A queue built straight from what the handler leaves on the actor - `actor+0x870`/`actor+0x874` and one
    /// companion task - so each law below reads off its own numbers.
    fn queued(remaining_rad: f32, step_rad_per_frame: f32, enable_frames: f32) -> PendingTurn {
        PendingTurn {
            remaining_rad,
            step_rad_per_frame,
            enable_frames: vec![enable_frames],
            carry_frames: 0.0,
        }
    }

    /// Two steps of the queued rate cover a turn measured as exactly twice that and stop there, so the
    /// heading handed on equals what was measured.
    #[test]
    fn stepping_covers_exactly_the_measured_angle() {
        let mut turn = queued(1.6, 0.8, 60.0);

        let halfway = turn.advance(0.0, 1.0).expect("one retail frame");
        assert!(close(halfway, 0.8));
        assert!(turn.remaining_rad > 0.0, "still owed");

        let full = turn.advance(halfway, 1.0).expect("a second frame");
        assert!(close(turn.remaining_rad, 0.0));
        assert!(close(full, 1.6), "net travel equals the measurement");
        assert!(turn.is_spent());
        assert!(turn.advance(full, 1.0).is_none(), "nothing left to travel");
    }

    /// A step that overshoots does not stop at the target and is not doubled: the heading backs off by the
    /// leftover against the direction of travel, both ways round.
    #[test]
    fn an_overshooting_step_backs_off_by_the_leftover() {
        let mut forward = queued(1.5, 1.0, 60.0);
        let after_one = forward.advance(0.0, 1.0).expect("stepped");
        assert!(close(after_one, 1.0));
        let after_two = forward.advance(after_one, 1.0).expect("stepped again");
        assert!(
            (after_two - 1.5).abs() < 0.2,
            "not a whole step past the target"
        );
        assert!(close(after_two, 1.5), "the leftover is undone");

        let mut back = queued(1.5, -1.0, 60.0);
        let neg_one = back.advance(0.0, 1.0).expect("stepped");
        let neg_two = back.advance(neg_one, 1.0).expect("stepped again");
        assert!(close(neg_two, -1.5), "mirrored the same way round");
    }

    /// One kuluu frame pays exactly as many whole retail frames as it covered and carries what is left over.
    #[test]
    fn one_kuluu_frame_pays_whole_retail_frames() {
        let mut turn = queued(3.0, 0.25, 100.0);
        let after_four = turn.advance(0.0, 4.5).expect("four retail frames in one");
        assert!(close(after_four, 1.0));
        assert!(close(turn.remaining_rad, 2.0));

        // Half a frame left unpaid still counts towards the next one.
        let after_five = turn
            .advance(after_four, 0.5)
            .expect("the remainder completes a frame");
        assert!(close(after_five, 1.25));
    }

    /// A stepped heading is re-wrapped with the pair every stored angle uses, so travel past one limit comes
    /// back at the other instead of running off.
    #[test]
    fn a_stepped_heading_is_rewrapped_like_every_other_angle() {
        let mut turn = queued(10.0, 0.4, 60.0);
        let stepped = turn.advance(3.0, 1.0).expect("stepped");
        assert!(close(stepped, wrap_component(3.4)));
        assert!(stepped < 0.0, "wrapped past the limit");
    }

    /// The companion countdown bounds the turn: when every bump has expired the owed angle stays unpaid and
    /// nothing more is written, which is what makes `duration_frames` a length rather than a suggestion.
    #[test]
    fn an_expired_companion_stops_the_turn_short() {
        let mut turn = queued(3.0, 0.25, 2.0);
        let first = turn.advance(0.0, 1.0).expect("first frame steps");
        assert!(turn.enabled());

        // A long stall: the companion is gone, so the queue freezes with the angle still owing.
        let stalled = turn.advance(first, 30.0);
        assert!(!turn.enabled(), "the companion expired during the stall");
        assert!(stalled.is_none());
        assert!(close(turn.remaining_rad, 2.75));
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }
}
