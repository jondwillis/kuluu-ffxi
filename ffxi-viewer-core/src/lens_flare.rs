//! Screen-space lens flare for the **Enhanced** sky style.
//!
//! A single fullscreen-ish quad rides in front of the camera, scaled
//! each frame to exactly fill the frustum so its UVs map 1:1 to the
//! screen. [`lens_flare_system`] projects the sun's world position to a
//! screen UV and feeds it — plus an intensity that gates on sun
//! altitude, on-screen visibility, and the active [`SkyStyle`] — into
//! the [`LensFlareMaterial`] uniform. The WGSL (`lens_flare.wgsl`) draws
//! the halo, ghost chain, and anamorphic streak additively.
//!
//! Retail style: intensity is forced to 0 (quad hidden). Retail's sun
//! is a bare billboard + bloom; a compound-lens flare would be *less*
//! faithful, so the effect is exclusive to Enhanced.
//!
//! Occlusion (flare fading when the sun slips behind terrain) is a
//! deliberate follow-up — it needs a depth-prepass sample and isn't
//! wired yet. Today the flare tracks the sun whenever it's above the
//! horizon and on screen.
//!
//! Lifecycle: the quad is one `InGameEntity` spawned at world setup; the
//! standard `despawn_ingame_entities` drain reclaims it on logout.

use bevy::asset::embedded_asset;
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::components::InGameEntity;
use crate::graphics_settings::{GraphicsSettings, SkyStyle};
use crate::sun_moon::{SunDisc, VanaSky};

#[derive(Clone, Debug, ShaderType)]
pub struct LensFlareUniform {
    /// `xy` = sun screen UV (origin bottom-left), `z` = intensity
    /// `[0,1]`, `w` = viewport aspect (width / height).
    pub params: Vec4,
    /// `rgb` = flare tint, `a` = unused.
    pub tint: Vec4,
}

impl Default for LensFlareUniform {
    fn default() -> Self {
        Self {
            params: Vec4::new(0.5, 0.5, 0.0, 1.0),
            tint: Vec4::new(1.0, 0.95, 0.85, 1.0),
        }
    }
}

#[derive(Asset, AsBindGroup, Clone, Debug, TypePath, Default)]
pub struct LensFlareMaterial {
    #[uniform(0)]
    pub data: LensFlareUniform,
}

impl Material for LensFlareMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://ffxi_viewer_core/lens_flare.wgsl".into()
    }

    /// Additive — the flare adds light over the rendered scene and the
    /// "empty" parts of the quad contribute black (zero).
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }
}

/// Tag for the singleton flare quad.
#[derive(Component)]
pub struct LensFlareQuad;

/// How far in front of the camera the quad sits. Small enough that no
/// real scene geometry is ever nearer (so the additive overlay always
/// wins the depth test), still beyond the default near plane (0.1).
const FLARE_DISTANCE: f32 = 0.2;

fn spawn_lens_flare(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<LensFlareMaterial>>,
) {
    let quad = meshes.add(Rectangle::new(1.0, 1.0));
    let material = materials.add(LensFlareMaterial::default());
    commands.spawn((
        InGameEntity,
        LensFlareQuad,
        Mesh3d(quad),
        MeshMaterial3d(material),
        // Real transform set every frame by `lens_flare_system`; start
        // hidden so a frame before the first update doesn't flash a
        // mis-scaled quad.
        Transform::default(),
        Visibility::Hidden,
        bevy::light::NotShadowCaster,
        bevy::light::NotShadowReceiver,
    ));
}

/// Per-frame: place + scale the quad to fill the frustum, project the
/// sun to screen space, and update the material uniform / visibility.
#[allow(clippy::type_complexity)]
fn lens_flare_system(
    settings: Res<GraphicsSettings>,
    sky: Res<VanaSky>,
    cam_q: Query<
        (&GlobalTransform, &Camera, &Projection),
        (
            With<crate::camera::OperatorCamera>,
            Without<LensFlareQuad>,
            Without<SunDisc>,
        ),
    >,
    sun_q: Query<&Transform, (With<SunDisc>, Without<LensFlareQuad>)>,
    mut flare_q: Query<
        (
            &mut Transform,
            &mut Visibility,
            &MeshMaterial3d<LensFlareMaterial>,
        ),
        With<LensFlareQuad>,
    >,
    mut mats: ResMut<Assets<LensFlareMaterial>>,
) {
    let Ok((mut flare_xf, mut vis, flare_mat)) = flare_q.single_mut() else {
        return;
    };

    let enhanced = settings.sky_style == SkyStyle::Enhanced;
    let sun_up = sky.sun_altitude > 0.0;
    if !enhanced || !sun_up {
        *vis = Visibility::Hidden;
        return;
    }

    let Ok((cam_gt, camera, proj)) = cam_q.single() else {
        *vis = Visibility::Hidden;
        return;
    };
    let Ok(sun_xf) = sun_q.single() else {
        *vis = Visibility::Hidden;
        return;
    };

    let Some(vp) = camera.logical_viewport_size() else {
        *vis = Visibility::Hidden;
        return;
    };

    // Project the sun disc's world position to screen pixels. `Err`
    // means it's behind the camera — no flare then.
    let Ok(screen) = camera.world_to_viewport(cam_gt, sun_xf.translation) else {
        *vis = Visibility::Hidden;
        return;
    };

    // Pixels → UV. Both `world_to_viewport` (origin top-left, +y down)
    // and Bevy's `Rectangle` UVs (v=0 at the +Y/top vertex, increasing
    // downward) share a top-down convention, so no flip is needed —
    // the quad rides with `rotation = cam.rotation()`, mapping local
    // +X→screen-right and +Y→screen-up.
    let sun_uv = Vec2::new(screen.x / vp.x, screen.y / vp.y);
    let aspect = vp.x / vp.y.max(1.0);

    // Intensity ramps in with sun elevation — a touch dimmer right at
    // the horizon (thicker atmosphere scatters the beam), full higher
    // up. Kept ≤ ~1 so the additive pass doesn't blow out the scene.
    let elev = (sky.sun_altitude / std::f32::consts::FRAC_PI_2).clamp(0.0, 1.0);
    let intensity = 0.55 + 0.45 * elev;

    // Fit the quad to the frustum at `FLARE_DISTANCE`. `p.fov` is the
    // vertical FOV in radians.
    let fov_y = match proj {
        Projection::Perspective(p) => p.fov,
        _ => std::f32::consts::FRAC_PI_3,
    };
    let half_h = FLARE_DISTANCE * (fov_y * 0.5).tan();
    let height = 2.0 * half_h;
    let width = height * aspect;

    flare_xf.translation = cam_gt.translation() + cam_gt.forward() * FLARE_DISTANCE;
    flare_xf.rotation = cam_gt.rotation();
    flare_xf.scale = Vec3::new(width, height, 1.0);
    *vis = Visibility::Inherited;

    if let Some(mat) = mats.get_mut(&flare_mat.0) {
        mat.data.params = Vec4::new(sun_uv.x, sun_uv.y, intensity, aspect);
    }
}

pub struct LensFlarePlugin;

impl Plugin for LensFlarePlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "lens_flare.wgsl");
        app.add_plugins(MaterialPlugin::<LensFlareMaterial>::default())
            .add_systems(Startup, spawn_lens_flare)
            // After sun_moon_system so the SunDisc transform is current.
            .add_systems(Update, lens_flare_system.after(crate::sun_moon::sun_moon_system));
    }
}
