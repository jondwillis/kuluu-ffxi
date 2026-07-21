use bevy::prelude::*;

use ffxi_viewer_core::components::{IsSelf, WorldEntity};
use ffxi_viewer_core::dat_mzb::{MzbCollisionGeometry, FLOOR_NORMAL_MIN};
use ffxi_viewer_core::scene::BakedActor;
use ffxi_viewer_core::snapshot::SceneState;

use super::navmesh_overlay::NavmeshState;

#[derive(Message, Debug, Clone, Copy)]
pub struct DebugHeightsRequest;

/// Focus-less GUI driving (kuluu-0pof): a socket `debug_heights` command bumps
/// the shared handle's seq; fire the request when it changes.
pub fn trigger_debug_heights_from_socket(
    handle: Option<Res<super::DebugControlHandle>>,
    mut last_seq: Local<u64>,
    mut requests: MessageWriter<DebugHeightsRequest>,
) {
    let Some(handle) = handle else {
        return;
    };
    let Ok(ctrl) = handle.0.lock() else {
        return;
    };
    let seq = ctrl.heights_seq();
    if seq != *last_seq {
        *last_seq = seq;
        requests.write(DebugHeightsRequest);
    }
}

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
        let _ = player_w;
        let has_baked = baked.is_some();

        let nav_h_bevy: Option<f32> = nav.nav.as_ref().and_then(|lock| {
            let guard = lock.lock().ok()?;
            let ffxi_x = player_t.translation.x;
            let ffxi_y = -player_t.translation.z;
            let z_hint = -player_t.translation.y;
            let h = guard.nearest_height_at(ffxi_x, ffxi_y, z_hint)?;

            Some(-h)
        });

        let player_xz = Vec2::new(player_t.translation.x, player_t.translation.z);
        let ranked_hits = collision_geom.ground_raycast_all(player_xz);

        const STEP_TOLERANCE: f32 = 2.0;
        let ceiling_y = player_t.translation.y + STEP_TOLERANCE;
        let mzb_h_bevy = collision_geom.ground_raycast(player_xz, ceiling_y);

        let server_line = format!(
            "/debug heights — server pos: x={:.2} y={:.2} z={:.2}",
            server_pos.x, server_pos.y, server_pos.z,
        );
        let player_line = format!(
            "  player bevy (feet): x={:.2} y={:.2} z={:.2}  (baked={})",
            player_t.translation.x, player_t.translation.y, player_t.translation.z, has_baked,
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

        // Focus-less GUI driving (kuluu-0pof): mirror the grounding numbers to the
        // log so a socket-triggered `debug_heights` is readable without a screenshot.
        tracing::info!(
            target: "debug_heights",
            server_x = server_pos.x, server_y = server_pos.y, server_z = server_pos.z,
            player_bevy_y = player_t.translation.y,
            mzb_floor_bevy_y = mzb_h_bevy,
            nav_bevy_y = nav_h_bevy,
            "debug heights (server z=height; bevy y=-z)"
        );

        push(&mut toasts, server_line);
        push(&mut toasts, player_line);
        push(&mut toasts, mzb_line);
        push(&mut toasts, nav_line);
        push(&mut toasts, nav_mzb_line);
        push(&mut toasts, baked_line);

        // Nearest mob (the one you're likely fighting): the engage/melee range
        // and the HUD's "d=" are full 3D over wire coords, so a height gap
        // between our reported self Y and the mob's server Y inflates distance
        // and blocks melee even when horizontally adjacent. Also report how far
        // the mob's server Y sits above our rendered MZB floor (the "float").
        let nearest_mob = scene_state
            .snapshot
            .entities
            .iter()
            .filter(|e| matches!(e.kind, ffxi_viewer_wire::EntityKind::Mob))
            .min_by(|a, b| {
                let da = (a.pos.x - server_pos.x).hypot(a.pos.y - server_pos.y);
                let db = (b.pos.x - server_pos.x).hypot(b.pos.y - server_pos.y);
                da.total_cmp(&db)
            });
        match nearest_mob {
            None => push(&mut toasts, "  nearest mob: <none in range>".into()),
            Some(mob) => {
                let dx = mob.pos.x - server_pos.x;
                let dz_horiz = mob.pos.y - server_pos.y; // wire.y = loc.p.z (horizontal)
                let dy_height = mob.pos.z - server_pos.z; // wire.z = loc.p.y (height)
                let horiz = (dx * dx + dz_horiz * dz_horiz).sqrt();
                let full3d = (dx * dx + dz_horiz * dz_horiz + dy_height * dy_height).sqrt();
                let mob_bevy = ffxi_viewer_core::ffxi_to_bevy(mob.pos);
                let mob_mzb = collision_geom.ground_raycast(
                    Vec2::new(mob_bevy.x, mob_bevy.z),
                    mob_bevy.y + STEP_TOLERANCE,
                );
                let label = mob.name.as_deref().unwrap_or("<mob>");
                push(
                    &mut toasts,
                    format!(
                        "  nearest mob '{label}' wire: x={:.2} y={:.2} z(height)={:.2}",
                        mob.pos.x, mob.pos.y, mob.pos.z
                    ),
                );
                push(
                    &mut toasts,
                    format!(
                        "  self→mob: horiz={horiz:.2}  3D={full3d:.2}  Δheight(wire.z)={dy_height:+.2}",
                    ),
                );
                let mob_float = match mob_mzb {
                    Some(h) => format!(
                        "  mob bevy y={:.2}  mzb floor@mob={:.2}  floats {:+.2} over mesh",
                        mob_bevy.y,
                        h,
                        mob_bevy.y - h
                    ),
                    None => format!(
                        "  mob bevy y={:.2}  mzb floor@mob=<no walkable tri>",
                        mob_bevy.y
                    ),
                };
                push(&mut toasts, mob_float);

                let nav_at_mob_bevy: Option<f32> = nav.nav.as_ref().and_then(|lock| {
                    let guard = lock.lock().ok()?;
                    let h = guard.nearest_height_at(mob.pos.x, mob.pos.y, mob.pos.z)?;
                    Some(-h)
                });
                let nav_mob_line = match nav_at_mob_bevy {
                    Some(n) => format!(
                        "  navmesh@mob bevy y={n:.2}  (nav−server mob={:+.2}, nav−mzb@mob={})",
                        n - mob_bevy.y,
                        match mob_mzb {
                            Some(m) => format!("{:+.2}", n - m),
                            None => "<none>".into(),
                        }
                    ),
                    None => "  navmesh@mob = <off-mesh / no nav>".into(),
                };
                push(&mut toasts, nav_mob_line);
            }
        }

        if ranked_hits.is_empty() {
            push(&mut toasts, "  mzb hits @ player xz: <none>".to_string());
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

fn push(toasts: &mut MessageWriter<ffxi_viewer_core::snapshot::ToastEvent>, text: String) {
    toasts.write(ffxi_viewer_core::snapshot::ToastEvent::debug(text));
}
