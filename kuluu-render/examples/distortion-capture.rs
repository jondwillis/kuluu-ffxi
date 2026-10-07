use std::path::PathBuf;
use std::time::{Duration, Instant};

use bevy::app::{AppExit, ScheduleRunnerPlugin};
use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::view::screenshot::{save_to_disk, Capturing, Screenshot};
use bevy::winit::WinitPlugin;
use kuluu_render::camera::OperatorCamera;
use kuluu_render::distortion_pass::{
    ActiveDistortion, DistortionMap, DistortionPassPlugin, LiveField,
};

const SIZE: u32 = 512;
const BASELINE_FRAME: u32 = 60;
const ENABLE_FRAME: u32 = 90;
const ACTIVE_FRAME: u32 = 120;
const RESET_FRAME: u32 = 150;
const RESET_CAPTURE_FRAME: u32 = 180;
const EXIT_FRAME: u32 = 210;
const HOLD_SECONDS: u64 = 60;
const STEP_SECONDS: f64 = 1.0 / 60.0;
const MARKER_OFFSET: f32 = 1.0;
const CAMERA_DISTANCE: f32 = 6.0;
const FIELD_HALF_EXTENT: f32 = 0.7;

#[derive(Resource)]
struct CaptureTarget {
    image: Handle<Image>,
    directory: PathBuf,
}

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
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("kuluu-distortion-capture"));
    std::fs::create_dir_all(&directory).expect("capture directory must be writable");
    commands.insert_resource(CaptureTarget {
        image: target.clone(),
        directory,
    });
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
        // A soft disc stands in for the linked texture a real 0x22 def resolves from the DAT.
        const R: u32 = 64;
        let mut rgba = vec![0u8; (R * R * 4) as usize];
        for y in 0..R {
            for x in 0..R {
                let d = ((x as f32 - R as f32 / 2.0).powi(2) + (y as f32 - R as f32 / 2.0).powi(2))
                    .sqrt()
                    / (R as f32 / 2.0);
                let a = ((1.0 - d) * 255.0).clamp(0.0, 255.0) as u8;
                let i = ((y * R + x) as usize) * 4;
                rgba[i..i + 4].copy_from_slice(&[255, 255, 255, a]);
            }
        }
        distortion.push(LiveField {
            center: Vec3::Y,
            half_extent: Vec2::splat(FIELD_HALF_EXTENT),
            haze_offset_x: 0.1,
            started_at: Instant::now(),
            duration_secs: HOLD_SECONDS as f32,
            envelope: None,
            map: std::sync::Arc::new(DistortionMap::new(R, R, rgba)),
        });
    }
    if *frame == RESET_FRAME {
        *distortion = ActiveDistortion::default();
    }
    let filename = match *frame {
        BASELINE_FRAME => Some("distortion-baseline.png"),
        ACTIVE_FRAME => Some("distortion-active.png"),
        RESET_CAPTURE_FRAME => Some("distortion-after-reset.png"),
        _ => None,
    };
    if let Some(filename) = filename {
        commands
            .spawn(Screenshot::image(target.image.clone()))
            .observe(save_to_disk(target.directory.join(filename)));
    }
    if *frame >= EXIT_FRAME && capturing.is_empty() {
        exit.write(AppExit::Success);
    }
}
