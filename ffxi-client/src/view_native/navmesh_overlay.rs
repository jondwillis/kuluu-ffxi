use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use ffxi_viewer_core::snapshot::ToastEvent;
use ffxi_viewer_core::{InputMode, SceneState, WorldEntity};

use super::AppPhase;

pub struct NavmeshOverlayPlugin;

impl Plugin for NavmeshOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NavmeshOverlayVisible>()
            .init_resource::<NavmeshState>()
            .add_systems(
                Update,
                (
                    swap_navmesh_on_zone_change,
                    toggle_navmesh_overlay,
                    draw_navmesh_overlay.run_if(overlay_visible),
                )
                    .run_if(in_state(AppPhase::InGame)),
            )
            .add_systems(
                Update,
                snap_entities_to_mzb_floor_system
                    .after(ffxi_viewer_core::sync_entities_system)
                    .after(ffxi_viewer_core::combat_stance::predict_entities_system)
                    .before(ffxi_viewer_core::chase_camera_system)
                    .run_if(in_state(AppPhase::InGame)),
            );
    }
}

#[derive(Resource, Default)]
pub struct NavmeshOverlayVisible(pub bool);

#[derive(Resource, Default)]
pub struct NavmeshState {
    pub nav: Option<Arc<Mutex<ffxi_nav_recast::RecastNavMesh>>>,

    pub edges: Vec<([f32; 3], [f32; 3])>,

    pub zone_id: Option<u16>,

    pub in_myroom: bool,
}

fn overlay_visible(visible: Res<NavmeshOverlayVisible>) -> bool {
    visible.0
}

fn toggle_navmesh_overlay(
    keys: Res<ButtonInput<KeyCode>>,
    mode: Res<InputMode>,
    mut visible: ResMut<NavmeshOverlayVisible>,
    mut toasts: MessageWriter<ToastEvent>,
) {
    if !matches!(*mode, InputMode::World) {
        return;
    }
    if !keys.just_pressed(KeyCode::KeyN) {
        return;
    }
    visible.0 = !visible.0;
    let msg = if visible.0 {
        "navmesh overlay: ON".to_string()
    } else {
        "navmesh overlay: OFF".to_string()
    };
    toasts.write(ToastEvent::debug(msg));
}

fn swap_navmesh_on_zone_change(scene: Res<SceneState>, mut state: ResMut<NavmeshState>) {
    let zone_id = scene.snapshot.zone_id;
    let in_myroom = scene.snapshot.myroom.is_some();
    if state.zone_id == zone_id && state.in_myroom == in_myroom {
        return;
    }
    state.zone_id = zone_id;
    state.in_myroom = in_myroom;
    state.edges.clear();
    state.nav = None;

    let Some(zone) = zone_id else {
        return;
    };

    // LSB navmeshes cover the surrounding city, not the Mog House interior model
    // (the zone id stays the city's inside the MH) — re-grounding against the
    // city mesh teleports the player onto the interior model's exterior shell.
    if in_myroom {
        tracing::debug!(zone_id = zone, "in Mog House — city navmesh off");
        return;
    }

    match ffxi_nav_recast::RecastNavMesh::for_zone(zone) {
        Ok(nav) => {
            state.edges = nav.polygon_edges_detour();
            state.nav = Some(Arc::new(Mutex::new(nav)));
            tracing::info!(
                zone_id = zone,
                edge_count = state.edges.len(),
                "navmesh: loaded for overlay + wall-slide"
            );
        }
        Err(ffxi_nav_recast::LoadError::NotAvailable(_)) => {
            tracing::debug!(zone_id = zone, "no navmesh upstream — wall-slide off");
        }
        Err(e) => {
            tracing::warn!(zone_id = zone, error = %e, "navmesh load failed");
        }
    }
}

fn draw_navmesh_overlay(mut gizmos: Gizmos, state: Res<NavmeshState>) {
    let color = Color::srgba(0.25, 1.0, 0.40, 0.75);

    let lift_bevy_y = 0.05;
    for (a, b) in &state.edges {
        let pa = detour_to_bevy(*a) + Vec3::Y * lift_bevy_y;
        let pb = detour_to_bevy(*b) + Vec3::Y * lift_bevy_y;
        gizmos.line(pa, pb, color);
    }
}

fn snap_entities_to_mzb_floor_system(
    collision_geom: Res<ffxi_viewer_core::dat_mzb::MzbCollisionGeometry>,
    scene: Res<SceneState>,
    mut q: Query<(
        &WorldEntity,
        &mut Transform,
        Has<ffxi_viewer_core::components::IsSelf>,
    )>,
) {
    if collision_geom.tri_count() == 0 {
        let wire_self_y = ffxi_viewer_core::ffxi_to_bevy(scene.snapshot.self_pos.pos).y;
        for (_we, mut t, is_self) in q.iter_mut() {
            if is_self {
                t.translation.y = wire_self_y;
            }
        }
        return;
    }
    // The server sends pathing NPCs a fixed reference Y (e.g. a flat -50.0) and
    // relies on the client to snap them to terrain, so ground every entity, not
    // just self — otherwise NPCs float wherever the ground differs from that Y.
    // Snap to the nearest floor (not "highest below Y + tolerance"): a one-sided
    // cutoff sat right where the server reference Y and the MZB floor disagree,
    // so sub-unit per-frame Y wobble flipped the entity in and out of the cutoff
    // and the body bobbed every frame.
    for (_we, mut t, _is_self) in q.iter_mut() {
        if let Some(ground) = collision_geom
            .ground_nearest(Vec2::new(t.translation.x, t.translation.z), t.translation.y)
        {
            t.translation.y = ground;
        }
    }
}

#[inline]
fn detour_to_bevy(d: [f32; 3]) -> Vec3 {
    Vec3::new(d[0], d[1], d[2])
}
