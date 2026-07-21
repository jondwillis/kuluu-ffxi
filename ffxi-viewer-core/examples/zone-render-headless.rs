use bevy::app::AppExit;
use bevy::camera::ScalingMode;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Capturing, Screenshot};
use ffxi_dat::weather::collect_zone_weather_sets;
use ffxi_viewer_core::camera::OperatorCamera;
use ffxi_viewer_core::dat_mmb::{
    process_load_mmb_requests, LoadMmbRequest, MmbHandleCache, MmbLoadInFlight, MmbLoadQueue,
    MmbParseCache, MmbTexPools,
};
use ffxi_viewer_core::dat_mzb::{
    kick_load_mzb_tasks, poll_load_mzb_tasks, scroll_water_uv, spawn_zone_water, DrawDistance,
    LoadMzbInFlight, LoadMzbRequest, MzbCollisionGeometry, PendingWaterSpawns, ZoneGeomCache,
    ZoneGeomMode, ZoneWaterMaterial,
};
use ffxi_viewer_core::ffxi_zone_material::FfxiZoneMaterialPlugin;
use ffxi_viewer_core::scene::TrackedEntities;
use ffxi_viewer_core::snapshot::ToastEvent;
use ffxi_viewer_core::sun_moon::IsSun;
use ffxi_viewer_core::vana_time::VanaClock;
use ffxi_viewer_core::weather::{apply_zone_weather, sample_zone_weather, ZoneWeather};
use ffxi_viewer_core::SceneState;
use std::env;

#[derive(Resource, Clone)]
struct P {
    file_id: u32,
    out: String,
    cy: f32,
    cap: u32,
    mode: ZoneGeomMode,
    amb: f32,
    sun: f32,
    cam: Option<Vec3>,
    tgt: Vec3,
    fog: bool,
    hour: f32,
    // Reproduce the live client's sun exactly: sun_moon.rs bias values,
    // cascade_config_from_settings(High preset), 4096 shadow map, and the
    // hour-driven sun_direction() instead of the fixed debug sun position.
    client_sun: bool,
}
#[derive(Resource, Default)]
struct FC(u32);
#[derive(Resource, Default)]
struct CS(bool);
/// Off-screen render target; `Screenshot::primary_window()` returns black on
/// macOS when the window is never presented, so we capture this image instead.
#[derive(Resource)]
struct CapTarget(Handle<Image>);
fn main() {
    let a: Vec<String> = env::args().collect();
    let mut p = P {
        file_id: 216,
        out: "/tmp/zone_fix.png".into(),
        cy: 250.0,
        cap: 200,
        mode: ZoneGeomMode::Off,
        amb: 600.0,
        sun: 8000.0,
        cam: None,
        tgt: Vec3::ZERO,
        fog: false,
        hour: 12.0,
        client_sun: false,
    };
    let f3 = |a: &[String], i: usize| {
        Vec3::new(
            a[i + 1].parse().unwrap(),
            a[i + 2].parse().unwrap(),
            a[i + 3].parse().unwrap(),
        )
    };
    let mut i = 1;
    while i < a.len() {
        match a[i].as_str() {
            "--file" => {
                p.file_id = a[i + 1].parse().unwrap();
                i += 2;
            }
            "--out" => {
                p.out = a[i + 1].clone();
                i += 2;
            }
            "--cy" => {
                p.cy = a[i + 1].parse().unwrap();
                i += 2;
            }
            "--amb" => {
                p.amb = a[i + 1].parse().unwrap();
                i += 2;
            }
            "--sun" => {
                p.sun = a[i + 1].parse().unwrap();
                i += 2;
            }
            "--cam" => {
                p.cam = Some(f3(&a, i));
                i += 4;
            }
            "--tgt" => {
                p.tgt = f3(&a, i);
                i += 4;
            }
            "--cap" => {
                p.cap = a[i + 1].parse().unwrap();
                i += 2;
            }
            "--all" => {
                p.mode = ZoneGeomMode::All;
                i += 1;
            }
            "--fog" => {
                p.fog = true;
                i += 1;
            }
            "--hour" => {
                p.hour = a[i + 1].parse().unwrap();
                i += 2;
            }
            "--client-sun" => {
                p.client_sun = true;
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    let mut app = App::new();
    app.insert_resource(VanaClock::anchored_at_hour(p.hour))
        .insert_resource(p)
        .init_resource::<FC>()
        .init_resource::<CS>()
        .insert_resource(ClearColor(Color::BLACK))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                resolution: (1280u32, 1280u32).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(FfxiZoneMaterialPlugin)
        .add_message::<LoadMzbRequest>()
        .add_message::<LoadMmbRequest>()
        .add_message::<ToastEvent>()
        .init_resource::<DrawDistance>()
        .init_resource::<MzbCollisionGeometry>()
        .init_resource::<PendingWaterSpawns>()
        .init_resource::<ZoneWaterMaterial>()
        .init_resource::<ffxi_viewer_core::zone_point_lights::ActiveSceneLights>()
        .init_resource::<ffxi_viewer_core::graphics::settings::GraphicsSettings>()
        .init_resource::<ffxi_viewer_core::dat_mmb::MmbLoadInFlight>()
        .init_resource::<LoadMzbInFlight>()
        .init_resource::<ZoneGeomCache>()
        .init_resource::<MmbHandleCache>()
        .init_resource::<MmbLoadInFlight>()
        .init_resource::<MmbLoadQueue>()
        .init_resource::<MmbParseCache>()
        .init_resource::<MmbTexPools>()
        .init_resource::<TrackedEntities>()
        .init_resource::<SceneState>()
        .init_resource::<ZoneWeather>()
        .init_resource::<ffxi_viewer_core::weather_fx::CurrentWeather>()
        .init_resource::<ffxi_viewer_core::weather_fx::ActiveWeatherModifier>()
        .init_resource::<ffxi_viewer_core::weather::DefaultClearColor>()
        .add_systems(
            Update,
            (sample_zone_weather, apply_zone_weather)
                .chain()
                .run_if(|p: Res<P>| p.fog),
        )
        .add_systems(Startup, (setup, fire, load_weather))
        .add_systems(
            Update,
            (
                drain_toasts,
                kick_load_mzb_tasks,
                poll_load_mzb_tasks,
                spawn_zone_water,
                scroll_water_uv,
                process_load_mmb_requests,
                print_toasts,
                cap,
            )
                .chain(),
        );
    // spawn_zone_water requires the bevy_water extended-material assets;
    // without this plugin its param validation panics on any zone load.
    #[cfg(feature = "enhanced-water")]
    app.add_plugins(ffxi_viewer_core::water_enhanced::EnhancedWaterPlugin);
    app.run();
}
fn print_toasts(mut rx: MessageReader<ToastEvent>) {
    for t in rx.read() {
        println!("[toast] {}", t.line.text);
    }
}
fn setup(
    mut c: Commands,
    p: Res<P>,
    mut d: ResMut<DrawDistance>,
    mut images: ResMut<Assets<Image>>,
) {
    d.zone_geom_mode = p.mode;
    d.world = 1e5;
    d.mob = 1e5;
    // Off-screen render target (see CapTarget).
    let size = bevy::render::render_resource::Extent3d {
        width: 1280,
        height: 1280,
        depth_or_array_layers: 1,
    };
    let mut target = Image::new_fill(
        size,
        bevy::render::render_resource::TextureDimension::D2,
        &[0, 0, 0, 255],
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::default(),
    );
    target.texture_descriptor.usage = bevy::render::render_resource::TextureUsages::TEXTURE_BINDING
        | bevy::render::render_resource::TextureUsages::COPY_SRC
        | bevy::render::render_resource::TextureUsages::RENDER_ATTACHMENT;
    let target = images.add(target);
    c.insert_resource(CapTarget(target.clone()));
    let cam_target = bevy::camera::RenderTarget::Image(target.into());
    let cam = match p.cam {
        Some(pos) => c
            .spawn((
                Camera3d::default(),
                cam_target.clone(),
                Transform::from_translation(pos).looking_at(p.tgt, Vec3::Y),
            ))
            .id(),
        None => {
            let mut proj = OrthographicProjection::default_3d();
            proj.scaling_mode = ScalingMode::FixedVertical {
                viewport_height: 360.0,
            };
            c.spawn((
                Camera3d::default(),
                cam_target.clone(),
                Projection::Orthographic(proj),
                Transform::from_xyz(0., p.cy, 0.).looking_at(Vec3::new(0., 0., -0.001), Vec3::Z),
            ))
            .id()
        }
    };
    if p.fog {
        c.entity(cam).insert((
            OperatorCamera,
            bevy::camera::Hdr,
            bevy::light::VolumetricFog {
                step_count: 64,
                ambient_intensity: 0.03,
                ambient_color: Color::srgb(0.85, 0.88, 1.0),
                jitter: 0.0,
            },
        ));
        // Mirrors the client's zone fog volume (scene.rs); apply_zone_weather
        // retunes color/density from the zone weather record each frame.
        c.spawn((
            bevy::light::FogVolume {
                fog_color: Color::srgb(0.65, 0.72, 0.82),
                density_factor: 0.06,
                absorption: 0.25,
                scattering: 0.35,
                scattering_asymmetry: 0.7,
                light_tint: Color::srgb(1.0, 0.96, 0.88),
                light_intensity: 1.0,
                // Ground haze with vertical falloff so the sky stays visible;
                // see height_fog_density_texture in weather.rs.
                density_texture: Some(ffxi_viewer_core::weather::height_fog_density_texture(
                    &mut images,
                )),
                ..default()
            },
            Transform::from_xyz(0.0, ffxi_viewer_core::weather::FOG_VOLUME_CENTER_Y, 0.0)
                .with_scale(ffxi_viewer_core::weather::FOG_VOLUME_SCALE),
        ));
    }
    c.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: p.amb,
        ..default()
    });

    let sun = if p.client_sun {
        // Mirror sun_moon.rs exactly: same bias values, same cascade config
        // (High preset = default), 4096 map, hour-driven direction.
        let gfx = ffxi_viewer_core::graphics::settings::GraphicsSettings::default();
        c.insert_resource(bevy::light::DirectionalLightShadowMap {
            size: gfx.shadow_map_size as usize,
        });
        let sun_dir = ffxi_viewer_core::sun_moon::sun_direction(p.hour);
        c.spawn((
            IsSun,
            DirectionalLight {
                illuminance: p.sun,
                shadow_maps_enabled: true,
                shadow_depth_bias: 0.2,
                shadow_normal_bias: 0.6,
                ..default()
            },
            ffxi_viewer_core::graphics::settings::cascade_config_from_settings(&gfx),
            Transform::from_translation(sun_dir * 1000.0).looking_at(Vec3::ZERO, Vec3::Y),
        ))
        .id()
    } else {
        c.spawn((
            IsSun,
            DirectionalLight {
                illuminance: p.sun,
                shadow_maps_enabled: true,
                ..default()
            },
            Transform::from_xyz(300., 220., 120.).looking_at(Vec3::ZERO, Vec3::Y),
        ))
        .id()
    };
    if p.fog {
        c.entity(sun).insert(bevy::light::VolumetricLight);
    }
}
fn fire(mut tx: MessageWriter<LoadMzbRequest>, p: Res<P>) {
    tx.write(LoadMzbRequest {
        file_id: p.file_id,
        chunk_idx: None,
        world_pos: Vec3::ZERO,
        auto_loaded: false,
    });
}
// Headless mirror of weather::load_zone_weather: that system derives the zone
// from the live SceneState snapshot, but this example has no login flow — the
// zone comes straight from `P.file_id`, so read the same DAT bytes directly.
fn load_weather(p: Res<P>, mut zone_weather: ResMut<ZoneWeather>) {
    let Ok(root) = ffxi_dat::DatRoot::from_env_or_default() else {
        return;
    };
    let Ok(location) = root.resolve(p.file_id) else {
        return;
    };
    let path = location.path_under(root.root());
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    zone_weather.file_id = Some(p.file_id);
    zone_weather.sets = collect_zone_weather_sets(&bytes);
}
fn drain_toasts(mut rx: MessageReader<ToastEvent>) {
    for t in rx.read() {
        eprintln!("[toast] {}", t.line.text);
    }
}
#[allow(clippy::too_many_arguments)]
fn cap(
    mut c: Commands,
    mut f: ResMut<FC>,
    mut s: ResMut<CS>,
    p: Res<P>,
    q: Query<Entity, With<Capturing>>,
    mut e: MessageWriter<AppExit>,
    queue: Res<MmbLoadQueue>,
    water: Res<PendingWaterSpawns>,
    q_aabb: Query<(&bevy::camera::primitives::Aabb, &GlobalTransform)>,
    target: Res<CapTarget>,
) {
    f.0 += 1;
    if f.0.is_multiple_of(40) {
        eprintln!(
            "frame {} pending={} water_pending={}",
            f.0,
            queue.pending.len(),
            water.specs.len()
        );
    }
    if f.0 == 60 {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        let mut n = 0usize;
        for (aabb, gt) in q_aabb.iter() {
            let c = gt.transform_point(Vec3::from(aabb.center));
            min = min.min(c);
            max = max.max(c);
            n += 1;
        }
        eprintln!("mesh centers n={n} min={min:?} max={max:?}");
    }
    if !s.0 && f.0 >= p.cap {
        c.spawn(Screenshot::image(target.0.clone()))
            .observe(save_to_disk(p.out.clone()));
        s.0 = true;
        eprintln!("captured -> {}", p.out);
    }
    if s.0 && q.is_empty() && f.0 >= p.cap + 5 {
        e.write(AppExit::Success);
    }
}
