//! Per-zone time-of-day weather integration.
//!
//! This module hooks the `ffxi-dat` weather parser (`0x2F` chunks in
//! the zone DAT) into the live scene: when the player zones in, we
//! parse all weather keyframes from the zone DAT into a [`ZoneWeather`]
//! resource, then every frame we sample the keyframes at the current
//! Vana'diel time and apply the interpolated record to the scene's
//! fog volume and ambient light.
//!
//! Skybox dome rendering (the 8-color gradient) is intentionally out
//! of scope here — that requires a custom Bevy material pipeline and
//! is a larger separate piece of work. What we do here gives most of
//! the visible "time of day matters" effect via fog/ambient changes,
//! which is what mid-2000s engines like FFXI's original renderer
//! relied on for atmospheric feel.
//!
//! Native-only (mirrors `dat_mmb`): `ffxi-dat::DatRoot::resolve` does
//! synchronous fs reads of the local install. The browser viewer
//! would need a parallel HTTP-fetched path.

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

/// Loaded weather keyframes for the current zone. Empty when no zone
/// is loaded, when the zone's DAT has no Weather chunks, or when the
/// zone-id → DAT mapping is missing.
#[derive(Resource, Default)]
pub struct ZoneWeather {
    /// Keyframes sorted ascending by `time_minutes`.
    pub records: Vec<WeatherRecord>,
    /// Zone id the records were parsed from. Tracked separately from
    /// `LastAutoLoadedZone` (which lives in `dat_mzb`) so weather
    /// loading can advance even if the MZB scheduler hasn't yet.
    pub zone_id: Option<u16>,
}

/// Bevy plugin: registers the weather resource and its two systems.
pub struct WeatherPlugin;

impl Plugin for WeatherPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneWeather>().add_systems(
            Update,
            (
                load_zone_weather,
                // Run *before* the precipitation override so an active
                // Rain/Squall/Fog modifier still wins the camera's
                // DistanceFog slot — the baseline is just for the
                // None/Sunshine case (and any zone-DAT-keyframed haze).
                apply_zone_weather
                    .before(crate::weather_fx::apply_weather_to_ambient_and_fog_system),
            ),
        );
    }
}

/// Watch zone transitions and reload the weather keyframes. Mirrors
/// the trigger logic in `dat_mzb::auto_load_zone_geometry_system` —
/// fires once per zone change.
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

    // Resolve & read the DAT. Errors are silently swallowed — a zone
    // without Weather chunks just gets an empty `records` Vec and the
    // applier system becomes a no-op. We don't want to spam the chat
    // HUD with weather-load failures.
    let Ok(root) = DatRoot::from_env_or_default() else {
        return;
    };
    let Ok(location) = root.resolve(file_id) else {
        return;
    };
    let path = location.path_under(root.root());
    let Ok(bytes) = fs::read(&path) else { return };
    zone_weather.records = collect_weather_records(&bytes);

    // One-shot diagnostic on zone load: list keyframe times so a
    // "dusk too early" report can be cross-checked against the
    // actual keyframe distribution for the zone. Sparse keyframes
    // (e.g. only 0000/0600/1800/2400) produce long lerp segments
    // that look like "darkness ramps from noon" if the 1800 entry
    // is already dim. Logged only on records.is_empty() flipping to
    // populated — same `current != zone_weather.zone_id` guard
    // above ensures it doesn't repeat per frame.
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

/// Sample the loaded weather keyframes at the current Vana'diel time
/// and apply the interpolated record to scene atmospherics. Currently
/// targets:
///
/// * The first found [`FogVolume`]'s `fog_color`. We use the
///   "landscape" fog pair because most of what's in view is static
///   MMB/MZB geometry; entity fog would be more relevant when the
///   camera is jammed into a crowd of PCs/NPCs, which is rare.
/// * [`AmbientLight`] color + brightness. The `brightness_landscape`
///   field is a scalar multiplier in FFXI's authoring; we
///   normalise around 500 lux (the current hardcoded baseline) so
///   the average curve matches existing ambient feel while
///   night/dawn/dusk visibly dim.
///
/// Sun/moon directional-light color is NOT touched here — the
/// existing `sun_moon_system` already drives those from `vana_sky`
/// and changing two sources at once would make the lerps fight.
/// That integration is a follow-up.
pub fn apply_zone_weather(
    zone_weather: Res<ZoneWeather>,
    mut fog_q: Query<&mut FogVolume>,
    mut ambient: ResMut<AmbientLight>,
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

    // Apply landscape fog color to the first FogVolume in the scene.
    // The MZB renderer assumes there's exactly one (spawned by
    // `scene::setup_world`); if there are more, only the first wins
    // — that's intentional and matches the single-zone-fog model.
    if let Some(mut fog) = fog_q.iter_mut().next() {
        let [r, g, b, _a] = rec.fog_landscape;
        fog.fog_color = Color::srgb(r, g, b);
        // Map the FFXI fog-cylinder distance to a `FogVolume`
        // density factor so every zone shows *some* atmospheric
        // depth even when the server-side weather is `None`.
        //
        // Smaller `max_fog_dist_landscape` ⇒ tighter horizon ⇒
        // denser volumetric haze. The 15.0 numerator was picked so
        // a "normal" outdoor keyframe (~300y max) lands around the
        // scene's spawn-time 0.06 baseline, and heavy-fog
        // keyframes (50–80y) saturate near the ceiling without
        // blowing out the volumetric pass. Floor at 0.04 keeps
        // the cleanest-sky keyframes from going invisible.
        let dist = rec.max_fog_dist_landscape.max(50.0);
        let mut density = (15.0 / dist).clamp(0.04, 0.18);
        // Twilight god-ray boost: thicken the fog within ±3 V-hours
        // of sunrise/sunset so volumetric-light shafts catch through
        // buildings and trees the way real low-sun haze produces
        // crepuscular rays. Falls back to the base weather density
        // through the rest of the day.
        let band = 3.0_f32;
        let dist_from_horizon = (sky.hour - 6.0).min(18.0 - sky.hour).max(0.0);
        let twilight = ((band - dist_from_horizon) / band).clamp(0.0, 1.0);
        let twilight_smooth = twilight * twilight * (3.0 - 2.0 * twilight);
        density = (density * (1.0 + 1.2 * twilight_smooth)).min(0.30);
        fog.density_factor = density;
    }

    // Ambient: keep the hue from the weather record but scale
    // brightness around the existing 500 lux baseline so we don't
    // wildly amplify shadow contrast at noon or black out interiors
    // at midnight. FFXI's `brightness_landscape` ranges roughly 0.5
    // (deep night) to ~1.3 (bright noon).
    let [r, g, b, _a] = rec.ambient_landscape;
    ambient.color = Color::srgb(r.max(0.05), g.max(0.05), b.max(0.05));
    ambient.brightness = 500.0 * rec.brightness_landscape.clamp(0.4, 1.5);

    // Cheap fallback for Low / volumetric-off users: attach a
    // per-pixel `DistanceFog` to the camera tinted from the same
    // keyframe. The raymarched `FogVolume` written above is invisible
    // without a camera-side `VolumetricFog` component (gated by the
    // Low preset), so without this fallback Low-preset users see no
    // atmospheric depth at all.
    //
    // `apply_weather_to_ambient_and_fog_system` (precipitation
    // override) runs after this in the schedule (see `WeatherPlugin`),
    // so an active Rain/Squall/Fog modifier still wins. When the
    // operator toggles back to Medium+, the next-frame volumetric
    // pass renders on top of this — slight double-fog until weather
    // clears the slot or weather precipitation overwrites it. Rare
    // enough to leave for now.
    if !settings.volumetric_fog {
        if let Ok((cam_entity, slot)) = cam_q.single_mut() {
            let [fr, fg, fb, _] = rec.fog_landscape;
            let color = Color::srgb(fr, fg, fb);
            // Slight warm-shift on the in-scatter so the sun-facing
            // hemisphere reads as "lit haze" instead of flat tint.
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
