use std::path::PathBuf;
use std::time::Duration;

use bevy::app::{AppExit, ScheduleRunnerPlugin};
use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::view::screenshot::{save_to_disk, Capturing, Screenshot};
use bevy::winit::WinitPlugin;
use kuluu::view_native::{bridge::NativeSource, dev_diagnostics, perf_hud};
use kuluu_render::ffxi_actor_render::ActorLoadInFlight;
use kuluu_render::ffxi_zone_material::FfxiZoneMaterial;
use kuluu_render::hud::HudPanels;
use kuluu_render::skinned_ffxi_material::FfxiSkinnedMaterial;
use kuluu_render::snapshot::SceneState;

const WIDTH: u32 = 960;
const HEIGHT: u32 = 900;
const STEP_SECONDS: f64 = 1.0 / 60.0;
const HIDDEN_FRAME: u32 = 45;
const OPEN_FRAME: u32 = 60;
const OPEN_CAPTURE_FRAME: u32 = 120;
const CLOSE_FRAME: u32 = 150;
const CLOSED_CAPTURE_FRAME: u32 = 180;
const EXIT_FRAME: u32 = 210;

#[derive(Resource)]
struct Capture {
    image: Handle<Image>,
    directory: PathBuf,
}

fn main() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (_state_tx, state_rx) = tokio::sync::watch::channel(Default::default());
    let (_event_tx, event_rx) = tokio::sync::broadcast::channel(1);
    let (_changes_tx, changes_rx) = tokio::sync::mpsc::unbounded_channel();
    let source = NativeSource::new(runtime.handle(), state_rx, event_rx, changes_rx);
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    ..default()
                })
                .disable::<WinitPlugin>(),
        )
        .add_plugins(
            (
                ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(STEP_SECONDS)),
                FrameTimeDiagnosticsPlugin::default(),
                bevy::diagnostic::SystemInformationDiagnosticsPlugin,
                bevy::render::diagnostic::MeshAllocatorDiagnosticPlugin,
                bevy::render::diagnostic::RenderAssetDiagnosticPlugin::<
                    bevy::render::texture::GpuImage,
                >::new(" images"),
            ),
        )
        .init_asset::<FfxiZoneMaterial>()
        .init_asset::<FfxiSkinnedMaterial>()
        .insert_resource(source)
        .init_resource::<HudPanels>()
        .init_resource::<SceneState>()
        .init_resource::<ActorLoadInFlight>()
        .init_resource::<perf_hud::PerfMonitor>()
        .init_resource::<perf_hud::AssetChurn>()
        .insert_resource(
            bevy_framepace::FramepaceSettings::default()
                .with_limiter(bevy_framepace::Limiter::from_framerate(120.0)),
        )
        .add_systems(Startup, (setup, perf_hud::spawn_perf_hud))
        .add_systems(First, perf_hud::mark_frame_start)
        .add_systems(PostUpdate, perf_hud::mark_frame_end)
        .add_systems(Last, perf_hud::mark_last_end)
        .add_systems(
            Update,
            (
                perf_hud::track_asset_churn,
                perf_hud::update_perf_monitor,
                perf_hud::update_perf_graph,
                dev_diagnostics::update_summary,
                perf_hud::apply_perf_visibility,
                capture,
            )
                .chain(),
        )
        .run();
}

fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let mut image = Image::new_fill(
        Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC | TextureUsages::RENDER_ATTACHMENT;
    let image = images.add(image);
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("kuluu-dev-diagnostics"));
    std::fs::create_dir_all(&directory).unwrap();
    commands.spawn((
        Camera2d,
        IsDefaultUiCamera,
        RenderTarget::Image(image.clone().into()),
    ));
    commands.insert_resource(Capture { image, directory });
}

fn capture(
    mut commands: Commands,
    mut frame: Local<u32>,
    capture: Res<Capture>,
    mut panels: ResMut<HudPanels>,
    capturing: Query<(), With<Capturing>>,
    mut exit: MessageWriter<AppExit>,
) {
    *frame += 1;
    if *frame == OPEN_FRAME {
        panels.perf = true;
    }
    if *frame == CLOSE_FRAME {
        panels.perf = false;
    }
    let file = match *frame {
        HIDDEN_FRAME => Some("default-hidden.png"),
        OPEN_CAPTURE_FRAME => Some("diagnostics-open.png"),
        CLOSED_CAPTURE_FRAME => Some("diagnostics-closed.png"),
        _ => None,
    };
    if let Some(file) = file {
        commands
            .spawn(Screenshot::image(capture.image.clone()))
            .observe(save_to_disk(capture.directory.join(file)));
    }
    if *frame >= EXIT_FRAME && capturing.is_empty() {
        exit.write(AppExit::Success);
    }
}
