//! Operator chase camera with **decoupled yaw**: the camera sits at its own
//! `yaw` angle around the player Y-axis, independent of player heading. The
//! input layer mutates yaw/pitch directly; player heading is set elsewhere.
//!
//! When forward (W/S) is pressed, the input system snaps player heading to
//! [`heading_for_yaw`] so the player walks in the direction the camera looks
//! — FFXI's "third-person walk-toward-camera-forward" behavior.

use bevy::prelude::*;

use crate::components::IsSelf;
use crate::snapshot::SceneState;

/// Marker on the operator camera entity.
#[derive(Component)]
pub struct OperatorCamera;

/// Tunable chase-camera state. Yaw is the angle (Bevy radians, around +Y)
/// pointing from player toward camera. Default 0 = camera at player+Z. The
/// `synced_initial` flag lets the input layer one-shot align yaw to the
/// player's spawn heading so the camera starts behind, not "north" of a
/// south-facing avatar.
#[derive(Resource)]
pub struct ChaseCamera {
    /// Camera azimuth around the player, radians. yaw=0 → camera at +Z
    /// direction from player. Wraps via the input layer; not normalized
    /// here (`f32::sin`/`cos` are happy with any value).
    pub yaw: f32,
    /// Pitch from horizontal, radians. 0 = level (eye-line); π/2 = directly
    /// overhead. Clamp range: [`PITCH_MIN`]..[`PITCH_MAX`].
    pub pitch: f32,
    /// Total camera-to-player distance in Bevy units.
    pub distance: f32,
    /// Raise the look-at point above char origin so the camera doesn't aim
    /// at the ground (the capsule's center is ~1.0 above the floor).
    pub height_target: f32,
    /// Per-frame lerp factor for translation smoothing. 1.0 = snap.
    pub smoothing: f32,
    /// Set to true once we've snapped yaw to the spawn heading. Until then,
    /// the chase system performs the one-shot sync.
    pub synced_initial: bool,
}

impl ChaseCamera {
    /// Floor pitch ≈ 6° — keeps camera off the ground plane.
    pub const PITCH_MIN: f32 = 0.10;
    /// Ceiling pitch ≈ 80° — leaves a small angle so the camera doesn't go
    /// fully top-down (which would lose the chase aesthetic).
    pub const PITCH_MAX: f32 = 1.40;
}

impl Default for ChaseCamera {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.55,
            distance: 18.0,
            height_target: 1.0,
            smoothing: 0.18,
            synced_initial: false,
        }
    }
}

pub fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        OperatorCamera,
        Camera3d::default(),
        Transform::from_xyz(0.0, 12.0, 18.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(ChaseCamera::default());
}

/// Position camera using spherical coords (yaw, pitch, distance) anchored
/// on the [`IsSelf`] avatar.
///
/// Geometry: camera_offset = (sin(yaw)·cos(pitch), sin(pitch), cos(yaw)·cos(pitch)) · distance.
/// At yaw=0, pitch=0, the camera sits at player + (0, 0, distance) — straight
/// behind a player that faces -Z (= FFXI heading 0 / north).
pub fn chase_camera_system(
    mut chase: ResMut<ChaseCamera>,
    state: Res<SceneState>,
    q_self: Query<&Transform, (With<IsSelf>, Without<OperatorCamera>)>,
    mut q_cam: Query<&mut Transform, (With<OperatorCamera>, Without<IsSelf>)>,
) {
    let Ok(self_t) = q_self.single() else {
        return;
    };
    let Ok(mut cam_t) = q_cam.single_mut() else {
        return;
    };

    // One-shot: align yaw to player's spawn heading so camera starts behind.
    if !chase.synced_initial {
        chase.yaw = yaw_for_heading(state.snapshot.self_pos.heading);
        chase.synced_initial = true;
    }

    let cos_p = chase.pitch.cos();
    let sin_p = chase.pitch.sin();
    let yaw_dir = Vec3::new(chase.yaw.sin(), 0.0, chase.yaw.cos());
    let desired = self_t.translation
        + yaw_dir * (chase.distance * cos_p)
        + Vec3::Y * (chase.distance * sin_p);

    cam_t.translation = cam_t.translation.lerp(desired, chase.smoothing);
    cam_t.look_at(self_t.translation + Vec3::Y * chase.height_target, Vec3::Y);
}

/// FFXI heading u8 → camera yaw radians (camera-behind-player).
///
/// The relationship is `yaw = -heading_angle (mod τ)`. Derivation: a player
/// facing FFXI heading `h` has Bevy forward = (sin(α), 0, -cos(α)) where
/// α = h·τ/256. Camera should sit on the opposite side; the player→camera
/// direction is therefore (-sin(α), 0, cos(α)). With our parameterization
/// `(sin(yaw), 0, cos(yaw))`, that means `yaw = -α`.
#[inline]
pub fn yaw_for_heading(heading: u8) -> f32 {
    -(heading as f32) * std::f32::consts::TAU / 256.0
}

/// Camera yaw radians → FFXI heading u8 (player facing away from camera).
///
/// Inverse of [`yaw_for_heading`]. Used by the input layer to snap player
/// heading to "look in the camera's forward direction" when W/S is pressed.
#[inline]
pub fn heading_for_yaw(yaw: f32) -> u8 {
    let tau = std::f32::consts::TAU;
    let normalized = (-yaw).rem_euclid(tau);
    (normalized * 256.0 / tau).round() as u32 as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaw_heading_roundtrip_cardinals() {
        // North, East, South, West (FFXI heading 0/64/128/192).
        for &h in &[0u8, 64, 128, 192] {
            let y = yaw_for_heading(h);
            let back = heading_for_yaw(y);
            assert_eq!(back, h, "roundtrip for heading {h}");
        }
    }
}

