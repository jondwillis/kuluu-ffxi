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
}
#[derive(Resource, Default)]
struct FC(u32);
#[derive(Resource, Default)]
struct CS(bool);
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
            _ => {
                i += 1;
            }
        }
    }
    App::new()
        .insert_resource(VanaClock::anchored_at_hour(p.hour))
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
        )
        .run();
}
fn print_toasts(mut rx: MessageReader<ToastEvent>) {
    for t in rx.read() {
        println!("[toast] {}", t.line.text);
    }
}
fn setup(mut c: Commands, p: Res<P>, mut d: ResMut<DrawDistance>) {
    d.zone_geom_mode = p.mode;
    d.world = 1e5;
    d.mob = 1e5;
    let cam = match p.cam {
        Some(pos) => c
            .spawn((
                Camera3d::default(),
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
                ..default()
            },
            Transform::from_xyz(0.0, 200.0, 0.0).with_scale(Vec3::splat(2000.0)),
        ));
    }
    c.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: p.amb,
        ..default()
    });

    let sun = c
        .spawn((
            IsSun,
            DirectionalLight {
                illuminance: p.sun,
                shadow_maps_enabled: true,
                ..default()
            },
            Transform::from_xyz(300., 220., 120.).looking_at(Vec3::ZERO, Vec3::Y),
        ))
        .id();
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
    if !s.0 && f.0 >= p.cap {
        c.spawn(Screenshot::primary_window())
            .observe(save_to_disk(p.out.clone()));
        s.0 = true;
        eprintln!("captured -> {}", p.out);
    }
    if s.0 && q.is_empty() && f.0 >= p.cap + 5 {
        e.write(AppExit::Success);
    }
}
