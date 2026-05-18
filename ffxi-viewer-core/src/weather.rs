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
use bevy::prelude::*;
use ffxi_dat::weather::{collect_weather_records, sample_weather, WeatherRecord};
use ffxi_dat::DatRoot;

use crate::snapshot::SceneState;
use crate::sun_moon::vana_sky_now;

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
        app.init_resource::<ZoneWeather>()
            .add_systems(Update, (load_zone_weather, apply_zone_weather));
    }
}

/// Watch zone transitions and reload the weather keyframes. Mirrors
/// the trigger logic in `dat_mzb::auto_load_zone_geometry_system` —
/// fires once per zone change.
pub fn load_zone_weather(
    scene_state: Res<SceneState>,
    mut zone_weather: ResMut<ZoneWeather>,
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
) {
    if zone_weather.records.is_empty() {
        return;
    }
    let sky = vana_sky_now();
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
    }

    // Ambient: keep the hue from the weather record but scale
    // brightness around the existing 500 lux baseline so we don't
    // wildly amplify shadow contrast at noon or black out interiors
    // at midnight. FFXI's `brightness_landscape` ranges roughly 0.5
    // (deep night) to ~1.3 (bright noon).
    let [r, g, b, _a] = rec.ambient_landscape;
    ambient.color = Color::srgb(r.max(0.05), g.max(0.05), b.max(0.05));
    ambient.brightness = 500.0 * rec.brightness_landscape.clamp(0.4, 1.5);
}
