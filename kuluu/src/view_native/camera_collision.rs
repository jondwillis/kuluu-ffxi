use bevy::prelude::*;

use kuluu_render::components::IsSelf;
use kuluu_render::dat_mzb::{CameraCollisionSource, DrawDistance, ZoneGeomMode};
use kuluu_render::scene::BakedActor;
use kuluu_render::snapshot::SceneState;
use kuluu_render::{
    third_person_anchor_y, yaw_for_heading, CameraMode, ChaseCamera, OperatorCamera,
};

use kuluu_render::ffxi_actor_render::FfxiRenderActor;
use kuluu_render::mouse::{
    camera_height_pitch_rate_rad_per_sec, camera_orbit_yaw_rate_rad_per_sec,
    CAMERA_EYE_RISE_YALMS_PER_SEC,
};

use super::collision_bvh::{CollisionBvh, ZoneCollisionBvh};
use super::locked_camera::{
    follow_share, lock_focal, short_of_wall, LockedCamera, LockedWorld, PLAYER_POINT_SLOT,
    TARGET_POINT_SLOT, WALL_RAY_RISE,
};

/// The lock-release catch: a released camera eases back behind the body at this
/// rate (framerate-independent exponential) once the gap is inside
/// super::input::LOCK_CAM_ARRIVAL_GAP_RAD; wider gaps turn at
/// super::input::LOCK_CAM_MAX_TURN_RAD_PER_SEC. A deliberate tuning — no retail
/// number fixes how fast a released camera returns, and retail's own spring-back
/// consumer (engage FFXiMain.dll retail-2026-09 RVA 0xA6998 orbiting by −ref ×
/// 6/max(dist, .01) at applier 0x1EBB0) is read in
/// `.agents/skills/retail-observe/references/2026-10-07-camera-orbit-channels.md`
/// as a channel gated separately from direct camera actions, which this build
/// does not apply to manual orbit input.
const LOCK_TURN_RATE: f32 = 12.0;

/// The band's inner edge sits at zero, not a fraction of the zoom: no ratio
/// constant exists in FFXiMain.dll retail-2026-09's camera event block — the
/// half-zoom figure came from XIClient's CameraManager, which is a different
/// build. The eye's distance is set by the zoom (and clamped below by
/// CAMERA_MIN_DISTANCE through the wall pipeline); re-anchor events move it.
const LEASH_INNER_EDGE: f32 = 0.0;

/// Drag `point` toward `anchor` until it is no farther than `max` and no nearer
/// than `min`; inside the band it does not move. `fallback` is the direction
/// used when the two coincide. Instant: a clamp to a distance, never a rate.
pub fn leash(point: Vec2, anchor: Vec2, min: f32, max: f32, fallback: Vec2) -> Vec2 {
    let v = point - anchor;
    let d = v.length();
    if d < 1e-5 {
        return anchor + fallback * min;
    }
    let clamped = d.clamp(min, max);
    if clamped == d {
        point
    } else {
        anchor + v / d * clamped
    }
}

/// `next` as a yaw continuous with `yaw`: step by the wrapped difference, so
/// the accumulated chase yaw never jumps a full turn.
fn continuous_yaw(yaw: f32, next: f32) -> f32 {
    yaw + ((next - yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI)
}

/// One step of the catch closing `gap` radians of turn: a wide gap turns at the
/// constant max rate, the exponential settles in inside the arrival gap.
fn catch_step(gap: f32, dt: f32) -> f32 {
    if gap.abs() > super::input::LOCK_CAM_ARRIVAL_GAP_RAD {
        let step = super::input::LOCK_CAM_MAX_TURN_RAD_PER_SEC * dt;
        gap.signum() * step.min(gap.abs())
    } else {
        let alpha = 1.0 - (-LOCK_TURN_RATE * dt).exp();
        gap * alpha
    }
}

/// One step of the released camera's turn home ([`catch_step`]).
pub(crate) fn lock_turn(chase_yaw: f32, want: f32, dt: f32) -> f32 {
    chase_yaw + catch_step(continuous_yaw(chase_yaw, want) - chase_yaw, dt)
}

/// Where the camera's focus and eye sit in the world, bevy xz, carried frame
/// to frame; `yaw` is the chase yaw this system last wrote, so a different
/// value next frame means the player turned the camera.
#[derive(Default)]
pub struct LeashState {
    focus: Option<Vec2>,
    eye: Option<Vec2>,
    yaw: Option<f32>,
    /// The chase pitch at the end of last frame, so a different value now is a tilt the player made.
    pitch: Option<f32>,
    /// The point the camera looked at last frame: a lock begins from it.
    look: Option<Vec3>,
    /// The previous frame was locked on: the release frame starts the release
    /// ease and the handoff.
    was_locked: bool,
    /// Yaw the just-released camera eases back to (behind the body's
    /// heading): the lock leaves the rig on the locked view's bearing, and the
    /// released camera closes that with a fast catch-up turn instead of
    /// parking there or snapping. A manual turn, a new lock, or a zone snap
    /// takes the camera back.
    release_yaw: Option<f32>,
    /// Retail's locked camera ([`super::locked_camera`]) and the target it is locked on, while a lock
    /// holds.
    locked: Option<Lock>,
    /// The locked target and the height of its camera point, read once per lock as retail does.
    target_rise: Option<(u32, f32)>,
    /// How far the rendered view still sits from the chase rig's after a lock released.
    handoff: Option<Handoff>,
}

/// A lock in play: the target, the locked camera, and whether the lock's opening
/// ([`LockedCamera::open`]) is still swinging the eye out to the cone's edge.
#[derive(Clone, Copy)]
struct Lock {
    target: u32,
    view: LockedCamera,
    opening: bool,
}

/// The rendered eye and look point minus the chase rig's. Retail runs one camera, so its free camera
/// starts wherever the lock left the eye. Kuluu's chase rig keeps its own distance and pitch through a
/// lock, so on release the view eases from the locked camera back onto the rig instead of cutting to
/// it, at the share per tick retail's look point follows its goal with
/// ([`super::locked_camera::follow_share`]).
#[derive(Clone, Copy, Default)]
struct Handoff {
    eye: Vec3,
    look: Vec3,
}

impl Handoff {
    fn eased(self, dt: f32) -> Option<Self> {
        let keep = 1.0 - follow_share(dt);
        let next = Self {
            eye: self.eye * keep,
            look: self.look * keep,
        };
        (next.eye.length() > HANDOFF_SETTLED_YALMS || next.look.length() > HANDOFF_SETTLED_YALMS)
            .then_some(next)
    }
}

/// Whether the chase camera should collide with zone MMB static placements (Mog
/// House furniture and the exit-door model). Inside a Mog House this is always
/// on — retail's "furniture camera collision" — because the interior is sealed
/// only by ~two dozen MMB placements and the closed door is one of them; without
/// this the camera slips through the doorway gap in the MZB wall and escapes the
/// room. Enabling it zone-wide would raycast thousands of city placements every
/// frame, so outside a Mog House it stays gated on the explicit source setting.
/// The BVH-build gate ([`super::collision_bvh::build_collision_bvh_system`]) and
/// the camera raycast MUST use this same predicate or they disagree on coverage.
///
/// The gap is real and still open: `mh_391_doorway_is_a_gap_in_mzb_collision`
/// finds MZB walls on 23 of 24 headings from the spawn anchor and nothing at all
/// on the 24th. So this is not made redundant by kuluu-0nnl putting every MZB
/// submesh into the collision set — MMB placements are a separate set entirely.
///
/// Note the two camera sources now differ in *policy*, not just coverage: MZB
/// triangles are filtered by retail's `DoubleSidedSkipPolicy`
/// ([`ffxi_dat::mzb::double_sided_skip`]) while MMB models carry no
/// `CollisionMeshHeader.Flags` and are raycast whole.
pub fn camera_collides_with_mmb(source: CameraCollisionSource, in_mog_house: bool) -> bool {
    source.uses_mmb() || in_mog_house
}

// research/xim/src/jsMain/kotlin/xim/poc/camera/PolarCamera.kt getAdjustedRadiusFromCollision collisionDistance —
// `(distance - 0.25f).coerceAtLeast(0.5f)`: pad off the wall, but never pull the
// camera closer than 0.5 to the anchor (tiny interiors like the Mog House would
// otherwise collapse it inside the character model).
const WALL_PAD: f32 = 0.25;

const CAMERA_MIN_DISTANCE: f32 = 0.5;

/// How far below the pivot the eye may sit at full down-tilt, yalms. Retail adds the tilt to
/// the eye's world Y and rolls the camera back toward the player when you force it down
/// (CameraManager::UpdatePlayerFollowingCamera), so its reachable envelope never parks a full
/// boom under the anchor; the zone BVH raycast cannot see unauthored floor gaps, so without this
/// a fully lowered eye slips under the floor and views through it from below.
const EYE_FLOOR_BELOW_PIVOT: f32 = 0.5;

const OUTWARD_LERP: f32 = 0.18;

const INWARD_LERP: f32 = 0.45;

/// The released view counts as back on the chase rig once both offsets are this short.
const HANDOFF_SETTLED_YALMS: f32 = 1e-3;

/// A Q/E turn the camera owes lands outright once the catch leaves less than this of it.
const TURN_OWED_SETTLED_RAD: f32 = 1e-3;

/// The single chase-camera authority: a leash with slack at both ends, the
/// camera spring that decides how fast the eye reaches the leash's goal, the
/// locked-on camera, and the wall pull-in against the zone MZB BVH, with one
/// transform write at the end.
///
/// While a running event holds the camera (CutsceneMode::camera_locked) this
/// system writes nothing: the cutscene camera route (kuluu-render's
/// advance_cutscene_camera_task, registered after this one) owns the operator
/// camera, and a chase write would pull the scripted view back behind the
/// player every frame. The leash resets while held, so on release the chase
/// re-seats behind the player from a clean state (retail's DEFCAMERA release
/// re-seats the chase the same way).
///
/// The camera is a world point (the eye) on a leash from a focus point. The
/// focus is what the camera looks at: the player's anchor. It holds still
/// while the pivot moves inside the dead zone (the Debug menu Camera_leash
/// row) and is dragged to exactly that distance once the pivot leaves it, so
/// the per-frame wobble of the player transform (a server correction, an
/// interpolation seam, a stair step) never reaches the camera. The eye keeps
/// its spot within the zoom band (outer edge = the zoom distance; inner edge
/// zero — LEASH_INNER_EDGE), and outside it the leash clamps to the band's
/// edge: retail has no positional pull at all. `camera_spring` eases only the boom's
/// distance, never the bearing. The yaw
/// is the direction from the focus to the eye; a frame where something else wrote
/// chase.yaw (the mouse, the yaw keys, a stair warp) swings the eye around the focus to that yaw at
/// once, whatever `camera_spring` says. A Q/E turn
/// is paid separately (ChaseCamera::turn_owed): the rig swings round the
/// player after it at the lock-release catch, with the spring on or off.
///
/// Locked on, retail's locked camera renders the view instead
/// ([`super::locked_camera`]): it looks at the point midway between the player
/// and the target and holds the eye inside a cone round the line from the
/// target through the player. The lock opens by swinging the eye out to the
/// cone's edge, off to whichever side of the line it sat nearer, a quarter of
/// the way per tick, and a side step carries the player a few yalms before the
/// camera follows. The mouse and the pad stick
/// still turn and tilt that eye inside the cone. The chase rig runs on underneath on the
/// locked view's bearing, its eye carried on the boom at its own distance and
/// pitch, so the lock never changes the free camera. On release the view eases
/// back onto the rig (LeashState::handoff) and the rig's yaw eases back behind
/// the body's heading in well under a second (LeashState::release_yaw); a
/// manual turn, a new lock, or a zone snap takes the camera back.
///
/// Init sync aligns yaw behind the player on the first frame.
/// snap_to_anchor (zone/warp) resets the leash to the exact position.
///
/// The collision pull-in: a ray from the pivot along the boom, where walls
/// block and mobs do not — the same solid world the walker sweeps (the door
/// triangles join this ray when the obstacle set lands). The nearest hit
/// shortens the boom so the camera does not clip through geometry; cast from
/// the pivot (the player), not the glide origin, so wall pull-in is measured
/// from where the camera is actually looking. The BVH rebuilds ~1 s after
/// zone geometry goes quiet; until then there is no ray and the boom runs
/// unclipped.
///
/// Boom-length easing (not position) snaps in fast when a wall appears and
/// eases out slow when it clears, so the camera does not jitter at wall
/// edges; the camera spring moves the eye, this only smooths the pull-in
/// distance. The single write places the eye along the boom from the focus
/// while the camera looks at the focus, so however the rig moves the player
/// stays centered and rotation orbits the focus; with the spring off the eye
/// sits on the leash's goal and this reduces to the plain centered behavior.
pub fn resolve_camera(
    mode: Res<CameraMode>,
    settings: Res<kuluu_render::GraphicsSettings>,
    mut chase: ResMut<ChaseCamera>,
    step: Res<kuluu_render::camera::CameraStepSmoothing>,
    time: Res<Time>,
    scene_state: Res<SceneState>,
    cutscene: Res<kuluu_render::cutscene::CutsceneMode>,
    zone_bvh: Res<ZoneCollisionBvh>,
    self_q: Query<
        (&Transform, Option<&BakedActor>, Option<&Children>),
        (With<IsSelf>, Without<OperatorCamera>),
    >,
    mut cam_q: Query<&mut Transform, (With<OperatorCamera>, Without<IsSelf>)>,
    lock_on: Res<kuluu_render::lock_on::LockOn>,
    target_q: Query<
        (
            &kuluu_render::components::WorldEntity,
            &Transform,
            Option<&BakedActor>,
            Option<&Children>,
        ),
        (Without<IsSelf>, Without<OperatorCamera>),
    >,
    view_fov: Res<kuluu_render::ViewFov>,
    render_q: Query<(&FfxiRenderActor, &GlobalTransform)>,
    mut smoothed_effective: Local<Option<f32>>,
    mut leash_state: Local<LeashState>,
) {
    if !matches!(*mode, CameraMode::Chase) {
        *smoothed_effective = None;
        *leash_state = LeashState::default();
        chase.turn_owed = 0.0;
        return;
    }
    // A running event holding the camera owns the operator camera (the
    // cutscene camera route advances or freezes it after this system): the
    // chase must not attach to the player while the script owns the view.
    if cutscene.camera_locked {
        *smoothed_effective = None;
        *leash_state = LeashState::default();
        chase.turn_owed = 0.0;
        return;
    }
    let Ok((self_t, baked, self_children)) = self_q.single() else {
        *smoothed_effective = None;
        *leash_state = LeashState::default();
        chase.turn_owed = 0.0;
        return;
    };
    let Ok(mut cam_t) = cam_q.single_mut() else {
        return;
    };

    if !chase.synced_initial {
        // A freshly spawned camera owns no history: the resolver-local leash and eased boom distance
        // belong to whatever session last ran, so a same-zone reentry would otherwise rebuild on them.
        *smoothed_effective = None;
        *leash_state = LeashState::default();
        chase.yaw = yaw_for_heading(scene_state.snapshot.self_pos.heading);
        chase.synced_initial = true;
    }

    let player_pos = self_t.translation;
    let anchor_y = Vec3::Y * (third_person_anchor_y(baked) - step.offset);
    let pivot_y = player_pos.y + anchor_y.y;
    let pivot_xz = Vec2::new(player_pos.x, player_pos.z);
    let dt = time.delta_secs().max(1e-4);

    // Debug menu Camera_leash row: the focus's pause box in yalms — the
    // player moves inside it without the camera reacting, and the box is
    // that pause. 0 turns it off; the eye's start/stop spring is a separate
    // thing (settings.camera_spring) and does not ride on the row.
    let leash_on = settings.camera_leash_yalms > 0.0;

    let cos_p = chase.pitch.cos().max(1e-3);
    let sin_p = chase.pitch.sin();
    let max_h = chase.orbit_radius() * cos_p;
    let min_h = LEASH_INNER_EDGE;
    let yaw_dir = |yaw: f32| Vec2::new(yaw.sin(), yaw.cos());

    // Focus: held inside the dead zone, dragged to its edge outside it.
    let focus = match leash_state.focus {
        Some(f) if !chase.snap_to_anchor && leash_on => {
            leash(f, pivot_xz, 0.0, settings.camera_leash_yalms, Vec2::ZERO)
        }
        _ => pivot_xz,
    };

    // Yaw something else wrote since last frame (arrows, mouse): the eye
    // branch below treats it as a manual turn, and it takes the camera back
    // from the release ease and from a Q/E turn it still owes.
    let manual_yaw = leash_state
        .yaw
        .map(|ly| continuous_yaw(ly, chase.yaw) - ly)
        .unwrap_or(0.0);

    let lock_target = lock_on
        .target_id
        .and_then(|id| target_q.iter().find(|(we, ..)| we.id == id))
        .filter(|_| !chase.snap_to_anchor);
    let locked_view = match lock_target {
        Some((target_we, target_t, target_baked, target_children)) => {
            let target_feet = target_t.translation;
            let target_rise = match leash_state.target_rise {
                Some((id, rise)) if id == target_we.id => Some(rise),
                _ => point_rise(target_children, &render_q, TARGET_POINT_SLOT, target_feet),
            };
            let player_rise = point_rise(self_children, &render_q, PLAYER_POINT_SLOT, player_pos)
                .unwrap_or(anchor_y.y);
            let world = LockedWorld {
                player: player_pos,
                target: target_feet,
                player_point: player_pos + Vec3::Y * player_rise,
                target_point: target_feet
                    + Vec3::Y * target_rise.unwrap_or_else(|| third_person_anchor_y(target_baked)),
                focal_length: lock_focal(view_fov.focal_length, settings.fov_deg),
            };
            // A lock begins from the camera as it stands: the eye where it rendered, the look point where
            // it looked. A new target opens the lock again from wherever the last one left the camera.
            let (held, opening) = match leash_state.locked {
                Some(lock) => (lock.view, lock.opening || lock.target != target_we.id),
                None => (
                    LockedCamera {
                        eye: cam_t.translation,
                        look: leash_state
                            .look
                            .unwrap_or(Vec3::new(focus.x, pivot_y, focus.y)),
                    },
                    true,
                ),
            };
            // The player's own turns and tilts still move the eye (`FFXiMain.dll retail-2026-09` RVA
            // 0x1F01F..0x1F14A): a turn swings it round the look point at the orbit rate normalised by
            // its distance, which the free-run byte the lock cleared switches on, and a tilt lifts it by
            // the rise the input system priced into its pitch step.
            let last_pitch = leash_state.pitch.unwrap_or(chase.pitch);
            let orbit = manual_yaw
                * camera_orbit_yaw_rate_rad_per_sec(1.0, false, Some(held.distance()))
                / camera_orbit_yaw_rate_rad_per_sec(1.0, true, None);
            let tilt_rate =
                camera_height_pitch_rate_rad_per_sec(1.0, last_pitch, Some(chase.distance));
            let rise = if tilt_rate == 0.0 {
                0.0
            } else {
                (chase.pitch - last_pitch) / tilt_rate * CAMERA_EYE_RISE_YALMS_PER_SEC
            };
            let turned = held.orbit(orbit).lift(rise);
            let (mut view, opening) = if opening {
                let (view, landed) = turned.open(&world, dt);
                (view, !landed)
            } else {
                (turned.step(&world, dt), false)
            };
            // Retail's wall test runs from just above the player's feet to the eye and keeps the eye
            // short of a hit (`FFXiMain.dll retail-2026-09` RVA 0x20651..0x20765).
            if let Some(bvh) = zone_bvh.0.as_ref() {
                let origin = player_pos + Vec3::Y * WALL_RAY_RISE;
                let to_eye = view.eye - origin;
                let reach = to_eye.length();
                if reach > f32::EPSILON {
                    if let Some(hit) = bvh.ray_cast(origin, to_eye / reach, reach) {
                        view.eye = short_of_wall(origin, view.eye, hit);
                    }
                }
            }
            // The chase yaw follows the view, so whatever reads it sees where the camera faces and the rig
            // underneath stays on the locked bearing.
            let ground = Vec2::new(view.eye.x, view.eye.z) - pivot_xz;
            if ground.length_squared() > f32::EPSILON {
                chase.yaw = continuous_yaw(chase.yaw, ground.x.atan2(ground.y));
            }
            Some((
                Lock {
                    target: target_we.id,
                    view,
                    opening,
                },
                target_rise,
            ))
        }
        None => None,
    };
    let locked = locked_view.is_some();

    // Q/E's turn: the rig swings round the player toward the yaw the keys gave it, at the release catch,
    // so it trails the player's turn a little and settles once they come up. A release ease under way
    // turns with it, so the camera still comes home behind the turned body.
    if locked || chase.snap_to_anchor || manual_yaw != 0.0 {
        chase.turn_owed = 0.0;
    }
    let qe_catch = match catch_step(chase.turn_owed, dt) {
        step if (chase.turn_owed - step).abs() < TURN_OWED_SETTLED_RAD => chase.turn_owed,
        step => step,
    };
    chase.turn_owed -= qe_catch;
    chase.yaw += qe_catch;
    if let Some(release) = leash_state.release_yaw.as_mut() {
        *release += qe_catch;
    }

    // Lock release: the rig is already where the free camera left it, on the
    // lock's last bearing, and springs back behind the body's heading in well
    // under a second.
    if leash_state.was_locked && !locked && manual_yaw == 0.0 {
        leash_state.release_yaw = Some(yaw_for_heading(scene_state.snapshot.self_pos.heading));
    }
    if let Some(release) = leash_state.release_yaw {
        if manual_yaw != 0.0 || locked || chase.snap_to_anchor {
            leash_state.release_yaw = None;
        } else {
            let next = lock_turn(chase.yaw, release, dt);
            let settled = (continuous_yaw(next, release) - next).abs() < 1e-3;
            chase.yaw = next;
            if settled {
                leash_state.release_yaw = None;
            }
        }
    }

    // A turn of the yaw swings the whole rig — eye and focus together — so nothing jumps: arrows,
    // mouse drag, Q/E's catch or the release ease wrote chase.yaw since last frame. Swapping that swing
    // for an orbit against the request is what made a spring-on build reverse manual and mouse yaw;
    // `manual_orbit_preserves_direction_with_each_spring_setting` fails when it creeps back. Retail's
    // separately gated actor-facing reference channel — read in
    // `.agents/skills/retail-observe/references/2026-10-07-camera-orbit-channels.md` from the body-facing
    // producer at FFXiMain.dll retail-2026-09 RVA 0xA692A..0xA6998 — is left unimplemented here, because
    // that record does not establish applying it to direct camera actions.
    let (focus, eye_prev) = match (leash_state.eye, leash_state.yaw) {
        (Some(e), _) if !chase.snap_to_anchor && locked => {
            (focus, focus + yaw_dir(chase.yaw) * (e - focus).length())
        }
        (Some(e), Some(last_yaw)) if !chase.snap_to_anchor => {
            if last_yaw == chase.yaw {
                (focus, e)
            } else {
                let d = continuous_yaw(last_yaw, chase.yaw) - last_yaw;
                (
                    rotate_about(focus, pivot_xz, d),
                    rotate_about(e, pivot_xz, d),
                )
            }
        }
        _ => (focus, focus + yaw_dir(chase.yaw) * max_h),
    };
    let eye = leash(eye_prev, focus, min_h, max_h, yaw_dir(chase.yaw));
    let to_eye = eye - focus;
    if !locked {
        chase.yaw = continuous_yaw(chase.yaw, to_eye.x.atan2(to_eye.y));
    }

    let pivot = Vec3::new(focus.x, pivot_y, focus.y);
    let dir = Vec3::new(chase.yaw.sin() * cos_p, sin_p, chase.yaw.cos() * cos_p);
    let wanted = to_eye.length().max(min_h) / cos_p;

    let mut hit_t = wanted;
    if let Some(bvh) = zone_bvh.0.as_ref() {
        if let Some(t) = bvh.ray_cast(pivot, dir, wanted) {
            hit_t = t.min(hit_t);
        }
    }

    let target = clamped_camera_distance(hit_t, wanted);

    let mut effective = if !settings.camera_spring || chase.snap_to_anchor {
        target
    } else {
        match *smoothed_effective {
            Some(prev) if target < prev => target * INWARD_LERP + prev * (1.0 - INWARD_LERP),
            Some(prev) => prev + (target - prev) * OUTWARD_LERP,
            None => target,
        }
    };
    // The down-tilt floor: shorten the boom so the eye never drops a full
    // EYE_FLOOR_BELOW_PIVOT under the pivot — retail's roll-toward-the-player.
    if dir.y < 0.0 {
        effective = effective.min((EYE_FLOOR_BELOW_PIVOT / -dir.y).max(CAMERA_MIN_DISTANCE));
    }
    *smoothed_effective = Some(effective);
    let rig_eye = pivot + dir * effective;

    let handoff = match (leash_state.was_locked, leash_state.locked) {
        _ if locked || chase.snap_to_anchor => None,
        (true, Some(last)) => Handoff {
            eye: last.view.eye - rig_eye,
            look: last.view.look - pivot,
        }
        .eased(dt),
        _ => leash_state.handoff.and_then(|h| h.eased(dt)),
    };
    let (view_eye, view_look) = match (locked_view, handoff) {
        (Some((lock, _)), _) => (lock.view.eye, lock.view.look),
        (None, Some(h)) => (rig_eye + h.eye, pivot + h.look),
        (None, None) => (rig_eye, pivot),
    };
    cam_t.translation = view_eye;
    cam_t.look_at(view_look, Vec3::Y);

    *leash_state = LeashState {
        focus: Some(focus),
        eye: Some(eye),
        yaw: Some(chase.yaw),
        pitch: Some(chase.pitch),
        look: Some(view_look),
        was_locked: locked,
        release_yaw: if locked {
            None
        } else {
            leash_state.release_yaw
        },
        locked: locked_view.map(|(lock, _)| lock),
        target_rise: locked_view.and_then(|(lock, rise)| rise.map(|rise| (lock.target, rise))),
        handoff,
    };
    chase.snap_to_anchor = false;
}

/// How far above `feet` the posed skeleton reference `slot` of an entity's render actor sits; None while
/// the entity has no posed render actor or its skeleton has no such reference.
fn point_rise(
    children: Option<&Children>,
    render_q: &Query<(&FfxiRenderActor, &GlobalTransform)>,
    slot: usize,
    feet: Vec3,
) -> Option<f32> {
    children?
        .iter()
        .find_map(|child| render_q.get(child).ok())
        .and_then(|(actor, root)| {
            actor
                .standard_point(slot)
                .map(|point| root.transform_point(point).y - feet.y)
        })
        .filter(|rise| rise.is_finite())
}

/// `point` turned about `center` by `d` radians of chase yaw (bevy xz, the
/// boom direction is `(sin yaw, cos yaw)`, so a point on the boom at yaw `y`
/// lands on the boom at `y + d`).
pub fn rotate_about(point: Vec2, center: Vec2, d: f32) -> Vec2 {
    let v = point - center;
    let (s, c) = d.sin_cos();
    center + Vec2::new(v.x * c + v.y * s, v.y * c - v.x * s)
}

fn clamped_camera_distance(hit_t: f32, wanted: f32) -> f32 {
    (hit_t - WALL_PAD).min(wanted).max(CAMERA_MIN_DISTANCE)
}

pub fn draw_camera_collision_debug(
    draw: Res<DrawDistance>,
    mode: Res<CameraMode>,
    chase: Res<ChaseCamera>,
    self_q: Query<(&Transform, Option<&BakedActor>), (With<IsSelf>, Without<OperatorCamera>)>,
    cam_q: Query<&Transform, (With<OperatorCamera>, Without<IsSelf>)>,
    bvh_q: Query<&CollisionBvh>,
    zone_bvh: Res<ZoneCollisionBvh>,
    mut gizmos: Gizmos,
) {
    if draw.zone_geom_mode != ZoneGeomMode::Camera {
        return;
    }

    let source = draw.camera_collision_source;

    let mut draw_aabb = |mn: Vec3, mx: Vec3, color: Color| {
        gizmos.primitive_3d(
            &Cuboid::from_size(mx - mn),
            Isometry3d::from_translation((mn + mx) * 0.5),
            color,
        );
    };

    if source.uses_mzb() {
        if let Some((mn, mx)) = zone_bvh.0.as_ref().and_then(|b| b.root_aabb()) {
            draw_aabb(mn, mx, Color::srgba(0.20, 0.80, 1.0, 0.55));
        }
    }

    if source.uses_mmb() {
        for bvh in bvh_q.iter() {
            if let Some((mn, mx)) = bvh.root_aabb() {
                draw_aabb(mn, mx, Color::srgba(1.0, 0.55, 0.10, 0.55));
            }
        }
    }

    let Ok((self_t, baked)) = self_q.single() else {
        return;
    };
    let anchor = self_t.translation + Vec3::Y * third_person_anchor_y(baked);

    let cross = 0.3;
    let cross_color = Color::srgba(1.0, 1.0, 1.0, 0.90);
    gizmos.line(
        anchor - Vec3::X * cross,
        anchor + Vec3::X * cross,
        cross_color,
    );
    gizmos.line(
        anchor - Vec3::Y * cross,
        anchor + Vec3::Y * cross,
        cross_color,
    );
    gizmos.line(
        anchor - Vec3::Z * cross,
        anchor + Vec3::Z * cross,
        cross_color,
    );

    if !matches!(*mode, CameraMode::Chase) {
        return;
    }

    let cos_p = chase.pitch.cos();
    let sin_p = chase.pitch.sin();
    let dir = Vec3::new(chase.yaw.sin() * cos_p, sin_p, chase.yaw.cos() * cos_p);
    let wanted_end = anchor + dir * chase.orbit_radius();

    let effective_end = cam_q.single().map(|t| t.translation).unwrap_or(wanted_end);

    gizmos.line(anchor, effective_end, Color::srgba(1.0, 0.85, 0.15, 0.85));

    let clip_amount = (wanted_end - effective_end).length();
    if clip_amount > 0.05 {
        gizmos.line(
            effective_end,
            wanted_end,
            Color::srgba(1.0, 0.25, 0.55, 0.85),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_distance_never_collapses_into_the_anchor() {
        // XIM PolarCamera.kt getAdjustedRadiusFromCollision collisionDistance: (distance - 0.25).coerceAtLeast(0.5) — a wall
        // right at the anchor (tiny Mog House rooms) must not pull the camera
        // inside the character model.
        assert_eq!(clamped_camera_distance(0.0, 6.0), CAMERA_MIN_DISTANCE);
        assert_eq!(clamped_camera_distance(0.3, 6.0), CAMERA_MIN_DISTANCE);
    }

    #[test]
    fn collision_sweeps_the_distance_the_camera_actually_travels() {
        let chase = ChaseCamera {
            distance: ChaseCamera::DIST_MIN,
            pitch: ChaseCamera::PITCH_MAX,
            ..Default::default()
        };
        assert!(
            chase.orbit_radius() > chase.distance + 0.5,
            "a pitched close camera swings out well past chase.distance \
             ({} vs {}) — sweeping chase.distance here would leave the far end \
             of the eye's travel unswept, which is how it left the building",
            chase.orbit_radius(),
            chase.distance
        );
    }

    #[test]
    fn camera_distance_pads_off_walls_and_caps_at_wanted() {
        assert_eq!(clamped_camera_distance(3.0, 6.0), 3.0 - WALL_PAD);
        assert_eq!(clamped_camera_distance(100.0, 6.0), 6.0);
    }

    #[test]
    fn mog_house_camera_always_collides_with_mmb_furniture() {
        // The MH exit door is a zone MMB static placement, not MZB wall geometry;
        // with the default Mzb source the camera would slip through the doorway
        // gap. Inside a Mog House, MMB collision must be on regardless of source.
        assert!(camera_collides_with_mmb(CameraCollisionSource::Mzb, true));
        assert!(!camera_collides_with_mmb(CameraCollisionSource::Mzb, false));
        assert!(camera_collides_with_mmb(CameraCollisionSource::Mmb, false));
        assert!(camera_collides_with_mmb(CameraCollisionSource::Both, false));
    }

    /// Zone-in snap must land on frame one — no lerp from wherever the last
    /// zone left the eye. The empty zone BVH is resolve_camera's hard
    /// requirement: it raycasts the boom against zone MZB.
    #[test]
    fn snap_to_anchor_places_eye_behind_player_without_smoothing() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(CameraMode::Chase)
            .insert_resource(kuluu_render::GraphicsSettings {
                camera_spring: false,
                ..Default::default()
            })
            .insert_resource(SceneState::default())
            .init_resource::<ZoneCollisionBvh>()
            .insert_resource(kuluu_render::camera::CameraStepSmoothing::default())
            .init_resource::<kuluu_render::cutscene::CutsceneMode>()
            .init_resource::<kuluu_render::lock_on::LockOn>()
            .init_resource::<kuluu_render::ViewFov>()
            .insert_resource(ChaseCamera {
                snap_to_anchor: true,
                ..Default::default()
            })
            .add_systems(Update, resolve_camera);

        let player_pos = Vec3::new(10.0, 1.0, -4.0);
        app.world_mut()
            .spawn((IsSelf, Transform::from_translation(player_pos)));
        let cam = app
            .world_mut()
            .spawn((OperatorCamera, Transform::from_xyz(999.0, 500.0, -999.0)))
            .id();

        app.update();

        let chase = app.world().resource::<ChaseCamera>();
        assert!(!chase.snap_to_anchor, "snap flag consumed by the update");
        let expected_yaw = yaw_for_heading(
            app.world()
                .resource::<SceneState>()
                .snapshot
                .self_pos
                .heading,
        );
        assert_eq!(
            chase.yaw, expected_yaw,
            "zone-in yaw follows player heading"
        );

        let anchor = player_pos + Vec3::Y * third_person_anchor_y(None);
        let expected_dist = clamped_camera_distance(chase.orbit_radius(), chase.orbit_radius());
        let cos_p = chase.pitch.cos();
        let sin_p = chase.pitch.sin();
        let dir = Vec3::new(
            expected_yaw.sin() * cos_p,
            sin_p,
            expected_yaw.cos() * cos_p,
        );
        let expected_eye = anchor + dir * expected_dist;
        let cam_t = *app.world().get::<Transform>(cam).unwrap();
        assert!(
            (cam_t.translation - expected_eye).length() < 1e-4,
            "eye {:?} snapped to {expected_eye:?} behind the player, no lerp from the old zone",
            cam_t.translation
        );
        let look = *cam_t.forward();
        let want = (anchor - expected_eye).normalize();
        assert!(
            (look - want).length() < 1e-4,
            "camera faces along the player's heading: {look:?} != {want:?}"
        );
    }

    /// At full down-tilt the eye never drops a full EYE_FLOOR_BELOW_PIVOT
    /// under the pivot: a forced-down camera views the floor from above it,
    /// not from below it.
    #[test]
    fn full_down_tilt_keeps_the_eye_above_the_floor() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(CameraMode::Chase)
            .insert_resource(kuluu_render::GraphicsSettings {
                camera_spring: false,
                ..Default::default()
            })
            .insert_resource(SceneState::default())
            .init_resource::<ZoneCollisionBvh>()
            .insert_resource(kuluu_render::camera::CameraStepSmoothing::default())
            .init_resource::<kuluu_render::cutscene::CutsceneMode>()
            .init_resource::<kuluu_render::lock_on::LockOn>()
            .init_resource::<kuluu_render::ViewFov>()
            .insert_resource(ChaseCamera {
                pitch: ChaseCamera::PITCH_MIN,
                ..Default::default()
            })
            .add_systems(Update, resolve_camera);

        let player_pos = Vec3::new(0.0, 1.0, 0.0);
        app.world_mut()
            .spawn((IsSelf, Transform::from_translation(player_pos)));
        let cam = app
            .world_mut()
            .spawn((OperatorCamera, Transform::from_xyz(0.0, 0.0, 0.0)))
            .id();

        app.update();

        let cam_t = *app.world().get::<Transform>(cam).unwrap();
        let pivot_y = player_pos.y + third_person_anchor_y(None);
        let below = pivot_y - cam_t.translation.y;
        assert!(
            below <= EYE_FLOOR_BELOW_PIVOT + 1e-5,
            "eye {below} yalms under the pivot at full down-tilt"
        );
        assert!(below > 0.0, "a full down-tilt must still look down");
    }

    /// While a running event holds the camera, the chase writes nothing: the
    /// scripted view (frozen or routed by the cutscene camera task) is not
    /// pulled back behind the player on every frame.
    #[test]
    fn cutscene_camera_lock_holds_the_chase_off_the_camera() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(CameraMode::Chase)
            .insert_resource(kuluu_render::GraphicsSettings::default())
            .insert_resource(SceneState::default())
            .init_resource::<ZoneCollisionBvh>()
            .insert_resource(kuluu_render::camera::CameraStepSmoothing::default())
            .insert_resource(kuluu_render::cutscene::CutsceneMode::active_locked())
            .init_resource::<kuluu_render::lock_on::LockOn>()
            .init_resource::<kuluu_render::ViewFov>()
            .insert_resource(ChaseCamera::default())
            .add_systems(Update, resolve_camera);

        let player_pos = Vec3::new(10.0, 1.0, -4.0);
        app.world_mut()
            .spawn((IsSelf, Transform::from_translation(player_pos)));
        let cam = app
            .world_mut()
            .spawn((OperatorCamera, Transform::from_xyz(999.0, 500.0, -999.0)))
            .id();

        for _ in 0..5 {
            app.update();
        }
        let cam_t = *app.world().get::<Transform>(cam).unwrap();
        assert_eq!(
            cam_t.translation,
            Vec3::new(999.0, 500.0, -999.0),
            "the event-held camera must not be touched by the chase"
        );
    }

    /// Inside the dead zone the focus does not move: a pivot wobbling 5 cm
    /// every frame never reaches the camera. This is the jitter the old
    /// follow passed straight through.
    #[test]
    fn focus_deadzone_swallows_pivot_wobble() {
        const DEAD_ZONE: f32 = 0.5;
        let mut focus = Vec2::new(10.0, -4.0);
        let start = focus;
        for i in 0..600 {
            let wobble = Vec2::new(
                if i % 2 == 0 { 0.05 } else { -0.05 },
                if i % 3 == 0 { 0.04 } else { -0.03 },
            );
            focus = leash(focus, start + wobble, 0.0, DEAD_ZONE, Vec2::ZERO);
            assert_eq!(focus, start, "frame {i}: the focus moved");
        }
    }

    /// Inside the slack band the eye does not move, and so the yaw does not
    /// either; outside it the eye is dragged to the band's edge in one step.
    #[test]
    fn eye_holds_inside_the_band_and_clamps_outside_it() {
        let focus = Vec2::ZERO;
        let fb = Vec2::Y;
        let e = Vec2::new(0.0, 4.5);
        assert_eq!(leash(e, focus, 3.0, 6.0, fb), e);
        let far = leash(Vec2::new(0.0, 9.0), focus, 3.0, 6.0, fb);
        assert!((far.length() - 6.0).abs() < 1e-5);
        let near = leash(Vec2::new(0.0, 1.0), focus, 3.0, 6.0, fb);
        assert!((near.length() - 3.0).abs() < 1e-5);
    }

    /// Held D against this camera: the player runs camera-right every step.
    /// The yaw turns one way only, by at most the step over the band's inner
    /// edge per step. It never spins, and it never reverses.
    #[test]
    fn held_d_turns_the_leash_one_way_without_spinning() {
        const MAX: f32 = 6.0;
        const MIN: f32 = 3.0;
        const STEP: f32 = 0.1;
        const DEAD_ZONE: f32 = 0.5;
        let fb = Vec2::Y;
        let mut yaw = 0.0_f32;
        let mut focus = Vec2::ZERO;
        let mut eye = focus + Vec2::new(yaw.sin(), yaw.cos()) * MAX;
        let mut pivot = focus;
        let mut first_sign = 0.0_f32;
        for i in 0..600 {
            // Camera forward is -(sin yaw, cos yaw) in bevy xz; right of a
            // forward (fx, fz) with Y up is (-fz, fx), here (cos yaw, -sin yaw).
            pivot += Vec2::new(yaw.cos(), -yaw.sin()) * STEP;
            focus = leash(focus, pivot, 0.0, DEAD_ZONE, Vec2::ZERO);
            eye = leash(eye, focus, MIN, MAX, fb);
            let v = eye - focus;
            let next = continuous_yaw(yaw, v.x.atan2(v.y));
            let d = next - yaw;
            assert!(
                d.abs() <= (STEP / MIN).atan() * 1.05,
                "step {i}: yaw jumped {d}"
            );
            if d != 0.0 {
                if first_sign == 0.0 {
                    first_sign = d.signum();
                } else {
                    assert_eq!(d.signum(), first_sign, "step {i}: the turn reversed");
                }
            }
            yaw = next;
        }
        assert!(first_sign != 0.0, "a held D must turn the camera");
    }

    /// Running straight away drags the eye straight behind: no turn.
    #[test]
    fn leash_run_away_keeps_the_yaw() {
        const DEAD_ZONE: f32 = 0.5;
        let yaw = 0.7_f32;
        let away = -Vec2::new(yaw.sin(), yaw.cos());
        let fb = Vec2::new(yaw.sin(), yaw.cos());
        let mut focus = Vec2::ZERO;
        let mut eye = focus + fb * 6.0;
        let mut pivot = focus;
        for _ in 0..300 {
            pivot += away * 0.1;
            focus = leash(focus, pivot, 0.0, DEAD_ZONE, Vec2::ZERO);
            eye = leash(eye, focus, 3.0, 6.0, fb);
        }
        let v = eye - focus;
        assert!((v.x.atan2(v.y) - yaw).abs() < 1e-4);
    }

    /// Same-zone reentry: a session that ended with a short eased boom (spring pulled in, or a
    /// lock-release catch still running) must not hand that history to the next session's fresh camera.
    #[test]
    fn same_zone_camera_respawn_discards_old_resolver_state() {
        use crate::view_native::{input, AppPhase};
        use bevy::state::app::StatesPlugin;
        use bevy::time::TimeUpdateStrategy;
        use kuluu_render::camera::{reset_camera_follow, spawn_camera, AnchorFollow};
        use kuluu_render::components::{InGameEntity, WorldEntity};
        const ZONE: u16 = 100;
        const TARGET_ID: u32 = 77;
        const TICK_HZ: f64 = 60.0;
        const EPSILON: f32 = 0.0001;
        const MANUAL_STEP: f32 = 0.04;
        const SHORT_HOLD: usize = 90;
        const LEASH_YALMS: f32 = 1.0;
        const LAUNCHER_HOLD: usize = 10;
        const SETTLE_FRAMES: usize = 60;
        const TURN_FRAMES: usize = 30;
        for spring in [false, true] {
            for lock_history in [false, true] {
                let mut app = App::new();
                app.add_plugins((MinimalPlugins, StatesPlugin))
                    .insert_resource(TimeUpdateStrategy::ManualDuration(
                        std::time::Duration::from_secs_f64(1.0 / TICK_HZ),
                    ))
                    .init_state::<AppPhase>()
                    .insert_resource(kuluu_render::GraphicsSettings {
                        camera_spring: spring,
                        camera_leash_yalms: LEASH_YALMS,
                        volumetric_fog: false,
                        ..default()
                    })
                    .insert_resource(CameraMode::Chase)
                    .init_resource::<SceneState>()
                    .init_resource::<ZoneCollisionBvh>()
                    .init_resource::<AnchorFollow>()
                    .init_resource::<kuluu_render::camera::CameraStepSmoothing>()
                    .init_resource::<kuluu_render::cutscene::CutsceneMode>()
                    .init_resource::<kuluu_render::lock_on::LockOn>()
                    .init_resource::<kuluu_render::Target>()
                    .init_resource::<kuluu_render::ViewFov>()
                    .init_resource::<kuluu_render::combat_stance::RestStance>()
                    .init_resource::<input::AutoRun>()
                    .add_systems(OnEnter(AppPhase::InGame), spawn_camera)
                    .add_systems(
                        OnExit(AppPhase::InGame),
                        (
                            |mut commands: Commands,
                             entities: Query<Entity, With<InGameEntity>>,
                             mut scene: ResMut<SceneState>,
                             mut bvh: ResMut<ZoneCollisionBvh>| {
                                for entity in &entities {
                                    commands.entity(entity).try_despawn();
                                }
                                *scene = SceneState::default();
                                *bvh = ZoneCollisionBvh::default();
                            },
                            reset_camera_follow,
                        )
                            .chain(),
                    )
                    .add_systems(
                        Update,
                        (
                            input::reset_interaction_flags_on_zone_change,
                            resolve_camera,
                        )
                            .chain()
                            .run_if(in_state(AppPhase::InGame)),
                    );
                app.world_mut()
                    .resource_mut::<SceneState>()
                    .snapshot
                    .zone_id = Some(ZONE);
                app.world_mut()
                    .spawn((InGameEntity, IsSelf, Transform::default()));
                app.world_mut().spawn((
                    InGameEntity,
                    WorldEntity {
                        id: TARGET_ID,
                        act_index: 0,
                        kind: kuluu_snapshot::EntityKind::Mob,
                    },
                    Transform::from_xyz(3.0, 0.0, 4.0),
                ));
                app.world_mut()
                    .resource_mut::<NextState<AppPhase>>()
                    .set(AppPhase::InGame);
                app.update();
                let mut camera_query = app
                    .world_mut()
                    .query_filtered::<(Entity, &Transform), With<OperatorCamera>>();
                let (old_camera, transform) = camera_query.single(app.world()).unwrap();
                let initial_eye = transform.translation;
                app.world_mut().resource_mut::<ChaseCamera>().distance = ChaseCamera::DIST_MIN;
                for _ in 0..SHORT_HOLD {
                    app.update();
                }
                for _ in 0..TURN_FRAMES {
                    app.world_mut().resource_mut::<ChaseCamera>().yaw += MANUAL_STEP;
                    app.update();
                }
                if lock_history {
                    app.world_mut()
                        .resource_mut::<kuluu_render::lock_on::LockOn>()
                        .target_id = Some(TARGET_ID);
                    for _ in 0..TURN_FRAMES {
                        app.update();
                    }
                    app.world_mut()
                        .resource_mut::<kuluu_render::lock_on::LockOn>()
                        .target_id = None;
                    for _ in 0..LAUNCHER_HOLD {
                        app.update();
                    }
                }
                app.world_mut()
                    .resource_mut::<NextState<AppPhase>>()
                    .set(AppPhase::Launcher);
                app.update();
                assert!(app.world().get_entity(old_camera).is_err());
                for _ in 0..LAUNCHER_HOLD {
                    app.update();
                }
                app.world_mut()
                    .resource_mut::<SceneState>()
                    .snapshot
                    .zone_id = Some(ZONE);
                app.world_mut()
                    .spawn((InGameEntity, IsSelf, Transform::default()));
                app.world_mut()
                    .resource_mut::<NextState<AppPhase>>()
                    .set(AppPhase::InGame);
                app.update();
                let (new_camera, transform) = camera_query.single(app.world()).unwrap();
                assert_ne!(new_camera, old_camera);
                assert!((transform.translation - initial_eye).length() < EPSILON,
                    "spring={spring}, lock_history={lock_history}, initial={initial_eye:?}, reentry={:?}", transform.translation);
                for _ in 0..SETTLE_FRAMES {
                    app.update();
                }
                let (_, transform) = camera_query.single(app.world()).unwrap();
                assert!((transform.translation - initial_eye).length() < EPSILON);
            }
        }
    }

    /// A manual yaw request must move the camera by the requested amount under every
    /// `camera_spring` setting and then hold: a spring setting gates boom-distance easing,
    /// not the bearing (`.agents/skills/retail-observe/references/2026-10-07-camera-orbit-channels.md`).
    #[test]
    fn manual_orbit_preserves_direction_with_each_spring_setting() {
        const YAW_STEP: f32 = 0.1;
        const EPSILON: f32 = 0.0001;
        const TICK_HZ: f32 = 60.0;
        for spring in [false, true] {
            for step in [-YAW_STEP, YAW_STEP] {
                let mut app = App::new();
                app.init_resource::<Time>()
                    .insert_resource(CameraMode::Chase)
                    .insert_resource(kuluu_render::GraphicsSettings {
                        camera_spring: spring,
                        ..default()
                    })
                    .init_resource::<SceneState>()
                    .init_resource::<ZoneCollisionBvh>()
                    .init_resource::<kuluu_render::camera::CameraStepSmoothing>()
                    .init_resource::<kuluu_render::cutscene::CutsceneMode>()
                    .init_resource::<kuluu_render::lock_on::LockOn>()
                    .init_resource::<kuluu_render::ViewFov>()
                    .insert_resource(ChaseCamera {
                        yaw: 0.0,
                        synced_initial: true,
                        snap_to_anchor: true,
                        ..default()
                    })
                    .add_systems(Update, resolve_camera);
                app.world_mut().spawn((IsSelf, Transform::default()));
                let camera = app
                    .world_mut()
                    .spawn((OperatorCamera, Transform::default()))
                    .id();
                app.world_mut()
                    .resource_mut::<Time>()
                    .advance_by(std::time::Duration::from_secs_f32(1.0 / TICK_HZ));
                app.world_mut().run_schedule(Update);
                let initial_yaw = app.world().resource::<ChaseCamera>().yaw;
                app.world_mut().resource_mut::<ChaseCamera>().yaw += step;
                app.world_mut().run_schedule(Update);
                let actual = app.world().resource::<ChaseCamera>().yaw - initial_yaw;
                assert!(
                    (actual - step).abs() < EPSILON,
                    "spring={spring}, requested={step}, actual={actual}"
                );
                let eye = app.world().get::<Transform>(camera).unwrap().translation;
                for _ in 0..10 {
                    app.world_mut().run_schedule(Update);
                }
                assert!(
                    (app.world().resource::<ChaseCamera>().yaw - initial_yaw - step).abs()
                        < EPSILON
                );
                assert!(
                    (app.world().get::<Transform>(camera).unwrap().translation - eye).length()
                        < EPSILON
                );
            }
        }
    }

    /// The behind-the-target case: a wide gap turns at the constant max rate,
    /// not the exponential's small first step.
    #[test]
    fn lock_turn_uses_the_max_rate_from_a_wide_gap() {
        let want = 0.4;
        let yaw = want + std::f32::consts::FRAC_PI_2;
        let got = lock_turn(yaw, want, 1.0 / 60.0);
        let expected = want + std::f32::consts::FRAC_PI_2
            - super::super::input::LOCK_CAM_MAX_TURN_RAD_PER_SEC / 60.0;
        assert!(
            (got - expected).abs() < 1e-5,
            "wide gap turns at the max rate, got {got} want {expected}"
        );
    }

    #[test]
    fn lock_turn_lands_exactly_with_a_long_tick() {
        let want = 0.4;
        let got = lock_turn(want + 0.5, want, 1.0);
        assert!(
            (continuous_yaw(want, got) - want).abs() < 1e-5,
            "a tick longer than the gap must land on the bearing, not overshoot"
        );
    }

    #[test]
    fn lock_turn_settles_through_the_exponential_inside_the_arrival_gap() {
        let want = 0.4;
        let gap = super::super::input::LOCK_CAM_ARRIVAL_GAP_RAD * 0.5;
        let got = lock_turn(want + gap, want, 1.0 / 60.0);
        let remaining = gap * (-(LOCK_TURN_RATE / 60.0)).exp();
        assert!(
            (continuous_yaw(want, got) - want - remaining).abs() < 1e-5,
            "inside the arrival gap the exponential settles in"
        );
    }

    /// The rig turns rigidly about the player: the focus keeps its offset
    /// from the player, the eye keeps its distance from the focus, and the
    /// eye-to-focus direction lands on the new yaw.
    #[test]
    fn a_manual_turn_rotates_the_rig_about_the_player() {
        let player = Vec2::new(10.0, -3.0);
        let focus = player + Vec2::new(0.4, 0.1);
        let yaw0 = 0.3_f32;
        let eye = focus + Vec2::new(yaw0.sin(), yaw0.cos()) * 5.0;
        for d in [0.05_f32, 0.5, -1.2, 3.0] {
            let f = rotate_about(focus, player, d);
            let e = rotate_about(eye, player, d);
            assert!(((f - player).length() - (focus - player).length()).abs() < 1e-5);
            assert!(((e - f).length() - 5.0).abs() < 1e-4);
            let v = e - f;
            let got = v.x.atan2(v.y);
            let want = yaw0 + d;
            let diff = (got - want + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            assert!(
                diff.abs() < 1e-4,
                "turn {d}: eye-to-focus yaw {got}, want {want}"
            );
        }
    }

    /// Many small turns add up to one big one: holding an arrow key does not
    /// drift the rig off the player.
    #[test]
    fn held_turn_does_not_drift_off_the_player() {
        let player = Vec2::new(0.0, 0.0);
        let mut focus = player + Vec2::new(0.45, 0.0);
        let r0 = (focus - player).length();
        for _ in 0..720 {
            focus = rotate_about(focus, player, std::f32::consts::TAU / 720.0);
        }
        assert!(((focus - player).length() - r0).abs() < 1e-3);
        assert!((focus - (player + Vec2::new(0.45, 0.0))).length() < 1e-2);
    }

    /// One 60 Hz frame per update, so the lock tests step the same time on any machine.
    const TEST_FRAME: std::time::Duration = std::time::Duration::from_micros(16_667);

    /// A chase app with the camera, the player at `player` and a mob (world id 7) at `target`.
    fn lock_app(camera_spring: bool, player: Vec3, target: Vec3) -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(TEST_FRAME))
            .insert_resource(CameraMode::Chase)
            .insert_resource(kuluu_render::GraphicsSettings {
                camera_spring,
                ..Default::default()
            })
            .insert_resource(SceneState::default())
            .init_resource::<ZoneCollisionBvh>()
            .insert_resource(kuluu_render::camera::CameraStepSmoothing::default())
            .init_resource::<kuluu_render::cutscene::CutsceneMode>()
            .insert_resource(kuluu_render::lock_on::LockOn::default())
            .init_resource::<kuluu_render::ViewFov>()
            .insert_resource(ChaseCamera::default())
            .add_systems(Update, resolve_camera);
        let me = app
            .world_mut()
            .spawn((IsSelf, Transform::from_translation(player)))
            .id();
        let cam = app
            .world_mut()
            .spawn((OperatorCamera, Transform::from_translation(Vec3::ZERO)))
            .id();
        app.world_mut().spawn((
            kuluu_render::components::WorldEntity {
                id: 7,
                act_index: 0,
                kind: kuluu_snapshot::EntityKind::Mob,
            },
            Transform::from_translation(target),
        ));
        (app, me, cam)
    }

    fn lock(app: &mut App, on: bool) {
        app.world_mut()
            .resource_mut::<kuluu_render::lock_on::LockOn>()
            .target_id = on.then_some(7);
    }

    fn camera_of(app: &App, cam: Entity) -> Transform {
        *app.world().entity(cam).get::<Transform>().unwrap()
    }

    /// How far off the line from the target through the player the camera looks back at itself.
    fn angle_off_the_line(cam: &Transform, player: Vec3, target: Vec3) -> f32 {
        (-*cam.forward()).angle_between(player - target)
    }

    /// Frames (at [`TEST_FRAME`]) a lock's opening is given to land the eye on the cone's edge.
    const OPENING_FRAMES: usize = 60;

    /// H with the target off to the side of the free camera: the eye swings in part of the way each
    /// frame and only as far as the lock cone's edge, on the side it was already on, so the view opens
    /// off to that side instead of squarely behind the player, and stays off the line once the look
    /// point has slid to the midpoint. Without a model the camera points fall back to the anchor height
    /// on both actors.
    #[test]
    fn a_lock_opens_off_the_line_on_the_cameras_own_side() {
        let player = Vec3::new(0.0, 1.0, 0.0);
        let (mut app, _me, cam) = lock_app(false, player, Vec3::new(0.0, 1.0, 50.0));
        for _ in 0..10 {
            app.update();
        }
        // Put the target square to the camera's side: the free eye sits a quarter turn off the line.
        let eye = camera_of(&app, cam).translation;
        let back = Vec3::new(eye.x - player.x, 0.0, eye.z - player.z).normalize();
        let target = player + Vec3::new(back.z, 0.0, -back.x) * 5.0;
        let mut q = app
            .world_mut()
            .query_filtered::<&mut Transform, With<kuluu_render::components::WorldEntity>>();
        for mut t in q.iter_mut(app.world_mut()) {
            t.translation = target;
        }
        let half = super::super::locked_camera::lock_frame(
            (player - target).length(),
            kuluu_render::graphics_settings::RETAIL_DEFAULT_FOCAL_LENGTH,
        )
        .half_angle;
        let start = angle_off_the_line(&camera_of(&app, cam), player, target);
        lock(&mut app, true);
        app.update();
        let first = angle_off_the_line(&camera_of(&app, cam), player, target);
        assert!(
            first > half + 0.1 && first < start - 0.1,
            "the first locked frame swings the eye only part of the way in: {} deg from {} deg against {} deg",
            first.to_degrees(),
            start.to_degrees(),
            half.to_degrees()
        );
        for _ in 0..OPENING_FRAMES {
            app.update();
        }
        let opened = angle_off_the_line(&camera_of(&app, cam), player, target);
        assert!(
            (opened - half).abs() < 0.03,
            "the opening lands the eye on the cone edge: {} deg against {} deg",
            opened.to_degrees(),
            half.to_degrees()
        );
        for _ in 0..240 {
            app.update();
        }
        let view = camera_of(&app, cam);
        let off = angle_off_the_line(&view, player, target);
        assert!(
            off > half * 0.5 && off <= half + 1e-3,
            "settled off the line inside the cone: {} deg against {} deg",
            off.to_degrees(),
            half.to_degrees()
        );
        assert!(
            (view.translation - player).dot(back) > 0.0,
            "on the side the camera started on, not swung through the target"
        );
    }

    /// H with the free camera square behind the player and the target straight ahead, a hair to the
    /// camera's left, so the eye sits a hair left of the line through the player: the lock does not
    /// stay behind the player, it swings out part of the way each frame to the cone's edge on that
    /// nearer left side and settles there, well off the line.
    #[test]
    fn a_lock_from_behind_opens_off_to_the_nearer_side() {
        let player = Vec3::new(0.0, 1.0, 0.0);
        let (mut app, _me, cam) = lock_app(false, player, Vec3::new(0.0, 1.0, 50.0));
        for _ in 0..10 {
            app.update();
        }
        let eye = camera_of(&app, cam).translation;
        let back = Vec3::new(eye.x - player.x, 0.0, eye.z - player.z).normalize();
        let left = Vec3::new(-back.z, 0.0, back.x);
        let target = player - back * 5.0 + left * 0.1;
        let mut q = app
            .world_mut()
            .query_filtered::<&mut Transform, With<kuluu_render::components::WorldEntity>>();
        for mut t in q.iter_mut(app.world_mut()) {
            t.translation = target;
        }
        let half = super::super::locked_camera::lock_frame(
            (player - target).length(),
            kuluu_render::graphics_settings::RETAIL_DEFAULT_FOCAL_LENGTH,
        )
        .half_angle;
        lock(&mut app, true);
        app.update();
        let first = camera_of(&app, cam);
        let left_of_player = |at: Vec3| (at - player).dot(left);
        assert!(
            angle_off_the_line(&first, player, target) < half - 0.03
                && left_of_player(first.translation) > 0.0,
            "the first locked frame starts out toward the edge on the left, the side the camera sat \
             nearer: {} deg against {} deg, {:?}",
            angle_off_the_line(&first, player, target).to_degrees(),
            half.to_degrees(),
            first.translation
        );
        for _ in 0..OPENING_FRAMES {
            app.update();
        }
        let opened = camera_of(&app, cam);
        assert!(
            (angle_off_the_line(&opened, player, target) - half).abs() < 0.03
                && left_of_player(opened.translation) > 1.0,
            "the opening lands the eye on the cone edge on the left: {} deg against {} deg, {:?}",
            angle_off_the_line(&opened, player, target).to_degrees(),
            half.to_degrees(),
            opened.translation
        );
        for _ in 0..240 {
            app.update();
        }
        let view = camera_of(&app, cam);
        let off = angle_off_the_line(&view, player, target);
        assert!(
            off > half * 0.5 && left_of_player(view.translation) > 1.0,
            "settled off the line on that side: {} deg against {} deg, {:?}",
            off.to_degrees(),
            half.to_degrees(),
            view.translation
        );
    }

    /// Circling a locked target, every frame keeps the eye inside the cone round the line from the
    /// target through the player.
    #[test]
    fn circling_a_locked_target_holds_the_cone() {
        const RADIUS: f32 = 5.0;
        let target = Vec3::new(0.0, 1.0, 0.0);
        let at = |angle: f32| target + Vec3::new(RADIUS * angle.sin(), 0.0, RADIUS * angle.cos());
        let (mut app, me, cam) = lock_app(true, at(0.0), target);
        for _ in 0..10 {
            app.update();
        }
        lock(&mut app, true);
        for _ in 0..240 {
            app.update();
        }
        let half = super::super::locked_camera::lock_frame(
            RADIUS,
            kuluu_render::graphics_settings::RETAIL_DEFAULT_FOCAL_LENGTH,
        )
        .half_angle;
        // The locked side step: 3.75 y/s round the circle.
        let rate = 3.75 / RADIUS;
        let t0 = app.world().resource::<Time>().elapsed_secs();
        let mut elapsed = 0.0;
        while elapsed < 4.0 {
            app.update();
            elapsed = app.world().resource::<Time>().elapsed_secs() - t0;
            let player = at(rate * elapsed);
            app.world_mut()
                .entity_mut(me)
                .get_mut::<Transform>()
                .unwrap()
                .translation = player;
            let view = camera_of(&app, cam);
            assert!(
                angle_off_the_line(&view, player, target) <= half + 0.05,
                "{elapsed:.2}s: the eye left the cone"
            );
        }
    }

    /// A wider menu FOV widens the locked view instead of pulling the eye in to cancel it: the lock holds
    /// the eye exactly where it holds it at the menu's retail FOV, and the wider projection shows more.
    #[test]
    fn a_wider_menu_fov_widens_the_lock_instead_of_closing_in() {
        const WIDE_MENU_FOV: f32 = 80.0;
        let player = Vec3::new(0.0, 1.0, 0.0);
        let target = Vec3::new(5.0, 1.0, 2.0);
        let settled_eye = |menu_fov: Option<f32>| {
            let (mut app, _me, cam) = lock_app(true, player, target);
            if let Some(fov) = menu_fov {
                app.world_mut()
                    .resource_mut::<kuluu_render::GraphicsSettings>()
                    .fov_deg = fov;
                app.world_mut()
                    .resource_mut::<kuluu_render::ViewFov>()
                    .focal_length = kuluu_render::ViewFov::focal_for_deg(fov);
            }
            for _ in 0..10 {
                app.update();
            }
            lock(&mut app, true);
            for _ in 0..240 {
                app.update();
            }
            camera_of(&app, cam).translation
        };
        let retail = settled_eye(None);
        let wide = settled_eye(Some(WIDE_MENU_FOV));
        assert!(
            (wide - retail).length() < 1e-3,
            "the wide menu FOV moved the locked eye: {retail:?} -> {wide:?}"
        );
    }

    /// A lock is laid over the free camera, never written into it: after locking on a target off to the
    /// side and letting go, the free camera ends exactly where it stood going in, at the same distance
    /// and pitch, looking at the player from behind the body.
    #[test]
    fn a_lock_leaves_the_free_camera_as_it_found_it() {
        let player = Vec3::new(0.0, 1.0, 0.0);
        let (mut app, _me, cam) = lock_app(true, player, Vec3::new(5.0, 1.0, 2.0));
        for _ in 0..10 {
            app.update();
        }
        let before = camera_of(&app, cam);
        let pitch_before = app.world().resource::<ChaseCamera>().pitch;

        lock(&mut app, true);
        for _ in 0..240 {
            app.update();
        }
        let locked = camera_of(&app, cam);
        assert!(
            (locked.translation - before.translation).length() > 1.0,
            "the lock frames the pair from its own eye: {:?} -> {:?}",
            before.translation,
            locked.translation
        );
        assert_eq!(
            app.world().resource::<ChaseCamera>().pitch,
            pitch_before,
            "the lock leaves the chase pitch alone"
        );

        lock(&mut app, false);
        for _ in 0..120 {
            app.update();
        }
        let after = camera_of(&app, cam);
        // The release catch stops inside its own settle gap, a thousandth of a radian.
        assert!(
            (after.translation - before.translation).length() < 1e-2,
            "the free camera is back where it was: {:?} -> {:?}",
            before.translation,
            after.translation
        );
        assert!(
            (after.translation.y - before.translation.y).abs() < 1e-4,
            "at the same height, so the same distance and pitch: {} -> {}",
            before.translation.y,
            after.translation.y
        );
        assert!(
            after.forward().angle_between(*before.forward()) < 2e-3,
            "and looks where it looked"
        );
    }

    /// Lock release: the eye is not teleported - the view eases from the
    /// locked eye back onto the chase rig - and the yaw eases back behind the
    /// body's heading in well under a second instead of parking where the
    /// lock left it.
    #[test]
    fn lock_release_keeps_the_eye_and_eases_the_yaw_home() {
        let player = Vec3::new(0.0, 1.0, 0.0);
        let (mut app, _me, cam) = lock_app(false, player, Vec3::new(5.0, 1.0, 0.0));
        // The free camera places the eye first, as it has before any lock in play.
        for _ in 0..10 {
            app.update();
        }
        lock(&mut app, true);
        for _ in 0..240 {
            app.update();
        }
        let eye_locked = camera_of(&app, cam).translation;

        lock(&mut app, false);
        app.update();
        let eye_released = camera_of(&app, cam).translation;
        assert!(
            (eye_released - eye_locked).length() < 1.0,
            "the release frame must not teleport the eye: {eye_locked:?} -> {eye_released:?}"
        );

        let home = yaw_for_heading(
            app.world()
                .resource::<SceneState>()
                .snapshot
                .self_pos
                .heading,
        );
        let t0 = app.world().resource::<Time>().elapsed_secs();
        for _ in 0..20_000 {
            app.update();
            let chase = app.world().resource::<ChaseCamera>();
            if (continuous_yaw(home, chase.yaw) - home).abs() < 0.05 {
                break;
            }
        }
        let elapsed = app.world().resource::<Time>().elapsed_secs() - t0;
        let chase = app.world().resource::<ChaseCamera>();
        let gap = (continuous_yaw(home, chase.yaw) - home).abs();
        assert!(gap < 0.05, "the released camera must ease home, gap {gap}");
        assert!(
            elapsed < 1.0,
            "the ease home must close in well under a second, took {elapsed}s"
        );
    }

    /// The input system's Q/E turn: yaw the camera now owes.
    fn owe(app: &mut App, turn: f32) {
        app.world_mut().resource_mut::<ChaseCamera>().turn_owed += turn;
    }

    /// A Q/E turn the camera owes swings the whole rig round the player, with the spring on or off: the
    /// first frame pays only part of it, so the camera trails the body, and then it lands exactly on the
    /// turn with the eye at its own distance and height.
    #[test]
    fn an_owed_rotate_turn_swings_the_rig_round_the_player_and_settles() {
        const TURN: f32 = 0.3;
        const SETTLE_FRAMES: usize = 60;
        let player = Vec3::new(0.0, 1.0, 0.0);
        let bearing =
            |t: &Transform| (t.translation.x - player.x).atan2(t.translation.z - player.z);
        let reach = |t: &Transform| {
            Vec2::new(t.translation.x - player.x, t.translation.z - player.z).length()
        };
        for spring in [true, false] {
            let (mut app, _me, cam) = lock_app(spring, player, Vec3::new(0.0, 1.0, 50.0));
            for _ in 0..10 {
                app.update();
            }
            let before = camera_of(&app, cam);
            let yaw_before = app.world().resource::<ChaseCamera>().yaw;
            owe(&mut app, TURN);
            app.update();
            let first = app.world().resource::<ChaseCamera>().yaw - yaw_before;
            assert!(
                first > 0.0 && first < TURN * 0.5,
                "spring {spring}: the first frame pays {first} rad of {TURN}"
            );
            for _ in 0..SETTLE_FRAMES {
                app.update();
            }
            let chase = app.world().resource::<ChaseCamera>();
            assert_eq!(chase.turn_owed, 0.0, "spring {spring}: the turn is paid");
            assert!(
                (chase.yaw - yaw_before - TURN).abs() < 1e-4,
                "spring {spring}: the camera turned {} rad, want {TURN}",
                chase.yaw - yaw_before
            );
            let after = camera_of(&app, cam);
            let swung = continuous_yaw(0.0, bearing(&after) - bearing(&before));
            assert!(
                (swung - TURN).abs() < 1e-4,
                "spring {spring}: the eye swung {swung} rad round the player, want {TURN}"
            );
            assert!(
                (reach(&after) - reach(&before)).abs() < 1e-4
                    && (after.translation.y - before.translation.y).abs() < 1e-4,
                "spring {spring}: the swing kept the eye's distance and height: {:?} -> {:?}",
                before.translation,
                after.translation
            );
        }
    }

    /// A lock, or the player turning the camera, takes it back from a Q/E turn it still owes.
    #[test]
    fn a_lock_or_a_manual_turn_drops_an_owed_rotate_turn() {
        const TURN: f32 = 1.0;
        const MANUAL_TURN: f32 = 0.1;
        let player = Vec3::new(0.0, 1.0, 0.0);
        let (mut app, _me, _cam) = lock_app(true, player, Vec3::new(5.0, 1.0, 2.0));
        for _ in 0..10 {
            app.update();
        }
        owe(&mut app, TURN);
        lock(&mut app, true);
        app.update();
        assert_eq!(
            app.world().resource::<ChaseCamera>().turn_owed,
            0.0,
            "a lock takes the camera"
        );
        lock(&mut app, false);
        for _ in 0..120 {
            app.update();
        }
        owe(&mut app, TURN);
        app.world_mut().resource_mut::<ChaseCamera>().yaw += MANUAL_TURN;
        app.update();
        assert_eq!(
            app.world().resource::<ChaseCamera>().turn_owed,
            0.0,
            "a manual turn takes the camera"
        );
    }

    /// A Q/E turn while a released camera eases home turns its home with it, so the camera still comes
    /// to rest behind the turned body.
    #[test]
    fn a_rotate_turn_during_the_release_ease_carries_its_home_round() {
        const TURN: f32 = 0.3;
        let player = Vec3::new(0.0, 1.0, 0.0);
        let (mut app, _me, _cam) = lock_app(false, player, Vec3::new(5.0, 1.0, 0.0));
        for _ in 0..10 {
            app.update();
        }
        lock(&mut app, true);
        for _ in 0..240 {
            app.update();
        }
        lock(&mut app, false);
        app.update();
        owe(&mut app, TURN);
        for _ in 0..120 {
            app.update();
        }
        let home = yaw_for_heading(
            app.world()
                .resource::<SceneState>()
                .snapshot
                .self_pos
                .heading,
        ) + TURN;
        let yaw = app.world().resource::<ChaseCamera>().yaw;
        assert!(
            (continuous_yaw(home, yaw) - home).abs() < 2e-3,
            "the camera came to rest at {yaw} rad, want {home} rad"
        );
    }
}
