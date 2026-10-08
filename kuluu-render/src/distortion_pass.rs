//! Screen-space haze FIELD pass for retail's 0x22 `Distortion` generator element. A distortion
//! never draws pixels: it anchors a haze field at its attach site, with the footprint of its own
//! linked texture quad drawn at its authored scale (the DAT gives the element that texture — its
//! id resolves through the same mesh/texture chain every other particle element uses), and inside
//! that footprint it displaces THIS frame's scene horizontally where the texture's alpha says so.
//! Displacement only: source and destination are the same frame, so an extra copy of anything
//! cannot exist. Displacement width comes from the authored sec2 0x32 offset; footprint extent
//! and silhouette from the texture.
//!
//! Scheduled in Core3d AFTER `Core3dSystems::PostProcess` (bloom/DOF/fog/TAA/tonemapping done) and
//! before upscaling writes the window — the same bounds as [`crate::nameplate_final_pass`]. It is a
//! strict no-op unless a distortion field is alive ([`ActiveDistortion`]). While any field is
//! alive, one exact copy of the processed frame is taken first (fields write main; their displaced
//! samples must come from untouched pixels), then each field draws over its own rect.
//!
//! MSAA-safe by construction: bevy's ViewTarget "main" texture is ALWAYS single-sample — under
//! Msaa2/4/8 the geometry pass renders into a separate multi-sample buffer that wgpu resolves INTO
//! this main texture at pass end (ColorAttachment.resolve_target), so after PostProcess the
//! unsampled view holds the fully processed image in every AA mode. Both passes here touch only
//! that single-sample surface; nothing samples a multi-sample buffer.

use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use bevy::asset::{embedded_asset, AssetServer};
use bevy::core_pipeline::{upscaling::upscaling, Core3d, Core3dSystems};
use bevy::prelude::*;
use bevy::render::render_resource::{
    binding_types::{sampler as smp_entry, texture_2d, uniform_buffer},
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendComponent,
    BlendFactor, BlendOperation, BlendState, Buffer, BufferBinding, BufferDescriptor, BufferUsages,
    CachedRenderPipelineId, ColorTargetState, ColorWrites, Extent3d, FilterMode, FragmentState,
    LoadOp, MultisampleState, Operations, PipelineCache, PrimitiveState, PrimitiveTopology,
    RenderPassColorAttachment, RenderPassDescriptor, RenderPipelineDescriptor, Sampler,
    SamplerBindingType, SamplerDescriptor, ShaderStages, ShaderType, StoreOp, Texture,
    TextureDataOrder, TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType,
    TextureUsages, TextureView, TextureViewDescriptor, VertexState,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::view::{ExtractedView, ViewTarget};
use bevy::render::{Extract, RenderApp, RenderStartup};

/// The linked texture of a distortion element, RGBA8 with alpha already remapped to full range.
/// Its dimensions set the field's footprint aspect; its alpha sets where inside the footprint
/// haze acts (and the displacement follows that silhouette's horizontal gradient).
#[derive(Debug)]
pub struct DistortionMap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    id: u64,
}

static NEXT_MAP_ID: AtomicU64 = AtomicU64::new(1);

impl DistortionMap {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Self {
            width,
            height,
            rgba,
            id: NEXT_MAP_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
}

/// One live haze field. Main-world side; extracted verbatim each frame and drawn per-view.
#[derive(Clone)]
pub struct LiveField {
    /// World-space anchor — the generator's attach-frame origin (hit site).
    pub center: Vec3,
    /// Footprint half-size in world units, from the linked quad × authored init_scale.
    pub half_extent: Vec2,
    /// sec2 0x32 HazeOffsetInitializer horizontal offset as authored (g142: 0.02).
    pub haze_offset_x: f32,
    pub started_at: Instant,
    pub duration_secs: f32,
    /// sec2 0x2D KeyFrameValueSetup envelope over life (g142 binds k143); `None` = constant.
    pub envelope: Option<ffxi_dat::particle_gen::KeyFrameTrack>,
    pub map: Arc<DistortionMap>,
}

impl LiveField {
    /// The envelope value at `now`; drives both displacement width and coverage alpha.
    fn env(&self, now: Instant) -> f32 {
        let progress =
            ((now - self.started_at).as_secs_f32() / self.duration_secs.max(1e-6)).clamp(0.0, 1.0);
        self.envelope
            .as_ref()
            .map(|t| t.sample(progress))
            .unwrap_or(1.0)
    }

    fn is_expired(&self, now: Instant) -> bool {
        now >= self.started_at + std::time::Duration::from_secs_f32(self.duration_secs.max(0.0))
    }
}

/// Main-world: the live distortion fields. Pushed by `spawn_particle_generators` when a 0x22 def
/// spawns; pruned here on life expiry and reset (insert [`Default`]) by zone teardown like every
/// other transient effect resource.
#[derive(Resource, Default)]
pub struct ActiveDistortion {
    pub fields: Vec<LiveField>,
}

impl ActiveDistortion {
    pub fn push(&mut self, field: LiveField) {
        self.fields.push(field);
    }
}

fn prune_distortion_fields(mut d: ResMut<ActiveDistortion>) {
    let now = Instant::now();
    d.fields.retain(|f| !f.is_expired(now));
}

/// Render-world snapshot of the live fields (extracted from [`ActiveDistortion`]).
#[derive(Resource, Default)]
struct DistortionPassData {
    fields: Vec<LiveField>,
    /// The operator camera's main-world entity — only its primary view is distorted.
    operator_cam: Option<Entity>,
}

fn extract_distortion(
    mut data: ResMut<DistortionPassData>,
    src: Extract<Res<ActiveDistortion>>,
    operator_cameras: Extract<Query<Entity, With<crate::camera::OperatorCamera>>>,
) {
    data.fields.clone_from(&src.fields);
    data.operator_cam = operator_cameras.iter().next();
}

/// Per-field uniform. Byte layout must match distortion.wgsl `FieldUniform` (2×vec2 + 2 f32).
#[derive(ShaderType, Clone, Copy)]
struct FieldUniform {
    rect_min: Vec2,
    rect_max: Vec2,
    haze: f32,
    env: f32,
}

const FIELD_UNIFORM_SIZE: u64 = 24;

fn field_uniform_bytes(u: FieldUniform) -> [u8; FIELD_UNIFORM_SIZE as usize] {
    let mut bytes = [0u8; FIELD_UNIFORM_SIZE as usize];
    bytes[0..4].copy_from_slice(&u.rect_min.x.to_le_bytes());
    bytes[4..8].copy_from_slice(&u.rect_min.y.to_le_bytes());
    bytes[8..12].copy_from_slice(&u.rect_max.x.to_le_bytes());
    bytes[12..16].copy_from_slice(&u.rect_max.y.to_le_bytes());
    bytes[16..20].copy_from_slice(&u.haze.to_le_bytes());
    bytes[20..24].copy_from_slice(&u.env.to_le_bytes());
    bytes
}

/// Project a billboard-anchored field rect to NDC: the quad corners
/// `center ± right·hx ± up·hy` (basis taken from `world_from_view`, so it faces the camera like
/// every other hi14 element) through `clip`. Returns None when any corner is behind the camera,
/// or once clipped to [-1,1] nothing of the rect survives — untouched pixels either way.
fn field_rect(world_from_view: Mat4, clip: Mat4, center: Vec3, half: Vec2) -> Option<(Vec2, Vec2)> {
    let right = world_from_view.col(0).truncate();
    let up = world_from_view.col(1).truncate();
    let view_from_world = world_from_view.inverse();
    let mut min = Vec2::splat(f32::MAX);
    let mut max = Vec2::splat(-f32::MAX);
    for sx in [-1.0, 1.0] {
        for sy in [-1.0, 1.0] {
            let corner = center + right * (sx * half.x) + up * (sy * half.y);
            let c = clip * view_from_world * corner.extend(1.0);
            if c.w <= 1e-6 {
                return None;
            }
            let n = Vec2::new(c.x / c.w, c.y / c.w);
            min = min.min(n);
            max = max.max(n);
        }
    }
    // Preserve the original footprint so viewport clipping crops its texture.
    let visible_min = Vec2::new(min.x.max(-1.0), min.y.max(-1.0));
    let visible_max = Vec2::new(max.x.min(1.0), max.y.min(1.0));
    if visible_max.x - visible_min.x <= 0.0 || visible_max.y - visible_min.y <= 0.0 {
        return None;
    }
    Some((min, max))
}

fn field_bgl_descriptors() -> (BindGroupLayoutDescriptor, BindGroupLayoutDescriptor) {
    let group0 = BindGroupLayoutDescriptor::new(
        "distortion_field_bgl0",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                uniform_buffer::<FieldUniform>(false),
                texture_2d(TextureSampleType::Float { filterable: true }),
                smp_entry(SamplerBindingType::Filtering),
            ),
        ),
    );
    let group1 = BindGroupLayoutDescriptor::new(
        "distortion_field_bgl1",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                smp_entry(SamplerBindingType::Filtering),
            ),
        ),
    );
    (group0, group1)
}

fn copy_bgl_descriptor() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "distortion_copy_bgl",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (texture_2d(TextureSampleType::Float { filterable: false }),),
        ),
    )
}

/// GPU handles shared by the pass (created once at RenderStartup). The scene copy texture is
/// created lazily sized to the view; haze map textures are cached per [`DistortionMap`] id.
#[derive(Resource)]
struct DistortionPassGpu {
    field_shader: Handle<Shader>,
    copy_shader: Handle<Shader>,
    /// Shared between bind groups AND the pipeline descriptor (single source of truth for the
    /// layout shape) — same pattern as [`crate::nameplate_final_pass`].
    field_bgl0: BindGroupLayoutDescriptor,
    field_bgl1: BindGroupLayoutDescriptor,
    copy_bgl: BindGroupLayoutDescriptor,
    sampler: Sampler,
    scene_copy_texture: Option<Texture>,
    scene_copy_view: Option<TextureView>,
    copy_key: Option<(u32, u32, TextureFormat)>,
    map_cache: HashMap<u64, (Texture, TextureView)>,
    field_uniforms: Vec<Buffer>,
}

impl DistortionPassGpu {
    fn new(device: &RenderDevice, asset_server: &AssetServer) -> Self {
        let (field_bgl0, field_bgl1) = field_bgl_descriptors();
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("distortion_pass_sampler"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });
        Self {
            field_shader: asset_server.load("embedded://kuluu_render/distortion.wgsl"),
            copy_shader: asset_server.load("embedded://kuluu_render/distortion_copy.wgsl"),
            field_bgl0,
            field_bgl1,
            copy_bgl: copy_bgl_descriptor(),
            sampler,
            scene_copy_texture: None,
            scene_copy_view: None,
            copy_key: None,
            map_cache: HashMap::new(),
            field_uniforms: Vec::new(),
        }
    }

    /// Ensure the scene-copy texture exists at (w, h, format); recreate on any change. The view
    /// is dropped (refcounted) and the replaced texture destroyed explicitly so GPU memory frees
    /// immediately.
    fn ensure_copy(&mut self, device: &RenderDevice, size: Extent3d, format: TextureFormat) {
        let key = (size.width, size.height, format);
        if self.copy_key == Some(key) && self.scene_copy_texture.is_some() {
            return;
        }
        self.scene_copy_view = None;
        if let Some(old) = self.scene_copy_texture.take() {
            old.destroy();
        }
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("distortion_scene_copy"),
            size: Extent3d {
                width: size.width.max(1),
                height: size.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor {
            label: Some("distortion_scene_copy_view"),
            ..Default::default()
        });
        self.copy_key = Some(key);
        self.scene_copy_texture = Some(texture);
        self.scene_copy_view = Some(view);
    }

    /// Drop cached map textures whose [`DistortionMap`] id is no longer in the live set.
    fn prune_maps(&mut self, live: &HashSet<u64>) {
        let stale: Vec<u64> = self
            .map_cache
            .keys()
            .copied()
            .filter(|id| !live.contains(id))
            .collect();
        for id in stale {
            if let Some((texture, _)) = self.map_cache.remove(&id) {
                texture.destroy();
            }
        }
    }

    fn map_view_for(
        &mut self,
        device: &RenderDevice,
        queue: &RenderQueue,
        map: &DistortionMap,
    ) -> Option<TextureView> {
        if let Entry::Vacant(e) = self.map_cache.entry(map.id) {
            let texture = device.create_texture_with_data(
                queue,
                &TextureDescriptor {
                    label: Some("distortion_haze_map"),
                    size: Extent3d {
                        width: map.width.max(1),
                        height: map.height.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: TextureFormat::Rgba8Unorm,
                    usage: TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                TextureDataOrder::LayerMajor,
                &map.rgba,
            );
            let view = texture.create_view(&TextureViewDescriptor::default());
            e.insert((texture, view));
        }
        self.map_cache.get(&map.id).map(|(_, view)| view.clone())
    }

    fn uniform_for(&mut self, device: &RenderDevice, index: usize) -> Option<Buffer> {
        while self.field_uniforms.len() <= index {
            self.field_uniforms
                .push(device.create_buffer(&BufferDescriptor {
                    label: Some("distortion_field_uniform"),
                    size: FIELD_UNIFORM_SIZE,
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
        }
        self.field_uniforms.get(index).cloned()
    }
}

fn field_pipeline_descriptor(
    shader: &Handle<Shader>,
    bgl0: &BindGroupLayoutDescriptor,
    bgl1: &BindGroupLayoutDescriptor,
    format: TextureFormat,
) -> RenderPipelineDescriptor {
    RenderPipelineDescriptor {
        label: Some("distortion_field".into()),
        layout: vec![bgl0.clone(), bgl1.clone()],
        vertex: VertexState {
            shader: shader.clone(),
            entry_point: Some("vs".into()),
            ..Default::default()
        },
        fragment: Some(FragmentState {
            shader: shader.clone(),
            entry_point: Some("fs".into()),
            shader_defs: Vec::new(),
            targets: vec![Some(ColorTargetState {
                format,
                // Standard alpha blend (the old wgpu `BlendState::Alpha` constant): the field's
                // output alpha is map silhouette × envelope, so outside it pixels stay original.
                blend: Some(BlendState {
                    color: BlendComponent {
                        operation: BlendOperation::Add,
                        src_factor: BlendFactor::SrcAlpha,
                        dst_factor: BlendFactor::OneMinusSrcAlpha,
                    },
                    alpha: BlendComponent {
                        operation: BlendOperation::Add,
                        src_factor: BlendFactor::One,
                        dst_factor: BlendFactor::OneMinusSrcAlpha,
                    },
                }),
                write_mask: ColorWrites::ALL,
            })],
        }),
        primitive: PrimitiveState {
            topology: PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: MultisampleState::default(),
        immediate_size: 0,
        zero_initialize_workgroup_memory: true,
    }
}

fn copy_pipeline_descriptor(
    shader: &Handle<Shader>,
    bgl: &BindGroupLayoutDescriptor,
    format: TextureFormat,
) -> RenderPipelineDescriptor {
    RenderPipelineDescriptor {
        label: Some("distortion_copy".into()),
        layout: vec![bgl.clone()],
        vertex: VertexState {
            shader: shader.clone(),
            entry_point: Some("vs".into()),
            ..Default::default()
        },
        fragment: Some(FragmentState {
            shader: shader.clone(),
            entry_point: Some("fs".into()),
            shader_defs: Vec::new(),
            targets: vec![Some(ColorTargetState {
                format,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
        }),
        primitive: PrimitiveState {
            topology: PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: MultisampleState::default(),
        immediate_size: 0,
        zero_initialize_workgroup_memory: true,
    }
}

/// Core3d sub-schedule (per camera run): copy the processed frame exactly, then draw each live
/// field over its projected rect — displaced same-frame scene blended back with the map's alpha.
/// Gated to the operator camera; every other 3D camera (launcher, minimap bake, ...) runs its own
/// Core3d schedule and skips — same gate as [`crate::nameplate_final_pass`].
#[allow(clippy::type_complexity)]
fn draw_distortion_fields(
    view: ViewQuery<(&ExtractedView, &ViewTarget)>,
    data: Res<DistortionPassData>,
    mut gpu: ResMut<DistortionPassGpu>,
    device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
    queue: Res<RenderQueue>,
    mut ctx: RenderContext,
    mut pipe_state: Local<
        Option<(
            TextureFormat,
            (CachedRenderPipelineId, CachedRenderPipelineId),
        )>,
    >,
) {
    if data.fields.is_empty() {
        return;
    }

    let (ev, target) = view.into_inner();

    // Only the operator camera's PRIMARY view carries the effect. Note that `view.entity()` is a
    // RENDER-world view entity and does not equal a main-world camera Entity — match via
    // retained_view_entity instead.
    let Some(operator_cam) = data.operator_cam else {
        return;
    };
    if ev.retained_view_entity.main_entity.id() != operator_cam
        || ev.retained_view_entity.subview_index != 0
    {
        return;
    }

    let format = target.main_texture_format();
    let size = target.main_texture().size();
    gpu.ensure_copy(&device, size, format);
    let live_ids: HashSet<u64> = data.fields.iter().map(|f| f.map.id).collect();
    gpu.prune_maps(&live_ids);

    if pipe_state.as_ref().is_none_or(|(f, _)| *f != format) {
        let field_id = pipeline_cache.queue_render_pipeline(field_pipeline_descriptor(
            &gpu.field_shader,
            &gpu.field_bgl0,
            &gpu.field_bgl1,
            format,
        ));
        let copy_id = pipeline_cache.queue_render_pipeline(copy_pipeline_descriptor(
            &gpu.copy_shader,
            &gpu.copy_bgl,
            format,
        ));
        *pipe_state = Some((format, (field_id, copy_id)));
    }
    let (_, (field_id, copy_id)) = pipe_state.expect("set above");
    let (Some(field_pipeline), Some(copy_pipeline)) = (
        pipeline_cache.get_render_pipeline(field_id),
        pipeline_cache.get_render_pipeline(copy_id),
    ) else {
        return;
    };

    // Exact scene copy first: fields write main, and their displaced samples must come from
    // untouched pixels of THIS frame.
    let Some(scene_copy_view) = gpu.scene_copy_view.clone() else {
        return;
    };
    {
        let bgl = pipeline_cache.get_bind_group_layout(&gpu.copy_bgl);
        let copy_bg = device.create_bind_group(
            "distortion_copy",
            &bgl,
            &BindGroupEntries::sequential((target.main_texture_view(),)),
        );
        let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("distortion_copy"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &scene_copy_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    // wgpu is a native-only dep here; the app color converts to it (bevy's own
                    // no-camera clear does the same to_linear().into()).
                    load: LoadOp::Clear(Color::srgba(0.0, 0.0, 0.0, 0.0).to_linear().into()),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_render_pipeline(copy_pipeline);
        pass.set_bind_group(0, &copy_bg, &[]);
        pass.draw(0..3, 0..1);
    }

    let world_from_view = ev.world_from_view.to_matrix();
    let clip = ev.clip_from_view;
    let now = Instant::now();
    let field_bgl0 = pipeline_cache.get_bind_group_layout(&gpu.field_bgl0);
    let field_bgl1 = pipeline_cache.get_bind_group_layout(&gpu.field_bgl1);

    for (i, f) in data.fields.iter().enumerate() {
        let env = f.env(now);
        if env <= 0.0 {
            continue;
        }
        let Some((rect_min, rect_max)) = field_rect(world_from_view, clip, f.center, f.half_extent)
        else {
            continue;
        };
        let Some(uniform_buffer) = gpu.uniform_for(&device, i) else {
            continue;
        };
        queue.write_buffer(
            &uniform_buffer,
            0,
            &field_uniform_bytes(FieldUniform {
                rect_min,
                rect_max,
                haze: f.haze_offset_x,
                env,
            }),
        );
        let Some(map_view) = gpu.map_view_for(&device, &queue, &f.map) else {
            continue;
        };
        let bg0 = device.create_bind_group(
            "distortion_field_0",
            &field_bgl0,
            &BindGroupEntries::sequential((
                BufferBinding {
                    buffer: &uniform_buffer,
                    offset: 0,
                    size: None,
                },
                &scene_copy_view,
                &gpu.sampler,
            )),
        );
        let bg1 = device.create_bind_group(
            "distortion_field_1",
            &field_bgl1,
            &BindGroupEntries::sequential((&map_view, &gpu.sampler)),
        );
        let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("distortion_field"),
            color_attachments: &[Some(target.get_unsampled_color_attachment())],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_render_pipeline(field_pipeline);
        pass.set_bind_group(0, &bg0, &[]);
        pass.set_bind_group(1, &bg1, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn init_distortion_pass_gpu(
    mut commands: Commands,
    device: Res<RenderDevice>,
    asset_server: Res<AssetServer>,
) {
    commands.insert_resource(DistortionPassGpu::new(&device, &asset_server));
}

pub struct DistortionPassPlugin;

impl Plugin for DistortionPassPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "distortion.wgsl");
        embedded_asset!(app, "distortion_copy.wgsl");
        app.init_resource::<ActiveDistortion>()
            .add_systems(Update, prune_distortion_fields);
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<DistortionPassData>()
                .add_systems(ExtractSchedule, extract_distortion)
                .add_systems(RenderStartup, init_distortion_pass_gpu)
                .add_systems(
                    Core3d,
                    draw_distortion_fields
                        .after(Core3dSystems::PostProcess)
                        .before(bevy::ui_render::ui_pass)
                        .before(upscaling),
                );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The uniform byte layout is a wire contract with distortion.wgsl — pin it so a field
    /// reorder cannot silently desync the two.
    #[test]
    fn uniform_bytes_match_the_wgsl_layout() {
        let bytes = field_uniform_bytes(FieldUniform {
            rect_min: Vec2::new(-0.5, -0.25),
            rect_max: Vec2::new(0.5, 0.75),
            haze: 0.02,
            env: 0.48,
        });
        assert_eq!(bytes[0..4], (-0.5f32).to_le_bytes());
        assert_eq!(bytes[4..8], (-0.25f32).to_le_bytes());
        assert_eq!(bytes[8..12], 0.5f32.to_le_bytes());
        assert_eq!(bytes[12..16], 0.75f32.to_le_bytes());
        assert_eq!(bytes[16..20], 0.02f32.to_le_bytes());
        assert_eq!(bytes[20..24], 0.48f32.to_le_bytes());
    }

    #[test]
    fn field_facing_the_camera_projects_inside_ndc() {
        let identity = Mat4::IDENTITY;
        let clip = Mat4::perspective_rh(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 500.0);
        let Some((min, max)) = field_rect(
            identity,
            clip,
            Vec3::new(0.0, 0.0, -3.0),
            Vec2::new(1.0, 1.0),
        ) else {
            panic!("a centred front field must project");
        };
        assert!(min.x < 0.0 && max.x > 0.0);
        assert!(min.y < 0.0 && max.y > 0.0);
        // Smaller than the half-frustum at that depth: it must not fill the screen.
        assert!(max.x < 1.0);
    }

    #[test]
    fn partially_clipped_field_preserves_footprint_coordinates() {
        let center = Vec3::X;
        let half = Vec2::ONE;
        let (min, max) = field_rect(Mat4::IDENTITY, Mat4::IDENTITY, center, half)
            .expect("a partly visible footprint must project");
        assert_eq!(min.x, center.x - half.x);
        assert_eq!(max.x, center.x + half.x);
        let viewport_edge_u = (1.0 - min.x) / (max.x - min.x);
        assert_eq!(viewport_edge_u, 0.5);
    }

    #[test]
    fn field_behind_the_camera_does_not_draw() {
        let clip = Mat4::perspective_rh(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 500.0);
        assert!(field_rect(Mat4::IDENTITY, clip, Vec3::new(0.0, 0.0, 5.0), Vec2::ONE).is_none());
    }

    #[test]
    fn field_off_screen_after_clipping_does_not_draw() {
        let clip = Mat4::perspective_rh(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 500.0);
        // Far to the +X side at a shallow depth: projects past NDC on both corners → clipped away.
        assert!(field_rect(Mat4::IDENTITY, clip, Vec3::new(90.0, 0.0, -2.0), Vec2::ONE).is_none());
    }

    #[test]
    fn fields_expire_on_their_authored_life() {
        let map = Arc::new(DistortionMap::new(1, 1, vec![0, 0, 0, 255]));
        let live = LiveField {
            center: Vec3::ZERO,
            half_extent: Vec2::ONE,
            haze_offset_x: 0.02,
            started_at: Instant::now(),
            duration_secs: 60.0 / 60.0,
            envelope: None,
            map: map.clone(),
        };
        assert!(!live.is_expired(Instant::now()));
        let later = live.started_at + Duration::from_secs_f32(1.1);
        assert!(live.is_expired(later));

        let mut d = ActiveDistortion::default();
        assert!(d.fields.is_empty());
        d.push(live);
        assert_eq!(d.fields.len(), 1);
    }
}
