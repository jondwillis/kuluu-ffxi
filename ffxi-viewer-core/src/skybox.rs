use bevy::asset::embedded_asset;
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::components::InGameEntity;
use crate::weather::ZoneWeather;

const SKYBOX_RADIUS: f32 = 5500.0;

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

                altitudes_packed: [
                    Vec4::new(-1.0, -0.5, -0.2, 0.0),
                    Vec4::new(0.2, 0.4, 0.7, 1.0),
                ],

                cloud_params: Vec4::new(0.5, 0.0, 0.0, 0.0),
                extra: Vec4::ZERO,
            },
        }
    }
}

impl Material for SkyboxGradientMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://ffxi_viewer_core/skybox.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        if self.data.extra.x > 0.5 {
            AlphaMode::Blend
        } else {
            AlphaMode::Opaque
        }
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
) {
    let cam_pos = cam_q.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);

    let sky_mat = if let Ok((mut sky_xf, sky_mat)) = sky_q.single_mut() {
        sky_xf.translation = cam_pos;
        Some(sky_mat.0.clone())
    } else {
        None
    };

    // Single shared per-frame sample (weather::sample_zone_weather) avoids the
    // skybox/lighting drift from independently re-sampling. research/xim
    // EnvironmentManager.kt:399-451.
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

    if let Some(handle) = sky_mat {
        if let Some(mat) = mats.get_mut(&handle) {
            if let Some((colors, altitudes)) = gradient {
                mat.data.colors = colors;
                mat.data.altitudes_packed = altitudes;
            }
            // Procedural FBM clouds retired in favour of the weat/<type>/ mesh
            // clouds (zone_clouds.rs); the gradient dome carries no cloud layer.
            mat.data.cloud_params = Vec4::ZERO;
            mat.data.extra.x = 0.0;
        }
    }
}

pub struct SkyboxPlugin;

impl Plugin for SkyboxPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "skybox.wgsl");
        app.add_plugins(MaterialPlugin::<SkyboxGradientMaterial>::default())
            .add_systems(Startup, spawn_skybox_sphere)
            .add_systems(
                Update,
                update_skybox.after(crate::weather::WeatherSampleSet),
            );
    }
}
