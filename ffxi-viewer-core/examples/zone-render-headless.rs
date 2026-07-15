use bevy::app::AppExit;
use bevy::camera::ScalingMode;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Capturing, Screenshot};
use ffxi_viewer_core::dat_mmb::{
    process_load_mmb_requests, LoadMmbRequest, MmbHandleCache, MmbLoadInFlight, MmbLoadQueue,
    MmbParseCache, MmbTexPools,
};
use ffxi_viewer_core::dat_mzb::{
    kick_load_mzb_tasks, poll_load_mzb_tasks, scroll_zone_water, spawn_zone_water, DrawDistance,
    LoadMzbInFlight, LoadMzbRequest, MzbCollisionGeometry, PendingWaterSpawns, ZoneGeomCache,
    ZoneGeomMode, ZoneWaterScroll,
};
use ffxi_viewer_core::ffxi_zone_material::FfxiZoneMaterialPlugin;
use ffxi_viewer_core::graphics::GraphicsSettings;
use ffxi_viewer_core::scene::TrackedEntities;
use ffxi_viewer_core::snapshot::ToastEvent;
use ffxi_viewer_core::sun_moon::IsSun;
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
            _ => {
                i += 1;
            }
        }
    }
    App::new()
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
        .init_resource::<LoadMzbInFlight>()
        .init_resource::<ZoneGeomCache>()
        .init_resource::<MmbHandleCache>()
        .init_resource::<MmbLoadInFlight>()
        .init_resource::<MmbLoadQueue>()
        .init_resource::<MmbParseCache>()
        .init_resource::<MmbTexPools>()
        .init_resource::<TrackedEntities>()
        .init_resource::<SceneState>()
        .init_resource::<PendingWaterSpawns>()
        .init_resource::<ZoneWaterScroll>()
        .init_resource::<GraphicsSettings>()
        .add_systems(Startup, (setup, fire))
        .add_systems(
            Update,
            (
                drain_toasts,
                kick_load_mzb_tasks,
                poll_load_mzb_tasks,
                spawn_zone_water,
                scroll_zone_water,
                process_load_mmb_requests,
                cap,
            )
                .chain(),
        )
        .run();
}
fn setup(mut c: Commands, p: Res<P>, mut d: ResMut<DrawDistance>) {
    d.zone_geom_mode = p.mode;
    d.world = 1e5;
    d.mob = 1e5;
    match p.cam {
        Some(pos) => {
            c.spawn((
                Camera3d::default(),
                Transform::from_translation(pos).looking_at(p.tgt, Vec3::Y),
            ));
        }
        None => {
            let mut proj = OrthographicProjection::default_3d();
            proj.scaling_mode = ScalingMode::FixedVertical {
                viewport_height: 360.0,
            };
            c.spawn((
                Camera3d::default(),
                Projection::Orthographic(proj),
                Transform::from_xyz(0., p.cy, 0.).looking_at(Vec3::new(0., 0., -0.001), Vec3::Z),
            ));
        }
    }
    c.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: p.amb,
        ..default()
    });

    c.spawn((
        IsSun,
        DirectionalLight {
            illuminance: p.sun,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(300., 220., 120.).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}
fn fire(mut tx: MessageWriter<LoadMzbRequest>, p: Res<P>) {
    tx.write(LoadMzbRequest {
        file_id: p.file_id,
        chunk_idx: None,
        world_pos: Vec3::ZERO,
        auto_loaded: false,
    });
}
fn drain_toasts(mut rx: MessageReader<ToastEvent>) {
    for t in rx.read() {
        eprintln!("[toast] {}", t.line.text);
    }
}

fn cap(
    mut c: Commands,
    mut f: ResMut<FC>,
    mut s: ResMut<CS>,
    p: Res<P>,
    q: Query<Entity, With<Capturing>>,
    mut e: MessageWriter<AppExit>,
    queue: Res<MmbLoadQueue>,
) {
    f.0 += 1;
    if f.0.is_multiple_of(40) {
        eprintln!("frame {} pending={}", f.0, queue.pending.len());
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
