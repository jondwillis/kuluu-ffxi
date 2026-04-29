//! Operator camera: angled overhead, follows the `IsSelf` avatar with
//! exponential smoothing.
//!
//! Designed to swap to a `CameraMode::ThirdPerson` mode in a follow-on
//! by adding a resource + a sibling system; this one stays untouched.

use bevy::prelude::*;

use crate::components::IsSelf;

/// Marker on the operator camera entity.
#[derive(Component)]
pub struct OperatorCamera;

/// Tunable camera offset from the followed avatar, in Bevy world space.
/// Positioned above and behind for an over-the-shoulder operator view.
#[derive(Resource)]
pub struct CameraFollow {
    pub offset: Vec3,
    /// Per-frame lerp factor (0..=1). Higher = snappier; lower = smoother.
    /// 0.15 at 60 Hz is roughly a 100 ms time-constant.
    pub smoothing: f32,
}

impl Default for CameraFollow {
    fn default() -> Self {
        Self {
            offset: Vec3::new(0.0, 18.0, 18.0),
            smoothing: 0.15,
        }
    }
}

pub fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        OperatorCamera,
        Camera3d::default(),
        Transform::from_xyz(0.0, 18.0, 18.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(CameraFollow::default());
}

/// Each frame, lerp the camera toward (self_pos + offset) and re-aim at
/// self_pos. If no `IsSelf` avatar exists yet (pre-zone), the camera holds
/// its previous pose — which keeps the ground plane in view.
pub fn follow_self_system(
    follow: Res<CameraFollow>,
    q_self: Query<&Transform, (With<IsSelf>, Without<OperatorCamera>)>,
    mut q_cam: Query<&mut Transform, (With<OperatorCamera>, Without<IsSelf>)>,
) {
    let Ok(self_xform) = q_self.single() else {
        return;
    };
    let Ok(mut cam_xform) = q_cam.single_mut() else {
        return;
    };

    let target_pos = self_xform.translation + follow.offset;
    cam_xform.translation = cam_xform.translation.lerp(target_pos, follow.smoothing);
    cam_xform.look_at(self_xform.translation, Vec3::Y);
}

/// Chase camera: sits behind and above the player, following their forward
/// direction. Unlike `follow_self_system` (which uses a fixed world-space
/// offset), this projects the offset through the player's local frame so
/// "W always moves toward the top of the screen" regardless of facing.
///
/// Player-local chase offsets. Behind = `BACK` units along the player's
/// negative-forward axis; above = `UP` units in world Y.
const BACK: f32 = 12.0;
const UP: f32 = 8.0;

pub fn chase_camera_system(
    q_self: Query<&Transform, (With<IsSelf>, Without<OperatorCamera>)>,
    mut q_cam: Query<&mut Transform, (With<OperatorCamera>, Without<IsSelf>)>,
) {
    let Ok(self_t) = q_self.single() else {
        return;
    };
    let Ok(mut cam_t) = q_cam.single_mut() else {
        return;
    };

    let target = self_t.translation;
    let forward: Vec3 = (*self_t.forward()).into();
    cam_t.translation = target - forward * BACK + Vec3::Y * UP;
    cam_t.look_at(target + Vec3::Y * 0.5, Vec3::Y);
}
