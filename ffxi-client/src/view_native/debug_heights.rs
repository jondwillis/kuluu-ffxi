//! `/debug heights` slash command: diagnostic for the snap pipeline.
//! Prints, at the current player XZ:
//!
//! - **Server pos**: `snapshot.self_pos` from the wire (FFXI Y-down).
//! - **Player Bevy y**: where the player entity transform actually sits.
//! - **Player feet bevy y**: `player.y − visual_root_offset(...)` — the
//!   point where the snap *should* be putting the feet.
//! - **Navmesh height (Bevy)**: result of `nearest_height_at`, negated
//!   into Bevy frame (matching `snap_entities_to_navmesh_system`).
//! - **MZB collision Bevy y**: floor-filtered raycast against
//!   [`MzbCollisionGeometry`]. None when the player is off-mesh.
//! - **Top-5 MZB hits**: every triangle the downward raycast hits at
//!   the player's XZ, sorted descending by `hit_y`, annotated with
//!   each tri's normal.y (so a rejection by `FLOOR_NORMAL_MIN` is
//!   visible). Reveals when the snap could lock onto a rooftop /
//!   eave / vertical wall tri instead of the real floor.
//! - **BakedActor min_mesh_y**: the lowest local-y any baked vertex
//!   on the player reaches, derived during the VOS2 bake. Compare
//!   against `visual_root_offset`'s `-0.9` constant to spot a race
//!   for which the hip-to-foot estimate is wrong.
//!
//! Deltas:
//! - `feet − mzb`: the literal "how many yalms am I floating above
//!   the floor" number. Tight to zero = snap is working.
//! - `nav − mzb`: how far the two surfaces disagree.

use bevy::prelude::*;

use ffxi_viewer_core::components::{IsSelf, WorldEntity};
use ffxi_viewer_core::dat_mzb::{MzbCollisionGeometry, FLOOR_NORMAL_MIN};
use ffxi_viewer_core::scene::{visual_root_offset, BakedActor};
use ffxi_viewer_core::snapshot::SceneState;
use ffxi_viewer_wire::{ChatChannel, ChatLine};

use super::navmesh_overlay::NavmeshState;

/// Fired by the slash-command dispatcher; consumed by
/// [`process_debug_heights`].
#[derive(Message, Debug, Clone, Copy)]
pub struct DebugHeightsRequest;

/// Consumer for [`DebugHeightsRequest`]. Reads all the candidates and
/// pushes a multi-line system-toast into `SceneState`.
pub fn process_debug_heights(
    mut events: MessageReader<DebugHeightsRequest>,
    nav: Res<NavmeshState>,
    collision_geom: Res<MzbCollisionGeometry>,
    self_q: Query<(&Transform, &WorldEntity, Option<&BakedActor>), With<IsSelf>>,
    mut scene_state: ResMut<SceneState>,
) {
    if events.is_empty() {
        return;
    }
    for _ in events.read() {
        let server_pos = scene_state.snapshot.self_pos.pos;
        let (player_t, player_w, baked) = match self_q.single() {
            Ok((t, w, b)) => (*t, *w, b.copied()),
            Err(_) => {
                push(
                    &mut scene_state,
                    "/debug heights: no IsSelf entity yet".into(),
                );
                continue;
            }
        };
        let has_baked = baked.is_some();
        let offset = visual_root_offset(player_w.kind, has_baked);
        let feet_y = player_t.translation.y - offset;

        let nav_h_bevy: Option<f32> = nav.nav.as_ref().and_then(|lock| {
            let guard = lock.lock().ok()?;
            // Match `snap_entities_to_navmesh_system`'s convention:
            // ffxi_x = bevy.x, ffxi_y = -bevy.z; the cached/fallback
            // z_hint is `-player.y` (FFXI native).
            let ffxi_x = player_t.translation.x;
            let ffxi_y = -player_t.translation.z;
            let z_hint = -player_t.translation.y;
            let h = guard.nearest_height_at(ffxi_x, ffxi_y, z_hint)?;
            // `nearest_height_at` returns FFXI-native height (Y-down).
            // Negate to land in Bevy Y-up.
            Some(-h)
        });

        let player_xz = Vec2::new(player_t.translation.x, player_t.translation.z);
        let ranked_hits = collision_geom.ground_raycast_all(player_xz);
        // What the snap actually picks: floor-filtered + ceiling-bounded.
        // Mirror the snap's `STEP_TOLERANCE = 2.0` so this row matches.
        const STEP_TOLERANCE: f32 = 2.0;
        let ceiling_y = player_t.translation.y + STEP_TOLERANCE;
        let mzb_h_bevy = collision_geom.ground_raycast(player_xz, ceiling_y);

        let server_line = format!(
            "/debug heights — server pos: x={:.2} y={:.2} z={:.2}",
            server_pos.x, server_pos.y, server_pos.z,
        );
        let player_line = format!(
            "  player bevy: x={:.2} y={:.2} z={:.2}  (offset={:.2}, baked={})",
            player_t.translation.x,
            player_t.translation.y,
            player_t.translation.z,
            offset,
            has_baked,
        );
        let feet_line = format!("  player feet bevy y = {:.2}", feet_y);
        let nav_line = match nav_h_bevy {
            Some(h) => format!(
                "  navmesh bevy y = {:.2}  (delta_to_feet = {:+.2})",
                h,
                feet_y - h
            ),
            None => "  navmesh bevy y = <off-mesh / no nav>".to_string(),
        };
        let mzb_line = match mzb_h_bevy {
            Some(h) => format!(
                "  mzb_floor bevy y = {:.2}  (delta_to_feet = {:+.2})",
                h,
                feet_y - h
            ),
            None => "  mzb_floor bevy y = <no walkable tri below ceiling>".to_string(),
        };
        let nav_mzb_line = match (nav_h_bevy, mzb_h_bevy) {
            (Some(n), Some(m)) => format!("  >> nav − mzb_floor = {:+.2} yalms", n - m),
            _ => "  >> nav − mzb_floor = <missing>".to_string(),
        };
        let baked_line = match baked {
            Some(b) => format!(
                "  baked actor min_mesh_y = {:.2}  (vs visual_root_offset = -{:.2})",
                b.min_mesh_y, offset,
            ),
            None => "  baked actor min_mesh_y = <not baked yet>".to_string(),
        };

        push(&mut scene_state, server_line);
        push(&mut scene_state, player_line);
        push(&mut scene_state, feet_line);
        push(&mut scene_state, nav_line);
        push(&mut scene_state, mzb_line);
        push(&mut scene_state, nav_mzb_line);
        push(&mut scene_state, baked_line);

        // Top-5 MZB hits with normal annotations — surfaces *why* the
        // snap chose a given tri (or rejected the actual floor).
        if ranked_hits.is_empty() {
            push(
                &mut scene_state,
                "  mzb hits @ player xz: <none>".to_string(),
            );
        } else {
            push(
                &mut scene_state,
                format!(
                    "  mzb hits @ player xz (top {}, ceiling={:.2}, floor_min={:.2}):",
                    ranked_hits.len().min(5),
                    ceiling_y,
                    FLOOR_NORMAL_MIN,
                ),
            );
            for (i, (hit_y, normal)) in ranked_hits.iter().take(5).enumerate() {
                let walkable = normal.y >= FLOOR_NORMAL_MIN;
                let within_ceiling = *hit_y <= ceiling_y;
                let tag = match (walkable, within_ceiling) {
                    (true, true) => "FLOOR",
                    (true, false) => "above",
                    (false, true) => "wall/ceiling",
                    (false, false) => "above wall/ceiling",
                };
                push(
                    &mut scene_state,
                    format!(
                        "    #{}: y={:+.2} n.y={:+.2}  [{}]",
                        i + 1,
                        hit_y,
                        normal.y,
                        tag,
                    ),
                );
            }
        }
    }
}

fn push(scene_state: &mut SceneState, text: String) {
    scene_state.push_local_toast(ChatLine {
        channel: ChatChannel::Debug,
        sender: "client".into(),
        text,
        server_ts: 0,
        local_seq: 0,
    });
}
