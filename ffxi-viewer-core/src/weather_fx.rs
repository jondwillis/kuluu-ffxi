//! Weather visual effects: per-weather atmosphere modulation,
//! procedural rain/snow particle systems, and lightning flashes.
//!
//! ## Scheduling note
//!
//! The sun's `DirectionalLight` color + illuminance is owned by
//! [`crate::sun_moon::sun_moon_system`] (Vana'diel time-of-day).
//! Weather *modulates* — never replaces — that authoritative source.
//!
//! Approach chosen: a [`WeatherModifier`] resource is computed each
//! frame by [`update_weather_modifier_system`] (which also drives the
//! lightning flash), and a separate [`apply_weather_to_sun_system`]
//! runs *after* `sun_moon_system` (via the existing `.chain()` in
//! `ViewerCorePlugin::build`) to multiply the sun's illuminance and
//! tint the ambient light. This keeps `sun_moon.rs` untouched (per
//! file-ownership rules) and avoids resource ordering tricks.
//!
//! The other obvious approach — have `sun_moon_system` read a
//! `WeatherModifier` resource — would require editing `sun_moon.rs`,
//! which this track is forbidden from doing.

use std::f32::consts::PI;

use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;

// NOTE on the `Weather` type source:
//
// The task brief described `ffxi_viewer_wire::Weather` (20 variants
// Canonical weather type lives in `ffxi-viewer-wire` so the wire snapshot,
// the server-decode path (`ffxi-proto`/`ffxi-client`), and this visuals
// module all agree on variant identity.
pub use ffxi_viewer_wire::Weather;

use crate::camera::OperatorCamera;
use crate::snapshot::SceneState;
use crate::sun_moon::IsSun;

/// Current decoded weather. Written by the wire-decode track (track B,
/// `ffxi-client`/`ffxi-proto`) when LSB packet 0x057 lands; consumed
/// here as the only signal that drives the modifier.
///
/// Kept as a `Resource` rather than a direct `SceneState` read so the
/// system graph that fans out (modifier, ambient, sun, particles,
/// lightning) all depend on one cheap `Res<CurrentWeather>` instead of
/// re-fetching the snapshot. [`sync_current_weather_from_snapshot`]
/// keeps it in step with `SceneSnapshot.weather`.
#[derive(Resource, Default, Clone, Copy)]
pub struct CurrentWeather(pub Option<Weather>);

/// Pull the latest weather off the snapshot and into the
/// [`CurrentWeather`] resource. Runs ahead of
/// [`update_weather_modifier_system`] in the schedule.
pub fn sync_current_weather_from_snapshot(
    state: Res<SceneState>,
    mut current: ResMut<CurrentWeather>,
) {
    let next = state.snapshot.weather;
    if next != current.0 {
        current.0 = next;
    }
}

// =====================================================================
// Tunables (a.k.a. the user-shaped slot)
// =====================================================================

/// Per-weather visual parameters. Computed once on weather change and
/// applied each frame.
#[derive(Clone, Debug)]
pub struct WeatherModifier {
    /// Multiplier on the sun's `DirectionalLight::illuminance` (after
    /// `sun_moon_system` writes it). 1.0 = clear sky.
    pub sun_illuminance_mul: f32,
    /// Multiplier and tint applied to the global `AmbientLight`. The
    /// tint is component-wise multiplied with the zone's base ambient
    /// color; brightness is multiplied with the zone's base brightness.
    pub ambient_brightness_mul: f32,
    pub ambient_tint: Color,
    /// Distance fog override (when `Some`, replaces zone fog while the
    /// weather is active).
    pub fog: Option<DistanceFog>,
    /// Active particle effect (None = no precipitation).
    pub particle: Option<ParticleProfile>,
    /// Lightning flash min/max interval in seconds. `None` = no flashes.
    pub lightning: Option<(f32, f32)>,
}

impl Default for WeatherModifier {
    fn default() -> Self {
        Self {
            sun_illuminance_mul: 1.0,
            ambient_brightness_mul: 1.0,
            ambient_tint: Color::WHITE,
            fog: None,
            particle: None,
            lightning: None,
        }
    }
}

/// Look and density of a precipitation particle effect.
#[derive(Clone, Copy, Debug)]
pub struct ParticleProfile {
    pub kind: ParticleKind,
    /// Total live particles. Capped at [`MAX_PARTICLES`] (500).
    pub count: u32,
    pub color: Color,
    /// Fall speed (m/s, negative-Y direction). For rain ~25, snow ~2.
    pub fall_speed: f32,
    /// Lateral wind drift (m/s, +X).
    pub wind: f32,
    /// Particle scale (radius for sphere mesh).
    pub size: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticleKind {
    Rain,
    Snow,
}

/// Hard cap on simultaneously-rendered particles. Above this Bevy
/// starts to grunt on integrated GPUs.
pub const MAX_PARTICLES: u32 = 500;

/// Tuning table — adjust to taste. This is the "feel" of weather in
/// Vana'diel. Keep one match arm per [`Weather`] variant; pre-filled
/// with reasonable defaults that the user can tweak without
/// restructuring the surrounding system.
pub fn weather_modifier_for(weather: Weather) -> WeatherModifier {
    use Weather::*;

    // Common building blocks.
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
            // Slight shimmer haze on the horizon.
            fog: Some(dust_fog(1500.0, Color::srgba(0.95, 0.88, 0.74, 1.0))),
            ..default()
        },

        Rain => WeatherModifier {
            sun_illuminance_mul: 0.5,
            ambient_brightness_mul: 0.75,
            ambient_tint: Color::srgb(0.85, 0.88, 0.95),
            fog: Some(cool_grey_fog(400.0)),
            particle: Some(ParticleProfile {
                kind: ParticleKind::Rain,
                count: 220,
                color: Color::srgba(0.70, 0.78, 0.92, 0.85),
                fall_speed: 28.0,
                wind: 1.5,
                size: 0.04,
            }),
            ..default()
        },
        Squall => WeatherModifier {
            sun_illuminance_mul: 0.35,
            ambient_brightness_mul: 0.6,
            ambient_tint: Color::srgb(0.75, 0.80, 0.90),
            fog: Some(cool_grey_fog(220.0)),
            particle: Some(ParticleProfile {
                kind: ParticleKind::Rain,
                count: 420,
                color: Color::srgba(0.65, 0.72, 0.88, 0.9),
                fall_speed: 36.0,
                wind: 5.0,
                size: 0.05,
            }),
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
            particle: Some(ParticleProfile {
                kind: ParticleKind::Snow,
                count: 200,
                color: Color::srgba(1.0, 1.0, 1.0, 0.9),
                fall_speed: 1.8,
                wind: 0.8,
                size: 0.06,
            }),
            ..default()
        },
        Blizzards => WeatherModifier {
            sun_illuminance_mul: 0.4,
            ambient_brightness_mul: 0.85,
            ambient_tint: Color::srgb(0.92, 0.95, 1.05),
            fog: Some(cool_grey_fog(180.0)),
            particle: Some(ParticleProfile {
                kind: ParticleKind::Snow,
                count: 460,
                color: Color::srgba(1.0, 1.0, 1.0, 0.95),
                fall_speed: 3.5,
                wind: 3.0,
                size: 0.07,
            }),
            ..default()
        },

        Thunder => WeatherModifier {
            sun_illuminance_mul: 0.4,
            ambient_brightness_mul: 0.55,
            ambient_tint: Color::srgb(0.75, 0.78, 0.92),
            fog: Some(cool_grey_fog(300.0)),
            particle: Some(ParticleProfile {
                kind: ParticleKind::Rain,
                count: 280,
                color: Color::srgba(0.65, 0.72, 0.88, 0.9),
                fall_speed: 30.0,
                wind: 2.0,
                size: 0.045,
            }),
            lightning: Some((5.0, 20.0)),
            ..default()
        },
        Thunderstorms => WeatherModifier {
            sun_illuminance_mul: 0.3,
            ambient_brightness_mul: 0.4,
            ambient_tint: Color::srgb(0.68, 0.72, 0.88),
            fog: Some(cool_grey_fog(160.0)),
            particle: Some(ParticleProfile {
                kind: ParticleKind::Rain,
                count: 480,
                color: Color::srgba(0.60, 0.68, 0.86, 0.95),
                fall_speed: 38.0,
                wind: 6.0,
                size: 0.05,
            }),
            lightning: Some((2.0, 8.0)),
            ..default()
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

// =====================================================================
// Resources
// =====================================================================

/// Live weather modulation parameters. Updated when `SceneState.snapshot
/// .weather` changes; consumed each frame by the sun/ambient/fog/particle
/// systems. Defaults to "no modulation".
#[derive(Resource, Default, Clone)]
pub struct ActiveWeatherModifier {
    pub modifier: WeatherModifier,
    pub last_weather: Option<Weather>,
    /// Cached zone-base ambient color & brightness, captured the last
    /// time we saw the zone change. Lets us re-apply weather as a
    /// multiplier on the *unmodulated* base each frame.
    pub base_ambient_color: Color,
    pub base_ambient_brightness: f32,
}

/// Per-particle entity marker + state. We update positions in CPU
/// (cheap at < 500 particles) and rely on the camera transform each
/// frame so the particle volume follows the player.
#[derive(Component)]
pub struct WeatherParticle {
    /// Local offset from the camera at spawn time, so wrapping uses a
    /// box centered on the camera. Mutated each frame.
    pub local: Vec3,
    pub velocity: Vec3,
}

/// Per-camera tag marking the root entity that owns the active set of
/// `WeatherParticle` children. Despawned when weather flips to a kind
/// with no particles.
#[derive(Component)]
pub struct WeatherParticleRoot {
    pub kind: ParticleKind,
}

/// Cached mesh/material handles for particles. Created on demand once
/// per kind and reused.
#[derive(Resource, Default)]
pub struct ParticleAssets {
    pub rain_mesh: Option<Handle<Mesh>>,
    pub snow_mesh: Option<Handle<Mesh>>,
    pub rain_material: Option<Handle<StandardMaterial>>,
    pub snow_material: Option<Handle<StandardMaterial>>,
}

/// Lightning flash state. Counts down to the next strike; while
/// `flash_remaining > 0`, the sun + ambient are pulse-brightened.
#[derive(Resource, Default)]
pub struct LightningState {
    pub time_to_next: f32,
    pub flash_remaining: f32,
    /// Cheap LCG state.
    pub rng: u64,
}

const FLASH_DURATION: f32 = 0.15; // seconds
const FLASH_SUN_MUL: f32 = 4.0;
const FLASH_AMBIENT_MUL: f32 = 3.0;

fn lcg_next(state: &mut u64) -> f32 {
    // Numerical Recipes LCG constants. We only need ~uniform[0,1).
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*state >> 33) as f32) / (u32::MAX as f32)
}

// =====================================================================
// Systems
// =====================================================================

/// Detect weather changes and recompute the active modifier. Also
/// resets the lightning timer to a sensible first-strike delay.
///
/// Runs before [`apply_weather_to_ambient_and_fog_system`] and
/// [`apply_weather_to_sun_system`]. Cheap — only does work when the
/// weather variant flips.
pub fn update_weather_modifier_system(
    current: Res<CurrentWeather>,
    mut active: ResMut<ActiveWeatherModifier>,
    ambient: Res<AmbientLight>,
    mut lightning: ResMut<LightningState>,
    time: Res<Time>,
) {
    let new_weather = current.0;
    let changed = new_weather != active.last_weather;
    if changed {
        active.last_weather = new_weather;
        active.modifier = weather_modifier_for(new_weather.unwrap_or_default());
        // Capture base ambient *as currently configured by the zone
        // atmosphere*. `apply_zone_atmosphere_system` writes raw zone
        // values; we sample here right after, since this system is
        // chained after it.
        active.base_ambient_color = ambient.color;
        active.base_ambient_brightness = ambient.brightness;

        // Initialize lightning timer.
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

    // Drive lightning timer.
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

/// Apply ambient tint/brightness + fog override after zone atmosphere
/// has done its first pass. Runs every frame so a re-application
/// happens after `apply_zone_atmosphere_system` writes fresh zone
/// values (whose changes capture the new base into `active`).
pub fn apply_weather_to_ambient_and_fog_system(
    active: Res<ActiveWeatherModifier>,
    mut ambient: ResMut<AmbientLight>,
    mut q_cam: Query<Option<&mut DistanceFog>, With<OperatorCamera>>,
    mut commands: Commands,
    cam: Query<Entity, With<OperatorCamera>>,
) {
    // Tint ambient color and scale brightness against the captured
    // base. `active.base_*` was sampled right after the zone wrote it,
    // so this multiplication stays stable frame-to-frame.
    let base = active.base_ambient_color.to_linear();
    let tint = active.modifier.ambient_tint.to_linear();
    ambient.color = Color::LinearRgba(LinearRgba::new(
        base.red * tint.red,
        base.green * tint.green,
        base.blue * tint.blue,
        1.0,
    ));
    ambient.brightness = active.base_ambient_brightness * active.modifier.ambient_brightness_mul;

    if let (Ok(fog_slot), Ok(cam_entity)) = (q_cam.single_mut(), cam.single()) {
        if let Some(new_fog) = active.modifier.fog.clone() {
            match fog_slot {
                Some(mut existing) => *existing = new_fog,
                None => {
                    commands.entity(cam_entity).insert(new_fog);
                }
            }
        }
    }
}

/// Apply sun-illuminance multiplier + lightning flash *after*
/// [`crate::sun_moon::sun_moon_system`] has written the time-of-day
/// values. Relies on the `.chain()` ordering in `lib.rs` — must run
/// after `sun_moon_system`.
pub fn apply_weather_to_sun_system(
    active: Res<ActiveWeatherModifier>,
    lightning: Res<LightningState>,
    mut ambient: ResMut<AmbientLight>,
    mut q_sun: Query<&mut DirectionalLight, With<IsSun>>,
) {
    let flash_t = (lightning.flash_remaining / FLASH_DURATION).clamp(0.0, 1.0);
    let flash_curve = (flash_t * PI).sin(); // smooth in/out
    let sun_mul = active.modifier.sun_illuminance_mul * (1.0 + (FLASH_SUN_MUL - 1.0) * flash_curve);
    let amb_mul = 1.0 + (FLASH_AMBIENT_MUL - 1.0) * flash_curve;

    if let Ok(mut sun) = q_sun.single_mut() {
        // sun_moon_system wrote illuminance this frame; multiply.
        sun.illuminance *= sun_mul;
    }
    ambient.brightness *= amb_mul;
}

/// Spawn / despawn particle entities when the active particle profile
/// changes. Particles are children of the camera so they follow the
/// player automatically.
pub fn manage_weather_particles_system(
    active: Res<ActiveWeatherModifier>,
    mut assets: ResMut<ParticleAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    q_cam: Query<Entity, With<OperatorCamera>>,
    q_root: Query<(Entity, &WeatherParticleRoot)>,
    mut commands: Commands,
) {
    let Ok(cam_entity) = q_cam.single() else {
        return;
    };

    // Determine whether the current set of particle roots matches the
    // desired profile. We key on `ParticleKind` — count changes alone
    // don't trigger a respawn (the system early-exits and lets the
    // update system run at current count).
    let desired_kind = active.modifier.particle.map(|p| p.kind);
    let existing_kind = q_root.iter().next().map(|(_, r)| r.kind);

    if desired_kind == existing_kind && desired_kind.is_some() {
        // Already running the right kind; nothing to do here. (Count
        // changes are handled by re-spawning only if profile.kind
        // flips. This is intentional for simplicity.)
        return;
    }

    // Clear any existing particle roots.
    for (e, _) in q_root.iter() {
        commands.entity(e).despawn();
    }

    let Some(profile) = active.modifier.particle else {
        return;
    };

    let count = profile.count.min(MAX_PARTICLES);

    // Lazily build mesh/material handles per kind.
    let (mesh, material) = match profile.kind {
        ParticleKind::Rain => {
            let mesh = assets.rain_mesh.clone().unwrap_or_else(|| {
                let h = meshes.add(Sphere::new(1.0).mesh().ico(1).unwrap());
                assets.rain_mesh = Some(h.clone());
                h
            });
            let material = assets.rain_material.clone().unwrap_or_else(|| {
                let h = mats.add(StandardMaterial {
                    base_color: profile.color,
                    emissive: LinearRgba::new(0.6, 0.7, 0.9, 1.0),
                    unlit: true,
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                });
                assets.rain_material = Some(h.clone());
                h
            });
            (mesh, material)
        }
        ParticleKind::Snow => {
            let mesh = assets.snow_mesh.clone().unwrap_or_else(|| {
                let h = meshes.add(Sphere::new(1.0).mesh().ico(1).unwrap());
                assets.snow_mesh = Some(h.clone());
                h
            });
            let material = assets.snow_material.clone().unwrap_or_else(|| {
                let h = mats.add(StandardMaterial {
                    base_color: profile.color,
                    emissive: LinearRgba::new(1.2, 1.2, 1.4, 1.0),
                    unlit: true,
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                });
                assets.snow_material = Some(h.clone());
                h
            });
            (mesh, material)
        }
    };

    // Spawn a parent under the camera so all particles transform with
    // it. We still drive per-particle position in world-relative
    // local space — but parenting keeps the volume centered on the
    // camera with zero extra math.
    let root = commands
        .spawn((
            WeatherParticleRoot { kind: profile.kind },
            Transform::default(),
            Visibility::default(),
        ))
        .insert(ChildOf(cam_entity))
        .id();

    // Distribute particles in a box around the camera. Rain streaks
    // are stretched vertically by scaling the local transform; snow
    // is uniformly sized.
    let half_x = 18.0;
    let half_z = 18.0;
    let top = 20.0;
    let bottom = -3.0; // a bit below feet so reaching ground is visible

    let mut rng: u64 = 0xCAFEF00DDEADBEEF;
    let stretch_y = match profile.kind {
        ParticleKind::Rain => 8.0,
        ParticleKind::Snow => 1.0,
    };

    for _ in 0..count {
        let x = (lcg_next(&mut rng) - 0.5) * 2.0 * half_x;
        let y = bottom + lcg_next(&mut rng) * (top - bottom);
        let z = (lcg_next(&mut rng) - 0.5) * 2.0 * half_z;
        let scale = Vec3::new(profile.size, profile.size * stretch_y, profile.size);

        commands
            .spawn((
                WeatherParticle {
                    local: Vec3::new(x, y, z),
                    velocity: Vec3::new(profile.wind, -profile.fall_speed, 0.0),
                },
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material.clone()),
                Transform {
                    translation: Vec3::new(x, y, z),
                    scale,
                    ..default()
                },
                Visibility::default(),
                bevy::light::NotShadowCaster,
                bevy::light::NotShadowReceiver,
            ))
            .insert(ChildOf(root));
    }
}

/// Per-frame particle integration. Wraps particles back to the top of
/// the box when they fall past the bottom, and sideways when they
/// blow past the edge. The "box" is in camera-local space (the
/// particle root is parented to the camera), so this is just clamping
/// `local` coordinates against fixed bounds.
pub fn update_weather_particles_system(
    time: Res<Time>,
    mut q_particles: Query<(&mut WeatherParticle, &mut Transform)>,
) {
    let dt = time.delta_secs();
    let half_x = 18.0;
    let half_z = 18.0;
    let top = 20.0;
    let bottom = -3.0;
    for (mut p, mut xf) in q_particles.iter_mut() {
        let v = p.velocity;
        p.local += v * dt;
        if p.local.y < bottom {
            p.local.y = top;
        }
        if p.local.x > half_x {
            p.local.x -= half_x * 2.0;
        } else if p.local.x < -half_x {
            p.local.x += half_x * 2.0;
        }
        if p.local.z > half_z {
            p.local.z -= half_z * 2.0;
        } else if p.local.z < -half_z {
            p.local.z += half_z * 2.0;
        }
        xf.translation = p.local;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_weather_variant_has_a_modifier() {
        // If the wire enum grows, the match arm in
        // `weather_modifier_for` will fail to compile (good). This
        // test pins the structural property that every existing
        // variant produces a non-panicking `WeatherModifier`.
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
    fn clear_weather_has_no_particle_or_fog() {
        let m = weather_modifier_for(Weather::Sunshine);
        assert!(m.particle.is_none());
        assert!(m.fog.is_none());
        assert!(m.lightning.is_none());
    }

    #[test]
    fn thunderstorms_has_lightning_and_heavy_rain() {
        let m = weather_modifier_for(Weather::Thunderstorms);
        let p = m.particle.expect("thunderstorms must have particles");
        assert_eq!(p.kind, ParticleKind::Rain);
        assert!(
            p.count > 300,
            "thunderstorms should be denser than light rain"
        );
        let (lo, hi) = m.lightning.expect("thunderstorms must flash");
        assert!(lo < hi && lo > 0.0);
    }

    #[test]
    fn snow_kinds_use_snow_particles() {
        for w in [Weather::Snow, Weather::Blizzards] {
            let m = weather_modifier_for(w);
            assert_eq!(m.particle.unwrap().kind, ParticleKind::Snow);
        }
        for w in [
            Weather::Rain,
            Weather::Squall,
            Weather::Thunder,
            Weather::Thunderstorms,
        ] {
            let m = weather_modifier_for(w);
            assert_eq!(m.particle.unwrap().kind, ParticleKind::Rain);
        }
    }

    #[test]
    fn particle_count_within_cap() {
        for w in [
            Weather::Rain,
            Weather::Squall,
            Weather::Thunder,
            Weather::Thunderstorms,
            Weather::Snow,
            Weather::Blizzards,
        ] {
            let p = weather_modifier_for(w).particle.unwrap();
            assert!(p.count <= MAX_PARTICLES, "{:?} exceeded cap", w);
        }
    }
}
