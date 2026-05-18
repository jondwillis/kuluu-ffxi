//! Debug overlay: draw the current zone's Recast/Detour navmesh as a
//! wireframe, toggled with F9.
//!
//! Native-only — `ffxi-nav-recast` pulls in a C++ FFI that wouldn't
//! compile on the wasm build. Plugin lives under `view_native/` rather
//! than `ffxi-viewer-core/` for that reason.
//!
//! ## Architecture
//!
//! Two pieces:
//!
//! 1. A **swap system** detects zone changes (by watching
//!    `SceneState.snapshot.zone_id`) and re-loads `RecastNavMesh` for
//!    the new zone, caching its `polygon_edges()` in a resource. Loads
//!    are cheap (~50 ms parse from disk after the first fetch) and
//!    happen at most once per zone-in.
//!
//! 2. A **draw system** runs every frame in `Update` and re-emits the
//!    edges via Bevy `Gizmos`. Toggling visibility is a one-bool
//!    decision — when off, the system early-returns, so there's no
//!    per-frame cost.
//!
//! ## Why gizmos?
//!
//! For ~10k–30k edges, gizmos' immediate-mode line emission is fast
//! enough and avoids the asset-management complexity of building a
//! `LineList` mesh. If we ever want this always-on, switch to a baked
//! mesh; for a "press F9 to debug coords" overlay, this is right-sized.
//!
//! ## Coord transform
//!
//! Rendering uses [`detour_to_bevy`] directly on raw Detour-space
//! verts from `polygon_edges_detour()`. We deliberately **don't** go
//! through `detour_to_ffxi` here — that's the path-finding round-trip
//! transform (an involution that's only required to be self-inverse,
//! not absolute). Rendering needs an absolute transform to land in
//! Bevy world coords; the two are different concerns and conflating
//! them produces a perpendicular overlay (which is exactly the bug
//! the first version had).
//!
//! Empirically, xiNavmeshes are stored in Detour-standard y-up
//! coords. Bevy is also y-up; the two differ only in z-handedness,
//! so the transform is just `bevy = (d.x, d.y, -d.z)`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use ffxi_viewer_core::{
    scene::{visual_root_offset, BakedActor},
    InputMode, IsSelf, SceneState, WorldEntity,
};

use super::AppPhase;

/// Plugin entry point — register from `view_native::mod::run`.
pub struct NavmeshOverlayPlugin;

impl Plugin for NavmeshOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NavmeshOverlayVisible>()
            .init_resource::<NavmeshState>()
            .init_resource::<SnapHeightCache>()
            .add_systems(
                Update,
                (
                    swap_navmesh_on_zone_change,
                    toggle_navmesh_overlay,
                    draw_navmesh_overlay.run_if(overlay_visible),
                )
                    .run_if(in_state(AppPhase::InGame)),
            )
            // Explicit ordering bracket:
            //   sync_entities → snap → chase_camera
            // Without `.before(chase_camera_system)`, Bevy's scheduler
            // is free to parallelize and may run the camera *before*
            // our snap on some frames and *after* on others. Different
            // y values reach the camera each frame → visible vertical
            // jitter (the symptom triggers on any input because input
            // events perturb the schedule order). With both
            // `.after(sync_entities)` and `.before(chase_camera)`, the
            // snap runs in a deterministic window every frame.
            .add_systems(
                Update,
                snap_entities_to_navmesh_system
                    .after(ffxi_viewer_core::sync_entities_system)
                    .before(ffxi_viewer_core::chase_camera_system)
                    .run_if(in_state(AppPhase::InGame)),
            );
    }
}

/// Per-entity cache of the last navmesh height we resolved. Used as
/// `z_hint` next frame instead of `t.translation.y`, which would
/// otherwise oscillate between local-predicted and server-echoed z
/// at high render fps and cause `find_nearest_poly_1` to flip between
/// adjacent polys (visible as tick-tock vertical jitter).
#[derive(Resource, Default)]
struct SnapHeightCache(HashMap<u32, f32>);

/// Toggle state. Default `false`: overlay is hidden until the first
/// F9 press.
#[derive(Resource, Default)]
pub struct NavmeshOverlayVisible(pub bool);

/// Per-zone navmesh state. Holds both the live `RecastNavMesh` (used
/// by the wall-slide collision system) and the pre-extracted edge
/// list (used by the overlay renderer). Both are populated on zone
/// change by [`swap_navmesh_on_zone_change`].
///
/// The `Arc<Mutex<>>` wrapper around the nav is needed because Bevy
/// resources require `Send + Sync` and `RecastNavMesh` is
/// `Send + !Sync` (`dtNavMeshQuery`'s internal state can't be touched
/// from multiple threads concurrently). In practice contention is
/// zero — only `dispatch_movement_system` (FixedUpdate) calls into the
/// nav, and Bevy's schedule serializes that against `Update`.
#[derive(Resource, Default)]
pub struct NavmeshState {
    /// Live nav for path-snapping queries. `None` if no navmesh is
    /// available for the current zone (xiNavmeshes coverage gap, or
    /// load failure — both fall back to PNG/straight-line).
    pub nav: Option<Arc<Mutex<ffxi_nav_recast::RecastNavMesh>>>,
    /// Polygon outline edges in **raw Detour space**, cached at
    /// load-time so the per-frame draw system doesn't re-walk every
    /// tile. The render-time `detour_to_bevy` transform projects them
    /// into Bevy world coords.
    pub edges: Vec<([f32; 3], [f32; 3])>,
    /// Zone the state was loaded for. `None` until the first load.
    /// Compared against the snapshot's `zone_id` to detect changes.
    pub zone_id: Option<u16>,
}

fn overlay_visible(visible: Res<NavmeshOverlayVisible>) -> bool {
    visible.0
}

/// `N` toggles the overlay. World-mode-only — so typing 'n' in a chat
/// buffer or menu doesn't flip it. The `/navmesh` slash command is
/// the path that works mid-chat (it's how you'd toggle while typing).
fn toggle_navmesh_overlay(
    keys: Res<ButtonInput<KeyCode>>,
    mode: Res<InputMode>,
    mut visible: ResMut<NavmeshOverlayVisible>,
    mut state: ResMut<SceneState>,
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
    state.push_local_toast(ffxi_viewer_wire::ChatLine {
        channel: ffxi_viewer_wire::ChatChannel::Debug,
        sender: "client".into(),
        text: msg,
        server_ts: 0,
        local_seq: 0,
    });
}

/// Re-load the navmesh whenever the snapshot's zone id changes.
/// Loading is deliberately blocking — the parse is ~50 ms after the
/// first fetch, well under one frame at 60 Hz, and zone-ins already
/// stall briefly while the floor texture swaps.
fn swap_navmesh_on_zone_change(scene: Res<SceneState>, mut state: ResMut<NavmeshState>) {
    let zone_id = scene.snapshot.zone_id;
    if state.zone_id == zone_id {
        return;
    }
    state.zone_id = zone_id;
    state.edges.clear();
    state.nav = None;

    let Some(zone) = zone_id else {
        return;
    };

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

/// Per-frame line emission. Runs in `Update` and is gated by
/// `overlay_visible` — when toggled off, this system never runs and
/// pays no per-frame cost.
///
/// **Visual style — adjust to taste:** color/elevation are right here
/// in 4-5 lines. Nothing else in the plugin assumes anything about
/// how the lines look.
fn draw_navmesh_overlay(mut gizmos: Gizmos, state: Res<NavmeshState>) {
    let color = Color::srgb(0.2, 1.0, 0.4);
    // 0.05 yalm above the navmesh-height so the gizmo isn't depth-fought
    // against the MZB collision mesh at the same Y. The previous 1.0
    // lift dated from the floor-plane-only era — operators saw the
    // wireframe "floating" several yalms above the rendered MZB walls
    // even though the navmesh data sat ~0.4 below the MZB (per
    // `/debug heights`). Drop the lift to a hairline so the two
    // surfaces visually agree.
    let lift_bevy_y = 0.05;
    for (a, b) in &state.edges {
        let pa = detour_to_bevy(*a) + Vec3::Y * lift_bevy_y;
        let pb = detour_to_bevy(*b) + Vec3::Y * lift_bevy_y;
        gizmos.line(pa, pb, color);
    }
}

/// Visual gravity: each frame, query the navmesh for the height at
/// each entity's 2D position and snap the rendered Y to it. Runs
/// **after** `sync_entities_system` populates transforms from the
/// wire snapshot, so we override server-reported height with navmesh
/// height — necessary when the server's `z` doesn't track terrain
/// (often the case for static NPCs which sit at a fixed `z` regardless
/// of where the placement engine put them on the actual ground).
///
/// ## Stable z_hint
///
/// `find_nearest_poly_1` searches a vertical box around `z_hint`. If
/// `z_hint` oscillates between two values (e.g., local-predicted vs
/// server-echoed self z, which interleave at high render fps), the
/// query can pick different adjacent polys whose heights differ
/// slightly — visible as tick-tock jitter. We avoid this by feeding
/// the *previous* snapped height back as the hint, cached per
/// entity. The first frame for an entity falls back to its current
/// rendered y; subsequent frames are stable.
///
/// ## Capsule feet offset
///
/// Server doesn't encode entity heights — capsules are client-side
/// placeholders. With the snap setting `bevy.y = navmesh_h`, the
/// capsule center sits *on* the navmesh and its feet are 1.9 yalms
/// below it (capsule radius + half-height). We add a per-kind feet
/// offset so the **feet** rest on the navmesh instead.
fn snap_entities_to_navmesh_system(
    state: Res<NavmeshState>,
    collision_geom: Res<ffxi_viewer_core::dat_mzb::MzbCollisionGeometry>,
    mut cache: ResMut<SnapHeightCache>,
    mut q: Query<(&WorldEntity, &mut Transform, Has<IsSelf>, Has<BakedActor>)>,
) {
    // Two-tier ground snap:
    //
    // - **Self entity** uses the MZB collision-mesh raycast. This is
    //   what the operator's eye is locked on — they notice if the
    //   player floats off the rendered floor by even 0.4 yalms.
    // - **All other entities** use the cheap Detour `nearest_height_at`
    //   query. The two surfaces differ by ~0.4 yalms (per `/debug heights`
    //   in Bastok); invisible at third-person NPC distance.
    //
    // Rationale: the linear MZB raycast scans ~5k tris per query
    // (sufficient for one entity per frame at 1.5% CPU budget), but
    // running it for every entity in a populated zone scales to
    // tens-of-millions of intersections per frame — the cause of the
    // FPS tank observed after the initial C3.1 rollout.
    let nav_guard = state.nav.as_ref().and_then(|n| n.lock().ok());
    let mzb_loaded = collision_geom.tri_count() > 0;
    if !mzb_loaded && nav_guard.is_none() {
        return;
    }

    for (entity, mut t, is_self, has_baked_mesh) in q.iter_mut() {
        let ground_y: Option<f32> = if is_self && mzb_loaded {
            // Player: MZB raycast. Fall back to navmesh if the
            // column at the player's XZ has no triangle below
            // (e.g., off the edge of the loaded zone bounds).
            //
            // `ceiling_y = current_y + STEP_TOLERANCE` so the snap
            // ignores overhead geometry the player is walking
            // *under* (gate tops, arches, eaves) but still allows
            // small step-ups onto curbs and ramps.
            const STEP_TOLERANCE: f32 = 2.0;
            let ceiling_y = t.translation.y + STEP_TOLERANCE;
            let mzb = collision_geom
                .ground_raycast(Vec2::new(t.translation.x, t.translation.z), ceiling_y);
            mzb.or_else(|| nav_y_bevy(&nav_guard, &mut cache, entity.id, &t))
        } else {
            // NPCs / mobs / other PCs: cheap navmesh query.
            nav_y_bevy(&nav_guard, &mut cache, entity.id, &t)
        };

        if let Some(ground) = ground_y {
            t.translation.y = ground + visual_root_offset(entity.kind, has_baked_mesh);
        }
    }
}

/// Navmesh `nearest_height_at` lookup, converted to Bevy Y-up. Returns
/// `None` when no navmesh is loaded or the query fails. Mirrors the
/// pre-C3.1 snap path verbatim so NPCs render identically to before.
fn nav_y_bevy(
    nav_guard: &Option<std::sync::MutexGuard<'_, ffxi_nav_recast::RecastNavMesh>>,
    cache: &mut SnapHeightCache,
    entity_id: u32,
    t: &Transform,
) -> Option<f32> {
    let g = nav_guard.as_ref()?;
    let ffxi_x = t.translation.x;
    let ffxi_y = -t.translation.z;
    let z_hint = cache.0.get(&entity_id).copied().unwrap_or(-t.translation.y);
    let h = g.nearest_height_at(ffxi_x, ffxi_y, z_hint)?;
    cache.0.insert(entity_id, h);
    Some(-h)
}

/// Detour-space → Bevy world.
///
/// FFXI / MZB / xiNavmesh all use Y-down (height grows toward negative);
/// Bevy is Y-up. LSB's `ToDetourPos` stores Detour `y = -ffxi_native_y`,
/// and FFXI native y is itself `-real_height`, so detour.y = +real_height.
/// To land at +real_height in Bevy (Y-up), pass through: `bevy.y = +d[1]`.
/// X stays as-is; Z passes through because it shares the horizontal
/// frame the snap uses (`ffxi_y = -bevy.z`).
#[inline]
fn detour_to_bevy(d: [f32; 3]) -> Vec3 {
    Vec3::new(d[0], d[1], d[2])
}
