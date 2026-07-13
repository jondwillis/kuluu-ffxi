#![cfg(not(target_arch = "wasm32"))]

use std::fs;

use bevy::light::FogVolume;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use ffxi_dat::weather::{
    collect_zone_weather_sets, sample_weather, weather_type_id, WeatherRecord, WeatherTypeId,
    ZoneWeatherSets,
};
use ffxi_dat::DatRoot;
use ffxi_viewer_wire::Weather;

use crate::camera::OperatorCamera;
use crate::graphics_settings::GraphicsSettings;
use crate::snapshot::SceneState;

#[derive(Resource, Default)]
pub struct ZoneWeather {
    // Grouped per-weather-type / indoor sets for the loaded zone. The active set
    // is selected into `records` by (weather type, indoor) each frame.
    pub sets: ZoneWeatherSets,

    // The active (weather-type, indoor)-selected record set, sorted by time.
    pub records: Vec<WeatherRecord>,

    // Cache: which (weather-type fourcc, indoor) `records` currently mirrors, so
    // we only re-select on change.
    selected: Option<(WeatherTypeId, bool)>,

    pub file_id: Option<u32>,

    // research/xim EnvironmentManager.kt:399-451: one interpolated env source per
    // frame; skybox/lighting/sun_moon all read this instead of independently
    // re-sampling (was the skybox/lighting drift).
    pub current: Option<WeatherRecord>,
}

// wire::Weather shares the LSB weather.h discriminant ordering, so the variant
// index is the LSB weather id consumed by ffxi_dat::weather::weather_type_id (the
// authoritative weather-id -> weat/<type> subdir table).
fn weather_type_fourcc(weather: Option<Weather>) -> WeatherTypeId {
    weather_type_id(weather.unwrap_or(Weather::None) as u16)
}

// Pick the set for the requested weather type, falling back across the base sky
// families that actually ship before giving up.
fn select_records(sets: &ZoneWeatherSets, want: WeatherTypeId, indoor: bool) -> Vec<WeatherRecord> {
    if !sets.flat.is_empty() {
        return sets.flat.clone();
    }
    let pick = |id: &WeatherTypeId| {
        sets.by_type.get(id).map(|set| {
            let chosen = if indoor && !set.indoor.is_empty() {
                &set.indoor
            } else {
                &set.outdoor
            };
            chosen.clone()
        })
    };
    pick(&want)
        .or_else(|| pick(b"suny"))
        .or_else(|| pick(b"fine"))
        .or_else(|| pick(b"clod"))
        .or_else(|| pick(b"mist"))
        .or_else(|| sets.by_type.values().next().map(|s| s.outdoor.clone()))
        .unwrap_or_default()
}

// Cross-plugin ordering anchor: sample_zone_weather populates ZoneWeather.current
// before any consumer (apply_zone_weather, skybox::update_skybox, sun_moon) reads it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct WeatherSampleSet;

// research/xim EnvironmentSection.kt:130-172: the 0x2F record carries two distinct
// LightConfig blocks — model(entity) lighting for actors and terrain(landscape)
// lighting for zone geometry. sun_moon_system derives this from ZoneWeather.current
// each frame so the actor-material and zone-material consumers read one source
// instead of re-deriving from the synthetic sun/moon DirectionalLights.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct ZoneDirectionalLighting {
    pub valid: bool,
    pub indoors: bool,

    // Single time-blended model light (research/xim EnvironmentSection.kt:206-225
    // modelLightMix): moon<->sun cross-fade over minutes 355..365 / 1075..1085.
    pub model_dir: Vec3,
    pub model_color: Vec3,
    pub model_k: f32,
    pub ambient_entity: Vec3,

    // Terrain block feeds the zone material's sun(dir0)+moon(dir1) slots.
    pub sun_dir: Vec3,
    pub sun_color: Vec3,
    pub sun_k: f32,
    pub moon_dir: Vec3,
    pub moon_color: Vec3,
    pub moon_k: f32,
    pub ambient_landscape: Vec3,
}

pub struct WeatherPlugin;

impl Plugin for WeatherPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneWeather>().add_systems(
            Update,
            (
                load_zone_weather,
                sample_zone_weather
                    .in_set(WeatherSampleSet)
                    .after(load_zone_weather),
                // research/xim EnvironmentManager.kt:399-445: the 0x2F record is the
                // authoritative ambient base and weather modulates it. Run AFTER
                // apply_weather_to_ambient_and_fog (which recomputes ambient from the
                // hardcoded atmosphere seed) so the DAT base is the final word, not the
                // atmosphere.rs outdoor/indoor/cave clobber.
                apply_zone_weather
                    .after(WeatherSampleSet)
                    .after(crate::weather_fx::apply_weather_to_ambient_and_fog_system),
            ),
        );
    }
}

pub fn sample_zone_weather(
    mut zone_weather: ResMut<ZoneWeather>,
    current_weather: Res<crate::weather_fx::CurrentWeather>,
    vana_clock: Res<crate::vana_time::VanaClock>,
) {
    if zone_weather.sets.is_empty() {
        zone_weather.records.clear();
        zone_weather.selected = None;
        zone_weather.current = None;
        return;
    }

    // No zone-indoor flag is sourced in viewer-core yet; outdoor sky is the
    // default. The DAT-level indo/ records are retained for when a zone indoor
    // flag is plumbed (spec correction 4).
    let indoor = false;
    // The `selected` cache re-runs select_records whenever `want` changes, so the
    // active set reloads on CurrentWeather change as well as zone change.
    let want = weather_type_fourcc(current_weather.0);
    if zone_weather.selected != Some((want, indoor)) {
        zone_weather.records = select_records(&zone_weather.sets, want, indoor);
        zone_weather.selected = Some((want, indoor));
    }

    if zone_weather.records.is_empty() {
        zone_weather.current = None;
        return;
    }
    let sky = crate::sun_moon::vana_sky_from_clock(&vana_clock);
    let time_minutes = (sky.hour * 60.0).rem_euclid(1440.0) as u32;
    zone_weather.current = sample_weather(&zone_weather.records, time_minutes);
}

pub fn load_zone_weather(
    scene_state: Res<SceneState>,
    mut zone_weather: ResMut<ZoneWeather>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
) {
    let current = crate::snapshot::effective_zone_file_id(&scene_state.snapshot);
    if current == zone_weather.file_id {
        return;
    }
    zone_weather.file_id = current;
    zone_weather.sets = ZoneWeatherSets::default();
    zone_weather.records.clear();
    zone_weather.selected = None;

    if scene_state.snapshot.myroom.is_some() {
        return;
    }
    let Some(file_id) = current else { return };

    let Ok(root) = DatRoot::from_env_or_default() else {
        return;
    };
    let Ok(location) = root.resolve(file_id) else {
        return;
    };
    let path = location.path_under(root.root());
    let Ok(bytes) = fs::read(&path) else { return };
    zone_weather.sets = collect_zone_weather_sets(&bytes);

    if !zone_weather.sets.is_empty() {
        let types: Vec<String> = {
            let mut t: Vec<String> = zone_weather
                .sets
                .by_type
                .keys()
                .map(|k| k.iter().map(|&b| b as char).collect())
                .collect();
            t.sort();
            t
        };
        let summary = if types.is_empty() {
            format!("flat ({} keyframes)", zone_weather.sets.flat.len())
        } else {
            format!("types [{}]", types.join(", "))
        };
        info!(file_id, "zone weather loaded: {}", summary);
        toasts.write(crate::snapshot::ToastEvent::system(format!(
            "⛅ Zone weather loaded: DAT {file_id} ({summary})"
        )));
    }
}

pub fn apply_zone_weather(
    zone_weather: Res<ZoneWeather>,
    active: Res<crate::weather_fx::ActiveWeatherModifier>,
    mut fog_q: Query<&mut FogVolume>,
    mut ambient: ResMut<GlobalAmbientLight>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    settings: Res<GraphicsSettings>,
    mut cam_q: Query<(Entity, Option<&mut DistanceFog>), With<OperatorCamera>>,
    mut commands: Commands,
) {
    let Some(rec) = zone_weather.current else {
        return;
    };
    let sky = crate::sun_moon::vana_sky_from_clock(&vana_clock);

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

    // research/xim EnvironmentManager.kt:399-445: ambient_landscape is the
    // authoritative base; the active weather modifier tints/scales it rather than
    // replacing it (apply_weather_to_ambient_and_fog already ran on the now-overridden
    // atmosphere seed, so this is the final ambient for the frame).
    let [r, g, b, _a] = rec.ambient_landscape;
    let tint = active.modifier.ambient_tint.to_linear();
    ambient.color = Color::srgb(
        (r * tint.red).max(0.05),
        (g * tint.green).max(0.05),
        (b * tint.blue).max(0.05),
    );
    ambient.brightness =
        500.0 * rec.diffuse_mul_landscape.clamp(0.4, 1.5) * active.modifier.ambient_brightness_mul;

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
