//! Clamp the third-person chase camera against zone collision so it
//! doesn't tunnel through walls.
//!
//! Invariant: the camera is placed **at or closer than** the nearest
//! ray-trace intersection between the torso anchor (= look-at point,
//! `feet + Y * third_person_anchor_y(baked)`, a race-aware fraction
//! of the actor's full visual height) and the wanted camera position.
//! Both the ray origin AND the final camera placement pivot around
//! this anchor — mixing the two (e.g. ray from chest, camera placed
//! from feet) lifted the whole camera in open air; using the foot
//! position for both made stair risers behind the player clip the
//! ray at ankle level, "shoving" the camera in on every step.
//! Wanted distance is tracked separately
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
use ffxi_viewer_core::dat_mzb::{DrawDistance, ZoneGeomMode};
use ffxi_viewer_core::scene::BakedActor;
use ffxi_viewer_core::{third_person_anchor_y, CameraMode, ChaseCamera, OperatorCamera};

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
/// Lerp factor when **pulling in** (target < current — a wall just
/// came between player and camera). Was 1.0 (hard snap); softened to
/// 0.45 so the camera eases inward over a few frames instead of
/// teleporting, which read as a visible "pop". This **intentionally
/// relaxes** the "camera ≤ ray hit" invariant for a few frames
/// (≤ ~3 at 60 Hz before residual error falls below visible scale).
/// WALL_PAD's ~0.2 yalm margin keeps the visible clipping minimal,
/// and operator feedback preferred the easing over the snap.
const INWARD_LERP: f32 = 0.45;

/// Run AFTER `chase_camera_system` (Update) — schedule in PostUpdate.
/// Recomputes the camera position from `ChaseCamera`'s wanted distance
/// and the ray-clamped effective distance, overwriting the lerped
/// translation that `chase_camera_system` produced.
///
/// Skipped in [`CameraMode::FirstPerson`] (no chase distance to clamp).
pub fn clamp_chase_camera_to_collision(
    mode: Res<CameraMode>,
    chase: Res<ChaseCamera>,
    time: Res<Time>,
    self_q: Query<(&Transform, Option<&BakedActor>), (With<IsSelf>, Without<OperatorCamera>)>,
    mut cam_q: Query<&mut Transform, (With<OperatorCamera>, Without<IsSelf>)>,
    bvh_q: Query<&CollisionBvh>,
    // TEMP probe: count entities that are CameraOccluder-marked but
    // haven't yet received a CollisionBvh. If this stays > 0 across
    // frames, the build system is silently skipping some meshes — that
    // matches the user's "only some meshes" symptom.
    pending_q: Query<
        Entity,
        (
            With<ffxi_viewer_core::components::CameraOccluder>,
            Without<CollisionBvh>,
        ),
    >,
    mut last_summary: Local<Option<(usize, usize)>>,
    // TEMP probe (port-sandoria ceiling/floor bug): per-cast outcome,
    // throttled to ~1 Hz. Logs hit-or-miss, hit_t, and whether the
    // wanted endpoint lands inside any BVH AABB — if it does and the
    // ray still misses, that's a geometry/BVH bug; if it doesn't, the
    // ceiling/floor lives in an entity that has no CollisionBvh
    // (likely an MMB placement).
    mut last_probe_log: Local<f32>,
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
            tracing::debug!(
                bvh_index = i,
                tri_count = bvh.tri_count(),
                aabb_min = ?(mn.x, mn.y, mn.z),
                aabb_max = ?(mx.x, mx.y, mx.z),
                "camera_collision probe: BVH summary"
            );
        }
        tracing::debug!(
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
    let Ok((self_t, baked)) = self_q.single() else {
        return;
    };
    let Ok(mut cam_t) = cam_q.single_mut() else {
        return;
    };

    // Ray-cast geometry, matching `chase_camera_system` exactly:
    //
    //   - Ray origin    = torso anchor (`third_person_anchor_y(baked)`,
    //                     a fraction of the actor's full visual height).
    //                     This is the same point the camera frames via
    //                     `look_at`, so the ray represents the actual
    //                     line of sight from the framing point to the
    //                     camera. Anchoring at feet (an earlier rev)
    //                     made stair risers BEHIND the player clip the
    //                     ray at ankle level even when the camera
    //                     visually sat above them — the asymmetric
    //                     INWARD_LERP then snapped the camera in each
    //                     frame, producing the "shove in on stairs"
    //                     symptom.
    //   - Ray direction = spherical (yaw, pitch) unit vector, same as
    //                     chase_camera_system's `yaw_dir * cos_p + Y *
    //                     sin_p`.
    //   - Wanted endpoint = anchor + dir * chase.distance — the exact
    //                       point chase_camera_system would have placed
    //                       the camera if nothing were in the way.
    //
    // An even-earlier rev mixed the two (ray from chest, camera placed
    // from feet); that lifted the whole camera by the anchor in open
    // air. The fix is to make BOTH the ray AND the camera placement use
    // the same anchor — consistent geometry, no offset mismatch.
    let anchor = self_t.translation + Vec3::Y * third_person_anchor_y(baked);

    let cos_p = chase.pitch.cos();
    let sin_p = chase.pitch.sin();
    let dir = Vec3::new(chase.yaw.sin() * cos_p, sin_p, chase.yaw.cos() * cos_p);

    let wanted = chase.distance;

    // Closest forward hit across all collision-mesh BVHs. Each BVH's
    // ray_cast is roughly O(log N) plus a small leaf scan, so total
    // cost is bounded even in triangle-dense zones.
    let mut hit_t = wanted;
    let mut hit_any = false;
    for bvh in bvh_q.iter() {
        if let Some(t) = bvh.ray_cast(anchor, dir, hit_t) {
            if t < hit_t {
                hit_t = t;
                hit_any = true;
            }
        }
    }

    // TEMP probe: throttle to ~1 Hz. Compare BVH ray cast vs brute-force
    // ray cast over every triangle. Three outcomes carry different
    // diagnostic weight:
    //   BVH hit  & brute hit  -> working; the camera should clamp.
    //   BVH miss & brute miss -> no collision triangle along this ray.
    //                            The ceiling/floor isn't in this BVH at
    //                            all (channel coverage gap).
    //   BVH miss & brute hit  -> BVH traversal or structure is buggy.
    //                            Brute force is the ground truth.
    let now = time.elapsed_secs();
    if now - *last_probe_log >= 1.0 {
        *last_probe_log = now;
        let mut brute_hit_t = wanted;
        let mut brute_hit_any = false;
        let mut total_tris: usize = 0;
        for bvh in bvh_q.iter() {
            total_tris += bvh.tri_count();
            if let Some(t) = bvh.ray_cast_brute_force(anchor, dir, brute_hit_t) {
                if t < brute_hit_t {
                    brute_hit_t = t;
                    brute_hit_any = true;
                }
            }
        }
        tracing::debug!(
            anchor = ?(anchor.x, anchor.y, anchor.z),
            dir = ?(dir.x, dir.y, dir.z),
            wanted,
            bvh_hit = hit_any,
            bvh_hit_t = if hit_any { hit_t } else { f32::NAN },
            brute_hit = brute_hit_any,
            brute_hit_t = if brute_hit_any { brute_hit_t } else { f32::NAN },
            bvhs = bvh_q.iter().count(),
            total_tris,
            "camera_collision probe: per-cast outcome"
        );
    }

    // Target distance before smoothing. Invariant maintained: target ≤
    // hit_t. WALL_PAD pulls *in* (still ≤ hit_t); `.min(wanted)` clamps
    // to the operator-set zoom; `.max(ANCHOR_EPSILON)` only saves
    // `look_at` from origin == target.
    let target = (hit_t - WALL_PAD).min(wanted).max(ANCHOR_EPSILON);

    // Smooth between frames. Asymmetric lerp: when target shrinks
    // (wall appeared, must move closer) we ease inward at INWARD_LERP
    // (visibly fast, not a teleport); when target grows (wall cleared,
    // can ease back out) we lerp slower at OUTWARD_LERP. Without this the raw
    // `target` jitters by sub-yalm amounts as the ray sweeps across
    // triangle edges during yaw rotation, and the chase camera (which
    // we override every frame) loses its own lerp's smoothing.
    let effective = match *smoothed_effective {
        Some(prev) if target < prev => {
            // Pulling in — ease toward `target` at INWARD_LERP per frame.
            // The "camera ≤ hit_t" invariant is **intentionally relaxed**
            // here: the operator-perceived feel of a brief inward ease is
            // worth a few frames of partial wall-clip, which is itself
            // hidden by WALL_PAD's ~0.2 yalm margin. Retail's camera
            // behaves similarly — see the design note in the unit-doc.
            target * INWARD_LERP + prev * (1.0 - INWARD_LERP)
        }
        Some(prev) => {
            // Pulling out — smooth.
            prev + (target - prev) * OUTWARD_LERP
        }
        None => target,
    };
    *smoothed_effective = Some(effective);

    cam_t.translation = anchor + dir * effective;
    cam_t.look_at(anchor, Vec3::Y);
}

/// Debug-overlay gizmos for `/zonegeom camera`. Runs every frame but
/// is a no-op unless `DrawDistance.zone_geom_mode == ZoneGeomMode::Camera`,
/// so it costs essentially nothing when not active.
///
/// What it draws:
/// - Each [`CollisionBvh`]'s root AABB as a green wirebox — the bounds
///   the camera raycast actually tests against. Useful for spotting
///   coverage gaps ("this room's ceiling has no AABB, that's why the
///   camera tunnels through it").
/// - The active player→camera ray and clamp state — TODO below.
///
/// Lifecycle (per `bevy-lifecycle-symmetry`): gizmos are ephemeral —
/// drawn into a per-frame retained buffer that Bevy clears each frame.
/// **No despawn pair is required** for this system. If a future change
/// adds a cached `Resource` here (e.g. memoized triangle list), it
/// MUST get a paired drain on `OnExit(AppPhase::InGame)`.
pub fn draw_camera_collision_debug(
    draw: Res<DrawDistance>,
    mode: Res<CameraMode>,
    chase: Res<ChaseCamera>,
    self_q: Query<(&Transform, Option<&BakedActor>), (With<IsSelf>, Without<OperatorCamera>)>,
    cam_q: Query<&Transform, (With<OperatorCamera>, Without<IsSelf>)>,
    bvh_q: Query<&CollisionBvh>,
    mut gizmos: Gizmos,
) {
    if draw.zone_geom_mode != ZoneGeomMode::Camera {
        return;
    }

    // BVH coverage — one wirebox per loaded CollisionBvh. **Cyan** so it
    // doesn't blur into the navmesh overlay (bright green); slightly
    // translucent (alpha 0.55) so the actual zone geometry behind it
    // stays legible. Picked deliberately away from the green/yellow/red
    // ramp the ray-state segments use below.
    let aabb_color = Color::srgba(0.20, 0.80, 1.0, 0.55);
    for bvh in bvh_q.iter() {
        let Some((mn, mx)) = bvh.root_aabb() else {
            continue;
        };
        let center = (mn + mx) * 0.5;
        let extents = mx - mn;
        gizmos.cuboid(
            Transform::from_translation(center).with_scale(extents),
            aabb_color,
        );
    }

    let Ok((self_t, baked)) = self_q.single() else {
        return;
    };
    let anchor = self_t.translation + Vec3::Y * third_person_anchor_y(baked);

    // Anchor crosshair — small ±0.3 yalm axis cross at the chest anchor.
    // White, mostly opaque (0.90) so it pops against any background and
    // makes the chest-anchor height visually obvious (e.g. above stair
    // risers, not at ankle level).
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

    // Ray viz is only meaningful in Chase mode (FP doesn't ray-cast). In
    // FP we already drew the anchor crosshair above; that's enough.
    if !matches!(*mode, CameraMode::Chase) {
        return;
    }

    let cos_p = chase.pitch.cos();
    let sin_p = chase.pitch.sin();
    let dir = Vec3::new(chase.yaw.sin() * cos_p, sin_p, chase.yaw.cos() * cos_p);
    let wanted_end = anchor + dir * chase.distance;

    // The operator camera's actual world position IS the post-clamp
    // effective endpoint — no need to re-run the BVH cast or expose a
    // Resource. `effective_end == anchor` is possible mid-NaN-guard but
    // gizmo lines handle zero-length segments fine.
    let effective_end = cam_q.single().map(|t| t.translation).unwrap_or(wanted_end);

    // **Yellow** segment: anchor → actual camera position. The path the
    // camera ACTUALLY occupies. Yellow (not green) so it doesn't conflict
    // with the navmesh overlay; slightly translucent so it doesn't
    // overpower the cyan AABBs.
    gizmos.line(anchor, effective_end, Color::srgba(1.0, 0.85, 0.15, 0.85));

    // **Magenta-red** segment: actual camera → wanted endpoint. The
    // "missing" distance the collision clamp pulled in. Shifted toward
    // magenta so it stays distinct from the warm yellow of the effective
    // segment (red-vs-yellow can blur at gizmo line widths). Skipped when
    // the gap is sub-perceptible (< 0.05 yalm) to reduce flicker.
    let clip_amount = (wanted_end - effective_end).length();
    if clip_amount > 0.05 {
        gizmos.line(
            effective_end,
            wanted_end,
            Color::srgba(1.0, 0.25, 0.55, 0.85),
        );
    }
}
