//! Render scale: draw the 3D scene into an off-screen image at a fraction (or
//! multiple) of the window resolution, then upscale-composite it to the window
//! while the HUD stays at native resolution.
//!
//! At `render_scale == 1.0` this module is inert — the `OperatorCamera` renders
//! straight to the window exactly as before, with no composite camera and no
//! extra passes. Below 1.0 (downscale, perf) or above 1.0 (supersample) it:
//!   - points `OperatorCamera` at an `Image` render target sized `window * scale`,
//!   - spawns a window-targeted `Camera2d` composite that draws the image
//!     full-screen (bilinear upscale via the image's linear sampler); HUD ownership
//!     of it is asserted per frame by `assert_hud_camera_ownership` and consumed by
//!     bevy_ui's OWN per-frame system `propagate_ui_target_cameras`
//!     (PostUpdate, `UiSystems::Prepare`) — Bevy 0.19 has no standalone render-scale
//!     feature; every draw still goes through bevy_ui (`ImageNode` display quad +
//!     `ComputedUiRenderTargetInfo`, which keeps the HUD at native resolution), and
//!   - mirrors the window mouse pointer onto a synthetic picking pointer on the
//!     image target so click-to-target/hover keep working (Bevy's mesh-picking
//!     only casts a pointer through a camera whose render target matches the
//!     pointer's — see `bevy_picking::pointer::Location::is_in_viewport`).
//!
//! Bilinear is the first-pass upscaler; an FSR1 (EASU+RCAS) WGSL pass on the
//! composite is the follow-up.

use bevy::asset::RenderAssetUsages;
use bevy::camera::{ImageRenderTarget, NormalizedRenderTarget, RenderTarget};
use bevy::image::ImageSampler;
use bevy::picking::pointer::{Location, PointerId, PointerInput};
use bevy::picking::PickingSystems;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::ui::IsDefaultUiCamera;
use bevy::window::{PrimaryWindow, WindowRef};
use uuid::Uuid;

use crate::camera::OperatorCamera;
use crate::components::InGameEntity;
use crate::graphics::settings::GraphicsSettings;
use crate::picking::PickBridgePointer;

// Fixed so the bridge pointer's id is stable across runs (and matches the value
// `PickBridgePointer` is set to). The exact value is arbitrary.
const BRIDGE_POINTER_UUID: u128 = 0x6b756c75_72656e64_72736361_6c655f30;

/// Order slot of the render-scale composite camera: one past the retired
/// nameplate-overlay slot. The operator Camera3d writes the scene at order 0 and —
/// since the second overlay Camera3d that shared this target is gone — is
/// the ONLY other window writer on this path; anything spawned between them (or a
/// second Camera3d) would double-draw the frame. The slot values are pinned by
/// `scaled_mode_composite_is_one_slot_past_the_retired_overlay` below and by
/// [`crate::nameplate_overlay::NAMEPLATE_OVERLAY_CAMERA_ORDER`] /
/// [`crate::camera::build_operator_camera`].
pub const RENDER_SCALE_COMPOSITE_ORDER: isize =
    crate::nameplate_overlay::NAMEPLATE_OVERLAY_CAMERA_ORDER + 1;

/// The window-targeted 2D camera that upscales the off-screen 3D image and owns
/// the HUD while render scale is active.
#[derive(Component)]
struct RenderScaleCompositeCamera;

/// The full-window UI node that displays the off-screen 3D image.
#[derive(Component)]
struct RenderScaleDisplayNode;

#[derive(Resource)]
pub struct RenderScaleState {
    /// The off-screen 3D render target while active; `None` at native scale.
    image: Option<Handle<Image>>,
    /// Physical pixel size the current `image` was built for.
    built_size: UVec2,
    /// Image render-target scale factor (kept equal to the window's, so the
    /// image's logical size is `window_logical * render_scale`).
    scale_factor: f32,
    /// Kept alive one rebuild cycle so in-flight render passes do not draw into
    /// a freed texture during a live window resize (the resize crash).
    prev_image: Option<Handle<Image>>,
    /// Live-drag debounce: a new size must hold for two frames before the
    /// off-screen image is rebuilt.
    pending_size: UVec2,
    pending_streak: u8,
    /// The synthetic pointer that carries mouse input onto the image target.
    bridge: PointerId,
}

impl Default for RenderScaleState {
    fn default() -> Self {
        Self {
            image: None,
            built_size: UVec2::ZERO,
            scale_factor: 1.0,
            prev_image: None,
            pending_size: UVec2::ZERO,
            pending_streak: 0,
            bridge: PointerId::Custom(Uuid::from_u128(BRIDGE_POINTER_UUID)),
        }
    }
}

pub struct RenderScalePlugin;

impl Plugin for RenderScalePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderScaleState>()
            .add_systems(Startup, setup_render_scale_bridge)
            .add_systems(
                First,
                mirror_pointer_to_render_target_system
                    .after(PickingSystems::Input)
                    .before(PickingSystems::ProcessInput),
            )
            .add_systems(
                Update,
                (reconcile_render_scale_system, assert_hud_camera_ownership)
                    .chain()
                    .after(crate::graphics::settings::apply_anti_aliasing_system),
            );
    }
}

fn setup_render_scale_bridge(
    mut commands: Commands,
    mut bridge: ResMut<PickBridgePointer>,
    state: Res<RenderScaleState>,
) {
    // Spawning a `PointerId` auto-adds PointerLocation/Press/Interaction. It
    // stays inactive (no Location) until the mirror system feeds it.
    commands.spawn(state.bridge);
    bridge.0 = Some(state.bridge);
}

fn create_render_scale_image(images: &mut Assets<Image>, width: u32, height: u32) -> Handle<Image> {
    let mut image = Image::new_fill(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0u8, 0, 0, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
    // Linear sampling = bilinear upscale when the composite stretches it to the
    // window.
    image.sampler = ImageSampler::linear();
    images.add(image)
}

/// Reconciles the render-scale state with the settings and window size. Native
/// mode tears the composite down and points the operator straight at the window;
/// scaled mode keeps an off-screen image at `window * scale` and a window-targeted
/// composite that shows it. `assert_hud_camera_ownership` (same stage, later this
/// frame) marks exactly one default UI camera per configuration — the operator at
/// native scale, the composite while scaled — so bevy_ui's
/// `propagate_ui_target_cameras` binds every HUD node to it instead of falling
/// back to its "highest order window camera" rule (the double-rendered-UI path).
/// The image size is even-floored: an odd window times a scale can round to an
/// odd image, and odd attachment dimensions feed the same half-pixel class of
/// problems the window even-snap exists for. New sizes debounce for two frames so
/// a live drag does not rebuild per pixel. The rebuild is an atomic switchover —
/// new handle, RenderTarget insert, and prev_image retention in one command flush
/// — because a frame where the color image is new while the camera still targets
/// the prior handle leaves depth (allocated by prepare_core_3d_depth_textures
/// against the camera's target size) matching neither, and that mismatch is a wgpu
/// validation crash. The operator's RenderTarget is re-applied every frame: the
/// AA-driven camera respawn drops it, and the insert is a no-op when it already
/// points at the live image.
#[allow(clippy::type_complexity)]
fn reconcile_render_scale_system(
    settings: Res<GraphicsSettings>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut state: ResMut<RenderScaleState>,
    mut commands: Commands,
    q_op: Query<
        (Entity, Option<&RenderTarget>),
        (With<OperatorCamera>, Without<RenderScaleCompositeCamera>),
    >,
    q_composite: Query<Entity, With<RenderScaleCompositeCamera>>,
    mut q_display: Query<(Entity, &mut ImageNode), With<RenderScaleDisplayNode>>,
    mut dbg_snap: ResMut<crate::hud::graphics_debug::GraphicsDebugState>,
) {
    let Ok((op_entity, op_target)) = q_op.single() else {
        return;
    };

    if !settings.wants_render_scale() {
        if state.image.is_some() {
            commands
                .entity(op_entity)
                .insert(RenderTarget::Window(WindowRef::Primary));
            for e in &q_composite {
                commands.entity(e).despawn();
            }
            for (e, _) in &q_display {
                commands.entity(e).despawn();
            }
            state.image = None;
        }
        dbg_snap.img = (0, 0);
        return;
    }

    let Ok(window) = windows.single() else {
        return;
    };
    let phys = window.physical_size();
    if phys.x == 0 || phys.y == 0 {
        return;
    }
    let scale_factor = window.scale_factor();
    let s = settings.render_scale();
    let want = UVec2::new(
        (((phys.x as f32 * s).round() as u32).max(2)) & !1,
        (((phys.y as f32 * s).round() as u32).max(2)) & !1,
    );

    dbg_snap.img = (want.x, want.y);
    let need_rebuild = state.image.is_none()
        || state.built_size != want
        || (state.scale_factor - scale_factor).abs() > 1e-3;
    if need_rebuild {
        let first = state.image.is_none();
        if !first && state.pending_size != want {
            state.pending_size = want;
            state.pending_streak = 0;
        } else if !first && state.pending_streak < 1 {
            state.pending_streak += 1;
        } else {
            state.prev_image = state.image.take();
            let handle = create_render_scale_image(&mut images, want.x, want.y);
            commands
                .entity(op_entity)
                .insert(RenderTarget::Image(ImageRenderTarget {
                    handle: handle.clone(),
                    scale_factor,
                }));
            state.image = Some(handle);
            state.built_size = want;
            state.scale_factor = scale_factor;
            state.pending_size = want;
            state.pending_streak = 0;
        }
    }
    let Some(_) = state.image else {
        return;
    };
    let handle = state.image.clone().expect("image set above");

    if op_target.and_then(|t| t.as_image()) != Some(&handle) {
        commands
            .entity(op_entity)
            .insert(RenderTarget::Image(ImageRenderTarget {
                handle: handle.clone(),
                scale_factor,
            }));
    }

    // Ensure the composite/UI camera exists.
    let composite = match q_composite.iter().next() {
        Some(e) => e,
        None => commands
            .spawn((
                InGameEntity,
                RenderScaleCompositeCamera,
                Camera2d,
                Camera {
                    order: RENDER_SCALE_COMPOSITE_ORDER,
                    ..default()
                },
            ))
            .id(),
    };

    // Ensure the full-window display node exists and shows the current image.
    let mut found = false;
    for (_, mut node) in &mut q_display {
        if node.image != handle {
            node.image = handle.clone();
        }
        found = true;
    }
    if !found {
        commands.spawn((
            InGameEntity,
            RenderScaleDisplayNode,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            ImageNode::new(handle),
            // Behind every HUD node so the upscaled scene is the backdrop.
            GlobalZIndex(i32::MIN),
            UiTargetCamera(composite),
            // The mouse pointer must fall through to the 3D bridge pointer, not
            // get eaten by this backdrop.
            bevy::picking::Pickable::IGNORE,
        ));
    }
}

/// Assert HUD camera ownership EVERY frame — inside the UI flow itself, not an
/// outside gate function.
///
/// bevy_ui's own per-frame system `propagate_ui_target_cameras`
/// (PostUpdate, `UiSystems::Prepare`; bevy_ui/src/update.rs) renders each UI
/// root node into its explicit `UiTargetCamera`, or — none set — into THE
/// default ui camera: exactly one `IsDefaultUiCamera`-marked camera. When zero
/// (or two+) cameras carry the marker it falls back to "highest order camera
/// targeting the primary window" (bevy_ui/src/ui_node.rs) and warns — that
/// fallback is what ghosted the HUD on ambiguous frames, so no frame may ever
/// run with the operator unmarked while a composite exists (or vice versa).
/// This unconditional, idempotent system keeps exactly ONE marker at all times:
/// the operator owns the HUD at native scale; while render-scaled (and its
/// image target exists) the composite owns it. `propagate_ui_target_cameras`
/// runs in PostUpdate — AFTER this Update-stage system — so a marker placed
/// here takes effect on THIS frame's layout/extract.
fn assert_hud_camera_ownership(
    settings: Res<GraphicsSettings>,
    state: Res<RenderScaleState>,
    mut commands: Commands,
    q_op: Query<(Entity, Has<IsDefaultUiCamera>), With<OperatorCamera>>,
    q_comp: Query<(Entity, Has<IsDefaultUiCamera>), With<RenderScaleCompositeCamera>>,
) {
    let want_composite = settings.wants_render_scale() && state.image.is_some();
    for (entity, has_marker) in &q_op {
        let want = !want_composite;
        if has_marker != want {
            if want {
                commands.entity(entity).insert(IsDefaultUiCamera);
            } else {
                commands.entity(entity).remove::<IsDefaultUiCamera>();
            }
        }
    }
    for (entity, has_marker) in &q_comp {
        if has_marker != want_composite {
            if want_composite {
                commands.entity(entity).insert(IsDefaultUiCamera);
            } else {
                commands.entity(entity).remove::<IsDefaultUiCamera>();
            }
        }
    }
}

/// Mirror window mouse input onto the bridge pointer, remapped onto the
/// off-screen image target so mesh-picking casts through `OperatorCamera`.
fn mirror_pointer_to_render_target_system(
    settings: Res<GraphicsSettings>,
    state: Res<RenderScaleState>,
    mut io: ParamSet<(MessageReader<PointerInput>, MessageWriter<PointerInput>)>,
) {
    if !settings.wants_render_scale() {
        return;
    }
    let Some(handle) = state.image.clone() else {
        return;
    };
    let s = settings.render_scale();
    let target = NormalizedRenderTarget::Image(ImageRenderTarget {
        handle,
        scale_factor: state.scale_factor,
    });
    let bridge = state.bridge;

    // The image's logical size is `window_logical * s`, so a window-space
    // position maps onto it by scaling by `s`.
    let mirrored: Vec<PointerInput> = io
        .p0()
        .read()
        .filter(|e| e.pointer_id == PointerId::Mouse)
        .map(|e| {
            PointerInput::new(
                bridge,
                Location {
                    target: target.clone(),
                    position: e.location.position * s,
                },
                e.action,
            )
        })
        .collect();
    if mirrored.is_empty() {
        return;
    }
    let mut writer = io.p1();
    for ev in mirrored {
        writer.write(ev);
    }
}

/// WINDOW EVEN-SNAP. Odd physical window dimensions put every centered and
/// percent-sized UI element on a half-pixel, and half-pixel positions round
/// unstably under relayout -- with the debug text churning every frame, glyphs
/// and borders flip a pixel in different directions and the panel "spreads"
/// (the arbitrary-window-size jitter; default size and fullscreen are even, so
/// they stayed free of it). Snap windowed-mode size DOWN to even physical
/// dimensions; a 1px shrink is invisible. Fullscreen modes are left alone.
/// Self-quiescing: once even, nothing is written, so no resize-event loop.
///
/// Maximized windows are letterboxed instead of resized: Bevy 0.19 has no
/// public read of the OS maximize bit, so a physical size matching any
/// monitor's full extent on either axis is treated as OS-driven maximum, and
/// resizing there is what Windows reads as a manual resize, which un-maximizes
/// (the "click Max, it snaps back" bug). The cameras get the even-floored
/// viewport rect; the window keeps its OS geometry and the 1px dead row is
/// invisible. A camera whose render target is an off-screen image skips the
/// letterbox: a window-derived viewport can exceed that image, and wgpu
/// rejects the scissor at submit time ("scissor rect not contained in render
/// target", which quits the app via the default RenderErrorHandler).
#[allow(dead_code)]
fn snap_window_to_even_system(
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    monitors: Query<&bevy::window::Monitor>,
    mut cameras: Query<
        (&mut Camera, Option<&RenderTarget>),
        bevy::ecs::query::Or<(With<OperatorCamera>, With<RenderScaleCompositeCamera>)>,
    >,
) {
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    if !matches!(window.mode, bevy::window::WindowMode::Windowed) {
        return;
    }
    let p = window.physical_size();
    if p.x < 2 || p.y < 2 {
        return;
    }
    let even = UVec2::new(p.x & !1, p.y & !1);
    let maximized = monitors
        .iter()
        .any(|m| m.physical_size().x == p.x || m.physical_size().y == p.y);
    if maximized {
        let want_viewport = (even != p).then_some(bevy::camera::Viewport {
            physical_position: UVec2::ZERO,
            physical_size: even,
            depth: 0.0..1.0,
        });
        for (mut cam, target) in &mut cameras {
            if target.and_then(|t| t.as_image()).is_some() {
                continue;
            }
            let differs = match (&cam.viewport, &want_viewport) {
                (None, None) => false,
                (Some(a), Some(b)) => a.physical_size != b.physical_size,
                _ => true,
            };
            if differs {
                cam.viewport = want_viewport.clone();
            }
        }
        return;
    }
    for (mut cam, _target) in &mut cameras {
        if cam.viewport.is_some() {
            cam.viewport = None;
        }
    }
    if even != p {
        window.resolution.set_physical_resolution(even.x, even.y);
    }
}

#[cfg(test)]
mod tests {
    /// Exactly one camera writes the game-window path per mode: the operator
    /// Camera3d renders the scene at order 0, and in scaled mode the only other
    /// writer is this composite, which sits exactly one slot past the retired
    /// overlay's order constant. A second window writer between them would
    /// double-draw the frame, so the test fails if that slot moves.
    #[test]
    fn scaled_mode_composite_is_one_slot_past_the_retired_overlay() {
        assert_eq!(crate::nameplate_overlay::NAMEPLATE_OVERLAY_CAMERA_ORDER, 1);
        let composite: isize = super::RENDER_SCALE_COMPOSITE_ORDER;
        assert_eq!(
            composite,
            crate::nameplate_overlay::NAMEPLATE_OVERLAY_CAMERA_ORDER + 1,
        );
        assert_eq!(composite, 2);
    }
}
