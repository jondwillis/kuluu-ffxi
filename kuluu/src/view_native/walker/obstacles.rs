//! Dynamic obstacles (plan §2.5): closed door leaves as world-baked triangles,
//! mobs as horizontal circles. Rebuilt every fixed tick into [`ObstacleSet`]
//! before dispatch — the slot the avian collider syncs used; `step` is pure
//! over it. Bevy space throughout: xz horizontal, y up.
//!
//! Doors: the MZB gives door groups no collision by authorial intent (retail
//! door solidity is dynamic), so a closed leaf's mesh — baked through its
//! AUTHORED pose, which is the closed one — is the solid geometry: walls for
//! the sweep AND floors for the column probe (a closed drawbridge). Any swing
//! displacement means open or mid-swing and the leaf drops out of the set.
//!
//! Mobs: horizontal circles in xz from the model AABB's wider ground-plane
//! half-extent, with the old body-block rules — `EntityKind::Other` never
//! blocks, undrawn actors never block, self is never in the set.

use bevy::prelude::*;
use kuluu_render::{
    components::{IsSelf, WorldEntity},
    zone_doors::{DoorPose, ZoneDoorLeaf, ZoneDoors},
};

/// One tick's dynamic obstacle set (bevy space: xz horizontal, y up).
#[derive(Resource, Default)]
pub struct ObstacleSet {
    /// Closed door leaves: walls for the sweep AND floors for the column probe.
    pub doors: Vec<DoorObstacle>,
    /// Mobs that body-block this tick: circle-vs-circle in xz (plan §2.5).
    pub mobs: Vec<MobObstacle>,
}

/// A closed door leaf's solid geometry.
pub struct DoorObstacle {
    /// World-space triangles through the authored (closed) pose, each with its
    /// winding-derived face normal (world space, bevy up).
    pub tris: Vec<([Vec3; 3], Vec3)>,
    /// Bounding box for cheap culling in both contact and column queries.
    pub min: Vec3,
    pub max: Vec3,
}

/// A mob's horizontal block circle. Vertical extent is ignored by design: the
/// walker tests circles in xz only (plan §2.5).
#[derive(Clone, Copy, Debug)]
pub struct MobObstacle {
    /// The wire entity id — stable identity for PushThrough accrual.
    pub id: u32,
    pub center: Vec2,
    pub radius: f32,
}

/// Snapshot of a non-self entity's horizontal block radius, captured once from
/// the live model AABB (the wider ground-plane half-extent). The AABB lives on
/// a mesh DESCENDANT of the WorldEntity (WorldEntity -> actor_root -> mesh
/// child) and is updated every frame by `update_actor_mesh_aabbs`; we snapshot
/// it once the child exists rather than re-reading each tick — a walk cycle
/// barely moves the horizontal extent, and the old avian bridge thrashed its
/// broadphase resizing a collider per tick.
#[derive(Component, Clone, Copy)]
pub struct MobBlockRadius {
    pub radius: f32,
}

/// Snapshot pass (old `sync_mob_collider_radius`): insert [`MobBlockRadius`] on
/// each non-self actor once its descendant Aabb exists. Runs before the rebuild
/// so a freshly spawned mob blocks from the tick after its mesh is posed.
pub fn snapshot_mob_block_radius(
    mut commands: Commands,
    entities: Query<
        (Entity, Option<&Children>),
        (With<WorldEntity>, Without<IsSelf>, Without<MobBlockRadius>),
    >,
    children_q: Query<&Children>,
    aabb_q: Query<&bevy::camera::primitives::Aabb>,
) {
    for (entity, kids) in entities.iter() {
        let Some(kids) = kids else { continue };
        if let Some(aabb) = find_descendant_aabb(kids, &children_q, &aabb_q) {
            let he = aabb.half_extents;
            let radius = he.x.max(he.z);
            // Ignore degenerate/not-yet-posed bounds; retry next tick.
            if radius > 1e-3 && he.y > 1e-3 {
                commands.entity(entity).insert(MobBlockRadius { radius });
            }
        }
    }
}

/// Depth-first search of an entity's descendants for the first `Aabb` (same
/// walk as the old avian bridge's radius snapshot).
fn find_descendant_aabb(
    kids: &Children,
    children_q: &Query<&Children>,
    aabb_q: &Query<&bevy::camera::primitives::Aabb>,
) -> Option<bevy::camera::primitives::Aabb> {
    for child in kids.iter() {
        if let Ok(aabb) = aabb_q.get(child) {
            return Some(*aabb);
        }
        if let Ok(grandkids) = children_q.get(child) {
            if let Some(aabb) = find_descendant_aabb(grandkids, children_q, aabb_q) {
                return Some(aabb);
            }
        }
    }
    None
}

/// "Is the texture drawn": a rendered mesh in the actor's subtree. InheritedVisibility so a real mob doesn't turn walk-through when the camera looks away (same test as the old avian bridge).
fn drawn_mesh_in(
    kids: &Children,
    children_q: &Query<&Children>,
    mesh_vis: &Query<&InheritedVisibility, With<Mesh3d>>,
) -> bool {
    for child in kids.iter() {
        if let Ok(vis) = mesh_vis.get(child) {
            if vis.get() {
                return true;
            }
        }
        if let Ok(k) = children_q.get(child) {
            if drawn_mesh_in(k, children_q, mesh_vis) {
                return true;
            }
        }
    }
    false
}

/// Rebuild [`ObstacleSet`] for this tick. Runs in FixedUpdate before dispatch.
pub fn rebuild_obstacles_system(
    doors_res: Res<ZoneDoors>,
    meshes: Res<Assets<Mesh>>,
    leaf_q: Query<(&ZoneDoorLeaf, &Children)>,
    mesh_children: Query<&Mesh3d>,
    mob_q: Query<
        (
            Entity,
            &WorldEntity,
            Option<&Children>,
            &Transform,
            &MobBlockRadius,
        ),
        Without<IsSelf>,
    >,
    children_q: Query<&Children>,
    mesh_vis: Query<&InheritedVisibility, With<Mesh3d>>,
    mut set: ResMut<ObstacleSet>,
) {
    // Doors: bake the closed leaves' triangles through the authored pose. The
    // verts are mirror-correct via the full matrix and independent of the
    // current swing; only the CLOSED-ness gate is live state (old toggle pass).
    let mut doors = Vec::new();
    for (leaf, kids) in leaf_q.iter() {
        if doors_res.dir(leaf.four_cc).is_none() {
            continue; // not a door-routine group: MZB-only
        }
        let pose = doors_res.pose(leaf.key());
        let closed = pose.rotation == Vec3::ZERO && pose.translation == Vec3::ZERO;
        if !closed {
            continue; // open or mid-swing: passable this tick
        }
        let xform = leaf.posed_transform(DoorPose::default());
        let mut tris: Vec<([Vec3; 3], Vec3)> = Vec::new();
        for child in kids.iter() {
            let Ok(m3) = mesh_children.get(child) else {
                continue;
            };
            // Asset still loading: skip this leaf this tick, retry next.
            let Some(mesh) = meshes.get(m3.0.id()) else {
                continue;
            };
            let Some(positions) = mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .and_then(|a| a.as_float3())
            else {
                continue;
            };
            let Some(indices) = mesh.indices() else {
                continue;
            };
            let mut it = indices.iter();
            while let (Some(ia), Some(ib), Some(ic)) = (it.next(), it.next(), it.next()) {
                let v: [Vec3; 3] = [
                    xform.transform_point3(Vec3::from_array(*positions.get(ia).unwrap())),
                    xform.transform_point3(Vec3::from_array(*positions.get(ib).unwrap())),
                    xform.transform_point3(Vec3::from_array(*positions.get(ic).unwrap())),
                ];
                let n = (v[1] - v[0]).cross(v[2] - v[0]);
                if n.length_squared() < 1e-12 {
                    continue; // degenerate: no face to collide with
                }
                tris.push((v, n.normalize()));
            }
        }
        if tris.is_empty() {
            continue;
        }
        let mut min = Vec3::INFINITY;
        let mut max = Vec3::NEG_INFINITY;
        for (v, _) in &tris {
            for p in v.iter() {
                min = min.min(*p);
                max = max.max(*p);
            }
        }
        doors.push(DoorObstacle { tris, min, max });
    }

    // Mobs: the old body-block rules (old `mob_body_blocks`), in order.
    let mut mobs = Vec::new();
    for (_ent, we, kids, t, r) in mob_q.iter() {
        // 1. EntityKind::Other — the HUD's "[obj]": door objects, "???" points,
        //    event triggers. These NEVER body-block, whatever mesh they carry.
        if matches!(we.kind, kuluu_snapshot::EntityKind::Other) {
            continue;
        }
        // 2. Character kinds block only when their texture is actually drawn:
        //    undrawn actor = invisible entity = walk through.
        let Some(kids) = kids else {
            continue;
        };
        if !drawn_mesh_in(kids, &children_q, &mesh_vis) {
            continue;
        }
        mobs.push(MobObstacle {
            id: we.id,
            center: Vec2::new(t.translation.x, t.translation.z),
            radius: r.radius,
        });
    }

    set.doors = doors;
    set.mobs = mobs;
}
