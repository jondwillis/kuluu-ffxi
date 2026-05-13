//! Clamp the third-person chase camera against zone collision so it
//! doesn't tunnel through walls.
//!
//! Model: [`ChaseCamera::distance`] is the player's **wanted** distance
//! (unchanged by collision — it's what mouse-wheel zoom controls). Each
//! frame we ray-cast from the look-at anchor toward the wanted camera
//! position against [`MzbCollisionMesh`] triangles. The **effective**
//! distance is `min(wanted, hit_t - WALL_PAD)`, and we write the camera
//! transform at `anchor + dir * effective`. When the obstruction clears,
//! the next frame's `chase_camera_system` lerp picks up from the clamped
//! position and pulls back toward wanted naturally.
//!
//! Why ray-vs-triangle and not Detour's `slide_along` (the prior
//! implementation): Detour only knows the walkable surface, so it
//! couldn't catch ceilings, overhangs, or cases where the camera was
//! pitched up through a wall above the navmesh. Zone collision (MZB) is
//! the actual 3D geometry the player can see.
//!
//! Cost: brute-force Möller–Trumbore over every triangle in every
//! [`MzbCollisionMesh`] entity, every frame. Same approach as the
//! existing `/debug heights` downward raycast. Acceptable for current
//! zone sizes; if this becomes a hot path, add a BVH around the merged
//! collision mesh.

use bevy::prelude::*;

use ffxi_viewer_core::components::IsSelf;
use ffxi_viewer_core::{CameraMode, ChaseCamera, OperatorCamera};

use super::collision_bvh::CollisionBvh;

/// Pull the camera this many Bevy units shy of the wall hit so the near
/// plane doesn't slice into the geometry. 0.2 yalms ≈ a few centimeters
/// in FFXI scale — invisible but enough margin for floating-point slop.
const WALL_PAD: f32 = 0.2;

/// Never collapse the camera closer than this to the anchor — at zero
/// distance the chase camera degenerates into first-person, which would
/// be a jarring view change just because the player brushed a wall.
/// 0.5 yalm keeps a recognizable "pulled-in shoulder cam" feel.
const MIN_EFFECTIVE_DISTANCE: f32 = 0.5;

/// Run AFTER `chase_camera_system` (Update) — schedule in PostUpdate.
/// Recomputes the camera position from `ChaseCamera`'s wanted distance
/// and the ray-clamped effective distance, overwriting the lerped
/// translation that `chase_camera_system` produced.
///
/// Skipped in [`CameraMode::FirstPerson`] (no chase distance to clamp).
pub fn clamp_chase_camera_to_collision(
    mode: Res<CameraMode>,
    chase: Res<ChaseCamera>,
    self_q: Query<&Transform, (With<IsSelf>, Without<OperatorCamera>)>,
    mut cam_q: Query<&mut Transform, (With<OperatorCamera>, Without<IsSelf>)>,
    mzb_q: Query<(&Mesh3d, &GlobalTransform), With<MzbCollisionMesh>>,
    meshes: Res<Assets<Mesh>>,
) {
    if !matches!(*mode, CameraMode::Chase) {
        return;
    }
    let Ok(self_t) = self_q.single() else {
        return;
    };
    let Ok(mut cam_t) = cam_q.single_mut() else {
        return;
    };

    // Anchor = the camera's look-at point. Same expression as in
    // `chase_camera_system`: lift off the floor by `height_target` so
    // we frame the player's torso, not their feet.
    let anchor = self_t.translation + Vec3::Y * chase.height_target;

    // Wanted direction — matches `chase_camera_system`'s spherical
    // parameterization. `dir` already has unit length: yaw_dir is unit
    // in the XZ plane scaled by cos(pitch), plus Y * sin(pitch).
    let cos_p = chase.pitch.cos();
    let sin_p = chase.pitch.sin();
    let dir = Vec3::new(
        chase.yaw.sin() * cos_p,
        sin_p,
        chase.yaw.cos() * cos_p,
    );

    let wanted = chase.distance;

    // Closest forward hit on any collision triangle along the ray.
    let mut hit_t = wanted;
    for (mesh3d, global) in mzb_q.iter() {
        let Some(mesh) = meshes.get(mesh3d.0.id()) else {
            continue;
        };
        let Some(positions) = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|a| a.as_float3())
        else {
            continue;
        };
        let Some(indices) = mesh.indices() else {
            continue;
        };
        let xform = global.to_matrix();

        let mut iter = indices.iter();
        while let (Some(i0), Some(i1), Some(i2)) = (iter.next(), iter.next(), iter.next()) {
            let v0 = xform.transform_point3(Vec3::from_array(positions[i0]));
            let v1 = xform.transform_point3(Vec3::from_array(positions[i1]));
            let v2 = xform.transform_point3(Vec3::from_array(positions[i2]));
            if let Some(t) = ray_tri_intersect(anchor, dir, v0, v1, v2) {
                if t < hit_t {
                    hit_t = t;
                }
            }
        }
    }

    let effective = (hit_t - WALL_PAD).max(MIN_EFFECTIVE_DISTANCE).min(wanted);
    cam_t.translation = anchor + dir * effective;
    cam_t.look_at(anchor, Vec3::Y);
}

/// Möller–Trumbore ray/triangle intersection. Returns the ray parameter
/// `t` of the hit (positive forward distance along `dir`, which must be
/// unit length for `t` to be interpreted as world distance), or `None`
/// if the ray misses the triangle or hits behind the origin.
///
/// Duplicated from `view_native::debug_heights` — the two callers want
/// slightly different shape (this one accepts only forward hits and
/// returns `t` directly; the heights one tracks max-Y of the hit point)
/// but the math is identical. If a third caller appears, factor into a
/// shared `ray_tri.rs` helper.
fn ray_tri_intersect(orig: Vec3, dir: Vec3, v0: Vec3, v1: Vec3, v2: Vec3) -> Option<f32> {
    const EPS: f32 = 1e-7;
    let e1 = v1 - v0;
    let e2 = v2 - v0;
    let h = dir.cross(e2);
    let a = e1.dot(h);
    if a.abs() < EPS {
        return None;
    }
    let f = 1.0 / a;
    let s = orig - v0;
    let u = f * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = f * dir.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = f * e2.dot(q);
    if t > EPS {
        Some(t)
    } else {
        None
    }
}
