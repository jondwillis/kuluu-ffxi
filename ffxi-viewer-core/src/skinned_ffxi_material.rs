#![cfg(not(target_arch = "wasm32"))]

use bevy::asset::embedded_asset;
use bevy::ecs::system::lifetimeless::SRes;
use bevy::ecs::system::SystemParamItem;
use bevy::mesh::{Mesh, MeshVertexAttribute, MeshVertexBufferLayoutRef, VertexFormat};
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    encase, AsBindGroup, AsBindGroupError, BindGroupLayout, BindGroupLayoutEntry, BindingResources,
    BindingType, Buffer, BufferBindingType, BufferDescriptor, BufferUsages, OwnedBindingResource,
    RenderPipelineDescriptor, SamplerBindingType, ShaderStages, ShaderType,
    SpecializedMeshPipelineError, TextureSampleType, TextureViewDimension, UnpreparedBindGroup,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::texture::{FallbackImage, GpuImage};
use bevy::render::{Extract, ExtractSchedule, RenderApp};
use bevy::shader::ShaderRef;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

pub const MAX_JOINTS: usize = 128;

const ATTR_ID_BASE: u64 = 0x4646_5849_0000_0000;

pub const ATTR_POSITION0: MeshVertexAttribute =
    MeshVertexAttribute::new("Ffxi_Position0", ATTR_ID_BASE, VertexFormat::Float32x3);

pub const ATTR_POSITION1: MeshVertexAttribute =
    MeshVertexAttribute::new("Ffxi_Position1", ATTR_ID_BASE + 1, VertexFormat::Float32x3);

pub const ATTR_NORMAL0: MeshVertexAttribute =
    MeshVertexAttribute::new("Ffxi_Normal0", ATTR_ID_BASE + 2, VertexFormat::Float32x3);

pub const ATTR_NORMAL1: MeshVertexAttribute =
    MeshVertexAttribute::new("Ffxi_Normal1", ATTR_ID_BASE + 3, VertexFormat::Float32x3);

pub const ATTR_JOINT_WEIGHT: MeshVertexAttribute =
    MeshVertexAttribute::new("Ffxi_JointWeight", ATTR_ID_BASE + 4, VertexFormat::Float32);

pub const ATTR_JOINT0: MeshVertexAttribute =
    MeshVertexAttribute::new("Ffxi_Joint0", ATTR_ID_BASE + 5, VertexFormat::Uint32);

pub const ATTR_JOINT1: MeshVertexAttribute =
    MeshVertexAttribute::new("Ffxi_Joint1", ATTR_ID_BASE + 6, VertexFormat::Uint32);

pub const ATTR_COLOR: MeshVertexAttribute =
    MeshVertexAttribute::new("Ffxi_Color", ATTR_ID_BASE + 7, VertexFormat::Float32x4);

#[derive(Clone, Debug, ShaderType)]
pub struct FfxiLightingUniform {
    pub ambient: Vec4,
    pub dir0_dir: Vec4,
    pub dir0_color: Vec4,
    pub dir1_dir: Vec4,
    pub dir1_color: Vec4,
    pub point_pos: [Vec4; 4],

    pub point_color: [Vec4; 4],

    pub point_atten: [Vec4; 4],

    /// Shared per-frame animation parameters, written once per frame into the
    /// single persistent lighting buffer (see `ZoneGlobalLighting`):
    /// - `x` = elapsed time in seconds (uv scroll, wind phase)
    /// - `y` = global wind strength scalar (foliage vertex blend, Phase C)
    /// - `z`, `w` = reserved
    pub time_params: Vec4,
}

impl Default for FfxiLightingUniform {
    fn default() -> Self {
        Self {
            ambient: Vec4::new(0.5, 0.5, 0.5, 1.0),
            dir0_dir: Vec4::new(0.0, -1.0, 0.0, 0.0),
            dir0_color: Vec4::new(0.6, 0.6, 0.6, 1.0),
            dir1_dir: Vec4::ZERO,
            dir1_color: Vec4::ZERO,
            point_pos: [Vec4::ZERO; 4],
            point_color: [Vec4::ZERO; 4],
            point_atten: [Vec4::ZERO; 4],
            time_params: Vec4::ZERO,
        }
    }
}

#[derive(Clone, Debug, ShaderType)]
pub struct FfxiJointMatrices {
    pub matrices: [Mat4; MAX_JOINTS],
}

impl Default for FfxiJointMatrices {
    fn default() -> Self {
        Self {
            matrices: [Mat4::IDENTITY; MAX_JOINTS],
        }
    }
}

impl FfxiJointMatrices {
    pub fn set_from(&mut self, pose: &[Mat4]) {
        let n = pose.len().min(MAX_JOINTS);
        self.matrices[..n].copy_from_slice(&pose[..n]);
    }
}

#[derive(Clone, Debug, ShaderType)]
pub struct FfxiMaterialFlags {
    pub flags: Vec4,
}

impl Default for FfxiMaterialFlags {
    fn default() -> Self {
        Self {
            flags: Vec4::new(1.0, 0.0, 0.0, 0.0),
        }
    }
}

// research/xim SkeletonMeshSection.kt:61 — skinned meshes alpha-test at 69/255.
pub const SKINNED_ALPHA_DISCARD: f32 = 69.0 / 255.0;

// FFXI half-color convention: 0x80 is the neutral multiplier (research/xim
// ByteColor.half; GLDrawer.kt:329-331 feeds the mesh t_factor as uEffectColor).
pub const T_FACTOR_NEUTRAL: f32 = 128.0;

pub fn t_factor_tint(t_factor: [u8; 4]) -> Vec4 {
    Vec4::new(
        t_factor[0] as f32 / T_FACTOR_NEUTRAL,
        t_factor[1] as f32 / T_FACTOR_NEUTRAL,
        t_factor[2] as f32 / T_FACTOR_NEUTRAL,
        t_factor[3] as f32 / T_FACTOR_NEUTRAL,
    )
}

#[derive(Clone, Debug, PartialEq, ShaderType)]
pub struct FfxiSkinnedFlags {
    pub flags: Vec4,
    // Per-mesh t_factor modulation color (skel_mesh RenderProperties), neutral = 1.0.
    pub tint: Vec4,
}

impl Default for FfxiSkinnedFlags {
    fn default() -> Self {
        Self {
            flags: Vec4::new(1.0, 0.0, 0.0, 0.0),
            tint: Vec4::ONE,
        }
    }
}

static NEXT_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Asset, TypePath, Clone, Debug)]
pub struct FfxiSkinnedMaterial {
    pub lighting: FfxiLightingUniform,
    pub base_color_texture: Option<Handle<Image>>,
    pub joints: FfxiJointMatrices,
    pub material_flags: FfxiSkinnedFlags,
    // All of one actor's sub-mesh materials share this id (the actor root entity
    // bits). joints + lighting are identical across them, so they share one set of
    // persistent GPU buffers uploaded ONCE per actor per frame.
    pub skin_id: u64,
    // Unique per material; keys its per-submesh flags buffer (has_texture differs).
    // Per-frame data lives in persistent buffers refreshed via write_buffer, so
    // mutating these fields never marks the asset Modified and the bind group is
    // built once instead of recreated every frame.
    pub instance_id: u64,
}

impl FfxiSkinnedMaterial {
    pub fn new(
        skin_id: u64,
        lighting: FfxiLightingUniform,
        base_color_texture: Option<Handle<Image>>,
        joints: FfxiJointMatrices,
        material_flags: FfxiSkinnedFlags,
    ) -> Self {
        Self {
            lighting,
            base_color_texture,
            joints,
            material_flags,
            skin_id,
            instance_id: NEXT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
}

struct SkinBuffers {
    joints: Buffer,
    lighting: Buffer,
}

struct FlagsBuffer {
    buffer: Buffer,
    last: FfxiSkinnedFlags,
}

/// Render-world owner of the persistent material uniform buffers, written every
/// frame by [`upload_ffxi_material_buffers`] and referenced by bind groups built
/// once. `skin` (joints binding 3 + lighting binding 0) is shared per actor and
/// uploaded once each; `flags` (binding 4) is per material (has_texture differs).
#[derive(Resource, Default)]
pub struct FfxiMaterialBuffers {
    skin: HashMap<u64, SkinBuffers>,
    flags: HashMap<u64, FlagsBuffer>,
}

impl AsBindGroup for FfxiSkinnedMaterial {
    type Data = ();
    type Param = (
        SRes<FfxiMaterialBuffers>,
        SRes<RenderAssets<GpuImage>>,
        SRes<FallbackImage>,
    );

    fn label() -> &'static str {
        "ffxi_skinned_material"
    }

    fn bind_group_data(&self) -> Self::Data {}

    fn unprepared_bind_group(
        &self,
        _layout: &BindGroupLayout,
        _render_device: &RenderDevice,
        param: &mut SystemParamItem<'_, '_, Self::Param>,
        _force_no_bindless: bool,
    ) -> Result<UnpreparedBindGroup, AsBindGroupError> {
        let (buffers, images, fallback) = param;
        let skin = buffers
            .skin
            .get(&self.skin_id)
            .ok_or(AsBindGroupError::RetryNextUpdate)?;
        let flags = buffers
            .flags
            .get(&self.instance_id)
            .ok_or(AsBindGroupError::RetryNextUpdate)?;
        let image = match &self.base_color_texture {
            Some(handle) => images
                .get(handle)
                .ok_or(AsBindGroupError::RetryNextUpdate)?,
            None => &fallback.d2,
        };
        Ok(UnpreparedBindGroup {
            bindings: BindingResources(vec![
                (0, OwnedBindingResource::Buffer(skin.lighting.clone())),
                (
                    1,
                    OwnedBindingResource::TextureView(
                        TextureViewDimension::D2,
                        image.texture_view.clone(),
                    ),
                ),
                (
                    2,
                    OwnedBindingResource::Sampler(
                        SamplerBindingType::Filtering,
                        image.sampler.clone(),
                    ),
                ),
                (3, OwnedBindingResource::Buffer(skin.joints.clone())),
                (4, OwnedBindingResource::Buffer(flags.buffer.clone())),
            ]),
        })
    }

    fn bind_group_layout_entries(
        _render_device: &RenderDevice,
        _force_no_bindless: bool,
    ) -> Vec<BindGroupLayoutEntry> {
        let uniform = |binding: u32, min: std::num::NonZeroU64| BindGroupLayoutEntry {
            binding,
            visibility: ShaderStages::VERTEX_FRAGMENT,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: Some(min),
            },
            count: None,
        };
        vec![
            uniform(0, FfxiLightingUniform::min_size()),
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::VERTEX_FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 2,
                visibility: ShaderStages::VERTEX_FRAGMENT,
                ty: BindingType::Sampler(SamplerBindingType::Filtering),
                count: None,
            },
            uniform(3, FfxiJointMatrices::min_size()),
            uniform(4, FfxiSkinnedFlags::min_size()),
        ]
    }
}

impl Material for FfxiSkinnedMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://ffxi_viewer_core/skinned_ffxi.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://ffxi_viewer_core/skinned_ffxi.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Mask(SKINNED_ALPHA_DISCARD)
    }

    fn enable_prepass() -> bool {
        true
    }

    fn enable_shadows() -> bool {
        true
    }

    fn prepass_vertex_shader() -> ShaderRef {
        "embedded://ffxi_viewer_core/skinned_ffxi_prepass.wgsl".into()
    }

    fn prepass_fragment_shader() -> ShaderRef {
        "embedded://ffxi_viewer_core/skinned_ffxi_prepass.wgsl".into()
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let vertex_layout = layout.0.get_layout(&[
            ATTR_POSITION0.at_shader_location(0),
            ATTR_POSITION1.at_shader_location(1),
            ATTR_NORMAL0.at_shader_location(2),
            ATTR_NORMAL1.at_shader_location(3),
            Mesh::ATTRIBUTE_UV_0.at_shader_location(4),
            ATTR_JOINT_WEIGHT.at_shader_location(5),
            ATTR_JOINT0.at_shader_location(6),
            ATTR_JOINT1.at_shader_location(7),
            ATTR_COLOR.at_shader_location(8),
        ])?;
        descriptor.vertex.buffers = vec![vertex_layout];

        descriptor.primitive.cull_mode = None;

        Ok(())
    }
}

pub(crate) fn write_uniform<T: ShaderType + encase::internal::WriteInto>(
    queue: &RenderQueue,
    buffer: &Buffer,
    value: &T,
) {
    let mut data = encase::UniformBuffer::new(Vec::<u8>::new());
    data.write(value).expect("encode ffxi material uniform");
    queue.write_buffer(buffer, 0, &data.into_inner());
}

/// Refreshes every material's persistent uniform buffers from the CPU-side asset
/// fields each frame (write_buffer, not realloc), so the material asset is never
/// marked Modified by animation/lighting and its bind group is built only once.
fn upload_ffxi_material_buffers(
    materials: Extract<Res<Assets<FfxiSkinnedMaterial>>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut cache: ResMut<FfxiMaterialBuffers>,
) {
    let uniform_buffer = |label: &'static str, size: std::num::NonZeroU64| {
        device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: size.get(),
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };

    let mut live_skins: HashSet<u64> = HashSet::new();
    let mut uploaded_skins: HashSet<u64> = HashSet::new();
    let mut live_flags: HashSet<u64> = HashSet::with_capacity(materials.len());
    for (_id, mat) in materials.iter() {
        let skin = cache
            .skin
            .entry(mat.skin_id)
            .or_insert_with(|| SkinBuffers {
                joints: uniform_buffer("ffxi_skin_joints", FfxiJointMatrices::min_size()),
                lighting: uniform_buffer("ffxi_skin_lighting", FfxiLightingUniform::min_size()),
            });
        live_skins.insert(mat.skin_id);
        // joints + lighting are identical across an actor's sub-meshes, so upload
        // the shared buffers once per actor per frame, not once per material.
        if uploaded_skins.insert(mat.skin_id) {
            write_uniform(&queue, &skin.joints, &mat.joints);
            write_uniform(&queue, &skin.lighting, &mat.lighting);
        }

        // material_flags only changes when a graphics setting is toggled, so write
        // it on first sight and on change, not every frame — this is the bulk of the
        // per-frame write_buffer/staging churn the actor count otherwise multiplies.
        match cache.flags.entry(mat.instance_id) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                let fb = e.get_mut();
                if fb.last != mat.material_flags {
                    write_uniform(&queue, &fb.buffer, &mat.material_flags);
                    fb.last = mat.material_flags.clone();
                }
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                let buffer = uniform_buffer("ffxi_mat_flags", FfxiSkinnedFlags::min_size());
                write_uniform(&queue, &buffer, &mat.material_flags);
                e.insert(FlagsBuffer {
                    buffer,
                    last: mat.material_flags.clone(),
                });
            }
        }
        live_flags.insert(mat.instance_id);
    }
    cache.skin.retain(|id, _| live_skins.contains(id));
    cache.flags.retain(|id, _| live_flags.contains(id));
}

pub struct FfxiMaterialPlugin;

impl Plugin for FfxiMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "skinned_ffxi.wgsl");
        embedded_asset!(app, "skinned_ffxi_prepass.wgsl");
        app.add_plugins(MaterialPlugin::<FfxiSkinnedMaterial>::default());
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<FfxiMaterialBuffers>()
                .add_systems(ExtractSchedule, upload_ffxi_material_buffers);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_factor_half_color_is_neutral() {
        assert_eq!(t_factor_tint([0x80, 0x80, 0x80, 0x80]), Vec4::ONE);
        assert_eq!(
            t_factor_tint([0x00, 0x40, 0x80, 0xFF]),
            Vec4::new(0.0, 0.5, 1.0, 255.0 / T_FACTOR_NEUTRAL)
        );
    }
}
