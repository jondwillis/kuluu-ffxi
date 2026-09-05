//! NVIDIA DLSS 5 Neural Rendering ("Neural Uplift", a.k.a. DLSSNR) — kuluu's
//! own pipeline hook, driving `nvngx_dlssnr.dll`'s Vulkan NGX API directly.
//!
//! All unsafe FFI lives in the [`kuluu_dlss_nr`] crate; this module is safe
//! Bevy wiring around it:
//! - [`apply_neural_uplift_system`] (main world, every frame) mirrors the
//!   settings onto the operator camera as the extracted marker component
//!   [`NrEnabled`];
//! - extraction into the render world happens via
//!   `ExtractComponentPlugin::<NrEnabled>` (registered by ViewerCorePlugin);
//! - [`prepare_nr`] (Render schedule, PrepareViews) lazily loads + inits the
//!   runtime once per process and creates/recreates the 0x12 feature at full
//!   window resolution when the size changes;
//! - [`nr_node`] (Core3d, PostProcess — after EarlyPostProcess where DLSS SR
//!   lives) encodes one EvaluateFeature per frame into its own command buffer
//!   and adds it to the render context.
//!
//! Full ABI story: `ffxi_dlss5.md` at the repo root.

use std::sync::Mutex;

use bevy::{
    ecs::query::QueryItem,
    prelude::*,
    render::{
        extract_component::ExtractComponent,
        renderer::{RenderAdapter, RenderContext, RenderDevice, RenderQueue, ViewQuery},
        sync_component::SyncComponent,
        view::{prepare_view_targets, ExtractedView, ViewTarget},
        Render, RenderApp, RenderSystems,
    },
};
// Raw wgpu types that bevy does not re-export (TextureTransition/TextureUses)
// plus the ones we name directly. Same pinned copy as bevy's wgpu — no new
// crate gets built; this only adds a direct edge to it.
use bevy::camera::{CameraMainTextureUsages, MainPassResolutionOverride};
use bevy::core_pipeline::prepass::ViewPrepassTextures;
use bevy::core_pipeline::schedule::{Core3d, Core3dSystems};
use kuluu_dlss_nr::{
    raw_command_buffer, result_name, wait_device_idle, NrParams, NrRuntime, NvngxHandle,
    NvngxResourceVk, VulkanHandles,
};
use wgpu::{
    CommandEncoderDescriptor, Extent3d, LoadOp, Operations, RenderPassColorAttachment,
    RenderPassDescriptor, StoreOp, TextureAspect, TextureDescriptor, TextureDimension,
    TextureFormat, TextureTransition, TextureUsages, TextureUses, TextureViewDescriptor,
};

use super::dlss::KULUU_DLSS_PROJECT_ID;
use super::settings::GraphicsSettings;
use crate::camera::OperatorCamera;

/// Main-world marker on the operator camera: "Neural Uplift is enabled, with
/// these knobs". Extracted into the render world (see impls below); the
/// prepare/node systems key off it there. The knob values ride along so no
/// separate settings extraction is needed — they are re-synced every frame by
/// [`apply_neural_uplift_system`] while enabled.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct NrEnabled {
    pub intensity: f32,
    pub local_tone_strength: f32,
    pub structure_strength: f32,
}

impl SyncComponent for NrEnabled {
    type Target = Self;
}

impl ExtractComponent for NrEnabled {
    type QueryData = &'static Self;
    type QueryFilter = ();
    type Out = Self;

    fn extract_component(item: QueryItem<Self::QueryData>) -> Option<Self::Out> {
        Some(*item)
    }
}

/// Process-wide NR runtime state (render world). One DLL load + one Init_Ext
/// per process, created lazily on the first frame where a camera carries
/// [`NrEnabled`]. A missing `nvngx_dlssnr.dll` or failed init is logged and
/// retried at most once per second — never per frame.
#[derive(Resource, Default)]
pub struct NrState {
    runtime: Option<NrRuntime>,
    handles: Option<VulkanHandles>,
    initialized: bool,
    last_attempt_ms: u64,
    /// Last failed CreateFeature attempt — recreate is throttled to 1/s after a
    /// failure (same policy as init) so a broken gate/forwarder doesn't log per frame.
    last_create_fail_ms: u64,
}

impl NrState {
    /// Loads the DLL + extracts raw Vulkan handles + runs Init_Ext exactly
    /// once. Returns true when the runtime is ready for CreateFeature /
    /// EvaluateFeature.
    fn ensure_initialized(&mut self, device: &RenderDevice) -> bool {
        if self.initialized {
            return true;
        }

        // Throttle retries (missing DLL / failed init): at most once per second.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if self.last_attempt_ms != 0 && now_ms.saturating_sub(self.last_attempt_ms) < 1000 {
            return false;
        }
        self.last_attempt_ms = now_ms;

        if self.runtime.is_none() {
            match NrRuntime::load() {
                Ok(rt) => {
                    // load() also resolved the forwarder (nvngx.dll_kuluu.dll)
                    // and checked its ABI version — without it every gated entry
                    // point (Init_Ext, CreateFeature, ReleaseFeature) fails.
                    info!("dlss-nr: loaded nvngx_dlssnr.dll + forwarder (nvngx.dll_kuluu.dll)");
                    self.runtime = Some(rt);
                }
                Err(e) => {
                    // LoadLibraryW returns NULL for ANY load failure (missing
                    // dependency, DllMain refusal, policy block), not just a
                    // missing file — the win32 code says which.
                    warn!(
                        "dlss-nr: failed to load nvngx_dlssnr.dll ({e}) — Neural Uplift unavailable (DLSS SR unaffected)"
                    );
                    return false;
                }
            }
        }

        if self.handles.is_none() {
            // RenderDevice is a bevy wrapper, not wgpu::Device — unwrap it first.
            match VulkanHandles::from_wgpu(device.wgpu_device()) {
                Some(h) => self.handles = Some(h),
                None => {
                    warn!("dlss-nr: not running on the Vulkan backend — Neural Uplift unavailable");
                    return false;
                }
            }
        }

        let runtime = self.runtime.as_ref().unwrap();
        let handles = self.handles.unwrap();
        // Same data-path convention as dlss_wgpu's SR init: the OS temp dir.
        let data_path = std::env::temp_dir().to_string_lossy().into_owned();
        // Lower 64 bits of KULUU_DLSS_PROJECT_ID (the u128 UUID) — the NGX API
        // takes a u64 app id; documented in ffxi_dlss5.md §3.
        let app_id = KULUU_DLSS_PROJECT_ID as u64;

        match runtime.init(app_id, &data_path, &handles, None) {
            kuluu_dlss_nr::NGX_SUCCESS => {
                info!(
                    "dlss-nr: Init_Ext succeeded via forwarder (app id 0x{app_id:016x}, data path {})",
                    data_path
                );
                self.initialized = true;
                true
            }
            r => {
                error!("dlss-nr: Init_Ext failed: {}", result_name(r));
                if r == kuluu_dlss_nr::FWD_NULL_TARGET {
                    // Forwarder got a null target — our load-order bug, not NGX.
                    error!("dlss-nr: forwarder received a null Init_Ext pointer (kuluu-dlss-nr load-order bug)");
                } else if r as u32 == 0xBAD0_0002 {
                    // Still gated: the call did not land inside nvngx.dll_kuluu.dll.
                    warn!("dlss-nr: still module-gated — confirm nvngx.dll_kuluu.dll sits next to this exe (staging in docs/DLSS.md)");
                }
                false
            }
        }
    }
}

/// Per-camera NR feature (render world). Created by [`prepare_nr`]; removed
/// when the camera loses [`NrEnabled`] (the SyncComponent removal hook takes
/// care of that automatically, and dropping `NrInner` releases the params map;
/// the NGX feature itself is released on recreate/despawn paths below).
#[derive(Component)]
pub struct NrContext {
    inner: Mutex<NrInner>,
}

struct NrInner {
    handle: NvngxHandle,
    params: NrParams,
    runtime: NrRuntime,
    device: wgpu::Device,
    out_w: u32,
    out_h: u32,
    /// Zero-filled motion-vector stand-in (Rg16Float, full window size). NGX
    /// samples it every frame as an explicit "no camera motion" — see the
    /// evaluate docs in kuluu-dlss-nr for why a NULL MVec is not used. The
    /// view keeps its texture alive; no separate handle is stored.
    mvec_view: wgpu::TextureView,
    /// (depth available, valid depth subrect) of the last evaluated frame.
    /// A change — or the first frame after create — sends DLSSNR.Reset so the
    /// runtime's temporal history does not carry stale geometry across it.
    last_depth_sig: Option<(bool, u32, u32)>,
}

impl Drop for NrInner {
    fn drop(&mut self) {
        // Best-effort release when the context goes away without a recreate
        // (camera despawn). The runtime may already be gone; ignore results.
        if !self.handle.is_empty() {
            // Wait for in-flight evaluate work before releasing: freeing the
            // feature while it is still on the GPU lets the runtime drop
            // internal resources those commands reference (UAF -> device loss).
            // Same ordering as dlss_wgpu's Drop; a lost device just makes the
            // wait report an error, which we log and proceed past.
            if let Err(code) = wait_device_idle(&self.device) {
                warn!("dlss-nr: device not idle before feature release (vk result {code})");
            }
            let mut h = self.handle;
            let _ = self.runtime.release_feature(&mut h);
        }
    }
}

/// Mirrors the Neural Uplift settings onto the operator camera. Runs every
/// frame (ungated) so it self-heals across the AA/DLSS camera respawn — same
/// pattern as `apply_camera_prepass_system`. Steady state is a single-entity
/// query with no writes.
pub fn apply_neural_uplift_system(
    settings: Res<GraphicsSettings>,
    mut commands: Commands,
    q_cam: Query<(Entity, Option<&NrEnabled>), With<OperatorCamera>>,
) {
    let Ok((entity, nr)) = q_cam.single() else {
        return;
    };
    let want = settings.nr_active();
    match (want, nr) {
        (true, None) => {
            commands.entity(entity).try_insert(NrEnabled {
                intensity: settings.nr_intensity,
                local_tone_strength: settings.nr_local_tone_strength,
                structure_strength: settings.nr_structure_strength,
            });
        }
        (false, Some(_)) => {
            commands.entity(entity).try_remove::<NrEnabled>();
        }
        // Disabled and no marker present: steady state, nothing to do.
        (false, None) => {}
        // Knob changed while enabled: re-insert with fresh values. The
        // extraction system picks the new copy up next frame; one frame of
        // stale knobs is invisible in practice.
        (true, Some(existing)) => {
            let fresh = NrEnabled {
                intensity: settings.nr_intensity,
                local_tone_strength: settings.nr_local_tone_strength,
                structure_strength: settings.nr_structure_strength,
            };
            if *existing != fresh {
                commands.entity(entity).try_insert(fresh);
            }
        }
    }
}

/// Render-schedule prepare (PrepareViews set, before view targets are built):
/// lazily inits the runtime and keeps one 0x12 feature per NR camera at the
/// current full-window resolution. Also ORs STORAGE_BINDING into the main
/// texture usages — NGX writes its output through storage ops, exactly like
/// bevy's own DLSS prepare does for SR.
pub fn prepare_nr(
    mut query: Query<
        (
            Entity,
            &ExtractedView,
            Option<&mut NrContext>,
            Option<&mut CameraMainTextureUsages>,
        ),
        With<NrEnabled>,
    >,
    mut nr_state: ResMut<NrState>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    mut commands: Commands,
) {
    for (entity, view, context, main_texture_usages) in &mut query {
        if let Some(mut usages) = main_texture_usages {
            usages.0 |= TextureUsages::STORAGE_BINDING;
        }

        // Lazy one-time init: load DLL + extract handles + Init_Ext.
        if !nr_state.ensure_initialized(&render_device) {
            continue;
        }
        let runtime = nr_state.runtime.as_ref().unwrap();

        let out_size = view.viewport.zw();
        // Borrow, don't move — `context` is used again below for the release path.
        let needs_recreate = match context {
            Some(ref ctx) => {
                let inner = ctx.inner.lock().unwrap();
                (inner.out_w, inner.out_h) != (out_size.x, out_size.y)
            }
            None => true,
        };
        if !needs_recreate {
            continue;
        }

        // Throttle recreate attempts after a failed CreateFeature: while the gate
        // or forwarder is broken this would otherwise log once per frame.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if nr_state.last_create_fail_ms != 0
            && now_ms.saturating_sub(nr_state.last_create_fail_ms) < 1000
        {
            continue;
        }

        // Drop the old feature first. Release is not command-buffer encoded in
        // this ABI — it takes only the handle. Wait for idle before releasing:
        // last frame's evaluate may still be on the GPU, and releasing under it
        // lets the runtime free resources those commands reference (UAF ->
        // device loss). Same ordering as dlss_wgpu's Drop.
        if let Some(ctx) = context {
            let mut inner = ctx.inner.lock().unwrap();
            if !inner.handle.is_empty() {
                if let Err(code) = wait_device_idle(&inner.device) {
                    warn!("dlss-nr: device not idle before feature recreate (vk result {code})");
                }
                let r = runtime.release_feature(&mut inner.handle);
                if r != kuluu_dlss_nr::NGX_SUCCESS {
                    warn!("dlss-nr: ReleaseFeature failed: {}", result_name(r));
                }
            }
        }

        let Ok(params) = NrParams::allocate() else {
            error!("dlss-nr: AllocateParameters failed");
            continue;
        };

        // Create the new feature on its own command buffer, submitted now —
        // same pattern as dlss_wgpu's DlssSuperResolution::new. Native-res
        // enhancement pass: input and output are both the full window size.
        let mut encoder = render_device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("kuluu_dlss_nr_create"),
        });

        // Zero-filled motion-vector stand-in, created alongside the feature so
        // it matches this window resolution and is cleared exactly once. The
        // pass ends (storing the clear) when its RenderPass drops.
        // The inner wgpu::Device, not RenderDevice's own create_texture (which
        // returns bevy's Texture wrapper — we need the raw wgpu texture/view).
        let mvec_texture = render_device
            .wgpu_device()
            .create_texture(&TextureDescriptor {
                label: Some("kuluu_dlss_nr_mvec"),
                size: Extent3d {
                    width: out_size.x,
                    height: out_size.y,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Rg16Float,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
        let mvec_view = mvec_texture.create_view(&TextureViewDescriptor::default());

        // The one-time clear runs on its OWN encoder: wgpu-core 29 forbids mixing the high-level and raw encoding APIs on one CommandEncoder (the first use locks the EncodingApi — build-12 panic). The create path below encodes through raw Vulkan, so it keeps a separate raw-only encoder; submitting this clear first guarantees it completes before any frame samples MVec.
        let mut mvec_encoder = render_device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("kuluu_dlss_nr_mvec_clear"),
        });
        {
            mvec_encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("kuluu_dlss_nr_mvec_clear_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &mvec_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(wgpu::Color::default()),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        render_queue.submit([mvec_encoder.finish()]);

        let Some(raw_cmd) = raw_command_buffer(&mut encoder) else {
            warn!("dlss-nr: no raw Vulkan command buffer (non-Vulkan backend?)");
            continue;
        };

        match runtime.create_nr_feature(
            raw_cmd, &params, out_size.x, out_size.y, out_size.x, out_size.y,
        ) {
            Ok(handle) => {
                render_queue.submit([encoder.finish()]);
                commands.entity(entity).insert(NrContext {
                    inner: Mutex::new(NrInner {
                        handle,
                        params,
                        runtime: *runtime,
                        // NrInner needs the inner wgpu::Device (RenderDevice
                        // derives Clone — .clone() alone would keep the wrapper).
                        device: render_device.wgpu_device().clone(),
                        out_w: out_size.x,
                        out_h: out_size.y,
                        mvec_view,
                        last_depth_sig: None,
                    }),
                });
                info!(
                    "dlss-nr: feature created at {}x{} (handle {:#018x})",
                    out_size.x, out_size.y, handle.ptr
                );
            }
            Err(r) => {
                nr_state.last_create_fail_ms = now_ms;
                error!(
                    "dlss-nr: CreateFeature(0x12) failed at {}x{}: {}",
                    out_size.x,
                    out_size.y,
                    result_name(r)
                );
                if r == kuluu_dlss_nr::FWD_NULL_TARGET {
                    error!("dlss-nr: forwarder received a null CreateFeature pointer (kuluu-dlss-nr load-order bug)");
                } else if r as u32 == 0xBAD0_0002 {
                    warn!("dlss-nr: still module-gated — confirm nvngx.dll_kuluu.dll sits next to this exe and is current (staging in docs/DLSS.md)");
                }
            }
        }
    }
}

/// Core3d node (PostProcess set — runs after EarlyPostProcess where DLSS SR
/// lives, so NR enhances whatever SR produced). One EvaluateFeature per frame:
/// Color = current main texture, Depth = prepass depth when sampleable,
/// Output = the ping-pong destination.
pub fn nr_node(
    view: ViewQuery<(
        &NrEnabled,
        &NrContext,
        Option<&MainPassResolutionOverride>,
        &ViewTarget,
        Option<&ViewPrepassTextures>,
    )>,
    adapter: Res<RenderAdapter>,
    mut ctx: RenderContext,
) {
    let (nr_enabled, nr_context, resolution_override, view_target, prepass_textures) =
        view.into_inner();

    // Depth is only usable when the prepass exists and is single-sampled.
    // Under MSAA the prepass depth texture is multisampled and unsampleable by
    // NGX — NR then runs color-only (the parser tolerates a missing depth).
    // `pre.depth` is an Option<ColorAttachment>; its `.texture` is a
    // CachedTexture, so sample_count() lives on the inner wgpu::Texture.
    let depth_view = prepass_textures
        .as_ref()
        .and_then(|pre| pre.depth.as_ref())
        .filter(|depth| depth.texture.texture.sample_count() == 1)
        .map(|depth| &depth.texture.default_view);

    // Build the color resource from the CURRENT main texture view (no flip yet),
    // so a failed extraction skips the frame without losing the ping-pong state.
    let Some(color_res) =
        NvngxResourceVk::from_texture_view(view_target.main_texture_view(), &adapter)
    else {
        return;
    };

    let mut inner = nr_context.inner.lock().unwrap();
    // Zero-filled stand-in sized to the input (created in prepare_nr); its
    // resource is built from the same view every frame.
    let Some(mvec_res) = NvngxResourceVk::from_texture_view(&inner.mvec_view, &adapter) else {
        return;
    };
    let out_extent = [color_res.width, color_res.height, 1];

    // Commit to the flip only once we know the runtime + output resource work.
    let view_target = view_target.post_process_write();
    let Some(output_res) = NvngxResourceVk::from_texture_view(view_target.destination, &adapter)
    else {
        warn!("dlss-nr: could not build output resource; preserving main texture");
        preserve_main_after_flip(
            ctx.command_encoder(),
            view_target.source_texture,
            view_target.destination_texture,
            out_extent,
        );
        return;
    };

    let depth_res = depth_view.and_then(|dv| NvngxResourceVk::from_texture_view(dv, &adapter));

    // Barriers on the shared encoder (source -> shader-readable, output ->
    // storage-writable), then encode the NGX evaluate into our own command
    // buffer — exactly dlss_wgpu's per-frame pattern. The separate buffer is
    // submitted immediately after the main one via add_command_buffer below.
    // Annotate explicitly: PostProcessWrite hands out &bevy Texture, which
    // deref-coerces to &wgpu::Texture only when the target type is known (no
    // coercion into a generic). dv.texture() already yields &wgpu::Texture.
    let mut barriers: Vec<TextureTransition<&wgpu::Texture>> = Vec::with_capacity(3);
    barriers.push(TextureTransition {
        texture: view_target.source_texture,
        selector: None,
        state: TextureUses::RESOURCE,
    });
    if let Some(dv) = depth_view {
        barriers.push(TextureTransition {
            texture: dv.texture(),
            selector: None,
            state: TextureUses::RESOURCE,
        });
    }
    // The MVec stand-in was last written as a render attachment (its one-time
    // clear in prepare_nr); sampling it from the evaluate command buffer needs
    // the shader-read transition too.
    barriers.push(TextureTransition {
        texture: inner.mvec_view.texture(),
        selector: None,
        state: TextureUses::RESOURCE,
    });
    barriers.push(TextureTransition {
        texture: view_target.destination_texture,
        selector: None,
        state: TextureUses::STORAGE_READ_WRITE,
    });
    ctx.command_encoder()
        .transition_resources(std::iter::empty(), barriers.into_iter());

    let mut encoder = inner
        .device
        .create_command_encoder(&CommandEncoderDescriptor {
            label: Some("kuluu_dlss_nr_evaluate"),
        });
    // Unreachable after the color extraction above (both need Vulkan), but a
    // miss here would still leave the flipped main texture unwritten.
    let Some(raw_cmd) = raw_command_buffer(&mut encoder) else {
        preserve_main_after_flip(
            ctx.command_encoder(),
            view_target.source_texture,
            view_target.destination_texture,
            out_extent,
        );
        return;
    };

    // When SR is active the prepass depth only has valid data in its top-left
    // subrect (the render resolution); tell the runtime so. Without SR the
    // whole depth texture is valid — pass its full size explicitly rather than
    // relying on the parser default, since the params map persists across
    // frames and would otherwise keep a stale SR subrect.
    let has_depth = depth_res.is_some();
    let (sub_w, sub_h) = match resolution_override {
        Some(ovr) => (ovr.0.x, ovr.0.y),
        None => (color_res.width, color_res.height),
    };

    // Flush the runtime's temporal history when the input geometry changes
    // (SR on/off or tier change moves the depth subrect; MSAA toggles depth
    // availability) — and on the first frame after create, whose internal
    // buffers are not yet meaningful. Steady-state frames keep accumulating.
    let sig = (has_depth, sub_w, sub_h);
    let reset = inner.last_depth_sig != Some(sig);
    inner.last_depth_sig = Some(sig);

    // Bevy's prepass writes standard (non-inverted) depth; the parser defaults
    // DepthInverted to 1, so say explicitly that ours is not inverted.
    let r = inner.runtime.evaluate_nr(
        raw_cmd,
        &inner.handle,
        &inner.params,
        &color_res,
        &mvec_res,
        depth_res.as_ref(),
        &output_res,
        nr_enabled.intensity,
        nr_enabled.local_tone_strength,
        nr_enabled.structure_strength,
        false, // bevy prepass depth is not inverted
        sub_w,
        sub_h,
        reset,
    );

    if r != kuluu_dlss_nr::NGX_SUCCESS {
        warn!("dlss-nr: EvaluateFeature failed: {}", result_name(r));
        // The flip above already moved the main texture to `destination`;
        // without a write into it, next frame would read undefined contents.
        preserve_main_after_flip(
            ctx.command_encoder(),
            view_target.source_texture,
            view_target.destination_texture,
            out_extent,
        );
        return;
    }

    ctx.add_command_buffer(encoder.finish());
}

/// Copies the pre-flip main texture into the post-flip destination after a
/// frame where [`nr_node`] flipped via `post_process_write()` but could not
/// encode the NGX evaluate. Without this, the new "current" main texture would
/// hold undefined contents and every later pass (and the next frame's read)
/// would see garbage.
pub fn preserve_main_after_flip(
    encoder: &mut wgpu::CommandEncoder,
    source: &wgpu::Texture,
    destination: &wgpu::Texture,
    extent: [u32; 3],
) {
    let copy = |texture| wgpu::TexelCopyTextureInfo {
        texture,
        mip_level: 0,
        origin: Default::default(),
        aspect: TextureAspect::All, // default; the main texture is color-only anyway
    };
    encoder.copy_texture_to_texture(
        copy(source),
        copy(destination),
        Extent3d {
            width: extent[0],
            height: extent[1],
            depth_or_array_layers: extent[2],
        },
    );
}

/// Registers the NR systems on a Bevy app. Called from ViewerCorePlugin under
/// the `dlss` feature:
/// - main world: the apply system + component extraction plugin;
/// - render world: prepare (PrepareViews, before view targets) and the node
///   (Core3d PostProcess — after EarlyPostProcess where DLSS SR lives).
pub fn register(app: &mut App) {
    // Main world: mirror settings onto the operator camera, every frame.
    app.add_systems(Update, apply_neural_uplift_system);

    // Extraction into the render world (adds its own ExtractSchedule system to
    // the RenderApp sub-app and wires removal propagation).
    app.add_plugins(bevy::render::extract_component::ExtractComponentPlugin::<
        NrEnabled,
    >::default());

    let render_app = app.sub_app_mut(RenderApp);
    render_app.init_resource::<NrState>();
    render_app.add_systems(
        Render,
        prepare_nr
            .in_set(RenderSystems::PrepareViews)
            .before(prepare_view_targets),
    );
    // PostProcess runs after EarlyPostProcess (where DLSS SR lives), so NR
    // enhances whatever SR produced when both are on. `.before(tonemapping)`
    // pins it ahead of the LDR conversion: without it the scheduler may run NR
    // last, enhancing already-tonemapped data instead of the HDR scene.
    render_app.add_systems(
        Core3d,
        nr_node
            .in_set(Core3dSystems::PostProcess)
            .before(bevy::core_pipeline::tonemapping::tonemapping),
    );
}
