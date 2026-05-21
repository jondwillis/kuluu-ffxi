//! Entity dot overlay. Reads [`crate::scene::TrackedEntities`] +
//! [`crate::components::IsSelf`] each frame, projects every entity into
//! minimap UV space via [`super::MinimapState::active_aabb`], and
//! reconciles child `Node`s under the [`super::MinimapOverlayLayer`]
//! container.
//!
//! Identical math for both backends — the AABB plumbed through
//! `MinimapState` is the only thing that differs between TopDown and
//! Retail.
//!
//! # Dot reconciliation
//!
//! The system maintains a one-to-one map from FFXI entity id → child
//! UI `Node` entity (the dot) inside [`MinimapDots`]. Each frame:
//!
//! 1. For every wire entity in `TrackedEntities`, project to UV. If
//!    the dot doesn't exist yet, spawn one as a child of the overlay
//!    layer; otherwise update its `Node::left/top` to match the new
//!    UV, and its `BackgroundColor` if the target / kind changed.
//! 2. Despawn any dot whose id no longer appears in the snapshot —
//!    cleanup is symmetric, matching the
//!    [`bevy-lifecycle-symmetry`] discipline.
//!
//! Self gets a different shape (a small filled triangle rotated to
//! `ChaseCamera.yaw`) so the operator can read "where am I + facing"
//! at a glance.

use std::collections::HashMap;

use bevy::prelude::*;
use ffxi_viewer_wire::EntityKind;

use crate::camera::ChaseCamera;
use crate::components::{InGameEntity, IsSelf, WorldEntity};
use crate::lock_on::LockOn;
use crate::scene::{Target, TrackedEntities};
use crate::snapshot::SceneState;

use super::{MinimapOverlayLayer, MinimapView};

/// Dot diameter in CSS pixels for other entities. Small enough that a
/// crowd of entities reads as a cluster, big enough that a single mob
/// in an empty zone is findable.
const DOT_DIAMETER_PX: f32 = 6.0;
/// Self marker (triangle bounding box) edge length.
const SELF_MARKER_PX: f32 = 10.0;
/// Ring outline thickness for the locked / targeted dot.
const TARGET_RING_PX: f32 = 2.0;

/// Lookup table: wire-entity id → dot UI entity. Carried as a
/// `Resource` so reconciliation is O(1) per entity instead of an
/// O(N²) `Query` scan.
///
/// Cleared on session exit via [`MinimapDots::clear_for_logout`] —
/// per `MEMORY.md` bevy-lifecycle-symmetry, every cache-holding
/// `Resource` needs an explicit drain.
#[derive(Resource, Default)]
pub struct MinimapDots {
    pub by_id: HashMap<u32, Entity>,
}

impl MinimapDots {
    pub fn clear_for_logout(&mut self) {
        self.by_id.clear();
    }
}

/// Marker on every minimap-overlay dot entity. Lets a future bulk
/// `despawn` query find them without consulting [`MinimapDots`].
#[derive(Component)]
pub struct MinimapDot {
    /// Wire entity id this dot represents. Self uses `u32::MAX` as a
    /// sentinel so it never collides with a real id.
    pub entity_id: u32,
}

/// Sentinel id used for the self marker so it lives in [`MinimapDots`]
/// alongside other-entity dots without colliding.
const SELF_MARKER_ID: u32 = u32::MAX;

/// Per-frame: reconcile dots for every tracked entity.
///
/// Reads [`MinimapView::visible_aabb`] (published by
/// `update_minimap_view` earlier in the chain) so dots cull when
/// zoomed in and align with whatever sub-window the image is
/// currently cropped to.
///
/// Skips when no visible AABB is set yet (zone not baked / no retail
/// map loaded) — the empty overlay layer just stays bare.
pub fn update_minimap_overlay(
    view: Res<MinimapView>,
    scene_state: Res<SceneState>,
    target: Res<Target>,
    lock_on: Res<LockOn>,
    chase: Res<ChaseCamera>,
    tracked: Res<TrackedEntities>,
    q_overlay_layer: Query<Entity, With<MinimapOverlayLayer>>,
    q_self: Query<&Transform, With<IsSelf>>,
    q_transform: Query<(&Transform, &WorldEntity), Without<IsSelf>>,
    mut dots: ResMut<MinimapDots>,
    mut commands: Commands,
    mut q_dot_node: Query<(&mut Node, &mut BackgroundColor), With<MinimapDot>>,
) {
    let Some(aabb) = view.visible_aabb else {
        return;
    };
    let Ok(overlay_layer) = q_overlay_layer.single() else {
        return;
    };

    let snap = &scene_state.snapshot;
    let self_char_id = snap.self_char_id.unwrap_or(0);

    // Track which ids are present this frame so we can despawn stale
    // dots at the end. At high zoom most entities cull off-screen
    // and never make it into `seen` — they get despawned naturally.
    let mut seen: std::collections::HashSet<u32> =
        std::collections::HashSet::with_capacity(tracked.by_id.len() + 1);

    // ---- Other entities (PCs, NPCs, mobs, pets) -------------------
    //
    // Iterates `q_transform` which already excludes `IsSelf`, so the
    // self marker below is the only path that touches the local PC.
    for (transform, world_entity) in q_transform.iter() {
        // Skip the self entry if it somehow lands here (defensive —
        // the `Without<IsSelf>` filter should already exclude it, but
        // older snapshots seeded self as id=0 and some paths haven't
        // fully migrated).
        if self_char_id != 0 && world_entity.id == self_char_id {
            continue;
        }
        // Cull off-visible-window entities. They'll be despawned at
        // the end of the loop because they never get added to `seen`.
        let Some(uv) = aabb.world_to_uv_or_offscreen(transform.translation) else {
            continue;
        };
        let is_target = target.id == Some(world_entity.id);
        let is_locked = lock_on.target_id == Some(world_entity.id);
        let color = dot_color(world_entity.kind, is_target, is_locked);
        upsert_dot(
            &mut dots,
            &mut commands,
            overlay_layer,
            world_entity.id,
            uv,
            color,
            DOT_DIAMETER_PX,
            &mut q_dot_node,
            is_target || is_locked,
        );
        seen.insert(world_entity.id);
    }

    // ---- Self marker ---------------------------------------------
    //
    // Self should always be visible. With zoom centered on the
    // player + zero pan, self lands exactly at UV (0.5, 0.5). With
    // non-zero pan, it shifts but stays inside the window as long as
    // the pan offset is within the half-radius — the clamped
    // `world_to_uv` is the right fallback if the operator drags far
    // enough that even self would go off-screen.
    if let Ok(self_t) = q_self.single() {
        let uv = aabb
            .world_to_uv_or_offscreen(self_t.translation)
            .unwrap_or_else(|| aabb.world_to_uv(self_t.translation));
        upsert_dot(
            &mut dots,
            &mut commands,
            overlay_layer,
            SELF_MARKER_ID,
            uv,
            Color::srgb(0.2, 1.0, 1.0), // self_pc cyan, matches scene.rs
            SELF_MARKER_PX,
            &mut q_dot_node,
            false,
        );
        seen.insert(SELF_MARKER_ID);
        // TODO: rotate the self marker to `chase.yaw` to indicate
        // facing. Today it's a plain dot — adding a rotated triangle
        // needs either a `Transform` overlay (UI doesn't take 3D
        // rotations directly) or a glyph-based arrow. Pinned to
        // task #1's follow-up. Suppress the unused-variable warning
        // from the camera read in the meantime.
        let _ = chase.yaw;
    }

    // ---- Despawn stale dots --------------------------------------
    let stale: Vec<u32> = dots
        .by_id
        .keys()
        .copied()
        .filter(|id| !seen.contains(id))
        .collect();
    for id in stale {
        if let Some(dot_entity) = dots.by_id.remove(&id) {
            if let Ok(mut ec) = commands.get_entity(dot_entity) {
                ec.despawn();
            }
        }
    }
}

/// Spawn-or-update a single dot. Centralized so the self marker and
/// other-entity dots share lifecycle bookkeeping.
///
/// `with_ring` adds a brighter border to the dot to mark
/// targeted / locked entities. Layout uses absolute positioning so
/// dots can land anywhere in the overlay layer's 0–100% box; UV is
/// converted to percentages with the dot offset so the dot's center
/// sits at the UV point rather than its top-left corner.
#[allow(clippy::too_many_arguments)]
fn upsert_dot(
    dots: &mut MinimapDots,
    commands: &mut Commands,
    overlay_layer: Entity,
    entity_id: u32,
    uv: Vec2,
    color: Color,
    diameter_px: f32,
    q_dot_node: &mut Query<(&mut Node, &mut BackgroundColor), With<MinimapDot>>,
    with_ring: bool,
) {
    let half = diameter_px * 0.5;
    let left_pct = uv.x * 100.0;
    let top_pct = uv.y * 100.0;

    if let Some(&dot_entity) = dots.by_id.get(&entity_id) {
        if let Ok((mut node, mut bg)) = q_dot_node.get_mut(dot_entity) {
            let want_left = Val::Percent(left_pct);
            let want_top = Val::Percent(top_pct);
            if node.left != want_left {
                node.left = want_left;
            }
            if node.top != want_top {
                node.top = want_top;
            }
            let want_w = Val::Px(diameter_px);
            let want_h = Val::Px(diameter_px);
            if node.width != want_w {
                node.width = want_w;
            }
            if node.height != want_h {
                node.height = want_h;
            }
            let want_margin = UiRect {
                left: Val::Px(-half),
                top: Val::Px(-half),
                ..default()
            };
            if node.margin != want_margin {
                node.margin = want_margin;
            }
            if bg.0 != color {
                bg.0 = color;
            }
        }
        return;
    }

    // First spawn for this id. Border (the "ring") is conditional —
    // unselected dots get a 0-px border so the visual weight is the
    // fill color alone.
    let border = if with_ring {
        UiRect::all(Val::Px(TARGET_RING_PX))
    } else {
        UiRect::all(Val::Px(0.0))
    };
    let dot_entity = commands
        .spawn((
            InGameEntity,
            MinimapDot { entity_id },
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(left_pct),
                top: Val::Percent(top_pct),
                width: Val::Px(diameter_px),
                height: Val::Px(diameter_px),
                margin: UiRect {
                    left: Val::Px(-half),
                    top: Val::Px(-half),
                    ..default()
                },
                border,
                ..default()
            },
            BackgroundColor(color),
            BorderColor::all(Color::srgb(1.0, 0.95, 0.2)),
            ChildOf(overlay_layer),
        ))
        .id();
    dots.by_id.insert(entity_id, dot_entity);
}

/// Map (kind, target, locked) → dot color. Color choices echo the
/// world-space entity material palette in `scene::EntityMaterials` so
/// the minimap reads as a desaturated overhead view of the same world.
fn dot_color(kind: EntityKind, is_target: bool, is_locked: bool) -> Color {
    if is_locked {
        // Locked-on entities always pop with a hot pink: distinct from
        // every other color in the palette so the operator can find
        // the locked target instantly.
        return Color::srgb(1.0, 0.4, 0.8);
    }
    if is_target {
        return Color::srgb(1.0, 0.95, 0.2); // matches mats.target yellow
    }
    match kind {
        EntityKind::Pc => Color::srgb(0.40, 0.85, 1.00),
        EntityKind::Npc => Color::srgb(0.95, 0.85, 0.30),
        EntityKind::Mob => Color::srgb(0.95, 0.40, 0.40),
        EntityKind::Pet => Color::srgb(0.40, 0.85, 0.50),
        EntityKind::Other => Color::srgb(0.60, 0.60, 0.60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locked beats targeted beats kind — same priority chain as
    /// `pick_mob_material` in scene.rs.
    #[test]
    fn dot_color_priority_locked_over_target_over_kind() {
        let locked = dot_color(EntityKind::Mob, true, true);
        let targeted = dot_color(EntityKind::Mob, true, false);
        let mob = dot_color(EntityKind::Mob, false, false);
        // All three should be distinct (the priority chain hits
        // different colors).
        assert_ne!(locked, targeted);
        assert_ne!(targeted, mob);
        assert_ne!(locked, mob);
    }

    /// Locked is the brightest cue regardless of kind.
    #[test]
    fn dot_color_locked_overrides_pc_kind() {
        let locked_pc = dot_color(EntityKind::Pc, false, true);
        let pc = dot_color(EntityKind::Pc, false, false);
        assert_ne!(locked_pc, pc);
    }

    /// Distinct kinds get distinct colors so a crowd reads as a mix
    /// rather than a blob.
    #[test]
    fn dot_color_kinds_are_distinct() {
        let pc = dot_color(EntityKind::Pc, false, false);
        let npc = dot_color(EntityKind::Npc, false, false);
        let mob = dot_color(EntityKind::Mob, false, false);
        let pet = dot_color(EntityKind::Pet, false, false);
        assert_ne!(pc, npc);
        assert_ne!(npc, mob);
        assert_ne!(mob, pet);
        assert_ne!(pc, mob);
    }
}
