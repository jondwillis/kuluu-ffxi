//! RID door boxes follow server animation independently of visible leaves.
//! research/XIClient/src/XIClient/source/World/Zone/Triggers/RidManager.cpp RidManager::InitUnderscoreRid.

use bevy::prelude::*;
use ffxi_dat::zone_interaction::ZoneInteraction;
use ffxi_proto::decode::animation;
use kuluu_render::{
    components::{IsSelf, WorldEntity},
    zone_doors::{DoorPose, ZoneDoorLeaf, ZoneDoors},
};
use kuluu_snapshot::{Entity as WireEntity, EntityLook};

// FFXiMain.dll SHA-256 f4f90fbd080c05448aab3f866b127d7c1675b3cc15c8beaa57bfc584064b7e7c.
// DoorOpen RVA 0xAC5E0 / DoorClose RVA 0xAC670 toggle RID collision through RVA 0x177F20
// independently of visual schedulers.
fn rid_door_closed(rect: &ZoneInteraction, entities: &[WireEntity]) -> bool {
    !entities.iter().any(|entity| {
        matches!(entity.look, Some(EntityLook::Door { door_id: Some(id), .. })
            if id == rect.source_id.0)
            && entity.animation == animation::OPEN_DOOR
    })
}

fn rid_obstacle(rect: &ZoneInteraction) -> DoorObstacle {
    const HALF: f32 = 0.5;
    const CORNERS: [[f32; 3]; 8] = [
        [-HALF, -HALF, -HALF],
        [HALF, -HALF, -HALF],
        [-HALF, HALF, -HALF],
        [HALF, HALF, -HALF],
        [-HALF, -HALF, HALF],
        [HALF, -HALF, HALF],
        [-HALF, HALF, HALF],
        [HALF, HALF, HALF],
    ];
    const FACES: [[usize; 4]; 6] = [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
    ];
    // research/XIClient/src/XIClient/source/World/Zone/Triggers/RidManager.cpp RidManager::InitUnderscoreRid.
    let matrix = kuluu_render::dat_mzb::placement_bevy_transform(
        Vec3::from_array(rect.size).abs(),
        Vec3::new(0.0, rect.orientation[1], 0.0),
        Vec3::from_array(rect.position),
    );
    let vertices = CORNERS.map(|v| matrix.transform_point3(Vec3::from_array(v)));
    let mut tris = Vec::new();
    for [a, b, c, d] in FACES {
        for [a, b, c] in [[a, b, c], [a, c, d]] {
            let v = [vertices[a], vertices[b], vertices[c]];
            let n = (v[1] - v[0]).cross(v[2] - v[0]).normalize_or_zero();
            if n != Vec3::ZERO {
                tris.push((v, n));
            }
        }
    }
    DoorObstacle {
        tris,
        min: vertices.into_iter().fold(Vec3::INFINITY, Vec3::min),
        max: vertices.into_iter().fold(Vec3::NEG_INFINITY, Vec3::max),
    }
}

/// One tick's dynamic obstacle set (bevy space: xz horizontal, y up).
#[derive(Resource, Default)]
pub struct ObstacleSet {
    /// Enabled RID boxes and fallback leaves, in Bevy space.
    pub doors: Vec<DoorObstacle>,
    /// Mobs that body-block this tick: circle-vs-circle in xz (plan §2.5).
    pub mobs: Vec<MobObstacle>,
}

/// An enabled door's solid geometry.
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
    /// The wire entity id — stable identity for the contact budget.
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
    scene: Res<kuluu_render::snapshot::SceneState>,
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
    let mut doors: Vec<_> = doors_res
        .collision_rects()
        .iter()
        .filter(|rect| rid_door_closed(rect, &scene.snapshot.entities))
        .map(rid_obstacle)
        .collect();
    for (leaf, kids) in leaf_q.iter() {
        if doors_res
            .collision_rects()
            .iter()
            .any(|rect| rect.rect_id() == leaf.four_cc)
        {
            continue;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use kuluu_render::{dat_mzb::MzbCollisionGeometry, snapshot::SceneState};

    const GATE: [u8; 4] = *b"_6ww";
    const DOCK_POSITION: [f32; 3] = [18.000645, -2.385981, -59.55436];
    const DOCK_SIZE: [f32; 3] = [5.350002, 8.300005, 0.20000018];

    fn dock_dat(yaw: f32, rect_class: u32, size_sign: f32) -> Vec<u8> {
        const CHUNK_HEADER: usize = 16;
        const RID_TABLE: usize = 0x30;
        const TABLE_HEADER: usize = 16;
        const ENTRY_SIZE: usize = 64;
        const POSITION: usize = 0;
        const CLASS: usize = 0x0C;
        const YAW: usize = 0x10;
        const SIZE: usize = 0x18;
        const SOURCE: usize = 0x24;
        const TERRAIN: usize = 0x30;
        const CAMERA_SKIP: u16 = 0x100;
        let mut body = vec![0u8; RID_TABLE + TABLE_HEADER + ENTRY_SIZE];
        body[..4].copy_from_slice(b"RID\0");
        body[0x10..0x14].copy_from_slice(&(RID_TABLE as u32).to_le_bytes());
        body[RID_TABLE..RID_TABLE + 4].copy_from_slice(&1u32.to_le_bytes());
        let entry = &mut body[RID_TABLE + TABLE_HEADER..];
        for (base, values) in [
            (POSITION, DOCK_POSITION),
            (SIZE, DOCK_SIZE.map(|v| v * size_sign)),
        ] {
            for (i, value) in values.into_iter().enumerate() {
                entry[base + i * 4..base + (i + 1) * 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        entry[CLASS..CLASS + 4].copy_from_slice(&rect_class.to_le_bytes());
        entry[YAW..YAW + 4].copy_from_slice(&yaw.to_le_bytes());
        entry[SOURCE..SOURCE + 4].copy_from_slice(&GATE);
        entry[TERRAIN..TERRAIN + 2].copy_from_slice(&CAMERA_SKIP.to_le_bytes());
        let mut dat = vec![0u8; CHUNK_HEADER];
        dat[..4].copy_from_slice(b"test");
        let units = ((CHUNK_HEADER + body.len()) / CHUNK_HEADER) as u32;
        dat[4..8].copy_from_slice(&((units << 7) | ffxi_dat::ChunkKind::Rid as u32).to_le_bytes());
        dat.extend(body);
        dat
    }

    fn gate_entity(state: u8) -> WireEntity {
        WireEntity {
            id: 17_793_087,
            act_index: 63,
            kind: kuluu_snapshot::EntityKind::Other,
            name: None,
            pos: kuluu_snapshot::Vec3::default(),
            heading: 0,
            hp_pct: None,
            bt_target_id: 0,
            face_target: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: Some(EntityLook::Door {
                size: 2,
                door_id: Some(GATE),
            }),
            animation: state,
            animationsub: 0,
            mount: None,
            status: 0,
            char_flags: kuluu_snapshot::CharFlags::default(),
            monstrosity: false,
            name_vis: None,
        }
    }

    fn walk_past_gate(obstacles: &ObstacleSet, yaw: f32) -> f32 {
        const STEP: f32 = 0.1;
        const TICKS: usize = 100;
        const START_OFFSET: f32 = 2.5;
        const DT: f32 = 1.0 / 60.0;
        let outward = Vec2::new(yaw.sin(), yaw.cos());
        let center = Vec2::new(DOCK_POSITION[0], DOCK_POSITION[2]);
        let mut pos = center + outward * START_OFFSET;
        let mut walker = super::super::Walker::default();
        let geom = MzbCollisionGeometry::default();
        for _ in 0..TICKS {
            let result = super::super::step::step(
                &geom,
                obstacles,
                &mut walker,
                pos.x,
                pos.y,
                DOCK_POSITION[1],
                -outward.x * STEP,
                -outward.y * STEP,
                STEP / DT,
                DT,
                false,
                false,
            );
            pos += Vec2::new(result.dx, result.dy);
        }
        (pos - center).dot(outward)
    }

    #[test]
    fn transport_dock_collision_contract() {
        for (yaw, class, sign) in [
            (0.0, 0, 1.0),
            (std::f32::consts::FRAC_PI_2, 0, 1.0),
            (0.71, 500, -1.0),
        ] {
            let doors = ZoneDoors::from_dat(&dock_dat(yaw, class, sign));
            assert!(
                doors.dir(u32::from_le_bytes(GATE)).is_none(),
                "gate has no visual routine"
            );
            assert_eq!(doors.collision_rects().len(), 1);
            let mut app = App::new();
            app.insert_resource(doors)
                .init_resource::<SceneState>()
                .init_resource::<Assets<Mesh>>()
                .init_resource::<ObstacleSet>()
                .add_systems(Update, rebuild_obstacles_system);
            for state in [
                None,
                Some(animation::CLOSE_DOOR),
                Some(animation::OPEN_DOOR),
                Some(animation::CLOSE_DOOR),
            ] {
                app.world_mut()
                    .resource_mut::<SceneState>()
                    .snapshot
                    .entities = state.map(gate_entity).into_iter().collect();
                app.update();
                let obstacles = app.world().resource::<ObstacleSet>();
                let position = walk_past_gate(obstacles, yaw);
                if state == Some(animation::OPEN_DOOR) {
                    assert!(obstacles.doors.is_empty());
                    assert!(
                        position < -5.0,
                        "open gate must permit boarding: {position}"
                    );
                } else {
                    assert_eq!(obstacles.doors.len(), 1);
                    assert_eq!(obstacles.doors[0].tris.len(), 12);
                    for (tri, normal) in &obstacles.doors[0].tris {
                        if normal.y > 0.9 {
                            assert!((tri[0].y - obstacles.doors[0].max.y).abs() < 0.0001, "the upper face must remain upward-facing with signed source extents");
                        }
                    }
                    assert!(position > 0.0, "closed gate must stop boarding: {position}");
                }
            }
            app.insert_resource(ZoneDoors::default());
            app.update();
            assert!(app.world().resource::<ObstacleSet>().doors.is_empty());
        }
    }
}
