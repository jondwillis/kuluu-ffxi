//! Render scale: draw the 3D scene into an off-screen image at a fraction (or
//! multiple) of the window resolution, then upscale-composite it to the window
//! while the HUD stays at native resolution.
//!
//! At `render_scale == 1.0` this module is inert — the `OperatorCamera` renders
//! straight to the window exactly as before, with no composite camera and no
//! extra passes. Below 1.0 (downscale, perf) or above 1.0 (supersample) it:
//!   - points `OperatorCamera` at an `Image` render target sized `window * scale`,
//!   - spawns a window-targeted `Camera2d` composite that draws the image
//!     full-screen (bilinear upscale via the image's linear sampler) and owns the
//!     HUD as the default UI camera, and
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
use bevy::window::{PrimaryWindow, WindowRef};
use uuid::Uuid;

use crate::camera::OperatorCamera;
use crate::components::InGameEntity;
use crate::graphics::settings::GraphicsSettings;
use crate::picking::PickBridgePointer;

// Fixed so the bridge pointer's id is stable across runs (and matches the value
// `PickBridgePointer` is set to). The exact value is arbitrary.
const BRIDGE_POINTER_UUID: u128 = 0x6b756c75_72656e64_72736361_6c655f30;

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
    /// The synthetic pointer that carries mouse input onto the image target.
    bridge: PointerId,
}

impl Default for RenderScaleState {
    fn default() -> Self {
        Self {
            image: None,
            built_size: UVec2::ZERO,
            scale_factor: 1.0,
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
                reconcile_render_scale_system
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
) {
    let Ok((op_entity, op_target)) = q_op.single() else {
        return;
    };

    if !settings.wants_render_scale() {
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
        ((phys.x as f32 * s).round() as u32).max(1),
        ((phys.y as f32 * s).round() as u32).max(1),
    );

    let need_rebuild = state.image.is_none()
        || state.built_size != want
        || (state.scale_factor - scale_factor).abs() > 1e-3;
    if need_rebuild {
        let handle = create_render_scale_image(&mut images, want.x, want.y);
        state.image = Some(handle);
        state.built_size = want;
        state.scale_factor = scale_factor;
    }
    let handle = state.image.clone().expect("image set above");

    // Point the 3D camera at the off-screen image (re-applied every frame so it
    // self-heals across the AA-driven camera respawn, which drops this component).
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
                    order: 1,
                    ..default()
                },
                IsDefaultUiCamera,
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
