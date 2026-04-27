use bevy::core_pipeline::Skybox;
use bevy::light::light_consts;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;

use crate::camera::OperatorCamera;
use crate::snapshot::SceneState;

pub fn ffxi_distance_fog() -> DistanceFog {
    let fog_color = Color::srgba(0.62, 0.68, 0.76, 1.0);

    let sun_inscatter = Color::srgb(0.78, 0.82, 0.88);

    let visibility = 4000.0;
    DistanceFog {
        color: fog_color,
        directional_light_color: sun_inscatter,

        directional_light_exponent: 80.0,
        falloff: FogFalloff::from_visibility_colors(visibility, fog_color, sun_inscatter),
    }
}

#[derive(Clone)]
pub struct ZoneAtmosphere {
    pub ambient_color: Color,
    pub ambient_brightness: f32,

    pub sun_direction: Vec3,
    pub sun_color: Color,
    pub sun_illuminance: f32,

    pub fog: Option<DistanceFog>,

    pub skybox: Option<Handle<Image>>,
}

impl ZoneAtmosphere {
    pub fn outdoor() -> Self {
        Self {
            ambient_color: Color::srgb(0.82, 0.86, 1.00),
            ambient_brightness: 130.0,
            sun_direction: Vec3::new(0.4, 0.85, 0.35).normalize(),
            sun_color: Color::srgb(1.00, 0.96, 0.88),
            sun_illuminance: light_consts::lux::AMBIENT_DAYLIGHT,

            fog: None,
            skybox: None,
        }
    }

    pub fn indoor() -> Self {
        Self {
            ambient_color: Color::srgb(0.92, 0.92, 0.95),
            ambient_brightness: 250.0,
            sun_direction: Vec3::new(0.2, 0.9, 0.2).normalize(),
            sun_color: Color::srgb(1.00, 0.98, 0.92),
            sun_illuminance: 2_500.0,
            fog: None,
            skybox: None,
        }
    }

    pub fn cave() -> Self {
        Self {
            ambient_color: Color::srgb(0.55, 0.60, 0.75),
            ambient_brightness: 35.0,
            sun_direction: Vec3::Y,
            sun_color: Color::srgb(0.7, 0.8, 1.0),
            sun_illuminance: 600.0,

            fog: None,
            skybox: None,
        }
    }
}

#[derive(Resource)]
pub struct ZoneAtmosphereProvider(pub Box<dyn Fn(u16) -> ZoneAtmosphere + Send + Sync>);

impl Default for ZoneAtmosphereProvider {
    fn default() -> Self {
        Self(Box::new(|zone_id: u16| match zone_id {
            230..=246 => ZoneAtmosphere::indoor(),

            193..=199 | 207..=215 => ZoneAtmosphere::cave(),

            _ => ZoneAtmosphere::outdoor(),
        }))
    }
}

#[derive(Resource, Default)]
pub struct LastAtmosphereZone {
    pub zone_id: Option<u16>,
}

pub fn apply_zone_atmosphere_system(
    state: Res<SceneState>,
    provider: Res<ZoneAtmosphereProvider>,
    mut last: ResMut<LastAtmosphereZone>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut active_weather: ResMut<crate::weather_fx::ActiveWeatherModifier>,
    mut q_cam: Query<(Entity, Option<&mut DistanceFog>, Option<&Skybox>), With<OperatorCamera>>,
    mut commands: Commands,
) {
    let current = state.snapshot.zone_id;
    if current == last.zone_id {
        return;
    }
    last.zone_id = current;
    let Some(zone_id) = current else { return };

    let atmo = (provider.0)(zone_id);

    ambient.color = atmo.ambient_color;
    ambient.brightness = atmo.ambient_brightness;

    active_weather.base_ambient_color = atmo.ambient_color;
    active_weather.base_ambient_brightness = atmo.ambient_brightness;

    if let Ok((cam_entity, fog_slot, skybox_slot)) = q_cam.single_mut() {
        if let Some(new_fog) = atmo.fog {
            match fog_slot {
                Some(mut existing) => *existing = new_fog,
                None => {
                    commands.entity(cam_entity).insert(new_fog);
                }
            }
        }
        match (atmo.skybox, skybox_slot) {
            (Some(handle), _) => {
                commands.entity(cam_entity).insert(Skybox {
                    image: handle,
                    brightness: 1000.0,
                    ..default()
                });
            }
            (None, Some(_)) => {
                commands.entity(cam_entity).remove::<Skybox>();
            }
            (None, None) => {}
        }
    }
}
