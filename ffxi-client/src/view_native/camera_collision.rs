//! Clamp the third-person chase camera against zone collision so it
//! doesn't tunnel through walls.
//!
//! Invariant: the camera is placed **at or closer than** the nearest
//! ray-trace intersection between the look-at anchor and the wanted
//! camera position. Wanted distance is tracked separately
//! ([`ChaseCamera::distance`], untouched by collision — that's what the
//! mouse-wheel and `.`/`,` zoom controls). Effective distance is
//! `min(wanted, hit_t - WALL_PAD)`, never floored above the hit.
//!
//! When the obstruction clears, the next frame's `chase_camera_system`
//! lerp picks up from the clamped position and pulls back toward wanted
//! naturally.
//!
//! Why ray-vs-triangle and not Detour's `slide_along` (the prior
//! implementation): Detour only knows the walkable surface, so it
//! couldn't catch ceilings, overhangs, or cases where the camera was
//! pitched up through a wall above the navmesh. Zone collision (MZB) is
//! the actual 3D geometry the player can see.
//!
//! Cost: per-frame ray cast against a per-entity [`CollisionBvh`] (see
//! `collision_bvh.rs`). The BVH is built once when the
//! [`MzbCollisionMesh`]'s asset finishes loading and cached on the
//! entity, so per-frame work is roughly O(log N) plus a small leaf
//! scan. Was brute-force O(N) before — that tanked FPS in
//! triangle-dense zones.

use bevy::prelude::*;

use ffxi_viewer_core::components::IsSelf;
use ffxi_viewer_core::{CameraMode, ChaseCamera, OperatorCamera};

use super::collision_bvh::CollisionBvh;

/// Pull the camera this many Bevy units shy of the wall hit so the near
/// plane doesn't slice into the geometry. 0.2 yalms ≈ a few centimeters
/// in FFXI scale — invisible but enough margin for floating-point slop.
const WALL_PAD: f32 = 0.2;

/// Tiny non-zero floor for the camera-to-anchor distance. Exists only
/// to keep `Transform::look_at` from receiving origin == target (which
/// produces NaN in Bevy). At 1e-3 yalm the camera is functionally at
/// the anchor — this is *not* a UX guard, it's a math guard. The
/// stated invariant ("camera ≤ ray hit") forbids any larger floor.
const ANCHOR_EPSILON: f32 = 1e-3;

/// Per-frame lerp factor for the smoothed effective distance when
/// extending **outward** (target > current). Lower = smoother pull-out.
/// Matches the chase camera's `smoothing = 0.18` so the feel doesn't
/// change vs. before-collision behavior when the camera is just
/// orbiting in open air.
const OUTWARD_LERP: f32 = 0.18;
/// Snap factor when **pulling in** (target < current — a wall just
/// came between player and camera). 1.0 = instant. The invariant
/// "camera ≤ ray hit" requires we never lag *outside* the wall, so
/// inward must be effectively immediate.
const INWARD_LERP: f32 = 1.0;

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
    bvh_q: Query<&CollisionBvh>,
    // TEMP probe: count entities that are MzbCollisionMesh-marked but
    // haven't yet received a CollisionBvh. If this stays > 0 across
    // frames, the build system is silently skipping some meshes — that
    // matches the user's "only some meshes" symptom.
    pending_q: Query<
        Entity,
        (
            With<ffxi_viewer_core::dat_mzb::MzbCollisionMesh>,
            Without<CollisionBvh>,
        ),
    >,
    mut last_summary: Local<Option<(usize, usize)>>,
    // Smoothed effective distance — low-pass filter on the noisy
    // BVH hit_t. Survives across frames as a `Local`, no extra
    // resource plumbing needed. `None` means "no prior frame" (first
    // run / mode just entered), in which case we initialize to the
    // target without smoothing.
    mut smoothed_effective: Local<Option<f32>>,
) {
    // TEMP probe: emit a summary whenever the BVH coverage count changes.
    // Stable counts = no log spam; fluctuating counts = build race or
    // partial coverage we need to chase. Logs:
    //   bvhs        = number of CollisionBvh components alive
    //   pending     = MzbCollisionMesh entities still without a BVH
    //   per_bvh     = tri count + world-space AABB of each BVH so the
    //                 operator can sanity-check positioning
    let bvh_count = bvh_q.iter().count();
    let pending_count = pending_q.iter().count();
    let summary = (bvh_count, pending_count);
    if *last_summary != Some(summary) {
        *last_summary = Some(summary);
        for (i, bvh) in bvh_q.iter().enumerate() {
            let (mn, mx) = bvh.root_aabb().unwrap_or((Vec3::ZERO, Vec3::ZERO));
            tracing::info!(
                bvh_index = i,
                tri_count = bvh.tri_count(),
                aabb_min = ?(mn.x, mn.y, mn.z),
                aabb_max = ?(mx.x, mx.y, mx.z),
                "camera_collision probe: BVH summary"
            );
        }
        tracing::info!(
            bvhs = bvh_count,
            pending_meshes = pending_count,
            "camera_collision probe: coverage summary"
        );
    }

    if !matches!(*mode, CameraMode::Chase) {
        // Reset the smoothed distance so re-entering chase from FP
        // doesn't start with a stale value.
        *smoothed_effective = None;
        return;
    }
    let Ok(self_t) = self_q.single() else {
        return;
    };
    let Ok(mut cam_t) = cam_q.single_mut() else {
        return;
    };

    // Ray-cast geometry, matching `chase_camera_system` exactly:
    //
    //   - Ray origin    = player root (self_t.translation). The user's
    //                     goal phrases it as "from the player to where
    //                     the camera would otherwise be."
    //   - Ray direction = spherical (yaw, pitch) unit vector, same as
    //                     chase_camera_system's `yaw_dir * cos_p + Y *
    //                     sin_p`.
    //   - Wanted endpoint = self_t + dir * chase.distance — the exact
    //                       point chase_camera_system would have placed
    //                       the camera if nothing were in the way.
    //
    // The look-at *target* lives at `self_t + Y * height_target`
    // (separate from ray geometry — that's just where the camera
    // frames, not where the camera sits). Using that as the ray
    // origin (which an earlier rev did) lifted the whole camera by
    // `height_target` even in open air, which read as "collision
    // isn't kicking in" when really the camera was placed wrong.
    let player = self_t.translation;
    let look_at = self_t.translation + Vec3::Y * chase.height_target;

    let cos_p = chase.pitch.cos();
    let sin_p = chase.pitch.sin();
    let dir = Vec3::new(
        chase.yaw.sin() * cos_p,
        sin_p,
        chase.yaw.cos() * cos_p,
    );

    let wanted = chase.distance;

    // Closest forward hit across all collision-mesh BVHs. Each BVH's
    // ray_cast is roughly O(log N) plus a small leaf scan, so total
    // cost is bounded even in triangle-dense zones.
    let mut hit_t = wanted;
    for bvh in bvh_q.iter() {
        if let Some(t) = bvh.ray_cast(player, dir, hit_t) {
            if t < hit_t {
                hit_t = t;
            }
        }
    }

    // Target distance before smoothing. Invariant maintained: target ≤
    // hit_t. WALL_PAD pulls *in* (still ≤ hit_t); `.min(wanted)` clamps
    // to the operator-set zoom; `.max(ANCHOR_EPSILON)` only saves
    // `look_at` from origin == target.
    let target = (hit_t - WALL_PAD).min(wanted).max(ANCHOR_EPSILON);

    // Smooth between frames. The asymmetric lerp keeps the invariant:
    // when target shrinks (wall appeared, must move closer) we snap
    // immediately (INWARD_LERP = 1.0); when target grows (wall cleared,
    // can ease back out) we lerp at OUTWARD_LERP. Without this the raw
    // `target` jitters by sub-yalm amounts as the ray sweeps across
    // triangle edges during yaw rotation, and the chase camera (which
    // we override every frame) loses its own lerp's smoothing.
    let effective = match *smoothed_effective {
        Some(prev) if target < prev => {
            // Pulling in — snap to satisfy the ≤ hit_t invariant.
            target * INWARD_LERP + prev * (1.0 - INWARD_LERP)
        }
        Some(prev) => {
            // Pulling out — smooth.
            prev + (target - prev) * OUTWARD_LERP
        }
        None => target,
    };
    *smoothed_effective = Some(effective);

    cam_t.translation = player + dir * effective;
    cam_t.look_at(look_at, Vec3::Y);
}
