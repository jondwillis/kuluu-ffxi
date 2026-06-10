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
//! 2. `Requested` — a zone change was detected. The bake camera renders
//!    the *live* scene, so we can't snapshot until the zone's textured
//!    MMB placements have finished streaming in and uploaded to the GPU
//!    — otherwise the snapshot captures black, untextured geometry. This
//!    stage holds until those visuals are ready (see [`spawn_bake_camera`]).
//! 3. `Awaiting` — wait one frame for the render graph to commit the
//!    bake texture, then despawn the camera and return to `Idle`.
//!
//! A despawned bake camera is harmless — it just stops contributing to
//! future render passes. The texture remains valid in `Assets<Image>`
//! and is what the minimap UI samples from.

use bevy::asset::RenderAssetUsages;
use bevy::camera::ScalingMode;
use bevy::camera::{ClearColorConfig, RenderTarget};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};

use crate::components::InGameEntity;
use crate::dat_mmb::MmbLoadQueue;
use crate::dat_mzb::{LoadMzbInFlight, MzbCollisionGeometry};
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
        Self {
            top_cull_yalms: 6.0,
        }
    }
}

/// Lifecycle state for the bake. See module docs for the transitions.
///
/// `Awaiting { frames_remaining }` is a frame counter, not a flag:
/// `spawn_bake_camera` and `despawn_bake_camera` both fire during the
/// same `Update` schedule. Bevy flushes commands between chained
/// systems, so a despawn queued right after a spawn would destroy the
/// entity *before* the Render schedule ever runs — the camera would
/// never get a frame to render into the texture.
///
/// To make sure the bake actually renders, the despawn waits for
/// `frames_remaining` to count down to zero, giving the entity at
/// least one full Update → Render cycle.
#[derive(Resource, Debug, Default)]
pub enum BakeStage {
    #[default]
    Idle,
    /// Zone-change or cull-policy-change detected. The bake can't fire
    /// yet: the camera snapshots the live scene, and the zone's textured
    /// MMB placements stream in over many frames *after* the collision
    /// geometry lands (which is what triggered the request). `waited`
    /// counts frames since the request; [`spawn_bake_camera`] holds here
    /// until the visual load has drained — see [`BAKE_MIN_WARMUP_FRAMES`].
    Requested { waited: u8 },
    /// Bake camera spawned; wait `frames_remaining` ticks for the
    /// render graph to commit, then despawn.
    Awaiting {
        entity: Entity,
        frames_remaining: u8,
    },
}

/// How many frames to keep the bake camera alive after spawn before
/// despawning it. Minimum 1 so the camera survives the
/// Update→PostUpdate→Render path of the same frame.
const BAKE_FRAMES_TO_HOLD: u8 = 1;

/// Minimum frames to hold in [`BakeStage::Requested`] before a bake may
/// fire, *on top of* the "visuals drained" gate.
///
/// The `LoadMmbRequest` events that spawn the zone's textured props are
/// written in the very same call that mutates `MzbCollisionGeometry`
/// (and thus triggers the request), but they only land in
/// [`MmbLoadQueue::pending`] a frame later, when `process_load_mmb_requests`
/// drains them. So for the first frame or two after a request the queue
/// reads *empty* even though a full city's worth of props is inbound —
/// gating on emptiness alone would bake black. This floor guarantees the
/// queue has had time to fill before an empty reading counts as "done",
/// and doubles as a couple of frames' GPU-upload settle for the last
/// batch of textures.
const BAKE_MIN_WARMUP_FRAMES: u8 = 4;

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
    *stage = BakeStage::Requested { waited: 0 };
}

/// Stage 2 of the bake loop: when stage is `Requested` *and the zone's
/// visuals have finished loading*, compute the zone AABB, allocate a
/// render target, spawn the bake camera, and publish the texture handle
/// + AABB onto [`MinimapState`].
///
/// The bake camera renders the live scene — the textured MMB props, not
/// the collision soup. Those props stream in over many frames after the
/// collision geometry lands, which is what fired the request. Snapshot
/// too early and the camera captures black, untextured geometry. So we
/// hold in `Requested` until [`MmbLoadQueue`] and [`LoadMzbInFlight`]
/// have both drained (plus a [`BAKE_MIN_WARMUP_FRAMES`] floor — see that
/// constant for the enqueue-lag subtlety).
///
/// The published handle is live before the GPU has actually rendered
/// into it — that's fine, because the UI's `ImageNode` doesn't sample
/// the texture until the next render frame, by which point the bake
/// camera has run.
pub fn spawn_bake_camera(
    geom: Res<MzbCollisionGeometry>,
    policy: Res<TopdownCullPolicy>,
    scene_state: Res<SceneState>,
    mmb_queue: Res<MmbLoadQueue>,
    mzb_in_flight: Res<LoadMzbInFlight>,
    mut state: ResMut<MinimapState>,
    mut stage: ResMut<BakeStage>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
) {
    let BakeStage::Requested { waited } = *stage else {
        return;
    };
    // The warmup floor is load-bearing, not just settle: right after a
    // request the queue still reads empty because the placement events
    // haven't enqueued yet, so "empty" there means "not filled", not
    // "done". See BAKE_MIN_WARMUP_FRAMES.
    let visuals_pending = !mmb_queue.pending.is_empty() || !mzb_in_flight.tasks.is_empty();
    if waited < BAKE_MIN_WARMUP_FRAMES || visuals_pending {
        *stage = BakeStage::Requested {
            waited: waited.saturating_add(1),
        };
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
            // Geometry only: layer 0 sees the world (props/terrain/lights
            // all default to layer 0) but NOT the gizmo overlays, which
            // live on `WORLD_GIZMO_LAYER`. Without this the bake captured
            // the blue camera-collision wireframes (and any other overlay
            // active at zone-enter) into the static minimap texture.
            bevy::camera::visibility::RenderLayers::layer(0),
            Camera3d::default(),
            Camera {
                // Negative order renders before the main view. The
                // main camera at the default order=0 then has the
                // baked texture available for any subsequent
                // sampling (we don't actually sample it from a 3D
                // shader — the UI does — but earlier is harmless and
                // makes the dependency direction explicit).
                order: -1,
                // Clear to transparent so unrendered margins (zones
                // whose AABB doesn't square-fill the texture) blend
                // cleanly with the UI's BACKGROUND color underneath.
                clear_color: ClearColorConfig::Custom(Color::NONE),
                ..default()
            },
            // bevy 0.18 moved the render target off `Camera` into a
            // standalone `RenderTarget` component spawned beside it.
            RenderTarget::Image(render_target.clone().into()),
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
            Transform::from_xyz(center_x, ceiling_y, center_z)
                .looking_at(Vec3::new(center_x, ceiling_y - 1.0, center_z), Vec3::NEG_Z),
        ))
        .id();

    state.zone_id = scene_state.snapshot.zone_id;
    state.topdown_image = Some(render_target);
    state.aabb = Some(aabb);

    *stage = BakeStage::Awaiting {
        entity: camera_entity,
        frames_remaining: BAKE_FRAMES_TO_HOLD,
    };
}

/// Stage 3 of the bake loop: count down the per-frame budget on
/// `BakeStage::Awaiting`. Despawn only when the counter hits zero
/// — that guarantees the camera lived through at least one full
/// Update → Render cycle (see the [`BakeStage`] docs for why the
/// chained spawn+despawn-same-frame path would otherwise destroy
/// the entity before any render pass ran).
pub fn despawn_bake_camera(mut stage: ResMut<BakeStage>, mut commands: Commands) {
    let BakeStage::Awaiting {
        entity,
        frames_remaining,
    } = *stage
    else {
        return;
    };
    if frames_remaining > 0 {
        *stage = BakeStage::Awaiting {
            entity,
            frames_remaining: frames_remaining - 1,
        };
        return;
    }
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
/// Uses `RenderAssetUsages::default()` (MAIN_WORLD | RENDER_WORLD)
/// so the CPU side keeps the texture descriptor — the zoom-cropper
/// (`update_minimap_crop_rect`) reads `Image::size_f32()` from main
/// world to convert visible-AABB to pixel-space rect bounds. The
/// RENDER_WORLD-only variant would free the CPU side after first
/// prep, causing `images.get(handle)` to return `None` and the
/// cropper to silently bail every frame — the symptom is the image
/// not moving while entity dots do.
///
/// CPU-side memory cost is ~256 KB per loaded zone (256² RGBA8).
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
        RenderAssetUsages::default(),
    );
    // RENDER_ATTACHMENT is what makes the texture a valid render target;
    // TEXTURE_BINDING is what lets the UI sample it; COPY_DST is the
    // standard companion that Bevy's image plumbing assumes.
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
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
