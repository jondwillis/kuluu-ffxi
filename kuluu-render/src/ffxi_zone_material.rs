#![cfg(not(target_arch = "wasm32"))]

use bevy::asset::embedded_asset;
use bevy::ecs::system::lifetimeless::SRes;
use bevy::ecs::system::SystemParamItem;
use bevy::mesh::{Mesh, MeshVertexBufferLayoutRef};
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin, MeshPipelineKey};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    AsBindGroup, AsBindGroupError, BindGroupBuilder, BindGroupLayout, BindGroupLayoutEntry,
    BindingType, Buffer, BufferBindingType, BufferDescriptor, BufferUsages, DepthBiasState, Face,
    FrontFace, RenderPipelineDescriptor, SamplerBindingType, ShaderStages, ShaderType,
    SpecializedMeshPipelineError, TextureSampleType, TextureViewDimension,
    UnpreparedBindingResource,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::texture::{FallbackImage, GpuImage};
use bevy::render::{Extract, ExtractSchedule, RenderApp};
use bevy::shader::ShaderRef;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::skinned_ffxi_material::{write_uniform, FfxiLightingUniform, FfxiMaterialFlags};

static NEXT_ZONE_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);

/// `FfxiMaterialFlags::flags.z` as `zone_ffxi.wesl` reads it: whether this mesh takes the
/// camera's `DistanceFog`. Retail gates fog per generator on the CMoElem render-state word
/// (research/XIClient CMoElem.cpp CMoElem::PrepDX, bit 0x2000000 -> `D3DRS_FOGENABLE` false), which
/// the weat/ sky canopies mostly set — and must, since they sit thousands of units past every
/// 0x2F fog distance, where fog would replace their colour outright.
pub const ZONE_FLAG_FOGGED: f32 = 0.0;
pub const ZONE_FLAG_UNFOGGED: f32 = 1.0;

pub fn zone_fog_flag(fog_enabled: bool) -> f32 {
    if fog_enabled {
        ZONE_FLAG_FOGGED
    } else {
        ZONE_FLAG_UNFOGGED
    }
}

#[derive(Resource, Clone, Default)]
pub struct ZoneGlobalLighting(
    pub FfxiLightingUniform,
    pub HashMap<u32, FfxiLightingUniform>,
);

/// Pipeline-key half of the MMB/MZB render-state word (ffxi-dat
/// `MmbRenderState`, decoded from the u16 at subrecord offset 18).
///
/// xim references:
/// - ZoneMeshSection.kt parseMesh — blended zone meshes render at
///   `ZBiasLevel.High` (1), opaque at `Normal` (0).
/// - XIClient ZoneRenderer.cpp ZoneRenderer::RenderChunk2 — blended meshes disable depth write and use
///   the integer `TransparentZBias` layer to pull decals over the base terrain.
/// - Bit `0x2000` CLEAR enables back-face culling.
/// - GLDrawer.kt — front face is `CW` (D3D-era winding), flipped to `CCW`
///   when the instance is mirrored (`scale.x * scale.y * scale.z < 0`).
///
/// These flow into `specialize` via `AsBindGroup::Data`, so each distinct
/// combination gets its own specialized render pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct FfxiZoneMaterialKey {
    /// Cull back faces (`Face::Back`). FFXI winding is **clockwise** (D3D
    /// convention, xim GLDrawer.kt drawXim face), not Bevy's CCW default.
    pub back_face_culling: bool,
    /// Placement transform has a negative determinant (mirrored). Flips the
    /// effective winding, so `specialize` flips `front_face` back to CCW.
    /// Zone tiles are routinely placed mirrored in alternating checkerboard
    /// patterns, so this must be per-placement, not per-chunk.
    pub mirrored: bool,
    /// Legacy D3D8 integer Z-bias layer; 0 for opaque and 8 for blended terrain.
    pub z_bias_level: u8,
    /// `false` for blended decals (they must not occlude later layers).
    pub depth_write: bool,
    /// Selects the generator (`CMoD3m`) texture-stage chain over the terrain
    /// (`ZoneRenderer`) one. Retail runs the same MMB vertex data through two
    /// different stage setups: zone placements take a single
    /// `MODULATE2X(TEXTURE, CURRENT)` (ZoneRenderer.cpp ZoneRenderer::ApplyDefaultRenderState), while a mesh
    /// hung off a generator takes `MODULATE2X(DIFFUSE, TEXTURE)` and then
    /// `MODULATE2X(CURRENT, TFACTOR)` (CMoD3m.cpp ZeroOneTSS `NonZeroTwoTSS`, reached
    /// for MMB links via CMoD3mElem.cpp CMoD3mElem::OnDraw `DoMMBDraw`) — twice the gain, and
    /// its TFACTOR is this material's `tint`.
    pub generator_stage_chain: bool,
}

impl FfxiZoneMaterialKey {
    /// Key for meshes that predate render-state plumbing: no culling, no
    /// bias, normal depth write — exactly the old hardcoded pipeline.
    pub const LEGACY: Self = Self {
        back_face_culling: false,
        mirrored: false,
        z_bias_level: 0,
        depth_write: true,
        generator_stage_chain: false,
    };
}

/// Shader def `specialize` pushes for [`FfxiZoneMaterialKey::generator_stage_chain`];
/// `zone_ffxi.wesl` matches on it.
pub const GENERATOR_STAGE_CHAIN_DEF: &str = "FFXI_GENERATOR_STAGE_CHAIN";

const WGPU_FORWARD_DECAL_BIAS: i32 = 1;
const WGPU_FORWARD_DECAL_SLOPE_SCALE: f32 = 1.0;

fn d3d8_z_bias(level: u8) -> DepthBiasState {
    DepthBiasState {
        constant: if level == 0 {
            0
        } else {
            WGPU_FORWARD_DECAL_BIAS
        },
        slope_scale: if level == 0 {
            0.0
        } else {
            WGPU_FORWARD_DECAL_SLOPE_SCALE
        },
        clamp: 0.0,
    }
}

pub type ZoneLightBindings = [Option<ffxi_dat::mzb::LightId>; ffxi_dat::mzb::LIGHT_REFERENCE_COUNT];

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct ZonePointLighting {
    pub positions: [Vec4; ffxi_dat::mzb::LIGHT_REFERENCE_COUNT],
    pub colors: [Vec4; ffxi_dat::mzb::LIGHT_REFERENCE_COUNT],
    pub attenuation: [Vec4; ffxi_dat::mzb::LIGHT_REFERENCE_COUNT],
}

impl ZonePointLighting {
    fn from_authored(
        lights: &[crate::zone_point_lights::ZonePointLight],
        bindings: &ZoneLightBindings,
    ) -> Self {
        const ACTOR_ONLY_PREFIX: u8 = b'c';
        let selected = bindings
            .iter()
            .flatten()
            .filter(|id| id.to_le_bytes()[0] != ACTOR_ONLY_PREFIX)
            .filter_map(|id| lights.iter().find(|light| light.light_id == *id));
        let (positions, colors, attenuation) =
            crate::zone_point_lights::pack_point_light_arrays(selected);
        Self {
            positions: std::array::from_fn(|i| positions[i]),
            colors: std::array::from_fn(|i| colors[i]),
            attenuation: std::array::from_fn(|i| attenuation[i]),
        }
    }
}

#[derive(Asset, TypePath, Clone, Debug)]
pub struct FfxiZoneMaterial {
    pub area_id: u32,
    pub light_bindings: ZoneLightBindings,
    pub point_lighting: ZonePointLighting,
    pub base_color_texture: Option<Handle<Image>>,
    pub material_flags: FfxiMaterialFlags,

    // research/xim ParticleGeneratorParser.kt sec3Handler ToD color: a per-mesh RGB(setter) +
    // alpha(multiplier) the weat/<type>/ ClockValueUpdaters drive over the Vana day. Folded
    // as a final modulate in the fragment shader. White (1,1,1,1) is the no-op default for
    // every other zone mesh — only the cloud/sun layers (zone_clouds.rs) write a live tint.
    pub tint: Vec4,

    // research/xim ParticleUpdaters.kt TextureCoordinateUpdater: animated UV scroll
    // (xy) that drifts the cloud canopy texture for wind. Zero (the default for every
    // other zone mesh) is a no-op.
    pub uv_offset: Vec4,

    pub alpha_mode: AlphaMode,

    /// Overrides where this mesh ranks in the transparent-phase depth sort.
    /// Only the camera-anchored sky layers need it — see
    /// [`crate::skybox::SKY_SORT_DEPTH_CLOUDS`].
    pub sort_depth_bias: f32,

    /// Render-state bits that must specialize the pipeline (cull / bias /
    /// depth write). See [`FfxiZoneMaterialKey`].
    pub render_key: FfxiZoneMaterialKey,

    // Keys this material's persistent flags/tint/uv buffers in ZoneMaterialBuffers.
    // Per-frame data flows through those buffers via write_buffer, so mutating
    // tint/uv (with get_mut_untracked) never marks the asset Modified and the bind
    // group is built once instead of recreated on every lighting/animation step.
    pub instance_id: u64,
}

impl FfxiZoneMaterial {
    pub fn new(
        base_color_texture: Option<Handle<Image>>,
        material_flags: FfxiMaterialFlags,
        tint: Vec4,
        uv_offset: Vec4,
        alpha_mode: AlphaMode,
        render_key: FfxiZoneMaterialKey,
    ) -> Self {
        Self {
            area_id: 0,
            light_bindings: Default::default(),
            point_lighting: Default::default(),
            base_color_texture,
            material_flags,
            tint,
            uv_offset,
            alpha_mode,
            sort_depth_bias: 0.0,
            render_key,
            instance_id: NEXT_ZONE_INSTANCE_ID.fetch_add(1, Ordering::Relaxed),
        }
    }

    pub fn with_area(mut self, area_id: u32) -> Self {
        self.area_id = area_id;
        self
    }

    pub fn with_light_bindings(mut self, bindings: ZoneLightBindings) -> Self {
        self.light_bindings = bindings;
        self
    }

    pub fn with_sort_depth_bias(mut self, bias: f32) -> Self {
        self.sort_depth_bias = bias;
        self
    }
}

struct ZoneInstanceBuffers {
    area_id: u32,
    points: Buffer,
    last_points: ZonePointLighting,
    flags: Buffer,
    tint: Buffer,
    uv: Buffer,
    last_flags: Vec4,
    last_tint: Vec4,
    last_uv: Vec4,
}

/// Zone materials whose instance buffers need (re)writing, gathered per frame
/// in the main world: the tracked asset events plus the untracked tint/uv
/// writers (`scroll_gen_water_uv`, `scroll_water_uv`, `drive_zone_clouds`) that
/// skip `Assets::get_mut` to keep their bind groups. The extract-side upload
/// visits only these instead of every material in the zone.
#[derive(Resource, Default)]
pub struct ZoneMaterialTouched {
    marks: Vec<AssetId<FfxiZoneMaterial>>,
    pending: Vec<AssetId<FfxiZoneMaterial>>,
    dropped: Vec<u64>,
    instance_of: HashMap<AssetId<FfxiZoneMaterial>, u64>,
}

impl ZoneMaterialTouched {
    pub fn mark(&mut self, id: AssetId<FfxiZoneMaterial>) {
        self.marks.push(id);
    }
}

/// Folds this frame's marks and asset events into the list the next extract
/// uploads; runs after `AssetEventSystems` so a material added this frame has
/// its buffers before its bind group is prepared.
fn collect_zone_material_touched(
    mut events: MessageReader<AssetEvent<FfxiZoneMaterial>>,
    materials: Res<Assets<FfxiZoneMaterial>>,
    mut touched: ResMut<ZoneMaterialTouched>,
) {
    let touched = &mut *touched;
    touched.dropped.clear();
    touched.pending.clear();
    touched.pending.append(&mut touched.marks);
    for ev in events.read() {
        match ev {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => {
                let Some(mat) = materials.get(*id) else {
                    continue;
                };
                if let Some(previous) = touched.instance_of.insert(*id, mat.instance_id) {
                    if previous != mat.instance_id {
                        touched.dropped.push(previous);
                    }
                }
                touched.pending.push(*id);
            }
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => {
                if let Some(instance) = touched.instance_of.remove(id) {
                    touched.dropped.push(instance);
                }
            }
            AssetEvent::LoadedWithDependencies { .. } => {}
        }
    }
}

#[derive(Resource)]
pub struct ZoneMaterialBuffers {
    lighting: Buffer,
    areas: HashMap<u32, Buffer>,
    instances: HashMap<u64, ZoneInstanceBuffers>,
}

impl FromWorld for ZoneMaterialBuffers {
    fn from_world(world: &mut World) -> Self {
        let device = world.resource::<RenderDevice>();
        Self {
            lighting: device.create_buffer(&BufferDescriptor {
                label: Some("ffxi_zone_lighting"),
                size: FfxiLightingUniform::min_size().get(),
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            areas: HashMap::new(),
            instances: HashMap::new(),
        }
    }
}

fn upload_zone_material_buffers(
    lighting: Extract<Res<ZoneGlobalLighting>>,
    materials: Extract<Res<Assets<FfxiZoneMaterial>>>,
    touched: Extract<Res<ZoneMaterialTouched>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut cache: ResMut<ZoneMaterialBuffers>,
) {
    write_uniform(&queue, &cache.lighting, &lighting.0);
    for instance in &touched.dropped {
        cache.instances.remove(instance);
    }

    let uniform_buffer = |label: &'static str, size: std::num::NonZeroU64| {
        device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: size.get(),
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };

    for id in &touched.pending {
        let Some(mat) = materials.get(*id) else {
            continue;
        };
        if mat.area_id != 0 {
            cache.areas.entry(mat.area_id).or_insert_with(|| {
                uniform_buffer("ffxi_area_lighting", FfxiLightingUniform::min_size())
            });
        }
        match cache.instances.entry(mat.instance_id) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                let inst = e.get_mut();
                if inst.last_points != mat.point_lighting {
                    write_uniform(&queue, &inst.points, &mat.point_lighting);
                    inst.last_points = mat.point_lighting;
                }
                if inst.last_flags != mat.material_flags.flags {
                    write_uniform(&queue, &inst.flags, &mat.material_flags);
                    inst.last_flags = mat.material_flags.flags;
                }
                if inst.last_tint != mat.tint {
                    write_uniform(&queue, &inst.tint, &mat.tint);
                    inst.last_tint = mat.tint;
                }
                if inst.last_uv != mat.uv_offset {
                    write_uniform(&queue, &inst.uv, &mat.uv_offset);
                    inst.last_uv = mat.uv_offset;
                }
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                let points = uniform_buffer("ffxi_zone_points", ZonePointLighting::min_size());
                write_uniform(&queue, &points, &mat.point_lighting);
                let flags = uniform_buffer("ffxi_zone_flags", FfxiMaterialFlags::min_size());
                let tint = uniform_buffer("ffxi_zone_tint", Vec4::min_size());
                let uv = uniform_buffer("ffxi_zone_uv", Vec4::min_size());
                write_uniform(&queue, &flags, &mat.material_flags);
                write_uniform(&queue, &tint, &mat.tint);
                write_uniform(&queue, &uv, &mat.uv_offset);
                e.insert(ZoneInstanceBuffers {
                    area_id: mat.area_id,
                    points,
                    last_points: mat.point_lighting,
                    flags,
                    tint,
                    uv,
                    last_flags: mat.material_flags.flags,
                    last_tint: mat.tint,
                    last_uv: mat.uv_offset,
                });
            }
        }
    }
    if !touched.dropped.is_empty() {
        let live: std::collections::HashSet<_> = cache
            .instances
            .values()
            .map(|instance| instance.area_id)
            .collect();
        cache.areas.retain(|id, _| live.contains(id));
    }
    for (area, buffer) in &cache.areas {
        let mut value = lighting
            .1
            .get(area)
            .cloned()
            .unwrap_or_else(|| lighting.0.clone());
        value.time_params = lighting.0.time_params;
        write_uniform(&queue, buffer, &value);
    }
}

impl AsBindGroup for FfxiZoneMaterial {
    type Data = FfxiZoneMaterialKey;
    type Param = (
        SRes<ZoneMaterialBuffers>,
        SRes<RenderAssets<GpuImage>>,
        SRes<FallbackImage>,
    );

    fn label() -> &'static str {
        "ffxi_zone_material"
    }

    fn bind_group_data(&self) -> Self::Data {
        self.render_key
    }

    fn build_bind_group(
        &self,
        _layout: &BindGroupLayout,
        _render_device: &RenderDevice,
        param: &mut SystemParamItem<'_, '_, Self::Param>,
        _force_no_bindless: bool,
        output: &mut BindGroupBuilder,
    ) -> Result<(), AsBindGroupError> {
        let (buffers, images, fallback) = param;
        let inst = buffers
            .instances
            .get(&self.instance_id)
            .ok_or(AsBindGroupError::RetryNextUpdate)?;
        let image = match &self.base_color_texture {
            Some(handle) => images
                .get(handle)
                .ok_or(AsBindGroupError::RetryNextUpdate)?,
            None => &fallback.d2,
        };
        output.extend([
            (
                0,
                UnpreparedBindingResource::Buffer(if self.area_id == 0 {
                    buffers.lighting.clone()
                } else {
                    buffers
                        .areas
                        .get(&self.area_id)
                        .ok_or(AsBindGroupError::RetryNextUpdate)?
                        .clone()
                }),
            ),
            (
                1,
                UnpreparedBindingResource::TextureView(
                    TextureViewDimension::D2,
                    image.texture_view.clone(),
                ),
            ),
            (
                2,
                UnpreparedBindingResource::Sampler(
                    SamplerBindingType::Filtering,
                    image.sampler.clone(),
                ),
            ),
            (3, UnpreparedBindingResource::Buffer(inst.flags.clone())),
            (4, UnpreparedBindingResource::Buffer(inst.tint.clone())),
            (5, UnpreparedBindingResource::Buffer(inst.uv.clone())),
            (6, UnpreparedBindingResource::Buffer(inst.points.clone())),
        ]);
        Ok(())
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
            uniform(3, FfxiMaterialFlags::min_size()),
            uniform(4, Vec4::min_size()),
            uniform(5, Vec4::min_size()),
            uniform(6, ZonePointLighting::min_size()),
        ]
    }
}

impl Material for FfxiZoneMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://kuluu_render/zone_ffxi.wesl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://kuluu_render/zone_ffxi.wesl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }

    fn depth_bias(&self) -> f32 {
        self.sort_depth_bias
    }

    fn enable_prepass() -> bool {
        true
    }

    fn enable_shadows() -> bool {
        true
    }

    fn prepass_vertex_shader() -> ShaderRef {
        "embedded://kuluu_render/zone_ffxi_prepass.wesl".into()
    }

    fn prepass_fragment_shader() -> ShaderRef {
        "embedded://kuluu_render/zone_ffxi_prepass.wesl".into()
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let vertex_layout = layout.0.get_layout(&[
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            Mesh::ATTRIBUTE_NORMAL.at_shader_location(1),
            Mesh::ATTRIBUTE_UV_0.at_shader_location(2),
            Mesh::ATTRIBUTE_COLOR.at_shader_location(3),
        ])?;
        descriptor.vertex.buffers = vec![vertex_layout];

        let rk = key.bind_group_data;

        if rk.generator_stage_chain {
            if let Ok(fragment) = descriptor.fragment_mut() {
                fragment.shader_defs.push(GENERATOR_STAGE_CHAIN_DEF.into());
            }
        }

        // xim renders FFXI zone geometry with back-face culling unless the
        // render-state word sets bit 0x2000 (two-sided decals, fences, foliage
        // cards). FFXI winding is CLOCKWISE (D3D convention) — GLDrawer.kt drawXim face
        // sets frontFace(CW), flipping to CCW for mirrored instances
        // (scale.x * scale.y * scale.z < 0). Using Bevy's CCW default here
        // culled every non-mirrored tile: inverted-checkerboard zone geometry.
        // Directional shadow views (UNCLIPPED_DEPTH_ORTHO is set only there —
        // bevy_pbr::render::light::check_views_lights_need_specialization) render single-sided walls
        // unculled: from the sun's viewpoint a wall's one sheet of triangles is
        // back-facing, so Face::Back culling writes no shadow-map depth — walls
        // cast nothing and sunlight leaks indoors (kuluu-lchx).
        let shadow_view = key
            .mesh_key
            .contains(MeshPipelineKey::UNCLIPPED_DEPTH_ORTHO);
        descriptor.primitive.cull_mode = if rk.back_face_culling && !shadow_view {
            Some(Face::Back)
        } else {
            None
        };
        descriptor.primitive.front_face = if rk.mirrored {
            FrontFace::Ccw
        } else {
            FrontFace::Cw
        };

        if let Some(ds) = descriptor.depth_stencil.as_mut() {
            // GLDrawer.kt drawXim — blended decals never write depth. Bevy's
            // transparent pass already disables depth write, but the prepass
            // (enable_prepass = true) would otherwise still write it; AND the
            // flag in rather than overwrite whatever the pass chose.
            ds.depth_write_enabled =
                Some(ds.depth_write_enabled.unwrap_or(false) && rk.depth_write);

            // D3D8 ZBIAS is a driver-defined ordering level, not a portable WGPU
            // depth-unit magnitude. Preserve its forward ordering with the minimum
            // reversed-Z constant and slope terms; applying the raw level pulls
            // decals through neighboring terrain.
            if rk.z_bias_level > 0 {
                ds.bias = d3d8_z_bias(rk.z_bias_level);
            }
        }

        Ok(())
    }
}

fn update_zone_material_lighting(
    ambient: Res<GlobalAmbientLight>,
    // Optional: minimal apps (e.g. the zone-render-headless example) use
    // FfxiZoneMaterialPlugin without the weather plugin, so the resource may
    // not exist. Absent == not valid -> take the GlobalAmbientLight fallback.
    zone_lighting: Option<Res<crate::weather::ZoneDirectionalLighting>>,
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
    mut global: ResMut<ZoneGlobalLighting>,
    weather: Option<Res<crate::weather::ZoneWeather>>,
    clock: Option<Res<crate::vana_time::VanaClock>>,
) {
    const AMBIENT_REF_LUX: f32 = 1000.0;
    const DIR_REF_LUX: f32 = 12000.0;
    const COLOR_BIAS: Vec3 = Vec3::new(1.4, 1.36, 1.45);
    const AMBIENT_BIAS_BELOW: f32 = 0.5;

    // Terrain ambient floor, matched to the actor path so ground and models darken
    // together at night. Both now run the same stage chain — a neutral vertex times
    // this floor, through one MODULATE2X — so the two land on the same value without
    // a per-path correction.
    const AMBIENT_FLOOR: f32 = 0.12;

    // research/xim EnvironmentSection.kt getTerrainLightingParams,168: the 0x2F landscape ambient is
    // the authoritative per-hour base (dark at night). Use it directly when the
    // zone ships records; the GlobalAmbientLight amb_k/COLOR_BIAS path is the
    // no-DAT fallback (it re-derives from the atmosphere seed and inflates).
    let zone_lighting = zone_lighting.filter(|z| z.valid);
    let mut amb_rgb = if let Some(zl) = zone_lighting.as_deref() {
        zl.ambient_landscape
    } else {
        let amb = ambient.color.to_linear();
        let amb_k = (ambient.brightness / AMBIENT_REF_LUX).clamp(0.0, 1.5);
        let mut a = Vec3::new(amb.red, amb.green, amb.blue) * amb_k;
        if a.max_element() < AMBIENT_BIAS_BELOW {
            a *= COLOR_BIAS;
        }
        a
    };
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
    // research/xim EnvironmentSection.kt getLightingParams: zone geometry takes both terrain
    // sun(dir0)+moon(dir1) diffuse lights. The DirectionalLight's `forward` is the
    // -to-celestial direction, so negate the stored to-sun/to-moon vectors to match.
    let (dir0_dir, dir0_color, dir1_dir, dir1_color) =
        if let Some(zone_lighting) = zone_lighting.as_deref() {
            let pack = |to_dir: Vec3, color: Vec3, k: f32| -> (Vec4, Vec4) {
                if k <= 0.0 || to_dir == Vec3::ZERO {
                    return (Vec4::ZERO, Vec4::ZERO);
                }
                let f = (-to_dir).normalize_or_zero();
                (
                    Vec4::new(f.x, f.y, f.z, 0.0),
                    Vec4::new(color.x, color.y, color.z, k.clamp(0.0, 1.0)),
                )
            };
            let (d0d, d0c) = pack(
                zone_lighting.sun_dir,
                zone_lighting.sun_color,
                zone_lighting.sun_k,
            );
            let (d1d, d1c) = pack(
                zone_lighting.moon_dir,
                zone_lighting.moon_color,
                zone_lighting.moon_k,
            );
            (d0d, d0c, d1d, d1c)
        } else {
            let (d0d, d0c) = extract(q_sun.single().ok());
            let (d1d, d1c) = extract(q_moon.single().ok());
            (d0d, d0c, d1d, d1c)
        };

    global.0.ambient = ambient_v;
    global.0.dir0_dir = dir0_dir;
    global.0.dir0_color = dir0_color;
    global.0.dir1_dir = dir1_dir;
    global.0.dir1_color = dir1_color;
    global.1.clear();
    if let (Some(weather), Some(clock), Some(zone)) = (weather, clock, zone_lighting) {
        let sky = crate::sun_moon::vana_sky_from_clock(&clock);
        const MINUTES_PER_HOUR: f32 = 60.0;
        let minutes = (sky.hour * MINUTES_PER_HOUR) as u32;
        for &area in weather.sets.by_area.keys() {
            let Some(record) = weather.sample_for_area(area, minutes) else {
                continue;
            };
            let land = crate::sun_moon::landscape_lighting(
                &record,
                zone.sun_dir,
                zone.moon_dir,
                sky.sun_altitude > 0.0,
                sky.moon_altitude > 0.0,
            );
            let mut uniform = global.0.clone();
            uniform.ambient = land.ambient.extend(1.0);
            uniform.dir0_dir = (-land.sun_dir).normalize_or_zero().extend(0.0);
            uniform.dir0_color = land.sun_color.extend(land.sun_k);
            uniform.dir1_dir = (-land.moon_dir).normalize_or_zero().extend(0.0);
            uniform.dir1_color = land.moon_color.extend(land.moon_k);
            global.1.insert(area, uniform);
        }
    }
}

fn update_zone_point_lighting(
    active: Option<Res<crate::zone_point_lights::ActiveSceneLights>>,
    mut materials: ResMut<Assets<FfxiZoneMaterial>>,
    mut touched: ResMut<ZoneMaterialTouched>,
    mut events: MessageReader<AssetEvent<FfxiZoneMaterial>>,
) {
    let mut added = false;
    for event in events.read() {
        added |= matches!(event, AssetEvent::Added { .. });
    }
    if !added && !active.as_ref().is_some_and(|active| active.is_changed()) {
        return;
    }
    let lights = active
        .as_deref()
        .map(|a| a.lights.as_slice())
        .unwrap_or_default();
    let changes: Vec<_> = materials
        .iter()
        .filter_map(|(id, mat)| {
            let points = ZonePointLighting::from_authored(lights, &mat.light_bindings);
            (points != mat.point_lighting).then_some((id, points))
        })
        .collect();
    for (id, points) in changes {
        if let Some(mat) = materials.get_mut_untracked(id) {
            mat.point_lighting = points;
            touched.mark(id);
        }
    }
}

/// Writes the shared per-frame animation params (`FfxiLightingUniform::
/// time_params`) into the single persistent lighting buffer: `x` = elapsed
/// seconds (uv scroll / wind phase), `y` = global wind strength. Consumers:
/// scrolling water (Phase A), foliage vertex blend (Phase C).
fn update_zone_material_time(
    time: Res<bevy::time::Time>,
    settings: Option<Res<crate::graphics_settings::GraphicsSettings>>,
    mut global: ResMut<ZoneGlobalLighting>,
) {
    global.0.time_params.x = time.elapsed_secs_wrapped();
    global.0.time_params.z = if settings.is_some_and(|s| s.dynamic_lights.point_shadows_enabled()) {
        1.0
    } else {
        0.0
    };
}

pub struct FfxiZoneMaterialPlugin;

impl Plugin for FfxiZoneMaterialPlugin {
    fn build(&self, app: &mut App) {
        bevy::shader::load_shader_library!(app, "directional_shadow.wesl");
        bevy::shader::load_shader_library!(app, "point_shadow.wesl");
        embedded_asset!(app, "zone_ffxi.wesl");
        embedded_asset!(app, "zone_ffxi_prepass.wesl");
        app.add_plugins(MaterialPlugin::<FfxiZoneMaterial>::default())
            .init_resource::<ZoneGlobalLighting>()
            // Idempotent: the full viewer also inits this (lib.rs). Minimal apps
            // (zone-render-headless) add only this plugin, and
            // update_zone_material_lighting reads the resource unconditionally.
            .init_resource::<crate::weather::ZoneDirectionalLighting>()
            .init_resource::<ZoneMaterialTouched>()
            .add_systems(
                Update,
                update_zone_material_lighting.after(crate::sun_moon::sun_moon_system),
            )
            .add_systems(
                Update,
                update_zone_point_lighting
                    .after(crate::zone_point_lights::build_active_scene_lights),
            )
            .add_systems(Update, update_zone_material_time)
            .add_systems(
                PostUpdate,
                collect_zone_material_touched.after(bevy::asset::AssetEventSystems),
            );
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(ExtractSchedule, upload_zone_material_buffers);
        }
    }

    fn finish(&self, app: &mut App) {
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.init_resource::<ZoneMaterialBuffers>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_uniform_contains_only_its_authored_terrain_lights() {
        use crate::zone_point_lights::ZonePointLight;
        let light = ZonePointLight {
            light_id: u32::from_le_bytes(*b"pl00"),
            world_pos: Vec3::ONE,
            color: Vec3::ONE,
            range: 6.0,
            attenuation: 0.125,
            theta_track: None,
            theta_multiplier: 1.0,
        };
        let ids = [Some(light.light_id), None, None, None];
        let packed = ZonePointLighting::from_authored(std::slice::from_ref(&light), &ids);
        assert_eq!(packed.positions[0], Vec3::ONE.extend(0.0));
        assert_eq!(packed.colors[0], Vec3::ONE.extend(6.0));
        assert_eq!(packed.attenuation[0], Vec4::new(0.0, 0.0, 0.125, 0.0));
        assert_eq!(packed.colors[1], Vec4::ZERO);
        assert_eq!(
            ZonePointLighting::from_authored(&[light], &[None; 4]),
            ZonePointLighting::default()
        );
    }

    #[test]
    fn area_buffers_keep_distinct_lighting_when_the_player_crosses() {
        use crate::weather::{sample_zone_weather, ZoneDirectionalLighting, ZoneWeather};
        use ffxi_dat::weather::{WeatherRecord, WeatherSet};
        const AREA: u32 = u32::from_le_bytes(*b"ev01");
        const NOON: f32 = 12.0;
        const ZONE_AMBIENT: Vec3 = Vec3::splat(0.7);
        const AREA_AMBIENT: [f32; 4] = [0.1, 0.2, 0.3, 1.0];
        let mut weather = ZoneWeather::default();
        weather.sets.by_type.insert(
            *b"suny",
            WeatherSet {
                outdoor: vec![WeatherRecord::default()],
                ..default()
            },
        );
        weather.sets.by_area.insert(
            AREA,
            [(
                *b"suny",
                WeatherSet {
                    outdoor: vec![WeatherRecord {
                        ambient_landscape: AREA_AMBIENT,
                        ..default()
                    }],
                    ..default()
                },
            )]
            .into_iter()
            .collect(),
        );
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(weather)
            .init_resource::<crate::weather_fx::CurrentWeather>()
            .insert_resource(crate::vana_time::VanaClock::anchored_at_hour(NOON))
            .init_resource::<GlobalAmbientLight>()
            .insert_resource(ZoneDirectionalLighting {
                valid: true,
                ambient_landscape: ZONE_AMBIENT,
                ..default()
            })
            .init_resource::<ZoneGlobalLighting>()
            .add_systems(
                Update,
                (sample_zone_weather, update_zone_material_lighting).chain(),
            );
        app.update();
        let before = app.world().resource::<ZoneGlobalLighting>().clone();
        assert_eq!(before.0.ambient, ZONE_AMBIENT.extend(1.0));
        assert_eq!(before.1[&AREA].ambient, Vec4::from_array(AREA_AMBIENT));
        app.world_mut().resource_mut::<ZoneWeather>().area = AREA;
        app.update();
        let after = app.world().resource::<ZoneGlobalLighting>();
        assert_eq!(after.0.ambient, before.0.ambient);
        assert_eq!(after.1[&AREA].ambient, before.1[&AREA].ambient);
        *app.world_mut().resource_mut::<ZoneWeather>() = ZoneWeather::default();
        app.update();
        assert!(app.world().resource::<ZoneGlobalLighting>().1.is_empty());
    }

    fn bare_material() -> FfxiZoneMaterial {
        FfxiZoneMaterial::new(
            None,
            FfxiMaterialFlags { flags: Vec4::ZERO },
            Vec4::ONE,
            Vec4::ZERO,
            AlphaMode::Opaque,
            FfxiZoneMaterialKey::default(),
        )
    }

    fn touched_app() -> App {
        bevy::tasks::IoTaskPool::get_or_init(bevy::tasks::TaskPool::default);
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<FfxiZoneMaterial>()
            .init_resource::<ZoneMaterialTouched>()
            .add_systems(
                PostUpdate,
                collect_zone_material_touched.after(bevy::asset::AssetEventSystems),
            );
        app
    }

    #[test]
    fn touched_list_carries_asset_events_and_untracked_marks_for_one_frame() {
        let mut app = touched_app();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<FfxiZoneMaterial>>()
            .add(bare_material());
        let instance = app
            .world()
            .resource::<Assets<FfxiZoneMaterial>>()
            .get(&handle)
            .unwrap()
            .instance_id;

        app.update();
        let touched = app.world().resource::<ZoneMaterialTouched>();
        assert_eq!(
            touched.pending,
            vec![handle.id()],
            "Added lands on the list"
        );
        assert!(touched.dropped.is_empty());

        app.update();
        assert!(
            app.world()
                .resource::<ZoneMaterialTouched>()
                .pending
                .is_empty(),
            "a quiet frame uploads nothing"
        );

        app.world_mut()
            .resource_mut::<ZoneMaterialTouched>()
            .mark(handle.id());
        app.update();
        assert_eq!(
            app.world().resource::<ZoneMaterialTouched>().pending,
            vec![handle.id()],
            "an untracked writer's mark lands on the list"
        );

        drop(handle);
        let mut dropped_seen = false;
        for _ in 0..3 {
            app.update();
            dropped_seen |= app
                .world()
                .resource::<ZoneMaterialTouched>()
                .dropped
                .contains(&instance);
        }
        assert!(dropped_seen, "the last handle drop retires the instance");
    }

    #[test]
    fn fog_flag_maps_the_generator_bit() {
        assert_eq!(zone_fog_flag(true), ZONE_FLAG_FOGGED);
        assert_eq!(zone_fog_flag(false), ZONE_FLAG_UNFOGGED);
    }

    #[test]
    fn d3d8_transparent_bias_moves_forward_by_one_portable_step() {
        let bias = d3d8_z_bias(ffxi_dat::mmb::TRANSPARENT_Z_BIAS_LEVEL);
        assert_eq!(bias.constant, WGPU_FORWARD_DECAL_BIAS);
        assert_eq!(bias.slope_scale, WGPU_FORWARD_DECAL_SLOPE_SCALE);
        assert_eq!(bias.clamp, 0.0);
    }

    // The lane crosses into WGSL, where no type holds the two sides together: if the
    // shader stops testing flags.z the emitters keep setting a flag nothing reads and
    // every sky layer silently goes back to being fogged flat.
    #[test]
    fn shader_gates_distance_fog_on_the_lane() {
        let lines: Vec<&str> = include_str!("zone_ffxi.wesl")
            .lines()
            .map(str::trim)
            .collect();
        let call = lines
            .iter()
            .position(|l| l.starts_with("out_color = apply_distance_fog("))
            .expect("zone_ffxi.wesl no longer applies distance fog");
        let guard = lines[..call]
            .iter()
            .rev()
            .find(|l| !l.is_empty() && !l.starts_with("//"))
            .copied()
            .unwrap_or_default();
        assert!(
            guard.contains("material_flags.flags.z"),
            "distance fog is not gated on the fog lane; the statement before it is `{guard}`"
        );
    }

    // WGSL cannot import a Rust const, so the two sides are pinned by reading the
    // shader's own declaration. `d3d_stage_chain` below then models retail's stage
    // math against the SAME number the GPU runs.
    fn wgsl_const(src: &str, name: &str) -> f32 {
        let needle = format!("const {name}: f32 =");
        let (_, rest) = src.split_once(&needle).unwrap_or_else(|| {
            panic!("shader no longer declares `{name}`");
        });
        rest.split(';')
            .next()
            .unwrap()
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("`{name}` is not a plain literal: {e}"))
    }

    const ZONE_WGSL: &str = include_str!("zone_ffxi.wesl");
    const ACTOR_WGSL: &str = include_str!("skinned_ffxi.wesl");

    // research/XIClient Rendering/ZoneRenderer.cpp ZoneRenderer::ApplyDefaultRenderState over the saturated
    // fixed-function T&L diffuse of Direct3D8Manager.cpp Direct3D8Manager::InitializeRenderStateBlocks,390,393,395.
    fn d3d_zone_stage_chain(vertex_rgb: Vec3, irradiance: Vec3, texel: Vec3) -> Vec3 {
        let gain = wgsl_const(ZONE_WGSL, "D3D_MODULATE_2X");
        ((vertex_rgb * irradiance).min(Vec3::ONE) * texel * gain).min(Vec3::ONE)
    }

    #[test]
    fn neutral_vertex_under_full_light_lands_on_retails_ceiling() {
        let neutral = Vec3::from_slice(&ffxi_dat::mmb::vertex_color_neutral()[..3]);
        // One MODULATE2X over the half-scale neutral is unity, not the doubled value a
        // second (D3m) stage would give.
        let lit = d3d_zone_stage_chain(neutral, Vec3::ONE, Vec3::ONE);
        assert!((lit.x - 1.0).abs() < 0.01, "lit was {}", lit.x);
    }

    #[test]
    fn the_lit_vertex_term_saturates_before_the_texture_stage() {
        let neutral = Vec3::from_slice(&ffxi_dat::mmb::vertex_color_neutral()[..3]);
        // D3D8 fixed-function T&L emits a D3DCOLOR, so irradiance past the point where
        // vertexColour x light reaches 1.0 cannot brighten the fragment any further.
        let at_ceiling = d3d_zone_stage_chain(neutral, Vec3::splat(2.0), Vec3::splat(0.4));
        let far_past = d3d_zone_stage_chain(neutral, Vec3::splat(6.0), Vec3::splat(0.4));
        assert_eq!(at_ceiling, far_past);
        assert!((far_past.x - 0.8).abs() < 1e-5, "was {}", far_past.x);
    }

    #[test]
    fn overbright_lamp_glass_keeps_retails_night_headroom() {
        // A fully authored 255 vertex at a night ambient still reads brighter than the
        // neutral terrain around it, which is what makes lamp glass glow with no light.
        let glass = Vec3::from_slice(&ffxi_dat::mmb::vertex_color_to_linear([255; 4])[..3]);
        let neutral = Vec3::from_slice(&ffxi_dat::mmb::vertex_color_neutral()[..3]);
        let night = Vec3::splat(0.21);
        let lit = d3d_zone_stage_chain(glass, night, Vec3::ONE);
        assert!((lit.x - 0.42).abs() < 0.01, "was {}", lit.x);
        assert!(lit.x > d3d_zone_stage_chain(neutral, night, Vec3::ONE).x);
    }

    // Both shaders composite through one MODULATE2X; a change to either alone would
    // put terrain and the characters standing on it on different exposures.
    #[test]
    fn both_shaders_share_the_modulate_gain() {
        assert_eq!(
            wgsl_const(ZONE_WGSL, "D3D_MODULATE_2X"),
            wgsl_const(ACTOR_WGSL, "D3D_MODULATE_2X"),
        );
    }

    #[test]
    fn shadow_floor_matches_the_actor_shader() {
        assert_eq!(
            wgsl_const(ZONE_WGSL, "FFXI_SHADOW_FLOOR"),
            wgsl_const(ACTOR_WGSL, "FFXI_SHADOW_FLOOR"),
        );
    }

    // The per-stage saturate is the whole point of the fix: without it the lit vertex
    // term is no longer a D3DCOLOR and the terrain's tonal range doubles. Every
    // composite that feeds the fragment output has to carry one.
    #[test]
    fn both_shaders_saturate_every_composite() {
        for (name, src) in [("zone_ffxi", ZONE_WGSL), ("skinned_ffxi", ACTOR_WGSL)] {
            let composites: Vec<&str> = src
                .lines()
                .map(str::trim)
                .filter(|l| l.starts_with("let ") && l.contains("D3D_MODULATE_2X *"))
                .collect();
            assert!(!composites.is_empty(), "{name}.wesl composites nothing");
            for l in composites {
                assert!(
                    l.contains("saturate("),
                    "{name}.wesl has an unsaturated texture stage: `{l}`"
                );
            }
        }
    }

    // Direct3D8Manager.cpp Direct3D8Manager::InitializeRenderStateBlocks,393,395 makes the lit vertex term a D3DCOLOR whichever stage
    // chain consumes it, so BOTH branches clamp it before the first texel — the outer
    // saturate the sweep above checks does not, on its own, catch a chain that feeds an
    // over-1.0 vertex colour straight into MODULATE2X.
    #[test]
    fn every_branch_clamps_the_vertex_term_before_the_texel() {
        let first_stages: Vec<&str> = ZONE_WGSL
            .lines()
            .map(str::trim)
            .filter(|l| {
                (l.starts_with("let rgb = ") || l.starts_with("let stage0 = ")) && l.contains("lit")
            })
            .collect();
        assert_eq!(
            first_stages.len(),
            2,
            "expected one terrain and one generator composite: {first_stages:?}"
        );
        for stage in first_stages {
            assert!(
                stage.contains("saturate(lit)"),
                "composite does not saturate the lit vertex term: `{stage}`"
            );
        }
    }

    // `specialize` pushes this def; the shader has to be the thing that matches on it,
    // otherwise the two chains silently collapse into whichever one the shader hardcodes.
    #[test]
    fn the_shader_branches_on_the_stage_chain_def() {
        assert!(ZONE_WGSL.contains(&format!("@if({GENERATOR_STAGE_CHAIN_DEF})")));
        assert_eq!(
            FfxiZoneMaterialKey::LEGACY.generator_stage_chain,
            FfxiZoneMaterialKey::default().generator_stage_chain,
            "the terrain chain must stay the default for every un-annotated mesh"
        );
    }
}
