use bevy::prelude::*;
use ffxi_viewer_wire::{EntityKind, EntityLook};

#[derive(Component, Debug, Clone, Copy)]
pub struct WorldEntity {
    pub id: u32,
    pub act_index: u16,
    pub kind: EntityKind,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct IsSelf;

#[derive(Component, Debug, Clone, Copy)]
pub struct InGameEntity;

#[derive(Component, Debug, Clone, Copy)]
pub struct Nameplate {
    pub entity_id: u32,
    pub kind: EntityKind,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct HpIndicator;

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct LookComp(pub EntityLook);

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityModel(pub EntityLook);

/// Model-load transition: grows the actor in while a transient orb stretches
/// into a light-column and dissolves. Both child entities are torn down on
/// completion (or with the parent, recursively).
#[derive(Component, Debug, Clone)]
pub struct MorphIn {
    pub elapsed: f32,
    pub actor_root: Entity,
    pub orb: Option<Entity>,
    pub orb_mat: Option<Handle<StandardMaterial>>,
    pub orb_emissive: LinearRgba,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct CameraOccluder;
