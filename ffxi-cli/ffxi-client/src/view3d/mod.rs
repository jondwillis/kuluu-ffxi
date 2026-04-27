//! 3D operator dashboard — a headless Bevy app rendered into the terminal
//! via `bevy_ratatui_camera`. Stage 1 here is a self-contained spike:
//! a spinning cube, no FFXI session involvement, used to validate the
//! dependency stack and the main-thread/tokio split before wiring the
//! actual session in Stage 2.

use std::time::Duration;

use bevy::{
    app::{AppExit, ScheduleRunnerPlugin},
    log::LogPlugin,
    prelude::*,
    winit::WinitPlugin,
};
use bevy_ratatui::{RatatuiContext, RatatuiPlugins, event::KeyMessage};
use bevy_ratatui_camera::{
    RatatuiCamera, RatatuiCameraPlugin, RatatuiCameraStrategy, RatatuiCameraWidget,
};
use crossterm::event::{KeyCode, KeyEventKind};
use ratatui::widgets::Widget;

#[derive(Component)]
struct Spinner;

/// Stage-1 spike entry point. Blocks the calling thread until the user
/// presses `q` (or any other AppExit trigger). Returns once Bevy's event
/// loop drains.
///
/// Bevy's `App::run()` is synchronous and never returns to async; callers
/// from a tokio runtime must invoke this via `spawn_blocking` (or just
/// drop into it from a sync context — there's no FFXI session running
/// alongside the spike).
pub fn run_spike() -> anyhow::Result<()> {
    App::new()
        .add_plugins((
            // `WinitPlugin` would try to open an OS window; we want headless
            // rendering into the terminal instead. `LogPlugin` would write
            // to stderr and corrupt the alternate-screen view, so it's off
            // too — bevy_ratatui owns terminal output now.
            DefaultPlugins
                .build()
                .disable::<WinitPlugin>()
                .disable::<LogPlugin>(),
            // Without winit there's no event-driven loop, so we drive Update
            // ourselves at 60 Hz. Keeps CPU bounded and matches the example.
            ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(1.0 / 60.0)),
            RatatuiPlugins::default(),
            RatatuiCameraPlugin,
        ))
        .insert_resource(ClearColor(Color::BLACK))
        .add_systems(Startup, setup_scene)
        .add_systems(PreUpdate, handle_input)
        .add_systems(Update, (rotate_spinner, draw_terminal))
        .run();
    Ok(())
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Spinner,
        Mesh3d(meshes.add(Cuboid::default())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.4, 0.54, 0.7),
            ..default()
        })),
    ));
    commands.spawn((
        PointLight {
            intensity: 2_000_000.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(3.0, 4.0, 6.0),
    ));
    commands.spawn((
        RatatuiCamera::default(),
        RatatuiCameraStrategy::halfblocks(),
        Camera3d::default(),
        Transform::from_xyz(2.5, 2.5, 2.5).looking_at(Vec3::ZERO, Vec3::Z),
    ));
}

fn handle_input(mut keys: MessageReader<KeyMessage>, mut exit: MessageWriter<AppExit>) {
    for k in keys.read() {
        if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat)
            && matches!(k.code, KeyCode::Char('q') | KeyCode::Esc)
        {
            exit.write_default();
        }
    }
}

fn rotate_spinner(time: Res<Time>, mut spinner: Single<&mut Transform, With<Spinner>>) {
    spinner.rotate_z(time.delta_secs());
}

fn draw_terminal(
    mut ratatui: ResMut<RatatuiContext>,
    mut camera_widget: Single<&mut RatatuiCameraWidget>,
) -> Result {
    ratatui.draw(|frame| {
        camera_widget.render(frame.area(), frame.buffer_mut());
    })?;
    Ok(())
}
