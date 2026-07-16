#[cfg(not(target_arch = "wasm32"))]
use std::fs;

use bevy::light::FogVolume;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use ffxi_dat::weather::collect_zone_weather_sets;
use ffxi_dat::weather::{
    sample_weather, weather_type_id, WeatherRecord, WeatherTypeId, ZoneWeatherSets,
};
#[cfg(not(target_arch = "wasm32"))]
use ffxi_dat::DatRoot;
use ffxi_viewer_wire::Weather;

use crate::camera::OperatorCamera;
use crate::graphics_settings::GraphicsSettings;
#[cfg(not(target_arch = "wasm32"))]
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
        // load_zone_weather is filesystem-backed (DatRoot), so it is native-only;
        // on wasm `sets` stays empty and sample_zone_weather leaves `current`
        // unset, which is every consumer's existing no-records fallback
        // (kuluu-ehye).
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Update, load_zone_weather.before(WeatherSampleSet));

        // kuluu-f1hk: remember the app's pre-weather backdrop so the fog
        // horizon painted by apply_zone_weather can be undone when no weather
        // record is active (weatherless zones, zone lines, volumetric off).
        app.init_resource::<DefaultClearColor>()
            .add_systems(PreStartup, capture_default_clear_color);

        app.init_resource::<ZoneWeather>().add_systems(
            Update,
            (
                sample_zone_weather.in_set(WeatherSampleSet),
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

#[cfg(not(target_arch = "wasm32"))]
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

// kuluu-f1hk: the app's pre-weather backdrop, captured at startup so the
// volumetric fog horizon can be restored instead of leaking a stale zone's
// color once no weather record is active.
#[derive(Resource, Clone, Copy)]
pub struct DefaultClearColor(pub Color);

impl Default for DefaultClearColor {
    fn default() -> Self {
        Self(ClearColor::default().0)
    }
}

fn capture_default_clear_color(
    clear_color: Option<Res<ClearColor>>,
    mut default: ResMut<DefaultClearColor>,
) {
    if let Some(clear) = clear_color {
        default.0 = clear.0;
    }
}

// The backdrop is a pure function of the current weather state so it can be
// written unconditionally every frame (kuluu-f1hk: the old code only wrote it
// on the volumetric-with-record path, so the painted horizon leaked across
// zone lines into weatherless zones and survived the volumetric-fog toggle).
//
// Retail fades the horizon to the zone fog color (the sky mesh does this in
// the client; headless captures and skyless indoor zones have no sky).
// Clearing to the daylight-scaled fog color gives the raymarch a matching
// backdrop instead of a hard black wall past the last drawn geometry. Every
// other state — no record (weatherless zone, wasm, mid-zone-line) or
// DistanceFog mode, which never owned the backdrop — restores the startup
// default.
pub(crate) fn zone_clear_color(
    rec: Option<&WeatherRecord>,
    volumetric_fog: bool,
    daylight_smooth: f32,
    default: Color,
) -> Color {
    match rec {
        Some(rec) if volumetric_fog => {
            let [fr, fg, fb, _] = rec.fog_landscape;
            let lum = 0.03 + 0.97 * daylight_smooth;
            Color::srgb(fr * lum, fg * lum, fb * lum)
        }
        _ => default,
    }
}

pub fn apply_zone_weather(
    zone_weather: Res<ZoneWeather>,
    active: Res<crate::weather_fx::ActiveWeatherModifier>,
    mut fog_q: Query<&mut FogVolume>,
    mut ambient: ResMut<GlobalAmbientLight>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    settings: Res<GraphicsSettings>,
    mut cam_q: Query<
        (
            Entity,
            Option<&mut DistanceFog>,
            Option<&mut bevy::light::VolumetricFog>,
        ),
        With<OperatorCamera>,
    >,
    mut clear_color: ResMut<ClearColor>,
    default_clear: Res<DefaultClearColor>,
    mut commands: Commands,
) {
    let sky = crate::sun_moon::vana_sky_from_clock(&vana_clock);

    // Signed hours from the nearer horizon crossing: negative at night,
    // positive during the day. `daylight` ramps 0 (night) → 1 (day) across
    // the horizon band.
    let band = 3.0_f32;
    let horizon_hours = (sky.hour - 6.0).min(18.0 - sky.hour);
    let daylight = ((horizon_hours + band) / (2.0 * band)).clamp(0.0, 1.0);
    let daylight_smooth = daylight * daylight * (3.0 - 2.0 * daylight);

    // kuluu-f1hk: derive the backdrop every frame — BEFORE the no-record early
    // return — so crossing a zone line into a weatherless zone (or turning
    // volumetric fog off) restores the startup default instead of leaking the
    // previous zone's fog horizon. Guarded write to keep change detection quiet
    // when the color is already correct.
    let want_clear = zone_clear_color(
        zone_weather.current.as_ref(),
        settings.volumetric_fog,
        daylight_smooth,
        default_clear.0,
    );
    if clear_color.0 != want_clear {
        clear_color.0 = want_clear;
    }

    let Some(rec) = zone_weather.current else {
        return;
    };

    if let Some(mut fog) = fog_q.iter_mut().next() {
        let [r, g, b, _a] = rec.fog_landscape;
        fog.fog_color = Color::srgb(r, g, b);
        // Tint the in-scattered light with the zone fog palette so the volume
        // reads as the zone's atmosphere rather than a neutral gray wall.
        fog.light_tint = Color::srgb(0.5 + 0.5 * r, 0.5 + 0.5 * g, 0.5 + 0.5 * b);

        // Extinction = density * (absorption + scattering) ≈ density * 0.6, so
        // density = 5/D gives ~5% transmittance (e^-3) at the DAT max fog
        // distance D. The old (15/dist).clamp(0.04, ..) floored at 0.04/unit for
        // any D > 375, crushing visibility to ~100 units regardless of the zone's
        // fog range; the extra twilight boost double-applied time-of-day (the
        // weather keyframes are already sampled per Vana'diel minute).
        let dist = rec.max_fog_dist_landscape.max(50.0);
        fog.density_factor = (5.0 / dist).clamp(0.002, 0.03);
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
        if let Ok((cam_entity, slot, _)) = cam_q.single_mut() {
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
    } else if let Ok((cam_entity, dist_fog, vol_fog)) = cam_q.single_mut() {
        // Volumetric fog owns the atmosphere now: a `DistanceFog` left over
        // from before the toggle would keep crushing view distance on top of
        // the volumetric pass, so strip it (this branch is what makes the menu
        // toggle usable without a zone change).
        if dist_fog.is_some() {
            commands.entity(cam_entity).remove::<DistanceFog>();
        }

        // Derive the volumetric ambient term from the zone fog palette and
        // time of day: at night the DAT fog should read as a dim haze lit by
        // the (already dim) moon, not a hardcoded blue-gray wall.
        if let Some(mut vol) = vol_fog {
            let [fr, fg, fb, _] = rec.fog_landscape;
            vol.ambient_color = Color::srgb(fr, fg, fb);
            vol.ambient_intensity = 0.01 + 0.17 * daylight_smooth;
        }

        // The fog-horizon ClearColor for this branch is written above by the
        // unconditional zone_clear_color pass (kuluu-f1hk).
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: Color = Color::srgb(0.1, 0.2, 0.3);

    fn rec_with_fog(fog_landscape: [f32; 4]) -> WeatherRecord {
        WeatherRecord {
            time_minutes: 0,
            indoors: false,
            sunlight_diffuse_entity: [0.0; 4],
            moonlight_diffuse_entity: [0.0; 4],
            ambient_entity: [0.0; 4],
            fog_entity: [0.0; 4],
            max_fog_dist_entity: 0.0,
            min_fog_dist_entity: 0.0,
            diffuse_mul_entity: 0.0,
            sunlight_diffuse_landscape: [0.0; 4],
            moonlight_diffuse_landscape: [0.0; 4],
            ambient_landscape: [0.0; 4],
            fog_landscape,
            max_fog_dist_landscape: 0.0,
            min_fog_dist_landscape: 0.0,
            diffuse_mul_landscape: 0.0,
            fog_offset: 0.0,
            max_far_clip: 0.0,
            skybox_colors: [[0.0; 4]; 8],
            skybox_altitudes: [0.0; 8],
        }
    }

    fn assert_color_close(got: Color, want: Color) {
        let (g, w) = (got.to_srgba(), want.to_srgba());
        for (a, b) in [(g.red, w.red), (g.green, w.green), (g.blue, w.blue)] {
            assert!((a - b).abs() < 1e-5, "got {g:?}, want {w:?}");
        }
    }

    #[test]
    fn volumetric_daytime_paints_daylight_scaled_fog_horizon() {
        let rec = rec_with_fog([0.5, 0.6, 0.7, 1.0]);
        let got = zone_clear_color(Some(&rec), true, 1.0, DEFAULT);
        // lum = 0.03 + 0.97 * 1.0 = 1.0 -> the raw zone fog color.
        assert_color_close(got, Color::srgb(0.5, 0.6, 0.7));
    }

    #[test]
    fn volumetric_night_dims_the_horizon() {
        let rec = rec_with_fog([0.5, 0.6, 0.7, 1.0]);
        let got = zone_clear_color(Some(&rec), true, 0.0, DEFAULT);
        assert_color_close(got, Color::srgb(0.5 * 0.03, 0.6 * 0.03, 0.7 * 0.03));
    }

    #[test]
    fn foggy_to_weatherless_transition_restores_default() {
        // Foggy zone paints a non-default horizon...
        let rec = rec_with_fog([0.5, 0.6, 0.7, 1.0]);
        let painted = zone_clear_color(Some(&rec), true, 1.0, DEFAULT);
        assert_ne!(painted, DEFAULT);
        // ...then a zone line drops the record: the backdrop must snap back to
        // the startup default rather than leaking the previous zone's color.
        assert_eq!(zone_clear_color(None, true, 1.0, DEFAULT), DEFAULT);
    }

    #[test]
    fn volumetric_toggle_off_restores_default() {
        // DistanceFog mode never owned the backdrop, so even with an active
        // record the default is restored once volumetric fog deactivates.
        let rec = rec_with_fog([0.5, 0.6, 0.7, 1.0]);
        assert_eq!(zone_clear_color(Some(&rec), false, 1.0, DEFAULT), DEFAULT);
    }

    #[test]
    fn default_clear_color_matches_bevy_stock_until_captured() {
        assert_eq!(DefaultClearColor::default().0, ClearColor::default().0);
    }
}
