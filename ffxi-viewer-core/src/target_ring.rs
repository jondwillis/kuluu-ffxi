//! Selected-target visual cues drawn with [`Gizmos`] (no mesh-asset
//! bookkeeping; everything here is re-emitted every frame).
//!
//! Two cues live in this module:
//!
//! 1. **Target arrow** — a downward-pointing, camera-facing triangle that
//!    floats just above the selected entity's nameplate, bobbing gently.
//!    This replaces the old flat ground *ring*: classic FFXI marks the
//!    current target with a cursor arrow above the head, not a circle on
//!    the floor. Yellow normally, red while the player is auto-attacking
//!    that exact target (see [`target_ring_color`]).
//! 2. **Engaged ring** — a red ring under the *player's own* feet while in
//!    combat. Unrelated to selection; kept as a separate "I am swinging"
//!    cue so it can coexist with the target arrow.
//!
//! The selected model itself is no longer recolored — `scene` leaves it at
//! its normal claim/kind material and a one-shot white strobe
//! (`target_strobe`) fires on selection. The arrow is the persistent cue.

use std::f32::consts::PI;

use bevy::prelude::*;

use crate::camera::{nameplate_anchor_y, OperatorCamera};
use crate::components::WorldEntity;
use crate::scene::{BakedActor, Target};
use crate::snapshot::SceneState;

/// Bright yellow matching the minimap target dot so the cue reads as
/// "the thing I selected".
const ARROW_COLOR: Color = Color::srgb(1.0, 0.95, 0.20);

/// Red arrow while auto-attacking this exact target. Distinct from the
/// yellow selection color so "selected" vs "fighting" stay legible.
const ARROW_ENGAGED_COLOR: Color = Color::srgb(1.00, 0.18, 0.22);

/// Arrow footprint in yalms. Width = the span of the wide (top) edge;
/// height = apex-to-edge. Tuned to read clearly above a nameplate
/// without dwarfing it.
const ARROW_WIDTH: f32 = 0.85;
const ARROW_HEIGHT: f32 = 0.65;

/// Lift of the arrow's *tip* above the nameplate anchor (crown + the
/// nameplate's small crown offset). The nameplate quad is centered at
/// the anchor and reaches up ~0.35 yalms; placing the tip 0.55 above
/// the anchor keeps the arrow clear of the text at all label sizes.
const ARROW_TIP_ABOVE_ANCHOR: f32 = 0.55;

/// Gentle vertical bob so the arrow reads as a live cursor rather than a
/// decal. Amplitude in yalms, angular frequency in rad/s (~0.5 Hz).
const ARROW_BOB_AMPLITUDE: f32 = 0.08;
const ARROW_BOB_FREQUENCY: f32 = 3.0;

/// Number of horizontal scanlines used to fake a *filled* triangle out
/// of line gizmos. 6 reads as solid at the arrow's on-screen size while
/// staying cheap (≈9 line segments total per frame for the one arrow).
const ARROW_FILL_SCANLINES: u32 = 6;

/// Red ring around the player when engaged (self has a non-zero
/// `bt_target_id`). Distinct from the arrow's yellow so both can be
/// visible simultaneously: yellow arrow over whoever the operator is
/// looking at, red ring under the operator's own feet while in combat.
const ENGAGED_RING_COLOR: Color = Color::srgb(1.00, 0.18, 0.22);

/// Lift above the entity's ground level to avoid z-fighting with the
/// navmesh / floor. Applied to the per-entity foot position.
const RING_Y_LIFT: f32 = 0.05;

const ENGAGED_RING_RADIUS: f32 = 1.7;

/// Pure decision: which color should the target arrow be?
///
/// Yellow on a non-combat selection, red when the player is currently
/// auto-attacking this exact target (`self.bt_target_id == target_id`,
/// the same wire signal `target_panel` uses for the engagement badge).
/// Lifting this into a pure function keeps the visual policy testable
/// without needing a Bevy world.
pub fn target_ring_color(engaged_on_target: bool) -> Color {
    if engaged_on_target {
        ARROW_ENGAGED_COLOR
    } else {
        ARROW_COLOR
    }
}

/// Pure helper: vertical bob offset (yalms) for the arrow at time `t`.
/// Factored out so the bob curve is unit-testable and so the drawing
/// code reads as "apex + bob".
pub fn arrow_bob_offset(seconds: f32) -> f32 {
    (seconds * ARROW_BOB_FREQUENCY).sin() * ARROW_BOB_AMPLITUDE
}

/// Draw the camera-facing target arrow above the selected entity, every
/// frame. Red when engaged on this target, yellow otherwise (see
/// [`target_ring_color`]).
///
/// Runs in `Update` after `sync_entities_system` (so `Target` and the
/// `WorldEntity` transforms are reconciled) and after the camera systems
/// (so the billboard orientation uses the settled camera pose).
pub fn draw_target_arrow_system(
    target: Res<Target>,
    state: Res<SceneState>,
    time: Res<Time>,
    cam_q: Query<&Transform, With<OperatorCamera>>,
    world_q: Query<(&Transform, &WorldEntity, Option<&BakedActor>)>,
    mut gizmos: Gizmos,
) {
    let Some(target_id) = target.id else {
        return;
    };
    let Ok(cam_t) = cam_q.single() else {
        return;
    };
    let cam_pos = cam_t.translation;

    // Server-authoritative engagement check: self's `bt_target_id` is the
    // server's notion of "what am I swinging at." Falls back to `false`
    // when self isn't in the snapshot yet (early post-zone-in).
    let engaged_on_target = state
        .snapshot
        .self_char_id
        .and_then(|sid| state.snapshot.entities.iter().find(|e| e.id == sid))
        .map(|self_pc| self_pc.bt_target_id == target_id)
        .unwrap_or(false);
    let color = target_ring_color(engaged_on_target);

    for (t, w, baked) in &world_q {
        if w.id != target_id {
            continue;
        }
        // Tip sits a little above the nameplate anchor, plus the bob.
        let tip_y = t.translation.y
            + nameplate_anchor_y(baked)
            + ARROW_TIP_ABOVE_ANCHOR
            + arrow_bob_offset(time.elapsed_secs());
        let apex = Vec3::new(t.translation.x, tip_y, t.translation.z);
        draw_camera_facing_arrow(&mut gizmos, apex, cam_pos, color);
        break;
    }
}

/// Emit the line gizmos for one downward-pointing, camera-facing,
/// Y-locked arrow whose tip is at `apex`. "Y-locked" means the arrow's
/// up axis stays world-up so the triangle never rolls when the camera
/// pitches — same trick the nameplate billboards use.
fn draw_camera_facing_arrow(gizmos: &mut Gizmos, apex: Vec3, cam_pos: Vec3, color: Color) {
    let up = Vec3::Y;
    // Screen-right in world space: perpendicular to both world-up and the
    // view direction. Degenerate only when the camera is directly above
    // the arrow (looking straight down), which the chase camera's pitch
    // clamp prevents; fall back to +X just in case.
    let to_cam = cam_pos - apex;
    let right = up.cross(to_cam).try_normalize().unwrap_or(Vec3::X);

    let top_center = apex + up * ARROW_HEIGHT;
    let half = right * (ARROW_WIDTH * 0.5);
    let top_left = top_center - half;
    let top_right = top_center + half;

    // Outline: the two slanted sides meeting at the tip, plus the top.
    gizmos.line(apex, top_left, color);
    gizmos.line(apex, top_right, color);
    gizmos.line(top_left, top_right, color);

    // Fake a filled triangle with horizontal scanlines. At fractional
    // height `f` (0 = tip, 1 = top edge) the half-width grows linearly
    // with `f`, so each scanline spans `±(ARROW_WIDTH/2)*f` around the
    // vertical axis.
    for i in 1..ARROW_FILL_SCANLINES {
        let f = i as f32 / ARROW_FILL_SCANLINES as f32;
        let center = apex + up * (ARROW_HEIGHT * f);
        let hw = right * (ARROW_WIDTH * 0.5 * f);
        gizmos.line(center - hw, center + hw, color);
    }
}

/// Draw a red ring at the player's feet while engaged. "Engaged" means
/// the self entity in the latest snapshot has a non-zero `bt_target_id`
/// (battle target id) — the same wire signal the server uses to gate
/// auto-attack. Cheap: one gizmo per frame, only while in combat.
///
/// Self is identified by `snap.self_char_id`; if that hasn't resolved
/// yet (early in the post-zone-in window) the system no-ops.
pub fn draw_engaged_ring_system(
    state: Res<SceneState>,
    world_q: Query<(&Transform, &WorldEntity)>,
    mut gizmos: Gizmos,
) {
    let Some(self_id) = state.snapshot.self_char_id else {
        return;
    };
    let engaged = state
        .snapshot
        .entities
        .iter()
        .find(|e| e.id == self_id)
        .map(|e| e.bt_target_id != 0)
        .unwrap_or(false);
    if !engaged {
        return;
    }
    for (t, w) in &world_q {
        if w.id == self_id {
            let ground_y = t.translation.y + RING_Y_LIFT;
            let pos = Vec3::new(t.translation.x, ground_y, t.translation.z);
            gizmos.circle(
                Isometry3d::new(pos, Quat::from_rotation_x(-PI / 2.0)),
                ENGAGED_RING_RADIUS,
                ENGAGED_RING_COLOR,
            );
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Engagement on the *same* entity that's currently targeted → red
    /// arrow. Operator wants the cue to visually echo combat state rather
    /// than stay yellow throughout a fight.
    #[test]
    fn engaged_target_uses_red() {
        assert_eq!(target_ring_color(true), ARROW_ENGAGED_COLOR);
    }

    /// Targeting an entity we aren't fighting (e.g. an idle mob before
    /// pressing F) keeps the yellow attention-getting color.
    #[test]
    fn unengaged_target_uses_yellow() {
        assert_eq!(target_ring_color(false), ARROW_COLOR);
    }

    /// Yellow and red must remain visually distinct — if a future
    /// refactor accidentally points them at the same constant, the "in
    /// combat" UI cue collapses silently.
    #[test]
    fn engaged_and_unengaged_colors_differ() {
        assert_ne!(ARROW_COLOR, ARROW_ENGAGED_COLOR);
    }

    /// The bob is a bounded oscillation centered on zero: never pushes the
    /// arrow more than the amplitude in either direction, so the tip can't
    /// drift into the nameplate or float away.
    #[test]
    fn bob_is_bounded_by_amplitude() {
        for i in 0..64 {
            let s = i as f32 * 0.1;
            assert!(arrow_bob_offset(s).abs() <= ARROW_BOB_AMPLITUDE + 1e-6);
        }
    }

    /// Bob crosses zero at t=0 so the arrow starts at its rest height the
    /// instant a target is acquired (no first-frame jump).
    #[test]
    fn bob_starts_at_rest() {
        assert!(arrow_bob_offset(0.0).abs() < 1e-6);
    }
}
