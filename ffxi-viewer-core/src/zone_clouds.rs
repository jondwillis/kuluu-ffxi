use bevy::prelude::*;

use crate::graphics_settings::{GraphicsSettings, SkyStyle};

#[derive(Component)]
pub struct CloudMesh;

const CLOUD_DRIFT_RATE: f32 = 0.012;

fn drive_cloud_mesh(
    time: Res<Time>,
    settings: Res<GraphicsSettings>,
    mut q: Query<(&mut Visibility, &mut Transform), With<CloudMesh>>,
) {
    let vanilla = settings.sky_style() == SkyStyle::Vanilla;
    let dt = time.delta_secs();
    for (mut vis, mut xf) in &mut q {
        *vis = if vanilla {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if vanilla {
            xf.rotate_y(CLOUD_DRIFT_RATE * dt);
        }
    }
}

pub struct ZoneCloudsPlugin;

impl Plugin for ZoneCloudsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, drive_cloud_mesh);
    }
}
