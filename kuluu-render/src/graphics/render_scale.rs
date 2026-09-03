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
/// since Phase 1 removed the second overlay Camera3d that shared this target — is
/// the ONLY other window writer on this path; anything spawned between them (or a
/// second Camera3d) would double-draw the frame, see docs/camera_notes.md.
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
    /// Kept alive one rebuild cycle so in-flight render passes never draw into
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
        // Native configuration: `assert_hud_camera_ownership` (same stage,
        // later this frame) marks the operator as THE default UI camera so
        // bevy_ui's propagate_ui_target_cameras binds every HUD node to it.
        // Tear back down to the native single-camera path.
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
    // Even-floored: an odd window (e.g. maximized 2560x1369) times a scale
    // can round to an odd image; odd attachment dimensions feed the same
    // half-pixel class of problems the window even-snap exists for.
    let want = UVec2::new(
        (((phys.x as f32 * s).round() as u32).max(2)) & !1,
        (((phys.y as f32 * s).round() as u32).max(2)) & !1,
    );

    // Scaled configuration: `assert_hud_camera_ownership` marks the composite
    // (spawned below) as THE default UI camera and removes the operator's —
    // exactly one marker at all times keeps bevy_ui's propagate off its
    // "highest order window camera" fallback (the double-rendered-UI path).
    dbg_snap.img = (want.x, want.y);
    let need_rebuild = state.image.is_none()
        || state.built_size != want
        || (state.scale_factor - scale_factor).abs() > 1e-3;
    if need_rebuild {
        let first = state.image.is_none();
        if !first && state.pending_size != want {
            // New size this frame: start the debounce, keep serving the OLD
            // image (and its OLD RenderTarget pointer -- see below) so a live
            // drag doesn't rebuild per pixel.
            state.pending_size = want;
            state.pending_streak = 0;
        } else if !first && state.pending_streak < 1 {
            state.pending_streak += 1;
        } else {
            // ATOMIC SWITCHOVER. Create the new image AND rewrite the
            // camera's RenderTarget to point at it in the SAME command flush.
            // The bug this fixes: previously `state.image` was reassigned
            // here while the RenderTarget update happened later in the
            // function, so for a frame the color image was new (720p) while
            // the camera still targeted the old handle -- and depth,
            // which is allocated by prepare_core_3d_depth_textures against
            // the camera's target size, matched neither. The result was
            // depth (old_size) + color (new_size) in one pass = wgpu
            // validation crash.
            //
            // Now: new handle, RenderTarget insert, and prev_image retention
            // all happen atomically. Depth will be reallocated to match the
            // new target size on the same frame the color image switches.
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
        return; // mid-debounce with no image yet (first frames only)
    };
    let handle = state.image.clone().expect("image set above");

    // Self-heal: the AA-driven camera respawn drops RenderTarget. Re-apply
    // it every frame when a live image exists and the camera isn't already
    // pointed at it. Safe outside a size change: same handle, no-op insert.
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
                    // One slot past the operator camera (0): with the nameplate
                    // overlay camera gone, nothing else writes this window path,
                    // so the composite is unambiguously last.
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
/// they never showed it). Snap windowed-mode size DOWN to even physical
/// dimensions; a 1px shrink is invisible. Fullscreen modes are left alone.
/// Self-quiescing: once even, nothing is written, so no resize-event loop.
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
        return; // fullscreen/borderless: OS owns the size
    }
    let p = window.physical_size();
    if p.x < 2 || p.y < 2 {
        return;
    }
    let even = UVec2::new(p.x & !1, p.y & !1);
    // MAXIMIZE detection (Bevy 0.19 has no public read of the OS bit): the
    // current physical size matching any monitor's full extent on either
    // axis means OS-driven maximum. Resizing there is what Windows treats
    // as a manual resize, which un-maximizes -- the "click Max, it snaps
    // back" bug. So: two strategies, one goal (even effective dimensions
    // everywhere).
    let maximized = monitors
        .iter()
        .any(|m| m.physical_size().x == p.x || m.physical_size().y == p.y);
    if maximized {
        // Maximized + odd: LETTERBOX instead of resize. Camera viewports get
        // the even-floored rect; the window keeps its OS geometry (Max stays
        // Max), rendering and UI layout see 2560x1368 instead of 2560x1369,
        // and the 1px dead row is invisible. Cleared when dimensions are
        // already even.
        let want_viewport = (even != p).then_some(bevy::camera::Viewport {
            physical_position: UVec2::ZERO,
            physical_size: even,
            depth: 0.0..1.0,
        });
        for (mut cam, target) in &mut cameras {
            // Letterboxing only makes sense on a camera whose render target IS
            // the window. When render scale is active the operator camera targets
            // an off-screen IMAGE sized window*scale — a window-derived viewport
            // can exceed that image and wgpu rejects the scissor at submit time
            // ("scissor rect not contained in render target", app quits via the
            // default RenderErrorHandler). Image-targeted cameras keep their own
            // full-image viewport.
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
    // Plain windowed sizes: snap the window itself down to even, and make
    // sure no stale letterbox viewport survives from a maximized phase.
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
    /// Phase 2 invariant: exactly one camera writes the game-window path per
    /// mode. The operator Camera3d renders the scene (order 0); with the second
    /// nameplate-overlay Camera3d gone (Phase 1), the ONLY other writer is this
    /// composite in scaled mode, and it must sit exactly one slot past the retired
    /// overlay's order constant — a second window writer between them is the whole-frame
    /// ghost regression. The spawn site reads [`RENDER_SCALE_COMPOSITE_ORDER`], so this
    /// test fails if that slot ever moves without updating the invariant.
    #[test]
    fn scaled_mode_composite_is_one_slot_past_the_retired_overlay() {
        assert_eq!(crate::nameplate_overlay::NAMEPLATE_OVERLAY_CAMERA_ORDER, 1);
        let composite: isize = super::RENDER_SCALE_COMPOSITE_ORDER;
        assert_eq!(
            composite,
            crate::nameplate_overlay::NAMEPLATE_OVERLAY_CAMERA_ORDER + 1,
        );
        // Operator is at the default 0 and nothing may share its slot.
        assert_eq!(composite, 2);
    }
}
