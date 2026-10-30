use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

const PREVIEW_ARGUMENT: &str = "--preview";

fn requested() -> bool {
    std::env::args().any(|arg| arg == PREVIEW_ARGUMENT)
}

pub fn window(resolution: (u32, u32)) -> Window {
    Window {
        visible: requested(),
        focused: false,
        resolution: resolution.into(),
        ..default()
    }
}

pub fn spawn(commands: &mut Commands, image: Handle<Image>, size: UVec2) {
    if !requested() {
        return;
    }
    commands.spawn((
        Camera2d,
        Camera {
            order: 1,
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: bevy::camera::ScalingMode::AutoMin {
                min_width: size.x as f32,
                min_height: size.y as f32,
            },
            ..OrthographicProjection::default_2d()
        }),
    ));
    commands.spawn(Sprite::from_image(image));
}

pub fn capture(commands: &mut Commands, image: Handle<Image>, path: &str) {
    let path = std::path::PathBuf::from(path);
    if requested() {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let preview = path.with_file_name(format!("{stem}-preview.png"));
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(preview));
    }
    commands
        .spawn(Screenshot::image(image))
        .observe(save_to_disk(path));
}
