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

// Separate dome so Enhanced keeps clouds: Bevy's atmosphere hides the opaque
// skybox and draws no clouds itself, so this transparent layer carries them.
#[derive(Component)]
pub struct SkyCloudLayer;

const CLOUD_LAYER_RADIUS: f32 = 5400.0;

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

    let cloud_mesh = meshes.add(Sphere::new(CLOUD_LAYER_RADIUS).mesh().uv(32, 16));
    let cloud_material = materials.add(SkyboxGradientMaterial {
        data: SkyboxUniform {
            extra: Vec4::new(1.0, 0.0, 0.0, 0.0),
            ..SkyboxGradientMaterial::default().data
        },
    });
    commands.spawn((
        InGameEntity,
        SkyCloudLayer,
        Mesh3d(cloud_mesh),
        MeshMaterial3d(cloud_material),
        Transform::from_scale(Vec3::new(-1.0, 1.0, 1.0)),
        Visibility::Hidden,
        bevy::light::NotShadowCaster,
        bevy::light::NotShadowReceiver,
    ));
}

#[allow(clippy::type_complexity)]
fn update_skybox(
    zone_weather: Res<ZoneWeather>,
    cam_q: Query<
        &Transform,
        (
            With<crate::camera::OperatorCamera>,
            Without<SkyboxSphere>,
            Without<SkyCloudLayer>,
        ),
    >,
    mut sky_q: Query<
        (&mut Transform, &MeshMaterial3d<SkyboxGradientMaterial>),
        (With<SkyboxSphere>, Without<SkyCloudLayer>),
    >,
    mut cloud_q: Query<
        (
            &mut Transform,
            &mut Visibility,
            &MeshMaterial3d<SkyboxGradientMaterial>,
        ),
        (With<SkyCloudLayer>, Without<SkyboxSphere>),
    >,
    mut mats: ResMut<Assets<SkyboxGradientMaterial>>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    settings: Res<crate::graphics_settings::GraphicsSettings>,
    time: Res<Time>,
    mut prev_keyframe_time: Local<Option<u32>>,
) {
    let cam_pos = cam_q.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);
    let enhanced = settings.sky_embellishments_enabled();
    let t = time.elapsed_secs();
    let animated_clouds = Vec4::new(0.55, 0.85, t * 0.010, t * 0.004);

    let cloud_mat = if let Ok((mut cxf, mut cvis, cmat)) = cloud_q.single_mut() {
        cxf.translation = cam_pos;
        // wasm has no atmosphere, so the gradient dome keeps its own clouds;
        // showing this layer there would double them.
        let want = if enhanced && !cfg!(target_arch = "wasm32") {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *cvis != want {
            *cvis = want;
        }
        Some(cmat.0.clone())
    } else {
        None
    };

    let sky_mat = if let Ok((mut sky_xf, sky_mat)) = sky_q.single_mut() {
        sky_xf.translation = cam_pos;
        Some(sky_mat.0.clone())
    } else {
        None
    };

    let gradient: Option<([Vec4; 8], [Vec4; 2])> = (!zone_weather.records.is_empty())
        .then(|| {
            let sky = crate::sun_moon::vana_sky_from_clock(&vana_clock);
            let v_minutes = (sky.hour * 60.0).rem_euclid(1440.0);
            let m0 = v_minutes.floor() as u32 % 1440;
            let m1 = (m0 + 1) % 1440;
            let frac = v_minutes - v_minutes.floor();
            let r0 = ffxi_dat::weather::sample_weather(&zone_weather.records, m0)?;
            let r1 = ffxi_dat::weather::sample_weather(&zone_weather.records, m1)?;

            let active_keyframe_time = zone_weather
                .records
                .iter()
                .rev()
                .find(|r| r.time_minutes <= m0)
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

            let lerp = |a: f32, b: f32| a + (b - a) * frac;
            let to_linear = |srgb: [f32; 4]| -> [f32; 4] {
                let lin = Color::srgb(srgb[0], srgb[1], srgb[2]).to_linear();
                [lin.red, lin.green, lin.blue, srgb[3]]
            };
            let mut colors = [Vec4::ZERO; 8];
            for i in 0..8 {
                let c0 = to_linear(r0.skybox_colors[i]);
                let c1 = to_linear(r1.skybox_colors[i]);
                colors[i] = Vec4::new(
                    lerp(c0[0], c1[0]),
                    lerp(c0[1], c1[1]),
                    lerp(c0[2], c1[2]),
                    lerp(c0[3], c1[3]),
                );
            }
            let a0 = r0.skybox_altitudes;
            let a1 = r1.skybox_altitudes;
            let altitudes = [
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
            Some((colors, altitudes))
        })
        .flatten();

    if let Some(handle) = sky_mat {
        if let Some(mat) = mats.get_mut(&handle) {
            if let Some((colors, altitudes)) = gradient {
                mat.data.colors = colors;
                mat.data.altitudes_packed = altitudes;
            }
            mat.data.cloud_params = if enhanced {
                animated_clouds
            } else {
                Vec4::new(0.55, 0.0, 0.0, 0.0)
            };
            mat.data.extra.x = 0.0;
        }
    }

    if let Some(handle) = cloud_mat {
        if let Some(mat) = mats.get_mut(&handle) {
            if let Some((colors, altitudes)) = gradient {
                mat.data.colors = colors;
                mat.data.altitudes_packed = altitudes;
            }
            mat.data.cloud_params = animated_clouds;
            mat.data.extra.x = 1.0;
        }
    }
}

pub struct SkyboxPlugin;

impl Plugin for SkyboxPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "skybox.wgsl");
        app.add_plugins(MaterialPlugin::<SkyboxGradientMaterial>::default())
            .add_systems(Startup, spawn_skybox_sphere)
            .add_systems(Update, update_skybox);
    }
}
