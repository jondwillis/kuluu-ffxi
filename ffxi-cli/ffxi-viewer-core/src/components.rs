//! ECS components used by the viewer scene. Distinct types from the ones
//! in `ffxi-client/src/view3d/scene.rs` so the two viewers don't accidentally
//! share component identity if both ever ran in the same App.

use bevy::prelude::*;
use ffxi_viewer_wire::EntityKind;

/// Marks a Bevy entity that mirrors a wire `Entity` — i.e. anything spawned
/// by `scene::sync_entities_system`. The `id` field is the FFXI `UniqueNo`
/// (`Entity::id`) — used to look up the same entity across frames.
#[derive(Component, Debug, Clone, Copy)]
pub struct WorldEntity {
    pub id: u32,
    pub act_index: u16,
    pub kind: EntityKind,
}

/// Marks the player's own avatar — the one to follow with the camera.
#[derive(Component, Debug, Clone, Copy)]
pub struct IsSelf;

/// Marks a UI nameplate node that displays the name of a `WorldEntity`.
/// Updated each frame by `nameplate::update_nameplates_system` to track the
/// owning entity's screen-projected position. `kind` lets the label
/// formatter branch on entity type — PCs (and self) display the bare name
/// without the HP% suffix that mobs/pets get, matching vanilla FFXI's
/// convention of not exposing other players' health to onlookers.
#[derive(Component, Debug, Clone, Copy)]
pub struct Nameplate {
    pub entity_id: u32,
    pub kind: EntityKind,
}

/// Marks an HP bar / dot child of a `WorldEntity`. Stage 0d wires this up;
/// the marker is here in scaffold so other systems can query for it.
#[derive(Component, Debug, Clone, Copy)]
pub struct HpIndicator;
