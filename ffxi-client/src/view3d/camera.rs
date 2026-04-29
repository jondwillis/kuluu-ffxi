//! Chase camera. Follows the player's IsSelf entity at a fixed offset
//! and pitch — Stage-2 simplicity. Stage 5 will add operator-controlled
//! orbit/zoom; the rig structure here is designed to take that without
//! reworking the system signatures.

use bevy::{pbr::DistanceFog, pbr::FogFalloff, prelude::*};
use bevy_ratatui_camera::{RatatuiCamera, RatatuiCameraEdgeDetection, RatatuiCameraStrategy};

use super::scene::IsSelf;

/// Spawn the chase camera at startup. The transform is overwritten on the
/// first run of `chase_camera_system`.
///
/// Three components on the camera worth calling out:
///  - `RatatuiCameraStrategy::halfblocks()` — the cell-encoding strategy.
///  - `RatatuiCameraEdgeDetection` — outline characters on geometry
///    boundaries. Without them capsules tend to read as fuzzy color blobs
///    at terminal resolution because adjacent half-blocks share the same
///    lit color.
///  - `DistanceFog { Linear { 35..90 } }` — fades distant geometry into
///    the clear color. Mostly cosmetic, but at terminal resolution a
///    25-unit-away mob and a 60-unit-away NPC look identically sized;
///    the fog gradient is the only depth cue the eye can read.
pub fn setup_camera(mut commands: Commands) {
    commands.spawn((
        RatatuiCamera::default(),
        RatatuiCameraStrategy::halfblocks(),
        RatatuiCameraEdgeDetection::default(),
        Camera3d::default(),
        DistanceFog {
            color: Color::srgb(0.05, 0.05, 0.08),
            falloff: FogFalloff::Linear {
                start: 35.0,
                end: 90.0,
            },
            ..default()
        },
        Transform::from_xyz(0.0, 6.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Player-local chase offsets. Behind = `BACK` units along the player's
/// negative-forward axis; above = `UP` units in world Y. By projecting
/// the offset through the player's *local* frame instead of world space,
/// "W moves toward the top of the screen" stays true regardless of which
/// way the player is facing.
const BACK: f32 = 12.0;
const UP: f32 = 8.0;

pub fn chase_camera_system(
    self_q: Query<&Transform, (With<IsSelf>, Without<Camera3d>)>,
    mut cam_q: Query<&mut Transform, With<Camera3d>>,
) {
    let Ok(self_t) = self_q.single() else { return };
    let Ok(mut cam_t) = cam_q.single_mut() else {
        return;
    };
    let target = self_t.translation;
    // `Transform::forward()` is `Dir3` in Bevy 0.17; deref gives Vec3.
    let forward: Vec3 = (*self_t.forward()).into();
    cam_t.translation = target - forward * BACK + Vec3::Y * UP;
    cam_t.look_at(target + Vec3::Y * 0.5, Vec3::Y);
}
