//! Camera-side atmospheric distance fog and the per-zone atmosphere
//! seam.
//!
//! Layered design:
//!
//! 1. **Camera-attached fog**: every camera gets a `DistanceFog`
//!    component (see [`ffxi_distance_fog`]) at spawn. This is the
//!    fallback look when no zone data is available.
//!
//! 2. **`ZoneAtmosphere` resource**: describes the look of the *current*
//!    zone — ambient color/brightness, sun direction & color, fog
//!    overrides, and an optional skybox cubemap handle.
//!
//! 3. **`apply_zone_atmosphere_system`**: watches the zone id, looks
//!    the new zone up through a [`ZoneAtmosphereProvider`], and patches
//!    the live `AmbientLight`, `DirectionalLight`, camera `DistanceFog`,
//!    and (when available) the camera `Skybox` component.
//!
//! The DAT side — extracting skybox cubemaps, per-zone ambient/sun
//! colors, and indoor light emitters from FFXI's `.DAT` files — is the
//! work that fills in the provider. The chunk kinds for those records
//! are *not yet identified* in [`ffxi_dat::kind`]; see the TODO there.
//! Until that work lands, the default provider returns hand-tuned
//! outdoor / indoor / cave presets keyed off zone id.

use bevy::core_pipeline::Skybox;
use bevy::light::light_consts;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;

use crate::camera::OperatorCamera;
use crate::snapshot::SceneState;

/// Default outdoor camera fog. Attached to the camera at spawn so the
/// scene never renders un-fogged even before zone data arrives.
///
/// Tunable: edit the three constants below to taste. `from_visibility_colors`
/// solves for the per-channel scattering values that produce the given
/// visibility distance under Koschmieder's contrast law.
pub fn ffxi_distance_fog() -> DistanceFog {
    // Cool blue-gray bulk fog. Slightly desaturated so it reads as
    // "air haze" rather than tinted glass.
    let fog_color = Color::srgba(0.62, 0.68, 0.76, 1.0);
    // Neutral in-scatter (same as bulk). A warm `directional_light_color`
    // produced a uniform yellow "piss filter" because the sun was
    // roughly aligned with the view — the in-scatter cone covered the
    // whole frame. Match bulk color so the sun rim is subtle.
    let sun_inscatter = Color::srgb(0.78, 0.82, 0.88);
    // Very long visibility — barely-there atmospheric haze, only
    // visible on the far horizon. With OLD_SCHOOL bloom carrying the
    // "atmospheric softness" load, the fog can step way back.
    let visibility = 4000.0;
    DistanceFog {
        color: fog_color,
        directional_light_color: sun_inscatter,
        // Higher exponent = much tighter halo around the sun (was 30,
        // which made a 60°-ish warm cone).
        directional_light_exponent: 80.0,
        falloff: FogFalloff::from_visibility_colors(visibility, fog_color, sun_inscatter),
    }
}

/// Per-zone atmospheric look.
///
/// Constructed by a [`ZoneAtmosphereProvider`] on every zone change.
/// Fields are deliberately broad so a future DAT parser can fill more
/// of them in without changing the consumer.
#[derive(Clone)]
pub struct ZoneAtmosphere {
    /// Ambient hemispheric fill color and brightness (lux). For caves
    /// or indoor zones, drop brightness toward 30–80; for open outdoor,
    /// 100–150 reads well next to a sun light at ~10k lux.
    pub ambient_color: Color,
    pub ambient_brightness: f32,
    /// Direction from world origin toward the sun. The directional
    /// light's transform is recomputed to *look from* this direction
    /// toward the origin so cascades stay anchored on the player.
    pub sun_direction: Vec3,
    pub sun_color: Color,
    pub sun_illuminance: f32,
    /// Whole-camera fog. `None` keeps whatever fog the camera was
    /// spawned with (default outdoor).
    pub fog: Option<DistanceFog>,
    /// Cubemap to install as the camera `Skybox`. `None` removes any
    /// existing skybox. Populating this requires DAT-side work to
    /// load FFXI's sky-dome textures as a cubemap `Image`.
    pub skybox: Option<Handle<Image>>,
}

impl ZoneAtmosphere {
    /// Open outdoor zone (San d'Oria gates, Konschtat, etc.). Bright
    /// warm sun, blue sky-bounce ambient, long visibility.
    pub fn outdoor() -> Self {
        Self {
            ambient_color: Color::srgb(0.82, 0.86, 1.00),
            ambient_brightness: 130.0,
            sun_direction: Vec3::new(0.4, 0.85, 0.35).normalize(),
            sun_color: Color::srgb(1.00, 0.96, 0.88),
            sun_illuminance: light_consts::lux::AMBIENT_DAYLIGHT,
            // Fog disabled for now; flip back to `Some(ffxi_distance_fog())`
            // to re-enable per-zone haze.
            fog: None,
            skybox: None,
        }
    }

    /// Indoor zone (Mog House interior, residential building). Cool,
    /// even ambient; weak "sun" simulating diffuse window light; no
    /// distance fog.
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

    /// Cave / dungeon (Maze of Shakhrami, Crawlers' Nest). Very low
    /// ambient with cool blue tint; no real sun.
    pub fn cave() -> Self {
        Self {
            ambient_color: Color::srgb(0.55, 0.60, 0.75),
            ambient_brightness: 35.0,
            sun_direction: Vec3::Y,
            sun_color: Color::srgb(0.7, 0.8, 1.0),
            sun_illuminance: 600.0,
            // Fog disabled for now; caves would benefit from short
            // dark fog when re-enabled.
            fog: None,
            skybox: None,
        }
    }
}

/// Resource that maps `zone_id -> ZoneAtmosphere`. The default
/// implementation is a hand-tuned heuristic over LSB's zone-id ranges;
/// a future DAT-backed implementation will read sky/light chunks from
/// the per-zone DAT directly.
///
/// This is a boxed `Fn` so different builds (native, MCP, headless)
/// can register different providers without leaking a trait through
/// `SceneState`.
#[derive(Resource)]
pub struct ZoneAtmosphereProvider(pub Box<dyn Fn(u16) -> ZoneAtmosphere + Send + Sync>);

impl Default for ZoneAtmosphereProvider {
    fn default() -> Self {
        // Heuristic: LSB zone-id ranges roughly correspond to
        // visual categories. This is a placeholder until per-zone DAT
        // data is decoded.
        Self(Box::new(|zone_id: u16| match zone_id {
            // Mog houses / residential interiors. Multiple ranges in
            // FFXI; this covers the common Phoenix-canonical ones.
            230..=246 => ZoneAtmosphere::indoor(),
            // Caves / underground (Shakhrami, Gusgen, Korroloka, etc.)
            // Approximate id range; refine when MZB sky chunks parse.
            193..=199 | 207..=215 => ZoneAtmosphere::cave(),
            // Everything else: treat as outdoor.
            _ => ZoneAtmosphere::outdoor(),
        }))
    }
}

/// Tracks the zone id we last applied atmosphere for, so we only
/// patch lights/fog on transitions (not every frame).
#[derive(Resource, Default)]
pub struct LastAtmosphereZone {
    pub zone_id: Option<u16>,
}

/// Apply [`ZoneAtmosphere`] to the live world on zone transitions.
///
/// Touches:
///   * `Res<AmbientLight>` — global fill color/brightness.
///   * The unique `DirectionalLight` — color, illuminance, transform.
///   * The `OperatorCamera`'s `DistanceFog` (when the provider supplied one).
///   * The `OperatorCamera`'s `Skybox` slot (added or removed).
///
/// Quietly no-ops when the camera or directional light hasn't been
/// spawned yet — the directional-light startup runs in the same
/// `OnEnter(InGame)` schedule and may not have executed on the first
/// tick.
pub fn apply_zone_atmosphere_system(
    state: Res<SceneState>,
    provider: Res<ZoneAtmosphereProvider>,
    mut last: ResMut<LastAtmosphereZone>,
    mut ambient: ResMut<AmbientLight>,
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
    // Refresh the captured "base" so the weather modifier multiplies
    // against the new zone's ambient instead of the old one. Without
    // this, zoning during rain would scale yesterday's brightness.
    active_weather.base_ambient_color = atmo.ambient_color;
    active_weather.base_ambient_brightness = atmo.ambient_brightness;

    // NOTE: sun light is owned by `crate::sun_moon::sun_moon_system`
    // (Vana'diel time-driven). Per-zone sun fields on `ZoneAtmosphere`
    // are retained for documentation / future indoor-override use.

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
