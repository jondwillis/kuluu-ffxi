use bevy::prelude::*;

use kuluu_render::components::IsSelf;
use kuluu_render::dat_mzb::{CameraCollisionSource, DrawDistance, ZoneGeomMode};
use kuluu_render::scene::BakedActor;
use kuluu_render::snapshot::SceneState;
use kuluu_render::{
    third_person_anchor_y, yaw_for_heading, CameraMode, ChaseCamera, OperatorCamera,
};

use super::collision_bvh::{CollisionBvh, ZoneCollisionBvh};

// Lock-on and lock-release easing remain playtest tuning.
const LOCK_TURN_RATE: f32 = 12.0;

/// The chase yaw that puts `target` straight ahead of the camera from behind
/// `player` (bevy xz; the boom direction is `(sin yaw, cos yaw)`, so the
/// camera looks along `-(sin yaw, cos yaw)`). `None` when they coincide.
pub fn lock_yaw(player: Vec2, target: Vec2) -> Option<f32> {
    let away = player - target;
    (away.length_squared() > 1e-6).then(|| away.x.atan2(away.y))
}

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

/// One step of the lock-on camera turn: a wide gap turns at the constant max
/// rate (the behind-the-target case), the exponential settles in inside the
/// arrival gap.
pub(crate) fn lock_turn(chase_yaw: f32, want: f32, dt: f32) -> f32 {
    let gap = continuous_yaw(chase_yaw, want) - chase_yaw;
    if gap.abs() > super::input::LOCK_CAM_ARRIVAL_GAP_RAD {
        let step = super::input::LOCK_CAM_MAX_TURN_RAD_PER_SEC * dt;
        chase_yaw + gap.signum() * step.min(gap.abs())
    } else {
        let alpha = 1.0 - (-LOCK_TURN_RATE * dt).exp();
        chase_yaw + gap * alpha
    }
}

/// Where the camera's focus and eye sit in the world, bevy xz, carried frame
/// to frame; `yaw` is the chase yaw this system last wrote, so a different
/// value next frame means the player turned the camera.
#[derive(Default)]
pub struct LeashState {
    focus: Option<Vec2>,
    eye: Option<Vec2>,
    yaw: Option<f32>,
    /// The previous frame was locked on: the release frame re-seeds the free
    /// camera from the live eye (see resolve_camera) and starts the release
    /// ease, so unlocking never teleports the eye.
    was_locked: bool,
    /// Yaw the just-released camera eases back to (behind the body's
    /// heading): the lock bearing leaves a small residual against the body
    /// facing, and the released camera closes it with a fast catch-up turn
    /// instead of parking at the lock bearing or snapping. A manual turn,
    /// a new lock, or a zone snap takes the camera back.
    release_yaw: Option<f32>,
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

    // Locked on: the camera sits behind the player aimed at the target and
    // turns onto that bearing: max rate from a wide gap, LOCK_TURN_RATE to
    // settle in inside the arrival gap.
    let locked_yaw = lock_on
        .target_id
        .and_then(|id| target_q.iter().find(|(we, _)| we.id == id))
        .and_then(|(_, t)| lock_yaw(pivot_xz, Vec2::new(t.translation.x, t.translation.z)));
    // Yaw something else wrote since last frame (Q/E, arrows, mouse): the eye
    // branch below treats it as a manual turn, and it takes the camera back from the release ease. While
    // locked on, this is only ever the lock turn's own correction of chase.yaw, so it is not a steer
    // event there.
    let manual_yaw = leash_state
        .yaw
        .map(|ly| continuous_yaw(ly, chase.yaw) - ly)
        .unwrap_or(0.0);
    if let Some(want) = locked_yaw {
        chase.yaw = lock_turn(chase.yaw, want, dt);
    }
    // Lock release: the free camera takes over where the locked eye rendered
    // (re-seed the leash from the live camera so nothing carried or reset
    // during the lock can move it) and then springs back behind the body's
    // heading - the lock bearing leaves a small residual against the body
    // facing that the released camera closes in well under a second.
    if leash_state.was_locked && locked_yaw.is_none() {
        leash_state.eye = Some(Vec2::new(cam_t.translation.x, cam_t.translation.z));
        leash_state.yaw = Some(chase.yaw);
        if manual_yaw == 0.0 {
            leash_state.release_yaw = Some(yaw_for_heading(scene_state.snapshot.self_pos.heading));
        }
    }
    if let Some(release) = leash_state.release_yaw {
        if manual_yaw != 0.0 || locked_yaw.is_some() || chase.snap_to_anchor {
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

    // .agents/skills/retail-observe/references/2026-10-07-camera-orbit-channels.md Manual orbit input:
    // manual yaw is not a body-facing reference; no grounded body-reference handoff exists here.
    let (focus, eye_prev) = match (leash_state.eye, leash_state.yaw) {
        (Some(e), _) if !chase.snap_to_anchor && locked_yaw.is_some() => {
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
    if locked_yaw.is_none() {
        chase.yaw = continuous_yaw(chase.yaw, to_eye.x.atan2(to_eye.y));
    }
    let was_locked = locked_yaw.is_some();
    let release_yaw = if was_locked {
        None
    } else {
        leash_state.release_yaw
    };
    *leash_state = LeashState {
        focus: Some(focus),
        eye: Some(eye),
        yaw: Some(chase.yaw),
        was_locked,
        release_yaw,
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

    fn heading_byte_for_rad(rad: f32) -> u8 {
        let turns = rad / std::f32::consts::TAU;
        (turns * 256.0).round().rem_euclid(256.0) as u8
    }

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

    /// Lock release: the eye is not teleported - the free camera takes over
    /// at the rendered eye - and the yaw eases back behind the body's heading
    /// (the residual the lock bearing leaves against the body facing) in well
    /// under a second instead of parking at the lock bearing.
    #[test]
    fn lock_release_keeps_the_eye_and_eases_the_yaw_home() {
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
            .insert_resource(kuluu_render::lock_on::LockOn::default())
            .insert_resource(ChaseCamera::default())
            .add_systems(Update, resolve_camera);

        let player = Vec3::new(0.0, 1.0, 0.0);
        app.world_mut()
            .spawn((IsSelf, Transform::from_translation(player)));
        let cam = app
            .world_mut()
            .spawn((OperatorCamera, Transform::from_translation(Vec3::ZERO)))
            .id();
        // The target sits off to the side of the body heading, so the lock
        // turns the camera a quarter turn to behind the player aimed at it.
        app.world_mut().spawn((
            kuluu_render::components::WorldEntity {
                id: 7,
                act_index: 0,
                kind: kuluu_snapshot::EntityKind::Mob,
            },
            Transform::from_translation(Vec3::new(0.0, 1.0, 5.0)),
        ));

        app.world_mut()
            .resource_mut::<kuluu_render::lock_on::LockOn>()
            .target_id = Some(7);
        let bearing = lock_yaw(Vec2::new(0.0, 0.0), Vec2::new(0.0, 5.0)).expect("distinct");
        for _ in 0..20_000 {
            app.update();
            let chase = app.world().resource::<ChaseCamera>();
            if (continuous_yaw(bearing, chase.yaw) - bearing).abs() < 0.02 {
                break;
            }
        }
        let chase = app.world().resource::<ChaseCamera>();
        let gap = (continuous_yaw(bearing, chase.yaw) - bearing).abs();
        assert!(gap < 0.02, "the lock must settle on the bearing, gap {gap}");
        let eye_locked = app
            .world()
            .entity(cam)
            .get::<Transform>()
            .unwrap()
            .translation;

        app.world_mut()
            .resource_mut::<kuluu_render::lock_on::LockOn>()
            .target_id = None;
        app.update();

        let eye_released = app
            .world()
            .entity(cam)
            .get::<Transform>()
            .unwrap()
            .translation;
        assert!(
            (eye_released - eye_locked).length() < 1.0,
            "the release frame must not teleport the eye: {eye_locked:?} -> {eye_released:?}"
        );

        // The yaw springs back behind the body's heading in well under a
        // second of game time, not parked at the lock bearing.
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

    /// Locked on with the spring enabled, a player circling the target keeps the camera at one
    /// distance: the lock turn owns the yaw, so the eye rides the boom instead of being orbited off it
    /// (which left it beside the player, where every strafe step ran along the boom and pumped the
    /// distance between the zoom and half of it on each circuit).
    #[test]
    fn circling_a_locked_target_keeps_the_camera_distance() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(CameraMode::Chase)
            .insert_resource(kuluu_render::GraphicsSettings {
                camera_spring: true,
                ..Default::default()
            })
            .insert_resource(SceneState::default())
            .init_resource::<ZoneCollisionBvh>()
            .insert_resource(kuluu_render::camera::CameraStepSmoothing::default())
            .init_resource::<kuluu_render::cutscene::CutsceneMode>()
            .insert_resource(kuluu_render::lock_on::LockOn::default())
            .insert_resource(ChaseCamera::default())
            .add_systems(Update, resolve_camera);

        let radius = 3.0_f32;
        let player = app
            .world_mut()
            .spawn((
                IsSelf,
                Transform::from_translation(Vec3::new(radius, 1.0, 0.0)),
            ))
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
            Transform::from_translation(Vec3::new(0.0, 1.0, 0.0)),
        ));
        app.world_mut()
            .resource_mut::<kuluu_render::lock_on::LockOn>()
            .target_id = Some(7);
        // Let the lock settle before measuring.
        for _ in 0..200 {
            app.update();
        }

        // The locked-on side step: 3.75 y/s on a 3-yalm circle, body re-aimed at the target each
        // frame the way input.rs does it.
        let angular_rate = 3.75 / radius;
        let t0 = app.world().resource::<Time>().elapsed_secs();
        let mut min_d = f32::MAX;
        let mut max_d = 0.0_f32;
        let mut elapsed = 0.0;
        while elapsed < 6.0 {
            app.update();
            elapsed = app.world().resource::<Time>().elapsed_secs() - t0;
            let angle = angular_rate * elapsed;
            let pos = Vec3::new(radius * angle.cos(), 1.0, radius * angle.sin());
            app.world_mut()
                .entity_mut(player)
                .get_mut::<Transform>()
                .unwrap()
                .translation = pos;
            let heading = heading_byte_for_rad((-pos.z).atan2(-pos.x));
            app.world_mut()
                .resource_mut::<SceneState>()
                .snapshot
                .self_pos
                .heading = heading;
            let eye = app
                .world()
                .entity(cam)
                .get::<Transform>()
                .unwrap()
                .translation;
            let d = Vec2::new(eye.x - pos.x, eye.z - pos.z).length();
            min_d = min_d.min(d);
            max_d = max_d.max(d);
        }
        assert!(
            max_d - min_d < 0.25,
            "the locked camera distance must hold while circling, got {min_d}..{max_d}"
        );
    }
}
