use bevy::asset::embedded_asset;
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::components::InGameEntity;
use crate::weather::ZoneWeather;

/// World radius the camera-centred gradient dome is drawn at. Everything else
/// sky — canopy rim, sun and moon discs — is placed inside it.
pub const SKYBOX_RADIUS: f32 = 5500.0;

/// Frustum headroom past the dome, so the far plane can never clip it.
const SKY_FAR_MARGIN: f32 = 100.0;

/// Transparent-phase sort depths for the camera-anchored sky layers, back to
/// front.
///
/// Bevy ranks `Transparent3d` by the mesh AABB centre's view-space Z
/// (bevy_core_pipeline-0.19.0 src/core_3d/mod.rs TransparentSortingInfo3d), which increases toward the
/// camera. A layer that rides the camera has its centre *on* it, so that rank is
/// 0 — the nearest value there is — and it draws over every other transparent
/// object rather than behind them. Transparent draws leave the depth buffer
/// alone, so against sky the canopy still passed the depth test and blended over
/// the nameplates and particles already there (kuluu-w4jf). Each layer overrides
/// the rank with a `Material::depth_bias` (bevy_pbr material.rs opaque_render_method — added
/// to the sort distance and used for nothing else); the opaque dome bounds
/// everything visible, so a depth at its radius cannot be beaten by world
/// geometry.
pub const SKY_SORT_DEPTH_STARS: f32 = -SKYBOX_RADIUS;
pub const SKY_SORT_DEPTH_CLOUDS: f32 = SKY_SORT_DEPTH_STARS + SKY_LAYER_SORT_STEP;

const SKY_LAYER_SORT_STEP: f32 = 1.0;

/// Camera far plane.
///
/// The sky sits at fixed world radii and the world ends at the zone draw
/// distance (dat_mzb.rs zone_draw_distance, a few thousand yalms at most), so
/// the frustum only has to reach past the dome. Costs nothing: bevy's perspective is
/// `Mat4::perspective_infinite_reverse_rh(fov, aspect, near)`
/// (bevy_camera-0.19.0 src/projection.rs `impl CameraProjection for PerspectiveProjection`), so `far` feeds frustum culling only and
/// never the depth range.
///
/// Sizing the sky off the frustum instead — retail's
/// `(FarClipPlane - NearClipPlane) * 0.8` (research/XIClient World/Zone/
/// XiZone.cpp DrawSky v17) — is only right in a renderer that draws the sky in its
/// own pass. Sharing the world's depth buffer and fog the way we do, it
/// collapsed the canopy onto the camera at 200 (`layer_scale`'s rim factor
/// clamps at 1.0, so one cloud tile filled the sky) and left the discs behind
/// terrain.
pub const CAMERA_FAR: f32 = SKYBOX_RADIUS + SKY_FAR_MARGIN;
const _: () = assert!(CAMERA_FAR > SKYBOX_RADIUS);

#[derive(Clone, Debug, ShaderType)]
pub struct SkyboxUniform {
    pub colors: [Vec4; 8],
    pub altitudes_packed: [Vec4; 2],

    pub cloud_params: Vec4,

    pub extra: Vec4,
}

#[derive(Asset, AsBindGroup, Clone, Debug, TypePath)]
pub struct SkyboxGradientMaterial {
    #[uniform(0)]
    pub data: SkyboxUniform,
}

impl Default for SkyboxGradientMaterial {
    fn default() -> Self {
        let horizon = Vec4::new(0.15, 0.20, 0.35, 1.0);
        let mid = Vec4::new(0.35, 0.55, 0.85, 1.0);
        let zenith = Vec4::new(0.55, 0.75, 0.95, 1.0);
        Self {
            data: SkyboxUniform {
                colors: [horizon, horizon, mid, mid, mid, zenith, zenith, zenith],

                // Coupled to skybox.wesl's lookup parameter: the shader brackets
                // against a polar-angle fraction in [0, 1] (horizon -> zenith),
                // which is the domain every shipped 0x2F record uses. This
                // placeholder ramp only shows before a record loads, but it has to
                // live in that domain or the fallback gradient reads as a hard
                // band at the horizon.
                altitudes_packed: [
                    Vec4::new(0.0, 1.0 / 7.0, 2.0 / 7.0, 3.0 / 7.0),
                    Vec4::new(4.0 / 7.0, 5.0 / 7.0, 6.0 / 7.0, 1.0),
                ],

                // Procedural FBM clouds retired in favour of the weat/<type>/
                // mesh clouds (zone_clouds.rs); the gradient dome carries no
                // cloud layer, so the uniform stays zero for layout compat.
                cloud_params: Vec4::ZERO,
                extra: Vec4::ZERO,
            },
        }
    }
}

impl Material for SkyboxGradientMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://kuluu_render/skybox.wesl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        if self.data.extra.x > 0.5 {
            AlphaMode::Blend
        } else {
            AlphaMode::Opaque
        }
    }
}

#[cfg(test)]
mod frustum_shell_tests {
    use super::*;

    // Bevy sorts Transparent3d ascending on view-space Z, so a *smaller* sort
    // depth draws earlier. Every sky layer must therefore land at or behind the
    // dome — no world object, all of which the opaque dome bounds, can reach
    // that — and the star field must land behind the canopy that occludes it.
    #[test]
    fn every_sky_layer_sorts_behind_the_world_and_in_back_to_front_order() {
        for depth in [SKY_SORT_DEPTH_STARS, SKY_SORT_DEPTH_CLOUDS] {
            assert!(
                depth <= -SKYBOX_RADIUS + SKY_LAYER_SORT_STEP,
                "sky sort depth {depth} is nearer than the {SKYBOX_RADIUS} dome, \
                 so world geometry can sort behind it"
            );
        }
        const { assert!(SKY_SORT_DEPTH_STARS < SKY_SORT_DEPTH_CLOUDS) };
    }
}

#[derive(Component)]
pub struct SkyboxSphere;

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
        Transform::from_scale(Vec3::new(-1.0, 1.0, 1.0)),
        Visibility::default(),
        bevy::light::NotShadowCaster,
        bevy::light::NotShadowReceiver,
    ));
}

#[allow(clippy::type_complexity)]
fn update_skybox(
    zone_weather: Res<ZoneWeather>,
    cam_q: Query<&Transform, (With<crate::camera::OperatorCamera>, Without<SkyboxSphere>)>,
    mut sky_q: Query<(&mut Transform, &MeshMaterial3d<SkyboxGradientMaterial>), With<SkyboxSphere>>,
    mut mats: ResMut<Assets<SkyboxGradientMaterial>>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    mut prev_keyframe_time: Local<Option<u32>>,
    mut last_applied: Local<Option<([Vec4; 8], [Vec4; 2])>>,
) {
    let cam_pos = cam_q.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);

    let sky_mat = if let Ok((mut sky_xf, sky_mat)) = sky_q.single_mut() {
        if sky_xf.translation != cam_pos {
            sky_xf.translation = cam_pos;
        }
        Some(sky_mat.0.clone())
    } else {
        None
    };

    // Single shared per-frame sample (weather::sample_zone_weather) avoids the
    // skybox/lighting drift from independently re-sampling. research/xim
    // EnvironmentManager.kt computeInterpolatedEnvResource.
    let gradient: Option<([Vec4; 8], [Vec4; 2])> = zone_weather.current.map(|rec| {
        let sky = crate::sun_moon::vana_sky_from_clock(&vana_clock);
        let v_minutes = (sky.hour * 60.0).rem_euclid(1440.0) as u32;
        let active_keyframe_time = zone_weather
            .records
            .iter()
            .rev()
            .find(|r| r.time_minutes <= v_minutes)
            .or_else(|| zone_weather.records.last())
            .map(|r| r.time_minutes);
        if active_keyframe_time.is_some() && *prev_keyframe_time != active_keyframe_time {
            if let (Some(prev), Some(now)) = (*prev_keyframe_time, active_keyframe_time) {
                toasts.write(crate::snapshot::ToastEvent::debug(format!(
                    "🌅 Skybox keyframe V{:02}:{:02} → V{:02}:{:02}",
                    prev / 60,
                    prev % 60,
                    now / 60,
                    now % 60,
                )));
            }
            *prev_keyframe_time = active_keyframe_time;
        }

        let to_linear = |srgb: [f32; 4]| -> [f32; 4] {
            let lin = Color::srgb(srgb[0], srgb[1], srgb[2]).to_linear();
            [lin.red, lin.green, lin.blue, srgb[3]]
        };
        let mut colors = [Vec4::ZERO; 8];
        for (i, color) in colors.iter_mut().enumerate() {
            let c = to_linear(rec.skybox_colors[i]);
            *color = Vec4::new(c[0], c[1], c[2], c[3]);
        }
        let a = rec.skybox_altitudes;
        let altitudes = [
            Vec4::new(a[0], a[1], a[2], a[3]),
            Vec4::new(a[4], a[5], a[6], a[7]),
        ];
        (colors, altitudes)
    });

    // Tracked get_mut marks the material Modified (uniform re-encode +
    // bind-group rebuild), so only write when the sampled gradient — which
    // steps once per Vana'diel minute — actually changed.
    if let (Some(handle), Some((colors, altitudes))) = (sky_mat, gradient) {
        if *last_applied != Some((colors, altitudes)) {
            if let Some(mut mat) = mats.get_mut(&handle) {
                mat.data.colors = colors;
                mat.data.altitudes_packed = altitudes;
                *last_applied = Some((colors, altitudes));
            }
        }
    }
}

pub struct SkyboxPlugin;

impl Plugin for SkyboxPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "skybox.wesl");
        app.add_plugins(MaterialPlugin::<SkyboxGradientMaterial>::default())
            .add_systems(Startup, spawn_skybox_sphere)
            .add_systems(
                Update,
                update_skybox.after(crate::weather::WeatherSampleSet),
            );
    }
}
