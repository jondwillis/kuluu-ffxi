use std::f32::consts::PI;

use bevy::prelude::*;

use crate::vana_time::EARTH_EPOCH_UNIX;

#[derive(Component)]
pub struct IsSun;

#[derive(Component)]
pub struct IsMoon;

#[derive(Component)]
pub struct SunDisc;

#[derive(Component)]
pub struct MoonDisc;

/// Distance the celestial discs ride at, inside the
/// [`crate::skybox::SKYBOX_RADIUS`] dome so they depth-sort in front of it.
/// Fixed, not frustum-derived: [`crate::skybox::camera_far`] is what keeps them
/// in view at every draw distance.
pub const SKY_RADIUS: f32 = 4000.0;

pub const SUN_DISC_RADIUS: f32 = 120.0;

/// Angular radius of the drawn sun disc as seen from the camera. The lens-flare occlusion
/// query samples exactly this cone: retail tests visibility by drawing the sun particle's own
/// quad with colour writes off and taking the fraction of pixels that pass
/// (research/xim src/jsMain/kotlin/xim/poc/ParticleDrawer.kt renderHazeTexture).
pub fn sun_angular_radius() -> f32 {
    (SUN_DISC_RADIUS / SKY_RADIUS).atan()
}

/// Edge length of the moon billboard at [`SKY_RADIUS`], sized to subtend retail's angle: the
/// `moon` generator's sprite quad measures 3.94 units and its init scale is 20 (dat-celestial-probe,
/// file 201), so retail's moon is 78.8 units across at the 900-unit celestial distance — 5.0°,
/// which is 350 units across at our 4000.
const MOON_DISC_EDGE: f32 = 350.0;

// The sun disc is authored HDR-overbright so it clears the bloom threshold with
// headroom; uncapped its ~22x peak blows the disc out to a solid white blob.
const SUN_DISC_MAX_INTENSITY: f32 = 8.0;

// moon.wgsl's `params` is a vec4 for uniform alignment; only xyz carry meaning.
const MOON_PARAMS_PAD: f32 = 0.0;

const MOON_CYCLE_VANA_DAYS: u64 = 84;
const MOON_PHASE_OFFSET: u64 = (886u64 * 360 + 26) % MOON_CYCLE_VANA_DAYS;

const LIGHT_DISTANCE: f32 = 200.0;

// research/XIClient Rendering/ShadowRenderer.cpp ShadowRenderer::Init — retail never lets a shadow rake:
// whenever the light's elevation is shallower than ANGLE_PI_OVER_3 it rewrites the vertical
// component to `-(sin(ANGLE_PI_OVER_3) * |xz|)` and renormalises, and :75-79 snaps a
// below-horizon light to a fixed steep vector outright. Without it a low sun stretches every
// cast shadow across the whole zone at dawn and dusk. research/XIClient/src/XIClient/source/Constants/Floats.cpp Values::ANGLE_PI_OVER_3.
const SHADOW_ELEVATION_SIN_ARG: f32 = PI / 3.0;

/// Elevation retail's rewrite actually settles at: `sin(ANGLE_PI_OVER_3) * |xz|` over an
/// unchanged `|xz|` is `atan(sin(ANGLE_PI_OVER_3))`, whatever the light started at.
pub fn shadow_min_elevation() -> f32 {
    SHADOW_ELEVATION_SIN_ARG.sin().atan()
}

/// Ground-projected shadow direction; unsuitable for depth-map self-shadowing.
pub fn shadow_cast_direction(to_light: Vec3) -> Vec3 {
    let horizontal = Vec2::new(to_light.x, to_light.z).length();
    if to_light.y.atan2(horizontal) >= SHADOW_ELEVATION_SIN_ARG {
        return to_light;
    }
    Vec3::new(
        to_light.x,
        SHADOW_ELEVATION_SIN_ARG.sin() * horizontal,
        to_light.z,
    )
    .try_normalize()
    .unwrap_or(Vec3::Y)
}

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

// The sun direction shared by the sun DirectionalLight, the lens flare, and the Sun-attached
// weather generators. research/xim EnvironmentManager.kt getSunPosition:
// `Vector3f(sin a, cos a, 0)` with `a = timeOfDaySeconds * (0.5pi / 6h)` — one full turn per
// Vana'diel day, in the XY plane, then mapped FFXI -> Bevy as (x, -y, -z).
pub fn sun_direction(hour: f32) -> Vec3 {
    let a = (hour / 24.0) * 2.0 * PI;
    Vec3::new(a.sin(), -a.cos(), 0.0)
}

// Whole Vana'diel days since the epoch — the index behind both the elemental weekday and the
// moon phase.
pub fn vana_day_index(clock: &crate::vana_time::VanaClock) -> u64 {
    let earth_since = (clock.earth_unix_now() - EARTH_EPOCH_UNIX as f64).max(0.0);
    (earth_since * 25.0 / 86400.0) as u64
}

// Set while the zone DAT's own Sun/Moon-attached particle generators are live, so the
// hand-authored disc primitives below stand down and retail's billboards are what the player
// sees. Written by celestial_particles (native only); stays false on wasm and in zones that
// ship no celestial set, where the procedural discs remain the fallback.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct DatCelestials {
    pub active: bool,
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

#[derive(Default)]
pub struct MoonTransitionState {
    pub prev_sun_up: Option<bool>,
    pub prev_moon_up: Option<bool>,
    pub prev_phase_bucket: Option<u8>,
    pub prev_disc_shown: Option<bool>,
    // Perf: last-written celestial outputs. Unconditional per-frame writes to
    // DirectionalLight/Transform and Assets::get_mut fire change detection /
    // AssetEvent::Modified every frame; under vendored bevy 0.19's retained
    // render world that re-specializes and re-prepares bins (incl. every
    // shadow cascade view) each frame. Celestial motion is slow, so skip the
    // writes until the target moves beyond a small epsilon. Asset ids are
    // cached so a zone-reload material swap invalidates the cache.
    pub sun_light_written: Option<(Vec3, f32, Vec3, bool)>,
    pub moon_light_written: Option<(Vec3, f32, Vec3, bool)>,
    pub sun_disc_written: Option<(AssetId<StandardMaterial>, Vec3)>,
    pub moon_disc_written: Option<(
        AssetId<crate::moon_material::MoonMaterial>,
        Vec4,
        Vec4,
        Option<Vec4>,
    )>,
}

// ~half an 8-bit color step; light direction moves ~1.8e-3 rad/s of real time,
// so 1e-4 turns per-frame writes into a few writes per second.
const CELESTIAL_COLOR_EPS: f32 = 1.0 / 512.0;
const CELESTIAL_DIR_EPS: f32 = 1e-4;

fn celestial_scalar_changed(prev: f32, next: f32) -> bool {
    (prev - next).abs() > prev.abs().max(1.0) * 1e-3
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
            shadow_maps_enabled: false,
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

            shadow_maps_enabled: false,
            shadow_depth_bias: 0.2,
            shadow_normal_bias: 1.0,
            ..default()
        },
        cascade_config_from_settings(settings),
        bevy::light::VolumetricLight,
        Transform::from_xyz(0.0, -LIGHT_DISTANCE, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    let sphere = meshes.add(Sphere::new(1.0).mesh().ico(3).unwrap());
    // Both celestial bodies ride SKY_RADIUS, far past every measured 0x2F fog distance,
    // and their DAT generators clear fog (measured sun0/sun1 and moon/kasa = 0x02C400C0;
    // research/XIClient CMoElem.cpp CMoElem::PrepDX). Bevy fogs unlit StandardMaterials too, so
    // without this the disc renders as a flat horizon-coloured blob.
    let sun_mat = materials.add(StandardMaterial {
        base_color: Color::linear_rgb(20.0, 18.0, 10.0),
        unlit: true,
        fog_enabled: false,
        ..default()
    });

    let moon_quad = meshes.add(Rectangle::new(1.0, 1.0));
    let moon_mat = moon_materials.add(crate::moon_material::MoonMaterial::default());

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
        Transform::from_scale(Vec3::splat(MOON_DISC_EDGE)),
        Visibility::Hidden,
        NotShadowCaster,
        NotShadowReceiver,
    ));

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

/// Turn a celestial billboard's quad face toward the camera.
///
/// `Rectangle`'s only face is its +Z side (`bevy_mesh` dim2.rs: normals `[0,0,1]`, CCW from
/// +Z), and a `Material` impl has no cull-mode hook — `render/mesh.rs` pins `cull_mode:
/// Some(Face::Back)` for every one. `look_at(cam)` aims the quad's FORWARD (-Z) at the
/// camera, which shows it its back face and culls it away; the direction has to be the one
/// pointing away from the camera so +Z lands on the viewer.
fn face_camera(disc: &mut Transform, cam_pos: Vec3) {
    disc.look_to(disc.translation - cam_pos, Vec3::Y);
}

// DAT-space (FFXI, Y-down/Z-flipped) direction -> Bevy, same axis mapping as
// scene::mzb_to_bevy but for a direction (no translation).
fn ffxi_dir_to_bevy(d: [f32; 3]) -> Vec3 {
    Vec3::new(d[0], -d[1], -d[2])
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

/// The terrain(landscape) half of [`crate::weather::ZoneDirectionalLighting`]: retail's two
/// weather diffuse lights plus the ambient a block is drawn with.
#[derive(Clone, Copy)]
struct LandscapeLighting {
    sun_dir: Vec3,
    sun_color: Vec3,
    sun_k: f32,
    moon_dir: Vec3,
    moon_color: Vec3,
    moon_k: f32,
    ambient: Vec3,
}

// ZoneRenderer.cpp ZoneRenderer::RenderChunk2 draws each block through `positionedBlock->Area`:
// `GetWeatherDiffuseLights` (XiArea.cpp XiArea::GetWeatherDiffuseLights - env2 ColorPalette[0]/[1], the terrain
// block's sun/moon diffuse) and `GetAmbient(_, 0)` (XiArea.cpp XiArea::GetAmbient - env2
// ColorPalette[2]) are both read off that area, so the record fed here is the area's and
// not the zone's. Its OWN indoor flag decides how the moon slot is read: indoors the arc
// is replaced by one static diffuse and the moon bytes are a signed direction rather than
// a color (research/xim EnvironmentSection.kt getLightingParams).
fn landscape_lighting(
    rec: &ffxi_dat::weather::WeatherRecord,
    sun_dir: Vec3,
    moon_dir: Vec3,
    sun_up: bool,
    moon_up: bool,
) -> LandscapeLighting {
    let ambient = Vec3::new(
        rec.ambient_landscape[0],
        rec.ambient_landscape[1],
        rec.ambient_landscape[2],
    );
    let (s_hue, s_k) = diffuse_to_light([
        rec.sunlight_diffuse_landscape[0],
        rec.sunlight_diffuse_landscape[1],
        rec.sunlight_diffuse_landscape[2],
    ]);

    if rec.indoors {
        let land_dir = ffxi_dir_to_bevy(rec.indoor_light_dir_landscape);
        return LandscapeLighting {
            sun_dir: land_dir,
            sun_color: s_hue,
            sun_k: if land_dir == Vec3::ZERO { 0.0 } else { s_k },
            moon_dir,
            moon_color: Vec3::ZERO,
            moon_k: 0.0,
            ambient,
        };
    }

    let (m_hue, m_k) = diffuse_to_light([
        rec.moonlight_diffuse_landscape[0],
        rec.moonlight_diffuse_landscape[1],
        rec.moonlight_diffuse_landscape[2],
    ]);
    LandscapeLighting {
        sun_dir,
        sun_color: s_hue,
        sun_k: if sun_up { s_k } else { 0.0 },
        moon_dir,
        moon_color: m_hue,
        moon_k: if moon_up { m_k } else { 0.0 },
        ambient,
    }
}

// research/xim EnvironmentSection.kt modelLightMix: models swap moon->sun
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

fn sun_transition_toast(sun_up: bool) -> crate::snapshot::ToastEvent {
    crate::snapshot::ToastEvent::debug(if sun_up { "☀ Sunrise" } else { "☀ Sunset" }.to_string())
}

fn moon_transition_toast(moon_up: bool) -> crate::snapshot::ToastEvent {
    crate::snapshot::ToastEvent::debug(
        if moon_up {
            "☾ Moonrise"
        } else {
            "☾ Moonset"
        }
        .to_string(),
    )
}

fn moon_phase_toast(
    phase_bucket: u8,
    illumination: f32,
    weekday: &str,
) -> crate::snapshot::ToastEvent {
    crate::snapshot::ToastEvent::debug(format!(
        "☾ Moon: {} ({:.0}% illuminated) — {}",
        MOON_PHASE_NAMES[phase_bucket as usize],
        illumination * 100.0,
        weekday,
    ))
}

// Map our moon phase (0 = full, 0.5 = new; daysmod/84) to the retail 12-frame
// sprite index, where 0 = New and 6 = Full (research/xim EnvironmentManager.MoonPhase).
pub fn moon_phase_frame(moon_phase: f32) -> usize {
    const N: usize = ffxi_dat::sprite_sheet::MOON_PHASE_FRAMES;
    (((moon_phase - 0.5).rem_euclid(1.0) * N as f32).round() as usize) % N
}

// No-DAT fallback only. The authoritative tints are the parsed 0x4E DayOfWeekColor /
// 0x4F MoonPhaseColor generator opcodes (research/xim ParticleUpdaters.kt DayOfWeekColorUpdater),
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

// research/xim Particle.kt: getColor() applies colorDayOfWeek then colorMoonPhase
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
    pub zone_weather: Res<'w, crate::weather::ZoneWeather>,
    pub zone_lighting: ResMut<'w, crate::weather::ZoneDirectionalLighting>,
    pub dat_celestials: Res<'w, DatCelestials>,
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
            Without<crate::camera::OperatorCamera>,
        ),
    >,
    q_cam: Query<&Transform, With<crate::camera::OperatorCamera>>,
    materials_handle: Option<Res<CelestialMaterials>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut moon_materials: ResMut<Assets<crate::moon_material::MoonMaterial>>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    mut render_cfg: SunMoonRenderCfg,
    mut transition_state: Local<MoonTransitionState>,
) {
    let MoonTransitionState {
        prev_sun_up,
        prev_moon_up,
        prev_phase_bucket,
        prev_disc_shown,
        sun_light_written,
        moon_light_written,
        sun_disc_written,
        moon_disc_written,
    } = &mut *transition_state;
    *sky = vana_sky_from_clock(&vana_clock);

    let sun_up_now = sky.sun_altitude > 0.0;
    if let Some(prev) = *prev_sun_up {
        if prev != sun_up_now {
            toasts.write(sun_transition_toast(sun_up_now));
        }
    }
    *prev_sun_up = Some(sun_up_now);

    let moon_up_now = sky.moon_altitude > 0.0;
    if let Some(prev) = *prev_moon_up {
        if prev != moon_up_now {
            toasts.write(moon_transition_toast(moon_up_now));
        }
    }
    *prev_moon_up = Some(moon_up_now);

    let phase_bucket = ((sky.moon_phase * 8.0).floor() as i32).rem_euclid(8) as u8;
    if let Some(prev) = *prev_phase_bucket {
        if prev != phase_bucket {
            let weekday =
                crate::vana_time::VanaWeekday::from_vana_day(vana_day_index(&vana_clock)).name();
            toasts.write(moon_phase_toast(
                phase_bucket,
                sky.moon_illumination,
                weekday,
            ));
        }
    }
    *prev_phase_bucket = Some(phase_bucket);

    let sun_dir = sun_direction(sky.hour);

    // research/xim EnvironmentSection.kt: the 0x2F terrain block's sun/moon
    // diffuse colors are authoritative when the zone ships records; the synthetic
    // sun_color_for_hour/moon_color_for_phase path is the records.is_empty() fallback.
    let dat = render_cfg.zone_weather.current;
    let time_minutes = (sky.hour * 60.0).rem_euclid(1440.0) as u32;

    let indoors = dat.is_some_and(|rec| rec.indoors);

    // research/xim EnvironmentSection.kt getLightingParams: indoors, the sun/moon arc is
    // replaced by ONE static diffuse light (direction from the moon-color bytes,
    // color from the sun diffuse) and cascade shadow-mapping has no retail
    // equivalent inside — the sun must not reach through walls.
    //
    // These two Bevy DirectionalLights stay on the ZONE record even though the
    // published terrain lighting below follows the player's area: they drive the
    // shadow cascade (a zone-wide directional whose retail counterpart is not the
    // per-block palette at all) and the no-record fallback in
    // ffxi_zone_material::update_zone_material_lighting, which the FFXI materials
    // reach only when the zone ships no 0x2F records — and then there is no area
    // record either.
    let (sun_color, sun_lux, sun_to_dir) = match dat {
        Some(rec) if rec.indoors => {
            let d = rec.sunlight_diffuse_entity;
            let (hue, k) = diffuse_to_light([d[0], d[1], d[2]]);
            let dir = ffxi_dir_to_bevy(rec.indoor_light_dir_entity);
            if dir == Vec3::ZERO || k <= 0.0 {
                (Color::BLACK, 0.0, sun_dir)
            } else {
                (
                    Color::linear_rgb(hue.x, hue.y, hue.z),
                    k * DAT_DIR_REF_LUX,
                    dir,
                )
            }
        }
        Some(rec) if sky.sun_altitude > 0.0 => {
            let d = rec.sunlight_diffuse_landscape;
            let (hue, k) = diffuse_to_light([d[0], d[1], d[2]]);
            (
                Color::linear_rgb(hue.x, hue.y, hue.z),
                k * DAT_DIR_REF_LUX,
                sun_dir,
            )
        }
        Some(_) => (Color::BLACK, 0.0, sun_dir),
        None => {
            let (c, lux) = sun_color_for_hour(sky.hour, sky.sun_altitude);
            (c, lux, sun_dir)
        }
    };

    // iter_mut over single_mut throughout: the InGame OnExit bulk-despawn can race
    // setup_world and leave duplicate/orphaned sun/moon entities; single_mut() then
    // returns Err and silently stops updating light, position and visibility.
    let sun_rgb_lin = {
        let c = sun_color.to_linear();
        Vec3::new(c.red, c.green, c.blue)
    };
    if sun_light_written.is_none_or(|(rgb, lux, dir, prev_indoors)| {
        rgb.distance(sun_rgb_lin) > CELESTIAL_COLOR_EPS
            || celestial_scalar_changed(lux, sun_lux)
            || dir.distance(sun_to_dir) > CELESTIAL_DIR_EPS
            || prev_indoors != indoors
    }) {
        for (mut light, mut xf) in q_sun.iter_mut() {
            light.color = sun_color;
            light.illuminance = sun_lux;
            light.shadow_maps_enabled = !indoors && sun_lux > 0.0;
            // A depth map used for self-shadowing must follow the illuminating light;
            // retail's ground-projected shadow direction is not an occlusion ray.
            *xf = Transform::from_translation(sun_to_dir * LIGHT_DISTANCE)
                .looking_at(Vec3::ZERO, Vec3::Y);
        }
        *sun_light_written = Some((sun_rgb_lin, sun_lux, sun_to_dir, indoors));
    }

    let moon_angle = (sky.hour / 24.0) * 2.0 * PI - PI / 2.0 + PI;
    let moon_dir = Vec3::new(moon_angle.cos(), moon_angle.sin(), 0.25).normalize();

    let moon_altitude = moon_dir.y.asin();
    sky.moon_altitude = moon_altitude;
    let moon_pos = moon_dir * LIGHT_DISTANCE;
    let (moon_color, moon_lux) = match dat {
        Some(_) if indoors => (Color::BLACK, 0.0),
        Some(rec) if sky.moon_altitude > 0.0 => {
            let d = rec.moonlight_diffuse_landscape;
            let (hue, k) = diffuse_to_light([d[0], d[1], d[2]]);
            (Color::linear_rgb(hue.x, hue.y, hue.z), k * DAT_DIR_REF_LUX)
        }
        Some(_) => (Color::BLACK, 0.0),
        None => moon_color_for_phase(sky.moon_illumination, sky.moon_altitude),
    };
    let moon_rgb_lin = {
        let c = moon_color.to_linear();
        Vec3::new(c.red, c.green, c.blue)
    };
    if moon_light_written.is_none_or(|(rgb, lux, dir, prev_indoors)| {
        rgb.distance(moon_rgb_lin) > CELESTIAL_COLOR_EPS
            || celestial_scalar_changed(lux, moon_lux)
            || dir.distance(moon_dir) > CELESTIAL_DIR_EPS
            || prev_indoors != indoors
    }) {
        for (mut light, mut xf) in q_moon.iter_mut() {
            light.color = moon_color;
            light.illuminance = moon_lux;
            light.shadow_maps_enabled = !indoors && moon_lux > 0.0;
            *xf = Transform::from_translation(moon_pos).looking_at(Vec3::ZERO, Vec3::Y);
        }
        *moon_light_written = Some((moon_rgb_lin, moon_lux, moon_dir, indoors));
    }

    // Publish the entity(model) + landscape(terrain) split for the actor- and
    // zone-material lighting consumers. The model light is a single moon<->sun
    // blend (research/xim EnvironmentSection.kt modelLightMix); landscape feeds both
    // sun(dir0) and moon(dir1) slots from the terrain block.
    //
    // The terrain half reads the record of the AREA the player stands in, the way
    // ZoneRenderer.cpp ZoneRenderer::RenderChunk2 lights each block from `positionedBlock->Area`; one
    // global light set here means the player's area stands in for the blocks around
    // them, the same approximation the distance fog already makes. The entity half
    // stays zone-wide: retail resolves it per actor from that actor's own area
    // (CMoElem.cpp CMoElem::PrepDX `FindAreaByFourCCAndGetWeatherDiffuseLights`), which one shared
    // model light cannot express.
    let sun_up = sky.sun_altitude > 0.0;
    let moon_up = sky.moon_altitude > 0.0;
    let zone_land = dat.map(|rec| landscape_lighting(&rec, sun_dir, moon_dir, sun_up, moon_up));
    let land = render_cfg
        .zone_weather
        .area_current
        .map(|rec| landscape_lighting(&rec, sun_dir, moon_dir, sun_up, moon_up))
        .or(zone_land);
    let zone_sun_k = zone_land.map_or(0.0, |z| z.sun_k);

    if let Some((rec, land)) = dat.zip(land).filter(|(r, _)| r.indoors) {
        // research/xim EnvironmentSection.kt getLightingParams: the model block collapses to one
        // static indoor diffuse — direction from the block's moon-color bytes, color
        // from its sun diffuse, no time gating.
        let model_dir = ffxi_dir_to_bevy(rec.indoor_light_dir_entity);
        let model_rgb = Vec3::new(
            rec.sunlight_diffuse_entity[0],
            rec.sunlight_diffuse_entity[1],
            rec.sunlight_diffuse_entity[2],
        )
        .max(Vec3::ZERO);
        let model_k = model_rgb.x.max(model_rgb.y).max(model_rgb.z).max(0.0);
        let model_color = if model_k > 1e-4 {
            model_rgb / model_k
        } else {
            Vec3::ZERO
        };

        *render_cfg.zone_lighting = crate::weather::ZoneDirectionalLighting {
            valid: true,
            indoors: true,
            model_dir,
            model_color,
            model_k: if model_dir == Vec3::ZERO {
                0.0
            } else {
                model_k
            },
            ambient_entity: Vec3::new(
                rec.ambient_entity[0],
                rec.ambient_entity[1],
                rec.ambient_entity[2],
            ),
            sun_dir: land.sun_dir,
            sun_color: land.sun_color,
            sun_k: land.sun_k,
            moon_dir: land.moon_dir,
            moon_color: land.moon_color,
            moon_k: land.moon_k,
            ambient_landscape: land.ambient,
            zone_sun_k,
        };
    } else if let Some((rec, land)) = dat.zip(land) {
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
            sun_dir: land.sun_dir,
            sun_color: land.sun_color,
            sun_k: land.sun_k,
            moon_dir: land.moon_dir,
            moon_color: land.moon_color,
            moon_k: land.moon_k,
            ambient_landscape: land.ambient,
            zone_sun_k,
        };
    } else {
        render_cfg.zone_lighting.valid = false;
    }

    let cam_pos = q_cam.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);

    // The zone DAT's own Sun/Moon billboards are retail's celestial bodies; where they run,
    // these hand-authored primitives would draw a second sun and moon on top of them.
    let dat_celestials = render_cfg.dat_celestials.active;
    let sun_visible = sky.sun_altitude > -0.05 && !dat_celestials;
    // No shipped zone DAT carries a sun sprite sheet (surveyed over all 298 resolvable zone
    // DATs, kuluu-nykm): retail's sun art is the Sun-attached StaticMesh generators sun0/sun1
    // (mesh `suns`/`sun2`, pinned by ffxi-dat particle_gen::tests
    // ::real_dat_sun_is_a_sun_attached_static_mesh_not_a_sprite_sheet) plus the lf0x
    // screen-space flare chain, so this disc is always the procedural sphere.
    for (mut disc, mut vis, _) in q_sun_disc.iter_mut() {
        disc.translation = cam_pos + sun_dir * SKY_RADIUS;
        disc.scale = Vec3::splat(SUN_DISC_RADIUS);
        *vis = if sun_visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }

    let moon_visible = sky.moon_altitude > 0.0 && sky.moon_illumination > 0.02;
    let disc_shown = moon_visible && !dat_celestials;
    let moon_world = cam_pos + moon_dir * SKY_RADIUS;
    let disc_count = q_moon_disc.iter().count();
    for (mut disc, mut vis) in q_moon_disc.iter_mut() {
        disc.translation = moon_world;
        disc.scale = Vec3::splat(MOON_DISC_EDGE);
        face_camera(&mut disc, cam_pos);
        *vis = if disc_shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    if *prev_disc_shown != Some(disc_shown) {
        info!(
            hour = sky.hour,
            moon_altitude = sky.moon_altitude,
            moon_illumination = sky.moon_illumination,
            disc_y = moon_world.y - cam_pos.y,
            sprite_loaded = render_cfg.moon_sprite.0.is_some(),
            shown = disc_shown,
            disc_count,
            "moon disc visibility"
        );
        *prev_disc_shown = Some(disc_shown);
    }

    // Perf A/B diagnostic: unconditional per-frame get_mut() on these two
    // StandardMaterials fires AssetEvent::Modified every frame (std churn in
    // the perf HUD). Set FFXI_FREEZE_CELESTIAL_MAT=1 to skip the writes.
    let freeze_celestial_mat = std::env::var_os("FFXI_FREEZE_CELESTIAL_MAT").is_some();

    if let Some(handles) = materials_handle
        .as_deref()
        .filter(|_| !freeze_celestial_mat)
    {
        {
            let visible = sky.sun_altitude.max(-0.2);

            let elev_norm = (visible / (PI / 2.0)).clamp(0.0, 1.0);
            let intensity = if visible > 0.0 {
                8.0 + 14.0 * elev_norm
            } else {
                (1.0 + 5.0 * (visible + 0.2) / 0.2).max(0.0)
            }
            .min(SUN_DISC_MAX_INTENSITY);
            let c = sun_color.to_linear();
            let rgb = Vec3::new(
                c.red * intensity,
                c.green * intensity * 0.95,
                c.blue * intensity * 0.75,
            );
            let id = handles.sun.id();
            if sun_disc_written.is_none_or(|(prev_id, prev_rgb)| {
                prev_id != id
                    || prev_rgb.distance(rgb) > CELESTIAL_COLOR_EPS * rgb.length().max(1.0)
            }) {
                if let Some(mut sun_mat) = materials.get_mut(&handles.sun) {
                    sun_mat.base_color = Color::linear_rgb(rgb.x, rgb.y, rgb.z);
                    *sun_disc_written = Some((id, rgb));
                }
            }
        }
        {
            let visibility = sky.moon_illumination.clamp(0.0, 1.0);

            let intensity = if sky.moon_altitude > 0.0 {
                0.6 + 1.4 * visibility
            } else {
                0.0
            };

            let total_v_days = vana_day_index(&vana_clock);
            let tint = celestial_moon_tint(&render_cfg.color_tables, total_v_days, sky.moon_phase);

            let frame_uv = render_cfg
                .moon_sprite
                .0
                .map(|frames| frames[moon_phase_frame(sky.moon_phase)]);
            let mode = if frame_uv.is_some() { 2.0 } else { 0.0 };
            let tint_v = Vec4::new(tint[0], tint[1], tint[2], mode);
            let params_v = Vec4::new(
                sky.moon_illumination,
                if sky.moon_waxing { 1.0 } else { -1.0 },
                intensity,
                MOON_PARAMS_PAD,
            );
            let id = handles.moon.id();
            if moon_disc_written.is_none_or(|(prev_id, prev_tint, prev_params, prev_frame)| {
                prev_id != id
                    || prev_tint.distance(tint_v) > CELESTIAL_COLOR_EPS
                    || prev_params.distance(params_v) > 1e-3
                    || prev_frame != frame_uv
            }) {
                if let Some(mut moon_mat) = moon_materials.get_mut(&handles.moon) {
                    if let Some(f) = frame_uv {
                        moon_mat.data.frame_uv = f;
                    }
                    moon_mat.data.tint = tint_v;
                    moon_mat.data.params = params_v;
                    *moon_disc_written = Some((id, tint_v, params_v, frame_uv));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ffxi_dat::weather::WeatherRecord;

    use super::*;
    use crate::vana_time::EARTH_SECS_PER_VANA_DAY;

    #[test]
    fn celestial_transition_toasts_are_devhud_only() {
        let toasts = [
            (sun_transition_toast(true), "☀ Sunrise"),
            (sun_transition_toast(false), "☀ Sunset"),
            (moon_transition_toast(true), "☾ Moonrise"),
            (moon_transition_toast(false), "☾ Moonset"),
            (
                moon_phase_toast(0, 1.0, "Firesday"),
                "☾ Moon: Full (100% illuminated) — Firesday",
            ),
        ];
        for (toast, want_text) in toasts {
            assert_eq!(toast.line.text, want_text);
            assert!(
                !crate::snapshot::chat_line_visible(toast.line.channel, false),
                "{want_text} must stay out of player chat"
            );
            assert!(crate::snapshot::chat_line_visible(toast.line.channel, true));
        }
    }

    // research/xim EnvironmentSection.kt modelLightMix: pure moon before 355, ramp to pure
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

    // The celestial discs are single-sided `Rectangle` quads under an unconditional
    // `cull_mode: Some(Face::Back)`, so an orientation that lands the quad's -Z on the viewer
    // draws nothing at all. Assert the face normal, not just "some rotation happened".
    #[test]
    fn a_billboard_shows_its_textured_face_to_the_camera() {
        let cam = Vec3::new(120.0, 30.0, -45.0);
        for dir in [Vec3::Y, Vec3::X, Vec3::NEG_Z, Vec3::new(0.6, 0.7, -0.4)] {
            let mut disc = Transform::from_translation(cam + dir.normalize() * SKY_RADIUS);
            face_camera(&mut disc, cam);
            let to_cam = (cam - disc.translation).normalize();
            let quad_normal = disc.rotation * Vec3::Z;
            assert!(
                quad_normal.dot(to_cam) > 0.999,
                "quad normal {quad_normal} faces away from the camera ({to_cam})"
            );
        }
    }

    // The outdoor terrain lights are horizon-gated, so every test that asserts on
    // them anchors the clock instead of letting `VanaClock::default()` read the
    // wall clock.
    const NOON_VANA_HOUR: f32 = 12.0;

    #[test]
    fn celestial_shadow_maps_follow_the_active_light_through_day_night_and_indoors() {
        const MORNING_HOUR: f32 = 7.0;
        const NIGHT_HOUR: f32 = 2.0;
        const DIFFUSE: [f32; 4] = [0.6, 0.6, 0.6, 1.0];
        const AMBIENT: [f32; 4] = [0.2, 0.2, 0.2, 1.0];
        const DIRECTION_EPSILON: f32 = 1e-4;
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<StandardMaterial>()
            .init_asset::<crate::moon_material::MoonMaterial>()
            .add_message::<crate::snapshot::ToastEvent>()
            .init_resource::<VanaSky>()
            .init_resource::<crate::graphics_settings::GraphicsSettings>()
            .init_resource::<crate::moon_material::MoonSpriteFrames>()
            .init_resource::<crate::moon_material::CelestialColorTables>()
            .init_resource::<crate::weather::ZoneWeather>()
            .init_resource::<crate::weather::ZoneDirectionalLighting>()
            .init_resource::<DatCelestials>()
            .add_systems(Update, sun_moon_system);
        let sun = app
            .world_mut()
            .spawn((IsSun, DirectionalLight::default(), Transform::default()))
            .id();
        let moon = app
            .world_mut()
            .spawn((IsMoon, DirectionalLight::default(), Transform::default()))
            .id();
        for (hour, indoors) in [
            (MORNING_HOUR, false),
            (NIGHT_HOUR, false),
            (NIGHT_HOUR, true),
            (NIGHT_HOUR, false),
            (NOON_VANA_HOUR, false),
        ] {
            let mut rec = terrain_rec(DIFFUSE, DIFFUSE, AMBIENT);
            rec.sunlight_diffuse_entity = DIFFUSE;
            rec.moonlight_diffuse_entity = DIFFUSE;
            rec.indoors = indoors;
            app.world_mut()
                .resource_mut::<crate::weather::ZoneWeather>()
                .current = Some(rec);
            app.insert_resource(crate::vana_time::VanaClock::anchored_at_hour(hour));
            app.update();
            let sky = app.world().resource::<VanaSky>();
            let sun_light = app.world().get::<DirectionalLight>(sun).unwrap();
            let moon_light = app.world().get::<DirectionalLight>(moon).unwrap();
            assert_eq!(
                sun_light.shadow_maps_enabled,
                !indoors && sky.sun_altitude > 0.0
            );
            assert_eq!(
                moon_light.shadow_maps_enabled,
                !indoors && sky.moon_altitude > 0.0
            );
            if !indoors {
                let model_dir = app
                    .world()
                    .resource::<crate::weather::ZoneDirectionalLighting>()
                    .model_dir;
                let active = if sun_light.shadow_maps_enabled {
                    sun
                } else {
                    moon
                };
                let to_light = app.world().get::<Transform>(active).unwrap().back();
                assert!(
                    to_light.distance(model_dir) < DIRECTION_EPSILON,
                    "hour {hour}: model light {model_dir:?} and shadow ray {to_light:?} disagree"
                );
            }
        }
    }

    fn terrain_rec(sun: [f32; 4], moon: [f32; 4], ambient: [f32; 4]) -> WeatherRecord {
        WeatherRecord {
            sunlight_diffuse_landscape: sun,
            moonlight_diffuse_landscape: moon,
            ambient_landscape: ambient,
            ..Default::default()
        }
    }

    // ZoneRenderer.cpp ZoneRenderer::RenderChunk2 — `positionedBlock->Area->GetWeatherDiffuseLights`
    // (XiArea.cpp XiArea::GetWeatherDiffuseLights) and `GetAmbient(_, 0)` (XiArea.cpp XiArea::GetAmbient) are the block's
    // AREA's env2 palette, so the terrain lights must move when the area does.
    #[test]
    fn landscape_lighting_reads_the_terrain_block_it_is_given() {
        let sun_dir = Vec3::Y;
        let moon_dir = Vec3::NEG_Y;

        let zone = terrain_rec(
            [0.9, 0.85, 0.7, 1.0],
            [0.2, 0.2, 0.4, 1.0],
            [0.6, 0.6, 0.6, 1.0],
        );
        let area = terrain_rec(
            [0.3, 0.1, 0.05, 1.0],
            [0.1, 0.0, 0.0, 1.0],
            [0.12, 0.08, 0.05, 1.0],
        );

        let zone_lit = landscape_lighting(&zone, sun_dir, moon_dir, true, true);
        let area_lit = landscape_lighting(&area, sun_dir, moon_dir, true, true);

        assert!((zone_lit.sun_k - 0.9).abs() < 1e-6);
        assert!((area_lit.sun_k - 0.3).abs() < 1e-6);
        assert!((area_lit.sun_color * area_lit.sun_k).distance(Vec3::new(0.3, 0.1, 0.05)) < 1e-6);
        assert!((area_lit.moon_color * area_lit.moon_k).distance(Vec3::new(0.1, 0.0, 0.0)) < 1e-6);
        assert!(area_lit.ambient.distance(Vec3::new(0.12, 0.08, 0.05)) < 1e-6);
        assert!(
            area_lit.ambient.length() < zone_lit.ambient.length(),
            "the darker area record must darken terrain ambient"
        );
    }

    // xim EnvironmentSection.kt getLightingParams — indoors the moon slot holds a signed direction,
    // not a color, so the indoor/outdoor split has to follow the record the terrain is
    // read from. Taking the zone's flag while reading an interior area's block would
    // publish those direction bytes as a moon color.
    #[test]
    fn landscape_lighting_takes_the_indoor_flag_from_its_own_record() {
        let sun_dir = Vec3::Y;
        let moon_dir = Vec3::NEG_Y;
        let mut indoor = terrain_rec([0.4, 0.4, 0.45, 1.0], [0.9, 0.0, 0.0, 1.0], [0.1; 4]);
        indoor.indoors = true;
        indoor.indoor_light_dir_landscape = [0.0, 1.0, 0.0];

        let lit = landscape_lighting(&indoor, sun_dir, moon_dir, true, true);
        assert_eq!(lit.moon_color, Vec3::ZERO);
        assert_eq!(lit.moon_k, 0.0);
        assert_eq!(lit.sun_dir, ffxi_dir_to_bevy([0.0, 1.0, 0.0]));
        assert!((lit.sun_k - 0.45).abs() < 1e-6);
    }

    // The celestial arc still gates the outdoor lights: a light below the horizon
    // contributes nothing, whatever area the player stands in.
    #[test]
    fn landscape_lighting_gates_outdoor_lights_on_the_horizon() {
        let rec = terrain_rec([0.9, 0.9, 0.9, 1.0], [0.3, 0.3, 0.5, 1.0], [0.2; 4]);
        let lit = landscape_lighting(&rec, Vec3::Y, Vec3::NEG_Y, false, true);
        assert_eq!(lit.sun_k, 0.0);
        assert!(lit.moon_k > 0.0);
    }

    // ZoneRenderer.cpp ZoneRenderer::RenderChunk2 resolves the block's lights through
    // `positionedBlock->Area`, so the published terrain lighting has to move when the
    // player's area does while the entity(model) half stays on the zone record
    // (CMoElem.cpp CMoElem::PrepDX resolves that one per actor, which one shared light cannot
    // express). This pins the wiring, not just `landscape_lighting`.
    #[test]
    fn published_terrain_lighting_follows_the_players_area() {
        const ZONE_SUN: [f32; 4] = [0.9, 0.88, 0.8, 1.0];
        const AREA_SUN: [f32; 4] = [0.25, 0.1, 0.05, 1.0];
        const ZONE_AMBIENT: [f32; 4] = [0.7, 0.7, 0.68, 1.0];
        const AREA_AMBIENT: [f32; 4] = [0.09, 0.06, 0.11, 1.0];
        const ENTITY_SUN: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<StandardMaterial>()
            .init_asset::<crate::moon_material::MoonMaterial>()
            .add_message::<crate::snapshot::ToastEvent>()
            .init_resource::<VanaSky>()
            .insert_resource(crate::vana_time::VanaClock::anchored_at_hour(
                NOON_VANA_HOUR,
            ))
            .init_resource::<crate::graphics_settings::GraphicsSettings>()
            .init_resource::<crate::moon_material::MoonSpriteFrames>()
            .init_resource::<crate::moon_material::CelestialColorTables>()
            .init_resource::<crate::weather::ZoneWeather>()
            .init_resource::<crate::weather::ZoneDirectionalLighting>()
            .init_resource::<DatCelestials>()
            .add_systems(Update, sun_moon_system);

        let mut zone = terrain_rec(ZONE_SUN, [0.2, 0.2, 0.4, 1.0], ZONE_AMBIENT);
        zone.sunlight_diffuse_entity = ENTITY_SUN;
        app.world_mut()
            .resource_mut::<crate::weather::ZoneWeather>()
            .current = Some(zone);
        app.update();

        let zone_lit = *app
            .world()
            .resource::<crate::weather::ZoneDirectionalLighting>();
        assert!(zone_lit.valid);
        assert!(
            zone_lit.ambient_landscape.distance(Vec3::new(
                ZONE_AMBIENT[0],
                ZONE_AMBIENT[1],
                ZONE_AMBIENT[2]
            )) < 1e-6
        );

        let mut area = zone;
        area.sunlight_diffuse_landscape = AREA_SUN;
        area.ambient_landscape = AREA_AMBIENT;
        app.world_mut()
            .resource_mut::<crate::weather::ZoneWeather>()
            .area_current = Some(area);
        app.update();

        let area_lit = *app
            .world()
            .resource::<crate::weather::ZoneDirectionalLighting>();
        assert!(
            area_lit.ambient_landscape.distance(Vec3::new(
                AREA_AMBIENT[0],
                AREA_AMBIENT[1],
                AREA_AMBIENT[2]
            )) < 1e-6,
            "terrain ambient stayed on the zone record: {}",
            area_lit.ambient_landscape
        );
        assert!(
            (area_lit.sun_color * area_lit.sun_k).distance(Vec3::new(
                AREA_SUN[0],
                AREA_SUN[1],
                AREA_SUN[2]
            )) < 1e-6,
            "terrain sun diffuse stayed on the zone record"
        );
        assert_eq!(
            area_lit.ambient_entity, zone_lit.ambient_entity,
            "the entity half is resolved per actor in retail and must stay zone-wide here"
        );
        assert_eq!(area_lit.model_color, zone_lit.model_color);
        assert_eq!(area_lit.model_k, zone_lit.model_k);
        assert_eq!(
            area_lit.zone_sun_k, zone_lit.zone_sun_k,
            "the whole-zone lamp gate must stay on the zone record"
        );
    }

    // `lamp_lit_factor` treats a black daytime sun diffuse as "covered zone, lamps
    // burn all day" and switches EVERY Generator light in the zone together, so it
    // reads `zone_sun_k` rather than the area-resolved `sun_k`; standing in a
    // sunless interior area must not light the lamps three streets away
    // (zone_point_lights::tests::lamps_stay_out_when_only_the_players_area_is_sunless
    // pins the consumer).
    #[test]
    fn zone_sun_k_stays_on_the_zone_record_inside_a_sunless_area() {
        const ZONE_SUN: [f32; 4] = [0.9, 0.88, 0.8, 1.0];
        const SUNLESS: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<StandardMaterial>()
            .init_asset::<crate::moon_material::MoonMaterial>()
            .add_message::<crate::snapshot::ToastEvent>()
            .init_resource::<VanaSky>()
            .insert_resource(crate::vana_time::VanaClock::anchored_at_hour(
                NOON_VANA_HOUR,
            ))
            .init_resource::<crate::graphics_settings::GraphicsSettings>()
            .init_resource::<crate::moon_material::MoonSpriteFrames>()
            .init_resource::<crate::moon_material::CelestialColorTables>()
            .init_resource::<crate::weather::ZoneWeather>()
            .init_resource::<crate::weather::ZoneDirectionalLighting>()
            .init_resource::<DatCelestials>()
            .add_systems(Update, sun_moon_system);

        let zone = terrain_rec(ZONE_SUN, [0.2, 0.2, 0.4, 1.0], [0.7; 4]);
        let mut area = zone;
        area.sunlight_diffuse_landscape = SUNLESS;
        {
            let mut weather = app
                .world_mut()
                .resource_mut::<crate::weather::ZoneWeather>();
            weather.current = Some(zone);
            weather.area_current = Some(area);
        }
        app.update();

        let lit = *app
            .world()
            .resource::<crate::weather::ZoneDirectionalLighting>();
        assert_eq!(lit.sun_k, 0.0, "terrain sun follows the sunless area");
        assert!(
            (lit.zone_sun_k - ZONE_SUN[0]).abs() < 1e-6,
            "the whole-zone lamp gate must still see the zone's daylight sun: {}",
            lit.zone_sun_k
        );
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

    // research/xim EnvironmentManager.kt getSunPosition — the arc is a unit circle in the FFXI XY
    // plane. An earlier revision carried a +0.25 z tilt, which pushed the sun (and the
    // lens flare and the Sun-attached weather generators, which all read this) off retail's
    // path and out of the plane the moon shares.
    #[test]
    fn sun_arc_is_the_untilted_retail_circle() {
        for hour in 0..24 {
            let d = sun_direction(hour as f32);
            assert!(d.z.abs() < 1e-6, "hour {hour}: retail's arc has no z tilt");
            assert!(
                (d.length() - 1.0).abs() < 1e-6,
                "hour {hour}: not unit length"
            );
        }
        // Midnight below, noon overhead, and the east->west swing through the horizon.
        assert!(sun_direction(0.0).distance(Vec3::NEG_Y) < 1e-6);
        assert!(sun_direction(12.0).distance(Vec3::Y) < 1e-6);
        assert!(sun_direction(6.0).distance(Vec3::X) < 1e-6);
        assert!(sun_direction(18.0).distance(Vec3::NEG_X) < 1e-6);
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

    #[test]
    fn shadow_direction_never_rakes_below_retails_minimum_elevation() {
        // research/XIClient ShadowRenderer.cpp ShadowRenderer::Init rewrites any light shallower than
        // ANGLE_PI_OVER_3 to sit at atan(sin(ANGLE_PI_OVER_3)) ~= 40.9 degrees, in either
        // hemisphere.
        for hour in 0..24 {
            let to_sun = sun_direction(hour as f32);
            let clamped = shadow_cast_direction(to_sun);
            let horizontal = Vec2::new(clamped.x, clamped.z).length();
            let elevation = clamped.y.atan2(horizontal);
            assert!(
                elevation >= shadow_min_elevation() - 1e-4,
                "hour {hour}: shadow elevation {elevation} below the retail minimum"
            );
            assert!(
                (clamped.length() - 1.0).abs() < 1e-4,
                "hour {hour}: not unit"
            );
        }
    }

    #[test]
    fn a_steep_sun_keeps_its_own_direction() {
        let noon = sun_direction(12.0);
        assert!(noon.y > 0.9, "noon sun should be near-overhead: {noon}");
        assert_eq!(shadow_cast_direction(noon), noon);
    }

    // The predicate and the assignment are two different angles, so a sun already steeper
    // than the 40.9-degree landing but shallower than the 60-degree test is FLATTENED. A
    // one-sided clamp would leave hour 9.9 alone and draw shadows ~40% short.
    #[test]
    fn a_sun_inside_the_rewrite_band_is_snapped_down_to_the_landing_elevation() {
        let mid = sun_direction(9.9);
        let horizontal = Vec2::new(mid.x, mid.z).length();
        let before = mid.y.atan2(horizontal);
        assert!(
            (shadow_min_elevation()..SHADOW_ELEVATION_SIN_ARG).contains(&before),
            "hour 9.9 should sit inside the rewrite band, got {before}"
        );
        let after = shadow_cast_direction(mid);
        let after_elev = after.y.atan2(Vec2::new(after.x, after.z).length());
        assert!(
            (after_elev - shadow_min_elevation()).abs() < 1e-4,
            "expected the landing elevation, got {after_elev}"
        );
    }

    #[test]
    fn the_lit_sun_direction_is_untouched_by_the_shadow_clamp() {
        // The clamp must not be folded into sun_direction itself — the 0x2F lit
        // direction and the cast direction are different vectors at dawn/dusk.
        let dawn = sun_direction(6.5);
        assert_ne!(shadow_cast_direction(dawn), dawn);
    }
}
