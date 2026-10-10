use super::*;
use std::path::PathBuf;

use bevy::app::ScheduleRunnerPlugin;
use bevy::camera::RenderTarget;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::view::screenshot::{save_to_disk, Capturing, Screenshot};
use bevy::winit::WinitPlugin;

const OUTPUT_ENV: &str = "KULUU_IMMEDIATE_BINDINGS_CAPTURE_DIR";
const FILENAME: &str = "linked-generator-bindings.png";
const SIZE: u32 = 512;
const CAPTURE_FRAME: u32 = 60;
const EXIT_FRAME: u32 = 90;
const CAMERA_DISTANCE: f32 = 24.0;

#[derive(Resource)]
struct Target {
    image: Handle<Image>,
    output: PathBuf,
}

fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let mut image = Image::new_fill(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC | TextureUsages::RENDER_ATTACHMENT;
    let target = images.add(image);
    commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        RenderTarget::Image(target.clone().into()),
        Transform::from_xyz(0.0, 0.0, CAMERA_DISTANCE).looking_at(Vec3::ZERO, Vec3::Y),
        OperatorCamera,
        Msaa::Off,
    ));
    let mut assets = assets(true);
    for (id, color) in [
        (SOURCE, Vec4::new(0.0, 1.0, 0.0, 1.0)),
        (LINK, Vec4::new(1.0, 0.0, 0.0, 1.0)),
    ] {
        let definition = assets.particle_defs.get_mut(&id).unwrap();
        definition.init_color = color.to_array();
        definition.init_scale = Vec3::ONE.to_array();
        definition.fog_enabled = false;
        definition.child_generator = None;
    }
    let actor = commands.spawn((assets, Transform::IDENTITY)).id();
    commands.queue(move |world: &mut World| {
        world.write_message(SchedulerStageEvent {
            identity: Default::default(),
            actor,
            target: Some(actor),
            stage: particle_stage(SOURCE),
            scheduler: SOURCE,
        });
    });
    commands.insert_resource(Target {
        image: target,
        output: std::env::var_os(OUTPUT_ENV).unwrap().into(),
    });
}

fn prepare(mut sim: ResMut<ParticleSimulator>, mut ready: Local<bool>) {
    if *ready || sim.generators.len() != SOURCE_AND_LINK {
        return;
    }
    advance_simulator(&mut sim, ONE_FRAME);
    for g in &mut sim.generators {
        g.particles[0].age_frames = HALF_LIFE;
        g.stopped = true;
    }
    *ready = true;
}

fn capture(
    mut commands: Commands,
    mut frame: Local<u32>,
    target: Res<Target>,
    sim: Res<ParticleSimulator>,
    capturing: Query<(), With<Capturing>>,
    mut exit: MessageWriter<bevy::app::AppExit>,
) {
    *frame += 1;
    if *frame == CAPTURE_FRAME {
        assert_eq!(
            sim.generators.len(),
            SOURCE_AND_LINK,
            "the source and its immediate link must both be live to capture"
        );
        for g in &sim.generators {
            assert!(g.stopped, "prepare must have posed {:?}", g.def.mesh_id);
            assert!(!g.particles.is_empty(), "{:?} drew nothing", g.def.mesh_id);
        }
        commands
            .spawn(Screenshot::image(target.image.clone()))
            .observe(save_to_disk(target.output.join(FILENAME)));
    }
    if *frame >= EXIT_FRAME && capturing.is_empty() {
        exit.write(bevy::app::AppExit::Success);
    }
}

#[test]
#[ignore = "requires a graphics adapter and an explicit capture output directory"]
fn captures_immediate_link_bindings_through_production_rendering() {
    let output = PathBuf::from(std::env::var_os(OUTPUT_ENV).expect("set capture directory"));
    std::fs::create_dir_all(&output).unwrap();
    let _ = std::fs::remove_file(output.join(FILENAME));
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
        .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
            1.0 / f64::from(ROUTINE_FPS),
        )))
        .add_plugins(crate::ffxi_particle_material::FfxiParticleMaterialPlugin)
        .init_resource::<ParticleSimulator>()
        .init_resource::<crate::distortion_pass::ActiveDistortion>()
        .init_resource::<crate::graphics_settings::GraphicsSettings>()
        .add_message::<SchedulerStageEvent>()
        .add_message::<crate::audio::SfxEvent>()
        .add_message::<crate::scheduler_runtime::ParticleSpawnTrace>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                spawn_particle_generators,
                prepare,
                sync_particle_meshes,
                capture,
            )
                .chain(),
        )
        .run();
    assert!(output.join(FILENAME).metadata().unwrap().len() > 0);
}
