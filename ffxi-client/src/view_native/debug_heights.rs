//! `/debug heights` slash command: diagnostic for the MZB-floor snap.
//! Prints, at the current player XZ:
//!
//! - **Server pos**: `snapshot.self_pos` from the wire (FFXI Y-down).
//! - **Player Bevy y**: where the player entity transform sits. After
//!   the feet-at-origin refactor this *is* the feet position.
//! - **MZB collision Bevy y**: floor-filtered raycast against
//!   [`MzbCollisionGeometry`]. None when the player is off-mesh.
//!   Equal to player.y when the snap is doing its job.
//! - **Navmesh height (Bevy)**: result of `nearest_height_at`. The
//!   snap doesn't use this anymore (navmesh is reactor-only), but
//!   it's logged so we can see when the two surfaces disagree
//!   without ambiguity about which one is authoritative.
//! - **Top-5 MZB hits**: every triangle the downward raycast hits at
//!   the player's XZ, sorted descending by `hit_y`, annotated with
//!   each tri's normal.y (so a rejection by `FLOOR_NORMAL_MIN` is
//!   visible). Reveals when the snap could lock onto an above-ceiling
//!   tri instead of the real floor.
//! - **BakedActor min_mesh_y / actor_height**: the empirical bake
//!   extent. The pivot/mesh spawn transform already absorbs
//!   `-min_mesh_y` so the snap doesn't read these; they're surfaced
//!   for debugging nameplate / camera anchor drift across races.
//!
//! Deltas:
//! - `player − mzb`: should be ~0 when the snap is working. The snap
//!   sets `transform.y = ground` directly (no offset).
//! - `nav − mzb`: gap between the pathing surface and the collision
//!   surface. Pure diagnostic — neither side feeds the snap anymore.

use bevy::prelude::*;

use ffxi_viewer_core::components::{IsSelf, WorldEntity};
use ffxi_viewer_core::dat_mzb::{MzbCollisionGeometry, FLOOR_NORMAL_MIN};
use ffxi_viewer_core::scene::BakedActor;
use ffxi_viewer_core::snapshot::SceneState;

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
    scene_state: Res<SceneState>,
    mut toasts: MessageWriter<ffxi_viewer_core::snapshot::ToastEvent>,
) {
    if events.is_empty() {
        return;
    }
    for _ in events.read() {
        let server_pos = scene_state.snapshot.self_pos.pos;
        let (player_t, player_w, baked) = match self_q.single() {
            Ok((t, w, b)) => (*t, *w, b.copied()),
            Err(_) => {
                push(&mut toasts, "/debug heights: no IsSelf entity yet".into());
                continue;
            }
        };
        let _ = player_w; // kind no longer drives the diagnostic
        let has_baked = baked.is_some();

        let nav_h_bevy: Option<f32> = nav.nav.as_ref().and_then(|lock| {
            let guard = lock.lock().ok()?;
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
        // Mirror the snap's `STEP_TOLERANCE = 2.0` so the picked hit
        // matches what the snap would land on.
        const STEP_TOLERANCE: f32 = 2.0;
        let ceiling_y = player_t.translation.y + STEP_TOLERANCE;
        let mzb_h_bevy = collision_geom.ground_raycast(player_xz, ceiling_y);

        let server_line = format!(
            "/debug heights — server pos: x={:.2} y={:.2} z={:.2}",
            server_pos.x, server_pos.y, server_pos.z,
        );
        let player_line = format!(
            "  player bevy (feet): x={:.2} y={:.2} z={:.2}  (baked={})",
            player_t.translation.x,
            player_t.translation.y,
            player_t.translation.z,
            has_baked,
        );
        let mzb_line = match mzb_h_bevy {
            Some(h) => format!(
                "  mzb_floor bevy y = {:.2}  (player − mzb = {:+.2})",
                h,
                player_t.translation.y - h
            ),
            None => "  mzb_floor bevy y = <no walkable tri below ceiling>".to_string(),
        };
        let nav_line = match nav_h_bevy {
            Some(h) => format!(
                "  navmesh bevy y = {:.2}  (player − nav = {:+.2})  [pathing only]",
                h,
                player_t.translation.y - h
            ),
            None => "  navmesh bevy y = <off-mesh / no nav>".to_string(),
        };
        let nav_mzb_line = match (nav_h_bevy, mzb_h_bevy) {
            (Some(n), Some(m)) => format!("  >> nav − mzb_floor = {:+.2} yalms", n - m),
            _ => "  >> nav − mzb_floor = <missing>".to_string(),
        };
        let baked_line = match baked {
            Some(b) => format!(
                "  baked actor: min_mesh_y = {:.2}  actor_height = {:.2}",
                b.min_mesh_y, b.actor_height,
            ),
            None => "  baked actor: <not loaded yet — capsule placeholder>".to_string(),
        };

        push(&mut toasts,server_line);
        push(&mut toasts,player_line);
        push(&mut toasts,mzb_line);
        push(&mut toasts,nav_line);
        push(&mut toasts,nav_mzb_line);
        push(&mut toasts,baked_line);

        // Top-5 MZB hits with normal annotations — surfaces *why* the
        // snap chose a given tri (or rejected the actual floor).
        if ranked_hits.is_empty() {
            push(
                &mut toasts,
                "  mzb hits @ player xz: <none>".to_string(),
            );
        } else {
            push(
                &mut toasts,
                format!(
                    "  mzb hits @ player xz (top {}, ceiling={:.2}, floor_min={:.2}):",
                    ranked_hits.len().min(5),
                    ceiling_y,
                    FLOOR_NORMAL_MIN,
                ),
            );
            for (i, (hit_y, normal)) in ranked_hits.iter().take(5).enumerate() {
                let walkable = normal.y.abs() >= FLOOR_NORMAL_MIN;
                let within_ceiling = *hit_y <= ceiling_y;
                let tag = match (walkable, within_ceiling) {
                    (true, true) => "FLOOR",
                    (true, false) => "above-ceiling",
                    (false, true) => "wall",
                    (false, false) => "above-wall",
                };
                push(
                    &mut toasts,
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

fn push(
    toasts: &mut MessageWriter<ffxi_viewer_core::snapshot::ToastEvent>,
    text: String,
) {
    toasts.write(ffxi_viewer_core::snapshot::ToastEvent::debug(text));
}
