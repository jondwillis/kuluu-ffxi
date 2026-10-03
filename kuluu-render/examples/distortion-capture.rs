use std::time::{Duration, Instant};

use bevy::app::{AppExit, ScheduleRunnerPlugin};
use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::view::screenshot::{save_to_disk, Capturing, Screenshot};
use bevy::winit::WinitPlugin;
use kuluu_render::camera::OperatorCamera;
use kuluu_render::distortion_pass::{ActiveDistortion, DistortionPassPlugin};

const SIZE: u32 = 512;
const BASELINE_FRAME: u32 = 60;
const ENABLE_FRAME: u32 = 90;
const ACTIVE_FRAME: u32 = 120;
const EXIT_FRAME: u32 = 150;
const HOLD_SECONDS: u64 = 60;
const STEP_SECONDS: f64 = 1.0 / 60.0;
const MARKER_OFFSET: f32 = 1.0;
const CAMERA_DISTANCE: f32 = 6.0;

#[derive(Resource)]
struct CaptureTarget(Handle<Image>);

#[derive(Component)]
struct MovingMarker;

fn main() {
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
            STEP_SECONDS,
        )))
        .add_plugins(DistortionPassPlugin)
        .add_systems(Startup, setup)
        .add_systems(Update, capture)
        .run();
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
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
    commands.insert_resource(CaptureTarget(target.clone()));
    commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        RenderTarget::Image(target.into()),
        Transform::from_xyz(0.0, 0.0, CAMERA_DISTANCE).looking_at(Vec3::ZERO, Vec3::Y),
        OperatorCamera,
        Msaa::Off,
    ));
    for (position, color, moving) in [
        (Vec3::Y, Color::srgb(1.0, 0.0, 0.0), false),
        (Vec3::NEG_Y, Color::srgb(0.0, 0.0, 1.0), false),
        (Vec3::X, Color::WHITE, true),
    ] {
        let mut entity = commands.spawn((
            Mesh3d(meshes.add(Cuboid::default())),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: color,
                unlit: true,
                ..default()
            })),
            Transform::from_translation(position),
        ));
        if moving {
            entity.insert(MovingMarker);
        }
    }
}

fn capture(
    mut commands: Commands,
    mut frame: Local<u32>,
    mut marker: Query<&mut Transform, With<MovingMarker>>,
    target: Res<CaptureTarget>,
    mut distortion: ResMut<ActiveDistortion>,
    capturing: Query<(), With<Capturing>>,
    mut exit: MessageWriter<AppExit>,
) {
    *frame += 1;
    for mut transform in &mut marker {
        transform.translation.x = if (*frame).is_multiple_of(2) {
            MARKER_OFFSET
        } else {
            -MARKER_OFFSET
        };
    }
    if *frame == ENABLE_FRAME {
        distortion.started_at = Instant::now();
        distortion.duration_secs = HOLD_SECONDS as f32;
        distortion.expires_at = Some(Instant::now() + Duration::from_secs(HOLD_SECONDS));
    }
    let filename = match *frame {
        BASELINE_FRAME => Some("/private/tmp/distortion-baseline.png"),
        ACTIVE_FRAME => Some("/private/tmp/distortion-active.png"),
        _ => None,
    };
    if let Some(filename) = filename {
        commands
            .spawn(Screenshot::image(target.0.clone()))
            .observe(save_to_disk(filename));
    }
    if *frame >= EXIT_FRAME && capturing.is_empty() {
        exit.write(AppExit::Success);
    }
}
