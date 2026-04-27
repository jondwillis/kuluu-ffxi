use bevy::app::AppExit;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Capturing, Screenshot};
use ffxi_viewer_core::dat_mmb::LoadMmbRequest;
use ffxi_viewer_core::dat_mzb::{
    kick_load_mzb_tasks, poll_load_mzb_tasks, DrawDistance, LoadMzbInFlight, LoadMzbRequest,
    MzbCollisionGeometry, ZoneGeomCache, ZoneGeomMode,
};
use ffxi_viewer_core::SceneState;
use std::env;

#[derive(Resource, Clone)]
struct RenderParams {
    zone_or_file_id: u32,
    camera_pos: Vec3,
    camera_target: Vec3,
    output: String,
    capture_frame: u32,
}

#[derive(Resource, Default)]
struct FrameCounter(u32);

#[derive(Resource, Default)]
struct CaptureState {
    spawned: bool,
}

fn main() {
    let params = parse_args();
    eprintln!(
        "mzb-render-headless: zone_or_file_id={} cam=({:.1}, {:.1}, {:.1}) -> ({:.1}, {:.1}, {:.1}) out={} capture_frame={}",
        params.zone_or_file_id,
        params.camera_pos.x,
        params.camera_pos.y,
        params.camera_pos.z,
        params.camera_target.x,
        params.camera_target.y,
        params.camera_target.z,
        params.output,
        params.capture_frame
    );
    if let Ok(s) = env::var("FFXI_MATERIAL_HIGHLIGHT") {
        eprintln!("  FFXI_MATERIAL_HIGHLIGHT={s} (isolating one material)");
    }
    if let Ok(s) = env::var("FFXI_MATERIAL_PALETTE") {
        eprintln!("  FFXI_MATERIAL_PALETTE={s}");
    }

    App::new()
        .insert_resource(params)
        .init_resource::<FrameCounter>()
        .init_resource::<CaptureState>()
        .insert_resource(ClearColor(Color::srgb(0.08, 0.08, 0.10)))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "mzb-render-headless".into(),
                resolution: (1280u32, 720u32).into(),
                ..default()
            }),
            ..default()
        }))
        .add_message::<LoadMzbRequest>()
        .add_message::<LoadMmbRequest>()
        .init_resource::<DrawDistance>()
        .init_resource::<MzbCollisionGeometry>()
        .init_resource::<LoadMzbInFlight>()
        .init_resource::<ZoneGeomCache>()
        .init_resource::<SceneState>()
        .add_systems(Startup, (setup_camera, fire_load_request))
        .add_systems(
            Update,
            (kick_load_mzb_tasks, poll_load_mzb_tasks, capture_and_exit).chain(),
        )
        .run();
}

fn parse_args() -> RenderParams {
    let mut p = RenderParams {
        zone_or_file_id: 234,
        camera_pos: Vec3::new(0.0, 100.0, 0.0),
        camera_target: Vec3::new(0.0, 0.0, -150.0),
        output: "mzb-render.png".to_string(),
        capture_frame: 120,
    };
    let args: Vec<String> = env::args().collect();
    let mut i = 1usize;
    while i < args.len() {
        let take_f32 = |i: usize| {
            args.get(i + 1)
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or_else(|| {
                    eprintln!("missing/invalid f32 for {}", args[i]);
                    std::process::exit(2);
                })
        };
        let take_u32 = |i: usize| {
            args.get(i + 1)
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or_else(|| {
                    eprintln!("missing/invalid u32 for {}", args[i]);
                    std::process::exit(2);
                })
        };
        let take_str = |i: usize| {
            args.get(i + 1).cloned().unwrap_or_else(|| {
                eprintln!("missing value for {}", args[i]);
                std::process::exit(2);
            })
        };
        match args[i].as_str() {
            "--zone" => {
                p.zone_or_file_id = take_u32(i);
                i += 2;
            }
            "--cx" => {
                p.camera_pos.x = take_f32(i);
                i += 2;
            }
            "--cy" => {
                p.camera_pos.y = take_f32(i);
                i += 2;
            }
            "--cz" => {
                p.camera_pos.z = take_f32(i);
                i += 2;
            }
            "--tx" => {
                p.camera_target.x = take_f32(i);
                i += 2;
            }
            "--ty" => {
                p.camera_target.y = take_f32(i);
                i += 2;
            }
            "--tz" => {
                p.camera_target.z = take_f32(i);
                i += 2;
            }
            "--out" => {
                p.output = take_str(i);
                i += 2;
            }
            "--capture-frame" => {
                p.capture_frame = take_u32(i);
                i += 2;
            }
            "--help" | "-h" => {
                eprintln!(
                    "usage: mzb-render-headless [--zone N] [--cx --cy --cz X Y Z] \
                     [--tx --ty --tz X Y Z] [--out PATH] [--capture-frame N]"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(2);
            }
        }
    }
    p
}

fn setup_camera(mut commands: Commands, params: Res<RenderParams>, mut draw: ResMut<DrawDistance>) {
    draw.zone_geom_mode = ZoneGeomMode::All;

    draw.world = 10_000.0;
    draw.mob = 10_000.0;

    commands.spawn((
        Camera3d::default(),
        Transform::from_translation(params.camera_pos).looking_at(params.camera_target, Vec3::Y),
    ));

    commands.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 200.0,
        ..default()
    });
}

fn fire_load_request(mut tx: MessageWriter<LoadMzbRequest>, params: Res<RenderParams>) {
    let zone_or_file_id = params.zone_or_file_id;
    let zone_id_u16 = u16::try_from(zone_or_file_id).ok();
    let file_id = match zone_id_u16.and_then(ffxi_dat::zone_dat::zone_id_to_mzb_file_id) {
        Some(fid) => {
            eprintln!("  zone_id {zone_or_file_id} -> mzb file_id {fid}");
            fid
        }
        None => {
            eprintln!("  zone_id {zone_or_file_id} has no MZB mapping; using as file_id directly");
            zone_or_file_id
        }
    };
    tx.write(LoadMzbRequest {
        file_id,
        chunk_idx: None,
        world_pos: Vec3::ZERO,
        auto_loaded: false,
    });
}

fn capture_and_exit(
    mut commands: Commands,
    mut frame: ResMut<FrameCounter>,
    mut capture: ResMut<CaptureState>,
    params: Res<RenderParams>,
    capturing_q: Query<Entity, With<Capturing>>,
    mut exit: MessageWriter<AppExit>,
) {
    frame.0 += 1;

    if !capture.spawned && frame.0 >= params.capture_frame {
        eprintln!(
            "mzb-render-headless: capturing at frame {} -> {}",
            frame.0, params.output
        );
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(params.output.clone()));
        capture.spawned = true;
    }

    if capture.spawned && capturing_q.is_empty() && frame.0 >= params.capture_frame + 5 {
        eprintln!("mzb-render-headless: done");
        exit.write(AppExit::Success);
    }
}
