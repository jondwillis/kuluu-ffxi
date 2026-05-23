//! Sun and moon directional lights driven by Vana'diel time.
//!
//! Two `DirectionalLight` entities — tagged [`IsSun`] and [`IsMoon`] —
//! are spawned at world setup. Each tick, [`sun_moon_system`] reads the
//! current Vana'diel hour and 84-day moon phase and:
//!
//!   * Rotates each light around the world (a "theoretical world
//!     rotation": the lights orbit a fixed scene as Vana'diel time
//!     progresses, simulating planetary rotation).
//!   * Tints and brightens each light by time-of-day curve:
//!     - Sun: cool-blue twilight → gold dawn/dusk → white noon → red
//!       sunset → dark night.
//!     - Moon: dim blue, brightness scaled by lunar phase.
//!   * Disables sun illuminance entirely below the horizon and likewise
//!     for the moon during day.
//!
//! Time matches LSB exactly: 1 Earth-sec = 25 Vana-sec, so 1 V-day ≈
//! 57.6 real minutes. The clock module ([`crate::hud::vana_clock`])
//! owns the epoch and ratio constants. Moon phase math mirrors
//! `vendor/server/src/common/vana_time.h::moon::get_phase` so the
//! client's displayed phase matches `/moon` and weather TOTD events.
//!
//! When a server `GameTime` packet has been received, the
//! [`crate::vana_time::VanaClock`] resource shifts the local Earth
//! clock by the server's offset; otherwise we read system time
//! directly.

use std::f32::consts::PI;
use std::time::SystemTime;

use bevy::prelude::*;

use crate::hud::vana_clock::{EARTH_EPOCH_UNIX, EARTH_SECS_PER_VANA_DAY};

/// Tag the canonical sun directional light. There should be exactly one.
#[derive(Component)]
pub struct IsSun;

/// Tag the moon directional light. There should be exactly one.
#[derive(Component)]
pub struct IsMoon;

/// Tag a visible sun disc (emissive sphere mesh, parented to the
/// camera in world space so it sits at a fixed "sky" radius).
#[derive(Component)]
pub struct SunDisc;

/// Tag a visible moon disc.
#[derive(Component)]
pub struct MoonDisc;

/// Distance from the camera to the celestial discs. Large enough that
/// player-scale parallax is negligible (the discs feel "infinitely
/// far"), small enough to stay within the camera far-clip plane
/// (`spawn_camera` overrides the projection's `far` to 6000m so this
/// has room).
const SKY_RADIUS: f32 = 4000.0;
/// Visible radius of the sun/moon discs. Retail FFXI's moon is much
/// larger than its sun — roughly a 5-10° apparent diameter, dominating
/// the upper sky. The sun is closer to ~2° and read through bloom as a
/// small bright disc.
const SUN_DISC_RADIUS: f32 = 60.0;
const MOON_DISC_RADIUS: f32 = 180.0;

/// FFXI's moon cycle is 84 V-days long. Each of the 12 named phases
/// lasts 7 V-days. LSB's `vana_time.h::moon::get_phase` defines
/// `daysmod = (vana_days_since_epoch + 886*360 + 26) % 84`. The
/// `886*360 + 26` offset bakes down to a constant 38 — the LSB epoch
/// lands 38 days *after* a Full Moon, so `daysmod = (vana_days + 38) % 84`.
const MOON_CYCLE_VANA_DAYS: u64 = 84;
const MOON_PHASE_OFFSET: u64 = (886u64 * 360 + 26) % MOON_CYCLE_VANA_DAYS;

/// Distance from origin at which the lights are positioned. The
/// `DirectionalLight` shader uses only the transform's *forward* axis,
/// but cascaded shadows use the position too — placing the source far
/// out keeps the shadow frustum well-behaved.
const LIGHT_DISTANCE: f32 = 200.0;

/// Resolved time-of-day + lunar state. Recomputed each frame; cheap.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct VanaSky {
    /// Hour of the V-day as a continuous float in `[0.0, 24.0)`.
    pub hour: f32,
    /// Position in the 84-V-day moon cycle as a fraction in `[0.0, 1.0)`.
    /// Matches LSB's `daysmod / 84`: 0.0 = Full, 0.5 = New, → 1.0 = Full.
    pub moon_phase: f32,
    /// Illumination fraction in `[0.0, 1.0]` derived from `moon_phase`
    /// to mirror LSB's `moon::get_phase`. 1.0 = Full, 0.0 = New.
    pub moon_illumination: f32,
    /// True if the moon is currently waxing (illumination increasing).
    pub moon_waxing: bool,
    /// Sun altitude angle in radians. Negative = below horizon.
    pub sun_altitude: f32,
    /// Moon altitude angle in radians. Negative = below horizon.
    pub moon_altitude: f32,
}

/// Sample Vana'diel sky state from system time (no server anchor).
/// Prefer [`vana_sky_from_clock`] in systems where a [`crate::vana_time::VanaClock`]
/// is available.
pub fn vana_sky_now() -> VanaSky {
    let earth_now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(EARTH_EPOCH_UNIX as f64);
    vana_sky_from_unix(earth_now)
}

/// Sample Vana'diel sky state from a server-anchored [`crate::vana_time::VanaClock`].
pub fn vana_sky_from_clock(clock: &crate::vana_time::VanaClock) -> VanaSky {
    vana_sky_from_unix(clock.earth_unix_now())
}

fn vana_sky_from_unix(earth_unix: f64) -> VanaSky {
    // Continuous (sub-second) seconds since the Vana epoch. `max(0.0)`
    // mirrors the old `saturating_sub` for pre-epoch inputs.
    let earth_since = (earth_unix - EARTH_EPOCH_UNIX as f64).max(0.0);
    // 1 Earth-sec = 25 Vana-sec.
    let vana_secs = earth_since * 25.0;
    let day_v_secs = 86400.0; // 24 * 3600 Vana-sec per V-day.
    let secs_into_day = vana_secs.rem_euclid(day_v_secs);
    let hour = (secs_into_day / 3600.0) as f32;

    let total_v_days = (vana_secs / day_v_secs).floor() as u64;
    let daysmod = (total_v_days + MOON_PHASE_OFFSET) % MOON_CYCLE_VANA_DAYS;
    let moon_phase = daysmod as f32 / MOON_CYCLE_VANA_DAYS as f32;
    // LSB formula: 0 = Full, 42 = New, 84 = Full again.
    let (moon_illumination, moon_waxing) = if daysmod < 42 {
        (1.0 - daysmod as f32 / 42.0, false) // waning Full→New
    } else {
        ((daysmod as f32 - 42.0) / 42.0, true) // waxing New→Full
    };

    // Sun is above horizon from hour 6 → 18 (noon at 12). Use a half
    // sine so altitude is 0 at sunrise/sunset and peaks at noon.
    // Below-horizon altitude becomes negative; consumers check sign
    // to gate illuminance.
    let sun_altitude = if (6.0..=18.0).contains(&hour) {
        ((hour - 6.0) / 12.0 * PI).sin() * (PI / 2.0)
    } else {
        // Reflect below horizon for completeness (used for moonlight
        // contrast effects, not for rendering).
        let night_hour = if hour < 6.0 { hour + 24.0 } else { hour };
        -((night_hour - 18.0) / 12.0 * PI).sin() * (PI / 2.0)
    };

    // Moon is the sun's anti-phase: rises at 18:00, sets at 06:00.
    let moon_hour = (hour + 12.0) % 24.0;
    let moon_altitude = if (6.0..=18.0).contains(&moon_hour) {
        ((moon_hour - 6.0) / 12.0 * PI).sin() * (PI / 2.0)
    } else {
        -1.0
    };

    VanaSky {
        hour,
        moon_phase,
        moon_illumination,
        moon_waxing,
        sun_altitude,
        moon_altitude,
    }
}

/// Handles for sun/moon disc materials. Cached so `sun_moon_system`
/// can recolor them each frame without re-allocating.
#[derive(Resource)]
pub struct CelestialMaterials {
    pub sun: Handle<StandardMaterial>,
    pub moon: Handle<crate::moon_material::MoonMaterial>,
}

/// Spawn the sun and moon directional lights *and* their visible
/// discs. Call from `setup_world`.
///
/// The cascade config is derived from the current
/// [`GraphicsSettings`](crate::graphics_settings::GraphicsSettings) so
/// users with a persisted non-default preset don't see a one-frame
/// flicker as the reactor systems re-snap the cascades.
pub fn spawn_sun_and_moon(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    moon_materials: &mut Assets<crate::moon_material::MoonMaterial>,
    settings: &crate::graphics_settings::GraphicsSettings,
) {
    use crate::graphics_settings::cascade_config_from_settings;
    commands.spawn((
        crate::components::InGameEntity,
        IsSun,
        DirectionalLight {
            illuminance: 0.0, // Real value set on first tick by sun_moon_system.
            shadows_enabled: true,
            shadow_depth_bias: 0.2,
            shadow_normal_bias: 1.0,
            ..default()
        },
        cascade_config_from_settings(settings),
        bevy::light::VolumetricLight,
        Transform::from_xyz(0.0, LIGHT_DISTANCE, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        crate::components::InGameEntity,
        IsMoon,
        DirectionalLight {
            illuminance: 0.0,
            // Moon shadows are subtle and expensive; off by default.
            // Flip to true if you want shadow-casting moonlight.
            shadows_enabled: false,
            shadow_depth_bias: 0.2,
            shadow_normal_bias: 1.0,
            ..default()
        },
        bevy::light::VolumetricLight,
        Transform::from_xyz(0.0, -LIGHT_DISTANCE, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Visible discs. Bevy 0.17's `pbr.wgsl` unlit branch (line 82-86)
    // returns `base_color` directly and never reads `emissive` — so HDR
    // colors must live in `base_color` for the bloom halo to fire.
    // `unlit: true` keeps the disc self-luminous (no shading on top).
    // Real per-frame colors are written by `sun_moon_system`.
    // Sun: unlit emissive sphere (bloom halo carries the rest).
    let sphere = meshes.add(Sphere::new(1.0).mesh().ico(3).unwrap());
    let sun_mat = materials.add(StandardMaterial {
        base_color: Color::linear_rgb(20.0, 18.0, 10.0),
        unlit: true,
        ..default()
    });
    // Moon: flat unit-quad billboard with our custom phase-shading
    // material. `Rectangle::new(1.0, 1.0)` is centered at origin in
    // its local XY plane; `sun_moon_system` rotates the quad to face
    // the camera each frame.
    let moon_quad = meshes.add(Rectangle::new(1.0, 1.0));
    let moon_mat = moon_materials.add(crate::moon_material::MoonMaterial::default());
    // `NotShadowCaster`: critical — without it the sun disc, being a
    // huge sphere positioned between the directional light source and
    // the world, would cast a sky-spanning shadow on the entire
    // ground. `NotShadowReceiver`: the discs are unlit emissive
    // anyway; skip the shadow sampling work.
    use bevy::light::{NotShadowCaster, NotShadowReceiver};
    commands.spawn((
        crate::components::InGameEntity,
        SunDisc,
        Mesh3d(sphere),
        MeshMaterial3d(sun_mat.clone()),
        Transform::from_scale(Vec3::splat(SUN_DISC_RADIUS)),
        Visibility::Hidden,
        NotShadowCaster,
        NotShadowReceiver,
    ));
    commands.spawn((
        crate::components::InGameEntity,
        MoonDisc,
        Mesh3d(moon_quad),
        MeshMaterial3d(moon_mat.clone()),
        Transform::from_scale(Vec3::splat(MOON_DISC_RADIUS)),
        Visibility::Hidden,
        NotShadowCaster,
        NotShadowReceiver,
    ));
    commands.insert_resource(CelestialMaterials {
        sun: sun_mat,
        moon: moon_mat,
    });
}

/// Map Vana'diel hour → sun color + illuminance. This is the curve
/// that defines the look of the day. Hand-tune to taste — the
/// 5–10 line spot where the lighting *feel* lives.
///
/// Returns `(color, illuminance_lux)`. Below horizon returns 0 illuminance.
pub fn sun_color_for_hour(hour: f32, sun_altitude: f32) -> (Color, f32) {
    if sun_altitude <= 0.0 {
        return (Color::BLACK, 0.0);
    }
    // Normalized "elevation" 0..1: 0 at rise/set, 1 at noon.
    let elev = (sun_altitude / (PI / 2.0)).clamp(0.0, 1.0);

    // Dawn warmth window (06–08) and dusk warmth window (16–18). Mid
    // day is white. Curve picks "warmth" by distance from noon.
    let warm = (1.0 - elev).powf(2.0); // 1 at horizon, 0 at noon.
    let near_dusk = hour > 12.0; // bias to red sunset, not gold dawn
    let (r, g, b) = if near_dusk {
        // Dusk: ramps toward deep red/orange.
        (1.0, 1.0 - 0.55 * warm, 1.0 - 0.85 * warm)
    } else {
        // Dawn: gentler, more gold than red.
        (1.0, 1.0 - 0.35 * warm, 1.0 - 0.65 * warm)
    };
    // Illuminance peaks at noon (~10k lux), drops to ~1.5k at horizon.
    let lux = 1500.0 + 8500.0 * elev;
    (Color::srgb(r, g, b), lux)
}

/// Map moon illumination + altitude → moon color + illuminance.
///
/// `illumination` is the LSB-style fraction in `[0, 1]` — 1 = Full,
/// 0 = New. Cool blue tint.
pub fn moon_color_for_phase(illumination: f32, moon_altitude: f32) -> (Color, f32) {
    if moon_altitude <= 0.0 {
        return (Color::BLACK, 0.0);
    }
    let visibility = illumination.clamp(0.0, 1.0);
    let elev = (moon_altitude / (PI / 2.0)).clamp(0.0, 1.0);
    // Stylized brightness — real moonlight is < 1 lux, but the scene's
    // ambient + tonemapping curves require a kick to read on screen.
    // 200 lux barely registered; bump to 1500 lux at full + zenith so
    // night scenes actually have a visible blue cast and PCs catch
    // moon highlights.
    let lux = 1500.0 * visibility * (0.3 + 0.7 * elev);
    (Color::srgb(0.62, 0.72, 1.00), lux)
}

/// Each-frame system: read Vana sky, update sun + moon transforms,
/// colors, illuminance, and visible disc positions/emissives.
/// 8-bucket moon-phase names. Indexed by `(phase * 8.0).floor() % 8`
/// where `phase` is the LSB-aligned `[0.0, 1.0)` cycle position on
/// `VanaSky` (0.0 = Full, 0.5 = New, → 1.0 = Full). The cycle is
/// Full → Waning → New → Waxing → Full to match LSB's
/// `moon::get_direction` semantics.
const MOON_PHASE_NAMES: [&str; 8] = [
    "Full",
    "Waning Gibbous",
    "Last Quarter",
    "Waning Crescent",
    "New",
    "Waxing Crescent",
    "First Quarter",
    "Waxing Gibbous",
];

pub fn sun_moon_system(
    mut sky: ResMut<VanaSky>,
    mut q_sun: Query<
        (&mut DirectionalLight, &mut Transform),
        (
            With<IsSun>,
            Without<IsMoon>,
            Without<SunDisc>,
            Without<MoonDisc>,
            Without<crate::camera::OperatorCamera>,
        ),
    >,
    mut q_moon: Query<
        (&mut DirectionalLight, &mut Transform),
        (
            With<IsMoon>,
            Without<IsSun>,
            Without<SunDisc>,
            Without<MoonDisc>,
            Without<crate::camera::OperatorCamera>,
        ),
    >,
    mut q_sun_disc: Query<
        (&mut Transform, &mut Visibility),
        (
            With<SunDisc>,
            Without<MoonDisc>,
            Without<IsSun>,
            Without<IsMoon>,
            Without<crate::camera::OperatorCamera>,
        ),
    >,
    mut q_moon_disc: Query<
        (&mut Transform, &mut Visibility),
        (
            With<MoonDisc>,
            Without<SunDisc>,
            Without<IsSun>,
            Without<IsMoon>,
            Without<crate::camera::OperatorCamera>,
        ),
    >,
    q_cam: Query<&Transform, With<crate::camera::OperatorCamera>>,
    materials_handle: Option<Res<CelestialMaterials>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut moon_materials: ResMut<Assets<crate::moon_material::MoonMaterial>>,
    mut scene_state: ResMut<crate::snapshot::SceneState>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    mut prev_sun_up: Local<Option<bool>>,
    mut prev_moon_up: Local<Option<bool>>,
    mut prev_phase_bucket: Local<Option<u8>>,
) {
    *sky = vana_sky_from_clock(&vana_clock);

    // Sun/moon altitude zero-crossings → System chat. Edge-triggered
    // so we get one line per rise/set, not one per frame while above
    // the horizon. First frame (`prev_*_up = None`) seeds the state
    // without firing, otherwise login at noon would fire a fake
    // "sunrise" because we'd treat the absent prev as "below."
    let sun_up_now = sky.sun_altitude > 0.0;
    if let Some(prev) = *prev_sun_up {
        if prev != sun_up_now {
            scene_state.push_local_toast(crate::snapshot::system_chat_line(
                if sun_up_now {
                    "☀ Sunrise"
                } else {
                    "☀ Sunset"
                }
                .to_string(),
            ));
        }
    }
    *prev_sun_up = Some(sun_up_now);

    let moon_up_now = sky.moon_altitude > 0.0;
    if let Some(prev) = *prev_moon_up {
        if prev != moon_up_now {
            scene_state.push_local_toast(crate::snapshot::system_chat_line(
                if moon_up_now {
                    "☾ Moonrise"
                } else {
                    "☾ Moonset"
                }
                .to_string(),
            ));
        }
    }
    *prev_moon_up = Some(moon_up_now);

    // Moon phase bucket — eight 12.5%-wide windows. The illumination
    // percent matches LSB's `moon::get_phase` so the line matches what
    // retail's lunar HUD shows the player.
    let phase_bucket = ((sky.moon_phase * 8.0).floor() as i32).rem_euclid(8) as u8;
    if let Some(prev) = *prev_phase_bucket {
        if prev != phase_bucket {
            scene_state.push_local_toast(crate::snapshot::system_chat_line(format!(
                "☾ Moon: {} ({:.0}% illuminated)",
                MOON_PHASE_NAMES[phase_bucket as usize],
                sky.moon_illumination * 100.0,
            )));
        }
    }
    *prev_phase_bucket = Some(phase_bucket);

    // Sun arcs east → up → west. We model the "world rotation" by
    // rotating the light source around the world Z axis (east-west)
    // with the sun at noon directly above (+Y) and at midnight
    // directly below (-Y).
    //
    // angle = 0 at midnight (sun directly below). 1 full revolution
    // per V-day. At hour=6 angle=π/2 (sun on +X horizon, rising).
    let sun_angle = (sky.hour / 24.0) * 2.0 * PI - PI / 2.0;
    let sun_dir = Vec3::new(sun_angle.cos(), sun_angle.sin(), 0.25).normalize();
    let sun_pos = sun_dir * LIGHT_DISTANCE;
    let (sun_color, sun_lux) = sun_color_for_hour(sky.hour, sky.sun_altitude);

    if let Ok((mut light, mut xf)) = q_sun.single_mut() {
        light.color = sun_color;
        light.illuminance = sun_lux;
        *xf = Transform::from_translation(sun_pos).looking_at(Vec3::ZERO, Vec3::Y);
    }

    let moon_angle = sun_angle + PI;
    let moon_dir = Vec3::new(moon_angle.cos(), moon_angle.sin(), 0.25).normalize();
    let moon_pos = moon_dir * LIGHT_DISTANCE;
    let (moon_color, moon_lux) =
        moon_color_for_phase(sky.moon_illumination, sky.moon_altitude);
    if let Ok((mut light, mut xf)) = q_moon.single_mut() {
        light.color = moon_color;
        light.illuminance = moon_lux;
        *xf = Transform::from_translation(moon_pos).looking_at(Vec3::ZERO, Vec3::Y);
    }

    // Visible discs ride the camera so they read as "infinitely far".
    let cam_pos = q_cam.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);

    // A small below-horizon margin so the sun fades through the horizon
    // line instead of popping. The emissive curve already dims it down
    // to -0.2 rad below.
    let sun_visible = sky.sun_altitude > -0.05;
    if let Ok((mut disc, mut vis)) = q_sun_disc.single_mut() {
        disc.translation = cam_pos + sun_dir * SKY_RADIUS;
        disc.scale = Vec3::splat(SUN_DISC_RADIUS);
        *vis = if sun_visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    // Moon: hide when below horizon *or* when illumination is ~0
    // (new moon is invisible by definition).
    let moon_visible = sky.moon_altitude > 0.0 && sky.moon_illumination > 0.02;
    if let Ok((mut disc, mut vis)) = q_moon_disc.single_mut() {
        let moon_world = cam_pos + moon_dir * SKY_RADIUS;
        disc.translation = moon_world;
        disc.scale = Vec3::splat(MOON_DISC_RADIUS);
        // Billboard: face the camera. The Rectangle mesh lives in
        // its local XY plane with +Z as its normal — so aim +Z at
        // the camera. Using `Vec3::Y` as up keeps the disc upright;
        // the phase-mask shader's "horizontal terminator" then maps
        // intuitively to waxing/waning.
        disc.look_at(cam_pos, Vec3::Y);
        *vis = if moon_visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }

    // Recolor base_color (NOT emissive — unlit ignores emissive). Sun
    // brightness scales with daylight (dawn/dusk red, noon blinding
    // white). Moon brightness scales by phase visibility — new moon
    // fades to nearly invisible (also gated by Visibility::Hidden
    // above for the hard cutoff).
    if let Some(handles) = materials_handle.as_deref() {
        if let Some(sun_mat) = materials.get_mut(&handles.sun) {
            let visible = sky.sun_altitude.max(-0.2);
            // Below horizon: dim the disc but don't fully kill it (a
            // faint glow on the horizon at twilight reads as "sun just
            // set").
            let intensity = if visible > 0.0 {
                8.0 + 14.0 * (visible / (PI / 2.0))
            } else {
                (1.0 + 5.0 * (visible + 0.2) / 0.2).max(0.0)
            };
            let c = sun_color.to_linear();
            sun_mat.base_color = Color::linear_rgb(
                c.red * intensity,
                c.green * intensity * 0.95,
                c.blue * intensity * 0.75,
            );
        }
        if let Some(moon_mat) = moon_materials.get_mut(&handles.moon) {
            let visibility = sky.moon_illumination.clamp(0.0, 1.0);
            // Slight floor so an above-horizon crescent still reads as
            // a faint disc rather than going completely black between
            // earthshine and the (small) lit fraction.
            let intensity = if sky.moon_altitude > 0.0 {
                0.6 + 1.4 * visibility
            } else {
                0.0
            };
            moon_mat.data.params = Vec4::new(
                sky.moon_illumination,
                if sky.moon_waxing { 1.0 } else { -1.0 },
                intensity,
                0.0,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noon_sun_is_overhead() {
        // 12 V-hours = 12 * 144 = 1728 Earth-seconds after epoch.
        let sky = vana_sky_from_unix((EARTH_EPOCH_UNIX + 12 * 144) as f64);
        assert!((sky.hour - 12.0).abs() < 0.01);
        assert!(sky.sun_altitude > 1.5); // ≈ π/2 = 1.5708
    }

    #[test]
    fn moon_phase_matches_lsb_formula() {
        // LSB: daysmod = (vana_days + 886*360 + 26) % 84
        // At V-day 4 (after epoch), daysmod = (4 + 38) % 84 = 42 → New Moon (0%).
        let v_day = EARTH_SECS_PER_VANA_DAY;
        let sky = vana_sky_from_unix((EARTH_EPOCH_UNIX + 4 * v_day) as f64);
        assert!(
            sky.moon_illumination < 0.05,
            "expected new moon at V-day 4, got illumination {}",
            sky.moon_illumination
        );
        // V-day 46 = daysmod (46+38) % 84 = 0 → Full Moon (100%).
        let sky = vana_sky_from_unix((EARTH_EPOCH_UNIX + 46 * v_day) as f64);
        assert!(
            sky.moon_illumination > 0.95,
            "expected full moon at V-day 46, got illumination {}",
            sky.moon_illumination
        );
    }

    #[test]
    fn midnight_sun_is_below() {
        let sky = vana_sky_from_unix(EARTH_EPOCH_UNIX as f64); // hour 0
        assert!(sky.sun_altitude < 0.0);
        // And moon is up.
        assert!(sky.moon_altitude > 0.0);
    }

    #[test]
    fn moon_phase_cycles_every_84_v_days() {
        let one_v_day = EARTH_SECS_PER_VANA_DAY;
        let s0 = vana_sky_from_unix(EARTH_EPOCH_UNIX as f64);
        let s84 = vana_sky_from_unix((EARTH_EPOCH_UNIX + 84 * one_v_day) as f64);
        assert!((s0.moon_phase - s84.moon_phase).abs() < 1e-4);
    }

    #[test]
    fn hour_advances_smoothly_within_a_second() {
        // Two samples 100ms apart must produce *different* hours —
        // regression for the old `as_secs()` quantization where the
        // hour ticked once per real second.
        let base = EARTH_EPOCH_UNIX as f64 + 6.0; // mid-morning, sun up.
        let a = vana_sky_from_unix(base);
        let b = vana_sky_from_unix(base + 0.1);
        assert!(a.hour != b.hour, "hour did not advance sub-second");
    }
}
