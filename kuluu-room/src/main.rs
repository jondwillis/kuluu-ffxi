#![allow(clippy::type_complexity, clippy::too_many_arguments)]

use std::sync::Arc;

use anyhow::Result;
use bevy::prelude::*;
use clap::Parser;
use kuluu_render::source::SceneSource;
use kuluu_snapshot::{SceneDelta, SceneSnapshot, ViewerEvent};

mod scene;

#[derive(Parser)]
#[command(about = "Inspect animation and effects through the Kuluu renderer.")]
struct Args {
    #[arg(long, help = "Print the room controls without starting the renderer.")]
    knobs: bool,
    #[arg(long, help = "Open the window without keyboard focus.")]
    unfocused: bool,
    #[arg(long, help = "Mute sound effects and music.")]
    mute: bool,
    #[arg(
        long,
        help = "Start without game files to test the missing-install path."
    )]
    no_install: bool,
}

#[derive(Resource)]
struct RoomSource;

impl SceneSource for RoomSource {
    fn poll_snapshot(&mut self) -> Option<Box<SceneSnapshot>> {
        None
    }

    fn drain_deltas(&mut self) -> Vec<SceneDelta> {
        Vec::new()
    }

    fn drain_events(&mut self) -> Vec<ViewerEvent> {
        Vec::new()
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.knobs {
        println!("{}", scene::knob_report());
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let root = if args.no_install {
        None
    } else {
        Some(Arc::new(anyhow::Context::context(
            ffxi_dat::DatRoot::from_env_or_default(),
            "Select an install with `kuluu install use NAME`, or set FFXI_DAT_PATH.",
        )?))
    };
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Kuluu animation room".into(),
                    focused: !args.unfocused,
                    ..default()
                }),
                ..default()
            })
            .set(AssetPlugin {
                file_path: format!("{}/../assets", env!("CARGO_MANIFEST_DIR")),
                ..default()
            })
            .disable::<bevy::log::LogPlugin>(),
    );
    if cfg!(target_os = "macos") && std::env::var_os("FFXI_GPU_PREPROCESS").is_none() {
        use bevy::render::batching::gpu_preprocessing::{
            GpuPreprocessingMode, GpuPreprocessingSupport,
        };
        if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
            render_app.insert_resource(GpuPreprocessingSupport {
                max_supported_mode: GpuPreprocessingMode::None,
            });
            render_app.add_systems(
                bevy::render::RenderStartup,
                (|mut support: bevy::prelude::ResMut<GpuPreprocessingSupport>| {
                    support.max_supported_mode = GpuPreprocessingMode::None;
                })
                .after(bevy::render::init_gpu_resource::<GpuPreprocessingSupport>),
            );
        }
    }

    app.add_plugins((
        kuluu_render::ViewerCorePlugin::<RoomSource>::default(),
        kuluu_render::hud::HudPlugin,
        kuluu_render::mouse::MousePlugin,
    ));
    app.insert_resource(kuluu_render::audio::AudioMuteState {
        bgm: args.mute,
        sfx: args.mute,
        ..default()
    });
    app.world_mut()
        .resource_mut::<kuluu_render::audio::BgmSlots>()
        .install_root = root.as_ref().map(|root| root.root().to_path_buf());
    app.insert_resource(kuluu_render::minimap::retail::MinimapDatRoot(root.clone()))
        .insert_resource(kuluu_render::hud::status_ribbon::StatusIconDatRoot(
            root.clone(),
        ))
        .insert_resource(kuluu_render::hud::item_dat_root::ItemDatRoot(root.clone()))
        .insert_resource(kuluu_render::moon_material::MoonDatRoot(root.clone()))
        .insert_resource(kuluu_render::ui_element_atlas::UiElementDatRoot(
            root.clone(),
        ))
        .insert_resource(kuluu_render::cutscene::CutsceneFadeDatRoot(root.clone()))
        .insert_resource(kuluu_render::scheduler_runtime::ActionDatRoot(root.clone()))
        .insert_resource(kuluu_render::ffxi_actor_render::ActorDatRoot(root.clone()))
        .insert_resource(kuluu_render::dat_root::SharedDatRoot(root))
        .add_plugins(scene::AnimationTestScenePlugin);
    app.run();
    Ok(())
}
