//! Click-to-target. Uses Bevy's `bevy_picking` mesh raycast backend to map
//! a left-mouse click on an entity capsule to a [`Target`] write.
//!
//! Design:
//! - Wire-side entity capsules are spawned with a default [`Pickable`] in
//!   `scene::sync_entities_system`. The `IsSelf` capsule and HP-bar children
//!   are tagged with [`Pickable::IGNORE`] so they don't intercept clicks
//!   destined for other entities or the ground.
//! - This module drives [`MeshPickingPlugin`] (the raycast backend, not part
//!   of `DefaultPickingPlugins`) and a single [`MessageReader`] system that
//!   reads `Pointer<Click>` messages each frame.
//! - For each LMB click, we resolve the hit entity to its [`WorldEntity::id`]
//!   and write into [`Target`]. A click that lands on anything that *isn't*
//!   a `WorldEntity` (the ground plane, primarily) clears the target —
//!   matching FFXI's "click empty ground to deselect" feel.
//! - `Pointer<Over>` / `Pointer<Out>` updates [`HoveredEntity`] (the wire
//!   id of the entity currently under the cursor, if any). The custom
//!   cursor sprite reads this to request the `Hand` look, and the entity
//!   hover-card HUD reads it to show a lightweight info chip.

use bevy::picking::events::{Out, Over};
use bevy::picking::mesh_picking::MeshPickingPlugin;
use bevy::picking::pointer::{PointerButton, PointerId};
use bevy::picking::prelude::*;
use bevy::prelude::*;

use crate::components::WorldEntity;
use crate::input_mode::{InputMode, QuickActionState};
use crate::scene::Target;

/// FFXI wire id of the entity currently under the mouse cursor, if any.
/// Updated by [`update_hovered_entity_system`] in response to
/// `Pointer<Over>` / `Pointer<Out>` on entities that carry a
/// [`WorldEntity`] component. Cleared when the cursor leaves the entity.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct HoveredEntity {
    pub id: Option<u32>,
}

/// Plugin: registers `MeshPickingPlugin` (the raycast backend), the
/// click→target reader, and the hover-state tracker.
/// `DefaultPickingPlugins` is already added by `DefaultPlugins` on both
/// the native and WASM front-ends, so we only need the backend here.
pub struct PickingPlugin;

impl Plugin for PickingPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MeshPickingPlugin)
            .init_resource::<HoveredEntity>()
            .add_systems(Update, (click_to_target_system, update_hovered_entity_system));
    }
}

/// Track which `WorldEntity` is under the mouse cursor. Uses
/// `Pointer<Over>` / `Pointer<Out>` so the resource only changes on
/// enter/leave, not per-frame. The self capsule (id == 0) is excluded —
/// targeting yourself isn't a thing in FFXI and a Hand cursor over your
/// own avatar would be misleading.
///
/// `Out` is processed before `Over` so a same-frame `Out → Over`
/// (cursor sliding from one entity onto another) ends with the new
/// entity's id, not `None`.
pub fn update_hovered_entity_system(
    mut over_events: MessageReader<Pointer<Over>>,
    mut out_events: MessageReader<Pointer<Out>>,
    world_q: Query<&WorldEntity>,
    parent_q: Query<&ChildOf>,
    mut hovered: ResMut<HoveredEntity>,
) {
    for ev in out_events.read() {
        if ev.pointer_id != PointerId::Mouse {
            continue;
        }
        if let Some(w) = find_world_entity(ev.entity, &world_q, &parent_q) {
            if hovered.id == Some(w.id) {
                hovered.id = None;
            }
        }
    }
    for ev in over_events.read() {
        if ev.pointer_id != PointerId::Mouse {
            continue;
        }
        if let Some(w) = find_world_entity(ev.entity, &world_q, &parent_q) {
            if w.id == 0 {
                continue; // self capsule — never target / hover
            }
            hovered.id = Some(w.id);
        }
    }
}

/// Walk up the `ChildOf` chain from `entity` until we find a `WorldEntity`,
/// or run out of ancestors. The picking backend reports the deepest hit
/// mesh, which for loaded MMB / skinned actors is a sub-mesh child of the
/// `WorldEntity`-bearing parent. Without this walk, click-to-target and
/// hover only work on the bare placeholder capsules. Cap the depth at 8
/// so a malformed hierarchy can't spin.
fn find_world_entity<'q>(
    mut entity: Entity,
    world_q: &'q Query<&WorldEntity>,
    parent_q: &Query<&ChildOf>,
) -> Option<&'q WorldEntity> {
    for _ in 0..8 {
        if let Ok(w) = world_q.get(entity) {
            return Some(w);
        }
        match parent_q.get(entity) {
            Ok(parent) => entity = parent.0,
            Err(_) => return None,
        }
    }
    None
}

/// Resolve a left-click hit into a [`ClickResolution`]. Decides between
/// retargeting, deselecting, and opening the contextual menu based on
/// the entity hit (if any) and the operator's current target.
///
/// Rules:
/// - Click on a different entity → retarget.
/// - Click on the *already-selected* entity → open the contextual menu.
///   (FFXI-retail: clicking your selected target is an interaction, not a
///   no-op.)
/// - Click on the self-capsule with no target → open the contextual menu.
///   (Self-clicks never target self, so falling back to the menu is more
///   useful than a silent no-op.)
/// - Click on the self-capsule with an existing target → clear the target.
///   (Acts like a "click off" gesture in retail.)
/// - Click on empty ground / non-entity:
///   - With a target → clear the target.
///   - Without a target → open the contextual menu.
///
/// Pulled into a standalone function so the click-resolution logic is
/// unit-testable without a Bevy app.
pub fn resolve_click_target(
    world_entity: Option<&WorldEntity>,
    current_target: Option<u32>,
) -> ClickResolution {
    match world_entity {
        // Self capsule (id == 0): never targets self.
        Some(w) if w.id == 0 => match current_target {
            Some(_) => ClickResolution::Clear,
            None => ClickResolution::OpenContextMenu,
        },
        // Already-selected entity → contextual menu.
        Some(w) if Some(w.id) == current_target => ClickResolution::OpenContextMenu,
        // Different entity → retarget.
        Some(w) => ClickResolution::Set(w.id),
        // Empty space:
        None => match current_target {
            Some(_) => ClickResolution::Clear,
            None => ClickResolution::OpenContextMenu,
        },
    }
}

/// Outcome of resolving a click — what to do with [`Target`] and
/// [`InputMode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickResolution {
    /// Set `Target.id = Some(id)`.
    Set(u32),
    /// Set `Target.id = None`.
    Clear,
    /// Push `InputMode::QuickAction(default)` (target is left untouched).
    OpenContextMenu,
}

/// Per-frame click handler. Drains `Pointer<Click>` messages, filters to
/// LMB, and updates [`Target`] / [`InputMode`].
///
/// Clicks while *not* in `InputMode::World` (chat focused, dialog open,
/// menu navigation) are dropped — typing in chat shouldn't retarget,
/// and a stray click during a dialog shouldn't open another menu on
/// top of it.
pub fn click_to_target_system(
    mut clicks: MessageReader<Pointer<Click>>,
    q_world: Query<&WorldEntity>,
    q_parent: Query<&ChildOf>,
    mut target: ResMut<Target>,
    mut input_mode: ResMut<InputMode>,
) {
    for ev in clicks.read() {
        if ev.button != PointerButton::Primary {
            continue;
        }
        if !matches!(*input_mode, InputMode::World) {
            continue;
        }
        let world_entity = find_world_entity(ev.entity, &q_world, &q_parent);
        match resolve_click_target(world_entity, target.id) {
            ClickResolution::Set(id) => target.id = Some(id),
            ClickResolution::Clear => target.id = None,
            ClickResolution::OpenContextMenu => {
                *input_mode =
                    InputMode::QuickAction(QuickActionState::for_target(target.id.is_some()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_viewer_wire::EntityKind;

    fn mob(id: u32) -> WorldEntity {
        WorldEntity {
            id,
            act_index: 0,
            kind: EntityKind::Mob,
        }
    }

    /// Click on a different entity → retarget.
    #[test]
    fn click_on_new_entity_retargets() {
        assert_eq!(
            resolve_click_target(Some(&mob(17)), Some(99)),
            ClickResolution::Set(17),
        );
    }

    /// Click on a wire entity with no current target → set target.
    #[test]
    fn click_on_entity_with_no_target_sets_target() {
        assert_eq!(
            resolve_click_target(Some(&mob(17)), None),
            ClickResolution::Set(17),
        );
    }

    /// Click on the already-selected entity → context menu.
    #[test]
    fn click_on_already_selected_opens_menu() {
        assert_eq!(
            resolve_click_target(Some(&mob(17)), Some(17)),
            ClickResolution::OpenContextMenu,
        );
    }

    /// Click on the self-capsule with a target → clear (act as deselect).
    #[test]
    fn click_on_self_capsule_with_target_clears() {
        let w = WorldEntity {
            id: 0,
            act_index: 0,
            kind: EntityKind::Pc,
        };
        assert_eq!(
            resolve_click_target(Some(&w), Some(17)),
            ClickResolution::Clear,
        );
    }

    /// Click on the self-capsule with no target → context menu.
    #[test]
    fn click_on_self_capsule_without_target_opens_menu() {
        let w = WorldEntity {
            id: 0,
            act_index: 0,
            kind: EntityKind::Pc,
        };
        assert_eq!(
            resolve_click_target(Some(&w), None),
            ClickResolution::OpenContextMenu,
        );
    }

    /// Click on empty ground with a target → clear (deselect).
    #[test]
    fn click_on_empty_with_target_clears() {
        assert_eq!(resolve_click_target(None, Some(17)), ClickResolution::Clear,);
    }

    /// Click on empty ground with no target → context menu.
    #[test]
    fn click_on_empty_without_target_opens_menu() {
        assert_eq!(
            resolve_click_target(None, None),
            ClickResolution::OpenContextMenu,
        );
    }
}
