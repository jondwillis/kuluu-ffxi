use bevy::app::{AppExit, ScheduleRunnerPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use kuluu_render::ffxi_particle_material::FfxiParticleMaterialPlugin;
use kuluu_render::particle_sim::*;
use kuluu_render::scheduler_runtime::*;
use std::time::Duration;
#[derive(Resource)]
struct Target(Handle<Image>);
#[derive(Resource, Default)]
struct Counter(u32);
#[derive(Resource, Default)]
struct DemoOwner(Option<Entity>);
#[derive(Resource)]
struct DemoOptions {
    capture: Option<String>,
}
const DEMO_BACKGROUND: Color = Color::srgb(0.04, 0.04, 0.06);
const CAMERA_POSITION: Vec3 = Vec3::new(0.0, 3.0, 8.0);
const CAMERA_FOCUS: Vec3 = Vec3::new(0.0, 2.2, 0.0);
const DEMO_FPS: f64 = 60.0;
const EFFECT_START_FRAME: u32 = 120;
const LAST_FRAME: u32 = EFFECT_START_FRAME + CAPTURE_END_FRAME + DEMO_FPS as u32;
const IMAGE_WIDTH: u32 = 1000;
const IMAGE_HEIGHT: u32 = 800;
const CAPTURE_INTERVAL: u32 = 3;
const CAPTURE_END_FRAME: u32 = 180;
fn main() {
    let capture = std::env::args().nth(1);
    let mut app = App::new();
    let plugins = DefaultPlugins
        .set(WindowPlugin {
            primary_window: capture.is_none().then(|| Window {
                title: "Kuluu: production level-up effect demo".into(),
                resolution: (IMAGE_WIDTH, IMAGE_HEIGHT).into(),
                ..default()
            }),
            exit_condition: if capture.is_some() {
                bevy::window::ExitCondition::DontExit
            } else {
                bevy::window::ExitCondition::OnAllClosed
            },
            ..default()
        })
        .set(AssetPlugin {
            file_path: format!("{}/../assets", env!("CARGO_MANIFEST_DIR")),
            ..default()
        })
        .disable::<bevy::audio::AudioPlugin>();
    if capture.is_some() {
        app.add_plugins(plugins.disable::<bevy::winit::WinitPlugin>());
    } else {
        app.add_plugins(plugins);
    }
    app.add_plugins(FfxiParticleMaterialPlugin)
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs_f64(1.0 / DEMO_FPS),
        ))
        .insert_resource(ClearColor(DEMO_BACKGROUND))
        .init_resource::<Counter>()
        .init_resource::<DemoOwner>()
        .init_resource::<ParticleSimulator>()
        .init_resource::<kuluu_render::graphics_settings::GraphicsSettings>()
        .add_message::<SchedulerStageEvent>()
        .add_message::<CutsceneMotionDone>()
        .add_message::<kuluu_render::audio::SfxEvent>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                advance,
                tick_active_schedulers,
                spawn_particle_generators,
                tick_particle_simulator,
            )
                .chain(),
        )
        .add_systems(
            PostUpdate,
            sync_particle_meshes.after(bevy::transform::TransformSystems::Propagate),
        )
        .insert_resource(DemoOptions {
            capture: capture.clone(),
        });
    if capture.is_some() {
        app.add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
            1.0 / DEMO_FPS,
        )));
    }
    app.run();
}
fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>, options: Res<DemoOptions>) {
    if options.capture.is_none() {
        commands.spawn((
            Camera3d::default(),
            kuluu_render::camera::OperatorCamera,
            Transform::from_translation(CAMERA_POSITION).looking_at(CAMERA_FOCUS, Vec3::Y),
        ));
        return;
    }
    let mut image = Image::new_fill(
        Extent3d {
            width: IMAGE_WIDTH,
            height: IMAGE_HEIGHT,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC | TextureUsages::RENDER_ATTACHMENT;
    let target = images.add(image);
    commands.insert_resource(Target(target.clone()));
    commands.spawn((
        Camera3d::default(),
        kuluu_render::camera::OperatorCamera,
        bevy::camera::RenderTarget::Image(target.into()),
        Transform::from_translation(CAMERA_POSITION).looking_at(CAMERA_FOCUS, Vec3::Y),
    ));
}
fn advance(
    mut commands: Commands,
    mut frame: ResMut<Counter>,
    mut owner: ResMut<DemoOwner>,
    keys: Res<ButtonInput<KeyCode>>,
    target: Option<Res<Target>>,
    options: Res<DemoOptions>,
    mut exit: MessageWriter<AppExit>,
) {
    frame.0 += 1;
    if frame.0 == EFFECT_START_FRAME {
        let root = ffxi_dat::DatRoot::from_env_or_default().unwrap();
        let bytes = std::fs::read(
            root.resolve(LEVEL_UP_EFFECT_DAT_ID)
                .unwrap()
                .path_under(&root),
        )
        .unwrap();
        eprintln!(
            "Level-up DAT {LEVEL_UP_EFFECT_DAT_ID}; client {:?}",
            root.profile()
        );
        let (schedulers, assets, _) = parse_action_bytes(&bytes);
        let active = ActiveScheduler::from_main(&schedulers, b"main").unwrap();

        owner.0 = Some(
            commands
                .spawn((
                    Transform::default(),
                    GlobalTransform::default(),
                    assets,
                    ActiveSchedulers::one(active),
                ))
                .id(),
        );
    }
    if let (Some(target), Some(dir)) = (target, options.capture.as_ref()) {
        if let Some(effect_frame) = frame.0.checked_sub(EFFECT_START_FRAME) {
            if effect_frame <= CAPTURE_END_FRAME && effect_frame.is_multiple_of(CAPTURE_INTERVAL) {
                std::fs::create_dir_all(dir).unwrap();
                let path = format!("{dir}/frame-{effect_frame}.png");
                commands
                    .spawn(Screenshot::image(target.0.clone()))
                    .observe(save_to_disk(path));
            }
        }
    }
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
    if frame.0 > LAST_FRAME {
        if options.capture.is_some() {
            exit.write(AppExit::Success);
        } else {
            if let Some(entity) = owner.0.take() {
                commands.entity(entity).despawn();
            }
            frame.0 = 0;
        }
    }
}
