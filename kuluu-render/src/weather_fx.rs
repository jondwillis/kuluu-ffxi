use std::f32::consts::PI;

use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;

pub use kuluu_snapshot::Weather;

use crate::camera::OperatorCamera;
use crate::graphics_settings::GraphicsSettings;
use crate::snapshot::SceneState;
use crate::sun_moon::IsSun;

#[derive(Resource, Default, Clone, Copy)]
pub struct CurrentWeather(pub Option<Weather>);

pub fn sync_current_weather_from_snapshot(
    state: Res<SceneState>,
    mut current: ResMut<CurrentWeather>,
) {
    // LSB sends the surrounding town's weather in the MH 0x00A
    // (vendor/server/src/map/packets/s2c/0x00a_login.cpp:154); interiors show none.
    let next = if state.snapshot.myroom.is_some() {
        None
    } else {
        state.snapshot.weather
    };
    if next != current.0 {
        current.0 = next;
    }
}

#[derive(Clone, Debug)]
pub struct WeatherModifier {
    pub sun_illuminance_mul: f32,

    pub ambient_brightness_mul: f32,
    pub ambient_tint: Color,

    pub fog: Option<DistanceFog>,

    pub lightning: Option<(f32, f32)>,
}

impl Default for WeatherModifier {
    fn default() -> Self {
        Self {
            sun_illuminance_mul: 1.0,
            ambient_brightness_mul: 1.0,
            ambient_tint: Color::WHITE,
            fog: None,
            lightning: None,
        }
    }
}

pub fn weather_modifier_for(weather: Weather) -> WeatherModifier {
    use Weather::*;

    let cool_grey_fog = |vis: f32| {
        let c = Color::srgba(0.55, 0.60, 0.66, 1.0);
        let inscatter = Color::srgb(0.60, 0.64, 0.70);
        DistanceFog {
            color: c,
            directional_light_color: inscatter,
            directional_light_exponent: 60.0,
            falloff: FogFalloff::from_visibility_colors(vis, c, inscatter),
        }
    };
    let dust_fog = |vis: f32, color: Color| DistanceFog {
        color,
        directional_light_color: color,
        directional_light_exponent: 40.0,
        falloff: FogFalloff::from_visibility_colors(vis, color, color),
    };

    match weather {
        None | Sunshine => WeatherModifier::default(),

        Clouds => WeatherModifier {
            sun_illuminance_mul: 0.7,
            ambient_brightness_mul: 0.85,
            ambient_tint: Color::srgb(0.95, 0.96, 1.0),
            ..default()
        },

        Fog => WeatherModifier {
            sun_illuminance_mul: 0.5,
            ambient_brightness_mul: 0.9,
            ambient_tint: Color::srgb(0.9, 0.92, 0.95),
            fog: Some(cool_grey_fog(120.0)),
            ..default()
        },

        HotSpell => WeatherModifier {
            sun_illuminance_mul: 1.15,
            ambient_brightness_mul: 1.05,
            ambient_tint: Color::srgb(1.05, 0.98, 0.88),
            ..default()
        },
        HeatWave => WeatherModifier {
            sun_illuminance_mul: 1.25,
            ambient_brightness_mul: 1.1,
            ambient_tint: Color::srgb(1.10, 0.96, 0.82),

            fog: Some(dust_fog(1500.0, Color::srgba(0.95, 0.88, 0.74, 1.0))),
            ..default()
        },

        Rain => WeatherModifier {
            sun_illuminance_mul: 0.5,
            ambient_brightness_mul: 0.75,
            ambient_tint: Color::srgb(0.85, 0.88, 0.95),
            fog: Some(cool_grey_fog(400.0)),
            ..default()
        },
        Squall => WeatherModifier {
            sun_illuminance_mul: 0.35,
            ambient_brightness_mul: 0.6,
            ambient_tint: Color::srgb(0.75, 0.80, 0.90),
            fog: Some(cool_grey_fog(220.0)),
            ..default()
        },

        DustStorm => WeatherModifier {
            sun_illuminance_mul: 0.45,
            ambient_brightness_mul: 0.7,
            ambient_tint: Color::srgb(1.10, 0.85, 0.60),
            fog: Some(dust_fog(180.0, Color::srgba(0.78, 0.60, 0.38, 1.0))),
            ..default()
        },
        SandStorm => WeatherModifier {
            sun_illuminance_mul: 0.30,
            ambient_brightness_mul: 0.55,
            ambient_tint: Color::srgb(1.15, 0.80, 0.52),
            fog: Some(dust_fog(90.0, Color::srgba(0.82, 0.56, 0.30, 1.0))),
            ..default()
        },

        Wind => WeatherModifier {
            sun_illuminance_mul: 0.85,
            ambient_brightness_mul: 0.95,
            ambient_tint: Color::srgb(0.96, 0.98, 1.0),
            ..default()
        },
        Gales => WeatherModifier {
            sun_illuminance_mul: 0.7,
            ambient_brightness_mul: 0.85,
            ambient_tint: Color::srgb(0.92, 0.95, 1.0),
            fog: Some(cool_grey_fog(900.0)),
            ..default()
        },

        Snow => WeatherModifier {
            sun_illuminance_mul: 0.7,
            ambient_brightness_mul: 1.05,
            ambient_tint: Color::srgb(0.95, 0.97, 1.05),
            fog: Some(cool_grey_fog(500.0)),
            ..default()
        },
        Blizzards => WeatherModifier {
            sun_illuminance_mul: 0.4,
            ambient_brightness_mul: 0.85,
            ambient_tint: Color::srgb(0.92, 0.95, 1.05),
            fog: Some(cool_grey_fog(180.0)),
            ..default()
        },

        Thunder => WeatherModifier {
            sun_illuminance_mul: 0.4,
            ambient_brightness_mul: 0.55,
            ambient_tint: Color::srgb(0.75, 0.78, 0.92),
            fog: Some(cool_grey_fog(300.0)),
            lightning: Some((5.0, 20.0)),
        },
        Thunderstorms => WeatherModifier {
            sun_illuminance_mul: 0.3,
            ambient_brightness_mul: 0.4,
            ambient_tint: Color::srgb(0.68, 0.72, 0.88),
            fog: Some(cool_grey_fog(160.0)),
            lightning: Some((2.0, 8.0)),
        },

        Auroras => WeatherModifier {
            sun_illuminance_mul: 0.9,
            ambient_brightness_mul: 1.15,
            ambient_tint: Color::srgb(0.80, 1.05, 0.95),
            ..default()
        },
        StellarGlare => WeatherModifier {
            sun_illuminance_mul: 1.1,
            ambient_brightness_mul: 1.20,
            ambient_tint: Color::srgb(1.05, 1.02, 0.92),
            ..default()
        },

        Gloom => WeatherModifier {
            sun_illuminance_mul: 0.4,
            ambient_brightness_mul: 0.55,
            ambient_tint: Color::srgb(0.78, 0.78, 0.82),
            fog: Some(cool_grey_fog(350.0)),
            ..default()
        },
        Darkness => WeatherModifier {
            sun_illuminance_mul: 0.15,
            ambient_brightness_mul: 0.30,
            ambient_tint: Color::srgb(0.55, 0.55, 0.70),
            fog: Some(cool_grey_fog(150.0)),
            ..default()
        },
    }
}

#[derive(Resource, Default, Clone)]
pub struct ActiveWeatherModifier {
    pub modifier: WeatherModifier,
    pub last_weather: Option<Weather>,

    /// Last observed Debug-menu weather gate (`HudPanels::weather_off`), so a
    /// toggle-back-on re-seeds the modifier for the live weather instead of
    /// keeping the forced-neutral one.
    pub gated_last: bool,

    pub base_ambient_color: Color,
    pub base_ambient_brightness: f32,
}

#[derive(Resource, Default)]
pub struct LightningState {
    pub time_to_next: f32,
    pub flash_remaining: f32,

    pub rng: u64,
}

const FLASH_DURATION: f32 = 0.15;
const FLASH_SUN_MUL: f32 = 4.0;
const FLASH_AMBIENT_MUL: f32 = 3.0;

fn lcg_next(state: &mut u64) -> f32 {
    *state = crate::scheduler_runtime::lcg_next(*state);
    ((*state >> 33) as f32) / (u32::MAX as f32)
}

pub fn update_weather_modifier_system(
    current: Res<CurrentWeather>,
    panels: Res<crate::hud::HudPanels>,
    mut active: ResMut<ActiveWeatherModifier>,
    ambient: Res<GlobalAmbientLight>,
    mut lightning: ResMut<LightningState>,
    time: Res<Time>,
) {
    let new_weather = current.0;
    let changed = new_weather != active.last_weather;
    if changed {
        active.last_weather = new_weather;
        active.modifier = weather_modifier_for(new_weather.unwrap_or_default());

        active.base_ambient_color = ambient.color;
        active.base_ambient_brightness = ambient.brightness;

        if let Some((lo, hi)) = active.modifier.lightning {
            if lightning.rng == 0 {
                lightning.rng = 0x9E3779B97F4A7C15;
            }
            let r = lcg_next(&mut lightning.rng);
            lightning.time_to_next = lo + r * (hi - lo);
            lightning.flash_remaining = 0.0;
        } else {
            lightning.time_to_next = 0.0;
            lightning.flash_remaining = 0.0;
        }
    }

    // Debug-menu weather gate: a flip re-seeds the modifier alone (no base-
    // ambient recapture — that value is only meaningful on real weather
    // changes), so toggling Weather back on restores the live weather's
    // effects without waiting for the next server update.
    if panels.weather_off != active.gated_last {
        active.gated_last = panels.weather_off;
        active.modifier = weather_modifier_for(new_weather.unwrap_or_default());
    }
    // While the Weather row is off, every consumer (ambient tint/brightness,
    // sun mul, lightning, weather fog) sees clear-sky values.
    if panels.weather_off {
        active.modifier = WeatherModifier::default();
    }

    if let Some((lo, hi)) = active.modifier.lightning {
        let dt = time.delta_secs();
        if lightning.flash_remaining > 0.0 {
            lightning.flash_remaining = (lightning.flash_remaining - dt).max(0.0);
        } else {
            lightning.time_to_next -= dt;
            if lightning.time_to_next <= 0.0 {
                lightning.flash_remaining = FLASH_DURATION;
                let r = lcg_next(&mut lightning.rng);
                lightning.time_to_next = lo + r * (hi - lo);
            }
        }
    }
}

// DistanceFog/FogFalloff derive no PartialEq upstream; the compare-before-write
// guards need one so an unchanged weather fog stops re-marking the component.
fn fog_falloff_eq(a: &FogFalloff, b: &FogFalloff) -> bool {
    match (a, b) {
        (FogFalloff::Linear { start, end }, FogFalloff::Linear { start: s2, end: e2 }) => {
            start == s2 && end == e2
        }
        (FogFalloff::Exponential { density }, FogFalloff::Exponential { density: d2 }) => {
            density == d2
        }
        (
            FogFalloff::ExponentialSquared { density },
            FogFalloff::ExponentialSquared { density: d2 },
        ) => density == d2,
        (
            FogFalloff::Atmospheric {
                extinction,
                inscattering,
            },
            FogFalloff::Atmospheric {
                extinction: e2,
                inscattering: i2,
            },
        ) => extinction == e2 && inscattering == i2,
        _ => false,
    }
}

fn distance_fog_eq(a: &DistanceFog, b: &DistanceFog) -> bool {
    a.color == b.color
        && a.directional_light_color == b.directional_light_color
        && a.directional_light_exponent == b.directional_light_exponent
        && fog_falloff_eq(&a.falloff, &b.falloff)
}

pub fn apply_weather_to_ambient_and_fog_system(
    active: Res<ActiveWeatherModifier>,
    panels: Res<crate::hud::HudPanels>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut q_cam: Query<Option<&mut DistanceFog>, With<OperatorCamera>>,
    mut commands: Commands,
    cam: Query<Entity, With<OperatorCamera>>,
    settings: Res<GraphicsSettings>,
) {
    let base = active.base_ambient_color.to_linear();
    let tint = active.modifier.ambient_tint.to_linear();
    let want_color = Color::LinearRgba(LinearRgba::new(
        base.red * tint.red,
        base.green * tint.green,
        base.blue * tint.blue,
        1.0,
    ));
    let want_brightness = active.base_ambient_brightness * active.modifier.ambient_brightness_mul;
    if ambient.color != want_color || ambient.brightness != want_brightness {
        ambient.color = want_color;
        ambient.brightness = want_brightness;
    }

    if let Ok(cam_entity) = cam.single() {
        if panels.weather_off {
            // Debug gate: drop any weather fog the previous frame left on the
            // camera. The DAT distance fog is unaffected — when present it is
            // re-written by apply_zone_weather after this system.
            commands.entity(cam_entity).remove::<DistanceFog>();
        } else if let Ok(fog_slot) = q_cam.single_mut() {
            match (active.modifier.fog.clone(), fog_slot) {
                (Some(new_fog), Some(mut existing)) => {
                    if !distance_fog_eq(&existing, &new_fog) {
                        *existing = new_fog;
                    }
                }
                (Some(new_fog), None) => {
                    commands.entity(cam_entity).insert(new_fog);
                }

                (None, Some(_)) if settings.volumetric_fog => {
                    commands.entity(cam_entity).remove::<DistanceFog>();
                }
                (None, _) => {}
            }
        }
    }
}

pub fn apply_weather_to_sun_system(
    active: Res<ActiveWeatherModifier>,
    lightning: Res<LightningState>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut q_sun: Query<&mut DirectionalLight, With<IsSun>>,
    // (base, last written): sun_moon_system caches its writes, so an in-place
    // `*=` here would compound every frame the sun target is unchanged and decay
    // the sun to black under any weather with mul != 1. Track the pre-multiplied
    // base instead: an illuminance that differs from our last write means
    // sun_moon_system re-authored it and becomes the new base.
    mut sun_base: Local<Option<(f32, f32)>>,
) {
    let flash_t = (lightning.flash_remaining / FLASH_DURATION).clamp(0.0, 1.0);
    let flash_curve = (flash_t * PI).sin();
    let sun_mul = active.modifier.sun_illuminance_mul * (1.0 + (FLASH_SUN_MUL - 1.0) * flash_curve);
    let amb_mul = 1.0 + (FLASH_AMBIENT_MUL - 1.0) * flash_curve;

    if let Ok(mut sun) = q_sun.single_mut() {
        let base = match *sun_base {
            Some((base, last)) if sun.illuminance == last => base,
            _ => sun.illuminance,
        };
        let want = base * sun_mul;
        if sun.illuminance != want {
            sun.illuminance = want;
        }
        *sun_base = Some((base, want));
    }
    if amb_mul != 1.0 {
        ambient.brightness *= amb_mul;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_fog_eq_matches_identical_and_rejects_different() {
        let fog = |end: f32| DistanceFog {
            color: Color::srgb(0.5, 0.5, 0.5),
            falloff: FogFalloff::Linear { start: 10.0, end },
            ..default()
        };
        assert!(distance_fog_eq(&fog(100.0), &fog(100.0)));
        assert!(!distance_fog_eq(&fog(100.0), &fog(200.0)));

        let exp = DistanceFog {
            color: Color::srgb(0.5, 0.5, 0.5),
            falloff: FogFalloff::Exponential { density: 0.01 },
            ..default()
        };
        assert!(
            !distance_fog_eq(&fog(100.0), &exp),
            "different falloff variants must compare unequal"
        );
        assert!(distance_fog_eq(&exp, &exp.clone()));
    }

    #[test]
    fn every_weather_variant_has_a_modifier() {
        for w in [
            Weather::None,
            Weather::Sunshine,
            Weather::Clouds,
            Weather::Fog,
            Weather::HotSpell,
            Weather::HeatWave,
            Weather::Rain,
            Weather::Squall,
            Weather::DustStorm,
            Weather::SandStorm,
            Weather::Wind,
            Weather::Gales,
            Weather::Snow,
            Weather::Blizzards,
            Weather::Thunder,
            Weather::Thunderstorms,
            Weather::Auroras,
            Weather::StellarGlare,
            Weather::Gloom,
            Weather::Darkness,
        ] {
            let m = weather_modifier_for(w);
            assert!(m.sun_illuminance_mul.is_finite());
            assert!(m.ambient_brightness_mul.is_finite());
        }
    }

    #[test]
    fn clear_weather_has_no_fog_or_lightning() {
        let m = weather_modifier_for(Weather::Sunshine);
        assert!(m.fog.is_none());
        assert!(m.lightning.is_none());
    }

    #[test]
    fn thunderstorms_flash() {
        let m = weather_modifier_for(Weather::Thunderstorms);
        let (lo, hi) = m.lightning.expect("thunderstorms must flash");
        assert!(lo < hi && lo > 0.0);
    }
}
