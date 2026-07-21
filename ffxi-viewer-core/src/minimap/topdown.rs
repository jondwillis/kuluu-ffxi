use bevy::asset::RenderAssetUsages;
use bevy::camera::ScalingMode;
use bevy::camera::{ClearColorConfig, RenderTarget};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};

pub const MINIMAP_BAKE_LAYER: usize = 3;

use crate::components::InGameEntity;
use crate::dat_mzb::{LoadMzbInFlight, MzbCollisionGeometry};
use crate::ffxi_zone_material::ZoneGlobalLighting;
use crate::snapshot::SceneState;

use super::{MinimapAabb, MinimapState, MINIMAP_TEX_SIZE};

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

// Re-bake the lit top-down map when time-of-day lighting crosses one of these
// quantized brightness buckets, so a night-baked interior doesn't stay frozen
// as dawn breaks. Coarse on purpose: a full Vana'diel day is ~57 real minutes,
// so a few buckets re-bake only a handful of times per cycle instead of every
// frame the sun creeps past an epsilon.
const LIGHTING_REBAKE_BUCKETS: f32 = 12.0;

// Only ambient + the two directional (sun/moon) terms drive the bucket; point
// lights (braziers) flicker every frame and would thrash the re-bake trigger.
pub(crate) fn lighting_bucket(l: &crate::skinned_ffxi_material::FfxiLightingUniform) -> u64 {
    let q = |x: f32| (x.clamp(0.0, 4.0) * LIGHTING_REBAKE_BUCKETS).round() as u64;
    let mut sig = 0u64;
    for v in [l.ambient, l.dir0_color, l.dir1_color] {
        for c in [v.x, v.y, v.z, v.w] {
            sig = sig.wrapping_mul(97).wrapping_add(q(c));
        }
    }
    sig
}

#[derive(Resource, Debug, Default)]
pub enum BakeStage {
    #[default]
    Idle,

    Requested {
        waited: u8,
    },

    Awaiting {
        entity: Entity,
        frames_remaining: u8,
    },
}

const BAKE_FRAMES_TO_HOLD: u8 = 2;

const BAKE_MIN_WARMUP_FRAMES: u8 = 4;

#[derive(Component)]
pub struct MinimapBakeCamera;

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

pub fn bake_topdown_on_zone_or_policy_change(
    geom: Res<MzbCollisionGeometry>,
    policy: Res<TopdownCullPolicy>,
    lighting: Res<ZoneGlobalLighting>,
    scene_state: Res<SceneState>,
    state: Res<MinimapState>,
    mut stage: ResMut<BakeStage>,
) {
    if geom.positions.is_empty() {
        return;
    }
    let policy_changed = policy.is_changed();
    let snapshot_file_id = crate::snapshot::effective_zone_file_id(&scene_state.snapshot);
    let zone_changed = snapshot_file_id != state.baked_file_id;

    // A re-bake keeps the lit map current as Vana'diel time shifts, but only
    // once a zone is already baked — the first bake is the zone/geom trigger.
    let bucket = lighting_bucket(&lighting.0);
    let lighting_changed =
        state.baked_file_id.is_some() && state.baked_lighting_bucket != Some(bucket);

    if !geom.is_changed() && !policy_changed && !zone_changed && !lighting_changed {
        return;
    }
    if !matches!(*stage, BakeStage::Idle) {
        return;
    }
    *stage = BakeStage::Requested { waited: 0 };
}

pub fn spawn_bake_camera(
    geom: Res<MzbCollisionGeometry>,
    policy: Res<TopdownCullPolicy>,
    lighting: Res<ZoneGlobalLighting>,
    scene_state: Res<SceneState>,
    mzb_in_flight: Res<LoadMzbInFlight>,
    mut state: ResMut<MinimapState>,
    mut stage: ResMut<BakeStage>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
) {
    let BakeStage::Requested { waited } = *stage else {
        return;
    };

    let collision_pending = !mzb_in_flight.tasks.is_empty();
    if waited < BAKE_MIN_WARMUP_FRAMES || collision_pending {
        *stage = BakeStage::Requested {
            waited: waited.saturating_add(1),
        };
        return;
    }
    let Some(aabb_3d) = compute_world_aabb(&geom.positions) else {
        *stage = BakeStage::Idle;
        return;
    };

    let span_x = (aabb_3d.max_x - aabb_3d.min_x).max(1.0);
    let span_z = (aabb_3d.max_z - aabb_3d.min_z).max(1.0);
    let center_x = 0.5 * (aabb_3d.min_x + aabb_3d.max_x);
    let center_z = 0.5 * (aabb_3d.min_z + aabb_3d.max_z);

    let ceiling_y = aabb_3d.max_y - policy.top_cull_yalms;
    let span_y_below_camera = (ceiling_y - aabb_3d.min_y).max(1.0) + 10.0;

    let render_target = create_render_target_image(&mut images);

    let aabb = MinimapAabb {
        min: Vec2::new(aabb_3d.min_x, aabb_3d.min_z),
        max: Vec2::new(aabb_3d.max_x, aabb_3d.max_z),
    };

    // No synthetic mesh: the offscreen camera renders the real textured static
    // zone geometry, which carries MINIMAP_BAKE_LAYER (see dat_mmb.rs). The
    // ceiling-clamped camera below places the near plane under the roof so roof
    // triangles are clipped and the interior floor/walls read from above.
    let camera_entity = commands
        .spawn((
            InGameEntity,
            MinimapBakeCamera,
            bevy::camera::visibility::RenderLayers::layer(MINIMAP_BAKE_LAYER),
            Camera3d::default(),
            Camera {
                order: -1,

                clear_color: ClearColorConfig::Custom(Color::NONE),
                ..default()
            },
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
            Transform::from_xyz(center_x, ceiling_y, center_z)
                .looking_at(Vec3::new(center_x, ceiling_y - 1.0, center_z), Vec3::NEG_Z),
        ))
        .id();

    state.baked_file_id = crate::snapshot::effective_zone_file_id(&scene_state.snapshot);
    state.baked_lighting_bucket = Some(lighting_bucket(&lighting.0));
    state.topdown_image = Some(render_target);
    state.aabb = Some(aabb);

    *stage = BakeStage::Awaiting {
        entity: camera_entity,
        frames_remaining: BAKE_FRAMES_TO_HOLD,
    };
}

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WorldAabb3 {
    pub min_x: f32,
    pub max_x: f32,
    pub min_y: f32,
    pub max_y: f32,
    pub min_z: f32,
    pub max_z: f32,
}

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

    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
    images.add(image)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn aabb_empty_returns_none() {
        assert!(compute_world_aabb(&[]).is_none());
    }

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
