//! Screen-space distortion (haze/smear) pass — retail's 0x22 `Distortion` generator element
//! (research/xim ParticleLinkedDataProviders.kt DistortionMeshProvider + GLDrawer.kt hazeSwitch).
//! It samples the previous frame's processed output with a horizontal bias so motion leaves a
//! directional ghost, composited over the current frame at low alpha.
//!
//! Scheduled in Core3d AFTER `Core3dSystems::PostProcess` (bloom/DOF/fog/TAA/tonemapping done) and
//! before upscaling writes the window — the same bounds as [`crate::nameplate_final_pass`]. It is a
//! strict no-op unless a distortion generator is alive ([`ActiveDistortion`]).
//!
//! MSAA-safe by construction: bevy's ViewTarget "main" texture is ALWAYS single-sample — under
//! Msaa2/4/8 the geometry pass renders into a separate multi-sample buffer that wgpu resolves INTO
//! this main texture at pass end (ColorAttachment.resolve_target), so after PostProcess the
//! unsampled view holds the fully processed image in every AA mode. Both passes here touch only
//! that single-sample surface; nothing samples a multi-sample buffer.

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
    TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType, TextureUsages,
    TextureView, TextureViewDescriptor, VertexState,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::view::{ExtractedView, ViewTarget};
use bevy::render::{Extract, RenderApp, RenderStartup};

/// Main-world: a distortion generator is alive. Written by `spawn_particle_generators` when a
/// SpawnGenerator stage targets a 0x22 Distortion def; the render pass extracts it each frame.
#[derive(Resource)]
pub struct ActiveDistortion {
    /// sec2 0x32 HazeOffsetInitializer horizontal offset — biases the smear direction.
    pub haze_offset_x: f32,
    /// When the generator's life ends; `None` means never active.
    pub expires_at: Option<Instant>,
    /// The sec2 0x2D strength/alpha envelope over life, PS2-rescaled at spawn (g142 binds k143:
    /// 0 -> full hold -> ~0). `None` = constant strength.
    pub envelope: Option<ffxi_dat::particle_gen::KeyFrameTrack>,
    /// When the generator's life started — progress runs from here over [`Self::duration_secs`].
    pub started_at: Instant,
    /// The generator's maxLifeSpan in seconds (life frames / 60).
    pub duration_secs: f32,
    /// Envelope value at this frame, maintained by `update_distortion_strength`; the render
    /// pass scales both the offset and the ghost intensity by it.
    pub strength: f32,
}

impl Default for ActiveDistortion {
    fn default() -> Self {
        Self {
            haze_offset_x: 0.0,
            expires_at: None,
            envelope: None,
            started_at: Instant::now(),
            duration_secs: 0.0,
            strength: 0.0,
        }
    }
}

impl ActiveDistortion {
    fn is_active(&self) -> bool {
        self.expires_at.is_some_and(|t| Instant::now() < t)
    }
}

/// Ghost alpha at full envelope strength; the authored k143 peak (PS2-rescaled ~0.95) lands
/// just under it.
const GHOST_BASE_INTENSITY: f32 = 0.25;

/// Main-world Update: advance the envelope while a distortion generator is alive. The render
/// pass only reads `strength`, so the curve lives on this side of the extract.
fn update_distortion_strength(mut d: ResMut<ActiveDistortion>) {
    let Some(exp) = d.expires_at else {
        d.strength = 0.0;
        return;
    };
    if Instant::now() >= exp {
        d.strength = 0.0;
        return;
    }
    let progress =
        ((Instant::now() - d.started_at).as_secs_f32() / d.duration_secs.max(1e-6)).clamp(0.0, 1.0);
    d.strength = d
        .envelope
        .as_ref()
        .map(|t| t.sample(progress))
        .unwrap_or(1.0);
}

/// Render-world snapshot of the live distortion (extracted from [`ActiveDistortion`]).
#[derive(Resource, Default)]
struct DistortionPassData {
    active: bool,
    haze_offset_x: f32,
    strength: f32,
    /// The operator camera's main-world entity — only its primary view is distorted.
    operator_cam: Option<Entity>,
}

fn extract_distortion(
    mut data: ResMut<DistortionPassData>,
    src: Extract<Res<ActiveDistortion>>,
    operator_cameras: Extract<Query<Entity, With<crate::camera::OperatorCamera>>>,
) {
    data.active = src.is_active();
    data.haze_offset_x = src.haze_offset_x;
    data.strength = src.strength;
    data.operator_cam = operator_cameras.iter().next();
}

/// Per-pass uniform. Byte layout must match distortion.wgsl `PassUniform` (vec2 + 2 f32 = 16 B).
#[derive(ShaderType, Clone, Copy)]
struct DistortionUniform {
    offset: Vec2,
    intensity: f32,
    copy_mode: f32,
}

/// Shader asset path. The file is embedded by the `embedded_asset!` call in
/// `DistortionPassPlugin::build` (distortion_pass.rs), so the path is fixed to that
/// embedded copy.
const DISTORTION_SHADER_PATH: &str = "embedded://kuluu_render/distortion.wgsl";

const DISTORTION_UNIFORM_SIZE: u64 = 16;

/// [offset.xy (8 B)][intensity f32 @ 8][copy_mode f32 @ 12] — the exact layout distortion.wgsl's
/// PassUniform reads. Factored out so a unit test pins it.
fn distortion_uniform_bytes(u: DistortionUniform) -> [u8; DISTORTION_UNIFORM_SIZE as usize] {
    let mut bytes = [0u8; DISTORTION_UNIFORM_SIZE as usize];
    bytes[0..4].copy_from_slice(&u.offset.x.to_le_bytes());
    bytes[4..8].copy_from_slice(&u.offset.y.to_le_bytes());
    bytes[8..12].copy_from_slice(&u.intensity.to_le_bytes());
    bytes[12..16].copy_from_slice(&u.copy_mode.to_le_bytes());
    bytes
}

fn distortion_bgl_descriptor() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "distortion_pass_bgl",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                uniform_buffer::<DistortionUniform>(false),
                texture_2d(TextureSampleType::Float { filterable: true }),
                smp_entry(SamplerBindingType::Filtering),
            ),
        ),
    )
}

/// GPU handles shared by the pass (created once at RenderStartup). The previous-frame texture is
/// created lazily on first use, sized to the view.
#[derive(Resource)]
struct DistortionPassGpu {
    shader: Handle<Shader>,
    /// Shared between bind groups AND the pipeline descriptor (single source of truth for the
    /// layout shape) — same pattern as [`crate::nameplate_final_pass`].
    bgl_descriptor: BindGroupLayoutDescriptor,
    sampler: Sampler,
    ghost_uniform: Buffer,
    capture_uniform: Buffer,
    prev_texture: Option<Texture>,
    prev_view: Option<TextureView>,
    prev_key: Option<(u32, u32, TextureFormat)>,
}

impl DistortionPassGpu {
    fn new(device: &RenderDevice, asset_server: &AssetServer) -> Self {
        let bgl_descriptor = distortion_bgl_descriptor();
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("distortion_pass_sampler"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });
        let make_uniform = |label| {
            device.create_buffer(&BufferDescriptor {
                label: Some(label),
                size: DISTORTION_UNIFORM_SIZE,
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Self {
            shader: asset_server.load(DISTORTION_SHADER_PATH),
            bgl_descriptor,
            sampler,
            ghost_uniform: make_uniform("distortion_ghost_uniform"),
            capture_uniform: make_uniform("distortion_capture_uniform"),
            prev_texture: None,
            prev_view: None,
            prev_key: None,
        }
    }

    /// Ensure the previous-frame texture exists at (w, h, format); recreate on any change. The old
    /// view is dropped (refcounted) and the old texture destroyed explicitly.
    fn ensure_prev(&mut self, device: &RenderDevice, size: Extent3d, format: TextureFormat) {
        let key = (size.width, size.height, format);
        if self.prev_key == Some(key) && self.prev_texture.is_some() {
            return;
        }
        self.prev_view = None;
        if let Some(old) = self.prev_texture.take() {
            old.destroy();
        }
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("distortion_prev_frame"),
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
            label: Some("distortion_prev_frame_view"),
            ..Default::default()
        });
        self.prev_key = Some(key);
        self.prev_texture = Some(texture);
        self.prev_view = Some(view);
    }
}

fn distortion_pipeline_descriptor(
    shader: &Handle<Shader>,
    bgl: &BindGroupLayoutDescriptor,
    format: TextureFormat,
) -> RenderPipelineDescriptor {
    RenderPipelineDescriptor {
        label: Some("distortion_pass".into()),
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
                // Standard alpha blend (the old wgpu `BlendState::Alpha` constant):
                // color = src*srcA + dst*(1-srcA), alpha = src + dst*(1-srcA).
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

/// Core3d sub-schedule (per camera run): ghost last frame's processed output over the CURRENT view
/// with the horizontal haze bias, then capture the result for next frame. Gated to the operator
/// camera; every other 3D camera (launcher, minimap bake, ...) runs its own Core3d schedule and
/// skips — same gate as [`crate::nameplate_final_pass`].
#[allow(clippy::type_complexity)]
fn draw_distortion_pass(
    view: ViewQuery<(&ExtractedView, &ViewTarget)>,
    data: Res<DistortionPassData>,
    mut gpu: ResMut<DistortionPassGpu>,
    device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
    queue: Res<RenderQueue>,
    mut ctx: RenderContext,
    mut pipe_state: Local<Option<(TextureFormat, CachedRenderPipelineId)>>,
    mut was_active: Local<bool>,
) {
    if !data.active {
        *was_active = false;
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

    // First active frame after an inactive gap: prev holds stale content — capture only, so the
    // ghost starts from a clean slate instead of flashing old frames.
    let format = target.main_texture_format();
    let size = target.main_texture().size();
    let first_frame = !*was_active || gpu.prev_key != Some((size.width, size.height, format));
    gpu.ensure_prev(&device, size, format);
    let Some(prev_view) = &gpu.prev_view else {
        return;
    };

    if pipe_state.as_ref().is_none_or(|(f, _)| *f != format) {
        let id = pipeline_cache.queue_render_pipeline(distortion_pipeline_descriptor(
            &gpu.shader,
            &gpu.bgl_descriptor,
            format,
        ));
        *pipe_state = Some((format, id));
    }
    let (_, pipeline_id) = pipe_state.expect("set above");
    let Some(pipeline) = pipeline_cache.get_render_pipeline(pipeline_id) else {
        return;
    };

    let bgl = pipeline_cache.get_bind_group_layout(&gpu.bgl_descriptor);

    // Pass A: ghost — sample last frame (prev) with the horizontal bias over the current
    // target. Both the offset and the ghost alpha scale by the sec2 0x2D envelope strength.
    if !first_frame {
        queue.write_buffer(
            &gpu.ghost_uniform,
            0,
            &distortion_uniform_bytes(DistortionUniform {
                offset: Vec2::new(data.haze_offset_x * data.strength, 0.0),
                intensity: GHOST_BASE_INTENSITY * data.strength,
                copy_mode: 0.0,
            }),
        );
        let ghost_bg = device.create_bind_group(
            "distortion_ghost",
            &bgl,
            &BindGroupEntries::sequential((
                BufferBinding {
                    buffer: &gpu.ghost_uniform,
                    offset: 0,
                    size: None,
                },
                prev_view,
                &gpu.sampler,
            )),
        );
        let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("distortion_ghost"),
            color_attachments: &[Some(target.get_unsampled_color_attachment())],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_render_pipeline(pipeline);
        pass.set_bind_group(0, &ghost_bg, &[]);
        pass.draw(0..3, 0..1);
    }

    // Pass B: capture — copy the current target into prev for next frame's ghost.
    queue.write_buffer(
        &gpu.capture_uniform,
        0,
        &distortion_uniform_bytes(DistortionUniform {
            offset: Vec2::ZERO,
            intensity: 1.0,
            copy_mode: 1.0,
        }),
    );
    let capture_bg = device.create_bind_group(
        "distortion_capture",
        &bgl,
        &BindGroupEntries::sequential((
            BufferBinding {
                buffer: &gpu.capture_uniform,
                offset: 0,
                size: None,
            },
            target.main_texture_view(),
            &gpu.sampler,
        )),
    );
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("distortion_capture"),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: prev_view,
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
    pass.set_render_pipeline(pipeline);
    pass.set_bind_group(0, &capture_bg, &[]);
    pass.draw(0..3, 0..1);

    *was_active = true;
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
        app.init_resource::<ActiveDistortion>()
            .add_systems(Update, update_distortion_strength);
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<DistortionPassData>()
                .add_systems(ExtractSchedule, extract_distortion)
                .add_systems(RenderStartup, init_distortion_pass_gpu)
                .add_systems(
                    Core3d,
                    draw_distortion_pass
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

    /// The uniform byte layout is a wire contract with distortion.wgsl — pin it so a field
    /// reorder cannot silently desync the two.
    #[test]
    fn uniform_bytes_match_the_wgsl_layout() {
        let bytes = distortion_uniform_bytes(DistortionUniform {
            offset: Vec2::new(0.5, -1.0),
            intensity: 0.25,
            copy_mode: 1.0,
        });
        assert_eq!(bytes[0..4], 0.5f32.to_le_bytes());
        assert_eq!(bytes[4..8], (-1.0f32).to_le_bytes());
        assert_eq!(bytes[8..12], 0.25f32.to_le_bytes());
        assert_eq!(bytes[12..16], 1.0f32.to_le_bytes());
    }

    #[test]
    fn active_distortion_expires() {
        let mut d = ActiveDistortion::default();
        assert!(!d.is_active());
        d.expires_at = Some(Instant::now() + std::time::Duration::from_millis(50));
        assert!(d.is_active());
        d.expires_at = Some(Instant::now() - std::time::Duration::from_millis(1));
        assert!(!d.is_active());
    }
}
