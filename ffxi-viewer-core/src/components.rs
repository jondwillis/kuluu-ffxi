//! ECS components used by the viewer scene. Distinct types from the ones
//! in `ffxi-client/src/view3d/scene.rs` so the two viewers don't accidentally
//! share component identity if both ever ran in the same App.

use bevy::prelude::*;
use ffxi_viewer_wire::{EntityKind, EntityLook};

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

/// Marks any entity that belongs to the in-game session (world entities,
/// camera, HUD nodes, PC/NPC mirrors, nameplates). The front-end binary
/// despawns every entity carrying this marker on session teardown
/// (`OnExit(AppPhase::InGame)` in the native viewer), so the UI/scene
/// reset cleanly when the player logs out or gets dropped.
///
/// Why a viewer-core-side marker rather than Bevy's built-in
/// `DespawnOnExit<S>(state)`: the state type (`AppPhase`) lives in the
/// front-end crate; viewer-core can't reference it without a circular
/// dependency. This marker is the cross-crate-clean equivalent — every
/// viewer-core spawner attaches it, and each front-end registers the
/// despawn system against its own state type.
///
/// Despawn is recursive in Bevy (children come along), so this only
/// needs to land on top-level spawn entities — not every nested child.
#[derive(Component, Debug, Clone, Copy)]
pub struct InGameEntity;

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

/// The most recently observed `EntityLook` for a `WorldEntity`, copied
/// from `ffxi_viewer_wire::Entity::look` each tick by
/// `sync_entities_system`. Held on the Bevy side so look-driven
/// systems (model spawning, equipment swap, etc.) can react to changes
/// without scraping the snapshot, and so Bevy's `Changed<LookComp>`
/// query filter only fires when the wire value actually differs from
/// what we stored last tick.
///
/// Carries `EntityLook`, not the raw `LookData`, because the
/// snapshot-to-Bevy boundary already paid the translation cost in
/// `wire_translate`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct LookComp(pub EntityLook);

/// Marks a `WorldEntity` that has an MMB model spawned as a child via
/// the [`crate::look_resolver`] dispatch path. The stored
/// `EntityLook` is the *signature* the model was built from — when
/// the entity's `LookComp` later differs, the look-driven respawn
/// system knows to despawn and rebuild.
///
/// Distinct from [`crate::dat_mmb::MmbOverlay`], which marks the
/// *mesh* children — `EntityModel` lives on the parent (the
/// `WorldEntity`), so a single query reveals "do we already have a
/// model for this entity?" without scanning children.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityModel(pub EntityLook);

/// Marks a static **MMB placement** mesh as a camera occluder. Attached
/// at spawn only to static MMB placements (zone-spawn buildings, free
/// `/load_mmb` overlays) — NOT to MZB submeshes, and NOT to
/// entity-attached MMBs (NPCs/PCs/pets move every frame and would force
/// a BVH-build storm).
///
/// `ffxi-client/src/view_native/collision_bvh.rs::build_collision_bvh_system`
/// keys off this marker to build a per-placement [`CollisionBvh`], but
/// **only when `/zonegeom source` is `mmb` or `both`**. The default
/// `mzb` source ignores these entirely and clamps the camera against the
/// single zone-level `ZoneCollisionBvh` (the MZB collision channel),
/// which is FFXI's authoritative "what is solid" signal. This MMB path
/// is the legacy / diagnostic source, retained until MZB-only collision
/// is verified retail-faithful (it occludes decorative props like grass,
/// which is the bug it exists to let us A/B against).
///
/// Why a dedicated marker rather than reusing `MzbCollisionMesh`: that
/// marker carries channel semantics — `/zonegeom` toggles MZB collision
/// vs. non-collision visibility on it, and player-movement / ground-snap
/// raycasts read the collision channel specifically. A separate camera
/// marker keeps each downstream system pointed at the data it wants.
#[derive(Component, Debug, Clone, Copy)]
pub struct CameraOccluder;
