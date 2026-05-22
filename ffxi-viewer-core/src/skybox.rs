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
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::components::InGameEntity;
use crate::weather::ZoneWeather;

/// Radius of the inverted skybox sphere. Must be larger than the
/// sun/moon discs (which orbit at `sun_moon::SKY_RADIUS = 4000`) so
/// the celestial discs sit *inside* the sphere and win the depth
/// test against the sky surface — otherwise the opaque sky fragments
/// occlude the sun and moon. Stays under the camera's far-clip
/// override of `6000` set in `spawn_camera`.
const SKYBOX_RADIUS: f32 = 5500.0;

/// 8 colors + 8 altitudes that drive the gradient, packed into a
/// single uniform block.
///
/// Why one block instead of two separate `#[uniform(N)]` fields:
/// WGSL forbids top-level `var<uniform>` of bare array types —
/// uniform globals must be structs. wgpu's validator surfaces the
/// violation as a cryptic "Storage / Uniform" pipeline-layout
/// mismatch. Wrapping both arrays inside one `ShaderType` struct
/// satisfies the spec on both sides cleanly.
///
/// Packing convention: each `Vec4` is 16-byte aligned in `std140`
/// uniform layout, so `colors: [Vec4; 8]` consumes 128 bytes and
/// `altitudes_packed: [Vec4; 2]` consumes 32 bytes for a total of
/// 160 bytes — well under wgpu's 64KB uniform limit.
#[derive(Clone, Debug, ShaderType)]
pub struct SkyboxUniform {
    pub colors: [Vec4; 8],
    pub altitudes_packed: [Vec4; 2],
}

#[derive(Asset, AsBindGroup, Clone, Debug, TypePath)]
pub struct SkyboxGradientMaterial {
    #[uniform(0)]
    pub data: SkyboxUniform,
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
            data: SkyboxUniform {
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
            },
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
    mut scene_state: ResMut<crate::snapshot::SceneState>,
    mut prev_keyframe_time: Local<Option<u32>>,
) {
    let cam_pos = cam_q.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);
    let Ok((mut sky_xf, sky_mat)) = sky_q.single_mut() else {
        return;
    };
    // Re-center on the camera but keep the flipping scale.
    sky_xf.translation = cam_pos;

    // Drive material from the current weather keyframe. We sub-V-minute
    // interpolate so colors animate smoothly between integer V-minute
    // ticks. `sample_weather` only accepts `u32 time_minutes`, which
    // quantizes to ~0.42 s real-time steps (1 V-min = 25/60 real-sec).
    // For 8 simultaneously visible colors that step is conspicuous.
    // Sampling at floor + ceil and lerping by the fractional V-minute
    // composes to the same straight-line lerp that `lerp_records`
    // produces internally, with f32 precision instead of u32.
    //
    // sRGB conversion: `WeatherRecord.skybox_colors` are stored
    // sRGB-decoded (`/255.0`), not linear (see `ffxi_dat::weather`
    // module docs lines 66-69). Bevy's render pipeline expects
    // linear-space colors and tonemaps to sRGB on output, so passing
    // raw f32s here would render every keyframe ~2.2-gamma darker
    // than authored. `weather.rs::apply_zone_weather` correctly wraps
    // its fog/ambient colors in `Color::srgb(..)`; we do the same
    // here per channel before the lerp so blended values are
    // interpolated in linear space (additive-light correct), not
    // in sRGB space (which biases mid-tones toward "muddy").
    if zone_weather.records.is_empty() {
        return;
    }
    let sky = crate::sun_moon::vana_sky_now();
    let v_minutes = (sky.hour * 60.0).rem_euclid(1440.0);
    let m0 = v_minutes.floor() as u32 % 1440;
    let m1 = (m0 + 1) % 1440;
    let frac = v_minutes - v_minutes.floor();
    let Some(r0) = ffxi_dat::weather::sample_weather(&zone_weather.records, m0) else {
        return;
    };
    let Some(r1) = ffxi_dat::weather::sample_weather(&zone_weather.records, m1) else {
        return;
    };
    // Keyframe-segment edge — fires only when we cross from one
    // authored weather record into the next. Zone DATs typically
    // carry 4–7 keyframes (e.g. 0000/0500/0600/1200/1700/1800/2100),
    // so this is ~7 lines per V-day (~10 Earth min). We resolve the
    // active *authored* keyframe time directly from `records`
    // because `sample_weather` overwrites the returned record's
    // `time_minutes` with the queried V-minute — reading r0's field
    // back would tell us nothing about the underlying segment and
    // would fire every V-minute (the original bug).
    let active_keyframe_time = zone_weather
        .records
        .iter()
        .rev()
        .find(|r| r.time_minutes <= m0)
        .or_else(|| zone_weather.records.last())
        .map(|r| r.time_minutes);
    if active_keyframe_time.is_some() && *prev_keyframe_time != active_keyframe_time {
        if let (Some(prev), Some(now)) = (*prev_keyframe_time, active_keyframe_time) {
            scene_state.push_local_toast(crate::snapshot::debug_chat_line(format!(
                "🌅 Skybox keyframe V{:02}:{:02} → V{:02}:{:02}",
                prev / 60,
                prev % 60,
                now / 60,
                now % 60,
            )));
        }
        *prev_keyframe_time = active_keyframe_time;
    }
    let Some(mat) = mats.get_mut(&sky_mat.0) else {
        return;
    };
    let lerp = |a: f32, b: f32| a + (b - a) * frac;
    let to_linear = |srgb: [f32; 4]| -> [f32; 4] {
        let lin = Color::srgb(srgb[0], srgb[1], srgb[2]).to_linear();
        [lin.red, lin.green, lin.blue, srgb[3]]
    };
    for i in 0..8 {
        let c0 = to_linear(r0.skybox_colors[i]);
        let c1 = to_linear(r1.skybox_colors[i]);
        mat.data.colors[i] = Vec4::new(
            lerp(c0[0], c1[0]),
            lerp(c0[1], c1[1]),
            lerp(c0[2], c1[2]),
            lerp(c0[3], c1[3]),
        );
    }
    // Altitudes are scalar bin thresholds, not colors — no gamma
    // conversion. Sub-V-minute lerped along with the colors.
    let a0 = r0.skybox_altitudes;
    let a1 = r1.skybox_altitudes;
    mat.data.altitudes_packed = [
        Vec4::new(
            lerp(a0[0], a1[0]),
            lerp(a0[1], a1[1]),
            lerp(a0[2], a1[2]),
            lerp(a0[3], a1[3]),
        ),
        Vec4::new(
            lerp(a0[4], a1[4]),
            lerp(a0[5], a1[5]),
            lerp(a0[6], a1[6]),
            lerp(a0[7], a1[7]),
        ),
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
