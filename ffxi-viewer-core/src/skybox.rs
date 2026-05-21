//! Screen-space sky gradient driven by FFXI Weather chunks.
//!
//! Per-zone Weather (`0x2F`) records carry an 8-color skybox gradient
//! paired with 8 altitude bands. This module renders that gradient as
//! an inverted-sphere material centered on the camera so the player
//! always sees a continuous sky regardless of position.
//!
//! Algorithmic reference (cite-only, no code copied): lotus-ffxi's
//! miss shader at `vendor/lotus-ffxi/ffxi/shaders/raytrace.slang:25-67`
//! bins ray altitude against `skybox_altitudes[8]` and lerps colors
//! from `skybox_colors[8]`. Our WGSL fragment does the same math on
//! standard rasterizer geometry (Bevy's raytrace path is wgpu, not
//! Vulkan-RT — the algorithm ports cleanly because it's pure math
//! over a uniform buffer).
//!
//! Lifecycle: the skybox sphere is a single `InGameEntity` spawned
//! once at world setup, parented logically to the camera by being
//! repositioned each frame. When the player leaves the in-game state,
//! the standard `despawn_ingame_entities` drain (see
//! [[feedback_bevy_lifecycle_symmetry]]) cleans it up.
//!
//! When no weather records are loaded (zone with no Weather chunk,
//! or DAT install missing), the material uses an editor-friendly
//! neutral gradient so a developer running headless still sees
//! *something* — a black sphere would mask backdrop bugs.

use bevy::asset::embedded_asset;
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

use crate::components::InGameEntity;
use crate::weather::ZoneWeather;

/// Radius of the inverted skybox sphere. Large enough that the sphere
/// engulfs the camera + visible terrain without clipping into the
/// far plane (`spawn_camera` overrides `far` to 6000 elsewhere).
const SKYBOX_RADIUS: f32 = 1500.0;

/// 8 colors + 8 altitudes that drive the gradient.
///
/// Packing convention: WGSL `std140` (and even `std430` in many drivers)
/// pads each array element of a scalar-or-vec3 to 16 bytes. Storing
/// the 8 altitudes as `[Vec4; 2]` (two packed quads) is the simplest
/// layout that survives the alignment rules across all backends —
/// the fragment shader unpacks via `get_altitude(i)`.
#[derive(Asset, AsBindGroup, Clone, Debug, TypePath)]
pub struct SkyboxGradientMaterial {
    #[uniform(0)]
    pub colors: [Vec4; 8],
    #[uniform(1)]
    pub altitudes_packed: [Vec4; 2],
}

impl Default for SkyboxGradientMaterial {
    fn default() -> Self {
        // Neutral debug gradient: deep blue at horizon → light blue at
        // zenith. Lets us see the sphere is rendering before any
        // Weather record loads.
        let horizon = Vec4::new(0.15, 0.20, 0.35, 1.0);
        let mid = Vec4::new(0.35, 0.55, 0.85, 1.0);
        let zenith = Vec4::new(0.55, 0.75, 0.95, 1.0);
        Self {
            colors: [
                horizon, horizon, mid, mid, mid, zenith, zenith, zenith,
            ],
            // Spread 8 altitudes evenly across [-1, 1]. Lotus does
            // nonlinear spacing per zone — we'll overwrite this once
            // a real WeatherRecord arrives.
            altitudes_packed: [
                Vec4::new(-1.0, -0.5, -0.2, 0.0),
                Vec4::new(0.2, 0.4, 0.7, 1.0),
            ],
        }
    }
}

impl Material for SkyboxGradientMaterial {
    fn fragment_shader() -> ShaderRef {
        // Loaded via `embedded_asset!` below; the path is the
        // canonical embedded form, prefixed with `embedded://` so
        // Bevy's asset server resolves it correctly even though
        // there's no `assets/` directory shipping the file.
        "embedded://ffxi_viewer_core/skybox.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Opaque
    }
}

/// Tag for the singleton skybox entity so the per-frame system can
/// find and reposition it without scanning every entity.
#[derive(Component)]
pub struct SkyboxSphere;

/// Spawn the inverted skybox sphere once at world setup.
///
/// The sphere's mesh winds outward; we render the **front** face only
/// after flipping the surface by negating the X scale — that means
/// fragments hit the interior, which is what we want for a sky dome.
/// Setting `unlit: false` doesn't matter because our material has its
/// own fragment shader and doesn't touch StandardMaterial.
fn spawn_skybox_sphere(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<SkyboxGradientMaterial>>,
) {
    let mesh = meshes.add(Sphere::new(SKYBOX_RADIUS).mesh().uv(32, 16));
    let material = materials.add(SkyboxGradientMaterial::default());
    commands.spawn((
        InGameEntity,
        SkyboxSphere,
        Mesh3d(mesh),
        MeshMaterial3d(material),
        // Negative-X scale flips the winding so the inside of the
        // sphere becomes the front-facing surface. The camera sits
        // inside the sphere and sees the gradient. NotShadow* prevents
        // the sphere from absorbing or projecting shadows.
        Transform::from_scale(Vec3::new(-1.0, 1.0, 1.0)),
        Visibility::default(),
        bevy::light::NotShadowCaster,
        bevy::light::NotShadowReceiver,
    ));
}

/// Keep the skybox centered on the camera each frame and push the
/// current weather keyframe into the material uniform. Without the
/// reposition the player would walk "out of the sky" once they
/// moved more than `SKYBOX_RADIUS` from world origin.
fn update_skybox(
    zone_weather: Res<ZoneWeather>,
    cam_q: Query<&Transform, (With<crate::camera::OperatorCamera>, Without<SkyboxSphere>)>,
    mut sky_q: Query<(&mut Transform, &MeshMaterial3d<SkyboxGradientMaterial>), With<SkyboxSphere>>,
    mut mats: ResMut<Assets<SkyboxGradientMaterial>>,
) {
    let cam_pos = cam_q.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);
    let Ok((mut sky_xf, sky_mat)) = sky_q.single_mut() else {
        return;
    };
    // Re-center on the camera but keep the flipping scale.
    sky_xf.translation = cam_pos;

    // Drive material from the current weather keyframe. The lerp
    // logic — sampling at the current Vana minute and blending two
    // bracketing records — already lives in `ZoneWeather` /
    // `sample_weather`. We reuse the same access pattern as
    // `apply_zone_weather` in `weather.rs`.
    if zone_weather.records.is_empty() {
        return;
    }
    let sky = crate::sun_moon::vana_sky_now();
    let time_minutes = (sky.hour * 60.0).rem_euclid(1440.0) as u32;
    let Some(rec) = ffxi_dat::weather::sample_weather(&zone_weather.records, time_minutes) else {
        return;
    };
    let Some(mat) = mats.get_mut(&sky_mat.0) else {
        return;
    };
    // Convert WeatherRecord's [[f32; 4]; 8] into [Vec4; 8].
    for i in 0..8 {
        let c = rec.skybox_colors[i];
        mat.colors[i] = Vec4::new(c[0], c[1], c[2], c[3]);
    }
    // Pack 8 altitudes into 2 vec4s.
    let a = rec.skybox_altitudes;
    mat.altitudes_packed = [
        Vec4::new(a[0], a[1], a[2], a[3]),
        Vec4::new(a[4], a[5], a[6], a[7]),
    ];
}

pub struct SkyboxPlugin;

impl Plugin for SkyboxPlugin {
    fn build(&self, app: &mut App) {
        // Register the WGSL fragment shader as an embedded asset so
        // we don't ship a separate `assets/` directory just for one
        // shader. The path is relative to this source file; Bevy
        // exposes it via `embedded://ffxi_viewer_core/skybox.wgsl`,
        // which `SkyboxGradientMaterial::fragment_shader` returns.
        embedded_asset!(app, "skybox.wgsl");
        app.add_plugins(MaterialPlugin::<SkyboxGradientMaterial>::default())
            .add_systems(Startup, spawn_skybox_sphere)
            .add_systems(Update, update_skybox);
    }
}
