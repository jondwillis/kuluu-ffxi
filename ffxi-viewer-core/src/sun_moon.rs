use std::f32::consts::PI;

use bevy::prelude::*;

use crate::hud::vana_clock::EARTH_EPOCH_UNIX;

#[derive(Component)]
pub struct IsSun;

#[derive(Component)]
pub struct IsMoon;

#[derive(Component)]
pub struct SunDisc;

#[derive(Component)]
pub struct MoonDisc;

#[derive(Component)]
pub struct MoonSphere;

const SKY_RADIUS: f32 = 4000.0;

const SUN_DISC_RADIUS: f32 = 120.0;
const MOON_DISC_RADIUS: f32 = 350.0;

const MOON_CYCLE_VANA_DAYS: u64 = 84;
const MOON_PHASE_OFFSET: u64 = (886u64 * 360 + 26) % MOON_CYCLE_VANA_DAYS;

const LIGHT_DISTANCE: f32 = 200.0;

// Maps an F1 diffuse brightness k in [0,1] onto a DirectionalLight illuminance so
// the zone/actor consumers' `k = illuminance / DIR_REF_LUX` recovers it (both use
// DIR_REF_LUX = 12000.0).
const DAT_DIR_REF_LUX: f32 = 12000.0;

#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct VanaSky {
    pub hour: f32,

    pub moon_phase: f32,

    pub moon_illumination: f32,

    pub moon_waxing: bool,

    pub sun_altitude: f32,

    pub moon_altitude: f32,
}

pub fn vana_sky_from_clock(clock: &crate::vana_time::VanaClock) -> VanaSky {
    vana_sky_from_unix(clock.earth_unix_now())
}

// The synthetic sun direction the sun DirectionalLight and sun-attached weather
// generators (weat/<type>/sun1, ParticleGeneratorAttachment.kt:46-52 getSunPosition)
// share: an east->west arc over the Vana'diel day with a small fixed +z tilt.
pub fn sun_direction(hour: f32) -> Vec3 {
    let sun_angle = (hour / 24.0) * 2.0 * PI - PI / 2.0;
    Vec3::new(sun_angle.cos(), sun_angle.sin(), 0.25).normalize()
}

fn vana_sky_from_unix(earth_unix: f64) -> VanaSky {
    let earth_since = (earth_unix - EARTH_EPOCH_UNIX as f64).max(0.0);

    let vana_secs = earth_since * 25.0;
    let day_v_secs = 86400.0;
    let secs_into_day = vana_secs.rem_euclid(day_v_secs);
    let hour = (secs_into_day / 3600.0) as f32;

    let total_v_days = (vana_secs / day_v_secs).floor() as u64;
    let daysmod = (total_v_days + MOON_PHASE_OFFSET) % MOON_CYCLE_VANA_DAYS;
    let moon_phase = daysmod as f32 / MOON_CYCLE_VANA_DAYS as f32;

    let (moon_illumination, moon_waxing) = if daysmod < 42 {
        (1.0 - daysmod as f32 / 42.0, false)
    } else {
        ((daysmod as f32 - 42.0) / 42.0, true)
    };

    let sun_altitude = if (6.0..=18.0).contains(&hour) {
        ((hour - 6.0) / 12.0 * PI).sin() * (PI / 2.0)
    } else {
        let night_hour = if hour < 6.0 { hour + 24.0 } else { hour };
        -((night_hour - 18.0) / 12.0 * PI).sin() * (PI / 2.0)
    };

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

#[derive(Resource)]
pub struct CelestialMaterials {
    pub sun: Handle<StandardMaterial>,
    pub moon: Handle<crate::moon_material::MoonMaterial>,
}

// The SunDisc swaps between the procedural emissive sphere (no sprite) and an additive
// textured billboard quad sized from the DAT "suns" sprite extents. Both meshes are
// pre-built so the swap is a Mesh3d component write.
#[derive(Resource)]
pub struct SunDiscMeshes {
    pub sphere: Handle<Mesh>,
    pub quad: Handle<Mesh>,
}

#[derive(Resource)]
pub struct MoonSphereMaterial(pub Handle<StandardMaterial>);

#[derive(Default)]
pub struct MoonTransitionState {
    pub prev_sun_up: Option<bool>,
    pub prev_moon_up: Option<bool>,
    pub prev_phase_bucket: Option<u8>,
    pub prev_illumination: Option<f32>,
    pub prev_disc_shown: Option<bool>,
}

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
            illuminance: 0.0,
            shadows_enabled: true,
            shadow_depth_bias: 0.2,

            shadow_normal_bias: 0.6,
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

            shadows_enabled: false,
            shadow_depth_bias: 0.2,
            shadow_normal_bias: 1.0,
            ..default()
        },
        bevy::light::VolumetricLight,
        Transform::from_xyz(0.0, -LIGHT_DISTANCE, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    let sphere = meshes.add(Sphere::new(1.0).mesh().ico(3).unwrap());
    let sun_quad = meshes.add(Rectangle::new(1.0, 1.0));
    let sun_mat = materials.add(StandardMaterial {
        base_color: Color::linear_rgb(20.0, 18.0, 10.0),
        unlit: true,
        ..default()
    });

    let moon_quad = meshes.add(Rectangle::new(1.0, 1.0));
    let moon_mat = moon_materials.add(crate::moon_material::MoonMaterial::default());

    use bevy::light::{NotShadowCaster, NotShadowReceiver};
    commands.spawn((
        crate::components::InGameEntity,
        SunDisc,
        Mesh3d(sphere.clone()),
        MeshMaterial3d(sun_mat.clone()),
        Transform::from_scale(Vec3::splat(SUN_DISC_RADIUS)),
        Visibility::Hidden,
        NotShadowCaster,
        NotShadowReceiver,
    ));
    commands.insert_resource(SunDiscMeshes {
        sphere,
        quad: sun_quad,
    });
    commands.spawn((
        crate::components::InGameEntity,
        MoonDisc,
        Mesh3d(moon_quad),
        MeshMaterial3d(moon_mat.clone()),
        Transform::from_scale(Vec3::splat(MOON_DISC_RADIUS * 2.0)),
        Visibility::Hidden,
        NotShadowCaster,
        NotShadowReceiver,
    ));

    let moon_sphere_mesh = meshes.add(Sphere::new(1.0).mesh().ico(4).unwrap());
    let moon_sphere_mat = materials.add(StandardMaterial {
        base_color: Color::linear_rgb(0.18, 0.18, 0.20),

        perceptual_roughness: 0.95,
        metallic: 0.0,

        emissive: LinearRgba::new(0.005, 0.005, 0.008, 1.0),
        ..default()
    });
    commands.spawn((
        crate::components::InGameEntity,
        MoonSphere,
        Mesh3d(moon_sphere_mesh),
        MeshMaterial3d(moon_sphere_mat.clone()),
        Transform::from_scale(Vec3::splat(MOON_DISC_RADIUS)),
        Visibility::Hidden,
        NotShadowCaster,
        NotShadowReceiver,
    ));
    commands.insert_resource(MoonSphereMaterial(moon_sphere_mat));
    commands.insert_resource(CelestialMaterials {
        sun: sun_mat,
        moon: moon_mat,
    });
}

pub fn sun_color_for_hour(hour: f32, sun_altitude: f32) -> (Color, f32) {
    if sun_altitude <= 0.0 {
        return (Color::BLACK, 0.0);
    }

    let elev = (sun_altitude / (PI / 2.0)).clamp(0.0, 1.0);

    let band = 3.0_f32;
    let dist_from_horizon = (hour - 6.0).min(18.0 - hour).max(0.0);
    let raw = ((band - dist_from_horizon) / band).clamp(0.0, 1.0);

    let warm = raw * raw * (3.0 - 2.0 * raw);

    let near_dusk = hour > 12.0;
    let (r, g, b) = if near_dusk {
        (1.0, 1.0 - 0.80 * warm, 1.0 - 0.95 * warm)
    } else {
        (1.0, 1.0 - 0.65 * warm, 1.0 - 0.85 * warm)
    };

    let lux = 1500.0 + 8500.0 * elev;
    (Color::srgb(r, g, b), lux)
}

// Split an F1 diffuse color [r,g,b,a] (already mul/bias-applied in ffxi-dat) into
// a hue (max-normalized so color * k == the original diffuse) and a brightness k
// in [0,1], matching how the zone/actor consumers reconstruct color * k.
fn diffuse_to_light(rgb: [f32; 3]) -> (Vec3, f32) {
    let v = Vec3::new(rgb[0], rgb[1], rgb[2]).max(Vec3::ZERO);
    let k = v.max_element();
    if k <= 1e-4 {
        (Vec3::ZERO, 0.0)
    } else {
        (v / k, k.min(1.0))
    }
}

// research/xim EnvironmentSection.kt:206-225 modelLightMix: models swap moon->sun
// at 06:00 (minute 360) and sun->moon at 18:00 (minute 1080), with a short blend
// window on either side; t=1 means pure sun, t=0 means pure moon.
fn model_light_mix(time_minutes: u32) -> f32 {
    let m = (time_minutes % 1440) as f32;
    if m < 355.0 {
        0.0
    } else if m < 365.0 {
        (m - 355.0) / 10.0
    } else if m < 1075.0 {
        1.0
    } else if m < 1085.0 {
        (1085.0 - m) / 10.0
    } else {
        0.0
    }
}

pub fn moon_color_for_phase(illumination: f32, moon_altitude: f32) -> (Color, f32) {
    if moon_altitude <= 0.0 {
        return (Color::BLACK, 0.0);
    }
    let visibility = illumination.clamp(0.0, 1.0);
    let elev = (moon_altitude / (PI / 2.0)).clamp(0.0, 1.0);

    let lux = 1500.0 * visibility * (0.3 + 0.7 * elev);
    (Color::srgb(0.62, 0.72, 1.00), lux)
}

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

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// Map our moon phase (0 = full, 0.5 = new; daysmod/84) to the retail 12-frame
// sprite index, where 0 = New and 6 = Full (research/xim EnvironmentManager.MoonPhase).
pub fn moon_phase_frame(moon_phase: f32) -> usize {
    const N: usize = ffxi_dat::sprite_sheet::MOON_PHASE_FRAMES;
    (((moon_phase - 0.5).rem_euclid(1.0) * N as f32).round() as usize) % N
}

// No-DAT fallback only. The authoritative tints are the parsed 0x4E DayOfWeekColor /
// 0x4F MoonPhaseColor generator opcodes (research/xim ParticleUpdaters.kt:289-317),
// resolved by celestial_moon_tint below.
const WEEKDAY_MOON_TINT: [[f32; 3]; 8] = [
    [1.00, 0.82, 0.78],
    [1.00, 0.92, 0.78],
    [0.82, 0.92, 1.00],
    [0.85, 1.00, 0.88],
    [0.92, 0.98, 1.00],
    [0.95, 0.85, 1.00],
    [1.00, 1.00, 0.92],
    [0.78, 0.72, 0.85],
];

// research/xim Particle.kt:217-218: getColor() applies colorDayOfWeek then colorMoonPhase
// each via modulateInPlace(it, 2f) — a 2x modulate (out *= 2*c, clamped). Returns the
// combined moon tint in RGB. Falls back to WEEKDAY_MOON_TINT when the zone ships no
// 0x4E/0x4F tables (so the celestial look degrades to the hand-tuned constants).
fn celestial_moon_tint(
    tables: &crate::moon_material::CelestialColorTables,
    total_v_days: u64,
    moon_phase: f32,
) -> [f32; 3] {
    let mut rgb = WEEKDAY_MOON_TINT[(total_v_days % 8) as usize];
    let mut has_dat = false;
    if let Some(dow) = tables.day_of_week {
        let c = dow[(total_v_days % 8) as usize];
        rgb = [c[0], c[1], c[2]];
        has_dat = true;
    }
    if let Some(mp) = tables.moon_phase {
        let f = moon_phase_frame(moon_phase);
        let c = mp[f];
        let base = if has_dat { rgb } else { [1.0, 1.0, 1.0] };
        rgb = [
            (base[0] * 2.0 * c[0]).min(1.0),
            (base[1] * 2.0 * c[1]).min(1.0),
            (base[2] * 2.0 * c[2]).min(1.0),
        ];
    }
    rgb
}

#[derive(bevy::ecs::system::SystemParam)]
pub struct SunMoonRenderCfg<'w> {
    pub settings: Res<'w, crate::graphics_settings::GraphicsSettings>,
    pub moon_sprite: Res<'w, crate::moon_material::MoonSpriteFrames>,
    pub color_tables: Res<'w, crate::moon_material::CelestialColorTables>,
    pub sun_sprite: Res<'w, crate::moon_material::SunSprite>,
    pub sun_disc_meshes: Option<Res<'w, SunDiscMeshes>>,
    pub zone_weather: Res<'w, crate::weather::ZoneWeather>,
    pub zone_lighting: ResMut<'w, crate::weather::ZoneDirectionalLighting>,
}

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
        (
            &mut Transform,
            &mut Visibility,
            &mut Mesh3d,
            &MeshMaterial3d<StandardMaterial>,
        ),
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
            Without<MoonSphere>,
            Without<crate::camera::OperatorCamera>,
        ),
    >,
    mut q_moon_sphere: Query<
        (&mut Transform, &mut Visibility),
        (
            With<MoonSphere>,
            Without<MoonDisc>,
            Without<SunDisc>,
            Without<IsSun>,
            Without<IsMoon>,
            Without<crate::camera::OperatorCamera>,
        ),
    >,
    q_cam: Query<&Transform, With<crate::camera::OperatorCamera>>,
    materials_handle: Option<Res<CelestialMaterials>>,
    moon_sphere_handle: Option<Res<MoonSphereMaterial>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut moon_materials: ResMut<Assets<crate::moon_material::MoonMaterial>>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    sky_realism: Res<crate::sky_realism::SkyRealism>,
    mut render_cfg: SunMoonRenderCfg,
    mut transition_state: Local<MoonTransitionState>,
) {
    let MoonTransitionState {
        prev_sun_up,
        prev_moon_up,
        prev_phase_bucket,
        prev_illumination,
        prev_disc_shown,
    } = &mut *transition_state;
    *sky = vana_sky_from_clock(&vana_clock);

    let sun_up_now = sky.sun_altitude > 0.0;
    if let Some(prev) = *prev_sun_up {
        if prev != sun_up_now {
            toasts.write(crate::snapshot::ToastEvent::system(
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
            toasts.write(crate::snapshot::ToastEvent::system(
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

    let phase_bucket = ((sky.moon_phase * 8.0).floor() as i32).rem_euclid(8) as u8;
    if let Some(prev) = *prev_phase_bucket {
        if prev != phase_bucket {
            let earth_since = (vana_clock.earth_unix_now()
                - crate::hud::vana_clock::EARTH_EPOCH_UNIX as f64)
                .max(0.0);
            let total_v_days = (earth_since * 25.0 / 86400.0) as u64;
            let weekday = crate::hud::vana_clock::VanaWeekday::from_vana_day(total_v_days).name();
            toasts.write(crate::snapshot::ToastEvent::system(format!(
                "☾ Moon: {} ({:.0}% illuminated) — {}",
                MOON_PHASE_NAMES[phase_bucket as usize],
                sky.moon_illumination * 100.0,
                weekday,
            )));
        }
    }
    *prev_phase_bucket = Some(phase_bucket);

    let sun_dir = sun_direction(sky.hour);
    let sun_pos = sun_dir * LIGHT_DISTANCE;

    // research/xim EnvironmentSection.kt:151-159: the 0x2F terrain block's sun/moon
    // diffuse colors are authoritative when the zone ships records; the synthetic
    // sun_color_for_hour/moon_color_for_phase path is the records.is_empty() fallback.
    let dat = render_cfg.zone_weather.current;
    let time_minutes = (sky.hour * 60.0).rem_euclid(1440.0) as u32;

    let (sun_color, sun_lux) = match dat {
        Some(rec) if sky.sun_altitude > 0.0 => {
            let d = rec.sunlight_diffuse_landscape;
            let (hue, k) = diffuse_to_light([d[0], d[1], d[2]]);
            (Color::linear_rgb(hue.x, hue.y, hue.z), k * DAT_DIR_REF_LUX)
        }
        Some(_) => (Color::BLACK, 0.0),
        None => sun_color_for_hour(sky.hour, sky.sun_altitude),
    };

    if let Ok((mut light, mut xf)) = q_sun.single_mut() {
        light.color = sun_color;
        light.illuminance = sun_lux;
        *xf = Transform::from_translation(sun_pos).looking_at(Vec3::ZERO, Vec3::Y);
    }

    let moon_dir = if sky_realism.physical_moon_orbit {
        let cos_theta = (1.0 - 2.0 * sky.moon_illumination).clamp(-1.0, 1.0);
        let theta = cos_theta.acos();
        let signed = if sky.moon_waxing { theta } else { -theta };
        Quat::from_rotation_z(signed) * sun_dir
    } else {
        let moon_angle = (sky.hour / 24.0) * 2.0 * PI - PI / 2.0 + PI;
        Vec3::new(moon_angle.cos(), moon_angle.sin(), 0.25).normalize()
    };

    let moon_altitude = moon_dir.y.asin();
    sky.moon_altitude = moon_altitude;
    let moon_pos = moon_dir * LIGHT_DISTANCE;
    let (moon_color, moon_lux) = match dat {
        Some(rec) if sky.moon_altitude > 0.0 => {
            let d = rec.moonlight_diffuse_landscape;
            let (hue, k) = diffuse_to_light([d[0], d[1], d[2]]);
            (Color::linear_rgb(hue.x, hue.y, hue.z), k * DAT_DIR_REF_LUX)
        }
        Some(_) => (Color::BLACK, 0.0),
        None => moon_color_for_phase(sky.moon_illumination, sky.moon_altitude),
    };
    if let Ok((mut light, mut xf)) = q_moon.single_mut() {
        light.color = moon_color;
        light.illuminance = moon_lux;
        *xf = Transform::from_translation(moon_pos).looking_at(Vec3::ZERO, Vec3::Y);
    }

    // Publish the entity(model) + landscape(terrain) split for the actor- and
    // zone-material lighting consumers. The model light is a single moon<->sun
    // blend (research/xim EnvironmentSection.kt:206-225); landscape feeds both
    // sun(dir0) and moon(dir1) slots from the terrain block.
    if let Some(rec) = dat {
        let sun_up = sky.sun_altitude > 0.0;
        let moon_up = sky.moon_altitude > 0.0;

        let (e_sun_hue, e_sun_k) = diffuse_to_light([
            rec.sunlight_diffuse_entity[0],
            rec.sunlight_diffuse_entity[1],
            rec.sunlight_diffuse_entity[2],
        ]);
        let (e_moon_hue, e_moon_k) = diffuse_to_light([
            rec.moonlight_diffuse_entity[0],
            rec.moonlight_diffuse_entity[1],
            rec.moonlight_diffuse_entity[2],
        ]);
        let mix = model_light_mix(time_minutes);
        let model_dir = moon_dir.lerp(sun_dir, mix).normalize_or_zero();
        let sun_rgb = e_sun_hue * e_sun_k;
        let moon_rgb = e_moon_hue * e_moon_k;
        let model_rgb = moon_rgb.lerp(sun_rgb, mix);
        // Keep the authored overbright magnitude (>1): diffuse_to_light clamps k to 1.0,
        // which cropped the entity directional and flattened actor form. The actor caps
        // the ceiling (MODEL_DIR_MAX). Scalar max for the cranelift dev backend, which
        // can't lower glam's horizontal-max intrinsic.
        let model_k = model_rgb.x.max(model_rgb.y).max(model_rgb.z).max(0.0);
        let model_color = if model_k > 1e-4 {
            model_rgb / model_k
        } else {
            Vec3::ZERO
        };

        let (s_hue, s_k) = diffuse_to_light([
            rec.sunlight_diffuse_landscape[0],
            rec.sunlight_diffuse_landscape[1],
            rec.sunlight_diffuse_landscape[2],
        ]);
        let (m_hue, m_k) = diffuse_to_light([
            rec.moonlight_diffuse_landscape[0],
            rec.moonlight_diffuse_landscape[1],
            rec.moonlight_diffuse_landscape[2],
        ]);

        *render_cfg.zone_lighting = crate::weather::ZoneDirectionalLighting {
            valid: true,
            indoors: rec.indoors,
            model_dir,
            model_color,
            model_k,
            ambient_entity: Vec3::new(
                rec.ambient_entity[0],
                rec.ambient_entity[1],
                rec.ambient_entity[2],
            ),
            sun_dir,
            sun_color: s_hue,
            sun_k: if sun_up { s_k } else { 0.0 },
            moon_dir,
            moon_color: m_hue,
            moon_k: if moon_up { m_k } else { 0.0 },
            ambient_landscape: Vec3::new(
                rec.ambient_landscape[0],
                rec.ambient_landscape[1],
                rec.ambient_landscape[2],
            ),
        };
    } else {
        render_cfg.zone_lighting.valid = false;
    }

    let cam_pos = q_cam.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);

    let sun_visible = sky.sun_altitude > -0.05;
    let sun_sprite_tex = render_cfg.sun_sprite.texture.clone();
    if let Ok((mut disc, mut vis, mut mesh3d, _)) = q_sun_disc.single_mut() {
        disc.translation = cam_pos + sun_dir * SKY_RADIUS;
        // research/xim: the sun is an attach=0xE additive billboard. With a "suns"
        // sprite the disc is a camera-facing textured quad; otherwise the procedural
        // emissive sphere (no sprite fallback).
        if let Some(meshes) = render_cfg.sun_disc_meshes.as_deref() {
            let want = if sun_sprite_tex.is_some() {
                &meshes.quad
            } else {
                &meshes.sphere
            };
            if mesh3d.0 != *want {
                mesh3d.0 = want.clone();
            }
        }
        if sun_sprite_tex.is_some() {
            disc.scale = Vec3::splat(SUN_DISC_RADIUS * 2.0);
            disc.look_at(cam_pos, Vec3::Y);
        } else {
            disc.scale = Vec3::splat(SUN_DISC_RADIUS);
        }
        *vis = if sun_visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }

    let moon_visible = sky.moon_altitude > 0.0
        && (sky.moon_illumination > 0.02 || sky_realism.physical_moon_orbit);
    let illusion = if sky_realism.moon_illusion {
        let alt = sky.moon_altitude.max(0.0);
        let t = (alt / (PI / 6.0)).clamp(0.0, 1.0);
        1.30 - 0.30 * t
    } else {
        1.0
    };

    if let Ok((mut disc, mut vis)) = q_moon_disc.single_mut() {
        let moon_world = cam_pos + moon_dir * SKY_RADIUS;
        disc.translation = moon_world;

        disc.scale = Vec3::splat(MOON_DISC_RADIUS * 2.0 * illusion);

        disc.look_at(cam_pos, Vec3::Y);
        let disc_shown = moon_visible && !sky_realism.physical_moon_orbit;
        *vis = if disc_shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *prev_disc_shown != Some(disc_shown) {
            info!(
                hour = sky.hour,
                moon_altitude = sky.moon_altitude,
                moon_illumination = sky.moon_illumination,
                disc_y = moon_world.y - cam_pos.y,
                sprite_loaded = render_cfg.moon_sprite.0.is_some(),
                shown = disc_shown,
                "moon disc visibility"
            );
            *prev_disc_shown = Some(disc_shown);
        }
    }

    if let Ok((mut sphere, mut vis)) = q_moon_sphere.single_mut() {
        sphere.translation = cam_pos + moon_dir * SKY_RADIUS;
        sphere.scale = Vec3::splat(MOON_DISC_RADIUS * illusion);
        *vis = if moon_visible && sky_realism.physical_moon_orbit {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }

    if let Some(handle) = moon_sphere_handle.as_deref() {
        if let Some(mat) = materials.get_mut(&handle.0) {
            let earth_since = (vana_clock.earth_unix_now()
                - crate::hud::vana_clock::EARTH_EPOCH_UNIX as f64)
                .max(0.0);
            let total_v_days = (earth_since * 25.0 / 86400.0) as u64;
            let t = celestial_moon_tint(&render_cfg.color_tables, total_v_days, sky.moon_phase);

            mat.base_color = Color::linear_rgb(t[0] * 0.20, t[1] * 0.20, t[2] * 0.22);
        }
    }

    if let Some(handles) = materials_handle.as_deref() {
        if let Some(sun_mat) = materials.get_mut(&handles.sun) {
            let visible = sky.sun_altitude.max(-0.2);

            let elev_norm = (visible / (PI / 2.0)).clamp(0.0, 1.0);
            let mut intensity = if visible > 0.0 {
                2.0 + 20.0 * elev_norm.sqrt()
            } else {
                (1.0 + 5.0 * (visible + 0.2) / 0.2).max(0.0)
            };

            if !sky_realism.horizon_dimming && visible > 0.0 {
                intensity = 8.0 + 14.0 * elev_norm;
            }
            let c = sun_color.to_linear();
            sun_mat.base_color = Color::linear_rgb(
                c.red * intensity,
                c.green * intensity * 0.95,
                c.blue * intensity * 0.75,
            );
            // With a retail "suns" sprite the disc renders as an additive textured
            // billboard; without one it stays the untextured emissive sphere.
            if sun_sprite_tex.is_some() {
                if sun_mat.base_color_texture != sun_sprite_tex {
                    sun_mat.base_color_texture = sun_sprite_tex.clone();
                }
                let f = render_cfg.sun_sprite.frame_uv;
                sun_mat.uv_transform = bevy::math::Affine2::from_scale_angle_translation(
                    Vec2::new(f.z - f.x, f.w - f.y),
                    0.0,
                    Vec2::new(f.x, f.y),
                );
                sun_mat.alpha_mode = AlphaMode::Add;
            } else if sun_mat.base_color_texture.is_some() {
                sun_mat.base_color_texture = None;
                sun_mat.alpha_mode = AlphaMode::Opaque;
            }
        }
        if let Some(moon_mat) = moon_materials.get_mut(&handles.moon) {
            let visibility = sky.moon_illumination.clamp(0.0, 1.0);

            let mut intensity = if sky.moon_altitude > 0.0 {
                0.6 + 1.4 * visibility
            } else {
                0.0
            };

            let earth_since = (vana_clock.earth_unix_now()
                - crate::hud::vana_clock::EARTH_EPOCH_UNIX as f64)
                .max(0.0);
            let total_v_days = (earth_since * 25.0 / 86400.0) as u64;
            let mut tint =
                celestial_moon_tint(&render_cfg.color_tables, total_v_days, sky.moon_phase);

            if sky_realism.horizon_reddening && sky.moon_altitude > 0.0 {
                let alt_norm = (sky.moon_altitude / (PI / 9.0)).clamp(0.0, 1.0);
                let warmth = 1.0 - alt_norm;
                let red_tint = [1.00, 0.55, 0.35];
                tint = [
                    lerp(tint[0], red_tint[0], warmth * 0.7),
                    lerp(tint[1], red_tint[1], warmth * 0.7),
                    lerp(tint[2], red_tint[2], warmth * 0.7),
                ];
            }

            if sky_realism.horizon_dimming && sky.moon_altitude > 0.0 {
                let alt_norm = (sky.moon_altitude / (PI / 6.0)).clamp(0.0, 1.0);
                intensity *= 0.5 + 0.5 * alt_norm;
            }

            let earthshine = if sky_realism.earthshine {
                let crescent_strength = (1.0 - visibility).powf(2.0);
                0.06 + 0.10 * crescent_strength
            } else {
                0.0
            };

            let mode = match render_cfg.moon_sprite.0 {
                Some(frames) => {
                    moon_mat.data.frame_uv = frames[moon_phase_frame(sky.moon_phase)];
                    2.0
                }
                None => 0.0,
            };
            moon_mat.data.tint = Vec4::new(tint[0], tint[1], tint[2], mode);
            moon_mat.data.params = Vec4::new(
                sky.moon_illumination,
                if sky.moon_waxing { 1.0 } else { -1.0 },
                intensity,
                earthshine,
            );
        }
    }

    if sky_realism.physical_moon_orbit && sky_realism.eclipses {
        let illum = sky.moon_illumination;
        if let Some(prev) = *prev_illumination {
            if prev < 0.999 && illum >= 0.999 && sky.moon_altitude > 0.0 {
                toasts.write(crate::snapshot::ToastEvent::system(
                    "🌑 Lunar eclipse — Vana'diel's shadow falls upon the moon.".to_string(),
                ));
            }

            if prev > 0.001 && illum <= 0.001 && sky.sun_altitude > 0.0 {
                toasts.write(crate::snapshot::ToastEvent::system(
                    "🌒 Solar eclipse — the moon crosses the sun.".to_string(),
                ));
            }
        }
        *prev_illumination = Some(illum);
    } else {
        *prev_illumination = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::vana_clock::EARTH_SECS_PER_VANA_DAY;

    // research/xim EnvironmentSection.kt:206-225: pure moon before 355, ramp to pure
    // sun by 365, pure sun until 1075, ramp back to pure moon by 1085.
    #[test]
    fn model_light_mix_matches_xim_thresholds() {
        assert_eq!(model_light_mix(0), 0.0);
        assert_eq!(model_light_mix(354), 0.0);
        assert_eq!(model_light_mix(355), 0.0);
        assert!((model_light_mix(360) - 0.5).abs() < 1e-5);
        assert_eq!(model_light_mix(365), 1.0);
        assert_eq!(model_light_mix(720), 1.0);
        assert_eq!(model_light_mix(1074), 1.0);
        assert!((model_light_mix(1080) - 0.5).abs() < 1e-5);
        assert_eq!(model_light_mix(1085), 0.0);
        assert_eq!(model_light_mix(1200), 0.0);
    }

    #[test]
    fn diffuse_to_light_max_normalizes_and_preserves_product() {
        let (hue, k) = diffuse_to_light([0.5, 0.25, 0.0]);
        assert!((k - 0.5).abs() < 1e-6);
        assert!((hue.x * k - 0.5).abs() < 1e-6);
        assert!((hue.y * k - 0.25).abs() < 1e-6);
        assert_eq!(hue.z, 0.0);

        let (hue0, k0) = diffuse_to_light([0.0, 0.0, 0.0]);
        assert_eq!(hue0, Vec3::ZERO);
        assert_eq!(k0, 0.0);
    }

    #[test]
    fn noon_sun_is_overhead() {
        let sky = vana_sky_from_unix((EARTH_EPOCH_UNIX + 12 * 144) as f64);
        assert!((sky.hour - 12.0).abs() < 0.01);
        assert!(sky.sun_altitude > 1.5);
    }

    #[test]
    fn moon_phase_matches_lsb_formula() {
        let v_day = EARTH_SECS_PER_VANA_DAY;
        let sky = vana_sky_from_unix((EARTH_EPOCH_UNIX + 4 * v_day) as f64);
        assert!(
            sky.moon_illumination < 0.05,
            "expected new moon at V-day 4, got illumination {}",
            sky.moon_illumination
        );

        let sky = vana_sky_from_unix((EARTH_EPOCH_UNIX + 46 * v_day) as f64);
        assert!(
            sky.moon_illumination > 0.95,
            "expected full moon at V-day 46, got illumination {}",
            sky.moon_illumination
        );
    }

    #[test]
    fn midnight_sun_is_below() {
        let sky = vana_sky_from_unix(EARTH_EPOCH_UNIX as f64);
        assert!(sky.sun_altitude < 0.0);

        assert!(sky.moon_altitude > 0.0);
    }

    #[test]
    fn moon_phase_frame_matches_xim_enum() {
        // moon_phase 0 == full (frame 6), 0.5 == new (frame 0); quarters at 3/9.
        assert_eq!(moon_phase_frame(0.0), 6);
        assert_eq!(moon_phase_frame(0.5), 0);
        assert_eq!(moon_phase_frame(0.75), 3); // waxing first-quarter
        assert_eq!(moon_phase_frame(0.25), 9); // waning last-quarter
        for k in 0..84 {
            let f = moon_phase_frame(k as f32 / 84.0);
            assert!(f < 12, "frame {f} out of range at daysmod {k}");
        }
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
        let base = EARTH_EPOCH_UNIX as f64 + 6.0;
        let a = vana_sky_from_unix(base);
        let b = vana_sky_from_unix(base + 0.1);
        assert!(a.hour != b.hour, "hour did not advance sub-second");
    }
}
