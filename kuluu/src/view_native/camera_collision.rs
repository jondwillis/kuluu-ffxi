use bevy::prelude::*;

use kuluu_render::components::IsSelf;
use kuluu_render::dat_mzb::{CameraCollisionSource, DrawDistance, ZoneGeomMode};
use kuluu_render::scene::BakedActor;
use kuluu_render::snapshot::SceneState;
use kuluu_render::{
    third_person_anchor_y, yaw_for_heading, CameraMode, ChaseCamera, OperatorCamera,
};

use super::collision_bvh::{CollisionBvh, ZoneCollisionBvh};

/// The focus dead zone, yalms: the camera's focus holds still while the pivot
/// moves inside it, and is dragged to exactly this distance once the pivot
/// leaves it. Longer than an average step so a single step never moves the
/// camera (0.25 was not), short enough that the player never reads
/// off-centre. The idea is the orbit camera's focus radius
/// (catlikecoding.com, Orbit Camera, "focus radius").
const FOCUS_DEADZONE: f32 = 0.5;

/// The camera spring: once the leash says the eye must move, it closes the
/// gap at this fraction of the gap per second, capped at CAM_MAX_SPEED, and
/// never overshoots. The Camera Spring setting on uses it; off, the eye jumps
/// straight to where the leash puts it. At run speed the eye settles about
/// speed / rate behind the band's edge, the trailing feel of the chase camera.
const CAM_PULL_RATE: f32 = 2.0;

/// Cap on the spring's travel per second. Must exceed sprint speed or the gap
/// grows without bound; warps use snap_to_anchor.
const CAM_MAX_SPEED: f32 = 12.0;

/// Locked on, the camera turns onto the target bearing at this rate
/// (framerate-independent exponential), the lock look-at from before the
/// leash rework.
const LOCK_TURN_RATE: f32 = 12.0;

/// Move `from` toward `to` by the spring: a gap-proportional step capped at
/// CAM_MAX_SPEED, never past `to`.
pub fn spring_toward(from: Vec2, to: Vec2, dt: f32) -> Vec2 {
    let gap = to - from;
    let d = gap.length();
    if d < 1e-5 {
        return to;
    }
    let speed = (d * CAM_PULL_RATE).min(CAM_MAX_SPEED);
    let travel = (speed * dt).min(d);
    from + gap / d * travel
}

/// The chase yaw that puts `target` straight ahead of the camera from behind
/// `player` (bevy xz; the boom direction is `(sin yaw, cos yaw)`, so the
/// camera looks along `-(sin yaw, cos yaw)`). `None` when they coincide.
pub fn lock_yaw(player: Vec2, target: Vec2) -> Option<f32> {
    let away = player - target;
    (away.length_squared() > 1e-6).then(|| away.x.atan2(away.y))
}

/// The eye's slack band: it holds still while its horizontal distance from the
/// focus is between `max * LEASH_SLACK_MIN_RATIO` and `max` (the zoom), and is
/// dragged or pushed to the band's edge outside it. The reference client pulls
/// its eye in past 6 and pushes it out under 3
/// (research/XIClient/src/XIClient/source/World/Camera/CameraManager.cpp
/// CameraManager::UpdatePlayerFollowingCamera), hence one half.
const LEASH_SLACK_MIN_RATIO: f32 = 0.5;

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

/// Where the camera's focus and eye sit in the world, bevy xz, carried frame
/// to frame; `yaw` is the chase yaw this system last wrote, so a different
/// value next frame means the player turned the camera.
#[derive(Default)]
pub struct LeashState {
    focus: Option<Vec2>,
    eye: Option<Vec2>,
    yaw: Option<f32>,
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

const OUTWARD_LERP: f32 = 0.18;

const INWARD_LERP: f32 = 0.45;

/// The single chase-camera authority: a leash with slack at both ends, the
/// camera spring that decides how fast the eye reaches the leash's goal, the
/// lock look-at behind the player, and the wall pull-in against the zone MZB
/// BVH, with one transform write at the end.
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
/// while the pivot moves inside FOCUS_DEADZONE and is dragged to exactly
/// that distance once the pivot leaves it, so the per-frame wobble of the
/// player transform (a server correction, an interpolation seam, a stair
/// step) never reaches the camera. The eye holds still while its horizontal
/// distance from the focus is between half and all of the zoom
/// (LEASH_SLACK_MIN_RATIO); outside the band the leash says where it must be
/// and the camera spring (CAM_PULL_RATE, capped at CAM_MAX_SPEED) says how
/// fast it gets there — with the spring off the eye snaps to the goal. The
/// yaw is the direction from the focus to the eye; the one exception is a
/// frame where something else wrote chase.yaw since last frame (the mouse,
/// the yaw keys, Q/E, a stair warp) — then the eye swings around the focus
/// to that yaw at once, no spring. Locked on, the camera instead sits behind
/// the player aimed at the target and turns onto that bearing at
/// LOCK_TURN_RATE, and the yaw does not come back from the eye.
///
/// Init sync aligns yaw behind the player on the first frame.
/// snap_to_anchor (zone/warp) resets the leash to the exact position.
///
/// Pass 3 is the collision pull-in: a ray from the pivot along the boom,
/// where walls block and mobs do not — the same solid world the walker
/// sweeps (the door triangles join this ray when the obstacle set lands).
/// The nearest hit shortens the boom so the camera does not clip through
/// geometry; cast from the pivot (the player), not the glide origin, so wall
/// pull-in is measured from where the camera is actually looking. The BVH
/// rebuilds ~1 s after zone geometry goes quiet; until then there is no ray
/// and the boom runs unclipped.
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
    self_q: Query<(&Transform, Option<&BakedActor>), (With<IsSelf>, Without<OperatorCamera>)>,
    mut cam_q: Query<&mut Transform, (With<OperatorCamera>, Without<IsSelf>)>,
    lock_on: Res<kuluu_render::lock_on::LockOn>,
    target_q: Query<
        (&kuluu_render::components::WorldEntity, &Transform),
        (Without<IsSelf>, Without<OperatorCamera>),
    >,
    mut smoothed_effective: Local<Option<f32>>,
    mut leash_state: Local<LeashState>,
) {
    if !matches!(*mode, CameraMode::Chase) {
        *smoothed_effective = None;
        *leash_state = LeashState::default();
        return;
    }
    // A running event holding the camera owns the operator camera (the
    // cutscene camera route advances or freezes it after this system): the
    // chase must not attach to the player while the script owns the view.
    if cutscene.camera_locked {
        *smoothed_effective = None;
        *leash_state = LeashState::default();
        return;
    }
    let Ok((self_t, baked)) = self_q.single() else {
        *smoothed_effective = None;
        *leash_state = LeashState::default();
        return;
    };
    let Ok(mut cam_t) = cam_q.single_mut() else {
        return;
    };

    if !chase.synced_initial {
        chase.yaw = yaw_for_heading(scene_state.snapshot.self_pos.heading);
        chase.synced_initial = true;
    }

    let player_pos = self_t.translation;
    let anchor_y = Vec3::Y * (third_person_anchor_y(baked) - step.offset);
    let pivot_y = player_pos.y + anchor_y.y;
    let pivot_xz = Vec2::new(player_pos.x, player_pos.z);
    let dt = time.delta_secs().max(1e-4);

    let cos_p = chase.pitch.cos().max(1e-3);
    let sin_p = chase.pitch.sin();
    let max_h = chase.orbit_radius() * cos_p;
    let min_h = max_h * LEASH_SLACK_MIN_RATIO;
    let yaw_dir = |yaw: f32| Vec2::new(yaw.sin(), yaw.cos());

    // Focus: held inside the dead zone, dragged to its edge outside it. The
    // dead zone is always on; it decides when the camera moves at all.
    let focus = match leash_state.focus {
        Some(f) if !chase.snap_to_anchor => leash(f, pivot_xz, 0.0, FOCUS_DEADZONE, Vec2::ZERO),
        _ => pivot_xz,
    };

    // Locked on: the camera sits behind the player aimed at the target and
    // turns onto that bearing at LOCK_TURN_RATE, as before the leash rework.
    let locked_yaw = lock_on
        .target_id
        .and_then(|id| target_q.iter().find(|(we, _)| we.id == id))
        .and_then(|(_, t)| lock_yaw(pivot_xz, Vec2::new(t.translation.x, t.translation.z)));
    if let Some(want) = locked_yaw {
        let alpha = 1.0 - (-LOCK_TURN_RATE * dt).exp();
        let gap = continuous_yaw(chase.yaw, want) - chase.yaw;
        chase.yaw += gap * alpha;
    }

    // Eye: where it was; swung straight to the yaw if the player (or the
    // lock) turned the camera since last frame, no spring; then the leash
    // says where it must be and the spring says how fast it gets there.
    // A yaw this system did not write (arrows, mouse drag, Q/E, the lock
    // turn) is a manual turn: the whole rig, eye and focus together, turns
    // about the player by the yaw's change. Nothing jumps and the player
    // keeps its spot on screen; the focus's dead-zone offset turns with the
    // rig instead of making the camera orbit a point beside the player.
    let (focus, eye_prev) = match (leash_state.eye, leash_state.yaw) {
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
    let eye_goal = leash(eye_prev, focus, min_h, max_h, yaw_dir(chase.yaw));
    let eye = if settings.camera_spring && !chase.snap_to_anchor {
        spring_toward(eye_prev, eye_goal, dt)
    } else {
        eye_goal
    };
    let to_eye = eye - focus;
    if locked_yaw.is_none() {
        chase.yaw = continuous_yaw(chase.yaw, to_eye.x.atan2(to_eye.y));
    }
    *leash_state = LeashState {
        focus: Some(focus),
        eye: Some(eye),
        yaw: Some(chase.yaw),
    };

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

    let effective = if !settings.camera_spring || chase.snap_to_anchor {
        target
    } else {
        match *smoothed_effective {
            Some(prev) if target < prev => target * INWARD_LERP + prev * (1.0 - INWARD_LERP),
            Some(prev) => prev + (target - prev) * OUTWARD_LERP,
            None => target,
        }
    };
    *smoothed_effective = Some(effective);

    cam_t.translation = pivot + dir * effective;
    cam_t.look_at(pivot, Vec3::Y);
    chase.snap_to_anchor = false;
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
            .init_resource::<ZoneCollisionBvh>()
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
        let mut focus = Vec2::new(10.0, -4.0);
        let start = focus;
        for i in 0..600 {
            let wobble = Vec2::new(
                if i % 2 == 0 { 0.05 } else { -0.05 },
                if i % 3 == 0 { 0.04 } else { -0.03 },
            );
            focus = leash(focus, start + wobble, 0.0, FOCUS_DEADZONE, Vec2::ZERO);
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
            focus = leash(focus, pivot, 0.0, FOCUS_DEADZONE, Vec2::ZERO);
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
        let yaw = 0.7_f32;
        let away = -Vec2::new(yaw.sin(), yaw.cos());
        let fb = Vec2::new(yaw.sin(), yaw.cos());
        let mut focus = Vec2::ZERO;
        let mut eye = focus + fb * 6.0;
        let mut pivot = focus;
        for _ in 0..300 {
            pivot += away * 0.1;
            focus = leash(focus, pivot, 0.0, FOCUS_DEADZONE, Vec2::ZERO);
            eye = leash(eye, focus, 3.0, 6.0, fb);
        }
        let v = eye - focus;
        assert!((v.x.atan2(v.y) - yaw).abs() < 1e-4);
    }

    /// The spring closes toward the leash's goal without overshooting, faster
    /// on a larger gap, and a zero gap stays put.
    #[test]
    fn spring_closes_the_gap_without_overshoot() {
        let dt = 1.0 / 60.0;
        let to = Vec2::new(0.0, 6.0);
        let near = spring_toward(Vec2::new(0.0, 5.5), to, dt);
        let far = spring_toward(Vec2::new(0.0, 2.0), to, dt);
        assert!(near.y > 5.5 && near.y <= 6.0);
        assert!(
            (far.y - 2.0) > (near.y - 5.5),
            "a larger gap must close faster"
        );
        assert_eq!(spring_toward(to, to, dt), to);
        let mut p = Vec2::ZERO;
        for _ in 0..600 {
            p = spring_toward(p, to, dt);
            assert!(p.y <= 6.0 + 1e-5, "overshot to {}", p.y);
        }
        assert!((p - to).length() < 1e-2);
    }

    /// Locked, the camera's forward points at the target from behind the player.
    #[test]
    fn lock_yaw_puts_the_target_ahead() {
        let player = Vec2::new(1.0, 2.0);
        let target = Vec2::new(4.0, -2.0);
        let yaw = lock_yaw(player, target).expect("distinct points");
        let forward = -Vec2::new(yaw.sin(), yaw.cos());
        let to_target = (target - player).normalize();
        assert!(
            forward.dot(to_target) > 0.9999,
            "forward {forward:?}, target {to_target:?}"
        );
        assert!(lock_yaw(player, player).is_none());
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
}
