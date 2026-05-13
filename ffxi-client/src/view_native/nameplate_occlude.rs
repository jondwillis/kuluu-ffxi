//! Hide nameplate UI nodes whose owning entity is occluded by zone
//! geometry. Without this the chat-HUD ends up looking like a screen
//! full of names from NPCs the operator can't actually see — every
//! Auction House counter, every retainer, every Home Point on the far
//! side of a building.
//!
//! Approach: piggyback on Detour's `slide_along` from `ffxi-nav-recast`.
//! For each frame, walk every nameplate, look up the owner's world
//! position, and slide along the navmesh from **player** → entity. If
//! the slid endpoint lands short of the entity (i.e., a wall edge
//! clamped the move), the entity is on the far side of geometry — hide
//! the nameplate. Otherwise show it.
//!
//! The slide originates at the player (not the camera) so it stays
//! correct after `camera_collision.rs` pulls the chase camera tight
//! against a wall — at that point the camera's planar projection can
//! land in the wrong navmesh poly and bogusly hide every nameplate. The
//! player position, by contrast, is always snapped to the navmesh.
//!
//! Limitations:
//! - 2D in the navmesh plane. Doesn't catch ceilings or floors —
//!   irrelevant for the dominant case (player and NPCs at ground level
//!   with walls between them).
//! - Skips when the navmesh is absent (still-loading or no-nav zones).
//!   The pre-occlusion state is "all nameplates visible," matching
//!   the prior behavior.

use bevy::prelude::*;
use ffxi_nav::glam;

use ffxi_viewer_core::components::{IsSelf, Nameplate, WorldEntity};

use super::navmesh_overlay::NavmeshState;

/// How close the slid endpoint has to land to the target before we
/// consider line-of-sight clear. 1 yalm matches FFXI's per-tick step
/// distance (~0.08 yalm × 12 tps + jitter), generous enough that we
/// don't false-negative when the entity is right next to a wall.
const REACHED_TOLERANCE_YALMS: f32 = 1.0;

/// Hide a `Nameplate` UI node when its owning `WorldEntity` is on the
/// other side of the navmesh from the camera. Scheduled in `Update`
/// after `update_nameplates_system` so it runs against the screen-
/// projected position for this frame.
///
/// Run-cost note: O(N_entities × log polys-along-segment). Detour's
/// `move_along_surface` caps visited polys at 16 internally and the
/// segments are short, so even 200+ entities in Jeuno cost <0.5 ms.
pub fn occlude_nameplates_system(
    nav: Res<NavmeshState>,
    self_q: Query<&Transform, (With<IsSelf>, Without<WorldEntity>)>,
    world_q: Query<(&Transform, &WorldEntity), Without<Nameplate>>,
    mut nameplate_q: Query<(&Nameplate, &mut Visibility)>,
) {
    let Some(nav_lock) = nav.nav.as_ref() else {
        // No navmesh — leave nameplates as-is so this system never
        // turns into a regression on zones without nav data.
        return;
    };
    // Anchor the visibility slide at the **player**, not the camera.
    // Once the chase camera collides with a wall and clamps inward, the
    // camera's planar projection can land in an unintended poly (or the
    // wrong side of a wall edge). The player is always snapped to the
    // navmesh, so anchoring here keeps `slide_along` honest. It also
    // matches operator intent — nameplates indicate "in your character's
    // awareness," which is reliably about the player's position.
    let Ok(self_t) = self_q.single() else { return };
    let Ok(guard) = nav_lock.lock() else { return };

    // Same Bevy → FFXI z-up flip as `camera_collision.rs`.
    let to_ffxi = |b: Vec3| glam::Vec3::new(b.x, -b.z, -b.y);

    // Build an entity_id → world-pos lookup once per frame.
    let mut pos_by_id: std::collections::HashMap<u32, Vec3> =
        std::collections::HashMap::with_capacity(world_q.iter().len());
    for (t, w) in &world_q {
        pos_by_id.insert(w.id, t.translation);
    }

    let self_ffxi = to_ffxi(self_t.translation);

    for (np, mut vis) in &mut nameplate_q {
        let Some(&entity_pos_bevy) = pos_by_id.get(&np.entity_id) else {
            continue;
        };
        let entity_ffxi = to_ffxi(entity_pos_bevy);
        // Slide on the entity's height layer so Detour stays on the
        // right polys when origin and target sit on different floors.
        let origin = glam::Vec3::new(self_ffxi.x, self_ffxi.y, entity_ffxi.z);
        let target = glam::Vec3::new(entity_ffxi.x, entity_ffxi.y, entity_ffxi.z);
        let want = match guard.slide_along(origin, target) {
            Some(slid) => {
                let dx = slid.x - entity_ffxi.x;
                let dy = slid.y - entity_ffxi.y;
                let reached = (dx * dx + dy * dy).sqrt() <= REACHED_TOLERANCE_YALMS;
                if reached {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                }
            }
            // Player position not on any nearby poly (briefly
            // off-mesh during a teleport / fall). Fail-open so we
            // don't blank every nameplate during a transient.
            None => Visibility::Inherited,
        };
        if *vis != want {
            *vis = want;
        }
    }
}
