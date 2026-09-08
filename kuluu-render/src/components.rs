use bevy::prelude::*;
use kuluu_snapshot::{EntityKind, EntityLook};

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

/// On an entity currently riding a mount, whose body its animation lifts clear
/// of the ground the entity Transform still sits on. Anything anchored off that
/// Transform has to answer for the difference — see [`crate::camera::nameplate_anchor_y`].
#[derive(Component, Debug, Clone, Copy)]
pub struct MountedRider;

#[derive(Component, Debug, Clone, Copy)]
pub struct Nameplate {
    pub entity_id: u32,
    pub kind: EntityKind,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct HpIndicator;

/// Which MMB submesh a zone-geometry mesh entity came from. Attached by
/// `dat_mmb` at spawn; read by the `hud::mesh_debug` hover panel and by
/// `zone_lights` diagnostics.
#[derive(Component, Debug, Clone)]
pub struct MmbDebugInfo {
    pub file_id: u32,

    pub chunk_idx: usize,

    pub sub_index: usize,

    pub asset_name: String,

    pub variant_name: String,
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct LookComp(pub EntityLook);

/// The look a model was loaded for, plus whether it was loaded in its mounted
/// form. Mounting swaps in a whole extra animation DAT, so it re-keys the model
/// exactly like a gear change does.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityModel {
    pub look: EntityLook,
    pub mounted: bool,
}

/// The mount whose model is currently loaded onto a mount actor entity. Memoises
/// the dispatch the way [`EntityModel`] does for looks: a rider can swap mounts
/// without the entity ever going away.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct MountModel(pub kuluu_snapshot::Mount);

/// Model-load transition: grows the actor in while a transient orb stretches
/// into a light-column and dissolves. The column's lifetime belongs to this
/// component — see `ffxi_actor_render::despawn_morph_column`.
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

/// Fixed-tick render-position history used to smooth self movement between
/// FixedUpdate ticks. `apply_self_prediction_system` writes the authoritative
/// per-tick render position into `CurrRenderPos` (and rolls the old value
/// into `PrevRenderPos`) instead of mutating Transform directly.
/// `interpolate_self_transform_system` runs every render frame and lerps
/// Transform.translation between the two using `Time<Fixed>::overstep_fraction`,
/// so the camera (which reads Transform) never sees the 60Hz-quantized wobble
/// that used to shake the world as you walked up stairs.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct PrevRenderPos(pub Vec3);

#[derive(Component, Debug, Clone, Copy, Default)]
pub struct CurrRenderPos(pub Vec3);
