//! Retail's following camera while it is locked on to a target. The camera looks at the point midway
//! between the player and the target, and its eye is held inside a cone around the line from the target
//! through the player. A lock opens by swinging the eye out to the cone's edge, off to whichever side of
//! the line it sits nearer, a quarter of what is left per tick; after that, inside the cone the eye stays
//! exactly where it is and at the edge the line drags it round, so a side step around the target carries
//! the player a few yalms before the camera follows. The eye's distance from the look point is held in a band set by that same player-target span
//! and the view's focal length.
//!
//! Provenance (`FFXiMain.dll retail-2026-09`): the following-camera update at RVA 0x1EE60 takes this
//! branch whenever the player's free-run byte is clear (RVA 0x1F60B), which every lock handler clears
//! (RVA 0xC5440, RVA 0xC54C0); the look point is built at RVA 0x1F44B..0x1F5F6 and the eye law runs at
//! RVA 0x201E6..0x20773.

use std::f32::consts::{PI, TAU};

use bevy::prelude::*;
use kuluu_render::graphics_settings::RETAIL_DEFAULT_FOCAL_LENGTH;
use kuluu_render::ViewFov;

use super::camera_collision::rotate_about;
use super::input::RETAIL_MOVE_TICKS_PER_SEC;

/// The player's camera point is its skeleton reference 13, one of the torso-height ring points
/// (`FFXiMain.dll retail-2026-09` RVA 0x1F4F8 pushes it to the actor's reference lookup); only its height
/// enters the look point.
pub const PLAYER_POINT_SLOT: usize = 13;

/// The target's camera point is its chest reference, read once when the lock begins (`FFXiMain.dll
/// retail-2026-09` RVA 0xD66D2 / RVA 0xD6700 push 7, RVA 0xD66E1 / RVA 0xD670F store its height); only its
/// height enters the look point.
pub const TARGET_POINT_SLOT: usize = ffxi_dat::skel::standard_position::CHEST;

/// The look point blends the two camera points half and half (`FFXiMain.dll retail-2026-09` RVA 0x1F50F).
const LOOK_BLEND: f32 = 0.5;

/// The look point and the eye's distance each close this share of what is left per camera tick
/// (`FFXiMain.dll retail-2026-09` immediate at RVA 0x1F5D3, `.rdata` RVA 0x329CE4 read at RVA 0x205CD).
const FOLLOW_PER_TICK: f32 = 0.25;

/// The cone's half-angle is `atan(CONE_REACH / (near base + span))` (`FFXiMain.dll retail-2026-09`
/// `.rdata` RVA 0x32A3A0 read at RVA 0x20412).
const CONE_REACH: f32 = 4.0;

/// The near and far distance bases before the focal scale (`FFXiMain.dll retail-2026-09` `.rdata` RVA
/// 0x32A3A8 read at RVA 0x203EB, RVA 0x32A3A4 read at RVA 0x203F9).
const NEAR_BASE: f32 = 7.2;
const FAR_BASE: f32 = 8.6;

/// The share of the player-target span both bases grow by (`FFXiMain.dll retail-2026-09` `.rdata` RVA
/// 0x32A3BC read at RVA 0x203D4).
const SPAN_SHARE: f32 = 0.125;

/// The focal length's scale into those bases (`FFXiMain.dll retail-2026-09` `.rdata` RVA 0x32A22C read at
/// RVA 0x203CA): the default 350 focal reads as 0.35, so zooming the view in pushes the eye out.
const FOCAL_SCALE: f32 = 0.001;

/// Half the player-target span joins both distance edges (`FFXiMain.dll retail-2026-09` `.rdata` RVA
/// 0x329A08 read at RVA 0x20561).
const SPAN_HALF: f32 = 0.5;

/// Distances under this floor read as it: the same `.rdata` floor the aim laws guard with, read here at
/// `FFXiMain.dll retail-2026-09` RVA 0x20277 and RVA 0x2028C.
const DIST_FLOOR: f32 = kuluu_render::mouse::CAMERA_AIM_MIN_DISTANCE_YALMS;

/// The wall test casts from this far above the player's feet toward the eye (`FFXiMain.dll retail-2026-09`
/// `.rdata` RVA 0x32961C subtracted at RVA 0x20691).
pub const WALL_RAY_RISE: f32 = 1.0;

/// The swing a lock's opening still owes, in radians, under which it lands the eye on the cone's edge.
const OPENING_SETTLED_RAD: f32 = 1e-3;

/// A wall between the player and the eye pulls the eye in to this short of the hit (`FFXiMain.dll
/// retail-2026-09` `.rdata` RVA 0x32A39C read at RVA 0x2072F).
const WALL_PULL: f32 = 0.2;

/// The locked camera's two world points, carried frame to frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LockedCamera {
    pub eye: Vec3,
    pub look: Vec3,
}

/// What one frame of the lock reads from the world: both actors' feet and camera points, and the focal
/// length it frames with ([`lock_focal`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LockedWorld {
    pub player: Vec3,
    pub target: Vec3,
    pub player_point: Vec3,
    pub target_point: Vec3,
    pub focal_length: f32,
}

impl LockedWorld {
    /// The unit line from the target through the player, and the cone and band its span sets.
    fn cone(&self) -> (Vec3, LockFrame) {
        let line = self.player - self.target;
        let span = line.length().max(DIST_FLOOR);
        (line / span, lock_frame(span, self.focal_length))
    }
}

/// The cone half-angle and the eye's distance band one player-target span sets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LockFrame {
    pub half_angle: f32,
    pub near: f32,
    pub far: f32,
}

/// The cone and band for a player-target `span` at `focal_length` (`FFXiMain.dll retail-2026-09` RVA
/// 0x203C5..0x2041C, RVA 0x2052B..0x205AD). A lock taken with H adds nothing for the target's size to the
/// span share.
pub fn lock_frame(span: f32, focal_length: f32) -> LockFrame {
    let focal = focal_length * FOCAL_SCALE;
    let reach = span * SPAN_SHARE;
    let near_base = (NEAR_BASE + reach) * focal;
    let far_base = (FAR_BASE + reach) * focal;
    LockFrame {
        half_angle: (CONE_REACH / (near_base + span)).atan(),
        near: near_base + span * SPAN_HALF,
        far: far_base + span * SPAN_HALF,
    }
}

/// The focal length the lock frames with: the live focal taken relative to the menu's base FOV and set
/// on retail's default focal. At the menu's retail FOV that is the live focal itself; a wider menu FOV
/// keeps retail's cone and band, so the locked view widens with it as the free camera's does, and the
/// zoom keys still move the band as retail's do.
pub fn lock_focal(live_focal: f32, menu_fov_deg: f32) -> f32 {
    live_focal * RETAIL_DEFAULT_FOCAL_LENGTH / ViewFov::focal_for_deg(menu_fov_deg)
}

/// `offset` turned toward the unit `toward`, about the axis square to both, just far enough that the
/// angle between them is at most `half_angle` (`FFXiMain.dll retail-2026-09` RVA 0x20469..0x20505); inside
/// the cone it is returned
/// untouched. A pair that spans no plane (parallel, anti-parallel, or no line at all because the player
/// stands on the target) is left alone too, as retail's zero cross product leaves it.
pub fn hold_in_cone(offset: Vec3, toward: Vec3, half_angle: f32) -> Vec3 {
    let length = offset.length();
    if length <= f32::EPSILON || toward.length_squared() <= f32::EPSILON {
        return offset;
    }
    let dir = offset / length;
    let cos = dir.dot(toward).clamp(-1.0, 1.0);
    if cos.acos() <= half_angle {
        return offset;
    }
    let Some(across) = (dir - toward * cos).try_normalize() else {
        return offset;
    };
    (toward * half_angle.cos() + across * half_angle.sin()) * length
}

/// The ground turn, in chase-yaw radians as [`LockedCamera::orbit`] takes it, that swings `offset` round
/// the vertical out to the edge of the cone of `half_angle` round the unit `toward`, on whichever side of
/// the line it already leans (the player's right, facing the target, when it leans neither way). None
/// when no turn out is wanted or possible: the offset is already at or past the edge, it rises so steeply
/// that no turn reaches the edge, or it or the line has no run on the ground to turn.
pub fn turn_to_edge(offset: Vec3, toward: Vec3, half_angle: f32) -> Option<f32> {
    let ground = Vec2::new(offset.x, offset.z);
    let line = Vec2::new(toward.x, toward.z);
    let (reach, run) = (ground.length(), line.length());
    if reach <= f32::EPSILON || run <= f32::EPSILON {
        return None;
    }
    let yaw = |v: Vec2| v.x.atan2(v.y);
    let bearing = (yaw(ground) - yaw(line) + PI).rem_euclid(TAU) - PI;
    let edge_cos = (offset.length() * half_angle.cos() - offset.y * toward.y) / (reach * run);
    if edge_cos > 1.0 {
        return None;
    }
    let edge = edge_cos.max(-1.0).acos();
    if bearing.abs() >= edge {
        return None;
    }
    let side = if bearing < 0.0 { -1.0 } else { 1.0 };
    Some(side * edge - bearing)
}

/// The share of what is left that the look point closes in `dt` seconds: the per-tick follow factor
/// carried as frame-rate independent time.
pub fn follow_share(dt: f32) -> f32 {
    1.0 - (1.0 - FOLLOW_PER_TICK).powf(dt * RETAIL_MOVE_TICKS_PER_SEC)
}

/// Where the eye lands when the wall test from `origin` meets geometry `hit` along the way to it.
pub fn short_of_wall(origin: Vec3, eye: Vec3, hit: f32) -> Vec3 {
    origin + (eye - origin).normalize_or_zero() * (hit - WALL_PULL)
}

impl LockedCamera {
    /// The eye's distance from the look point, floored.
    pub fn distance(&self) -> f32 {
        (self.eye - self.look).length().max(DIST_FLOOR)
    }

    /// A camera turn: the eye swings round the look point on the ground plane by `angle` of chase yaw,
    /// keeping its height and its ground distance (`FFXiMain.dll retail-2026-09` RVA 0x1EBB0).
    pub fn orbit(self, angle: f32) -> Self {
        if angle == 0.0 {
            return self;
        }
        let eye = rotate_about(
            Vec2::new(self.eye.x, self.eye.z),
            Vec2::new(self.look.x, self.look.z),
            angle,
        );
        Self {
            eye: Vec3::new(eye.x, self.eye.y, eye.y),
            ..self
        }
    }

    /// A camera tilt: the eye rises by `rise` yalms and nothing else moves (`FFXiMain.dll retail-2026-09`
    /// RVA 0x1F147).
    pub fn lift(self, rise: f32) -> Self {
        Self {
            eye: self.eye + Vec3::Y * rise,
            ..self
        }
    }

    /// One frame of `dt` seconds: the look point follows the blend of the two camera points, the eye is
    /// held inside the cone round the target-to-player line, and its distance eases into the band. The
    /// band push runs along the eye's direction from before the cone turn, as retail's does.
    pub fn step(self, world: &LockedWorld, dt: f32) -> Self {
        let share = follow_share(dt);
        let goal = world.player_point.lerp(world.target_point, LOOK_BLEND);
        let look = self.look + (goal - self.look) * share;

        let offset = self.eye - look;
        let distance = offset.length().max(DIST_FLOOR);
        let along = offset / distance;
        let (toward, frame) = world.cone();
        let held = hold_in_cone(offset, toward, frame.half_angle);
        let push = (distance.clamp(frame.near, frame.far) - distance) * share;

        Self {
            eye: look + held + along * push,
            look,
        }
    }

    /// One frame of a lock's opening: [`Self::step`]'s look point and distance band, with the eye swung
    /// toward the cone's edge on the side of the line it sits nearer by the share of what is left that the
    /// look point closes ([`follow_share`]). From outside the cone it comes in along the hold's arc, which
    /// retail's hold covers in one frame; from inside it turns out round the look point on the ground at
    /// its own height and distance ([`turn_to_edge`]). Retail opens every lock off to that side (seen in
    /// play), so a camera that starts behind the player reaches the edge the way one that starts off the
    /// line does, and both settle the same once the look point reaches the midpoint. Returns the camera
    /// and whether the eye has reached the edge, after which [`Self::step`] holds it there.
    pub fn open(self, world: &LockedWorld, dt: f32) -> (Self, bool) {
        let share = follow_share(dt);
        let goal = world.player_point.lerp(world.target_point, LOOK_BLEND);
        let look = self.look + (goal - self.look) * share;

        let offset = self.eye - look;
        let distance = offset.length().max(DIST_FLOOR);
        let along = offset / distance;
        let (toward, frame) = world.cone();
        let push = along * (distance.clamp(frame.near, frame.far) - distance) * share;
        let at = |held: Vec3| Self {
            eye: look + held + push,
            look,
        };
        if toward.length_squared() <= f32::EPSILON {
            return (at(offset), true);
        }

        let excess = along.angle_between(toward) - frame.half_angle;
        if excess > OPENING_SETTLED_RAD {
            let on_edge = hold_in_cone(offset, toward, frame.half_angle);
            if on_edge == offset {
                return (at(offset), true);
            }
            let eased = hold_in_cone(offset, toward, frame.half_angle + excess * (1.0 - share));
            return (at(eased), share >= 1.0);
        }
        if excess > 0.0 {
            return (at(hold_in_cone(offset, toward, frame.half_angle)), true);
        }
        match turn_to_edge(offset, toward, frame.half_angle) {
            Some(turn) if turn.abs() > OPENING_SETTLED_RAD => {
                (at(offset).orbit(turn * share), share >= 1.0)
            }
            Some(turn) => (at(offset).orbit(turn), true),
            None => (at(offset), true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuluu_render::ChaseCamera;

    /// Retail's default focal length (`RETAIL_DEFAULT_FOCAL_LENGTH`).
    fn default_focal() -> f32 {
        kuluu_render::graphics_settings::RETAIL_DEFAULT_FOCAL_LENGTH
    }

    const FRAME_30: f32 = 1.0 / 30.0;

    fn world(player: Vec3, target: Vec3) -> LockedWorld {
        const PLAYER_RISE: f32 = 1.4;
        const TARGET_RISE: f32 = 1.1;
        LockedWorld {
            player,
            target,
            player_point: player + Vec3::Y * PLAYER_RISE,
            target_point: target + Vec3::Y * TARGET_RISE,
            focal_length: default_focal(),
        }
    }

    fn angle_off_line(cam: &LockedCamera, w: &LockedWorld) -> f32 {
        (cam.eye - cam.look)
            .normalize()
            .angle_between((w.player - w.target).normalize())
    }

    /// Runs a lock's opening on from `cam` until it lands the eye on the cone's edge.
    fn open_until_landed(mut cam: LockedCamera, w: &LockedWorld) -> LockedCamera {
        const FRAMES_MAX: usize = 120;
        for _ in 0..FRAMES_MAX {
            let (next, landed) = cam.open(w, FRAME_30);
            cam = next;
            if landed {
                return cam;
            }
        }
        panic!("the opening never landed: {cam:?}");
    }

    /// The numbers the lock reads at the default 350 focal, worked by hand from the provenance constants:
    /// five yalms from the target the cone is 27.3 degrees and the band runs 5.24 to 5.73 from the look
    /// point; doubling the span narrows the cone and pushes the band out.
    #[test]
    fn the_cone_and_band_follow_the_span_and_the_focal_length() {
        let near5 = lock_frame(5.0, default_focal());
        assert!(
            (near5.half_angle.to_degrees() - 27.33).abs() < 0.01,
            "{near5:?}"
        );
        assert!((near5.near - 5.2388).abs() < 1e-3, "{near5:?}");
        assert!((near5.far - 5.7288).abs() < 1e-3, "{near5:?}");
        let far10 = lock_frame(10.0, default_focal());
        assert!(far10.half_angle < near5.half_angle && far10.near > near5.near);
        let zoomed = lock_frame(5.0, default_focal() * 2.0);
        assert!(
            zoomed.near > near5.near && zoomed.half_angle < near5.half_angle,
            "a longer focal pushes the eye out and narrows the cone"
        );
    }

    #[test]
    fn the_lock_frames_at_retails_focal_whatever_the_menu_fov() {
        use kuluu_render::graphics_settings::DEFAULT_FOV_DEG;
        const WIDE_MENU_FOV: f32 = 80.0;
        const ZOOM: f32 = 2.0;
        let retail_menu = ViewFov::focal_for_deg(DEFAULT_FOV_DEG);
        assert!(
            (lock_focal(retail_menu, DEFAULT_FOV_DEG) - retail_menu).abs() < 1e-3,
            "the menu's retail FOV reads the live focal"
        );
        let wide = ViewFov::focal_for_deg(WIDE_MENU_FOV);
        assert!(
            (lock_focal(wide, WIDE_MENU_FOV) - default_focal()).abs() < 1e-3,
            "a wider menu FOV keeps retail's cone and band"
        );
        assert!(
            (lock_focal(wide * ZOOM, WIDE_MENU_FOV) - default_focal() * ZOOM).abs() < 1e-2,
            "and the zoom keys still scale them"
        );
    }

    #[test]
    fn the_cone_turns_an_eye_outside_it_onto_its_edge_and_leaves_one_inside_alone() {
        let toward = Vec3::Z;
        let half = 30_f32.to_radians();
        let inside = Vec3::new(1.0, 0.0, 4.0);
        assert_eq!(hold_in_cone(inside, toward, half), inside);
        let outside = Vec3::new(4.0, 0.5, 1.0);
        let held = hold_in_cone(outside, toward, half);
        assert!(
            (held.length() - outside.length()).abs() < 1e-4,
            "the turn keeps the distance"
        );
        assert!(
            (held.angle_between(toward) - half).abs() < 1e-4,
            "lands on the edge"
        );
        assert!(held.x > 0.0, "and stays on the side it came from");
        assert_eq!(hold_in_cone(-toward * 3.0, toward, half), -toward * 3.0);
        assert_eq!(hold_in_cone(Vec3::X, Vec3::ZERO, half), Vec3::X);
    }

    /// Locking on to a target off to the player's side with the camera behind the player: the opening
    /// swings the eye part of the way each frame, and only to the cone's edge on the side it was already
    /// on, so the view opens off to that side of the target line instead of squarely behind the player.
    #[test]
    fn a_lock_begins_on_the_cameras_own_side_of_the_line() {
        let player = Vec3::ZERO;
        let target = Vec3::new(5.0, 0.0, 0.0);
        let w = world(player, target);
        let band = lock_frame(5.0, default_focal());
        // The camera sat behind a player facing -Z, a little up, at a distance already inside the band so
        // only the cone moves it.
        let free_look = w.player_point;
        let back = Vec3::new(0.0, 0.15, 1.0).normalize() * (band.near + band.far) * 0.5;
        let begun = LockedCamera {
            eye: free_look + back,
            look: free_look,
        };
        let (first, landed) = begun.open(&w, FRAME_30);
        assert!(
            !landed && angle_off_line(&first, &w) > band.half_angle + 0.1,
            "the first frame swings only part of the way in: {} vs {}",
            angle_off_line(&first, &w).to_degrees(),
            band.half_angle.to_degrees()
        );
        let cam = open_until_landed(first, &w);
        assert!(
            (angle_off_line(&cam, &w) - band.half_angle).abs() < 1e-3,
            "the opening lands on the cone's edge: {} vs {}",
            angle_off_line(&cam, &w).to_degrees(),
            band.half_angle.to_degrees()
        );
        assert!(
            cam.eye.z > cam.look.z + 1.0,
            "and stays on its own (+Z) side of the line: {cam:?}"
        );
    }

    /// An eye inside the cone turns on the ground out to the edge on the side it leans to, at its own
    /// height and distance; one square on the line goes to the player's right, and one already at or past
    /// the edge, or rising too steeply to reach it by turning, is not turned.
    #[test]
    fn an_eye_inside_the_cone_turns_out_to_the_edge_on_its_own_side() {
        // The player stands on +Z of the target, so facing it the player's right is +X.
        let toward = Vec3::Z;
        let half = 30_f32.to_radians();
        let look = Vec3::new(0.0, 1.0, 0.0);
        let turned = |offset: Vec3| {
            let turn = turn_to_edge(offset, toward, half)?;
            Some(
                LockedCamera {
                    eye: look + offset,
                    look,
                }
                .orbit(turn)
                .eye - look,
            )
        };
        for (offset, side) in [
            (Vec3::new(0.3, 0.5, 5.0), 1.0),
            (Vec3::new(-0.3, 0.5, 5.0), -1.0),
            (Vec3::new(0.0, 0.5, 5.0), 1.0),
        ] {
            let out = turned(offset).expect("an eye inside the cone turns out");
            assert!(
                (out.angle_between(toward) - half).abs() < 1e-4,
                "{offset:?} lands on the edge: {} deg",
                out.angle_between(toward).to_degrees()
            );
            assert!(
                (out.y - offset.y).abs() < 1e-5,
                "{offset:?} keeps its height"
            );
            assert!(
                (out.length() - offset.length()).abs() < 1e-4,
                "{offset:?} keeps its distance"
            );
            assert!(
                out.x * side > 1.0,
                "{offset:?} turns out to its own side: {out:?}"
            );
        }
        assert_eq!(turn_to_edge(Vec3::new(4.0, 0.0, 1.0), toward, half), None);
        assert_eq!(turn_to_edge(Vec3::new(0.0, 5.0, 1.0), toward, half), None);
        assert_eq!(turn_to_edge(Vec3::Y * 5.0, toward, half), None);
        assert_eq!(turn_to_edge(Vec3::Z * 5.0, Vec3::Y, half), None);
    }

    /// A lock taken with the camera square behind the player, the target straight ahead, opens off to the
    /// side instead of staying behind, and settles where a lock taken with the camera off to the side
    /// settles: the same angle off the line, the same side.
    #[test]
    fn a_lock_from_behind_opens_to_the_side_and_settles_like_one_from_off_the_line() {
        let w = world(Vec3::ZERO, Vec3::new(0.0, 0.0, -5.0));
        let half = lock_frame(5.0, default_focal()).half_angle;
        let free_look = w.player_point;
        let settle = |from: Vec3| {
            let begun = LockedCamera {
                eye: free_look + from.normalize() * ChaseCamera::DIST_MAX,
                look: free_look,
            };
            let (first, _) = begun.open(&w, FRAME_30);
            let opened = open_until_landed(first, &w);
            let mut cam = opened;
            for _ in 0..240 {
                cam = cam.step(&w, FRAME_30);
            }
            (first, opened, cam)
        };
        let (first, opened, from_behind) = settle(Vec3::new(0.0, 0.15, 1.0));
        assert!(
            angle_off_line(&first, &w) < half - 0.05 && first.eye.x > 0.0,
            "the first locked frame starts out toward the edge on the player's right: {} vs {} deg, {first:?}",
            angle_off_line(&first, &w).to_degrees(),
            half.to_degrees()
        );
        assert!(
            (angle_off_line(&opened, &w) - half).abs() < 1e-3,
            "the opening lands on the cone's edge: {} vs {} deg",
            angle_off_line(&opened, &w).to_degrees(),
            half.to_degrees()
        );
        assert!(opened.eye.x > 1.0, "to the player's right: {opened:?}");
        let (_, _, from_the_side) = settle(Vec3::new(1.0, 0.15, 0.0));
        let behind = angle_off_line(&from_behind, &w);
        let side = angle_off_line(&from_the_side, &w);
        assert!(
            behind > half * 0.5 && (behind - side).abs() < 1_f32.to_radians(),
            "settled {} deg off the line from behind, {} deg from the side",
            behind.to_degrees(),
            side.to_degrees()
        );
        assert!(from_behind.eye.x > 1.0 && from_the_side.eye.x > 1.0);
    }

    /// The opening swing closes a quarter of what is left per tick, from outside the cone and from inside
    /// it alike, with the look point already at the midpoint and the eye inside the band so that nothing
    /// else moves it.
    #[test]
    fn a_lock_opening_swings_a_quarter_of_the_way_per_tick() {
        let w = world(Vec3::ZERO, Vec3::new(0.0, 0.0, -5.0));
        let band = lock_frame(5.0, default_focal());
        let look = w.player_point.lerp(w.target_point, LOOK_BLEND);
        let off_line = |angle: f32| LockedCamera {
            eye: look + Vec3::new(angle.sin(), 0.0, angle.cos()) * (band.near + band.far) * 0.5,
            look,
        };
        let from_edge = |cam: &LockedCamera| (angle_off_line(cam, &w) - band.half_angle).abs();
        for start in [band.half_angle * 3.0, band.half_angle * 0.2] {
            let begun = off_line(start);
            let (cam, landed) = begun.open(&w, 1.0 / RETAIL_MOVE_TICKS_PER_SEC);
            assert!(!landed, "{} deg is not on the edge yet", start.to_degrees());
            assert!(
                (from_edge(&cam) - from_edge(&begun) * (1.0 - FOLLOW_PER_TICK)).abs() < 1e-4,
                "from {} deg one tick leaves {} deg of {} deg to swing",
                start.to_degrees(),
                from_edge(&cam).to_degrees(),
                from_edge(&begun).to_degrees()
            );
            assert_eq!(cam.look, look);
            assert!((cam.distance() - begun.distance()).abs() < 1e-4);
        }
    }

    /// A side step round the target with the eye centred behind the player: while the line swings inside
    /// the cone the eye does not move at all, then it is carried at the edge. Starting centred the slack is
    /// one half-angle of swing; from the far edge it is two, which is why a reversed side step travels
    /// about twice as far before the camera catches.
    #[test]
    fn a_side_step_runs_free_inside_the_cone_then_drags_the_eye() {
        const RADIUS: f32 = 5.0;
        const STEP_YALMS: f32 = 0.125;
        let target = Vec3::ZERO;
        let at = |travel: f32| {
            let a = travel / RADIUS;
            Vec3::new(RADIUS * a.sin(), 0.0, RADIUS * a.cos())
        };
        let w0 = world(at(0.0), target);
        let look0 = w0.player_point.lerp(w0.target_point, LOOK_BLEND);
        let mut cam = LockedCamera {
            eye: look0 + Vec3::Z * 5.5,
            look: look0,
        };
        for _ in 0..120 {
            cam = cam.step(&w0, FRAME_30);
        }
        let settled = cam.eye;
        let mut first_moved = None;
        let mut travel = 0.0;
        while travel < 12.0 {
            travel += STEP_YALMS;
            let w = world(at(travel), target);
            let before = cam.eye;
            cam = cam.step(&w, FRAME_30);
            let half = lock_frame((w.player - w.target).length(), w.focal_length).half_angle;
            assert!(
                angle_off_line(&cam, &w) <= half + 1e-3,
                "the eye never sits outside the cone ({travel} yalms)"
            );
            if first_moved.is_none() && (cam.eye - before).length() > 1e-3 {
                first_moved = Some(travel);
            }
        }
        let first_moved = first_moved.expect("the cone edge must catch the eye");
        assert!(
            first_moved > 1.0,
            "the eye held still for {first_moved} yalms of side step from {settled:?}"
        );
    }

    /// The look point's follow and the band's ease are time, not frames: the same 1.5 seconds at 30 and at
    /// 60 frames a second lands both in the same place. (The cone itself is a hold, not a rate: it acts on
    /// whichever frame finds the eye outside it.)
    #[test]
    fn the_follow_and_the_band_are_frame_rate_independent() {
        let w = world(Vec3::ZERO, Vec3::new(0.0, 0.0, -4.0));
        let start = LockedCamera {
            eye: Vec3::new(0.0, 1.6, 8.0),
            look: Vec3::new(0.0, 1.4, 0.0),
        };
        let mut slow = start;
        for _ in 0..45 {
            slow = slow.step(&w, FRAME_30);
        }
        let mut fast = start;
        for _ in 0..90 {
            fast = fast.step(&w, FRAME_30 / 2.0);
        }
        assert!((slow.look - fast.look).length() < 1e-3);
        assert!(
            (slow.eye - fast.eye).length() < 0.02,
            "{slow:?} vs {fast:?}"
        );
    }

    /// The eye eases into the band a quarter of the gap per tick and holds still inside it.
    #[test]
    fn the_distance_eases_into_the_band_and_holds_inside_it() {
        let w = world(Vec3::ZERO, Vec3::new(0.0, 0.0, -5.0));
        let look = w.player_point.lerp(w.target_point, LOOK_BLEND);
        let band = lock_frame(5.0, default_focal());
        let close = LockedCamera {
            eye: look + Vec3::Z * 2.0,
            look,
        };
        let one_tick = close.step(&w, 1.0 / RETAIL_MOVE_TICKS_PER_SEC);
        let want = 2.0 + (band.near - 2.0) * FOLLOW_PER_TICK;
        assert!(
            (one_tick.distance() - want).abs() < 1e-3,
            "{}",
            one_tick.distance()
        );
        let inside = LockedCamera {
            eye: look + Vec3::Z * (band.near + band.far) * 0.5,
            look,
        };
        assert_eq!(inside.step(&w, FRAME_30).eye, inside.eye);
    }

    #[test]
    fn a_wall_pulls_the_eye_in_short_of_the_hit() {
        let origin = Vec3::new(0.0, 1.0, 0.0);
        let eye = Vec3::new(0.0, 1.0, 6.0);
        assert_eq!(
            short_of_wall(origin, eye, 4.0),
            Vec3::new(0.0, 1.0, 4.0 - WALL_PULL)
        );
    }

    #[test]
    fn a_turn_swings_the_eye_round_the_look_point_on_the_ground() {
        let cam = LockedCamera {
            eye: Vec3::new(0.0, 3.0, 5.0),
            look: Vec3::new(0.0, 1.0, 0.0),
        };
        let turned = cam.orbit(std::f32::consts::FRAC_PI_2);
        assert_eq!(turned.eye.y, cam.eye.y);
        let ground = |v: Vec3| Vec2::new(v.x, v.z);
        assert!(
            (ground(turned.eye - turned.look).length() - ground(cam.eye - cam.look).length()).abs()
                < 1e-4
        );
        assert_eq!(cam.lift(0.5).eye, Vec3::new(0.0, 3.5, 5.0));
    }
}
