#![cfg(not(target_arch = "wasm32"))]

use bevy::asset::embedded_asset;
use bevy::ecs::lifecycle::Remove;
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
use std::collections::{BTreeSet, HashMap};

pub const MAX_JOINTS: usize = 128;

/// Point-light slots in `FfxiLightingUniform`, shared by the zone and skinned
/// shaders. Both `zone_ffxi.wgsl` and `skinned_ffxi.wgsl` hard-code this as the
/// array length and loop bound; `point_light_slots_match_shader` guards the
/// mirror. The active count (how many slots the nearest-N pickers fill) is the
/// runtime `GraphicsSettings::model_light_count`, capped here; empty slots
/// carry range 0 and the shaders skip them.
pub const MAX_POINT_LIGHTS: usize = 16;

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
    pub point_pos: [Vec4; MAX_POINT_LIGHTS],

    pub point_color: [Vec4; MAX_POINT_LIGHTS],

    pub point_atten: [Vec4; MAX_POINT_LIGHTS],

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
            point_pos: [Vec4::ZERO; MAX_POINT_LIGHTS],
            point_color: [Vec4::ZERO; MAX_POINT_LIGHTS],
            point_atten: [Vec4::ZERO; MAX_POINT_LIGHTS],
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

// research/xim SkeletonMeshSection.kt SkeletonMeshSection discardThreshold — skinned meshes alpha-test at 69/255.
pub const SKINNED_ALPHA_DISCARD: f32 = 69.0 / 255.0;

// FFXI half-color convention: 0x80 is the neutral multiplier (research/xim
// ByteColor.half; GLDrawer.kt drawXimSkinned meshColor feeds the mesh t_factor as uEffectColor).
pub const T_FACTOR_NEUTRAL: f32 = 128.0;

pub fn t_factor_tint(t_factor: [u8; 4]) -> Vec4 {
    Vec4::new(
        t_factor[0] as f32 / T_FACTOR_NEUTRAL,
        t_factor[1] as f32 / T_FACTOR_NEUTRAL,
        t_factor[2] as f32 / T_FACTOR_NEUTRAL,
        t_factor[3] as f32 / T_FACTOR_NEUTRAL,
    )
}

/// One per-actor record in the shared `skins` storage buffer (binding 0),
/// indexed by `FfxiInstance::skin_slot`. Mirrored as `FfxiSkin` in both WGSL
/// modules; `storage_structs_match_shader` guards the mirror.
#[derive(Clone, Debug, Default, ShaderType)]
pub struct FfxiSkin {
    pub joints: FfxiJointMatrices,
    pub lighting: FfxiLightingUniform,
}

/// One per-submesh record in the shared `instances` storage buffer (binding 3),
/// indexed per draw via `MeshTag`. `flags.x` = has_texture, `.y` = realistic
/// lighting, `.z` = receive shadows, `.w` = target-strobe highlight; `tint` =
/// per-mesh t_factor modulation.
#[derive(Clone, Debug, ShaderType)]
pub struct FfxiInstance {
    pub flags: Vec4,
    pub tint: Vec4,
    pub skin_slot: u32,
}

impl Default for FfxiInstance {
    fn default() -> Self {
        Self {
            flags: Vec4::new(1.0, 0.0, 0.0, 0.0),
            tint: Vec4::ONE,
            skin_slot: 0,
        }
    }
}

// Initial slot capacities cover the measured populated-Jeuno crowd (~100 PCs x
// ~8 submeshes, 2026-07-31) without a growth realloc; growth doubles and bumps
// `buffer_generation` so stale bind groups are rebuilt.
pub const INITIAL_SKIN_SLOTS: usize = 128;
pub const INITIAL_INSTANCE_SLOTS: usize = 1024;
const SLOT_GROWTH_FACTOR: usize = 2;

/// Actor-root marker carrying the actor's slot in the shared skins array.
/// Freed by observer when the entity despawns.
#[derive(Component, Debug, Clone, Copy)]
pub struct FfxiSkinSlot(pub u32);

/// Submesh-child marker carrying the mesh's slot in the shared instances
/// array (also written as its `MeshTag`). Freed by observer on despawn.
#[derive(Component, Debug, Clone, Copy)]
pub struct FfxiInstanceSlot(pub u32);

/// Main-world slab of every live actor's joints/lighting and every submesh's
/// flags/tint, uploaded to two shared storage buffers with 2 `write_buffer`
/// calls per frame (replacing ~2 per actor). Slots are recycled lowest-first
/// so the uploaded high-water region tracks the live count.
#[derive(Resource)]
pub struct FfxiSkinRegistry {
    skins: Vec<FfxiSkin>,
    instances: Vec<FfxiInstance>,
    free_skins: BTreeSet<u32>,
    free_instances: BTreeSet<u32>,
    skin_high_water: u32,
    instance_high_water: u32,
    buffer_generation: u64,
}

impl Default for FfxiSkinRegistry {
    fn default() -> Self {
        Self {
            skins: vec![FfxiSkin::default(); INITIAL_SKIN_SLOTS],
            instances: vec![FfxiInstance::default(); INITIAL_INSTANCE_SLOTS],
            free_skins: BTreeSet::new(),
            free_instances: BTreeSet::new(),
            skin_high_water: 0,
            instance_high_water: 0,
            buffer_generation: 0,
        }
    }
}

impl FfxiSkinRegistry {
    pub fn alloc_skin(&mut self) -> u32 {
        let slot = match self.free_skins.pop_first() {
            Some(s) => s,
            None => {
                let s = self.skin_high_water;
                if s as usize >= self.skins.len() {
                    let new_len = self.skins.len() * SLOT_GROWTH_FACTOR;
                    self.skins.resize(new_len, FfxiSkin::default());
                    self.buffer_generation += 1;
                }
                self.skin_high_water += 1;
                s
            }
        };
        self.skins[slot as usize] = FfxiSkin::default();
        slot
    }

    pub fn free_skin(&mut self, slot: u32) {
        if slot >= self.skin_high_water || !self.free_skins.insert(slot) {
            return;
        }
        while self.skin_high_water > 0 && self.free_skins.remove(&(self.skin_high_water - 1)) {
            self.skin_high_water -= 1;
        }
    }

    pub fn skin(&self, slot: u32) -> &FfxiSkin {
        &self.skins[slot as usize]
    }

    pub fn skin_mut(&mut self, slot: u32) -> &mut FfxiSkin {
        &mut self.skins[slot as usize]
    }

    pub fn alloc_instance(&mut self, record: FfxiInstance) -> u32 {
        let slot = match self.free_instances.pop_first() {
            Some(s) => s,
            None => {
                let s = self.instance_high_water;
                if s as usize >= self.instances.len() {
                    let new_len = self.instances.len() * SLOT_GROWTH_FACTOR;
                    self.instances.resize(new_len, FfxiInstance::default());
                    self.buffer_generation += 1;
                }
                self.instance_high_water += 1;
                s
            }
        };
        self.instances[slot as usize] = record;
        slot
    }

    pub fn free_instance(&mut self, slot: u32) {
        if slot >= self.instance_high_water || !self.free_instances.insert(slot) {
            return;
        }
        while self.instance_high_water > 0
            && self.free_instances.remove(&(self.instance_high_water - 1))
        {
            self.instance_high_water -= 1;
        }
    }

    pub fn instance_mut(&mut self, slot: u32) -> &mut FfxiInstance {
        &mut self.instances[slot as usize]
    }

    pub fn for_each_instance_mut(&mut self, mut f: impl FnMut(&mut FfxiInstance)) {
        let free = &self.free_instances;
        for (i, inst) in self.instances[..self.instance_high_water as usize]
            .iter_mut()
            .enumerate()
        {
            if !free.contains(&(i as u32)) {
                f(inst);
            }
        }
    }

    pub fn live_skins(&self) -> usize {
        self.skin_high_water as usize - self.free_skins.len()
    }

    pub fn live_instances(&self) -> usize {
        self.instance_high_water as usize - self.free_instances.len()
    }

    pub fn skin_capacity(&self) -> usize {
        self.skins.len()
    }

    pub fn instance_capacity(&self) -> usize {
        self.instances.len()
    }

    pub fn buffer_generation(&self) -> u64 {
        self.buffer_generation
    }

    fn skins_used(&self) -> &[FfxiSkin] {
        &self.skins[..self.skin_high_water as usize]
    }

    fn instances_used(&self) -> &[FfxiInstance] {
        &self.instances[..self.instance_high_water as usize]
    }
}

pub fn free_skin_slot_on_remove(
    trigger: On<Remove, FfxiSkinSlot>,
    q: Query<&FfxiSkinSlot>,
    mut registry: ResMut<FfxiSkinRegistry>,
) {
    if let Ok(slot) = q.get(trigger.event().event_target()) {
        registry.free_skin(slot.0);
    }
}

pub fn free_instance_slot_on_remove(
    trigger: On<Remove, FfxiInstanceSlot>,
    q: Query<&FfxiInstanceSlot>,
    mut registry: ResMut<FfxiSkinRegistry>,
) {
    if let Ok(slot) = q.get(trigger.event().event_target()) {
        registry.free_instance(slot.0);
    }
}

/// One material asset per distinct texture (see `FfxiSkinnedMaterialCache`);
/// all per-actor/per-submesh state lives in `FfxiSkinRegistry` and reaches the
/// shader through the shared storage buffers, so materials never churn.
#[derive(Asset, TypePath, Clone, Debug)]
pub struct FfxiSkinnedMaterial {
    pub base_color_texture: Option<Handle<Image>>,
}

/// Dedupes `FfxiSkinnedMaterial` assets by texture so every submesh drawing the
/// same DAT texture shares one material (one bind group). `None` = the shared
/// untextured material for blank-texture C/CS meshes.
#[derive(Resource, Default)]
pub struct FfxiSkinnedMaterialCache {
    by_texture: HashMap<Option<AssetId<Image>>, Handle<FfxiSkinnedMaterial>>,
}

impl FfxiSkinnedMaterialCache {
    pub fn get_or_create(
        &mut self,
        texture: Option<Handle<Image>>,
        materials: &mut Assets<FfxiSkinnedMaterial>,
    ) -> Handle<FfxiSkinnedMaterial> {
        let key = texture.as_ref().map(Handle::id);
        if let Some(h) = self.by_texture.get(&key) {
            if materials.contains(h) {
                return h.clone();
            }
        }
        let h = materials.add(FfxiSkinnedMaterial {
            base_color_texture: texture,
        });
        self.by_texture.insert(key, h.clone());
        h
    }

    pub fn len(&self) -> usize {
        self.by_texture.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_texture.is_empty()
    }
}

// The cache holds strong handles, so an unreferenced material (and the GpuImage
// its bind group pins) survives until pruned here after despawns.
fn prune_ffxi_material_cache(
    mut removed: RemovedComponents<MeshMaterial3d<FfxiSkinnedMaterial>>,
    q_live: Query<&MeshMaterial3d<FfxiSkinnedMaterial>>,
    mut cache: ResMut<FfxiSkinnedMaterialCache>,
) {
    if removed.is_empty() {
        return;
    }
    removed.clear();
    let live: std::collections::HashSet<AssetId<FfxiSkinnedMaterial>> =
        q_live.iter().map(|m| m.0.id()).collect();
    cache.by_texture.retain(|_, h| live.contains(&h.id()));
}

/// Render-world owner of the two shared storage buffers every
/// `FfxiSkinnedMaterial` bind group references. Rewritten in full each frame
/// by [`upload_ffxi_shared_buffers`] (2 `write_buffer` calls total).
#[derive(Resource, Default)]
pub struct FfxiSharedBuffers {
    skins: Option<Buffer>,
    instances: Option<Buffer>,
    skin_capacity: usize,
    instance_capacity: usize,
    scratch: Vec<u8>,
}

impl FfxiSharedBuffers {
    fn bind_buffers(&self) -> Option<(&Buffer, &Buffer)> {
        Some((self.skins.as_ref()?, self.instances.as_ref()?))
    }
}

impl AsBindGroup for FfxiSkinnedMaterial {
    type Data = ();
    type Param = (
        SRes<FfxiSharedBuffers>,
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
        let (skins, instances) = buffers
            .bind_buffers()
            .ok_or(AsBindGroupError::RetryNextUpdate)?;
        let image = match &self.base_color_texture {
            Some(handle) => images
                .get(handle)
                .ok_or(AsBindGroupError::RetryNextUpdate)?,
            None => &fallback.d2,
        };
        Ok(UnpreparedBindGroup {
            bindings: BindingResources(vec![
                (0, OwnedBindingResource::Buffer(skins.clone())),
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
                (3, OwnedBindingResource::Buffer(instances.clone())),
            ]),
        })
    }

    fn bind_group_layout_entries(
        _render_device: &RenderDevice,
        _force_no_bindless: bool,
    ) -> Vec<BindGroupLayoutEntry> {
        let storage = |binding: u32, min: std::num::NonZeroU64| BindGroupLayoutEntry {
            binding,
            visibility: ShaderStages::VERTEX_FRAGMENT,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: Some(min),
            },
            count: None,
        };
        vec![
            storage(0, FfxiSkin::min_size()),
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
            storage(3, FfxiInstance::min_size()),
        ]
    }
}

impl Material for FfxiSkinnedMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://kuluu_render/skinned_ffxi.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://kuluu_render/skinned_ffxi.wgsl".into()
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
        "embedded://kuluu_render/skinned_ffxi_prepass.wgsl".into()
    }

    fn prepass_fragment_shader() -> ShaderRef {
        "embedded://kuluu_render/skinned_ffxi_prepass.wgsl".into()
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

// wgpu keeps a replaced Buffer alive while any bind group references it, so a
// growth realloc silently freezes animation instead of crashing unless every
// material's bind group is rebuilt against the new buffer — this remark pass
// is that rebuild trigger (Modified -> re-prepare).
fn remark_materials_on_buffer_growth(
    registry: Res<FfxiSkinRegistry>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
    mut last_generation: Local<u64>,
) {
    if *last_generation == registry.buffer_generation() {
        return;
    }
    *last_generation = registry.buffer_generation();
    let ids: Vec<AssetId<FfxiSkinnedMaterial>> = materials.ids().collect();
    for id in ids {
        let _ = materials.get_mut(id);
    }
}

fn upload_ffxi_shared_buffers(
    registry: Extract<Res<FfxiSkinRegistry>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut buffers: ResMut<FfxiSharedBuffers>,
) {
    if buffers.skins.is_none() || buffers.skin_capacity != registry.skin_capacity() {
        buffers.skin_capacity = registry.skin_capacity();
        buffers.skins = Some(device.create_buffer(&BufferDescriptor {
            label: Some("ffxi_shared_skins"),
            size: FfxiSkin::min_size().get() * buffers.skin_capacity as u64,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
    }
    if buffers.instances.is_none() || buffers.instance_capacity != registry.instance_capacity() {
        buffers.instance_capacity = registry.instance_capacity();
        buffers.instances = Some(device.create_buffer(&BufferDescriptor {
            label: Some("ffxi_shared_instances"),
            size: FfxiInstance::min_size().get() * buffers.instance_capacity as u64,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
    }

    let mut scratch = std::mem::take(&mut buffers.scratch);

    let skins = registry.skins_used();
    if !skins.is_empty() {
        scratch.clear();
        let mut sb = encase::StorageBuffer::new(scratch);
        sb.write(skins).expect("encode ffxi shared skins");
        scratch = sb.into_inner();
        queue.write_buffer(buffers.skins.as_ref().unwrap(), 0, &scratch);
    }

    let instances = registry.instances_used();
    if !instances.is_empty() {
        scratch.clear();
        let mut sb = encase::StorageBuffer::new(scratch);
        sb.write(instances).expect("encode ffxi shared instances");
        scratch = sb.into_inner();
        queue.write_buffer(buffers.instances.as_ref().unwrap(), 0, &scratch);
    }

    buffers.scratch = scratch;
}

pub struct FfxiMaterialPlugin;

impl Plugin for FfxiMaterialPlugin {
    fn build(&self, app: &mut App) {
        bevy::shader::load_shader_library!(app, "directional_shadow.wgsl");
        bevy::shader::load_shader_library!(app, "point_shadow.wgsl");
        embedded_asset!(app, "skinned_ffxi.wgsl");
        embedded_asset!(app, "skinned_ffxi_prepass.wgsl");
        app.add_plugins(MaterialPlugin::<FfxiSkinnedMaterial>::default());
        app.init_resource::<FfxiSkinRegistry>();
        app.init_resource::<FfxiSkinnedMaterialCache>();
        app.add_observer(free_skin_slot_on_remove);
        app.add_observer(free_instance_slot_on_remove);
        app.add_systems(
            PostUpdate,
            (remark_materials_on_buffer_growth, prune_ffxi_material_cache),
        );
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<FfxiSharedBuffers>()
                .add_systems(ExtractSchedule, upload_ffxi_shared_buffers);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The uniform's point arrays are an ABI contract: both shaders declare the
    // FfxiLighting mirror, so both must size the arrays at MAX_POINT_LIGHTS (WGSL
    // can't import the Rust const). The skinned shader still loops the custom
    // per-actor feed; the zone shader now lights via Bevy clustered forward, so
    // it declares the arrays (layout) but no longer loops them.
    #[test]
    fn point_light_slots_match_shader() {
        let want_array = format!("array<vec4<f32>, {MAX_POINT_LIGHTS}>");
        for (name, src) in [
            ("skinned_ffxi.wgsl", include_str!("skinned_ffxi.wgsl")),
            ("zone_ffxi.wgsl", include_str!("zone_ffxi.wgsl")),
        ] {
            assert!(
                src.contains(&want_array),
                "{name} must declare point arrays as {want_array} (MAX_POINT_LIGHTS)"
            );
        }
        assert!(
            include_str!("skinned_ffxi.wgsl").contains(&format!("i < {MAX_POINT_LIGHTS}u")),
            "skinned_ffxi.wgsl must loop `i < {MAX_POINT_LIGHTS}u` over the per-actor point slots"
        );
    }

    // A per-actor slot knows its light only by world position, so the shared module
    // must resolve the clusterable id by position match before it can sample the cube
    // map; the skinned loop has to route every slot through it, gated by the same
    // receive flag as the sun.
    #[test]
    fn point_slots_receive_shadows_through_the_shared_module() {
        let skinned = include_str!("skinned_ffxi.wgsl");
        assert!(skinned.contains("#import kuluu_render::point_shadow::point_shadow_factor"));
        assert!(skinned.contains(
            "point_shadow_factor(p, n, skins[si].lighting.point_pos[i].xyz, frag_coord)"
        ));
        assert!(skinned.contains("if (point_shadows) {"));
        assert!(skinned.contains("shadow_scale, receive_shadows, in.clip_position.xy)"));

        let module = include_str!("point_shadow.wgsl");
        assert!(module.contains("#define_import_path kuluu_render::point_shadow"));
        assert!(module.contains("POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) == 0u"));
        assert!(module.contains("distance((*light).position_radius.xyz, light_pos)"));
        assert!(module.contains("return fetch_point_shadow(light_id, vec4<f32>(world_pos, 1.0)"));
    }

    // The storage structs are an ABI contract with both WGSL modules: same
    // struct names, same joint-array length, same read-only storage bindings.
    #[test]
    fn storage_structs_match_shader() {
        let want_joints = format!("array<mat4x4<f32>, {MAX_JOINTS}>");
        for (name, src) in [
            ("skinned_ffxi.wgsl", include_str!("skinned_ffxi.wgsl")),
            (
                "skinned_ffxi_prepass.wgsl",
                include_str!("skinned_ffxi_prepass.wgsl"),
            ),
        ] {
            assert!(
                src.contains(&want_joints),
                "{name} must declare joints as {want_joints} (MAX_JOINTS)"
            );
            assert!(
                src.contains("var<storage, read> skins: array<FfxiSkin>"),
                "{name} must bind the shared skins storage array"
            );
            assert!(
                src.contains("var<storage, read> instances: array<FfxiInstance>"),
                "{name} must bind the shared instances storage array"
            );
            assert!(
                src.contains("mesh_functions::get_tag"),
                "{name} must resolve its instance slot via MeshTag (get_tag)"
            );
        }
    }

    // min_size feeds both the bind-group layout validation and the GPU buffer
    // stride; a layout drift here would misindex every actor on the GPU.
    #[test]
    fn shared_buffer_layouts_are_stable() {
        assert_eq!(FfxiLightingUniform::min_size().get(), 864);
        assert_eq!(
            FfxiJointMatrices::min_size().get(),
            (MAX_JOINTS * 64) as u64
        );
        assert_eq!(FfxiSkin::min_size().get(), 9056);
        assert_eq!(FfxiInstance::min_size().get(), 48);
    }

    #[test]
    fn t_factor_half_color_is_neutral() {
        assert_eq!(t_factor_tint([0x80, 0x80, 0x80, 0x80]), Vec4::ONE);
        assert_eq!(
            t_factor_tint([0x00, 0x40, 0x80, 0xFF]),
            Vec4::new(0.0, 0.5, 1.0, 255.0 / T_FACTOR_NEUTRAL)
        );
    }

    #[test]
    fn slot_allocator_reuses_lowest_and_tracks_high_water() {
        let mut reg = FfxiSkinRegistry::default();
        let a = reg.alloc_skin();
        let b = reg.alloc_skin();
        let c = reg.alloc_skin();
        assert_eq!((a, b, c), (0, 1, 2));
        assert_eq!(reg.live_skins(), 3);

        reg.free_skin(a);
        reg.free_skin(b);
        assert_eq!(reg.live_skins(), 1);
        assert_eq!(reg.alloc_skin(), a, "lowest freed slot is reused first");
        assert_eq!(reg.alloc_skin(), b);

        reg.free_skin(c);
        reg.free_skin(b);
        reg.free_skin(a);
        assert_eq!(reg.live_skins(), 0);
        assert_eq!(
            reg.skin_high_water, 0,
            "trailing frees shrink the uploaded high-water region"
        );

        reg.free_skin(a);
        assert_eq!(reg.live_skins(), 0, "double free is a no-op");
    }

    #[test]
    fn growth_bumps_buffer_generation() {
        let mut reg = FfxiSkinRegistry::default();
        let gen0 = reg.buffer_generation();
        for _ in 0..INITIAL_SKIN_SLOTS {
            reg.alloc_skin();
        }
        assert_eq!(reg.buffer_generation(), gen0);
        reg.alloc_skin();
        assert_eq!(reg.buffer_generation(), gen0 + 1);
        assert_eq!(reg.skin_capacity(), INITIAL_SKIN_SLOTS * SLOT_GROWTH_FACTOR);

        for _ in 0..INITIAL_INSTANCE_SLOTS + 1 {
            reg.alloc_instance(FfxiInstance::default());
        }
        assert_eq!(reg.buffer_generation(), gen0 + 2);
    }

    #[test]
    fn material_cache_dedupes_by_texture() {
        let mut materials = Assets::<FfxiSkinnedMaterial>::default();
        let mut images = Assets::<Image>::default();
        let mut cache = FfxiSkinnedMaterialCache::default();
        let tex_a = images.add(Image::default());
        let tex_b = images.add(Image::default());

        let a1 = cache.get_or_create(Some(tex_a.clone()), &mut materials);
        let a2 = cache.get_or_create(Some(tex_a), &mut materials);
        let b = cache.get_or_create(Some(tex_b), &mut materials);
        let untextured1 = cache.get_or_create(None, &mut materials);
        let untextured2 = cache.get_or_create(None, &mut materials);

        assert_eq!(a1, a2, "same texture must share one material");
        assert_ne!(a1, b, "distinct textures get distinct materials");
        assert_eq!(untextured1, untextured2, "one shared untextured material");
        assert_eq!(cache.len(), 3);
        assert_eq!(materials.len(), 3);
    }

    #[test]
    fn slots_are_freed_on_despawn() {
        let mut app = App::new();
        app.init_resource::<FfxiSkinRegistry>();
        app.add_observer(free_skin_slot_on_remove);
        app.add_observer(free_instance_slot_on_remove);

        let (skin, inst) = {
            let mut reg = app.world_mut().resource_mut::<FfxiSkinRegistry>();
            (
                reg.alloc_skin(),
                reg.alloc_instance(FfxiInstance::default()),
            )
        };
        let e = app
            .world_mut()
            .spawn((FfxiSkinSlot(skin), FfxiInstanceSlot(inst)))
            .id();
        {
            let reg = app.world().resource::<FfxiSkinRegistry>();
            assert_eq!((reg.live_skins(), reg.live_instances()), (1, 1));
        }

        app.world_mut().entity_mut(e).despawn();

        let mut reg = app.world_mut().resource_mut::<FfxiSkinRegistry>();
        assert_eq!(
            (reg.live_skins(), reg.live_instances()),
            (0, 0),
            "despawn must return both slots to the registry"
        );
        assert_eq!(reg.alloc_skin(), skin, "freed slot is reusable");
    }
}
