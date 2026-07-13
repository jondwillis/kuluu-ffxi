use bevy::prelude::*;

use ffxi_viewer_core::components::IsSelf;
use ffxi_viewer_core::dat_mzb::{DrawDistance, ZoneGeomMode};
use ffxi_viewer_core::scene::BakedActor;
use ffxi_viewer_core::{third_person_anchor_y, CameraMode, ChaseCamera, OperatorCamera};

use super::collision_bvh::{CollisionBvh, ZoneCollisionBvh};

// research/xim/src/jsMain/kotlin/xim/poc/camera/PolarCamera.kt:209 —
// `(distance - 0.25f).coerceAtLeast(0.5f)`: pad off the wall, but never pull the
// camera closer than 0.5 to the anchor (tiny interiors like the Mog House would
// otherwise collapse it inside the character model).
const WALL_PAD: f32 = 0.25;

const CAMERA_MIN_DISTANCE: f32 = 0.5;

const OUTWARD_LERP: f32 = 0.18;

const INWARD_LERP: f32 = 0.45;

pub fn clamp_chase_camera_to_collision(
    mode: Res<CameraMode>,
    chase: Res<ChaseCamera>,
    time: Res<Time>,
    draw: Res<DrawDistance>,
    zone_bvh: Res<ZoneCollisionBvh>,
    self_q: Query<(&Transform, Option<&BakedActor>), (With<IsSelf>, Without<OperatorCamera>)>,
    mut cam_q: Query<&mut Transform, (With<OperatorCamera>, Without<IsSelf>)>,
    bvh_q: Query<&CollisionBvh>,

    pending_q: Query<
        Entity,
        (
            With<ffxi_viewer_core::components::CameraOccluder>,
            Without<CollisionBvh>,
        ),
    >,
    mut last_summary: Local<Option<(usize, usize)>>,

    mut last_probe_log: Local<f32>,

    mut smoothed_effective: Local<Option<f32>>,
) {
    let source = draw.camera_collision_source;
    let bvh_count = bvh_q.iter().count();
    let pending_count = pending_q.iter().count();
    let summary = (bvh_count, pending_count);
    if *last_summary != Some(summary) {
        *last_summary = Some(summary);
        for (i, bvh) in bvh_q.iter().enumerate() {
            let (mn, mx) = bvh.root_aabb().unwrap_or((Vec3::ZERO, Vec3::ZERO));
            tracing::debug!(
                bvh_index = i,
                tri_count = bvh.tri_count(),
                aabb_min = ?(mn.x, mn.y, mn.z),
                aabb_max = ?(mx.x, mx.y, mx.z),
                "camera_collision probe: MMB BVH summary"
            );
        }
        tracing::debug!(
            source = source.label(),
            mmb_bvhs = bvh_count,
            mmb_pending = pending_count,
            zone_bvh = zone_bvh.0.is_some(),
            zone_bvh_tris = zone_bvh.0.as_ref().map(|b| b.tri_count()).unwrap_or(0),
            "camera_collision probe: coverage summary"
        );
    }

    if !matches!(*mode, CameraMode::Chase) {
        *smoothed_effective = None;
        return;
    }
    let Ok((self_t, baked)) = self_q.single() else {
        return;
    };
    let Ok(mut cam_t) = cam_q.single_mut() else {
        return;
    };

    let anchor = self_t.translation + Vec3::Y * third_person_anchor_y(baked);

    let cos_p = chase.pitch.cos();
    let sin_p = chase.pitch.sin();
    let dir = Vec3::new(chase.yaw.sin() * cos_p, sin_p, chase.yaw.cos() * cos_p);

    let wanted = chase.distance;

    let mut hit_t = wanted;
    let mut hit_any = false;

    if source.uses_mzb() {
        if let Some(bvh) = &zone_bvh.0 {
            if let Some(t) = bvh.ray_cast(anchor, dir, hit_t) {
                if t < hit_t {
                    hit_t = t;
                    hit_any = true;
                }
            }
        }
    }

    if source.uses_mmb() {
        for bvh in bvh_q.iter() {
            if let Some(t) = bvh.ray_cast(anchor, dir, hit_t) {
                if t < hit_t {
                    hit_t = t;
                    hit_any = true;
                }
            }
        }
    }

    let now = time.elapsed_secs();
    if now - *last_probe_log >= 1.0 && tracing::enabled!(tracing::Level::DEBUG) {
        *last_probe_log = now;
        let probe_start = std::time::Instant::now();

        let mut brute_hit_t = wanted;
        let mut brute_hit_any = false;
        let mut total_tris: usize = 0;
        if source.uses_mzb() {
            if let Some(bvh) = &zone_bvh.0 {
                total_tris += bvh.tri_count();
                if let Some(t) = bvh.ray_cast_brute_force(anchor, dir, brute_hit_t) {
                    if t < brute_hit_t {
                        brute_hit_t = t;
                        brute_hit_any = true;
                    }
                }
            }
        }
        if source.uses_mmb() {
            for bvh in bvh_q.iter() {
                total_tris += bvh.tri_count();
                if let Some(t) = bvh.ray_cast_brute_force(anchor, dir, brute_hit_t) {
                    if t < brute_hit_t {
                        brute_hit_t = t;
                        brute_hit_any = true;
                    }
                }
            }
        }
        tracing::debug!(
            source = source.label(),
            anchor = ?(anchor.x, anchor.y, anchor.z),
            dir = ?(dir.x, dir.y, dir.z),
            wanted,
            bvh_hit = hit_any,
            bvh_hit_t = if hit_any { hit_t } else { f32::NAN },
            brute_hit = brute_hit_any,
            brute_hit_t = if brute_hit_any { brute_hit_t } else { f32::NAN },
            total_tris,
            "camera_collision probe: per-cast outcome"
        );
        ffxi_viewer_core::perf_probe::note_debug_probe(probe_start.elapsed());
    }

    let target = clamped_camera_distance(hit_t, wanted);

    let effective = match *smoothed_effective {
        Some(prev) if target < prev => target * INWARD_LERP + prev * (1.0 - INWARD_LERP),
        Some(prev) => prev + (target - prev) * OUTWARD_LERP,
        None => target,
    };
    *smoothed_effective = Some(effective);

    cam_t.translation = anchor + dir * effective;
    cam_t.look_at(anchor, Vec3::Y);
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
    let wanted_end = anchor + dir * chase.distance;

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
        // XIM PolarCamera.kt:209: (distance - 0.25).coerceAtLeast(0.5) — a wall
        // right at the anchor (tiny Mog House rooms) must not pull the camera
        // inside the character model.
        assert_eq!(clamped_camera_distance(0.0, 6.0), CAMERA_MIN_DISTANCE);
        assert_eq!(clamped_camera_distance(0.3, 6.0), CAMERA_MIN_DISTANCE);
    }

    #[test]
    fn camera_distance_pads_off_walls_and_caps_at_wanted() {
        assert_eq!(clamped_camera_distance(3.0, 6.0), 3.0 - WALL_PAD);
        assert_eq!(clamped_camera_distance(100.0, 6.0), 6.0);
    }
}
