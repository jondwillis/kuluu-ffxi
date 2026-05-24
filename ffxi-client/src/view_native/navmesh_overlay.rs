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

use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use ffxi_viewer_core::snapshot::ToastEvent;
use ffxi_viewer_core::{InputMode, SceneState, WorldEntity};

use super::AppPhase;

/// Plugin entry point — register from `view_native::mod::run`.
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
                snap_entities_to_mzb_floor_system
                    .after(ffxi_viewer_core::sync_entities_system)
                    .before(ffxi_viewer_core::chase_camera_system)
                    .run_if(in_state(AppPhase::InGame)),
            );
    }
}

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
    // Lime-green at 0.75 alpha. Translucent so dense navmesh tiles don't
    // hide the underlying MZB collision color, and pinned to the green
    // axis so the navmesh has a clear visual identity vs.
    // `/zonegeom camera` (cyan AABBs) and the chase-ray viz
    // (yellow/magenta segments).
    let color = Color::srgba(0.25, 1.0, 0.40, 0.75);
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

/// Visual gravity: each frame, query the MZB collision mesh for the
/// floor height under self's XZ and snap `transform.y` to it.
///
/// **Applies to self only.** Every non-self entity — Mob, other Pc,
/// Pet, static NPC, Other — is server-positioned; `sync_entities_system`
/// writes their wire Y directly and we leave it alone. Overriding any
/// of them to the local MZB floor would cause the rendered visual to
/// disagree with the server's range check (a mob visually "on the
/// ground" 5y away while the server sees it 15y up the hillside and
/// rejects attacks as out of range). Even static NPCs: their server
/// record is canonical, and silently rewriting their position to a
/// raycast result would move them between visits.
///
/// Self snaps because the player input pipeline writes X/Z but not Y,
/// so the snap is the sole writer of self's vertical position. On
/// zone-in, when MZB hasn't loaded yet, the snap falls back to wire Y
/// for self (see the `tri_count() == 0` branch below) — otherwise
/// self would stay at the *previous* zone's snap result and appear to
/// fall through the world.
///
/// Runs **after** `sync_entities_system` (which writes X/Z for self
/// and full XYZ for active non-self) and **before** `chase_camera_system`.
///
/// ## Why MZB only
///
/// The MZB collision mesh is the authoritative ground surface — it's
/// what the visible floor renders from (decorative tris filtered out
/// by the `is_collision` flag in `dat_mzb::process_load_mzb_requests`).
/// The xiNavmesh is purpose-built for **pathing**, not gravity: its
/// `nearest_height_at` smooths to polygon centers, oscillates between
/// adjacent polys at high render fps (which forced a per-entity
/// z-hint cache), and disagrees with the MZB collision surface by
/// ~0.4 yalm in zones where both are loaded. Using it for gravity
/// confused the two roles. After this refactor the navmesh is
/// reactor-only (path queries from `dispatch_movement_system`); the
/// snap touches only the collision surface.
///
/// Self off the loaded zone bounds gets no snap — `transform.y`
/// stays at whatever the previous frame set, which for fresh spawns
/// is the wire-derived `ffxi_to_bevy` value. No silent fallback to
/// the pathing surface.
///
/// ## Why no offset table
///
/// Every entity's mesh is spawned with its feet at the parent's local
/// y=0 — capsule placeholders via `Mesh::translated_by(Vec3::Y *
/// (r + hl))` in `setup_world`, baked actors via the pivot/mesh-entity
/// translation in `dat_vos2::spawn_skinned_actor` /
/// `spawn_vos2_meshes_with_skeleton`. So `transform.y = ground` puts
/// feet at ground by construction; no per-kind feet_offset, no
/// per-actor `visual_root_offset` estimate.
fn snap_entities_to_mzb_floor_system(
    collision_geom: Res<ffxi_viewer_core::dat_mzb::MzbCollisionGeometry>,
    scene: Res<SceneState>,
    mut q: Query<(
        &WorldEntity,
        &mut Transform,
        Has<ffxi_viewer_core::components::IsSelf>,
    )>,
) {
    // Zone-change fallback: until the new zone's MZB collision finishes
    // loading, the snap has no floor to anchor against. Without this
    // branch the system early-returned, leaving self.y at the *previous*
    // zone's snap result — visually "falling through the world" when
    // the new zone's floor is at a different elevation at the same
    // XZ. Trust wire Y for self in this window; the server's Y for the
    // player is authoritative across zone transitions. Snap takes over
    // again on the first frame `tri_count() > 0`.
    if collision_geom.tri_count() == 0 {
        let wire_self_y = ffxi_viewer_core::ffxi_to_bevy(scene.snapshot.self_pos.pos).y;
        for (_we, mut t, is_self) in q.iter_mut() {
            if is_self {
                t.translation.y = wire_self_y;
            }
        }
        return;
    }
    for (_we, mut t, is_self) in q.iter_mut() {
        // Self only. Everything else — Mob, other Pc, Pet, static NPC,
        // Other — is server-positioned; `sync_entities_system` writes
        // their wire Y directly. Overriding any of them to the local
        // MZB floor causes the rendered visual to disagree with the
        // server's range check (a mob visually "on the ground" 5y away
        // while the server sees it 15y up the hillside and rejects
        // attacks as out of range). Even static NPCs: their server-
        // recorded Y is the canonical position, and rewriting it to a
        // raycast result silently moves them between visits.
        //
        // The player's input pipeline writes X/Z but not Y, so the
        // snap remains the sole writer of self's vertical position.
        if !is_self {
            continue;
        }
        // `ceiling_y` filters overhead floor-like geometry (arches,
        // gate tops, second-floor surfaces the player is walking
        // *under*). Of the candidates that pass `FLOOR_NORMAL_MIN`
        // and the ceiling bound, `ground_raycast` picks the highest —
        // multi-floor step-up support intact.
        //
        // STEP_TOLERANCE is generous (2 yalms) to absorb the case
        // where the player briefly clips above the floor between snap
        // ticks; the floor-normal filter does the actual "is this a
        // walkable surface" work.
        const STEP_TOLERANCE: f32 = 2.0;
        let ceiling_y = t.translation.y + STEP_TOLERANCE;
        if let Some(ground) = collision_geom
            .ground_raycast(Vec2::new(t.translation.x, t.translation.z), ceiling_y)
        {
            t.translation.y = ground;
        }
    }
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
