use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use kuluu_render::{SceneState, WorldEntity};

use super::AppPhase;

pub struct NavmeshOverlayPlugin;

impl Plugin for NavmeshOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NavmeshOverlayVisible>()
            .init_resource::<NavmeshState>()
            .add_systems(
                Update,
                (
                    swap_navmesh_on_zone_change,
                    draw_navmesh_overlay.run_if(overlay_visible),
                )
                    .run_if(in_state(AppPhase::InGame)),
            )
            .add_systems(
                Update,
                snap_entities_to_mzb_floor_system
                    .after(kuluu_render::sync_entities_system)
                    .after(kuluu_render::combat_stance::predict_entities_system)
                    .after(kuluu_render::scene::pin_mount_actors_system)
                    .before(kuluu_render::chase_camera_system)
                    .run_if(in_state(AppPhase::InGame)),
            );
    }
}

#[derive(Resource, Default)]
pub struct NavmeshOverlayVisible(pub bool);

#[derive(Resource, Default)]
pub struct NavmeshState {
    pub nav: Option<Arc<Mutex<ffxi_nav_recast::RecastNavMesh>>>,

    pub edges: Vec<([f32; 3], [f32; 3])>,

    pub zone_id: Option<u16>,

    pub in_myroom: bool,
}

fn overlay_visible(visible: Res<NavmeshOverlayVisible>) -> bool {
    visible.0
}

fn swap_navmesh_on_zone_change(scene: Res<SceneState>, mut state: ResMut<NavmeshState>) {
    let zone_id = scene.snapshot.zone_id;
    let in_myroom = scene.snapshot.myroom.is_some();
    if state.zone_id == zone_id && state.in_myroom == in_myroom {
        return;
    }
    state.zone_id = zone_id;
    state.in_myroom = in_myroom;
    state.edges.clear();
    state.nav = None;

    let Some(zone) = zone_id else {
        return;
    };

    // LSB navmeshes cover the surrounding city, not the Mog House interior model
    // (the zone id stays the city's inside the MH) — re-grounding against the
    // city mesh teleports the player onto the interior model's exterior shell.
    if in_myroom {
        tracing::debug!(zone_id = zone, "in Mog House — city navmesh off");
        return;
    }

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

fn draw_navmesh_overlay(mut gizmos: Gizmos, state: Res<NavmeshState>) {
    let color = Color::srgba(0.25, 1.0, 0.40, 0.75);

    let lift_bevy_y = 0.05;
    for (a, b) in &state.edges {
        let pa = detour_to_bevy(*a) + Vec3::Y * lift_bevy_y;
        let pb = detour_to_bevy(*b) + Vec3::Y * lift_bevy_y;
        gizmos.line(pa, pb, color);
    }
}

// Below this the re-snap is invisible; writing anyway marks every crowd
// Transform changed each frame and defeats Bevy's sparse mesh-uniform uploads.
const GROUND_SNAP_EPSILON_YALMS: f32 = 1e-3;

fn ground_snap_needed(current_y: f32, ground_y: f32) -> bool {
    (current_y - ground_y).abs() > GROUND_SNAP_EPSILON_YALMS
}

// Remote Mob/Pc/Pet/Npc ground movers are grounded by combat_stance::ground_remote_movers_system
// on the MZB collision mesh every frame: LSB grounds them to the Detour navmesh (pathfind.cpp
// CPathFind::StepTo walks Y to a waypoint from detour_navmesh.cpp), so the POS packet Y only picks the
// level. This system keeps two jobs: snapping self onto its wire Y when no collision is loaded,
// and grounding the static Other kind (doors/transports), whose wire position is an authored
// placement rather than a pathfind step. Under an unloaded interior shell the entity keeps
// its reported pose: the shell cannot supply a floor.
fn snap_entities_to_mzb_floor_system(
    collision_geom: Res<kuluu_render::dat_mzb::MzbCollisionGeometry>,
    interiors: Res<kuluu_render::sub_area_activation::SubAreaActivation>,
    scene: Res<SceneState>,
    tracked: Res<kuluu_render::scene::TrackedEntities>,
    mut q: Query<
        (
            Entity,
            &WorldEntity,
            &mut Transform,
            Has<kuluu_render::components::IsSelf>,
        ),
        With<WorldEntity>,
    >,
) {
    if collision_geom.tri_count() == 0 {
        let wire_self_y = kuluu_render::ffxi_to_bevy(scene.snapshot.self_pos.pos).y;
        for (_entity, _world, mut t, is_self) in &mut q {
            if is_self && ground_snap_needed(t.translation.y, wire_self_y) {
                t.translation.y = wire_self_y;
            }
        }
        return;
    }
    for (_entity, world, mut t, is_self) in &mut q {
        if is_self || kuluu_render::scene::mount_actor_rider(world.id).is_some() {
            continue;
        }
        if interiors.unloaded_interior_at([t.translation.x, -t.translation.y, -t.translation.z]) {
            continue;
        }
        if matches!(world.kind, kuluu_snapshot::EntityKind::Other) {
            if let Some(ground) = collision_geom
                .ground_nearest(Vec2::new(t.translation.x, t.translation.z), t.translation.y)
            {
                if ground_snap_needed(t.translation.y, ground) {
                    t.translation.y = ground;
                }
            }
        }
    }
    for (&id, &mount) in &tracked.by_id {
        let Some(rider_id) = kuluu_render::scene::mount_actor_rider(id) else {
            continue;
        };
        let Some(&rider) = tracked.by_id.get(&rider_id) else {
            continue;
        };
        let Ok((_, _, rider_transform, _)) = q.get(rider) else {
            continue;
        };
        let rider_transform = *rider_transform;
        if let Ok((_, _, mut mount_transform, _)) = q.get_mut(mount) {
            if *mount_transform != rider_transform {
                *mount_transform = rider_transform;
            }
        }
    }
}

#[inline]
fn detour_to_bevy(d: [f32; 3]) -> Vec3 {
    Vec3::new(d[0], d[1], d[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuluu_render::combat_stance::{predict_entities_system, EntityPrediction, MotionProbe};
    use kuluu_render::dat_mzb::{MzbCollisionBlock, MzbCollisionGeometry};
    use kuluu_render::scene::{mount_actor_id, pin_mount_actors_system, TrackedEntities};
    use kuluu_snapshot::EntityKind;

    const REMOTE_ID: u32 = 123;
    const TEST_STEP_RISE: f32 = 0.25;
    const TEST_FLOOR_HEIGHT: f32 = 3.0;
    const TEST_WIRE_HEIGHT: f32 = 1.6;
    const TEST_UPPER_FLOOR: f32 = 6.0;

    fn floors(patches: &[(f32, f32, f32)]) -> MzbCollisionGeometry {
        let mut block = MzbCollisionBlock::default();
        for &(start, end, height) in patches {
            let base = block.positions.len() as u32;
            block.positions.extend([
                Vec3::new(start, height, -1.0),
                Vec3::new(end, height, -1.0),
                Vec3::new(end, height, 1.0),
                Vec3::new(start, height, 1.0),
            ]);
            block
                .indices
                .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            block.tri_normals.extend([Vec3::Y; 2]);
        }
        MzbCollisionGeometry::from_block(block)
    }

    /// Minimal app for the snap tests: predict_entities_system reads the probe,
    /// which production inserts in the render plugin.
    fn remote_app(geometry: MzbCollisionGeometry, kind: EntityKind) -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<SceneState>()
            .init_resource::<EntityPrediction>()
            .init_resource::<kuluu_render::scheduler_runtime::CutsceneActorState>()
            .init_resource::<TrackedEntities>()
            .init_resource::<kuluu_render::sub_area_activation::SubAreaActivation>()
            .insert_resource(MotionProbe::init())
            .insert_resource(geometry)
            .add_systems(
                Update,
                (
                    predict_entities_system,
                    pin_mount_actors_system,
                    snap_entities_to_mzb_floor_system,
                )
                    .chain(),
            );
        let entity = app
            .world_mut()
            .spawn((
                WorldEntity {
                    id: REMOTE_ID,
                    act_index: 1,
                    kind,
                },
                kuluu_render::components::InGameEntity,
                Transform::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<TrackedEntities>()
            .by_id
            .insert(REMOTE_ID, entity);
        (app, entity)
    }

    /// Advance one frame with a synthetic POS update. These updates carry no
    /// wire speed byte; zero-speed-with-move is the case CPathFind::StepTo
    /// substitutes its own speed for, so nothing here is invented.
    fn frame(app: &mut App, entity: Entity, incoming: Vec3) -> Vec3 {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
        let mut prediction = app.world_mut().resource_mut::<EntityPrediction>();
        prediction.observe(REMOTE_ID, incoming, 0, 0, 0);
        prediction.by_id.get_mut(&REMOTE_ID).unwrap().rendered_pos = incoming;
        app.update();
        app.world().get::<Transform>(entity).unwrap().translation
    }

    #[test]
    fn other_entities_keep_their_grounded_height_without_accumulating_offsets() {
        let (mut app, entity) = remote_app(
            floors(&[(0.0, 2.0, TEST_FLOOR_HEIGHT), (0.0, 2.0, TEST_UPPER_FLOOR)]),
            EntityKind::Other,
        );
        app.world_mut()
            .get_mut::<Transform>(entity)
            .unwrap()
            .translation = Vec3::new(0.5, TEST_WIRE_HEIGHT, 0.0);
        for _ in 0..3 {
            app.update();
            assert!(
                (app.world().get::<Transform>(entity).unwrap().translation.y - TEST_FLOOR_HEIGHT)
                    .abs()
                    < GROUND_SNAP_EPSILON_YALMS
            );
        }
    }

    #[test]
    fn rider_and_mount_share_the_server_resolved_pose() {
        let (mut app, rider) = remote_app(floors(&[(0.0, 0.9, TEST_FLOOR_HEIGHT)]), EntityKind::Pc);
        let mount_id = mount_actor_id(REMOTE_ID);
        let mount = app
            .world_mut()
            .spawn((
                WorldEntity {
                    id: mount_id,
                    act_index: 1,
                    kind: EntityKind::Pc,
                },
                Transform::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<TrackedEntities>()
            .by_id
            .insert(mount_id, mount);
        frame(&mut app, rider, Vec3::new(0.5, TEST_WIRE_HEIGHT, 0.0));
        let held = frame(&mut app, rider, Vec3::new(1.0, TEST_WIRE_HEIGHT, 0.0));
        assert_eq!(
            held.y, TEST_WIRE_HEIGHT,
            "Y is server-resolved: no client re-grounding"
        );
        assert_eq!(
            held,
            app.world().get::<Transform>(mount).unwrap().translation,
            "the mount copies the rider's pose"
        );
    }

    #[test]
    fn newly_spawned_mount_copies_the_rider_pose() {
        let (mut app, rider) = remote_app(floors(&[(0.0, 0.9, TEST_FLOOR_HEIGHT)]), EntityKind::Pc);
        frame(&mut app, rider, Vec3::new(0.5, TEST_WIRE_HEIGHT, 0.0));
        frame(&mut app, rider, Vec3::new(1.0, TEST_WIRE_HEIGHT, 0.0));
        let mount_id = mount_actor_id(REMOTE_ID);
        let mount = app
            .world_mut()
            .spawn((
                WorldEntity {
                    id: mount_id,
                    act_index: 1,
                    kind: EntityKind::Pc,
                },
                Transform::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<TrackedEntities>()
            .by_id
            .insert(mount_id, mount);
        let held = frame(&mut app, rider, Vec3::new(1.2, TEST_WIRE_HEIGHT, 0.0));
        assert_eq!(
            held.y, TEST_WIRE_HEIGHT,
            "Y is server-resolved: no client re-grounding"
        );
        assert_eq!(
            held,
            app.world().get::<Transform>(mount).unwrap().translation
        );
    }

    #[test]
    fn ground_snap_skips_sub_epsilon_deltas() {
        assert!(!ground_snap_needed(10.0, 10.0));
        assert!(!ground_snap_needed(
            10.0,
            10.0 + GROUND_SNAP_EPSILON_YALMS * 0.5
        ));
        assert!(ground_snap_needed(
            10.0,
            10.0 + GROUND_SNAP_EPSILON_YALMS * 2.0
        ));
        assert!(ground_snap_needed(
            10.0,
            10.0 - GROUND_SNAP_EPSILON_YALMS * 2.0
        ));
    }
    const BOARDING_INTERIOR: u32 = 485;
    const BOARDING_ZONE: u16 = 108;
    const BOARDING_POSITION: Vec3 = Vec3::new(0.5, TEST_WIRE_HEIGHT, 0.0);

    fn boarding_activation() -> kuluu_render::sub_area_activation::SubAreaActivation {
        let mut activation = kuluu_render::sub_area_activation::SubAreaActivation::default();
        let file_id =
            ffxi_dat::zone_dat::effective_zone_dat_file_id(Some(BOARDING_ZONE), None).unwrap();
        activation.install_zone(
            file_id,
            &[],
            vec![ffxi_dat::sub_area::SubAreaShell {
                id: BOARDING_INTERIOR,
                min: [0.0, -TEST_UPPER_FLOOR, -1.0],
                max: [2.0, -1.0, 1.0],
            }],
            vec![BOARDING_INTERIOR],
        );
        activation
    }

    /// Pinned by scripts/checks.sh run_contracts (the ferry passenger guarantee
    /// from main): a remote passenger under an unloaded interior shell keeps
    /// its reported pose. Y is server-resolved: the wire height holds even over
    /// a loaded floor; no client re-grounding happens for the battle kinds.
    #[test]
    fn remote_passenger_keeps_reported_height_under_unloaded_interior_shell() {
        let (mut app, passenger) =
            remote_app(floors(&[(0.0, 4.0, TEST_UPPER_FLOOR)]), EntityKind::Pc);
        assert_eq!(
            frame(&mut app, passenger, BOARDING_POSITION).y,
            TEST_WIRE_HEIGHT
        );
        app.insert_resource(boarding_activation());
        assert_eq!(
            frame(&mut app, passenger, BOARDING_POSITION),
            BOARDING_POSITION
        );
        let moved = BOARDING_POSITION + Vec3::X * TEST_STEP_RISE;
        assert_eq!(frame(&mut app, passenger, moved), moved);
    }

    /// sync_entities_system owns the Other kind's x/z; this harness runs only
    /// the snap system, so the transform is seeded the way sync writes it on
    /// ingest. Outside the shell the door grounds onto the upper floor by the
    /// direct probe; under the unloaded shell a fresh ingest must retain the
    /// reported (wire) pose instead of probing a floor the observer does not
    /// have loaded.
    #[test]
    fn other_entity_keeps_reported_pose_under_unloaded_interior_shell() {
        let (mut app, door) =
            remote_app(floors(&[(0.0, 4.0, TEST_UPPER_FLOOR)]), EntityKind::Other);
        app.world_mut()
            .get_mut::<Transform>(door)
            .unwrap()
            .translation = BOARDING_POSITION;
        app.update();
        assert_eq!(
            app.world().get::<Transform>(door).unwrap().translation.y,
            TEST_UPPER_FLOOR
        );
        app.insert_resource(boarding_activation());
        let seed = |app: &mut App, door: Entity, pose: Vec3| {
            app.world_mut()
                .get_mut::<Transform>(door)
                .unwrap()
                .translation = pose;
        };
        seed(&mut app, door, BOARDING_POSITION);
        app.update();
        assert_eq!(
            app.world().get::<Transform>(door).unwrap().translation,
            BOARDING_POSITION
        );
        let moved = BOARDING_POSITION + Vec3::X * TEST_STEP_RISE;
        seed(&mut app, door, moved);
        app.update();
        assert_eq!(
            app.world().get::<Transform>(door).unwrap().translation,
            moved
        );
    }

    /// sync_entities_system owns the Other kind's x/z; this harness runs only
    /// the snap system, so the transform is seeded the way sync writes it on
    /// ingest.
    #[test]
    fn other_entity_grounds_on_loaded_interior_floor() {
        use bevy::ecs::system::RunSystemOnce;
        use kuluu_render::dat_mzb::{
            LoadMzbInFlight, LoadMzbRequest, PendingWaterSpawns, ZoneAreaMap, ZoneChunkLightMap,
        };
        use kuluu_render::sub_area_activation::{
            drive_sub_area_activation, SetSubArea, SubAreaActivation, SubAreaChanged,
        };
        let (mut app, door) =
            remote_app(floors(&[(0.0, 4.0, TEST_UPPER_FLOOR)]), EntityKind::Other);
        app.world_mut()
            .get_mut::<Transform>(door)
            .unwrap()
            .translation = BOARDING_POSITION;
        app.add_message::<SetSubArea>()
            .add_message::<SubAreaChanged>()
            .add_message::<LoadMzbRequest>()
            .init_resource::<ZoneAreaMap>()
            .init_resource::<ZoneChunkLightMap>()
            .init_resource::<PendingWaterSpawns>()
            .init_resource::<kuluu_render::dat_mmb::MmbLoadQueue>()
            .init_resource::<LoadMzbInFlight>()
            .insert_resource(boarding_activation());
        {
            let mut scene = app.world_mut().resource_mut::<SceneState>();
            scene.snapshot.zone_id = Some(BOARDING_ZONE);
            scene.snapshot.self_pos.pos = kuluu_snapshot::Vec3 {
                x: BOARDING_POSITION.x,
                y: -BOARDING_POSITION.z,
                z: -BOARDING_POSITION.y,
            };
            scene.snapshot.sub_area = Some(BOARDING_INTERIOR as u16);
        }
        app.world_mut()
            .run_system_once(drive_sub_area_activation)
            .unwrap();
        assert_eq!(
            app.world().resource::<SubAreaActivation>().active(),
            Some(BOARDING_INTERIOR)
        );
        let interior = floors(&[(0.0, 4.0, TEST_FLOOR_HEIGHT)])
            .block(kuluu_render::dat_mzb::ZONE_SLOT_MAIN)
            .clone();
        app.world_mut()
            .resource_mut::<MzbCollisionGeometry>()
            .set_block(kuluu_render::dat_mzb::ZONE_SLOT_SUB_AREA, interior);
        app.update();
        assert_eq!(
            app.world().get::<Transform>(door).unwrap().translation.y,
            TEST_FLOOR_HEIGHT
        );
    }
}
