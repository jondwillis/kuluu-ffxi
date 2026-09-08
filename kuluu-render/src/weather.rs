#[cfg(not(target_arch = "wasm32"))]
use std::fs;

use bevy::light::FogVolume;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use ffxi_dat::mzb::AreaResourceId;
#[cfg(not(target_arch = "wasm32"))]
use ffxi_dat::weather::collect_zone_weather_sets;
use ffxi_dat::weather::{
    sample_weather, weather_type_id_or_default, WeatherRecord, WeatherSetsByType, WeatherTypeId,
    ZoneWeatherSets, WEATHER_TYPE_FALLBACK,
};
#[cfg(not(target_arch = "wasm32"))]
use ffxi_dat::DatRoot;
use kuluu_snapshot::Weather;

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

    /// Area the player stands in, 0 for the zone-wide environment
    /// (`ffxi_dat::mzb::AreaResourceId`).
    pub area: AreaResourceId,

    // `records` for `area`. Identical to `records` while the player is in the
    // zone-wide environment or an area the DAT ships no container for.
    area_records: Vec<WeatherRecord>,
    area_selected: Option<(WeatherTypeId, bool, AreaResourceId)>,

    pub file_id: Option<u32>,

    // research/xim EnvironmentManager.kt:399-451: one interpolated env source per
    // frame; skybox/lighting/sun_moon all read this instead of independently
    // re-sampling (was the skybox/lighting drift).
    pub current: Option<WeatherRecord>,

    /// The record retail draws the *actor's* distance fog from — its own area's,
    /// not the zone's (SkeletalMeshActor.cpp:2749
    /// `FindAreaByFourCCAndGetFog(..., VirtActor88())`). Everything else on this
    /// resource stays zone-wide: the sky dome, the celestial arc and the far
    /// color are set from the zone's weather condition (XiArea.cpp:126-136
    /// `ApplyWeatherCondition` -> `SetFarColor`), not from the block underfoot.
    pub area_current: Option<WeatherRecord>,
}

// Fallback shared by the 0x2F record selection and the celestial/particle consumers
// that resolve a weat/<type>/ subtree: not every zone authors every sky family.
// Retail does exactly one hop, straight to `suny`, when the requested container
// misses — research/XIClient/src/XIClient/source/World/Weather/
// WeatherTransition.cpp:52-54.
pub(crate) fn weather_type_preference(want: WeatherTypeId) -> impl Iterator<Item = WeatherTypeId> {
    std::iter::once(want).chain(std::iter::once(WEATHER_TYPE_FALLBACK))
}

impl ZoneWeather {
    // The weat/<type> subdirectory the zone's environment is actually being read from —
    // `selected` after the same fallback `select_records` applies, so a consumer walking the
    // DAT tree lands in the subtree whose records are live rather than one the zone omits.
    pub fn active_weather_type(&self) -> Option<WeatherTypeId> {
        let want = self.selected?.0;
        Some(
            weather_type_preference(want)
                .find(|id| self.sets.by_type.contains_key(id))
                .unwrap_or(want),
        )
    }

    /// The ambient beds for the live weather, resolved against the zone's own `weat`
    /// container rather than the area container `select_records` uses for the 0x2F records.
    ///
    /// WeatherTransition.cpp:432-479 `FindPrevSound` enumerates the beds from
    /// `this->WeatherFile` — the `weat/<tag>` resource — so the bed is a property of the
    /// zone's weather, not of the block the player stands on. The area containers ship no
    /// beds at all (`ffxi_dat::weather` harvests them only from `weat`), so routing this
    /// through the area map silences the bed inside every modelled interior.
    pub fn ambient_cues(&self) -> &[ffxi_dat::weather::AmbientCue] {
        let Some((want, _)) = self.selected else {
            return &[];
        };
        weather_type_preference(want)
            .find_map(|id| self.sets.by_type.get(&id))
            .or_else(|| self.sets.by_type.values().next())
            .map(|set| set.ambient(self.indoor()))
            .unwrap_or_default()
    }

    /// Always false: no zone-indoor flag is sourced in viewer-core yet, so the
    /// `indoor`-keyed halves of the DAT harvest (`WeatherSet::indoor`,
    /// `WeatherSet::indoor_ambient`) stay unreachable. Retail splits on
    /// `XiZone::zone->PlaceCode` (WeatherTransition.cpp:454-455) — kuluu-dldr.
    pub fn indoor(&self) -> bool {
        self.selected.map(|(_, indoor)| indoor).unwrap_or(false)
    }

    /// The distance fed to `FogFalloff::from_visibility_colors` for the record the player's
    /// own area is fogged through, i.e. the range past which DAT fog leaves no contrast.
    pub fn fog_visibility_dist(&self) -> Option<f32> {
        let rec = self.area_current.or(self.current)?;
        Some(fog_visibility_dist(&rec))
    }
}

// FogFalloff::from_visibility_colors divides by the visibility distance and a DAT record may
// carry 0 (weather.rs harvest default), which would make the extinction non-finite.
const MIN_FOG_VISIBILITY_DIST: f32 = 80.0;

pub fn fog_visibility_dist(rec: &WeatherRecord) -> f32 {
    rec.max_fog_dist_landscape.max(MIN_FOG_VISIBILITY_DIST)
}

// wire::Weather shares the LSB weather.h discriminant ordering, so the variant
// index is the LSB weather id consumed by ffxi_dat::weather::weather_type_id (the
// authoritative weather-id -> weat/<type> subdir table).
fn weather_type_fourcc(weather: Option<Weather>) -> WeatherTypeId {
    weather_type_id_or_default(weather.map(|w| w as u16))
}

// Pick the set for the requested weather type in `area`, falling back across the
// base sky families that actually ship before giving up.
fn select_records(
    sets: &ZoneWeatherSets,
    want: WeatherTypeId,
    indoor: bool,
    area: AreaResourceId,
) -> Vec<WeatherRecord> {
    if !sets.flat.is_empty() {
        return sets.flat.clone();
    }
    let by_type: &WeatherSetsByType = sets.area_by_type(area);
    let pick = |id: &WeatherTypeId| {
        by_type.get(id).map(|set| {
            let chosen = if indoor && !set.indoor.is_empty() {
                &set.indoor
            } else {
                &set.outdoor
            };
            chosen.clone()
        })
    };
    weather_type_preference(want)
        .find_map(|id| pick(&id))
        .or_else(|| by_type.values().next().map(|s| s.outdoor.clone()))
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
        app.add_systems(
            Update,
            (load_zone_weather, resolve_zone_area).before(WeatherSampleSet),
        );

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
        zone_weather.area_records.clear();
        zone_weather.area_selected = None;
        zone_weather.area_current = None;
        return;
    }

    // See ZoneWeather::indoor (kuluu-dldr).
    let indoor = false;
    // The `selected` cache re-runs select_records whenever `want` changes, so the
    // active set reloads on CurrentWeather change as well as zone change.
    let want = weather_type_fourcc(current_weather.0);
    if zone_weather.selected != Some((want, indoor)) {
        zone_weather.records = select_records(&zone_weather.sets, want, indoor, ZONE_WIDE_AREA);
        zone_weather.selected = Some((want, indoor));
    }
    let area = zone_weather.area;
    if zone_weather.area_selected != Some((want, indoor, area)) {
        zone_weather.area_records = select_records(&zone_weather.sets, want, indoor, area);
        zone_weather.area_selected = Some((want, indoor, area));
    }

    if zone_weather.records.is_empty() {
        zone_weather.current = None;
        zone_weather.area_current = None;
        return;
    }
    let sky = crate::sun_moon::vana_sky_from_clock(&vana_clock);
    let time_minutes = (sky.hour * 60.0).rem_euclid(1440.0) as u32;
    zone_weather.current = sample_weather(&zone_weather.records, time_minutes);
    zone_weather.area_current =
        sample_weather(&zone_weather.area_records, time_minutes).or(zone_weather.current);
}

/// `AreaResourceId` 0 — retail's "no area", which routes every environment
/// accessor to the zone's own `XiArea` (XiArea.cpp:377, :434).
pub const ZONE_WIDE_AREA: AreaResourceId = 0;

/// Tracks the area the player is standing in, the way retail tracks it from the
/// ground query each frame (CollidableActor.cpp:218-228). Consumed by the
/// distance fog; see [`ZoneWeather::area_current`].
#[cfg(not(target_arch = "wasm32"))]
pub fn resolve_zone_area(
    mut zone_weather: ResMut<ZoneWeather>,
    area_map: Res<crate::dat_mzb::ZoneAreaMap>,
    self_q: Query<&GlobalTransform, With<crate::components::IsSelf>>,
) {
    let area = self_q
        .single()
        .ok()
        .and_then(|t| area_map.area_at(t.translation()))
        .unwrap_or(ZONE_WIDE_AREA);
    if area == zone_weather.area {
        return;
    }
    let label = crate::dat_mzb::area_id_label(&area);
    if area == ZONE_WIDE_AREA || zone_weather.sets.by_area.contains_key(&area) {
        debug!(area = label, "zone area changed; distance fog follows it");
    } else {
        info!(
            area = label,
            "placements name an area the zone DAT ships no environment for; \
             distance fog stays on the zone-wide records"
        );
    }
    zone_weather.area = area;
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
    zone_weather.area_records.clear();
    zone_weather.area_selected = None;
    zone_weather.area = ZONE_WIDE_AREA;

    let Some(file_id) = current else { return };

    let Ok(root) = DatRoot::from_env_or_default() else {
        return;
    };
    let Ok(location) = root.resolve(file_id) else {
        return;
    };
    let path = location.path_under(&root);
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

// FFXI's DAT fog is distance fog on zone geometry only — the sky dome is never
// fogged. Bevy's raymarch instead clamps each ray by scene depth, and the
// skybox sphere (radius 5500) writes depth beyond the volume, so sky pixels
// used to traverse the volume's full chord and drown in fog ("sky hidden").
// Approximate the client look with a height falloff: full density near the
// ground, exponential decay with altitude, so overhead sky clears while
// eye-level rays toward terrain still accumulate the DAT fog distance.
pub const FOG_VOLUME_CENTER_Y: f32 = 100.0;
pub const FOG_VOLUME_SCALE: Vec3 = Vec3::new(2000.0, 800.0, 2000.0);

/// Builds the 1×64×1 R8 vertical-falloff density texture sampled by the
/// volumetric fog raymarch (multiplies `density_factor` per step, volume-local
/// UVW with `v` up). Shared by the viewer (scene.rs) and the headless example.
pub fn height_fog_density_texture(images: &mut Assets<Image>) -> Handle<Image> {
    use bevy::asset::RenderAssetUsages;
    use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    const N: usize = 64;
    // World-space falloff: full density below Y0, scale height H above it.
    const Y0: f32 = 40.0;
    const H: f32 = 110.0;
    let y_min = FOG_VOLUME_CENTER_Y - FOG_VOLUME_SCALE.y * 0.5;
    let data: Vec<u8> = (0..N)
        .map(|i| {
            let v = (i as f32 + 0.5) / N as f32;
            let y = y_min + v * FOG_VOLUME_SCALE.y;
            let d = if y <= Y0 { 1.0 } else { (-(y - Y0) / H).exp() };
            (d.clamp(0.0, 1.0) * 255.0).round() as u8
        })
        .collect();
    let mut image = Image::new(
        Extent3d {
            width: 1,
            height: N as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D3,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        address_mode_w: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    images.add(image)
}

// kuluu-f1hk: the app's pre-weather backdrop, captured at startup so the
// fog horizon can be restored instead of leaking a stale zone's color once
// no weather record is active.
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
// on the with-record path, so the painted horizon leaked across zone lines
// into weatherless zones).
//
// Retail derives no lighting from the skybox — ambient/sun/fog are
// independently authored channels of the same weather keyframe
// (XiArea.cpp:140-220 UpdateLightingColorsFromWeather vs XiZone.cpp:149-206
// DrawSky). The one sky->scene coupling is this backdrop: outdoors it is the
// interpolated horizon slice SkyPalette[0] (XiZone.cpp:191
// SetBackColor(bsarr[0]); xim EnvironmentManager.kt:139-140), indoors the
// record's own background color, which is all a windowless interior ever
// clears to. Fog deliberately stays a touch darker than the horizon band in
// the authored data, so terrain fading to `fog_landscape` meets a brighter
// sky. With no record (weatherless zone, wasm, mid-zone-line) the startup
// default is restored.
pub(crate) fn zone_clear_color(rec: Option<&WeatherRecord>, default: Color) -> Color {
    match rec {
        Some(rec) => {
            let [r, g, b, _] = if rec.indoors {
                rec.background_color
            } else {
                rec.skybox_colors[0]
            };
            Color::srgb(r, g, b)
        }
        None => default,
    }
}

pub fn apply_zone_weather(
    zone_weather: Res<ZoneWeather>,
    active: Res<crate::weather_fx::ActiveWeatherModifier>,
    mut fog_q: Query<(&mut FogVolume, &mut Transform, Option<&mut Visibility>)>,
    cam_tf_q: Query<&GlobalTransform, With<OperatorCamera>>,
    mut ambient: ResMut<GlobalAmbientLight>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    settings: Res<GraphicsSettings>,
    panels: Res<crate::hud::HudPanels>,
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
    // return — so crossing a zone line into a weatherless zone restores the
    // startup default instead of leaking the previous zone's fog horizon.
    // Guarded write to keep change detection quiet when the color is already
    // correct.
    let want_clear = zone_clear_color(zone_weather.current.as_ref(), default_clear.0);
    if clear_color.0 != want_clear {
        clear_color.0 = want_clear;
    }

    // Debug gate (Debug menu Fog row): with fog off, strip every fog layer so
    // scene-graphic errors can be isolated. Runs even without a weather record —
    // stale DistanceFog/VolumetricFog from the previous zone or weather must not
    // survive the toggle in a weatherless zone either; the weather modifier's own
    // fog is suppressed upstream (apply_weather_to_ambient_and_fog_system).
    if panels.fog_off {
        for (_vol, _tf, vis_slot) in fog_q.iter_mut() {
            if let Some(mut vis) = vis_slot {
                *vis = Visibility::Hidden;
            }
        }
        if let Ok((cam_entity, _, _)) = cam_q.single_mut() {
            commands
                .entity(cam_entity)
                .remove::<DistanceFog>()
                .remove::<bevy::light::VolumetricFog>();
        }
    }

    let Some(rec) = zone_weather.current else {
        return;
    };
    // SkeletalMeshActor.cpp:2749 — the fog an actor is drawn through comes from
    // the actor's own area. Falls back to `rec` when the player is in the
    // zone-wide environment, so a zone with no areas is byte-identical.
    let fog_rec = zone_weather.area_current.unwrap_or(rec);

    if !panels.fog_off {
        if let Some((mut fog, mut fog_tf, vis_slot)) = fog_q.iter_mut().next() {
            // The Fog row may have hidden this volume; restoring visibility is
            // part of re-enabling the layer.
            if let Some(mut vis) = vis_slot {
                *vis = Visibility::Inherited;
            }
            // Keep the camera inside the volume in XZ so the ground haze never
            // ends at a visible box edge; Y stays world-anchored so the height
            // falloff (density texture) tracks true altitude.
            if let Ok(cam_tf) = cam_tf_q.single() {
                let c = cam_tf.translation();
                fog_tf.translation.x = c.x;
                fog_tf.translation.z = c.z;
            }
            let [r, g, b, _a] = fog_rec.fog_landscape;
            fog.fog_color = Color::srgb(r, g, b);
            // Tint the in-scattered light with the zone fog palette so the volume
            // reads as the zone's atmosphere rather than a neutral gray wall.
            fog.light_tint = Color::srgb(0.5 + 0.5 * r, 0.5 + 0.5 * g, 0.5 + 0.5 * b);

            // The volume is a low-density lit ground haze, NOT the DAT distance
            // fog (DistanceFog owns that, below). It cannot be both: bevy's
            // raymarch attenuates directional in-scatter by
            // exp(-density * bounding_radius * (absorption + scattering))
            // (volumetric_fog.wgsl), the same density*sigma product extinction
            // needs, so a volume dense enough to reproduce DAT fog distances
            // (density*sigma*D ~= 3) crushes its own lighting by e^-(3R/D) and
            // renders black instead of fog-colored. Cap density so the light term
            // survives (R ~= 1470 for the 2000x800x2000 volume) and let the haze
            // scale gently with the zone's DAT fog range.
            let dist = fog_rec.max_fog_dist_landscape.max(50.0);
            fog.density_factor = (0.9 / dist).clamp(0.0008, 0.0018);
            // Recover the bounding-radius attenuation (~e^-1.3 at ground density)
            // so the haze reads as lit fog, not soot.
            fog.light_intensity = 3.0;
        }
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

    // The ClearColor backdrop is written above by the unconditional
    // zone_clear_color pass (kuluu-f1hk) and stays on the *zone* record: retail's
    // far color is set by the weather condition (XiArea.cpp:126-136), not by the
    // block the actor stands on, so an interior's black fog must not paint the
    // horizon the player can still see out of the doorway.
    let [fr, fg, fb, _] = fog_rec.fog_landscape;
    let fog_color = Color::srgb(fr, fg, fb);

    // DistanceFog is the authoritative DAT distance fog in BOTH modes. The
    // geometry materials sample it (zone_ffxi.wgsl / skinned_ffxi.wgsl call
    // apply_distance_fog under DISTANCE_FOG); the sky-dome material does not, so
    // like the client, fog swallows terrain but not the sky. The volumetric
    // layer can't take this role — see the density_factor note above — it only
    // adds the lit ground haze on top. Skipped entirely while the Debug menu Fog
    // row is off (the strip block above already removed both layers).
    if !panels.fog_off {
        if let Ok((cam_entity, dist_slot, vol_slot)) = cam_q.single_mut() {
            let inscatter = Color::srgb(
                (fr * 1.08).min(1.0),
                (fg * 1.06).min(1.0),
                (fb * 1.02).min(1.0),
            );
            let visibility = fog_visibility_dist(&fog_rec);
            let want = DistanceFog {
                color: fog_color,
                directional_light_color: inscatter,
                directional_light_exponent: 60.0,
                falloff: FogFalloff::from_visibility_colors(visibility, fog_color, inscatter),
            };
            match dist_slot {
                Some(mut existing) => *existing = want,
                None => {
                    commands.entity(cam_entity).insert(want);
                }
            }

            if settings.volumetric_fog {
                // Ambient term for the raymarch: unlike DistanceFog's inscatter
                // constant, VolumetricFog.ambient_intensity is the only luminance
                // source at night (no sun contribution), so derive it from the
                // day/night curve instead of a fixed value.
                let ambient_intensity = 0.01 + 0.17 * daylight_smooth;
                // Insert/remove of VolumetricFog (and step_count) is owned by
                // graphics::settings::apply_volumetric_fog_system; we only steer the
                // ambient fields on the component it manages. On the toggle frame
                // the insert lands next frame and we pick it up then.
                if let Some(mut vol) = vol_slot {
                    vol.ambient_color = fog_color;
                    vol.ambient_intensity = ambient_intensity;
                }
            }
        }
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
            indoor_light_dir_entity: [0.0; 3],
            ambient_entity: [0.0; 4],
            fog_entity: [0.0; 4],
            max_fog_dist_entity: 0.0,
            min_fog_dist_entity: 0.0,
            diffuse_mul_entity: 0.0,
            sunlight_diffuse_landscape: [0.0; 4],
            moonlight_diffuse_landscape: [0.0; 4],
            indoor_light_dir_landscape: [0.0; 3],
            ambient_landscape: [0.0; 4],
            fog_landscape,
            max_fog_dist_landscape: 0.0,
            min_fog_dist_landscape: 0.0,
            diffuse_mul_landscape: 0.0,
            fog_offset: 0.0,
            max_far_clip: 0.0,
            background_color: [0.0; 4],
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

    // The occlusion reach in kuluu sun_occlusion.rs reads this to decide how far a lens-flare
    // ray may see; it must stay the area-fogged record apply_zone_weather hands FogFalloff.
    #[test]
    fn zone_weather_reports_the_area_records_fog_visibility() {
        let mut zone = ZoneWeather::default();
        assert_eq!(zone.fog_visibility_dist(), None);

        let mut zone_rec = rec_with_fog([0.5; 4]);
        zone_rec.max_fog_dist_landscape = 900.0;
        zone.current = Some(zone_rec);
        assert_eq!(zone.fog_visibility_dist(), Some(900.0));

        let mut area_rec = zone_rec;
        area_rec.max_fog_dist_landscape = 300.0;
        zone.area_current = Some(area_rec);
        assert_eq!(zone.fog_visibility_dist(), Some(300.0));

        area_rec.max_fog_dist_landscape = 0.0;
        zone.area_current = Some(area_rec);
        assert_eq!(zone.fog_visibility_dist(), Some(MIN_FOG_VISIBILITY_DIST));
    }

    #[test]
    fn outdoor_record_paints_the_sky_horizon_not_the_fog_color() {
        // XiZone.cpp:191 — the outdoor backdrop is the interpolated horizon
        // slice, deliberately distinct from the (darker) authored fog color.
        let mut rec = rec_with_fog([0.5, 0.6, 0.7, 1.0]);
        rec.skybox_colors[0] = [0.8, 0.7, 0.6, 1.0];
        let got = zone_clear_color(Some(&rec), DEFAULT);
        assert_color_close(got, Color::srgb(0.8, 0.7, 0.6));
    }

    #[test]
    fn indoor_record_paints_its_own_background_color() {
        // xim EnvironmentManager.kt:139-140 — indoors never sees the dome, so
        // the backdrop is the record's background color (@76), not the sky.
        let mut rec = rec_with_fog([0.5, 0.6, 0.7, 1.0]);
        rec.indoors = true;
        rec.background_color = [0.05, 0.04, 0.06, 1.0];
        rec.skybox_colors[0] = [0.8, 0.7, 0.6, 1.0];
        let got = zone_clear_color(Some(&rec), DEFAULT);
        assert_color_close(got, Color::srgb(0.05, 0.04, 0.06));
    }

    #[test]
    fn foggy_to_weatherless_transition_restores_default() {
        // Foggy zone paints a non-default horizon...
        let mut rec = rec_with_fog([0.5, 0.6, 0.7, 1.0]);
        rec.skybox_colors[0] = [0.8, 0.7, 0.6, 1.0];
        let painted = zone_clear_color(Some(&rec), DEFAULT);
        assert_ne!(painted, DEFAULT);
        // ...then a zone line drops the record: the backdrop must snap back to
        // the startup default rather than leaking the previous zone's color.
        assert_eq!(zone_clear_color(None, DEFAULT), DEFAULT);
    }

    #[test]
    fn default_clear_color_matches_bevy_stock_until_captured() {
        assert_eq!(DefaultClearColor::default().0, ClearColor::default().0);
    }

    fn set_with_fog_r(fog_r: f32) -> ffxi_dat::weather::WeatherSet {
        ffxi_dat::weather::WeatherSet {
            outdoor: vec![rec_with_fog([fog_r, 0.0, 0.0, 1.0])],
            ..Default::default()
        }
    }

    fn zone_and_area_sets() -> ZoneWeatherSets {
        const ZONE_FOG_R: f32 = 0.5;
        const AREA_FOG_R: f32 = 0.1;
        let mut sets = ZoneWeatherSets::default();
        sets.by_type.insert(*b"suny", set_with_fog_r(ZONE_FOG_R));
        sets.by_area.insert(
            ffxi_dat::mzb::area_resource_id_from_dir_name(b"ev01"),
            [(*b"suny", set_with_fog_r(AREA_FOG_R))]
                .into_iter()
                .collect(),
        );
        sets
    }

    // ZoneRenderer.cpp:1133-1152 — a block bound to an area is drawn with *that*
    // area's fog and lights, not the zone's; the interiors that carry one ship a
    // darker, sunless environment.
    #[test]
    fn area_records_replace_the_zone_environment() {
        let sets = zone_and_area_sets();
        let ev01 = ffxi_dat::mzb::area_resource_id_from_dir_name(b"ev01");

        let zone = select_records(&sets, *b"suny", false, ZONE_WIDE_AREA);
        let area = select_records(&sets, *b"suny", false, ev01);
        assert_eq!(zone[0].fog_landscape[0], 0.5);
        assert_eq!(area[0].fog_landscape[0], 0.1);
    }

    // XiArea.cpp:880-892 — FindAreaByFourCC answers the zone when nothing matches,
    // and zones do ship placements naming an area with no container (`ent4`).
    #[test]
    fn area_with_no_container_falls_back_to_the_zone_environment() {
        let sets = zone_and_area_sets();
        let missing = ffxi_dat::mzb::area_resource_id_from_dir_name(b"ent4");
        let got = select_records(&sets, *b"suny", false, missing);
        assert_eq!(got[0].fog_landscape[0], 0.5);
    }

    // WeatherTransition.cpp:432-479 reads the beds off the zone's own `weat` resource, and
    // ffxi_dat harvests them from nowhere else, so walking into a modelled sub-area must not
    // take the bed away with the fog.
    #[test]
    fn the_ambient_bed_survives_walking_into_an_area_container() {
        const BED_SE: u32 = 1005;
        let mut zone_weather = ZoneWeather {
            sets: zone_and_area_sets(),
            ..Default::default()
        };
        zone_weather
            .sets
            .by_type
            .get_mut(b"suny")
            .unwrap()
            .outdoor_ambient
            .push(ffxi_dat::weather::AmbientCue {
                time_minutes: 0,
                se_id: BED_SE,
                loops: true,
            });
        zone_weather.selected = Some((*b"suny", false));

        assert_eq!(zone_weather.ambient_cues()[0].se_id, BED_SE);
        zone_weather.area = ffxi_dat::mzb::area_resource_id_from_dir_name(b"ev01");
        assert_eq!(
            zone_weather.ambient_cues()[0].se_id,
            BED_SE,
            "the area container ships no beds; falling through to it silences the zone"
        );
    }

    // The weather-type ladder stays inside the resolved area: one XiArea owns one
    // WeatherCondition, so a type the area does not author must not silently
    // reach back into the zone's set.
    #[test]
    fn weather_type_fallback_stays_within_the_resolved_area() {
        let mut sets = zone_and_area_sets();
        let ev02 = ffxi_dat::mzb::area_resource_id_from_dir_name(b"ev02");
        sets.by_area.insert(
            ev02,
            [(*b"dark", set_with_fog_r(0.9))].into_iter().collect(),
        );
        let got = select_records(&sets, *b"suny", false, ev02);
        assert_eq!(got[0].fog_landscape[0], 0.9);
    }
}
