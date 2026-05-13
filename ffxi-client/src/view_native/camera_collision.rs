//! Clamp the third-person chase camera against zone collision so it
//! doesn't tunnel through walls.
//!
//! Approach: each frame, after `chase_camera_system` writes the camera's
//! desired translation, call Detour's `slide_along` from the player's
//! position to the camera's. If the line would cross a navmesh edge
//! (i.e., the camera is on the far side of a wall from the player),
//! `slide_along` returns the clamped position on the player's side of
//! the wall. Convert back to Bevy world space and write it onto the
//! camera transform.
//!
//! Limitations:
//! - Detour navmesh only represents the *walkable surface*. Ceilings,
//!   chimneys, decorative overhead clutter aren't in the navmesh, so a
//!   camera angled up through a ceiling won't get clamped. Fine for
//!   the common case (player at ground, camera behind them in plan).
//! - The clamp is 2D in the navmesh plane. Camera height (Bevy y) is
//!   preserved unchanged so a pitched-up shot still rises above the
//!   player's head — the clamp only prevents the horizontal projection
//!   from crossing a wall.

use bevy::prelude::*;
use ffxi_nav::glam;

use ffxi_viewer_core::components::IsSelf;
use ffxi_viewer_core::OperatorCamera;

use super::navmesh_overlay::NavmeshState;

/// Run AFTER `chase_camera_system` (which writes the desired-pos
/// lerp into `cam_t.translation`). Slides the camera back along the
/// player→camera line if a wall sits between them.
pub fn clamp_chase_camera_to_collision(
    nav: Res<NavmeshState>,
    self_q: Query<&Transform, (With<IsSelf>, Without<OperatorCamera>)>,
    mut cam_q: Query<&mut Transform, (With<OperatorCamera>, Without<IsSelf>)>,
) {
    // Run every frame — earlier `Changed<Transform>` throttle skipped
    // ticks where the camera was steady but the *player* had just
    // walked toward a wall, leaving the camera embedded inside it
    // until the next yaw/zoom event nudged the camera transform.
    // The navmesh lock + slide_along cost is small compared to the
    // alternative of clipping through walls.
    let Some(nav_lock) = nav.nav.as_ref() else {
        return;
    };
    let Ok(self_t) = self_q.single() else {
        return;
    };
    let Ok(mut cam_t) = cam_q.single_mut() else {
        return;
    };
    let Ok(guard) = nav_lock.lock() else {
        return;
    };

    // Bevy → FFXI z-up (codebase frame) — inverse of `ffxi_to_bevy`:
    //   ffxi_to_bevy(p) = (p.x, -p.z, -p.y)   (post-May 2026 Y-flip)
    //   inverse:          ffxi.x = bevy.x
    //                     ffxi.y = -bevy.z
    //                     ffxi.z = -bevy.y
    let to_ffxi = |b: Vec3| glam::Vec3::new(b.x, -b.z, -b.y);
    let to_bevy = |f: glam::Vec3| Vec3::new(f.x, -f.z, -f.y);

    // Slide in the navmesh plane only — use the player's z (height)
    // for both endpoints so Detour searches the same horizontal layer.
    let player_ffxi = to_ffxi(self_t.translation);
    let cam_ffxi_full = to_ffxi(cam_t.translation);
    let cam_ffxi_planar = glam::Vec3::new(cam_ffxi_full.x, cam_ffxi_full.y, player_ffxi.z);

    if let Some(slid) = guard.slide_along(player_ffxi, cam_ffxi_planar) {
        // Restore the original camera height — slide_along snapped the
        // result to the navmesh poly's surface, but the camera is
        // allowed to float above for chase pitch.
        let clamped_ffxi = glam::Vec3::new(slid.x, slid.y, cam_ffxi_full.z);
        cam_t.translation = to_bevy(clamped_ffxi);
    }
}
