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
//! All math is closed-form against system time — there is no server
//! component. The clock module ([`crate::hud::vana_clock`]) is the
//! shared source of truth for the epoch.
//!
//! One Vana'diel day ≈ 10 real minutes (25 real seconds per V-hour ×
//! 24), so the sun visibly traverses the sky during normal play.

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
/// Visible radius of the sun/moon discs. Sized for ~1.5° apparent
/// diameter at SKY_RADIUS=4000m, so a ~50m sphere reads as "real
/// celestial body" with bloom halo. Scale up to make them larger.
const SUN_DISC_RADIUS: f32 = 60.0;
const MOON_DISC_RADIUS: f32 = 50.0;

/// FFXI's moon cycle is 84 V-days long. Phase 0 (New) starts at the
/// V-epoch (8866-01-01); each of the 12 named phases lasts 7 V-days.
const MOON_CYCLE_VANA_DAYS: u64 = 84;

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
    /// Lunar phase fraction in `[0.0, 1.0)`. 0 = New, 0.5 = Full.
    pub moon_phase: f32,
    /// Sun altitude angle in radians. Negative = below horizon.
    pub sun_altitude: f32,
    /// Moon altitude angle in radians. Negative = below horizon.
    pub moon_altitude: f32,
}

/// Sample Vana'diel sky state from current system time.
pub fn vana_sky_now() -> VanaSky {
    let earth_now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(EARTH_EPOCH_UNIX);
    vana_sky_from_unix(earth_now)
}

fn vana_sky_from_unix(earth_unix: u64) -> VanaSky {
    let since = earth_unix.saturating_sub(EARTH_EPOCH_UNIX);
    let secs_into_day = (since % EARTH_SECS_PER_VANA_DAY) as f32;
    let hour = secs_into_day / 25.0; // 25 earth seconds per V-hour.

    let total_v_days = since / EARTH_SECS_PER_VANA_DAY;
    let moon_phase = (total_v_days % MOON_CYCLE_VANA_DAYS) as f32
        / MOON_CYCLE_VANA_DAYS as f32;

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
        sun_altitude,
        moon_altitude,
    }
}

/// Handles for sun/moon disc materials. Cached so `sun_moon_system`
/// can recolor them each frame without re-allocating.
#[derive(Resource)]
pub struct CelestialMaterials {
    pub sun: Handle<StandardMaterial>,
    pub moon: Handle<StandardMaterial>,
}

/// Spawn the sun and moon directional lights *and* their visible
/// discs. Call from `setup_world`.
pub fn spawn_sun_and_moon(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    use crate::scene::cascade_config_for_sun;
    commands.spawn((
        IsSun,
        DirectionalLight {
            illuminance: 0.0, // Real value set on first tick by sun_moon_system.
            shadows_enabled: true,
            shadow_depth_bias: 0.2,
            shadow_normal_bias: 1.0,
            ..default()
        },
        cascade_config_for_sun(),
        bevy::light::VolumetricLight,
        Transform::from_xyz(0.0, LIGHT_DISTANCE, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        IsMoon,
        DirectionalLight {
            illuminance: 0.0,
            // Moon shadows are subtle and expensive; off by default.
            // Flip to true if you want shadow-casting moonlight.
            shadows_enabled: false,
            ..default()
        },
        bevy::light::VolumetricLight,
        Transform::from_xyz(0.0, -LIGHT_DISTANCE, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Visible discs. Emissive `StandardMaterial` with high HDR
    // emissive intensity so the OLD_SCHOOL bloom halos them. Real
    // emissive values are written each frame by `sun_moon_system`.
    let sphere = meshes.add(Sphere::new(1.0).mesh().ico(3).unwrap());
    let sun_mat = materials.add(StandardMaterial {
        base_color: Color::BLACK,
        emissive: LinearRgba::new(20.0, 18.0, 10.0, 1.0),
        unlit: true,
        ..default()
    });
    let moon_mat = materials.add(StandardMaterial {
        base_color: Color::BLACK,
        emissive: LinearRgba::new(2.0, 2.4, 4.0, 1.0),
        unlit: true,
        ..default()
    });
    // `NotShadowCaster`: critical — without it the sun disc, being a
    // huge sphere positioned between the directional light source and
    // the world, would cast a sky-spanning shadow on the entire
    // ground. `NotShadowReceiver`: the discs are unlit emissive
    // anyway; skip the shadow sampling work.
    use bevy::light::{NotShadowCaster, NotShadowReceiver};
    commands.spawn((
        SunDisc,
        Mesh3d(sphere.clone()),
        MeshMaterial3d(sun_mat.clone()),
        Transform::from_scale(Vec3::splat(SUN_DISC_RADIUS)),
        NotShadowCaster,
        NotShadowReceiver,
    ));
    commands.spawn((
        MoonDisc,
        Mesh3d(sphere),
        MeshMaterial3d(moon_mat.clone()),
        Transform::from_scale(Vec3::splat(MOON_DISC_RADIUS)),
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
        (
            1.0,
            1.0 - 0.55 * warm,
            1.0 - 0.85 * warm,
        )
    } else {
        // Dawn: gentler, more gold than red.
        (
            1.0,
            1.0 - 0.35 * warm,
            1.0 - 0.65 * warm,
        )
    };
    // Illuminance peaks at noon (~10k lux), drops to ~1.5k at horizon.
    let lux = 1500.0 + 8500.0 * elev;
    (Color::srgb(r, g, b), lux)
}

/// Map moon phase + altitude → moon color + illuminance.
///
/// Phase 0/1 = New (invisible), 0.5 = Full (brightest). Cool blue tint.
pub fn moon_color_for_phase(phase: f32, moon_altitude: f32) -> (Color, f32) {
    if moon_altitude <= 0.0 {
        return (Color::BLACK, 0.0);
    }
    // Distance from full (phase 0.5). 0 at full, 1 at new.
    let from_full = (phase - 0.5).abs() * 2.0;
    let visibility = (1.0 - from_full).clamp(0.0, 1.0);
    let elev = (moon_altitude / (PI / 2.0)).clamp(0.0, 1.0);
    let lux = 200.0 * visibility * (0.3 + 0.7 * elev);
    (Color::srgb(0.62, 0.72, 1.00), lux)
}

/// Each-frame system: read Vana sky, update sun + moon transforms,
/// colors, illuminance, and visible disc positions/emissives.
pub fn sun_moon_system(
    mut sky: ResMut<VanaSky>,
    mut q_sun: Query<
        (&mut DirectionalLight, &mut Transform),
        (With<IsSun>, Without<IsMoon>, Without<SunDisc>, Without<MoonDisc>, Without<crate::camera::OperatorCamera>),
    >,
    mut q_moon: Query<
        (&mut DirectionalLight, &mut Transform),
        (With<IsMoon>, Without<IsSun>, Without<SunDisc>, Without<MoonDisc>, Without<crate::camera::OperatorCamera>),
    >,
    mut q_sun_disc: Query<
        &mut Transform,
        (With<SunDisc>, Without<MoonDisc>, Without<IsSun>, Without<IsMoon>, Without<crate::camera::OperatorCamera>),
    >,
    mut q_moon_disc: Query<
        &mut Transform,
        (With<MoonDisc>, Without<SunDisc>, Without<IsSun>, Without<IsMoon>, Without<crate::camera::OperatorCamera>),
    >,
    q_cam: Query<&Transform, With<crate::camera::OperatorCamera>>,
    materials_handle: Option<Res<CelestialMaterials>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    *sky = vana_sky_now();

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
    let (moon_color, moon_lux) = moon_color_for_phase(sky.moon_phase, sky.moon_altitude);
    if let Ok((mut light, mut xf)) = q_moon.single_mut() {
        light.color = moon_color;
        light.illuminance = moon_lux;
        *xf = Transform::from_translation(moon_pos).looking_at(Vec3::ZERO, Vec3::Y);
    }

    // Visible discs ride the camera so they read as "infinitely far".
    let cam_pos = q_cam.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);

    if let Ok(mut disc) = q_sun_disc.single_mut() {
        disc.translation = cam_pos + sun_dir * SKY_RADIUS;
        disc.scale = Vec3::splat(SUN_DISC_RADIUS);
    }
    if let Ok(mut disc) = q_moon_disc.single_mut() {
        disc.translation = cam_pos + moon_dir * SKY_RADIUS;
        disc.scale = Vec3::splat(MOON_DISC_RADIUS);
    }

    // Recolor emissives. Sun emissive scales with daylight (so dawn /
    // dusk sun reads as deep red, noon as blinding white). Moon
    // emissive scales by phase visibility — new moon fades to nearly
    // invisible.
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
            sun_mat.emissive = LinearRgba::new(
                c.red * intensity,
                c.green * intensity * 0.95,
                c.blue * intensity * 0.75,
                1.0,
            );
        }
        if let Some(moon_mat) = materials.get_mut(&handles.moon) {
            let from_full = (sky.moon_phase - 0.5).abs() * 2.0;
            let visibility = (1.0 - from_full).clamp(0.0, 1.0);
            let intensity = if sky.moon_altitude > 0.0 {
                1.5 + 5.0 * visibility
            } else {
                0.0
            };
            moon_mat.emissive = LinearRgba::new(
                0.65 * intensity,
                0.80 * intensity,
                1.20 * intensity,
                1.0,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noon_sun_is_overhead() {
        let sky = vana_sky_from_unix(EARTH_EPOCH_UNIX + 12 * 25);
        assert!((sky.hour - 12.0).abs() < 0.01);
        assert!(sky.sun_altitude > 1.5); // ≈ π/2 = 1.5708
    }

    #[test]
    fn midnight_sun_is_below() {
        let sky = vana_sky_from_unix(EARTH_EPOCH_UNIX); // hour 0
        assert!(sky.sun_altitude < 0.0);
        // And moon is up.
        assert!(sky.moon_altitude > 0.0);
    }

    #[test]
    fn moon_phase_cycles_every_84_v_days() {
        let one_v_day = EARTH_SECS_PER_VANA_DAY;
        let s0 = vana_sky_from_unix(EARTH_EPOCH_UNIX);
        let s84 = vana_sky_from_unix(EARTH_EPOCH_UNIX + 84 * one_v_day);
        assert!((s0.moon_phase - s84.moon_phase).abs() < 1e-4);
    }
}
