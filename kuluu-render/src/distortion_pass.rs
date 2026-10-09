//! Screen-space haze FIELD pass for retail's 0x22 `Distortion` generator element. A distortion never
//! draws pixels of its own: it anchors a haze field at its attach site, and inside that field the
//! scene drawn so far is redrawn displaced. The footprint is built into the element — its center plus
//! four axis endpoints, scaled by the authored init scale — because type 0x22 deliberately bypasses
//! named-resource lookup: no mesh, image or sprite sheet is ever resolved for it, and the copy's
//! texture is this frame's scene. Retail-grounded in
//! `.agents/skills/retail-observe/references/2026-10-07-procedural-distortion.md`.
//! Coverage comes from raster interpolation of vertex alpha (center = the element's current alpha,
//! four rim vertices transparent) across a fan of four triangles — not a gradient texture. The authored
//! haze parameter translates the *drawn* fan along both draw axes while sampling coordinates stay on
//! the original footprint, and it stays put while the bound alpha track fades coverage in and out.
//!
//! Scheduled in Core3d AFTER `Core3dSystems::PostProcess` (bloom/DOF/fog/TAA/tonemapping done) and
//! before upscaling writes the window — the same bounds as [`crate::nameplate_final_pass`]. It is a
//! strict no-op unless a distortion field is alive ([`ActiveDistortion`]). While any field is alive,
//! one exact copy of the processed frame is taken first (fields write main; their displaced samples
//! must come from untouched pixels), then every live fan draws in one pass.
//!
//! MSAA-safe by construction: bevy's ViewTarget "main" texture is ALWAYS single-sample — under
//! Msaa2/4/8 the geometry pass renders into a separate multi-sample buffer that wgpu resolves INTO
//! this main texture at pass end (ColorAttachment.resolve_target), so after PostProcess the
//! unsampled view holds the fully processed image in every AA mode. Both passes here touch only
//! that single-sample surface; nothing samples a multi-sample buffer.

use std::time::Instant;

use bevy::asset::{embedded_asset, AssetServer};
use bevy::core_pipeline::{upscaling::upscaling, Core3d, Core3dSystems};
use bevy::mesh::VertexBufferLayout;
use bevy::prelude::*;
use bevy::render::render_resource::{
    binding_types::{sampler as smp_entry, texture_2d},
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendComponent,
    BlendFactor, BlendOperation, BlendState, Buffer, BufferDescriptor, BufferUsages,
    CachedRenderPipelineId, ColorTargetState, ColorWrites, Extent3d, FilterMode, FragmentState,
    FrontFace, LoadOp, MultisampleState, Operations, PipelineCache, PrimitiveState,
    PrimitiveTopology, RenderPassColorAttachment, RenderPassDescriptor, RenderPipelineDescriptor,
    Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages, StoreOp, Texture,
    TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType, TextureUsages,
    TextureView, TextureViewDescriptor, VertexFormat, VertexState, VertexStepMode,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::view::{ExtractedView, ViewTarget};
use bevy::render::{Extract, RenderApp, RenderStartup};

/// The element's built-in local footprint: its center and the four axis endpoints, in the element's
/// own units before the authored scale. Retail has no other geometry for this element type — that is
/// what lets it draw with no mesh or image bound at all (`.agents/skills/retail-observe/references/2026-10-07-procedural-distortion.md`,
/// "Footprint and scene sampling").
pub const FOOTPRINT_LOCAL: [[f32; 2]; 5] =
    [[0.0, 0.0], [-1.0, 0.0], [0.0, -1.0], [1.0, 0.0], [0.0, 1.0]];

/// The fan's four triangles as (center, rim, next rim) index triples into [`FOOTPRINT_LOCAL`]:
/// center, the four ordered rim points, and the first rim point again.
pub const FAN_TRIANGLES: [[usize; 3]; 4] = [[0, 1, 2], [0, 2, 3], [0, 3, 4], [0, 4, 1]];

/// Retail caps the intermediate capture at this many texels across an axis and magnifies it back over
/// the footprint, so a field wider than that shows coarse scene content (same record: "Capture
/// dimensions at or above 256 are reduced using 255 divided by the projected extent on that axis").
const CAPTURE_MAX_TEXELS_PER_AXIS: f32 = 255.0;
const CAPTURE_REDUCED_FROM_EXTENT_PX: f32 = 256.0;

/// The width of the alpha channel retail's tick keeps sampled values in: scaled by 255, negatives sent
/// to zero, held as one byte (same record, "Coverage, color and time"). Stepping through this is what
/// makes a fade quantise rather than glide.
const ALPHA_BYTE_MAX: f32 = 255.0;

/// The actor carrying a live field while it lives, and where that actor stood when the carry last
/// applied. Retail attaches g142's element to an actor
/// (`.agents/skills/retail-observe/references/2026-10-07-procedural-distortion.md`, "The clean global
/// generator g142"), so a directly armed field tracks that actor frame by frame instead of sitting
/// where the hit landed. Only translation is carried: the fan is built screen-facing from `center` by
/// [`build_fan`], so there is no geometry for a rotation to steer.
#[derive(Clone, Copy)]
pub struct FieldFollow {
    /// The anchor actor — the attach frame's own reference, not necessarily the routine's owner.
    pub actor: Entity,
    last_actor_pos: Option<Vec3>,
}

impl FieldFollow {
    /// A carry that has not seen its anchor yet: the first tick records where the actor stands and
    /// moves nothing, so a field armed before its anchor is measured stays at its attach site.
    pub fn new(actor: Entity) -> Self {
        Self {
            actor,
            last_actor_pos: None,
        }
    }
}

/// One live haze field. Main-world side; extracted verbatim each frame and drawn per-view.
#[derive(Clone)]
pub struct LiveField {
    /// World-space anchor — the generator's attach-frame origin (hit site).
    pub center: Vec3,
    /// Footprint half-size in world units: [`FOOTPRINT_LOCAL`]'s unit axes at the authored init scale.
    pub half_extent: Vec2,
    /// sec2 0x32 HazeOffsetInitializer's authored offset (g142: 0.02). It shifts the drawn fan along
    /// both draw axes; it is not a pixel or viewport-UV amount, so projection and attachment set how
    /// far the displaced image actually lands.
    pub haze_offset: f32,
    pub started_at: Instant,
    pub duration_secs: f32,
    /// sec2 0x2D KeyFrameValueSetup — the alpha track over life (g142 binds k143). It fades coverage
    /// only; in that authored path it never scales the haze translation. `None` = constant full alpha.
    pub envelope: Option<ffxi_dat::particle_gen::KeyFrameTrack>,
    /// Motion carry while an anchor actor lives; dropped when that actor is gone, which leaves the
    /// field finishing its authored life in world space rather than vanishing with it.
    pub follow: Option<FieldFollow>,
}

impl LiveField {
    /// Life progress, computed retail-side as one minus remaining life over initial life.
    fn progress(&self, now: Instant) -> f32 {
        let elapsed = (now - self.started_at).as_secs_f32();
        (elapsed / self.duration_secs.max(1e-6)).clamp(0.0, 1.0)
    }

    /// The element's current alpha as the graphics pipeline holds it: the track is sampled linearly,
    /// scaled by 255, negatives clamped to zero and the result kept in 8 bits (same record, "Coverage,
    /// color and time"). Quantizing here rather than keeping a float is what makes the fade step.
    pub fn alpha_byte(&self, now: Instant) -> u8 {
        let sampled = self
            .envelope
            .as_ref()
            .map(|track| track.sample(self.progress(now)))
            .unwrap_or(1.0);
        (sampled * ALPHA_BYTE_MAX).clamp(0.0, ALPHA_BYTE_MAX) as u8
    }

    /// The center vertex's alpha — the fan rim is transparent, so this is the field's peak coverage.
    fn center_alpha(&self, now: Instant) -> f32 {
        self.alpha_byte(now) as f32 / ALPHA_BYTE_MAX
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

/// Carries each live field with its anchor actor while that actor is present. When the actor is gone
/// the carry drops, leaving the field's own life clock to finish in world space — the field stops
/// tracking rather than disappearing early.
fn carry_attached_distortion_fields(
    q_actor: Query<&GlobalTransform>,
    mut distortion: ResMut<ActiveDistortion>,
) {
    for field in &mut distortion.fields {
        let Some(follow) = field.follow.as_mut() else {
            continue;
        };
        let Ok(actor) = q_actor.get(follow.actor) else {
            field.follow = None;
            continue;
        };
        let here = actor.translation();
        if let Some(there) = follow.last_actor_pos {
            field.center += here - there;
        }
        follow.last_actor_pos = Some(here);
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

/// One fan vertex. `FAN_TRIANGLES` × 3 vertices per field, all fields in one buffer and one draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FanVertex {
    /// Projected corner with the haze translation applied (drawn position).
    pub pos_shifted: Vec2,
    /// The same corner without it — sampling happens here, so what shows inside the fan is scene
    /// content from the original footprint.
    pub sample_ndc: Vec2,
    /// Top-left of the field's projected bounding box, carried flat so the fragment can step a
    /// capture grid.
    pub foot_origin: Vec2,
    /// One texel of that grid in NDC per axis.
    pub capture_step: Vec2,
    pub vertex_alpha: f32,
    pub vertex_rgb: f32,
}

/// Floats written per [`FanVertex`], and the attribute order `distortion.wgsl` expects.
pub const FAN_VERTEX_FLOATS: usize = 10;
const FAN_VERTEX_BYTES: usize = FAN_VERTEX_FLOATS * std::mem::size_of::<f32>();

/// The vertex RGB a fan corner carries. Retail's four rim vertices and the closing vertex hold
/// [128,128,128]; the center takes the element's own colour, which no g142 observation pins down.
/// Until something grounds it the center carries the same value, because the fan's colour op is a
/// doubled modulate of texture and vertex colour, so [128,128,128] leaves the scene copy untouched —
/// coverage stays the only thing shaping what shows (same record, "Coverage, color and time").
pub const FAN_RIM_VERTEX_RGB: f32 = 128.0 / 255.0;
pub const FAN_CENTER_VERTEX_RGB: f32 = FAN_RIM_VERTEX_RGB;

fn fan_vertex_bytes(v: &FanVertex, out: &mut Vec<u8>) {
    for value in [
        v.pos_shifted.x,
        v.pos_shifted.y,
        v.sample_ndc.x,
        v.sample_ndc.y,
        v.foot_origin.x,
        v.foot_origin.y,
        v.capture_step.x,
        v.capture_step.y,
        v.vertex_alpha,
        v.vertex_rgb,
    ] {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

/// Project one footprint point (its local unit axes scaled by the field's authored extent) through the
/// view. `Some` only when the point survives the near plane; a billboard basis comes from
/// `world_from_view`, so the footprint faces the camera like every other hi14 element.
fn project_footprint_point(
    world_from_view: Mat4,
    clip: Mat4,
    center: Vec3,
    half: Vec2,
    local: [f32; 2],
    offset_local: Vec2,
) -> Option<Vec2> {
    let right = world_from_view.col(0).truncate();
    let up = world_from_view.col(1).truncate();
    let point = center
        + right * ((local[0] + offset_local.x) * half.x)
        + up * ((local[1] + offset_local.y) * half.y);
    let c = clip * world_from_view.inverse() * point.extend(1.0);
    if c.w <= 1e-6 {
        return None;
    }
    Some(Vec2::new(c.x / c.w, c.y / c.w))
}

/// The intermediate capture for a footprint whose projected box starts at `bbox_min` and spans
/// `extent_ndc`: that same origin, plus the NDC width of one capture texel on each axis. Below
/// [`CAPTURE_REDUCED_FROM_EXTENT_PX`] there is no reduction — one texel per screen pixel — and from
/// there up the capture holds [`CAPTURE_MAX_TEXELS_PER_AXIS`] across however wide the field gets.
pub fn capture_grid(bbox_min: Vec2, extent_ndc: Vec2, viewport_px: Vec2) -> (Vec2, Vec2) {
    let extent_px = extent_ndc.abs() * 0.5 * viewport_px;
    let texels = Vec2::new(
        if extent_px.x >= CAPTURE_REDUCED_FROM_EXTENT_PX {
            CAPTURE_MAX_TEXELS_PER_AXIS
        } else {
            extent_px.max(Vec2::ONE).x
        },
        if extent_px.y >= CAPTURE_REDUCED_FROM_EXTENT_PX {
            CAPTURE_MAX_TEXELS_PER_AXIS
        } else {
            extent_px.max(Vec2::ONE).y
        },
    );
    (bbox_min, extent_ndc / texels)
}

/// The fan for one field: the five projected footprint points joined center→rim in [`FAN_TRIANGLES`],
/// drawn at the haze-translated position while sampling stays on the untranslated projection. `None`
/// when anything of the footprint falls behind the near plane, or once the bounding box has no area —
/// untouched pixels either way.
pub fn build_fan(
    world_from_view: Mat4,
    clip: Mat4,
    field: &LiveField,
    now: Instant,
    viewport_px: Vec2,
) -> Option<Vec<FanVertex>> {
    let center_alpha = field.center_alpha(now);
    if center_alpha <= 0.0 {
        return None;
    }
    // The haze offset is authored in the element's own units, so it travels with the footprint: it is
    // a fraction of the extent, not of the screen. Both draw axes take it; sampling must not.
    let haze_local = Vec2::new(field.haze_offset, field.haze_offset);

    let mut shifted = [Vec2::ZERO; 5];
    let mut unshifted = [Vec2::ZERO; 5];
    for i in 0..5 {
        let (Some(s), Some(u)) = (
            project_footprint_point(
                world_from_view,
                clip,
                field.center,
                field.half_extent,
                FOOTPRINT_LOCAL[i],
                haze_local,
            ),
            project_footprint_point(
                world_from_view,
                clip,
                field.center,
                field.half_extent,
                FOOTPRINT_LOCAL[i],
                Vec2::ZERO,
            ),
        ) else {
            return None;
        };
        shifted[i] = s;
        unshifted[i] = u;
    }

    let mut min = Vec2::splat(f32::MAX);
    let mut max = Vec2::splat(-f32::MAX);
    for p in unshifted.iter() {
        min = min.min(*p);
        max = max.max(*p);
    }
    let extent_ndc = max - min;
    if extent_ndc.x <= 0.0 || extent_ndc.y <= 0.0 {
        return None;
    }
    let (origin, step) = capture_grid(min, extent_ndc, viewport_px);

    let mut fan = Vec::with_capacity(FAN_TRIANGLES.len() * 3);
    for tri in FAN_TRIANGLES.iter() {
        for &idx in tri.iter() {
            let is_center = FOOTPRINT_LOCAL[idx][0] == 0.0 && FOOTPRINT_LOCAL[idx][1] == 0.0;
            fan.push(FanVertex {
                pos_shifted: shifted[idx],
                sample_ndc: unshifted[idx],
                foot_origin: origin,
                capture_step: step,
                vertex_alpha: if is_center { center_alpha } else { 0.0 },
                vertex_rgb: if is_center {
                    FAN_CENTER_VERTEX_RGB
                } else {
                    FAN_RIM_VERTEX_RGB
                },
            });
        }
    }
    Some(fan)
}

fn field_bgl_descriptors() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "distortion_field_bgl0",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                smp_entry(SamplerBindingType::Filtering),
            ),
        ),
    )
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

/// GPU handles shared by the pass (created once at RenderStartup). The scene copy texture is created
/// lazily sized to the view; the fan vertex buffer grows with the number of fields drawing.
#[derive(Resource)]
struct DistortionPassGpu {
    field_shader: Handle<Shader>,
    copy_shader: Handle<Shader>,
    field_bgl0: BindGroupLayoutDescriptor,
    copy_bgl: BindGroupLayoutDescriptor,
    sampler: Sampler,
    scene_copy_texture: Option<Texture>,
    scene_copy_view: Option<TextureView>,
    copy_key: Option<(u32, u32, TextureFormat)>,
    fan_buffer: Buffer,
    fan_capacity_verts: usize,
}

impl DistortionPassGpu {
    fn new(device: &RenderDevice, asset_server: &AssetServer) -> Self {
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("distortion_pass_sampler"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });
        let fan_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("distortion_fan_vertices"),
            size: FAN_VERTEX_BYTES as u64 * VERTICES_PER_FIELD as u64,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            field_shader: asset_server.load("embedded://kuluu_render/distortion.wgsl"),
            copy_shader: asset_server.load("embedded://kuluu_render/distortion_copy.wgsl"),
            field_bgl0: field_bgl_descriptors(),
            copy_bgl: copy_bgl_descriptor(),
            sampler,
            scene_copy_texture: None,
            scene_copy_view: None,
            copy_key: None,
            fan_buffer,
            fan_capacity_verts: VERTICES_PER_FIELD as usize,
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

    /// Recreate the fan buffer when more fields are drawing than it can hold; it never shrinks, so a
    /// burst of hits does not reallocate every frame afterwards.
    fn ensure_fan_capacity(&mut self, device: &RenderDevice, verts_needed: usize) {
        if verts_needed <= self.fan_capacity_verts && self.fan_capacity_verts > 0 {
            return;
        }
        let capacity = verts_needed
            .next_power_of_two()
            .max(VERTICES_PER_FIELD as usize);
        self.fan_buffer.destroy();
        self.fan_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("distortion_fan_vertices"),
            size: FAN_VERTEX_BYTES as u64 * capacity as u64,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.fan_capacity_verts = capacity;
    }
}

/// Four triangles per fan.
const VERTICES_PER_FIELD: u32 = (FAN_TRIANGLES.len() * 3) as u32;

fn fan_vertex_layout() -> VertexBufferLayout {
    VertexBufferLayout::from_vertex_formats(
        VertexStepMode::Vertex,
        [
            VertexFormat::Float32x2,
            VertexFormat::Float32x2,
            VertexFormat::Float32x2,
            VertexFormat::Float32x2,
            VertexFormat::Float32,
            VertexFormat::Float32,
        ],
    )
}

fn field_pipeline_descriptor(
    shader: &Handle<Shader>,
    bgl0: &BindGroupLayoutDescriptor,
    format: TextureFormat,
) -> RenderPipelineDescriptor {
    RenderPipelineDescriptor {
        label: Some("distortion_field".into()),
        layout: vec![bgl0.clone()],
        vertex: VertexState {
            shader: shader.clone(),
            entry_point: Some("vs".into()),
            buffers: vec![fan_vertex_layout()],
            ..Default::default()
        },
        fragment: Some(FragmentState {
            shader: shader.clone(),
            entry_point: Some("fs".into()),
            shader_defs: Vec::new(),
            targets: vec![Some(ColorTargetState {
                format,
                // The fan's own coverage decides how much displaced scene shows; outside it the pixel
                // stays as drawn. Same factors retail uses for this element (src-alpha / inv-src-alpha).
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
            // The fan's winding follows the projection basis, which flips with handedness; nothing
            // here needs back-face culling.
            front_face: FrontFace::Ccw,
            cull_mode: None,
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

/// Core3d sub-schedule (per camera run): copy the processed frame exactly, then draw every live fan in
/// one pass over it. Gated to the operator camera; every other 3D camera (launcher, minimap bake, ...)
/// runs its own Core3d schedule and skips — same gate as [`crate::nameplate_final_pass`].
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

    if pipe_state.as_ref().is_none_or(|(f, _)| *f != format) {
        let field_id = pipeline_cache.queue_render_pipeline(field_pipeline_descriptor(
            &gpu.field_shader,
            &gpu.field_bgl0,
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

    // Every visible field's fan into one vertex stream. A field whose alpha has faded to nothing (or
    // whose footprint fell behind the near plane) contributes no triangles rather than a transparent
    // draw of its own.
    let world_from_view = ev.world_from_view.to_matrix();
    let clip = ev.clip_from_view;
    let viewport_px = Vec2::new(size.width as f32, size.height as f32);
    let now = Instant::now();

    let mut verts: Vec<u8> = Vec::new();
    for field in data.fields.iter() {
        if let Some(fan) = build_fan(world_from_view, clip, field, now, viewport_px) {
            for v in &fan {
                fan_vertex_bytes(v, &mut verts);
            }
        }
    }
    if verts.is_empty() {
        return;
    }
    let vert_count = verts.len() / FAN_VERTEX_BYTES;
    gpu.ensure_fan_capacity(&device, vert_count);
    queue.write_buffer(&gpu.fan_buffer, 0, &verts);

    let bgl0 = pipeline_cache.get_bind_group_layout(&gpu.field_bgl0);
    let field_bg = device.create_bind_group(
        "distortion_field_0",
        &bgl0,
        &BindGroupEntries::sequential((&scene_copy_view, &gpu.sampler)),
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
    pass.set_bind_group(0, &field_bg, &[]);
    pass.set_vertex_buffer(0, gpu.fan_buffer.slice(..));
    pass.draw(0..vert_count as u32, 0..1);
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
        app.init_resource::<ActiveDistortion>().add_systems(
            Update,
            (carry_attached_distortion_fields, prune_distortion_fields).chain(),
        );
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

    fn field(half: Vec2, haze: f32) -> LiveField {
        LiveField {
            center: Vec3::ZERO,
            half_extent: half,
            haze_offset: haze,
            started_at: Instant::now(),
            duration_secs: 1.0,
            envelope: None,
            follow: None,
        }
    }

    /// The eye that has the world origin in front of it. `build_fan` takes the view's own pair of
    /// matrices, and `clip_from_view` is right-handed — its forward axis is −Z — so an eye pushed to +3
    /// on Z looks back along the origin, while one at −3 has the origin behind it.
    fn eye_ahead_of_origin() -> Mat4 {
        Mat4::from_translation(Vec3::new(0.0, 0.0, 3.0))
    }

    /// The fan is four triangles over the built-in footprint — twelve vertices, and every one of them
    /// references the center or a rim corner exactly once around.
    #[test]
    fn fan_covers_the_built_in_footprint_with_four_triangles() {
        let world_from_view = eye_ahead_of_origin();
        let clip = Mat4::perspective_rh(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 500.0);
        let f = field(Vec2::ONE, 0.0);
        let fan =
            build_fan(world_from_view, clip, &f, f.started_at, Vec2::splat(1280.0)).expect("fan");
        assert_eq!(fan.len(), FAN_TRIANGLES.len() * 3);

        // The diamond's own extents: the four rim corners sit left/right/below/above the center.
        let mut xs = fan.iter().map(|v| v.sample_ndc.x).collect::<Vec<_>>();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(xs[0] < 0.0 && *xs.last().unwrap() > 0.0);
        let mut ys = fan.iter().map(|v| v.sample_ndc.y).collect::<Vec<_>>();
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(ys[0] < 0.0 && *ys.last().unwrap() > 0.0);

        // Exactly four center vertices carry the alpha; eight rim vertices are transparent.
        let centers = fan.iter().filter(|v| v.vertex_alpha > 0.0).count();
        assert_eq!(centers, 4);
        assert!(fan.iter().filter(|v| v.vertex_alpha == 0.0).count() == 8);
    }

    /// The haze offset moves the drawn fan on BOTH axes by the same authored amount while leaving the
    /// sampling coordinate of that corner untouched — displacement without a second sample.
    #[test]
    fn haze_shifts_the_draw_on_both_axes_but_never_the_sample() {
        let clip = Mat4::perspective_rh(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 500.0);
        let world = eye_ahead_of_origin();
        let no_haze = field(Vec2::ONE, 0.0);
        let with = field(Vec2::ONE, 0.02);

        let a = build_fan(
            world,
            clip,
            &no_haze,
            no_haze.started_at,
            Vec2::splat(1280.0),
        )
        .unwrap();
        let b = build_fan(world, clip, &with, with.started_at, Vec2::splat(1280.0)).unwrap();

        // Same corner index in both fans (same triangle order), so compare per-corner.
        for i in 0..a.len() {
            let dx = (b[i].pos_shifted.x - a[i].pos_shifted.x).abs();
            let dy = (b[i].pos_shifted.y - a[i].pos_shifted.y).abs();
            assert!(dx > 1e-6, "corner {i} must move horizontally with the haze");
            assert!(dy > 1e-6, "corner {i} must move vertically too");
            assert_eq!(
                a[i].sample_ndc, b[i].sample_ndc,
                "sampling stays put at {i}"
            );
        }

        // And it scales with the authored extent (it is a fraction of the footprint, not of the
        // screen): doubling the footprint doubles how far the fan travels.
        let big = field(Vec2::splat(2.0), 0.02);
        let c = build_fan(world, clip, &big, big.started_at, Vec2::splat(1280.0)).unwrap();
        assert!((c[4].pos_shifted.x - a[4].pos_shifted.x).abs() > 0.0);
    }

    /// The envelope fades coverage only. Two fields with the same authored haze must be displaced by
    /// exactly the same amount regardless of where their alpha track has got to.
    #[test]
    fn haze_translation_does_not_scale_with_the_alpha_track() {
        let clip = Mat4::perspective_rh(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 500.0);
        let world = eye_ahead_of_origin();
        // g142's k143 knots at the precision the record carries them: the DAT stores these as 32-bit
        // floats, so rounding a knot here would move the plateau the test is measuring.
        #[allow(clippy::excessive_precision)]
        let track = ffxi_dat::particle_gen::KeyFrameTrack {
            points: vec![
                (0.0, 0.0),
                (0.26666688919067383, 0.479_999_661_445_617_7),
                (0.7260417938232422, 0.479_999_721_050_262_45),
                (1.0, 0.010_000_495_240_092_278),
            ],
        };
        let mut early = field(Vec2::ONE, 0.02);
        early.envelope = Some(track.clone());
        let late = LiveField {
            envelope: Some(track.clone()),
            ..field(Vec2::ONE, 0.02)
        };

        // Early in life the track has climbed to its plateau; later it has fallen away. The drawn
        // position must not care which of the two it is.
        let at_plateau = build_fan(
            world,
            clip,
            &early,
            early.started_at + Duration::from_secs_f32(0.3 * 1.0),
            Vec2::splat(1280.0),
        );
        let faded = build_fan(
            world,
            clip,
            &late,
            late.started_at + Duration::from_secs_f32(0.95 * 1.0),
            Vec2::splat(1280.0),
        );
        match (at_plateau, faded) {
            (Some(p), Some(d)) => {
                for i in 0..p.len() {
                    assert_eq!(p[i].pos_shifted.x, d[i].pos_shifted.x);
                    assert_eq!(p[i].pos_shifted.y, d[i].pos_shifted.y);
                }
            }
            (Some(_), None) => {
                panic!("a nearly-faded g142 field still draws (k143 ends above zero)")
            }
            _ => panic!("both fields must draw"),
        }

        // Coverage, though, is quantized to a byte and it does fall: the plateau is brighter than
        // the tail, and both are non-zero (which is why the tail still draws above).
        let mut f = field(Vec2::ONE, 0.0);
        f.envelope = Some(track.clone());
        let plateau_byte = f.alpha_byte(f.started_at + Duration::from_secs_f32(0.3));
        let tail_byte = f.alpha_byte(f.started_at + Duration::from_secs_f32(0.95));
        assert!(plateau_byte > 0 && tail_byte > 0);
        assert!(
            plateau_byte > tail_byte,
            "k143 fades coverage: plateau byte {plateau_byte} must sit above the tail byte {tail_byte}"
        );
    }

    /// A field whose alpha track has reached zero contributes no geometry at all.
    #[test]
    fn a_field_at_zero_alpha_draws_nothing() {
        let clip = Mat4::perspective_rh(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 500.0);
        let world = eye_ahead_of_origin();
        let mut f = field(Vec2::ONE, 0.02);
        f.envelope = Some(ffxi_dat::particle_gen::KeyFrameTrack {
            points: vec![(0.0, 0.0), (1.0, 0.0)],
        });
        assert!(build_fan(world, clip, &f, f.started_at, Vec2::splat(1280.0)).is_none());

        // Same field and same view with the track taken away: it is the alpha that produced no
        // geometry here, not the matrices.
        let untracked = LiveField {
            envelope: None,
            ..f
        };
        assert!(build_fan(world, clip, &untracked, f.started_at, Vec2::splat(1280.0)).is_some());
    }

    /// A footprint that has fallen behind the eye contributes nothing — there is no projected box to
    /// copy the scene out of.
    #[test]
    fn a_field_behind_the_eye_draws_nothing() {
        let clip = Mat4::perspective_rh(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 500.0);
        let behind = Mat4::from_translation(Vec3::new(0.0, 0.0, -3.0));
        let f = field(Vec2::ONE, 0.02);
        assert!(build_fan(behind, clip, &f, f.started_at, Vec2::splat(1280.0)).is_none());
    }

    /// The capture grid: one texel per pixel up to the cap, then 255 across however wide the footprint
    /// gets — which is what makes a big field look coarse.
    #[test]
    fn capture_grid_reduces_only_above_the_authored_extent_cap() {
        let viewport = Vec2::new(1280.0, 720.0);
        // Half the screen across (NDC extent 1.0 → 640 px) is over the cap, so the capture holds 255
        // texels and each one covers more than a pixel.
        let (_, big_step) = capture_grid(Vec2::new(-0.5, -0.25), Vec2::new(1.0, 0.5), viewport);
        assert!((big_step.x - 1.0 / CAPTURE_MAX_TEXELS_PER_AXIS).abs() < 1e-6);
        // A footprint exactly at the cap reduces too; one pixel below it keeps a texel per pixel.
        let at_cap_ndc = Vec2::new(
            2.0 * CAPTURE_REDUCED_FROM_EXTENT_PX / viewport.x,
            2.0 * CAPTURE_REDUCED_FROM_EXTENT_PX / viewport.y,
        );
        let (origin, at_cap) = capture_grid(Vec2::ZERO, at_cap_ndc, viewport);
        assert!((at_cap.x - at_cap_ndc.x / CAPTURE_MAX_TEXELS_PER_AXIS).abs() < 1e-6);
        assert_eq!(
            origin,
            Vec2::ZERO,
            "the grid starts at the footprint's own box"
        );
        let under = at_cap_ndc * 0.9;
        let (_, under_step) = capture_grid(Vec2::ZERO, under, viewport);
        let under_px = under.x * 0.5 * viewport.x;
        assert!(
            (under_step.x - under.x / under_px).abs() < 1e-6,
            "below the cap the capture keeps one texel per screen pixel"
        );
    }

    #[test]
    fn fields_expire_on_their_authored_life() {
        let live = field(Vec2::ONE, 0.02);
        assert!(!live.is_expired(Instant::now()));
        let later = live.started_at + Duration::from_secs_f32(1.1);
        assert!(live.is_expired(later));

        let mut d = ActiveDistortion::default();
        assert!(d.fields.is_empty());
        d.push(live);
        assert_eq!(d.fields.len(), 1);
    }

    /// A carried field moves by exactly what its anchor actor moved, and stops being carried once that
    /// actor despawns — leaving the authored life to run out where the field then stands.
    #[test]
    fn a_carried_field_moves_with_its_anchor_and_freezes_without_it() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::default();
        world.init_resource::<ActiveDistortion>();
        let actor = world.spawn(GlobalTransform::from_xyz(0.0, 0.0, 2.0)).id();

        let mut armed = field(Vec2::ONE, 0.0);
        armed.follow = Some(FieldFollow::new(actor));
        world.resource_mut::<ActiveDistortion>().push(armed);

        // First sight of the anchor only records where it stands: an armed field stays at its attach
        // site rather than snapping to the actor.
        let carried = |world: &mut World| {
            world
                .run_system_once(carry_attached_distortion_fields)
                .unwrap()
        };
        carried(&mut world);
        assert_eq!(
            world.resource::<ActiveDistortion>().fields[0].center,
            Vec3::ZERO,
            "the first tick of a carry moves nothing"
        );

        world
            .entity_mut(actor)
            .insert(GlobalTransform::from_xyz(0.0, 0.0, 5.0));
        carried(&mut world);
        assert_eq!(
            world.resource::<ActiveDistortion>().fields[0].center,
            Vec3::new(0.0, 0.0, 3.0),
            "the field follows its anchor's movement"
        );

        world.entity_mut(actor).despawn();
        carried(&mut world);
        let fields = &world.resource::<ActiveDistortion>().fields;
        assert!(
            fields[0].follow.is_none(),
            "a despawned anchor stops carrying the field"
        );
        assert_eq!(
            fields[0].center,
            Vec3::new(0.0, 0.0, 3.0),
            "the field keeps its last position rather than vanishing with the actor"
        );
    }

    /// A footprint hanging off the edge of the screen keeps its own projected coordinates: nothing in
    /// the mapping is clamped to the viewport, so the field neither stretches nor shrinks as it gets
    /// partly cut off. Corners outside NDC are simply not rasterized.
    #[test]
    fn partially_offscreen_field_keeps_its_projected_footprint() {
        // Close enough that the footprint is wider than the frustum: some corners land outside NDC.
        let world_from_view = Mat4::from_translation(Vec3::new(0.0, 0.0, 1.2));
        let clip = Mat4::perspective_rh(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 500.0);
        let f = field(Vec2::ONE, 0.0);
        let fan =
            build_fan(world_from_view, clip, &f, f.started_at, Vec2::splat(1280.0)).expect("fan");

        let off_screen = fan
            .iter()
            .any(|v| v.sample_ndc.x.abs() > 1.0 || v.sample_ndc.y.abs() > 1.0);
        assert!(
            off_screen,
            "this fixture is meant to put corners outside the viewport"
        );

        // Every rim corner equals its straight projection, outside the viewport exactly as inside it.
        for (idx, &local) in FOOTPRINT_LOCAL.iter().enumerate().skip(1) {
            let Some(expected) = project_footprint_point(
                world_from_view,
                clip,
                f.center,
                f.half_extent,
                local,
                Vec2::ZERO,
            ) else {
                panic!("a corner in front of the near plane must project");
            };
            assert!(
                fan.iter().any(|v| v.sample_ndc == expected),
                "corner {idx} projects to {expected:?}, which no fan vertex carries"
            );
        }
    }

    /// The vertex stream is a wire contract with distortion.wgsl: four vec2s then two scalars, little
    /// endian, so a reorder cannot silently desync the two.
    #[test]
    fn fan_vertex_bytes_match_the_wgsl_layout() {
        let v = FanVertex {
            pos_shifted: Vec2::new(-0.5, -0.25),
            sample_ndc: Vec2::new(0.5, 0.75),
            foot_origin: Vec2::new(-1.0, 1.0),
            capture_step: Vec2::new(0.001, 0.002),
            vertex_alpha: 0.48,
            vertex_rgb: 1.0,
        };
        let mut bytes = Vec::new();
        fan_vertex_bytes(&v, &mut bytes);
        assert_eq!(bytes.len(), FAN_VERTEX_BYTES);
        // Every slot of the contract in order: two draw axes, two sampling axes, the box origin, the
        // capture texel step, then the two scalars.
        let float_at = |slot: usize| -> [u8; 4] {
            bytes[slot * 4..(slot + 1) * 4]
                .try_into()
                .expect("one float")
        };
        let contract: [(usize, f32); FAN_VERTEX_FLOATS] = [
            (0, -0.5),
            (1, -0.25),
            (2, 0.5),
            (3, 0.75),
            (4, -1.0),
            (5, 1.0),
            (6, 0.001),
            (7, 0.002),
            (8, 0.48),
            (9, 1.0),
        ];
        for (slot, want) in contract {
            assert_eq!(float_at(slot), want.to_le_bytes(), "slot {slot}");
        }

        let layout = fan_vertex_layout();
        assert_eq!(layout.array_stride as usize, FAN_VERTEX_BYTES);
    }
}
