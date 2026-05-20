//! Headless MZB renderer for the empirical material-LUT learning workflow.
//!
//! Loads an MZB zone, spawns its grid-cell placement geometry, waits for
//! a few frames, captures a screenshot to disk, and exits. Reuses the
//! production `process_load_mzb_requests` system so the bake matches
//! exactly what the live client renders for the same zone — only the
//! palette is swappable for empirical work.
//!
//! Technically windowed (a brief Bevy window pops up). True headless via
//! `RenderTarget::Image` is more plumbing; deferred until the windowed
//! flow proves the workflow.
//!
//! Palette modes (env-driven, read by `mzb_palette_color` in `dat_mzb.rs`):
//!   * `FFXI_MATERIAL_HIGHLIGHT=N`  — paint material `N` red, rest dark gray
//!   * `FFXI_MATERIAL_PALETTE=hisat` — saturated 16-color rainbow
//!   * default                       — production muted palette
//!
//! Usage:
//!   FFXI_DAT_PATH=... \
//!   cargo run -p ffxi-viewer-core --example mzb-render-headless -- \
//!     --zone 234 --out bastok.png
//!
//!   FFXI_DAT_PATH=... FFXI_MATERIAL_HIGHLIGHT=3 \
//!   cargo run -p ffxi-viewer-core --example mzb-render-headless -- \
//!     --zone 234 --cx 50 --cy 80 --cz 50 --tx 0 --ty 0 --tz 0 \
//!     --out bastok-mat3.png

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Capturing, Screenshot};
use ffxi_viewer_core::dat_mmb::LoadMmbRequest;
use ffxi_viewer_core::dat_mzb::{
    process_load_mzb_requests, DrawDistance, LoadMzbRequest, MzbCollisionGeometry, ZoneGeomMode,
};
use ffxi_viewer_core::SceneState;
use std::env;

#[derive(Resource, Clone)]
struct RenderParams {
    /// Either a zone_id (looked up via `zone_id_to_mzb_file_id`) or a
    /// raw DAT file_id (used directly if the zone lookup fails).
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
        // Registered (no consumer here) so `process_load_mzb_requests`
        // can emit its companion LoadMmbRequest events without panicking.
        // We don't render MMB; only MZB is needed for the empirical
        // material workflow.
        .add_message::<LoadMmbRequest>()
        .init_resource::<DrawDistance>()
        .init_resource::<MzbCollisionGeometry>()
        .init_resource::<SceneState>()
        .add_systems(Startup, (setup_camera, fire_load_request))
        .add_systems(
            Update,
            (process_load_mzb_requests, capture_and_exit).chain(),
        )
        .run();
}

fn parse_args() -> RenderParams {
    let mut p = RenderParams {
        zone_or_file_id: 234, // Bastok Mines default
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

fn setup_camera(
    mut commands: Commands,
    params: Res<RenderParams>,
    mut draw: ResMut<DrawDistance>,
) {
    // Empirical workflow wants every visible surface, both collision
    // and decoration, so we override the production default (`Off`).
    draw.zone_geom_mode = ZoneGeomMode::Off;
    // Bump the cull distance well past any plausible zone extent so
    // distance culling doesn't hide the surfaces we're trying to capture.
    draw.world = 10_000.0;
    draw.mob = 10_000.0;

    commands.spawn((
        Camera3d::default(),
        Transform::from_translation(params.camera_pos).looking_at(params.camera_target, Vec3::Y),
    ));

    // MZB materials are `unlit` so light doesn't strictly matter, but
    // an ambient is cheap insurance if a future change adds PBR fall-
    // through paths.
    commands.insert_resource(AmbientLight {
        color: Color::WHITE,
        brightness: 200.0,
        ..default()
    });
}

fn fire_load_request(mut tx: MessageWriter<LoadMzbRequest>, params: Res<RenderParams>) {
    // Treat `zone_or_file_id` as a zone_id first; fall back to using
    // it as a raw DAT file_id if the zone-table mapping is missing.
    // FFXI zone IDs are small (<300); file_ids are 100..400-ish for
    // base zones, so either interpretation is plausible — the lookup
    // table is the authoritative source.
    // `zone_id_to_mzb_file_id` takes u16; cast safely (zone IDs fit).
    let zone_or_file_id = params.zone_or_file_id;
    let zone_id_u16 = u16::try_from(zone_or_file_id).ok();
    let file_id = match zone_id_u16.and_then(ffxi_dat::zone_dat::zone_id_to_mzb_file_id) {
        Some(fid) => {
            eprintln!("  zone_id {zone_or_file_id} -> mzb file_id {fid}");
            fid
        }
        None => {
            eprintln!(
                "  zone_id {zone_or_file_id} has no MZB mapping; using as file_id directly"
            );
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

    // Wait until the screenshot is no longer in-flight (the entity with
    // `Capturing` is removed when save completes), then give one extra
    // frame for the file write to flush before exiting.
    if capture.spawned && capturing_q.is_empty() && frame.0 >= params.capture_frame + 5 {
        eprintln!("mzb-render-headless: done");
        exit.write(AppExit::Success);
    }
}
