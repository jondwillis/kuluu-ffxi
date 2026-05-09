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
use ffxi_viewer_core::{IsSelf, InputMode, SceneState};

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
                    // Gravity snap runs *after* sync_entities (which is
                    // ordered with .chain() inside ViewerCorePlugin's
                    // Update tuple). Putting it in the same Update lets
                    // Bevy schedule it after the entity transforms are
                    // populated this frame.
                    snap_self_to_navmesh_system,
                )
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
        channel: ffxi_viewer_wire::ChatChannel::System,
        sender: "client".into(),
        text: msg,
        server_ts: 0,
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
    // 1.0 yalm above the floor plane so the overlay isn't depth-fought
    // into invisibility by the placeholder ground. With a real terrain
    // mesh this would shrink back to ~0.05 (or be replaced by gizmo
    // depth-bias config), but for the floor-plane-only viewer it needs
    // visible separation.
    let lift_bevy_y = 1.0;
    for (a, b) in &state.edges {
        let pa = detour_to_bevy(*a) + Vec3::Y * lift_bevy_y;
        let pb = detour_to_bevy(*b) + Vec3::Y * lift_bevy_y;
        gizmos.line(pa, pb, color);
    }
}

/// Visual gravity: each frame, query the navmesh for the height at
/// the self-entity's 2D position and snap the rendered Y to it. This
/// runs **after** `sync_entities_system` populates the transform from
/// the wire snapshot, so we override the wire's height with the
/// navmesh's height — handy when the server's reported `z` lags
/// terrain (e.g. it never updates `z` for routine moves) or when our
/// own `slide_along`'s slid `z` doesn't make it back into the snapshot.
///
/// Only touches the self entity. Other entities (mobs, NPCs, PCs)
/// keep their server-reported height; their packets generally include
/// the right value, and overriding them would be both more expensive
/// (N entities × 60 Hz) and more wrong (their actual server-side
/// position is what matters for combat range etc).
fn snap_self_to_navmesh_system(
    state: Res<NavmeshState>,
    mut self_q: Query<&mut Transform, With<IsSelf>>,
) {
    let Some(nav) = &state.nav else { return };
    let Ok(mut t) = self_q.single_mut() else { return };

    // Bevy → FFXI ground-plane: bevy.x = ffxi.x, bevy.z = -ffxi.y.
    // Bevy.y is the current rendered height (= ffxi.z); use that as
    // the z-hint so multi-level zones disambiguate to the right layer.
    let ffxi_x = t.translation.x;
    let ffxi_y = -t.translation.z;
    let z_hint = t.translation.y;

    let height = match nav.lock() {
        Ok(guard) => guard.nearest_height_at(ffxi_x, ffxi_y, z_hint),
        Err(_) => return,
    };

    if let Some(h) = height {
        // ffxi_to_bevy puts FFXI.z into Bevy.y, so the override is
        // direct — no transform composition needed.
        t.translation.y = h;
    }
}

/// Detour-space → Bevy world. xiNavmeshes are stored in Detour-
/// standard y-up coords; Bevy is also y-up. They differ only in
/// z-handedness (Bevy is right-handed with -Z forward; Detour's
/// reference samples are left-handed). Negating z is the standard
/// fix for that single-handedness flip.
///
/// If the overlay still misaligns with the floor texture / entity
/// positions after this fix, the *next* most-likely tweak is whether
/// to also negate x. Try `Vec3::new(-d[0], d[1], -d[2])` if so.
#[inline]
fn detour_to_bevy(d: [f32; 3]) -> Vec3 {
    Vec3::new(d[0], d[1], -d[2])
}
