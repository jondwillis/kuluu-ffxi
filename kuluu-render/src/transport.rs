#![cfg(not(target_arch = "wasm32"))]

use crate::{
    components::WorldEntity, dat_mmb::LoadMmbRequest, entity_table::EntityTable,
    vana_time::VanaClock,
};
use bevy::prelude::*;
use bevy::tasks::{futures_lite::future, AsyncComputeTaskPool, Task};
use ffxi_dat::{
    scheduler::Scheduler,
    vehicle::{transport_file_id, PointList, Spline, POINT_LIST_KIND},
    walk, ChunkKind, DatRoot,
};
use kuluu_snapshot::EntityLook;
use std::collections::HashMap;

use ffxi_vocab::transport::MODEL_SHIP;

// FFXiMain.dll SHA-256 f4f90fbd080c05448aab3f866b127d7c1675b3cc15c8beaa57bfc584064b7e7c.
// RVA 0x96E90 maps ship animations to seq0..seq7.
const FIRST_SHIP_ANIMATION: u8 = 18;
const SHIP_ANIMATION_COUNT: u8 = 8;
const PATH_FORWARD: u32 = 8;
const PATH_FACE_DIRECTION: u32 = 2;

#[derive(Component)]
struct LoadingTransport {
    file_id: u32,
    task: Task<Option<TransportAsset>>,
}
#[derive(Component, Default)]
struct TransportAsset {
    meshes: Vec<usize>,
    routines: Vec<Scheduler>,
    paths: HashMap<[u8; 4], Spline>,
}

fn load_transport(file_id: u32) -> Option<TransportAsset> {
    let root = DatRoot::from_env_or_default().ok()?;
    let bytes = std::fs::read(root.resolve(file_id).ok()?.path_under(&root)).ok()?;
    let mut asset = TransportAsset {
        meshes: Vec::new(),
        routines: Vec::new(),
        paths: HashMap::new(),
    };
    for (idx, chunk) in walk(&bytes).filter_map(Result::ok).enumerate() {
        match ChunkKind::from_u8(chunk.kind) {
            Some(ChunkKind::Mmb) => asset.meshes.push(idx),
            Some(ChunkKind::Scheduler) => {
                if let Ok(s) = Scheduler::parse(chunk.name, chunk.data) {
                    asset.routines.push(s);
                }
            }
            _ if chunk.kind == POINT_LIST_KIND => {
                if let Some(p) = PointList::parse(chunk.data).and_then(|p| Spline::new(p.0)) {
                    asset.paths.insert(chunk.name, p);
                }
            }
            _ => {}
        }
    }
    Some(asset)
}

fn load_transport_models(
    mut commands: Commands,
    entities: Res<EntityTable>,
    candidates: Query<(Entity, &WorldEntity), (Without<TransportAsset>, Without<LoadingTransport>)>,
    mut loading: Query<(Entity, &WorldEntity, &mut LoadingTransport)>,
    mut models: MessageWriter<LoadMmbRequest>,
) {
    for (entity, world) in &candidates {
        let Some(record) = entities.get(world.id) else {
            continue;
        };
        let Some(EntityLook::Transport {
            size: MODEL_SHIP,
            model_id: Some(selector),
            ..
        }) = record.entity.look
        else {
            continue;
        };
        let Some(file_id) = transport_file_id(selector) else {
            continue;
        };
        commands.entity(entity).insert(LoadingTransport {
            file_id,
            task: AsyncComputeTaskPool::get().spawn(async move { load_transport(file_id) }),
        });
    }
    for (entity, world, mut pending) in &mut loading {
        let Some(result) = future::block_on(future::poll_once(&mut pending.task)) else {
            continue;
        };
        let asset = result.unwrap_or_default();
        {
            for &chunk_idx in &asset.meshes {
                models.write(LoadMmbRequest {
                    file_id: pending.file_id,
                    chunk_idx,
                    world_pos: Vec3::ZERO,
                    entity_id: Some(world.id),
                    world_transform: None,
                    water: None,
                    lod: None,
                    door: None,
                    slot: 0,
                    sub_area_link: 0,
                    voyage_backdrop: false,
                });
            }
            commands
                .entity(entity)
                .insert((asset, TransportPlayback::default()));
        }
        commands.entity(entity).remove::<LoadingTransport>();
    }
}

fn transport_pose(
    asset: &TransportAsset,
    animation: u8,
    elapsed_frames: f32,
) -> Option<(Vec3, Option<f32>)> {
    let sequence = animation.checked_sub(FIRST_SHIP_ANIMATION)?;
    if sequence >= SHIP_ANIMATION_COUNT {
        return None;
    }
    let name = [b's', b'e', b'q', b'0' + sequence];
    let routine = asset.routines.iter().find(|r| r.name == name)?;
    let mut position = None;
    let mut yaw = None;
    for stage in &routine.stages {
        let Some(motion) = stage.stage.follow_points else {
            continue;
        };
        if stage.frame as f32 > elapsed_frames {
            break;
        }
        let Some(path) = asset.paths.get(&stage.stage.id) else {
            continue;
        };
        let duration = stage.stage.duration_frames as f32;
        let progress = if duration > 0.0 {
            ((elapsed_frames - stage.frame as f32) / duration).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let forward = motion.flags & PATH_FORWARD != 0;
        let t = eased_progress(
            if forward { progress } else { 1.0 - progress },
            motion.easing,
        );
        position = Some(Vec3::from_array(path.sample(t)));
        if motion.flags & PATH_FACE_DIRECTION != 0 {
            let delta = Vec3::from_array(path.tangent(t));
            let delta = if forward { delta } else { -delta };
            if delta.x != 0.0 || delta.z != 0.0 {
                yaw = Some(-delta.z.atan2(delta.x) + motion.rotation);
            }
        }
    }
    position.map(|p| (p, yaw))
}

// FFXiMain.dll SHA-256 f4f90fbd080c05448aab3f866b127d7c1675b3cc15c8beaa57bfc584064b7e7c.
// Point-list motion task RVA 0x5DCC0.
fn eased_progress(t: f32, mode: u32) -> f32 {
    match mode {
        1 => (t * std::f32::consts::FRAC_PI_2).sin(),
        2 => 1.0 - (t * std::f32::consts::FRAC_PI_2).cos(),
        4 => 0.5 - 0.5 * (t * std::f32::consts::PI).cos(),
        _ => t,
    }
}

#[derive(Component, Default)]
struct TransportPlayback {
    key: Option<(u8, Option<u32>)>,
    offset_frames: f32,
}

impl TransportPlayback {
    // FFXiMain.dll SHA-256 f4f90fbd080c05448aab3f866b127d7c1675b3cc15c8beaa57bfc584064b7e7c.
    // RVA 0x96E90.
    fn elapsed(&mut self, animation: u8, start: Option<u32>, frames: f32) -> f32 {
        const SEEK_THRESHOLD_FRAMES: f32 = 600.0;
        let key = Some((animation, start));
        if self.key != key {
            self.key = key;
            self.offset_frames = if frames < SEEK_THRESHOLD_FRAMES {
                frames
            } else {
                0.0
            };
        }
        (frames - self.offset_frames).max(0.0)
    }
}

fn pose_transports(
    table: Res<EntityTable>,
    clock: Res<VanaClock>,
    mut query: Query<(
        &WorldEntity,
        &TransportAsset,
        &mut TransportPlayback,
        &mut Transform,
        &mut Visibility,
    )>,
) {
    for (world, asset, mut playback, mut transform, mut visibility) in &mut query {
        let Some(record) = table.get(world.id) else {
            continue;
        };
        let wire = &record.entity;
        *visibility = if wire.is_invisible() {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        let Some(EntityLook::Transport {
            animation_start, ..
        }) = wire.look
        else {
            continue;
        };
        let native = Vec3::new(wire.pos.x, wire.pos.z, wire.pos.y);
        let elapsed = animation_start.map(|start| {
            (clock.earth_unix_now() - crate::vana_time::EARTH_EPOCH_UNIX as f64 - start as f64)
                .max(0.0) as f32
                * crate::scheduler_runtime::ROUTINE_FPS
        });
        let (position, yaw) = elapsed
            .and_then(|time| {
                transport_pose(
                    asset,
                    wire.animation,
                    playback.elapsed(wire.animation, animation_start, time),
                )
            })
            .unwrap_or((native, None));
        let yaw = yaw.unwrap_or(wire.heading as f32 * std::f32::consts::TAU / 256.0);
        *transform = Transform::from_matrix(crate::dat_mzb::placement_bevy_transform(
            Vec3::ONE,
            Vec3::new(0.0, yaw, 0.0),
            position,
        ));
    }
}

pub struct TransportPlugin;
impl Plugin for TransportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VoyageState>()
            .add_systems(
                Update,
                load_transport_models.after(crate::scene::sync_entities_system),
            )
            .add_systems(
                PostUpdate,
                (pose_transports, pose_voyage).before(bevy::transform::TransformSystems::Propagate),
            );
    }
}

#[derive(Component)]
pub struct VoyageBackdrop(pub Mat4);

#[derive(Resource, Default)]
pub struct VoyageState {
    file_id: Option<u32>,
    routes: Vec<([u8; 4], ffxi_dat::vehicle::VoyageRoute)>,
}

impl VoyageState {
    pub fn new(file_id: u32, routes: Vec<([u8; 4], ffxi_dat::vehicle::VoyageRoute)>) -> Self {
        Self {
            file_id: Some(file_id),
            routes,
        }
    }
}

fn voyage_progress(zone: u16, now: f64, timing: Option<kuluu_snapshot::Voyage>) -> (f32, bool, u8) {
    if let Some(timing) = timing.filter(|t| t.duration != 0) {
        return (
            ((now - timing.start as f64) / f64::from(timing.duration)).clamp(0.0, 1.0) as f32,
            timing.reverse,
            timing.route,
        );
    }
    // vendor/server/src/map/transport.cpp CTransportHandler::TransportTimer; LSB LOGIN leaves voyage timing empty.
    const EARTH_SECONDS_PER_VANA_MINUTE: f64 =
        crate::vana_time::EARTH_SECS_PER_VANA_HOUR as f64 / 60.0;
    let Some(s) = ffxi_vocab::transport::voyage(zone) else {
        return (0.0, false, 0);
    };
    let interval = f64::from(s.interval);
    let departure = f64::from(s.arrival + s.waiting + s.departure);
    let phase = (now / EARTH_SECONDS_PER_VANA_MINUTE - f64::from(s.offset)).rem_euclid(interval);
    let departure_event = f64::from(s.arrival + s.waiting);
    if phase >= departure_event && phase < departure {
        return (0.0, false, 0);
    }
    let elapsed = (phase - departure).rem_euclid(interval);
    let duration = interval + f64::from(s.arrival)
        - f64::from(ffxi_vocab::transport::EVICTION_LEAD_VANA_MINUTES)
        - departure;
    ((elapsed / duration).clamp(0.0, 1.0) as f32, false, 0)
}

fn pose_voyage(
    scene: Res<crate::snapshot::SceneState>,
    clock: Res<VanaClock>,
    mut voyage: ResMut<VoyageState>,
    mut query: Query<(&VoyageBackdrop, &mut Transform)>,
) {
    if voyage.file_id != crate::snapshot::effective_zone_file_id(&scene.snapshot) {
        *voyage = VoyageState::default();
        return;
    }
    let now = clock.earth_unix_now() - crate::vana_time::EARTH_EPOCH_UNIX as f64;
    let (progress, reverse, selector) = voyage_progress(
        scene.snapshot.zone_id.unwrap_or_default(),
        now,
        scene.snapshot.voyage,
    );
    let name = if selector <= 3 {
        [b'c', b'0', b'0', b'0' + selector]
    } else {
        *b"c000"
    };
    let Some((_, route)) = voyage
        .routes
        .iter()
        .find(|(key, _)| *key == name)
        .or_else(|| voyage.routes.iter().find(|(key, _)| *key == *b"c000"))
    else {
        return;
    };
    let t = if reverse { 1.0 - progress } else { progress };
    let position = Vec3::from_array(route.position.sample(t));
    let target = Vec3::from_array(route.facing.sample(t));
    let direction = if reverse {
        position - target
    } else {
        target - position
    };
    let yaw = std::f32::consts::FRAC_PI_2 - direction.z.atan2(direction.x);
    let basis = crate::dat_mzb::placement_bevy_transform(Vec3::ONE, Vec3::ZERO, Vec3::ZERO);
    let frame = Mat4::from_rotation_y(yaw).inverse() * Mat4::from_translation(-position);
    let world_to_ship = basis * frame * basis;
    for (backdrop, mut transform) in &mut query {
        *transform = Transform::from_matrix(world_to_ship * backdrop.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::scheduler::{FollowPoints, SchedulerStage, StageKind, TimedStage};

    fn stage(frame: u32, id: [u8; 4], flags: u32, easing: u32) -> TimedStage {
        TimedStage {
            frame,
            stage: SchedulerStage {
                kind: StageKind::FollowPoints,
                raw_type: 0x27,
                actor_fade: None,
                idle_transition_time: None,
                flinch_duration: None,
                delay_frames: 0,
                duration_frames: 60,
                id,
                max_loops: 0,
                transition_in: 0,
                transition_out: 0,
                model_transform: None,
                follow_points: Some(FollowPoints {
                    flags,
                    easing,
                    rotation: 0.0,
                }),
                screen_color: None,
                random_group: None,
                local_dir: [0; 4],
            },
        }
    }

    #[test]
    fn transport_state_contract() {
        let asset = TransportAsset {
            routines: vec![Scheduler {
                name: *b"seq0",
                stages: vec![stage(0, *b"path", 3, 2), stage(60, *b"dock", 9, 0)],
            }],
            paths: HashMap::from([
                (
                    *b"path",
                    Spline::new(vec![[10.0, 0.0, 0.0], [110.0, 0.0, 0.0]]).unwrap(),
                ),
                (
                    *b"dock",
                    Spline::new(vec![[10.0, 0.0, 0.0], [10.0, 0.0, 30.0]]).unwrap(),
                ),
            ]),
            ..Default::default()
        };
        let start = transport_pose(&asset, FIRST_SHIP_ANIMATION, 0.0).unwrap();
        assert!(start.0.distance(Vec3::new(110.0, 0.0, 0.0)) < 0.001);
        let mid = transport_pose(&asset, FIRST_SHIP_ANIMATION, 30.0).unwrap();
        assert!(mid.0.distance(Vec3::new(39.289_32, 0.0, 0.0)) < 0.001);
        assert!((mid.1.unwrap().abs() - std::f32::consts::PI).abs() < 0.001);
        let dock = transport_pose(&asset, FIRST_SHIP_ANIMATION, 90.0).unwrap();
        assert!(dock.0.distance(Vec3::new(10.0, 0.0, 15.0)) < 0.001);
        let late = transport_pose(&asset, FIRST_SHIP_ANIMATION, 9000.0).unwrap();
        assert!(late.0.distance(Vec3::new(10.0, 0.0, 30.0)) < 0.001);
        assert!(transport_pose(&asset, 0, 0.0).is_none());

        assert!((eased_progress(0.5, 1) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.0001);
        let mut playback = TransportPlayback::default();
        assert_eq!(playback.elapsed(18, Some(100), 300.0), 0.0);
        assert_eq!(playback.elapsed(18, Some(100), 360.0), 60.0);
        assert_eq!(playback.elapsed(19, Some(200), 900.0), 900.0);
        assert_eq!(playback.elapsed(18, Some(300), 600.0), 600.0);
        voyage_frame_contract();

        let timing = kuluu_snapshot::Voyage {
            start: 1000,
            duration: 900,
            reverse: true,
            route: 2,
        };
        assert_eq!(voyage_progress(228, 1450.0, Some(timing)), (0.5, true, 2));
        assert_eq!(voyage_progress(228, 999.0, Some(timing)).0, 0.0);
        assert_eq!(voyage_progress(228, 2000.0, Some(timing)).0, 1.0);
        let seconds = |minute: f64| minute * 2.4;
        assert_eq!(voyage_progress(228, seconds(0.0), None).0, 0.0);
        assert_eq!(voyage_progress(228, seconds(16.0), None).0, 0.0);
        assert_eq!(voyage_progress(228, seconds(17.0), None).0, 0.0);
        assert_eq!(voyage_progress(228, seconds(390.0), None).0, 1.0);
        let halfway = voyage_progress(228, seconds(203.5), None).0;
        assert!((halfway - 0.5).abs() < 0.0001);
        assert!((voyage_progress(228, seconds(683.5), None).0 - halfway).abs() < 0.0001);
    }

    fn voyage_frame_contract() {
        use crate::snapshot::SceneState;
        let mut app = App::new();
        let clock = VanaClock::anchored_at_hour(6.0);
        app.add_plugins(MinimalPlugins)
            .insert_resource(clock)
            .insert_resource(SceneState {
                snapshot: kuluu_snapshot::SceneSnapshot {
                    zone_id: Some(228),
                    voyage: Some(kuluu_snapshot::Voyage {
                        start: 2000,
                        duration: 900,
                        reverse: false,
                        route: 0,
                    }),
                    ..Default::default()
                },
                ..Default::default()
            })
            .insert_resource(VoyageState::new(
                328,
                vec![(
                    *b"c000",
                    ffxi_dat::vehicle::VoyageRoute {
                        position: Spline::new(vec![[100.0, 0.0, 200.0], [110.0, 0.0, 210.0]])
                            .unwrap(),
                        facing: Spline::new(vec![[100.0, 0.0, 210.0], [110.0, 0.0, 220.0]])
                            .unwrap(),
                    },
                )],
            ))
            .add_systems(Update, pose_voyage);
        app.world_mut()
            .resource_mut::<SceneState>()
            .snapshot
            .voyage
            .as_mut()
            .unwrap()
            .route = 2;
        {
            let mut voyage = app.world_mut().resource_mut::<VoyageState>();
            let mut decoy = voyage.routes[0].clone();
            decoy.0 = *b"c003";
            decoy.1.position =
                Spline::new(vec![[999.0, 0.0, 999.0], [1000.0, 0.0, 1000.0]]).unwrap();
            voyage.routes.insert(0, decoy);
        }
        let backdrop = app
            .world_mut()
            .spawn((
                VoyageBackdrop(Mat4::from_translation(Vec3::new(100.0, 0.0, -200.0))),
                Transform::IDENTITY,
            ))
            .id();
        let passenger = app
            .world_mut()
            .spawn(Transform::from_xyz(0.15, 2.1, -3.25))
            .id();
        app.update();
        assert!(
            app.world()
                .get::<Transform>(backdrop)
                .unwrap()
                .translation
                .length()
                < 0.001
        );
        assert_eq!(
            app.world().get::<Transform>(passenger).unwrap().translation,
            Vec3::new(0.15, 2.1, -3.25)
        );
        app.world_mut()
            .resource_mut::<SceneState>()
            .snapshot
            .voyage
            .as_mut()
            .unwrap()
            .route = u8::MAX;
        app.update();
        assert!(
            app.world()
                .get::<Transform>(backdrop)
                .unwrap()
                .translation
                .length()
                < 0.001,
            "route transform must not accumulate"
        );
        app.world_mut()
            .resource_mut::<SceneState>()
            .snapshot
            .zone_id = Some(248);
        app.update();
        assert!(app.world().resource::<VoyageState>().routes.is_empty());
    }

    #[test]
    fn installed_ferry_has_a_walkable_ship_and_separate_backdrop() {
        if ffxi_dat::DatRoot::from_env_or_default().is_err() {
            return;
        }
        AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        const FERRY_ZONE_FILE: u32 = 328;
        let (submeshes, instances) =
            crate::dat_mzb::load_mzb_placed(FERRY_ZONE_FILE, None).unwrap();
        let geometry =
            crate::dat_mzb::build_collision_geometry(&submeshes, &instances, Some(FERRY_ZONE_FILE));
        let mut collision = crate::dat_mzb::MzbCollisionGeometry::default();
        collision.set_block(0, geometry);
        let floor = collision
            .ground_nearest(Vec2::new(0.15, -3.25), 2.1)
            .expect("ship deck at LSB zone-in position");
        assert!((floor - 2.1).abs() < 0.1, "floor={floor}");
        let build = crate::dat_mzb::build_zone_mmb_spawns(FERRY_ZONE_FILE, None, None).unwrap();
        assert!(!build.voyage_routes.is_empty());
        assert!(build.spawns.iter().any(|s| s.voyage_backdrop));
        assert!(build
            .spawns
            .iter()
            .any(|s| !s.voyage_backdrop && s.door.is_some()));
        assert!(build
            .spawns
            .iter()
            .filter(|s| !s.voyage_backdrop)
            .all(|s| s.chunk_idx > 220));
    }
}
