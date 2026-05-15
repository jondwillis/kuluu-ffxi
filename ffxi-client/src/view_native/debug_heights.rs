//! `/debug heights` slash command: diagnostic for the navmesh-vs-MZB
//! vertical offset. Prints, at the current player XZ:
//!
//! - **Server pos**: `snapshot.self_pos` from the wire (FFXI Y-down).
//! - **Player Bevy y**: where the player capsule actually sits.
//! - **Navmesh height (Bevy)**: result of `nearest_height_at`, negated
//!   into Bevy frame (matching `snap_entities_to_navmesh_system`).
//! - **MZB collision Bevy y**: downward ray-hit against the merged
//!   `MzbCollisionMesh`. None when the player is off-mesh.
//!
//! Deltas:
//! - `nav - mzb` ≠ 0 means the navmesh and MZB don't share a Y plane
//!   (the visual gap the user reports).
//! - `player - nav` ≈ feet_offset (~0–2 yalms) is expected: the snap
//!   system lifts the capsule center so feet land on the navmesh.
//! - `player - mzb` is what really matters for "is the player on the
//!   ground?" If this is large positive, the player floats; large
//!   negative, the player sinks.
//!
//! Uses a hand-rolled Möller–Trumbore intersection (no parry3d
//! dependency yet — see Stream C3 of the plan).

use bevy::prelude::*;
use ffxi_nav::glam;

use ffxi_viewer_core::components::IsSelf;
use ffxi_viewer_core::dat_mzb::MzbCollisionMesh;
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
    self_q: Query<&Transform, With<IsSelf>>,
    mzb_q: Query<(&Mesh3d, &GlobalTransform), With<MzbCollisionMesh>>,
    meshes: Res<Assets<Mesh>>,
    mut scene_state: ResMut<SceneState>,
) {
    if events.is_empty() {
        return;
    }
    for _ in events.read() {
        let server_pos = scene_state.snapshot.self_pos.pos;
        let player_t = match self_q.single() {
            Ok(t) => *t,
            Err(_) => {
                push(
                    &mut scene_state,
                    "/debug heights: no IsSelf entity yet".into(),
                );
                continue;
            }
        };

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

        let mzb_h_bevy = downward_raycast_against_collision(&player_t.translation, &mzb_q, &meshes);

        let server_line = format!(
            "/debug heights — server pos: x={:.2} y={:.2} z={:.2}",
            server_pos.x, server_pos.y, server_pos.z,
        );
        let player_line = format!(
            "  player bevy: x={:.2} y={:.2} z={:.2}",
            player_t.translation.x, player_t.translation.y, player_t.translation.z,
        );
        let nav_line = match nav_h_bevy {
            Some(h) => format!(
                "  navmesh bevy y = {:.2}  (delta_to_player = {:+.2})",
                h,
                player_t.translation.y - h
            ),
            None => "  navmesh bevy y = <off-mesh / no nav>".to_string(),
        };
        let mzb_line = match mzb_h_bevy {
            Some(h) => format!(
                "  mzb_collision bevy y = {:.2}  (delta_to_player = {:+.2})",
                h,
                player_t.translation.y - h
            ),
            None => "  mzb_collision bevy y = <no triangle below>".to_string(),
        };
        let nav_mzb_line = match (nav_h_bevy, mzb_h_bevy) {
            (Some(n), Some(m)) => format!("  >> nav − mzb = {:+.2} yalms", n - m),
            _ => "  >> nav − mzb = <missing>".to_string(),
        };

        for line in [server_line, player_line, nav_line, mzb_line, nav_mzb_line] {
            push(&mut scene_state, line);
        }
    }
}

fn push(scene_state: &mut SceneState, text: String) {
    scene_state.push_local_toast(ChatLine {
        channel: ChatChannel::System,
        sender: "client".into(),
        text,
        server_ts: 0,
    });
}

/// Cast a ray straight down (Bevy −Y) from a point well above the
/// player and return the **highest** Y of any triangle the ray hits in
/// the merged collision mesh, transformed by the mesh's `GlobalTransform`.
///
/// "Highest hit" picks the ceiling-of-floor — i.e., if the player is
/// inside a multi-level building, we want the floor they're standing on,
/// not the basement. The ray starts above and travels down; the first
/// hit (largest Y) is closest to the camera-down convention.
fn downward_raycast_against_collision(
    player_bevy: &Vec3,
    mzb_q: &Query<(&Mesh3d, &GlobalTransform), With<MzbCollisionMesh>>,
    meshes: &Assets<Mesh>,
) -> Option<f32> {
    // Start the ray well above any sane FFXI building height.
    let ray_origin = Vec3::new(player_bevy.x, 1000.0, player_bevy.z);
    let ray_dir = Vec3::new(0.0, -1.0, 0.0);

    let mut best_y: Option<f32> = None;

    for (mesh3d, global) in mzb_q.iter() {
        let Some(mesh) = meshes.get(mesh3d.0.id()) else {
            continue;
        };
        let Some(positions) = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|a| a.as_float3())
        else {
            continue;
        };
        let Some(indices) = mesh.indices() else {
            continue;
        };
        let xform = global.to_matrix();

        let mut iter = indices.iter();
        while let (Some(i0), Some(i1), Some(i2)) = (iter.next(), iter.next(), iter.next()) {
            let v0 = xform.transform_point3(Vec3::from_array(positions[i0]));
            let v1 = xform.transform_point3(Vec3::from_array(positions[i1]));
            let v2 = xform.transform_point3(Vec3::from_array(positions[i2]));
            if let Some(t) = ray_tri_intersect(ray_origin, ray_dir, v0, v1, v2) {
                let hit_y = ray_origin.y + t * ray_dir.y;
                best_y = Some(match best_y {
                    Some(prev) if prev > hit_y => prev,
                    _ => hit_y,
                });
            }
        }
    }

    best_y
}

/// Möller–Trumbore. Returns ray-parameter `t` (≥ 0) of the intersection,
/// or `None` if the ray misses the triangle.
fn ray_tri_intersect(orig: Vec3, dir: Vec3, v0: Vec3, v1: Vec3, v2: Vec3) -> Option<f32> {
    const EPS: f32 = 1e-7;
    let e1 = v1 - v0;
    let e2 = v2 - v0;
    let h = glam::Vec3::new(dir.x, dir.y, dir.z).cross(glam::Vec3::new(e2.x, e2.y, e2.z));
    let a = glam::Vec3::new(e1.x, e1.y, e1.z).dot(h);
    if a.abs() < EPS {
        return None;
    }
    let f = 1.0 / a;
    let s = orig - v0;
    let s_g = glam::Vec3::new(s.x, s.y, s.z);
    let u = f * s_g.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s_g.cross(glam::Vec3::new(e1.x, e1.y, e1.z));
    let v = f * glam::Vec3::new(dir.x, dir.y, dir.z).dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = f * glam::Vec3::new(e2.x, e2.y, e2.z).dot(q);
    if t > EPS {
        Some(t)
    } else {
        None
    }
}
