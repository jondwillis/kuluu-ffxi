//! Nameplates as a final in-view pass — the retired `NameplateOverlayCamera`,
//! rebuilt on top of the view pipeline instead of as a second camera.
//!
//! Retail draws names to the backbuffer AFTER the scene and its effects
//! (research/XIClient/.../CXiActorNameDraw.cpp). The old kuluu equivalent was
//! a second `Camera3d` sharing the operator's render target: correct pixel
//! order, but in Bevy 0.19 every camera owns its own Core3d schedule run
//! against the target's shared A/B main-texture double buffer — so plates cost
//! an extra clear/effects/upscale cycle per screen, with two cameras'
//! bookkeeping fighting over one swapchain write. The whole-frame ghost that
//! grows with movement speed (windowed AND fullscreen, MSAA off) is exactly
//! what that shared-flip machinery produces when it races itself.
//!
//! This pass draws the plates directly into the operator view's processed main
//! texture instead: scheduled in the Core3d sub-schedule AFTER all of
//! `Core3dSystems::PostProcess` (bloom/DOF/fog/TAA/tonemapping are done —
//! "ignore the effects" is structural, not opt-in) and BEFORE `upscaling`
//! writes the window. Scene depth survives from the main pass
//! (`StoreOp::Load`, no re-clear), so walls still occlude plates; plate
//! entities keep their layer-4 marker + unit-quad mesh (core_3d skips them —
//! the operator camera's render layers exclude 4); and
//! `update_nameplate_billboards_system` is untouched, this module only reads
//! its per-frame output. One draw call per plate (~80 plates), CPU far-to-near
//! sort for blend order, no instancing needed at that count.
//!
//!
//! Occlusion: single-sample views attach the main pass's depth buffer (`StoreOp::Load`,
//! no re-clear) and test it in hardware; with MSAA on the view's multi-sample depth
//! buffer cannot sit beside the 1× processed color image, so the fragment shader reads
//! that pixel's sub-sampled scene depths instead (per-sample textureLoad over a multisample
//! `texture_depth_multisampled_2d`, same mechanism bevy's depth-prepass mesh shaders use)
//! and discards where any of them is nearer. Under an upscaler (DLSS sets
//! MainPassResolutionOverride) the main pass renders into a top-left sub-rect at render
//! resolution, so neither test above is sound against the full-size depth buffer: this
//! pass instead binds the single-sample scene depth as a texture and does one nearest
//! load per fragment at `fragment_coord * (render_res / target_size)` — every fragment
//! then lands inside the sub-rect where valid geometry lives, so walls still occlude
//! plates post-upscale. Either way the read needs the operator camera to carry
//! `TEXTURE_BINDING` on its depth texture usage, set in `build_operator_camera`
//! (camera.rs) — bevy only adds it itself for cameras with OcclusionCulling.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use bevy::asset::{embedded_asset, AssetServer, Handle};
use bevy::camera::MainPassResolutionOverride;
use bevy::core_pipeline::{
    core_3d::CORE_3D_DEPTH_FORMAT, upscaling::upscaling, Core3d, Core3dSystems,
};
use bevy::image::Image;
use bevy::math::{FloatOrd, Mat4, Vec2, Vec3};
use bevy::mesh::VertexBufferLayout;
use bevy::prelude::*;
use bevy::render::render_resource::{
    binding_types::{
        sampler as smp_entry, texture_2d, texture_depth_2d, texture_depth_2d_multisampled,
        uniform_buffer,
    },
    BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendComponent,
    BlendFactor, BlendOperation, BlendState, Buffer, BufferBinding, BufferDescriptor,
    BufferInitDescriptor, BufferUsages, CachedRenderPipelineId, ColorTargetState, ColorWrites,
    CompareFunction, DepthBiasState, DepthStencilState, FragmentState, FrontFace, IndexFormat,
    MultisampleState, PipelineCache, PolygonMode, PrimitiveState, PrimitiveTopology,
    RenderPassDescriptor, RenderPipelineDescriptor, SamplerBindingType, ShaderStages, ShaderType,
    StencilFaceState, StencilState, StoreOp, TextureFormat, TextureSampleType, VertexFormat,
    VertexState, VertexStepMode,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::{
    render_asset::RenderAssets,
    texture::GpuImage,
    view::{ExtractedView, Msaa, ViewDepthTexture, ViewTarget},
    Extract, Render, RenderApp, RenderStartup, RenderSystems,
};

use crate::camera::OperatorCamera;
use crate::nameplate_billboard::NameplateBillboard;

// ---------------------------------------------------------------------------
// GPU data
// ---------------------------------------------------------------------------

/// Per-plate uniform. Byte layout must match nameplate_final.wgsl `PlateUniform`:
/// model mat4 @ 0 (64 B), fade alpha f32 @ 64, padded to [`PLATE_UNIFORM_SIZE`].
#[derive(ShaderType, Clone, Copy)]
pub struct PlateUniform {
    pub model: Mat4,
    pub fade_alpha: f32,
}

/// Per-view uniform for this frame's pass run: the clip matrix of the view
/// currently being drawn (always the operator camera — see the gate in the draw system),
/// that projection's near plane, and the render-res/full-res scale the fragment stage
/// uses to map a full-res fragment onto the depth sub-rect under an upscaler.
#[derive(ShaderType, Clone, Copy)]
pub struct ViewUniform {
    pub clip_from_world: Mat4,
    pub near: f32,
    /// (render_res / target_size) per axis; (1.0, 1.0) when no upscaler is active.
    pub subrect_scale: Vec2,
}

const PLATE_UNIFORM_SIZE: u32 = 80;
/// Byte layout of the view uniform (see nameplate_final.wgsl `ViewUniform`):
/// clip matrix @ 0, near f32 @ 64, subrect_scale vec2 @ 72. The buffer must be at
/// least encase's `min_size()` for ViewUniform — Mat4(64) + f32 + vec2 (align 8,
/// padded to offset 72) rounded up to the 16-byte struct alignment = 80; binding a
/// shorter slice fails wgpu validation with "Binding size ... less than minimum".
/// Same padding rule as PlateUniform.
const VIEW_UNIFORM_SIZE: u32 = 80;

/// The unit-quad geometry. Positions/uvs are the bevy_mesh `Rectangle` verbatim
/// (src/primitives/dim2.rs), so a plate projects identically to what core_3d
/// used to draw with the overlay camera. Interleaved pos(3)+uv(2) = 20 B stride.
const PLATE_VERTEX_DATA: [[f32; 5]; 4] = [
    [0.5, 0.5, 0.0, 1.0, 0.0],
    [-0.5, 0.5, 0.0, 0.0, 0.0],
    [-0.5, -0.5, 0.0, 0.0, 1.0],
    [0.5, -0.5, 0.0, 1.0, 1.0],
];

const PLATE_INDEX_DATA: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// One bind-group entry set per plate: uniform @0 (this plate), view uniform @1
/// (shared buffer), texture @2 + sampler @3 — matches nameplate_final.wgsl.
fn plate_bgl_descriptor() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "nameplate_final_pass_bgl",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                uniform_buffer::<PlateUniform>(false).visibility(ShaderStages::VERTEX),
                // VERTEX_FRAGMENT: vs reads clip_from_world, and the MSAA
                // manual-depth-test path in fs reads .near to turn stored
                // reversed-Z depth into a view distance. Was VERTEX-only until
                // the fragment gained that read — the layout must cover every
                // stage that touches the binding or wgpu rejects the pipeline
                // ("group 0 binding 1 not available … visibility flags don't
                // include the shader stage").
                uniform_buffer::<ViewUniform>(false).visibility(ShaderStages::VERTEX_FRAGMENT),
                texture_2d(TextureSampleType::Float { filterable: true }),
                smp_entry(SamplerBindingType::Filtering),
            ),
        ),
    )
}

// Bind-group entry for group 1 — the view's scene depth, shared by every plate of a
// tick (one bind group, rebuilt per frame while MSAA is on). The single-sample variant
// leaves this slot unbound and tests the attached depth buffer in hardware instead.
fn depth_bgl_descriptor() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "nameplate_final_pass_depth_bgl",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            // Multi-sample scene depth, read with per-sample textureLoad in the
            // fragment shader (see nameplate_final.wgsl). textureLoad needs no
            // sampler, so this group is a lone texture binding. Pairs with
            // bevy's own prepass BGL entry for the same buffer. (One-element
            // tuple keeps the same `sequential` constructor the rest of the
            // file uses.)
            (texture_depth_2d_multisampled(),),
        ),
    )
}

/// Group-1 layout for the upscaler sub-rect test: the SAME scene depth buffer,
/// single-sample, bound as a plain texture the fragment stage loads at scaled
/// coordinates. Same lone-texture shape as [`depth_bgl_descriptor`].
fn ss_depth_bgl_descriptor() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "nameplate_final_pass_ss_depth_bgl",
        &BindGroupLayoutEntries::sequential(ShaderStages::FRAGMENT, (texture_depth_2d(),)),
    )
}

/// GPU handles shared by the whole pass (created once at RenderStartup).
#[derive(Resource)]
pub struct NameplatePassGpu {
    pub shader: Handle<Shader>,
    /// Shared between bind groups AND the pipeline descriptor (single source of
    /// truth for the layout shape).
    pub bgl_descriptor: BindGroupLayoutDescriptor,
    /// Group 1: this view's multi-sample scene depth (a lone texture — the
    /// per-sample textureLoad path needs no sampler).
    pub depth_bgl_descriptor: BindGroupLayoutDescriptor,
    /// Group 1 for the upscaler sub-rect test: the same buffer, single-sample.
    pub ss_depth_bgl_descriptor: BindGroupLayoutDescriptor,
    pub vertex_buffer: Buffer,
    pub index_buffer: Buffer,
    /// Written once per frame by the draw system (the current view's clip matrix);
    /// every bind group references it at offset 0.
    pub view_uniforms: Buffer,
}

impl NameplatePassGpu {
    fn new(device: &RenderDevice, asset_server: &AssetServer) -> Self {
        let bgl_descriptor = plate_bgl_descriptor();
        let depth_bgl_descriptor = depth_bgl_descriptor();
        let ss_depth_bgl_descriptor = ss_depth_bgl_descriptor();

        let mut vertex_bytes: Vec<u8> = Vec::with_capacity(PLATE_VERTEX_DATA.len() * 20);
        for v in PLATE_VERTEX_DATA {
            for f in v {
                vertex_bytes.extend_from_slice(&f.to_le_bytes());
            }
        }
        let mut index_bytes: Vec<u8> = Vec::with_capacity(PLATE_INDEX_DATA.len() * 4);
        for i in PLATE_INDEX_DATA {
            index_bytes.extend_from_slice(&i.to_le_bytes());
        }

        let vertex_buffer = device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("nameplate_final_pass_vertices"),
            contents: &vertex_bytes,
            usage: BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("nameplate_final_pass_indices"),
            contents: &index_bytes,
            usage: BufferUsages::INDEX,
        });
        let view_uniforms = device.create_buffer(&BufferDescriptor {
            label: Some("nameplate_final_pass_view_uniforms"),
            size: VIEW_UNIFORM_SIZE as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shader = asset_server.load::<Shader>("embedded://kuluu_render/nameplate_final.wgsl");

        Self {
            shader,
            bgl_descriptor,
            depth_bgl_descriptor,
            ss_depth_bgl_descriptor,
            vertex_buffer,
            index_buffer,
            view_uniforms,
        }
    }
}

/// Pipeline descriptor. Keyed on (target format, depth mode, MSAA sample count):
/// this pass always draws single-sample AFTER all effects (the current main side
/// holds the resolved, processed image) and depth is Bevy's fixed 3D format
/// everywhere. `Hardware` attaches the scene depth for a hardware GreaterEqual test;
/// `Gather` (MSAA on — the multi-sample buffer cannot sit beside a 1× color attachment
/// in one pass) compiles the MANUAL_DEPTH_TEST shader def, moving the same test into
/// the fragment stage where per-sample textureLoad reads the pixel's sub-sampled scene
/// depths; `Subrect` (an upscaler is active — MainPassResolutionOverride present)
/// binds the single-sample scene depth as a texture and tests it in the fragment stage
/// at render-res sub-rect coordinates: under SR the main pass only wrote geometry into
/// the top-left render-resolution corner of the full-size depth buffer, so every
/// full-res fragment is mapped down into that corner before the load — walls still
/// occlude plates post-upscale. Group 1 (the depth binding) is set on the Gather and
/// Subrect runs only.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PlateDepthMode {
    /// Single-sample view: scene depth attached, hardware GreaterEqual.
    Hardware,
    /// MSAA view: no attachment, shader-side per-sample textureLoad test (group 1).
    Gather,
    /// Upscaled view: shader-side nearest load at the render-res sub-rect pixel
    /// (group 1; single-sample depth unless a future upscaler runs multisampled).
    Subrect,
}

fn plate_pipeline_descriptor(
    shader: &Handle<Shader>,
    bgl: &BindGroupLayoutDescriptor,
    depth_bgl: &BindGroupLayoutDescriptor,
    ss_depth_bgl: &BindGroupLayoutDescriptor,
    target_format: TextureFormat,
    mode: PlateDepthMode,
    // MSAA sample count — the manual variants read exactly this many sub-samples
    // per pixel via textureLoad when > 1 (WGSL forbids textureGather on
    // multisampled depth); Subrect with samples == 1 takes the single-load path
    // instead. Bevy's Msaa is 1/2/4/8; the shader falls back to a 4-sample loop
    // if no MSAA_SAMPLES_N def is set.
    samples: u32,
) -> RenderPipelineDescriptor {
    // Compiled into the manual-occlusion variants only; Hardware never references
    // group 1. A bare def name is a Bool(name, true) — exactly what #ifdef in the
    // shader expects. Subrect adds SUBRECT_SCALE (map fragment coords into the
    // render-res sub-rect) and SINGLE_SAMPLE_DEPTH when the view is single-sample
    // (the upscaler case today: DLSS forces MSAA off).
    let mut shader_defs: Vec<bevy::shader::ShaderDefVal> = match mode {
        PlateDepthMode::Hardware => Vec::new(),
        PlateDepthMode::Gather | PlateDepthMode::Subrect => vec!["MANUAL_DEPTH_TEST".into()],
    };
    if matches!(mode, PlateDepthMode::Subrect) {
        shader_defs.push("SUBRECT_SCALE".into());
        if samples == 1 {
            shader_defs.push("SINGLE_SAMPLE_DEPTH".into());
        }
    }
    let manual_loop = !matches!(mode, PlateDepthMode::Hardware)
        && !(matches!(mode, PlateDepthMode::Subrect) && samples == 1);
    if manual_loop {
        // Pick the sample-count def that matches this view. Bevy only ever
        // reports 1/2/4/8; 2 and 8 need a def, 4 is the shader's fallback so
        // no def emitted. Any other value silently uses the 4-sample fallback
        // rather than panicking.
        match samples {
            2 => shader_defs.push("MSAA_SAMPLES_2".into()),
            8 => shader_defs.push("MSAA_SAMPLES_8".into()),
            _ => {}
        }
    }
    let vertex_layout = VertexBufferLayout::from_vertex_formats(
        VertexStepMode::Vertex,
        [VertexFormat::Float32x3, VertexFormat::Float32x2],
    );

    // Premultiplied blend (ONE / ONE_MINUS_SRC_ALPHA) over the processed scene —
    // identical to what AlphaMode::Premultiplied produced in core_3d.
    let premult = BlendComponent {
        operation: BlendOperation::Add,
        src_factor: BlendFactor::One,
        dst_factor: BlendFactor::OneMinusSrcAlpha,
    };

    RenderPipelineDescriptor {
        label: Some("nameplate_final_pass".into()),
        // Only Gather and Subrect bind group 1 (the scene depth). Hardware leaves
        // slot 1 unbound at draw time, so declaring a depth BGL in its layout makes
        // wgpu reject the draw with "the current set RenderPipeline expects a
        // BindGroup to be set at index 1". Layout must match actual bindings, not
        // the union across modes; Subrect picks the single-sample BGL unless a
        // future upscaler runs multisampled.
        layout: {
            let depth_layout = match mode {
                PlateDepthMode::Hardware => None,
                // Gather only ever runs with samples > 1 (mode selection below).
                PlateDepthMode::Gather => Some(depth_bgl.clone()),
                PlateDepthMode::Subrect if samples > 1 => Some(depth_bgl.clone()),
                PlateDepthMode::Subrect => Some(ss_depth_bgl.clone()),
            };
            match depth_layout {
                Some(d) => vec![bgl.clone(), d],
                None => vec![bgl.clone()],
            }
        },
        immediate_size: 0,
        vertex: VertexState {
            shader: shader.clone(),
            shader_defs: shader_defs.clone(),
            entry_point: Some("vs".into()),
            buffers: vec![vertex_layout],
        },
        primitive: PrimitiveState {
            topology: PrimitiveTopology::TriangleList,
            front_face: FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: PolygonMode::Fill,
            ..Default::default()
        },
        // Bevy's depth is reversed-Z (closer = LARGER), so nearer-geometry-occludes
        // tests GreaterEqual — the same convention the skybox and zone materials use.
        // No depth WRITE: plate-vs-plate order comes from the CPU sort, not the buffer.
        // (The old material carried a huge depth_bias for TRANSPARENT-phase sorting;
        // nothing equivalent is needed here — first frame of A/B will confirm no
        // co-planar z-fight against zone geometry.)
        depth_stencil: if matches!(mode, PlateDepthMode::Hardware) {
            Some(DepthStencilState {
                format: CORE_3D_DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: StencilState {
                    front: StencilFaceState::IGNORE,
                    back: StencilFaceState::IGNORE,
                    read_mask: 0,
                    write_mask: 0,
                },
                bias: DepthBiasState {
                    constant: 0,
                    slope_scale: 0.0,
                    clamp: 0.0,
                },
            })
        } else {
            None
        },
        multisample: MultisampleState {
            count: 1,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        fragment: Some(FragmentState {
            shader: shader.clone(),
            shader_defs,
            entry_point: Some("fs".into()),
            targets: vec![Some(ColorTargetState {
                format: target_format,
                blend: Some(BlendState {
                    color: premult,
                    alpha: premult,
                }),
                write_mask: ColorWrites::ALL,
            })],
        }),
        zero_initialize_workgroup_memory: true,
    }
}

// ---------------------------------------------------------------------------
// Per-frame per-plate state (render world)
// ---------------------------------------------------------------------------

/// CPU description of one drawable plate for the current frame. `matrix` is the
/// billboard's GlobalTransform — the unit quad scaled to world size and rotated
/// camera-facing, exactly what core_3d used to draw through the overlay camera.
pub struct PlateDraw {
    entity: Entity,
    matrix: Mat4,
    /// World position of the plate center; only used for view-depth sorting.
    center: Vec3,
    alpha: f32,
    texture_handle: Handle<Image>,
    binding: Option<PlateBinding>,
}

/// GPU handles for one plate. The uniform buffer contents are rewritten every
/// frame by `prepare_nameplate_bindings` (matrix/alpha change with movement).
#[derive(Clone)]
pub struct PlateBinding {
    uniforms: Buffer,
    bind_group: BindGroup,
}

/// Render-side description of everything drawable this frame, plus the one view
/// the pass may run under. Built by `extract_nameplates` (ExtractSchedule),
/// bindings attached in `prepare_nameplate_bindings`.
#[derive(Resource, Default)]
pub struct NameplateFinalPassData {
    pub operator_cam: Option<Entity>,
    pub plates: Vec<PlateDraw>,
}

// ---------------------------------------------------------------------------
// Debug menu mirror (render thread -> main-world HUD panel)
// ---------------------------------------------------------------------------

/// One render tick's worth of pass state, for the Debug menu "Nameplate
/// Debug" panel. Counters cover every stage between a billboard existing in
/// the scene and its quad actually drawn: extract skips, GPU texture cache
/// misses, bind failures, pipeline readiness, and where the farthest plate
/// lands in clip space (the on-screen check).
#[derive(Clone, Copy)]
pub struct NameplateFrameSnap {
    pub plates_total: u32,
    /// Culled upstream (depth ramp / self-plate first-person) — never GPU-bound.
    pub hidden: u32,
    /// Plate texture not in the render world's GpuImage cache yet.
    pub no_gpu_image: u32,
    /// GpuImage exists but its data hasn't uploaded yet.
    pub not_had_data: u32,
    /// Plates that got a bind group + uniform write this tick.
    pub bound: u32,
    /// Render world GPU image cache size (texture-churn context).
    pub gpu_images_total: u32,
    /// Quads actually drawn into the operator view this tick.
    pub draws: u32,
    /// Pipeline was ready when the draw system ran this tick.
    pub pipeline_ready: bool,
    /// Operator view color attachment format (None = the draw stage never ran).
    pub target_fmt: Option<TextureFormat>,
    /// Operator view sample count (>1 means MSAA on — shader-side gather test
    /// instead of the hardware depth attachment).
    pub samples: u32,
    /// An operator camera existed when extract ran this tick.
    pub operator_cam: bool,
    /// The draw stage reached the operator view this tick.
    pub reached_draw: bool,
    /// Farthest plate (first in blend order) clip position. ndc xy inside
    /// [-1, 1] with w > 0 = on screen and in front of the camera.
    pub far_ndc_x: f32,
    pub far_ndc_y: f32,
    pub far_w: f32,
    /// That plate's fade alpha (0 = fully faded out).
    pub far_alpha: f32,
    // Billboard visibility breakdown mirrored from the main-world update
    // system (NameplateBillboardDebug) — answers "why is `hidden` what it is".
    /// Billboard entities present in the main world this frame.
    pub bb_total: u32,
    /// Self-plate camera-mode cull.
    pub bb_hide_self: u32,
    /// Behind-camera-plane / within 1 yalm gate.
    pub bb_hidden_depth: u32,
    /// Plates set Visible this frame (== plates that could be drawn).
    pub bb_visible: u32,
    /// Billboards despawned because their actor is gone.
    pub bb_despawned: u32,
}

impl Default for NameplateFrameSnap {
    fn default() -> Self {
        Self {
            plates_total: 0,
            hidden: 0,
            no_gpu_image: 0,
            not_had_data: 0,
            bound: 0,
            gpu_images_total: 0,
            draws: 0,
            pipeline_ready: false,
            target_fmt: None,
            samples: 0,
            operator_cam: false,
            reached_draw: false,
            far_ndc_x: 0.0,
            far_ndc_y: 0.0,
            far_w: 0.0,
            far_alpha: 0.0,
            bb_total: 0,
            bb_hide_self: 0,
            bb_hidden_depth: 0,
            bb_visible: 0,
            bb_despawned: 0,
        }
    }
}

/// Ring of the last two completed ticks for the panel: `prev` is the previous
/// frame, `cur` fills during this tick and rotates on extract's start.
#[derive(Default)]
pub struct NameplatePassDebug {
    pub frame: u64,
    pub cur: NameplateFrameSnap,
    pub prev: NameplateFrameSnap,
}

/// All-zero snapshot (the pass has not produced a tick yet) — const so the
/// static below initializes in place (`Mutex::new` is const on this toolchain).
const NAMEPLATE_SNAP_ZERO: NameplateFrameSnap = NameplateFrameSnap {
    plates_total: 0,
    hidden: 0,
    no_gpu_image: 0,
    not_had_data: 0,
    bound: 0,
    gpu_images_total: 0,
    draws: 0,
    pipeline_ready: false,
    target_fmt: None,
    samples: 0,
    operator_cam: false,
    reached_draw: false,
    far_ndc_x: 0.0,
    far_ndc_y: 0.0,
    far_w: 0.0,
    far_alpha: 0.0,
    bb_total: 0,
    bb_hide_self: 0,
    bb_hidden_depth: 0,
    bb_visible: 0,
    bb_despawned: 0,
};

/// Render-thread owned; read by the main-world HUD update — a Mutex, not
/// atomics, so the snapshot stays one readable struct. Never held across a
/// system boundary.
pub static NAMEPLATE_PASS_DEBUG: Mutex<NameplatePassDebug> = Mutex::new(NameplatePassDebug {
    frame: 0,
    cur: NAMEPLATE_SNAP_ZERO,
    prev: NAMEPLATE_SNAP_ZERO,
});

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// ExtractSchedule (render sub-app): snapshot the main-world billboards — pose,
/// fade, texture id, visibility — into render-side draw data. Runs after the
/// whole main schedule, so GlobalTransforms already include this frame's
/// billboard update and the fixed-loop interpolation tail.
fn extract_nameplates(
    mut data: ResMut<NameplateFinalPassData>,
    operator_cameras: Extract<Query<Entity, With<OperatorCamera>>>,
    plate_q: Extract<
        Query<(
            Entity,
            &GlobalTransform,
            &NameplateBillboard,
            &MeshMaterial3d<StandardMaterial>,
            &Visibility,
        )>,
    >,
    materials: Extract<Res<Assets<StandardMaterial>>>,
    bb_debug: Extract<Res<crate::nameplate_billboard::NameplateBillboardDebug>>,
) {
    data.operator_cam = operator_cameras.iter().next();

    // Rotate last completed tick into prev; this tick builds a fresh cur.
    let mut hidden = 0u32;
    {
        let mut dbg = NAMEPLATE_PASS_DEBUG
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        dbg.prev = dbg.cur;
        dbg.frame += 1;
        dbg.cur = NameplateFrameSnap::default();
        dbg.cur.operator_cam = data.operator_cam.is_some();
    }

    let mut plates = std::mem::take(&mut data.plates);
    plates.clear();
    for (entity, gt, np, mat_ref, vis) in &plate_q {
        // Hidden plates were culled by the depth-ramp legibility check upstream;
        // skip them before any GPU work.
        if matches!(vis, Visibility::Hidden) {
            hidden += 1;
            continue;
        }
        let Some(mat) = materials.get(&mat_ref.0) else {
            continue;
        };
        let Some(tex) = mat.base_color_texture.clone() else {
            continue;
        };
        plates.push(PlateDraw {
            entity,
            matrix: gt.to_matrix(),
            center: gt.translation(),
            alpha: np.last_alpha.clamp(0.0, 1.0),
            texture_handle: tex.clone(),
            binding: None,
        });
    }
    data.plates = plates;
    {
        let mut dbg = NAMEPLATE_PASS_DEBUG
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        dbg.cur.plates_total = data.plates.len() as u32;
        dbg.cur.hidden = hidden;
        dbg.cur.bb_total = bb_debug.total;
        dbg.cur.bb_hide_self = bb_debug.hide_self;
        dbg.cur.bb_hidden_depth = bb_debug.hidden_depth;
        dbg.cur.bb_visible = bb_debug.visible;
        dbg.cur.bb_despawned = bb_debug.despawned;
    }
}

/// Render schedule (Prepare set): resolve each plate's GpuImage and build/reuse
/// its bind group, then push this frame's matrix+alpha into its uniform buffer.
fn prepare_nameplate_bindings(
    mut data: ResMut<NameplateFinalPassData>,
    gpu: Res<NameplatePassGpu>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    pipeline_cache: Res<PipelineCache>,
    gpu_images: Res<RenderAssets<GpuImage>>,
    mut cache: Local<HashMap<Entity, PlateBinding>>,
) {
    let bind_group_layout = pipeline_cache.get_bind_group_layout(&gpu.bgl_descriptor);

    let mut no_gpu_image = 0u32;
    let mut not_had_data = 0u32;
    for plate in &mut data.plates {
        if cache.get(&plate.entity).is_none() {
            let Some(img) = gpu_images.get(plate.texture_handle.id()).cloned() else {
                no_gpu_image += 1; // texture still uploading — retries next frame
                continue;
            };
            if !img.had_data {
                not_had_data += 1;
                continue;
            }
            let uniforms = device.create_buffer(&BufferDescriptor {
                label: Some("nameplate_plate_uniforms"),
                size: PLATE_UNIFORM_SIZE as u64,
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let binding = PlateBinding {
                bind_group: device.create_bind_group(
                    "nameplate_plate_binding",
                    &bind_group_layout,
                    &BindGroupEntries::sequential((
                        BufferBinding {
                            buffer: &uniforms,
                            offset: 0,
                            size: None,
                        },
                        BufferBinding {
                            buffer: &gpu.view_uniforms,
                            offset: 0,
                            size: None,
                        },
                        &img.texture_view,
                        &img.sampler,
                    )),
                ),
                uniforms,
            };
            cache.insert(plate.entity, binding);
        }
        if let Some(b) = cache.get(&plate.entity).cloned() {
            plate.binding = Some(b);
        }
    }

    // Drop bindings for plates that no longer exist (the GPU objects die with
    // the wrappers).
    let live: HashSet<Entity> = data.plates.iter().map(|p| p.entity).collect();
    cache.retain(|e, _| live.contains(e));

    // Rewrite this frame's uniforms for every plate that has a binding.
    let mut bound = 0u32;
    for plate in &mut data.plates {
        if let Some(b) = cache.get(&plate.entity).cloned() {
            plate.binding = Some(b);
        }
        if plate.binding.is_some() {
            bound += 1;
        }
    }
    for plate in &data.plates {
        if let Some(b) = &plate.binding {
            queue_write(
                &queue,
                &b.uniforms,
                0,
                &plate_uniform_bytes(&plate.matrix, plate.alpha),
            );
        }
    }

    {
        let mut dbg = NAMEPLATE_PASS_DEBUG
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        dbg.cur.no_gpu_image = no_gpu_image;
        dbg.cur.not_had_data = not_had_data;
        dbg.cur.bound = bound;
        // RenderAssets has no len in bevy 0.19 — count the GpuImage cache.
        dbg.cur.gpu_images_total = gpu_images.iter().count() as u32;
    }
}

/// [model mat4 (64 B)][fade alpha f32 @ 64][zero pad to 80] — the exact layout
/// nameplate_final.wgsl's PlateUniform reads. Factored out so a unit test pins it.
fn plate_uniform_bytes(model: &Mat4, fade_alpha: f32) -> [u8; PLATE_UNIFORM_SIZE as usize] {
    let mut bytes = [0u8; PLATE_UNIFORM_SIZE as usize];
    for (i, c) in model.to_cols_array().iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&c.to_le_bytes());
    }
    bytes[64..68].copy_from_slice(&fade_alpha.to_le_bytes());
    bytes
}

/// [clip mat4 (64 B)][near f32 @ 64][subrect_scale vec2 @ 72] — the exact
/// layout nameplate_final.wgsl's ViewUniform reads. Factored out so a unit test pins it.
fn view_uniform_bytes(
    clip: Mat4,
    near: f32,
    subrect_scale: Vec2,
) -> [u8; VIEW_UNIFORM_SIZE as usize] {
    let mut bytes = [0u8; VIEW_UNIFORM_SIZE as usize];
    for (i, c) in clip.to_cols_array().iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&c.to_le_bytes());
    }
    bytes[64..68].copy_from_slice(&near.to_le_bytes());
    bytes[72..76].copy_from_slice(&subrect_scale.x.to_le_bytes());
    bytes[76..80].copy_from_slice(&subrect_scale.y.to_le_bytes());
    bytes
}

/// wgpu-queue write through Bevy's wrapped queue/buffer types. The tracked
/// encoder used by the pass is flushed to this same queue LATER (submit phase),
/// so a direct write here always lands before the draws that reference it.
fn queue_write(queue: &RenderQueue, buffer: &Buffer, offset: u64, data: &[u8]) {
    queue.write_buffer(buffer, offset, data);
}

/// Core3d sub-schedule (per camera run): draw the plates into the CURRENT view's
/// processed main texture — after all post effects, before upscaling writes the
/// window. Gated to the operator camera; every other 3D camera (launcher,
/// minimap bake, ...) runs its own Core3d schedule and skips.
#[allow(clippy::type_complexity)]
fn draw_nameplate_final_pass(
    view: ViewQuery<(
        &ExtractedView,
        Option<&MainPassResolutionOverride>,
        &ViewTarget,
        &ViewDepthTexture,
        Option<&Msaa>,
    )>,
    data: Res<NameplateFinalPassData>,
    gpu: Res<NameplatePassGpu>,
    device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
    queue: Res<RenderQueue>,
    mut ctx: RenderContext,
    mut pipe_state: Local<Option<((TextureFormat, PlateDepthMode, u32), CachedRenderPipelineId)>>,
) {
    let (ev, resolution_override, target, depth, msaa) = view.into_inner();

    // The gate: only the operator camera's PRIMARY view carries plates. Note that
    // `view.entity()` (CurrentView) is a RENDER-world view entity and can never
    // equal a main-world camera Entity — match via retained_view_entity instead.
    // subview_index 0 keeps this same camera's shadow-cascade subviews out of the
    // pass (they share the main entity but draw into depth targets, not the scene).
    let Some(operator_cam) = data.operator_cam else {
        return;
    };
    if ev.retained_view_entity.main_entity.id() != operator_cam
        || ev.retained_view_entity.subview_index != 0
    {
        return;
    }

    // The unsampled main texture is ALWAYS single-sample (MSAA resolves into it), so the color
    // attachment here is count 1 in every mode. Scene depth can only be ATTACHED when the view
    // itself is single-sample — with MSAA on, the multi-sample depth buffer cannot sit beside a
    // 1-sample color attachment in one pass; that variant instead compiles the fragment's
    // per-sample textureLoad test against the same buffer (see nameplate_final.wgsl).
    //
    // Under an upscaler (DLSS sets MainPassResolutionOverride) neither of those is sound:
    // this pass runs post-upscale at full res while the depth buffer only holds valid
    // geometry in its render-res sub-rect (bevy sizes the TEXTURE from
    // physical_target_size, so no size mismatch — just stale content past the sub-rect).
    // Subrect mode instead maps every fragment down into that sub-rect and loads it,
    // so walls still occlude plates. Deliberately NO viewport in any mode: this pass
    // positions plates in the CURRENT (post-upscale) target's clip space, so under
    // DLSS a MainPassResolutionOverride viewport would squish every plate into the
    // render-res corner of the full-res image. The mode check comes first because it
    // decides both pipeline and attachments; MSAA is forced off under DLSS anyway,
    // but override-first keeps this correct for any future upscaler that runs
    // multisampled.
    let samples = msaa.map_or(1, Msaa::samples);
    let mode = if resolution_override.is_some() {
        PlateDepthMode::Subrect
    } else if samples > 1 {
        PlateDepthMode::Gather
    } else {
        PlateDepthMode::Hardware
    };

    // Far-to-near in current view space: with premultiplied blending the NEAREST
    // plate must draw LAST, over everything behind it. View-space z is negative
    // in front of the camera — most-negative (farthest) sorts first.
    let mut draws: Vec<&PlateDraw> = data.plates.iter().filter(|p| p.binding.is_some()).collect();
    if draws.is_empty() {
        return;
    }
    // `center` is already WORLD space (GlobalTransform::translation at extract),
    // so the VIEW matrix projects it to view-space z — feeding it through
    // world_from_view was a double transform that produced garbage ordering.
    // View-space z is negative in front of the camera: most-negative = farthest,
    // drawn first. ExtractedView only carries world_from_view (a GlobalTransform),
    // so invert once and reuse for every comparison.
    let view_from_world = ev.world_from_view.to_matrix().inverse();
    draws.sort_by(|a, b| {
        let za = (view_from_world * a.center.extend(1.0)).z;
        let zb = (view_from_world * b.center.extend(1.0)).z;
        FloatOrd(za).cmp(&FloatOrd(zb))
    });

    // This frame's clip matrix for this view. Written before the pass is
    // recorded (same queue, earlier position) so GPU order holds even though
    // the tracked encoder's buffer is submitted at the Submit phase.
    //
    // NOTE: `ExtractedView.world_from_view` is the camera frame's LOCAL-TO-WORLD
    // matrix — Bevy inverts it before building its own clip matrix (see
    // `prepare_view_uniforms`, bevy_render 0.19 view/mod.rs ~L1052:
    // `view_from_world = world_from_view.inverse(); clip = P * view_from_world`).
    // Feeding the non-inverted form projects every plate through the camera's own
    // frame instead of through the camera: plates land ~an order of magnitude off
    // NDC in every MSAA/scale/TAA configuration while still issuing green draw
    // counters. Reuse the same `view_from_world` the sort above already derives.
    let clip = ev
        .clip_from_world
        .unwrap_or_else(|| ev.clip_from_view * view_from_world);
    // Projection near plane (manual-depth paths: fragment distances are `near / depth_value`).
    // Bevy's perspective-infinite-reverse projection stores `near` at column 3, row z
    // (bevy_render's own doc on ExtractedView.clip_from_view).
    let near = ev.clip_from_view.col(3).z;
    // Sub-rect mapping for the upscaler case: render resolution over the full target
    // extent. The depth texture is sized from physical_target_size, so its size IS the
    // full-res denominator; (1, 1) when no override is active.
    let subrect_scale = match resolution_override {
        Some(ovr) => {
            let ext = depth.texture.size();
            Vec2::new(
                ovr.0.x as f32 / ext.width.max(1) as f32,
                ovr.0.y as f32 / ext.height.max(1) as f32,
            )
        }
        None => Vec2::ONE,
    };
    queue_write(
        &queue,
        &gpu.view_uniforms,
        0,
        &view_uniform_bytes(clip, near, subrect_scale),
    );

    // Lazily specialize the pipeline for (target format, depth mode) — one in
    // practice each: Rgba16Float with Hdr; the render-scale image path reuses them.
    // Debug mirror: the draw stage reached the operator view this tick.
    {
        let mut dbg = NAMEPLATE_PASS_DEBUG
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        dbg.cur.reached_draw = true;
        dbg.cur.samples = samples;
    }

    // Key includes `samples` so cycling MSAA 2x -> 4x -> 8x compiles a fresh
    // pipeline per level (each variant's sample loop is a compile-time
    // constant). Hardware ignores the sample count on the shader side, but keying
    // on it costs nothing and avoids any special-casing here.
    let key = (target.main_texture_format(), mode, samples);
    if pipe_state.as_ref().is_none_or(|(k, _)| *k != key) {
        let id = pipeline_cache.queue_render_pipeline(plate_pipeline_descriptor(
            &gpu.shader,
            &gpu.bgl_descriptor,
            &gpu.depth_bgl_descriptor,
            &gpu.ss_depth_bgl_descriptor,
            key.0,
            key.1,
            key.2,
        ));
        *pipe_state = Some((key, id));
    }
    let (_, pipeline_id) = pipe_state.expect("set above");

    let Some(pipeline) = pipeline_cache.get_render_pipeline(pipeline_id) else {
        {
            let mut dbg = NAMEPLATE_PASS_DEBUG
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            dbg.cur.draws = 0;
            dbg.cur.pipeline_ready = false;
            dbg.cur.target_fmt = Some(key.0);
        }
        return; // still compiling (first frame or two) — plates reappear next tick;
                // a compile failure surfaces as a RenderError and quits the app.
    };

    // Color: the CURRENT main side after all effects. `get_unsampled_color_attachment`
    // is the plain single-sample view with Load semantics from here on (the first
    // pass of the frame cleared it) — no MSAA resolve interference when MSAA is
    // on, and exactly the processed image when it is off.
    // Depth: LOAD what geometry wrote; storing it back keeps later passes seeing
    // the same "already used" state (no re-clear).
    {
        let mut dbg = NAMEPLATE_PASS_DEBUG
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        dbg.cur.draws = draws.len() as u32;
        dbg.cur.pipeline_ready = true;
        dbg.cur.target_fmt = Some(key.0);
        if let Some(far) = draws.first() {
            // Farthest plate (head of the blend order): where it lands in clip
            // space — ndc xy inside [-1, 1] with w > 0 means on screen.
            // glam has no Mat4*Vec3 — project with a w=1 Vec4.
            let c = clip * far.center.extend(1.0);
            dbg.cur.far_alpha = far.alpha;
            dbg.cur.far_w = c.w;
            if c.w != 0.0 {
                dbg.cur.far_ndc_x = c.x / c.w;
                dbg.cur.far_ndc_y = c.y / c.w;
            }
        }
    }

    // Manual-depth runs (Gather, Subrect): one shared bind group for this view's scene
    // depth. Rebuilt each tick — a single handle, and it tracks target/MSAA changes
    // without any cache bookkeeping. It lives to the end of the function because the
    // tracked pass retains every binding set on it until its scope.
    let depth_bg = match mode {
        PlateDepthMode::Hardware => None,
        _ => {
            let bgl_desc = if matches!(mode, PlateDepthMode::Subrect) && samples == 1 {
                &gpu.ss_depth_bgl_descriptor
            } else {
                &gpu.depth_bgl_descriptor
            };
            let bgl = pipeline_cache.get_bind_group_layout(bgl_desc);
            Some(device.create_bind_group(
                "nameplate_final_pass_depth_binding",
                &bgl,
                &BindGroupEntries::sequential((depth.view(),)),
            ))
        }
    };

    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("nameplate_final_pass"),
        color_attachments: &[Some(target.get_unsampled_color_attachment())],
        // Only Hardware attaches this buffer; Gather and Subrect read it as a texture
        // in the fragment stage instead (see `PlateDepthMode`).
        depth_stencil_attachment: if matches!(mode, PlateDepthMode::Hardware) {
            Some(depth.get_attachment(StoreOp::Store))
        } else {
            None
        },
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });

    // Deliberately NO viewport here in any mode: this pass positions plates in
    // the CURRENT (post-upscale) target's clip space, so under DLSS the old
    // MainPassResolutionOverride viewport squished every plate into the
    // render-res corner of the full-res image. Non-upscaled views never had an
    // override, making the removed block a no-op for them.
    pass.set_render_pipeline(pipeline);

    if let Some(bg) = &depth_bg {
        pass.set_bind_group(1, bg, &[]);
    }

    pass.set_index_buffer(gpu.index_buffer.slice(..), IndexFormat::Uint32);
    for plate in &draws {
        let Some(binding) = &plate.binding else {
            continue;
        };
        pass.set_vertex_buffer(0, gpu.vertex_buffer.slice(..));
        pass.set_bind_group(0, &binding.bind_group, &[]);
        pass.draw_indexed(0..6, 0, 0..1);
    }
}

/// RenderStartup: the device exists by now — build the shared GPU objects.
fn init_nameplate_pass_gpu(
    mut commands: Commands,
    device: Res<RenderDevice>,
    asset_server: Res<AssetServer>,
) {
    commands.insert_resource(NameplatePassGpu::new(&device, &asset_server));
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct NameplateFinalPassPlugin;

impl Plugin for NameplateFinalPassPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "nameplate_final.wgsl");

        // No RenderErrorHandler override here: any render error (validation,
        // pipeline compile failure, ...) hits bevy's default handler and quits
        // the app with full detail — failures must crash loudly, not get swallowed.

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<NameplateFinalPassData>()
                .add_systems(ExtractSchedule, extract_nameplates)
                .add_systems(RenderStartup, init_nameplate_pass_gpu)
                .add_systems(
                    Render,
                    prepare_nameplate_bindings.in_set(RenderSystems::PrepareBindGroups),
                )
                // AFTER every post effect (bloom/DOF/fog/TAA/tonemapping all live in or
                // before the PostProcess set — verified against bevy 0.19 sources),
                // BEFORE upscaling writes the window. Per-camera sub-schedule: runs
                // once per Camera3d, gated to the operator inside.
                //
                // ALSO before `ui_pass`: bevy's UI composite is scheduled
                // `.after(Core3dSystems::PostProcess).before(upscaling)` — the
                // exact same bounds as this pass (bevy_ui_render 0.19,
                // render_pass::ui_pass). With no explicit edge between them the
                // two ran in arbitrary order and plates could land on TOP of the
                // HUD. Ordering before ui_pass makes the UI composite over the
                // plates, so nameplates never overwrite menus/HUD.
                .add_systems(
                    Core3d,
                    draw_nameplate_final_pass
                        .after(Core3dSystems::PostProcess)
                        .before(bevy::ui_render::ui_pass)
                        .before(upscaling),
                );
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use bevy::math::Quat;

    use super::*;
    use crate::camera::{build_operator_camera, WORLD_GIZMO_LAYER};
    use crate::graphics_settings::GraphicsSettings;
    use bevy::camera::visibility::RenderLayers;

    /// The operator camera must NOT include the plate layer: that exclusion is
    /// what keeps core_3d from drawing the plates (and keeps them out of every
    /// other 3D view). If this ever changes, either core_3d draws the plates AND
    /// our final pass (double-draw) or something else broke.
    #[test]
    fn operator_camera_excludes_the_plate_layer() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(GraphicsSettings::default());
        app.add_systems(
            Startup,
            |mut commands: Commands, settings: Res<GraphicsSettings>| {
                build_operator_camera(&mut commands, &settings, None);
            },
        );
        app.update();

        let mut q = app
            .world_mut()
            .query_filtered::<&RenderLayers, With<OperatorCamera>>();
        let layers = q.single(app.world()).expect("operator camera spawned");
        assert!(
            !layers.intersects(&crate::nameplate_overlay::nameplate_render_layers()),
            "the plate layer must stay excluded from the operator view"
        );
        // ...and still sees world + gizmo (unchanged contract).
        assert!(layers.intersects(&RenderLayers::layer(0)));
        assert!(layers.intersects(&RenderLayers::layer(WORLD_GIZMO_LAYER)));
    }

    /// The vertex data must stay the bevy_mesh Rectangle layout: any drift flips
    /// or mirrors every plate relative to what retail draws.
    #[test]
    fn vertex_data_matches_bevy_rectangle() {
        let expected = [
            [0.5f32, 0.5, 0.0, 1.0, 0.0],
            [-0.5, 0.5, 0.0, 0.0, 0.0],
            [-0.5, -0.5, 0.0, 0.0, 1.0],
            [0.5, -0.5, 0.0, 1.0, 1.0],
        ];
        assert_eq!(PLATE_VERTEX_DATA, expected);
        assert_eq!(PLATE_INDEX_DATA, [0u32, 1, 2, 0, 2, 3]);

        // The interleaved stride is what from_vertex_formats derives — pin it so a
        // format swap cannot silently desync the shader.
        let layout = VertexBufferLayout::from_vertex_formats(
            VertexStepMode::Vertex,
            [VertexFormat::Float32x3, VertexFormat::Float32x2],
        );
        assert_eq!(layout.array_stride, 20);
        assert_eq!(layout.attributes[0].offset, 0);
        assert_eq!(layout.attributes[1].offset, 12);
    }

    /// Sort: farthest plate (most-negative view-space z) first.
    ///
    /// Uses a NON-identity view on purpose: an identity matrix makes
    /// `world_from_view * center` and `view_from_world * center` agree, which is
    /// exactly why the original double-transform bug passed this test. The view
    /// here rotates +90° about Y (camera looks down −X in world terms… i.e.
    /// world +x maps to view z = −x), so any formula that transforms with the
    /// wrong matrix sign-flips every depth and sorts reversed.
    #[test]
    fn draw_order_is_farthest_first() {
        let view_from_world = Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2);
        struct P(usize, Vec3);
        // World-space plate centers; under this view each maps to z_view = −x,
        // so x=50 is farthest (most-negative), x=1 nearest.
        let mut plates = [
            P(0, Vec3::new(1.0, 0.0, 0.0)),  // view z -1   (nearest)
            P(1, Vec3::new(50.0, 0.0, 0.0)), // view z -50  (farthest)
            P(2, Vec3::new(20.0, 0.0, 0.0)), // view z -20
        ];
        plates.sort_by(|a, b| {
            let za = view_from_world * a.1.extend(1.0);
            let zb = view_from_world * b.1.extend(1.0);
            FloatOrd(za.z).cmp(&FloatOrd(zb.z))
        });
        assert_eq!(
            plates.iter().map(|p| p.0).collect::<Vec<_>>(),
            vec![1, 2, 0]
        );
    }

    /// View uniform packing: clip @ 0, near @ 64, subrect scale @ 72 — the exact
    /// layout nameplate_final.wgsl's ViewUniform reads (total stays 80).
    #[test]
    fn view_uniform_bytes_pack_clip_near_scale() {
        let m = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let bytes = view_uniform_bytes(m, 0.5, Vec2::new(0.5, 0.25));

        for (i, c) in m.to_cols_array().iter().enumerate() {
            assert_eq!(&bytes[i * 4..i * 4 + 4], &c.to_le_bytes());
        }
        assert_eq!(f32::from_le_bytes(bytes[64..68].try_into().unwrap()), 0.5);
        assert_eq!(f32::from_le_bytes(bytes[72..76].try_into().unwrap()), 0.5);
        assert_eq!(f32::from_le_bytes(bytes[76..80].try_into().unwrap()), 0.25);
    }

    /// Uniform packing: mat4 at byte 0, alpha at byte 64, rest zero — the exact
    /// layout nameplate_final.wgsl's PlateUniform reads.
    #[test]
    fn plate_uniform_bytes_pack_model_then_alpha() {
        let m = Mat4::from_scale_rotation_translation(
            Vec3::new(2.0, 3.0, 1.0),
            Quat::IDENTITY,
            Vec3::ZERO,
        );
        let bytes = plate_uniform_bytes(&m, 0.5);

        // mat4 occupies the first 64 bytes: compare against a manual pack.
        let mut expected_head = [0u8; 64];
        for (i, c) in m.to_cols_array().iter().enumerate() {
            expected_head[i * 4..i * 4 + 4].copy_from_slice(&c.to_le_bytes());
        }
        assert_eq!(&bytes[..64], &expected_head);
        assert_eq!(f32::from_le_bytes(bytes[64..68].try_into().unwrap()), 0.5);
        assert!(bytes[68..].iter().all(|&b| b == 0));
    }
}
