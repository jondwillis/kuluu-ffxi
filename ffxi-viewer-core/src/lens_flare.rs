use bevy::asset::embedded_asset;
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::components::InGameEntity;
use crate::graphics_settings::GraphicsSettings;
use crate::sun_moon::VanaSky;

#[derive(Clone, Debug, ShaderType)]
pub struct LensFlareUniform {
    // xyz = normalized world-space sun direction (projected to screen in the
    // shader against the render-frame view matrix — no CPU frame lag), w = intensity.
    pub sun_dir_intensity: Vec4,

    pub tint: Vec4,
}

impl Default for LensFlareUniform {
    fn default() -> Self {
        Self {
            sun_dir_intensity: Vec4::new(0.0, 1.0, 0.0, 0.0),
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

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }
}

#[derive(Component)]
pub struct LensFlareQuad;

const FLARE_DISTANCE: f32 = 0.2;

// The quad is placed from the camera's current transform (lag-free, since the
// projection now happens in the shader), but oversize it so it still covers the
// whole frustum during a fast camera swing.
const FLARE_OVERSCAN: f32 = 1.15;

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
        Transform::default(),
        Visibility::Hidden,
        bevy::light::NotShadowCaster,
        bevy::light::NotShadowReceiver,
    ));
}

#[allow(clippy::type_complexity)]
fn lens_flare_system(
    settings: Res<GraphicsSettings>,
    sky: Res<VanaSky>,
    cam_q: Query<
        (&Transform, &Camera, &Projection),
        (With<crate::camera::OperatorCamera>, Without<LensFlareQuad>),
    >,
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

    // The painterly flare is the Vanilla-mode sun glare; Enhanced uses bloom.
    let vanilla = !settings.sky_embellishments_enabled();
    let sun_up = sky.sun_altitude > 0.0;
    if !vanilla || !sun_up {
        *vis = Visibility::Hidden;
        return;
    }

    let Ok((cam_t, camera, proj)) = cam_q.single() else {
        *vis = Visibility::Hidden;
        return;
    };
    let Some(vp) = camera.logical_viewport_size() else {
        *vis = Visibility::Hidden;
        return;
    };

    // World-space sun direction (camera-independent; same formula as
    // sun_moon::sun_moon_system). The shader projects it against the live view
    // matrix, so the flare can't lag the camera.
    let sun_angle = (sky.hour / 24.0) * 2.0 * std::f32::consts::PI - std::f32::consts::FRAC_PI_2;
    let sun_dir = Vec3::new(sun_angle.cos(), sun_angle.sin(), 0.25).normalize();

    let elev = (sky.sun_altitude / std::f32::consts::FRAC_PI_2).clamp(0.0, 1.0);
    let intensity = 0.55 + 0.45 * elev;

    let fov_y = match proj {
        Projection::Perspective(p) => p.fov,
        _ => std::f32::consts::FRAC_PI_3,
    };
    let aspect = vp.x / vp.y.max(1.0);
    let height = 2.0 * FLARE_DISTANCE * (fov_y * 0.5).tan();
    let width = height * aspect;

    flare_xf.translation = cam_t.translation + cam_t.forward() * FLARE_DISTANCE;
    flare_xf.rotation = cam_t.rotation;
    flare_xf.scale = Vec3::new(width, height, 1.0) * FLARE_OVERSCAN;
    *vis = Visibility::Inherited;

    if let Some(mat) = mats.get_mut(&flare_mat.0) {
        mat.data.sun_dir_intensity = sun_dir.extend(intensity);
    }
}

pub struct LensFlarePlugin;

impl Plugin for LensFlarePlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "lens_flare.wgsl");
        app.add_plugins(MaterialPlugin::<LensFlareMaterial>::default())
            .add_systems(Startup, spawn_lens_flare)
            .add_systems(
                Update,
                lens_flare_system.after(crate::sun_moon::sun_moon_system),
            );
    }
}
