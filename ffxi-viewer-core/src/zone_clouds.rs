//! Faithful cloud mesh — the zone's authored `"clod"` MMB.
//!
//! FFXI ships each outdoor zone's clouds as a pre-baked textured MMB
//! whose header name starts with `"clod"` (cite-only: lotus
//! `landscape_entity`, RZN `FFXILandscapeMesh::generateCloudMeshBuffer`).
//! Our normal MZB→MMB spawn pipeline already instances it as a prop;
//! `dat_mmb` tags that parent with [`CloudMesh`] when it sees the name.
//!
//! This module owns what to *do* with it, split by sky style:
//!   * **Retail** — show the authored cloud mesh and drift it slowly
//!     (retail scrolls the cloud texture's UVs; lacking a custom cloud
//!     material we approximate with a slow yaw of the mesh, which the
//!     references explicitly note as an acceptable stand-in).
//!   * **Enhanced** — hide it; the procedural scrolling dome in
//!     [`crate::skybox`] owns the Enhanced look.
//!
//! Follow-up (needs a retail DAT to verify): a dedicated cloud material
//! doing retail's `texColor·vertexColor + skyColor` with true UV scroll,
//! and confirming/overriding the authored placement against retail's
//! hardcoded `(0,50,0)` / scale-4 transform.

use bevy::prelude::*;

use crate::graphics_settings::{GraphicsSettings, SkyStyle};

/// Marker on the parent entity of the zone's `"clod"` cloud MMB.
#[derive(Component)]
pub struct CloudMesh;

/// Radians/second of cloud yaw in Retail style — a slow drift.
const CLOUD_DRIFT_RATE: f32 = 0.012;

fn drive_cloud_mesh(
    time: Res<Time>,
    settings: Res<GraphicsSettings>,
    mut q: Query<(&mut Visibility, &mut Transform), With<CloudMesh>>,
) {
    // Faithful MMB clouds show only in *full* Retail; the procedural dome
    // (`crate::skybox`) owns Enhanced/Custom.
    let retail = settings.sky_style() == SkyStyle::Retail;
    let dt = time.delta_secs();
    for (mut vis, mut xf) in &mut q {
        *vis = if retail {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if retail {
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
