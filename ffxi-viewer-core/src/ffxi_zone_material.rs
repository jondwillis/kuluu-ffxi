#![cfg(not(target_arch = "wasm32"))]

use bevy::asset::embedded_asset;
use bevy::ecs::system::lifetimeless::SRes;
use bevy::ecs::system::SystemParamItem;
use bevy::mesh::{Mesh, MeshVertexBufferLayoutRef};
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    AsBindGroup, AsBindGroupError, BindGroupLayout, BindGroupLayoutEntry, BindingResources,
    BindingType, Buffer, BufferBindingType, BufferDescriptor, BufferUsages, Face, FrontFace,
    OwnedBindingResource, RenderPipelineDescriptor, SamplerBindingType, ShaderStages, ShaderType,
    SpecializedMeshPipelineError, TextureSampleType, TextureViewDimension, UnpreparedBindGroup,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::texture::{FallbackImage, GpuImage};
use bevy::render::{Extract, ExtractSchedule, RenderApp};
use bevy::shader::ShaderRef;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::skinned_ffxi_material::{write_uniform, FfxiLightingUniform, FfxiMaterialFlags};

static NEXT_ZONE_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);

/// Zone lighting is identical for every zone submesh, so it lives in ONE
/// persistent GPU buffer shared by all zone-material bind groups
/// ([`ZoneMaterialBuffers::lighting`]) and is refreshed by `write_buffer`, never
/// by touching the material assets. The previous design gave each of the
/// hundreds of per-submesh materials its own lighting uniform and pushed
/// updates via `Assets::iter_mut()`, which flagged every material Modified the
/// moment the Vana'diel sun crept past an epsilon — a full bind-group rebuild
/// wave (~45ms) every ~0.9s, the visible periodic frame hitch.
#[derive(Resource, Clone, Default)]
pub struct ZoneGlobalLighting(pub FfxiLightingUniform);

/// Pipeline-key half of the MMB/MZB render-state word (ffxi-dat
/// `MmbRenderState`, decoded from the u16 at subrecord offset 18).
///
/// xim references:
/// - ZoneMeshSection.kt:120-123 — blended zone meshes render at
///   `ZBiasLevel.High` (1), opaque at `Normal` (0).
/// - GLDrawer.kt:198-201 — blended meshes disable depth write and apply
///   `glPolygonOffset(zBias * -1, 1)` to pull decals over the base terrain.
/// - Bit `0x2000` CLEAR enables back-face culling.
/// - GLDrawer.kt:186 — front face is `CW` (D3D-era winding), flipped to `CCW`
///   when the instance is mirrored (`scale.x * scale.y * scale.z < 0`).
///
/// These flow into `specialize` via `AsBindGroup::Data`, so each distinct
/// combination gets its own specialized render pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct FfxiZoneMaterialKey {
    /// Cull back faces (`Face::Back`). FFXI winding is **clockwise** (D3D
    /// convention, xim GLDrawer.kt:186), not Bevy's CCW default.
    pub back_face_culling: bool,
    /// Placement transform has a negative determinant (mirrored). Flips the
    /// effective winding, so `specialize` flips `front_face` back to CCW.
    /// Zone tiles are routinely placed mirrored in alternating checkerboard
    /// patterns, so this must be per-placement, not per-chunk.
    pub mirrored: bool,
    /// 0 = Normal, 1 = High (blended decal layers).
    pub z_bias_level: u8,
    /// `false` for blended decals (they must not occlude later layers).
    pub depth_write: bool,
}

impl FfxiZoneMaterialKey {
    /// Key for meshes that predate render-state plumbing: no culling, no
    /// bias, normal depth write — exactly the old hardcoded pipeline.
    pub const LEGACY: Self = Self {
        back_face_culling: false,
        mirrored: false,
        z_bias_level: 0,
        depth_write: true,
    };
}

#[derive(Asset, TypePath, Clone, Debug)]
pub struct FfxiZoneMaterial {
    pub base_color_texture: Option<Handle<Image>>,
    pub material_flags: FfxiMaterialFlags,

    // research/xim ParticleGeneratorParser.kt:431-434 ToD color: a per-mesh RGB(setter) +
    // alpha(multiplier) the weat/<type>/ ClockValueUpdaters drive over the Vana day. Folded
    // as a final modulate in the fragment shader. White (1,1,1,1) is the no-op default for
    // every other zone mesh — only the cloud/sun layers (zone_clouds.rs) write a live tint.
    pub tint: Vec4,

    // research/xim ParticleUpdaters.kt TextureCoordinateUpdater: animated UV scroll
    // (xy) that drifts the cloud canopy texture for wind. Zero (the default for every
    // other zone mesh) is a no-op.
    pub uv_offset: Vec4,

    pub alpha_mode: AlphaMode,

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
            base_color_texture,
            material_flags,
            tint,
            uv_offset,
            alpha_mode,
            render_key,
            instance_id: NEXT_ZONE_INSTANCE_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
}

struct ZoneInstanceBuffers {
    flags: Buffer,
    tint: Buffer,
    uv: Buffer,
    last_flags: Vec4,
    last_tint: Vec4,
    last_uv: Vec4,
}

#[derive(Resource)]
pub struct ZoneMaterialBuffers {
    lighting: Buffer,
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
            instances: HashMap::new(),
        }
    }
}

fn upload_zone_material_buffers(
    lighting: Extract<Res<ZoneGlobalLighting>>,
    materials: Extract<Res<Assets<FfxiZoneMaterial>>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut cache: ResMut<ZoneMaterialBuffers>,
) {
    write_uniform(&queue, &cache.lighting, &lighting.0);

    let uniform_buffer = |label: &'static str, size: std::num::NonZeroU64| {
        device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: size.get(),
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };

    let mut live: HashSet<u64> = HashSet::with_capacity(materials.len());
    for (_id, mat) in materials.iter() {
        live.insert(mat.instance_id);
        match cache.instances.entry(mat.instance_id) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                let inst = e.get_mut();
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
                let flags = uniform_buffer("ffxi_zone_flags", FfxiMaterialFlags::min_size());
                let tint = uniform_buffer("ffxi_zone_tint", Vec4::min_size());
                let uv = uniform_buffer("ffxi_zone_uv", Vec4::min_size());
                write_uniform(&queue, &flags, &mat.material_flags);
                write_uniform(&queue, &tint, &mat.tint);
                write_uniform(&queue, &uv, &mat.uv_offset);
                e.insert(ZoneInstanceBuffers {
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
    cache.instances.retain(|id, _| live.contains(id));
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

    fn unprepared_bind_group(
        &self,
        _layout: &BindGroupLayout,
        _render_device: &RenderDevice,
        param: &mut SystemParamItem<'_, '_, Self::Param>,
        _force_no_bindless: bool,
    ) -> Result<UnpreparedBindGroup, AsBindGroupError> {
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
        Ok(UnpreparedBindGroup {
            bindings: BindingResources(vec![
                (0, OwnedBindingResource::Buffer(buffers.lighting.clone())),
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
                (3, OwnedBindingResource::Buffer(inst.flags.clone())),
                (4, OwnedBindingResource::Buffer(inst.tint.clone())),
                (5, OwnedBindingResource::Buffer(inst.uv.clone())),
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
            uniform(3, FfxiMaterialFlags::min_size()),
            uniform(4, Vec4::min_size()),
            uniform(5, Vec4::min_size()),
        ]
    }
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

        // xim renders FFXI zone geometry with back-face culling unless the
        // render-state word sets bit 0x2000 (two-sided decals, fences, foliage
        // cards). FFXI winding is CLOCKWISE (D3D convention) — GLDrawer.kt:186
        // sets frontFace(CW), flipping to CCW for mirrored instances
        // (scale.x * scale.y * scale.z < 0). Using Bevy's CCW default here
        // culled every non-mirrored tile: inverted-checkerboard zone geometry.
        descriptor.primitive.cull_mode = if rk.back_face_culling {
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
            // GLDrawer.kt:198-201 — blended decals never write depth. Bevy's
            // transparent pass already disables depth write, but the prepass
            // (enable_prepass = true) would otherwise still write it; AND the
            // flag in rather than overwrite whatever the pass chose.
            ds.depth_write_enabled &= rk.depth_write;

            // GLDrawer.kt: glPolygonOffset(zBias * -1, 1) pulls ZBiasLevel::High
            // decal layers toward the camera over the coplanar base terrain.
            // Bevy uses a reversed-Z depth buffer (closer = larger depth,
            // GreaterEqual compare), so both GL terms flip sign: slope +zBias,
            // constant -1.
            if rk.z_bias_level > 0 {
                ds.bias.slope_scale = rk.z_bias_level as f32;
                ds.bias.constant = -1;
            }
        }

        Ok(())
    }
}

fn update_zone_material_lighting(
    ambient: Res<GlobalAmbientLight>,
    zone_lighting: Res<crate::weather::ZoneDirectionalLighting>,
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
) {
    const AMBIENT_REF_LUX: f32 = 1000.0;
    const DIR_REF_LUX: f32 = 12000.0;
    const COLOR_BIAS: Vec3 = Vec3::new(1.4, 1.36, 1.45);
    const AMBIENT_BIAS_BELOW: f32 = 0.5;

    // Terrain ambient floor, matched to the actor path so ground and models
    // darken together at night. (Was 0.28, which — ×2 in the overbright shader —
    // floored night terrain to ~0.56 and washed the darkness out.)
    const AMBIENT_FLOOR: f32 = 0.12;

    // research/xim EnvironmentSection.kt:163-164: the 0x2F landscape ambient is
    // the authoritative per-hour base (dark at night). Use it directly when the
    // zone ships records; the GlobalAmbientLight amb_k/COLOR_BIAS path is the
    // no-DAT fallback (it re-derives from the atmosphere seed and inflates).
    let mut amb_rgb = if zone_lighting.valid {
        zone_lighting.ambient_landscape
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
    // research/xim EnvironmentSection.kt:163-164: zone geometry takes both terrain
    // sun(dir0)+moon(dir1) diffuse lights. The DirectionalLight's `forward` is the
    // -to-celestial direction, so negate the stored to-sun/to-moon vectors to match.
    let (dir0_dir, dir0_color, dir1_dir, dir1_color) = if zone_lighting.valid {
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
}

// Feeds the shader's four point-light slots GLOBALLY: the four lights nearest
// the viewer go to every zone material identically via the shared lighting
// buffer. Per-submesh selection is impossible here because instanced MMB
// placements SHARE one cached FfxiZoneMaterial handle (dat_mmb.rs keys it by
// file_id/chunk_idx/sub_index) — writing position-dependent data into a shared
// material makes co-located submeshes fight every frame and flicker as
// streaming overlays reshuffle query order. A single global set sidesteps
// that, and the range cutoff in nearest_point_light_arrays keeps far geometry
// dark.
fn update_zone_material_point_lights(
    active: Res<crate::zone_point_lights::ActiveSceneLights>,
    q_self: Query<&GlobalTransform, With<crate::components::IsSelf>>,
    q_cam: Query<&GlobalTransform, With<Camera3d>>,
    mut global: ResMut<ZoneGlobalLighting>,
    mut selected: Local<Vec<Vec3>>,
) {
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

    global.0.point_pos = point_pos;
    global.0.point_color = point_color;
    global.0.point_atten = point_atten;
}

pub struct FfxiZoneMaterialPlugin;

impl Plugin for FfxiZoneMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "zone_ffxi.wgsl");
        embedded_asset!(app, "zone_ffxi_prepass.wgsl");
        app.add_plugins(MaterialPlugin::<FfxiZoneMaterial>::default())
            .init_resource::<ZoneGlobalLighting>()
            // Idempotent: the full viewer also inits this (lib.rs). Minimal apps
            // (zone-render-headless) add only this plugin, and
            // update_zone_material_lighting reads the resource unconditionally.
            .init_resource::<crate::weather::ZoneDirectionalLighting>()
            .add_systems(Update, update_zone_material_lighting)
            .add_systems(
                Update,
                update_zone_material_point_lights
                    .after(crate::zone_point_lights::build_active_scene_lights),
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
