#![cfg(not(target_arch = "wasm32"))]

use bevy::asset::embedded_asset;
use bevy::mesh::{Mesh, MeshVertexBufferLayoutRef};
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;

use crate::skinned_ffxi_material::{FfxiLightingUniform, FfxiMaterialFlags};

// Cranelift's dev codegen backend can't lower the Vec4 horizontal-max NEON
// intrinsic (`fmaxnmv.f32.v4f32`) that `Vec4::max_element()` emits, so reduce
// component-wise with scalar `f32::max` (cranelift issue #171).
fn vec4_max_element(v: Vec4) -> f32 {
    v.x.max(v.y).max(v.z).max(v.w)
}

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct FfxiZoneMaterial {
    #[uniform(0)]
    pub lighting: FfxiLightingUniform,
    #[texture(1)]
    #[sampler(2)]
    pub base_color_texture: Option<Handle<Image>>,
    #[uniform(3)]
    pub material_flags: FfxiMaterialFlags,

    pub alpha_mode: AlphaMode,
}

impl Material for FfxiZoneMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://ffxi_viewer_core/zone_ffxi.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://ffxi_viewer_core/zone_ffxi.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }

    fn enable_prepass() -> bool {
        true
    }

    fn enable_shadows() -> bool {
        true
    }

    fn prepass_vertex_shader() -> ShaderRef {
        "embedded://ffxi_viewer_core/zone_ffxi_prepass.wgsl".into()
    }

    fn prepass_fragment_shader() -> ShaderRef {
        "embedded://ffxi_viewer_core/zone_ffxi_prepass.wgsl".into()
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let vertex_layout = layout.0.get_layout(&[
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            Mesh::ATTRIBUTE_NORMAL.at_shader_location(1),
            Mesh::ATTRIBUTE_UV_0.at_shader_location(2),
            Mesh::ATTRIBUTE_COLOR.at_shader_location(3),
        ])?;
        descriptor.vertex.buffers = vec![vertex_layout];
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

const LIGHTING_EPSILON: f32 = 1.5e-3;

fn update_zone_material_lighting(
    ambient: Res<GlobalAmbientLight>,
    q_sun: Query<
        (&DirectionalLight, &GlobalTransform),
        (
            With<crate::sun_moon::IsSun>,
            Without<crate::sun_moon::IsMoon>,
        ),
    >,
    q_moon: Query<
        (&DirectionalLight, &GlobalTransform),
        (
            With<crate::sun_moon::IsMoon>,
            Without<crate::sun_moon::IsSun>,
        ),
    >,
    mut materials: ResMut<Assets<FfxiZoneMaterial>>,
    mut last: Local<Option<(usize, [Vec4; 5])>>,
) {
    if materials.is_empty() {
        return;
    }

    const AMBIENT_REF_LUX: f32 = 1000.0;
    const DIR_REF_LUX: f32 = 12000.0;
    const COLOR_BIAS: Vec3 = Vec3::new(1.4, 1.36, 1.45);
    const AMBIENT_BIAS_BELOW: f32 = 0.5;

    const AMBIENT_FLOOR: f32 = 0.28;

    let amb = ambient.color.to_linear();
    let amb_k = (ambient.brightness / AMBIENT_REF_LUX).clamp(0.0, 1.5);
    let mut amb_rgb = Vec3::new(amb.red, amb.green, amb.blue) * amb_k;
    if amb_rgb.max_element() < AMBIENT_BIAS_BELOW {
        amb_rgb *= COLOR_BIAS;
    }
    amb_rgb = amb_rgb.max(Vec3::splat(AMBIENT_FLOOR));
    let ambient_v = amb_rgb.extend(1.0);

    let extract = |opt: Option<(&DirectionalLight, &GlobalTransform)>| -> (Vec4, Vec4) {
        match opt {
            Some((dl, gt)) if dl.illuminance > 0.0 => {
                let f = gt.forward();
                let c = dl.color.to_linear();
                let k = (dl.illuminance / DIR_REF_LUX).clamp(0.0, 1.0);
                (
                    Vec4::new(f.x, f.y, f.z, 0.0),
                    Vec4::new(c.red, c.green, c.blue, k),
                )
            }
            _ => (Vec4::ZERO, Vec4::ZERO),
        }
    };
    let (dir0_dir, dir0_color) = extract(q_sun.single().ok());
    let (dir1_dir, dir1_color) = extract(q_moon.single().ok());

    let next = [ambient_v, dir0_dir, dir0_color, dir1_dir, dir1_color];
    let count = materials.len();

    // Every zone submesh is its own FfxiZoneMaterial (hundreds per zone) and
    // Assets::iter_mut() flags every one Modified, so the render world rebuilds
    // all their bind groups that frame. The Vana'diel sun creeps ~1e-4 rad/frame,
    // so skip the push until the lighting actually shifts past a perceptual
    // epsilon (or a chunk streams in, changing the count) to keep that O(materials)
    // upload off every frame.
    if let Some((prev_count, prev)) = *last {
        let unchanged = prev_count == count
            && next
                .iter()
                .zip(prev.iter())
                .all(|(a, b)| vec4_max_element((*a - *b).abs()) <= LIGHTING_EPSILON);
        if unchanged {
            return;
        }
    }

    for (_, m) in materials.iter_mut() {
        m.lighting.ambient = ambient_v;
        m.lighting.dir0_dir = dir0_dir;
        m.lighting.dir0_color = dir0_color;
        m.lighting.dir1_dir = dir1_dir;
        m.lighting.dir1_color = dir1_color;
    }
    *last = Some((count, next));
}

const POINT_FEED_EPSILON: f32 = 1.0e-3;

// Feeds the shader's four point-light slots (the "later per-zone feed"
// zone_ffxi.wgsl anticipates) GLOBALLY: the four lights nearest the viewer go to
// every zone material identically. Per-submesh selection is impossible here
// because instanced MMB placements SHARE one cached FfxiZoneMaterial handle
// (dat_mmb.rs keys it by file_id/chunk_idx/sub_index) — writing position-
// dependent data into a shared material makes co-located submeshes fight every
// frame and flicker as streaming overlays reshuffle query order. A single global
// set sidesteps that, and the range cutoff in nearest_point_light_arrays keeps
// far geometry dark. Change-gated so the O(materials) upload only fires when the
// chosen set actually shifts (the viewer crossing a light boundary), not on the
// flame flicker (which is steady here — see animate_zone_lights).
fn update_zone_material_point_lights(
    active: Res<crate::zone_point_lights::ActiveSceneLights>,
    q_self: Query<&GlobalTransform, With<crate::components::IsSelf>>,
    q_cam: Query<&GlobalTransform, With<Camera3d>>,
    mut materials: ResMut<Assets<FfxiZoneMaterial>>,
    mut last: Local<Option<(usize, [Vec4; 4], [Vec4; 4], [Vec4; 4])>>,
    mut selected: Local<Vec<Vec3>>,
) {
    if materials.is_empty() {
        return;
    }
    let Some(focus) = q_self
        .iter()
        .next()
        .or_else(|| q_cam.iter().next())
        .map(|t| t.translation())
    else {
        return;
    };

    let (point_pos, point_color, point_atten) =
        crate::zone_point_lights::sticky_nearest_point_light_arrays(
            focus,
            &active.lights,
            &mut selected,
        );

    let count = materials.len();
    let close = |a: &[Vec4; 4], b: &[Vec4; 4]| {
        a.iter()
            .zip(b)
            .all(|(x, y)| vec4_max_element((*x - *y).abs()) <= POINT_FEED_EPSILON)
    };
    if let Some((pc, pp, pcol, pat)) = last.as_ref() {
        if *pc == count
            && close(pp, &point_pos)
            && close(pcol, &point_color)
            && close(pat, &point_atten)
        {
            return;
        }
    }

    for (_, m) in materials.iter_mut() {
        m.lighting.point_pos = point_pos;
        m.lighting.point_color = point_color;
        m.lighting.point_atten = point_atten;
    }
    *last = Some((count, point_pos, point_color, point_atten));
}

pub struct FfxiZoneMaterialPlugin;

impl Plugin for FfxiZoneMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "zone_ffxi.wgsl");
        embedded_asset!(app, "zone_ffxi_prepass.wgsl");
        app.add_plugins(MaterialPlugin::<FfxiZoneMaterial>::default())
            .add_systems(Update, update_zone_material_lighting)
            .add_systems(
                Update,
                update_zone_material_point_lights
                    .after(crate::zone_point_lights::build_active_scene_lights),
            );
    }
}
