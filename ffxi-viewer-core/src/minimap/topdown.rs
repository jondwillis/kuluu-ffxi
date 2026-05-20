//! Top-down minimap backend: secondary orthographic `Camera3d` that
//! renders the loaded MZB geometry into a texture, **once per
//! zone-enter** (bake-once). Subsequent frames just read the cached
//! texture — no per-frame render cost.
//!
//! # Ceiling-cull trick
//!
//! The "hide ceilings / roofs / tunnel-tops" idea is a positional
//! cull, not a depth shader: the camera sits at `aabb.max.y -
//! top_cull_yalms` looking straight down. Anything *above* that Y is
//! **behind the camera** and the render pipeline drops it for free.
//! Anything *below* `aabb.min.y` is past the `far` plane and also
//! dropped. So a single orthographic pass with carefully-placed
//! near/far planes effectively slices the zone at the cull height.
//!
//! For multi-level zones the [`TopdownCullPolicy::top_cull_yalms`]
//! knob adjusts that slice. Default 6.0 yalms — covers most
//! single-story buildings without sacrificing tall structures' visible
//! ground plane.
//!
//! # Lifecycle
//!
//! A small state machine in [`BakeStage`] drives bake-once semantics:
//!
//! 1. `Idle` — nothing to do.
//! 2. `Requested` — a zone change was detected. The next render-budget
//!    system spawns the bake camera and moves to `Awaiting`.
//! 3. `Awaiting` — wait one frame for the render graph to commit the
//!    bake texture, then despawn the camera and return to `Idle`.
//!
//! A despawned bake camera is harmless — it just stops contributing to
//! future render passes. The texture remains valid in `Assets<Image>`
//! and is what the minimap UI samples from.

use bevy::asset::RenderAssetUsages;
use bevy::camera::ScalingMode;
use bevy::prelude::*;
use bevy::camera::{ClearColorConfig, RenderTarget};
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureUsages,
};

use crate::components::InGameEntity;
use crate::dat_mzb::MzbCollisionGeometry;
use crate::snapshot::SceneState;

use super::{MinimapAabb, MinimapState, MINIMAP_TEX_SIZE};

/// How much of the zone's vertical extent (in yalms) to trim from the
/// top before rendering. Default `6.0` — clips off the upper floor of
/// most two-story buildings while leaving tall outdoor zones with
/// their visible ground.
///
/// Operator-tunable via `/minimap cull <N>` (slash command lands in
/// task #2). On change, the next frame requests a re-bake (see
/// [`bake_topdown_on_zone_or_policy_change`]).
#[derive(Resource, Debug, Clone, Copy)]
pub struct TopdownCullPolicy {
    pub top_cull_yalms: f32,
}

impl Default for TopdownCullPolicy {
    fn default() -> Self {
        Self { top_cull_yalms: 6.0 }
    }
}

/// Lifecycle state for the bake. See module docs for the transitions.
#[derive(Resource, Debug, Default)]
pub enum BakeStage {
    #[default]
    Idle,
    /// Zone-change or cull-policy-change detected; spawn the bake
    /// camera on the next [`spawn_bake_camera`] tick.
    Requested,
    /// Bake camera spawned; wait one frame for the render graph to
    /// commit, then despawn.
    Awaiting(Entity),
}

/// Marker on the secondary orthographic camera used for the bake. Lets
/// [`despawn_bake_camera`] find and clean up the entity without
/// holding an extra `Entity` reference outside [`BakeStage`].
#[derive(Component)]
pub struct MinimapBakeCamera;

/// Plugin registration. Owns the cull policy, the bake-stage state
/// machine, and the three systems that drive the bake-once lifecycle.
pub struct TopdownBackendPlugin;

impl Plugin for TopdownBackendPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TopdownCullPolicy>()
            .init_resource::<BakeStage>()
            .add_systems(
                Update,
                (
                    bake_topdown_on_zone_or_policy_change,
                    spawn_bake_camera,
                    despawn_bake_camera,
                )
                    .chain(),
            );
    }
}

/// Stage 1 of the bake loop: notice when a new zone's MZB geometry has
/// loaded (or the cull policy changed) and flip [`BakeStage`] to
/// `Requested`. No camera work here — that's stage 2's job.
///
/// The trigger fires when:
///   * `MzbCollisionGeometry.is_changed()` — a new zone's geometry
///     just landed via `process_load_mzb_requests`.
///   * OR the cull policy changed.
///   * AND the geometry is non-empty (a zone with zero collision tris
///     can't be baked meaningfully — likely a parse failure).
///   * AND the snapshot's `zone_id` differs from the last baked zone
///     (or this is the first bake / a cull-policy bake).
pub fn bake_topdown_on_zone_or_policy_change(
    geom: Res<MzbCollisionGeometry>,
    policy: Res<TopdownCullPolicy>,
    scene_state: Res<SceneState>,
    state: Res<MinimapState>,
    mut stage: ResMut<BakeStage>,
) {
    let geom_changed = geom.is_changed();
    let policy_changed = policy.is_changed();
    if !geom_changed && !policy_changed {
        return;
    }
    if geom.positions.is_empty() {
        return;
    }
    let snapshot_zone = scene_state.snapshot.zone_id;
    // Same zone as last bake AND only the policy moved → still
    // re-bake; cull-policy edits are the operator's signal to refresh.
    // Same zone AND geometry-only change (rare; usually MZB is replaced
    // wholesale on zone-in) → also re-bake to pick up the new positions.
    if !matches!(*stage, BakeStage::Idle) {
        // Don't pile up requests if we're mid-bake. The current bake
        // will finish; if the operator wants the new policy applied,
        // a second change will re-trigger after BakeStage returns to
        // Idle.
        return;
    }
    if snapshot_zone == state.zone_id && !policy_changed {
        // Same zone, geometry just re-published with identical content
        // — nothing to do.
        return;
    }
    *stage = BakeStage::Requested;
}

/// Stage 2 of the bake loop: when stage is `Requested`, compute the
/// zone AABB, allocate a render target, spawn the bake camera, and
/// publish the texture handle + AABB onto [`MinimapState`].
///
/// The published handle is live before the GPU has actually rendered
/// into it — that's fine, because the UI's `ImageNode` doesn't sample
/// the texture until the next render frame, by which point the bake
/// camera has run.
pub fn spawn_bake_camera(
    geom: Res<MzbCollisionGeometry>,
    policy: Res<TopdownCullPolicy>,
    scene_state: Res<SceneState>,
    mut state: ResMut<MinimapState>,
    mut stage: ResMut<BakeStage>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
) {
    if !matches!(*stage, BakeStage::Requested) {
        return;
    }
    let Some(aabb_3d) = compute_world_aabb(&geom.positions) else {
        // Geometry was non-empty (the trigger checks) but couldn't
        // produce a valid AABB — only possible with all-NaN positions.
        // Bail to Idle so we don't spin.
        *stage = BakeStage::Idle;
        return;
    };

    // Orthographic-camera frustum dimensions = zone XZ span. Bevy's
    // `OrthographicProjection::scaling_mode = Fixed { width, height }`
    // makes the camera see exactly that much world per render.
    let span_x = (aabb_3d.max_x - aabb_3d.min_x).max(1.0);
    let span_z = (aabb_3d.max_z - aabb_3d.min_z).max(1.0);
    let center_x = 0.5 * (aabb_3d.min_x + aabb_3d.max_x);
    let center_z = 0.5 * (aabb_3d.min_z + aabb_3d.max_z);

    // The ceiling cull: the camera sits at `max_y - top_cull_yalms`
    // looking straight down. Anything above that Y is behind the
    // camera (not rendered). Use a small +0.1 epsilon so geometry
    // exactly at `max_y - top_cull_yalms` isn't z-fighting the near
    // plane.
    let ceiling_y = aabb_3d.max_y - policy.top_cull_yalms;
    let span_y_below_camera = (ceiling_y - aabb_3d.min_y).max(1.0) + 10.0;

    let render_target = create_render_target_image(&mut images);

    let aabb = MinimapAabb {
        min: Vec2::new(aabb_3d.min_x, aabb_3d.min_z),
        max: Vec2::new(aabb_3d.max_x, aabb_3d.max_z),
    };

    let camera_entity = commands
        .spawn((
            InGameEntity,
            MinimapBakeCamera,
            Camera3d::default(),
            Camera {
                // Negative order renders before the main view. The
                // main camera at the default order=0 then has the
                // baked texture available for any subsequent
                // sampling (we don't actually sample it from a 3D
                // shader — the UI does — but earlier is harmless and
                // makes the dependency direction explicit).
                order: -1,
                target: RenderTarget::Image(render_target.clone().into()),
                // Clear to transparent so unrendered margins (zones
                // whose AABB doesn't square-fill the texture) blend
                // cleanly with the UI's BACKGROUND color underneath.
                clear_color: ClearColorConfig::Custom(Color::NONE),
                ..default()
            },
            Projection::Orthographic(OrthographicProjection {
                scaling_mode: ScalingMode::Fixed {
                    width: span_x,
                    height: span_z,
                },
                near: 0.0,
                far: span_y_below_camera,
                ..OrthographicProjection::default_3d()
            }),
            // Camera sits at (center_x, ceiling_y, center_z) looking
            // straight down. The `up` axis for `look_at` is the world
            // direction that should appear at the top of the rendered
            // image — we pick Bevy `-Z` (FFXI +y, "north") so the
            // resulting texture has north at the top, matching the
            // overlay's UV convention (V increases southward).
            Transform::from_xyz(center_x, ceiling_y, center_z).looking_at(
                Vec3::new(center_x, ceiling_y - 1.0, center_z),
                Vec3::NEG_Z,
            ),
        ))
        .id();

    state.zone_id = scene_state.snapshot.zone_id;
    state.topdown_image = Some(render_target);
    state.aabb = Some(aabb);

    *stage = BakeStage::Awaiting(camera_entity);
}

/// Stage 3 of the bake loop: one frame after `spawn_bake_camera` ran,
/// despawn the camera entity. The render target lives on in
/// `Assets<Image>` and continues to be sampled by the UI.
///
/// One-frame delay is enough because Bevy's `Render` schedule runs
/// after `Update` within the same frame; by the time this system
/// observes `BakeStage::Awaiting`, the render graph has committed the
/// bake texture from the previous tick.
pub fn despawn_bake_camera(
    mut stage: ResMut<BakeStage>,
    mut commands: Commands,
) {
    let BakeStage::Awaiting(entity) = *stage else {
        return;
    };
    if let Ok(mut ec) = commands.get_entity(entity) {
        ec.despawn();
    }
    *stage = BakeStage::Idle;
}

/// 3D AABB derived from a triangle-soup position list. Used internally
/// by [`spawn_bake_camera`] to size the orthographic frustum and pick
/// the ceiling-cull height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WorldAabb3 {
    pub min_x: f32,
    pub max_x: f32,
    pub min_y: f32,
    pub max_y: f32,
    pub min_z: f32,
    pub max_z: f32,
}

/// Compute the AABB of `positions` in a single pass. Returns `None`
/// when the slice is empty or every coordinate is NaN — both indicate
/// the caller can't meaningfully bake.
pub(crate) fn compute_world_aabb(positions: &[Vec3]) -> Option<WorldAabb3> {
    let mut iter = positions.iter().copied().filter(|p| p.is_finite());
    let first = iter.next()?;
    let mut min = first;
    let mut max = first;
    for p in iter {
        min = min.min(p);
        max = max.max(p);
    }
    Some(WorldAabb3 {
        min_x: min.x,
        max_x: max.x,
        min_y: min.y,
        max_y: max.y,
        min_z: min.z,
        max_z: max.z,
    })
}

/// Allocate a fresh blank RGBA8 render target sized for the minimap.
/// `RenderAssetUsages::RENDER_WORLD` keeps the texture GPU-side only —
/// no CPU read-back, which would otherwise force a per-frame stalling
/// download.
fn create_render_target_image(images: &mut Assets<Image>) -> Handle<Image> {
    let size = Extent3d {
        width: MINIMAP_TEX_SIZE,
        height: MINIMAP_TEX_SIZE,
        depth_or_array_layers: 1,
    };
    let mut image = Image::new_fill(
        size,
        TextureDimension::D2,
        &[0u8, 0, 0, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    // RENDER_ATTACHMENT is what makes the texture a valid render target;
    // TEXTURE_BINDING is what lets the UI sample it; COPY_DST is the
    // standard companion that Bevy's image plumbing assumes.
    image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING
        | TextureUsages::COPY_DST
        | TextureUsages::RENDER_ATTACHMENT;
    images.add(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AABB shrinks to a degenerate point when the position list has
    /// one entry. Catches the "first / chained iter eats the first
    /// element" bug class.
    #[test]
    fn aabb_single_position_is_degenerate() {
        let aabb = compute_world_aabb(&[Vec3::new(1.0, 2.0, 3.0)]).unwrap();
        assert_eq!(aabb.min_x, 1.0);
        assert_eq!(aabb.max_x, 1.0);
        assert_eq!(aabb.min_y, 2.0);
        assert_eq!(aabb.max_y, 2.0);
        assert_eq!(aabb.min_z, 3.0);
        assert_eq!(aabb.max_z, 3.0);
    }

    /// Two opposite-corner positions reproduce as the natural box.
    #[test]
    fn aabb_two_opposite_corners_bound_the_box() {
        let aabb = compute_world_aabb(&[
            Vec3::new(-100.0, -5.0, -200.0),
            Vec3::new(50.0, 30.0, 150.0),
        ])
        .unwrap();
        assert_eq!(aabb.min_x, -100.0);
        assert_eq!(aabb.max_x, 50.0);
        assert_eq!(aabb.min_y, -5.0);
        assert_eq!(aabb.max_y, 30.0);
        assert_eq!(aabb.min_z, -200.0);
        assert_eq!(aabb.max_z, 150.0);
    }

    /// Empty input returns `None` so the caller can bail without
    /// spinning on `Idle ↔ Requested`.
    #[test]
    fn aabb_empty_returns_none() {
        assert!(compute_world_aabb(&[]).is_none());
    }

    /// NaN positions are filtered out — only finite values count.
    /// All-NaN returns `None`. Catches the case where a parse glitch
    /// produces NaN-laden geometry and we don't want the camera to
    /// land at NaN coordinates.
    #[test]
    fn aabb_ignores_nan_positions() {
        let nan = f32::NAN;
        let aabb = compute_world_aabb(&[
            Vec3::new(nan, nan, nan),
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(nan, nan, nan),
        ])
        .unwrap();
        assert_eq!(aabb.min_x, 1.0);
        assert_eq!(aabb.max_x, 1.0);

        assert!(compute_world_aabb(&[Vec3::new(nan, nan, nan)]).is_none());
    }
}
