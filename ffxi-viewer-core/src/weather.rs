#![cfg(not(target_arch = "wasm32"))]

use std::fs;

use bevy::light::FogVolume;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use ffxi_dat::weather::{collect_weather_records, sample_weather, WeatherRecord};
use ffxi_dat::DatRoot;

use crate::camera::OperatorCamera;
use crate::graphics_settings::GraphicsSettings;
use crate::snapshot::SceneState;

#[derive(Resource, Default)]
pub struct ZoneWeather {
    pub records: Vec<WeatherRecord>,

    pub zone_id: Option<u16>,
}

pub struct WeatherPlugin;

impl Plugin for WeatherPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneWeather>().add_systems(
            Update,
            (
                load_zone_weather,
                apply_zone_weather
                    .before(crate::weather_fx::apply_weather_to_ambient_and_fog_system),
            ),
        );
    }
}

pub fn load_zone_weather(
    scene_state: Res<SceneState>,
    mut zone_weather: ResMut<ZoneWeather>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
) {
    let current = scene_state.snapshot.zone_id;
    if current == zone_weather.zone_id {
        return;
    }
    zone_weather.zone_id = current;
    zone_weather.records.clear();

    let Some(zone_id) = current else { return };
    let Some(file_id) = ffxi_dat::zone_dat::zone_id_to_mzb_file_id(zone_id) else {
        return;
    };

    let Ok(root) = DatRoot::from_env_or_default() else {
        return;
    };
    let Ok(location) = root.resolve(file_id) else {
        return;
    };
    let path = location.path_under(root.root());
    let Ok(bytes) = fs::read(&path) else { return };
    zone_weather.records = collect_weather_records(&bytes);

    if !zone_weather.records.is_empty() {
        let times: Vec<String> = zone_weather
            .records
            .iter()
            .map(|r| format!("{:02}:{:02}", r.time_minutes / 60, r.time_minutes % 60))
            .collect();
        info!(
            zone_id,
            count = zone_weather.records.len(),
            "weather keyframes loaded at V-times: {}",
            times.join(", ")
        );
        toasts.write(crate::snapshot::ToastEvent::system(format!(
            "⛅ Zone weather loaded: zone 0x{:04X} ({} keyframes)",
            zone_id,
            zone_weather.records.len(),
        )));
    }
}

pub fn apply_zone_weather(
    zone_weather: Res<ZoneWeather>,
    mut fog_q: Query<&mut FogVolume>,
    mut ambient: ResMut<GlobalAmbientLight>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    settings: Res<GraphicsSettings>,
    mut cam_q: Query<(Entity, Option<&mut DistanceFog>), With<OperatorCamera>>,
    mut commands: Commands,
) {
    if zone_weather.records.is_empty() {
        return;
    }
    let sky = crate::sun_moon::vana_sky_from_clock(&vana_clock);
    let time_minutes = (sky.hour * 60.0).rem_euclid(1440.0) as u32;
    let Some(rec) = sample_weather(&zone_weather.records, time_minutes) else {
        return;
    };

    if let Some(mut fog) = fog_q.iter_mut().next() {
        let [r, g, b, _a] = rec.fog_landscape;
        fog.fog_color = Color::srgb(r, g, b);

        let dist = rec.max_fog_dist_landscape.max(50.0);
        let mut density = (15.0 / dist).clamp(0.04, 0.18);

        let band = 3.0_f32;
        let dist_from_horizon = (sky.hour - 6.0).min(18.0 - sky.hour).max(0.0);
        let twilight = ((band - dist_from_horizon) / band).clamp(0.0, 1.0);
        let twilight_smooth = twilight * twilight * (3.0 - 2.0 * twilight);
        density = (density * (1.0 + 1.2 * twilight_smooth)).min(0.30);
        fog.density_factor = density;
    }

    let [r, g, b, _a] = rec.ambient_landscape;
    ambient.color = Color::srgb(r.max(0.05), g.max(0.05), b.max(0.05));
    ambient.brightness = 500.0 * rec.brightness_landscape.clamp(0.4, 1.5);

    if !settings.volumetric_fog {
        if let Ok((cam_entity, slot)) = cam_q.single_mut() {
            let [fr, fg, fb, _] = rec.fog_landscape;
            let color = Color::srgb(fr, fg, fb);

            let inscatter = Color::srgb(
                (fr * 1.08).min(1.0),
                (fg * 1.06).min(1.0),
                (fb * 1.02).min(1.0),
            );
            let visibility = rec.max_fog_dist_landscape.max(80.0);
            let want = DistanceFog {
                color,
                directional_light_color: inscatter,
                directional_light_exponent: 60.0,
                falloff: FogFalloff::from_visibility_colors(visibility, color, inscatter),
            };
            match slot {
                Some(mut existing) => *existing = want,
                None => {
                    commands.entity(cam_entity).insert(want);
                }
            }
        }
    }
}
