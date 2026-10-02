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

// FFXiMain.dll horizonxi-2023 RVA 0x96E90 / retail-2026-09 RVA 0x97A80 indexes the actor's
// animation number into an .rdata 4CC table (horizonxi-2023 RVA 0x3292B4 / retail-2026-09
// RVA 0x32D42C) whose entries 18..25 read seq0..seq7.
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

fn load_transport(root: &DatRoot, file_id: u32) -> Option<TransportAsset> {
    let bytes = std::fs::read(root.resolve(file_id).ok()?.path_under(root)).ok()?;
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
    dat_root: Res<crate::dat_root::SharedDatRoot>,
) {
    let Some(root) = dat_root.get() else {
        return;
    };
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
            task: {
                let root = root.clone();
                AsyncComputeTaskPool::get().spawn(async move { load_transport(&root, file_id) })
            },
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
                    light_bindings: Default::default(),
                    area_id: 0,
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

// FFXiMain.dll horizonxi-2023 RVA 0x5DCC0 / retail-2026-09 RVA 0x5E890: point-list motion
// task ctor. Its easing switch (sin, 1-cos, half-sin, cos-blend) is horizonxi-2023 RVA 0x54F00
// / retail-2026-09 RVA 0x55AD0.
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
    // FFXiMain.dll horizonxi-2023 RVA 0x96E90 / retail-2026-09 RVA 0x97A80 hands (start, now)
    // frames to the seek routine at horizonxi-2023 RVA 0xC36A0 / retail-2026-09 RVA 0xC4730
    // whose `cmp eax, 0x258` is SEEK_THRESHOLD_FRAMES.
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
    // vendor/server/src/map/transports/ship_handler.cpp ShipHandler::tick; LSB LOGIN leaves
    // ShipStart/ShipEnd empty, so the crossing is timed from the ship's own cycle: the
    // backdrop holds at the berth from the moment boarding closes until the ship is hidden,
    // then runs until riders are put ashore.
    let Some(s) = ffxi_vocab::transport::voyage(zone) else {
        return (0.0, false, 0);
    };
    let every = f64::from(s.every);
    let into = (now - f64::from(s.offset)).rem_euclid(every);
    let held = (f64::from(s.departs) - f64::from(s.boarding_ends)).rem_euclid(every);
    if (into - f64::from(s.boarding_ends)).rem_euclid(every) < held {
        return (0.0, false, 0);
    }
    let elapsed = (into - f64::from(s.departs)).rem_euclid(every);
    let duration = (f64::from(s.disembark) - f64::from(s.departs)).rem_euclid(every);
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
                stage_words: ffxi_dat::scheduler::SYNTHESIZED_STAGE_WORDS,
                kind: StageKind::FollowPoints,
                raw_type: 0x27,
                actor_fade: None,
                idle_transition_time: None,
                flinch_duration: None,
                model_visibility: None,
                spell_effect: None,
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
        // vendor/server/data/zones/mhaura/zone.yaml mhaura_selbina_boat: every 1152,
        // offset 920, docked ends at 233, departing hides the ship at 272, riders
        // ashore at 26.
        let s = ffxi_vocab::transport::voyage(228).unwrap();
        assert_eq!(
            (s.every, s.offset, s.boarding_ends, s.departs, s.disembark),
            (1152, 920, 233, 272, 26)
        );
        let cycle = |into: f64| f64::from(s.offset) + into;
        assert_eq!(voyage_progress(228, cycle(233.0), None).0, 0.0);
        assert_eq!(voyage_progress(228, cycle(271.0), None).0, 0.0);
        assert_eq!(voyage_progress(228, cycle(272.0), None).0, 0.0);
        assert_eq!(voyage_progress(228, cycle(1152.0 + 26.0), None).0, 1.0);
        let halfway = voyage_progress(228, cycle(272.0 + 453.0), None).0;
        assert!((halfway - 0.5).abs() < 0.0001);
        assert!(
            (voyage_progress(228, cycle(1152.0 + 272.0 + 453.0), None).0 - halfway).abs() < 0.0001
        );
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
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        const FERRY_ZONE_FILE: u32 = 328;
        let (submeshes, instances) =
            crate::dat_mzb::load_mzb_placed(&root, FERRY_ZONE_FILE, None).unwrap();
        let geometry =
            crate::dat_mzb::build_collision_geometry(&submeshes, &instances, Some(FERRY_ZONE_FILE));
        let mut collision = crate::dat_mzb::MzbCollisionGeometry::default();
        collision.set_block(0, geometry);
        let floor = collision
            .ground_nearest(Vec2::new(0.15, -3.25), 2.1)
            .expect("ship deck at LSB zone-in position");
        assert!((floor - 2.1).abs() < 0.1, "floor={floor}");
        let build =
            crate::dat_mzb::build_zone_mmb_spawns(&root, FERRY_ZONE_FILE, None, None).unwrap();
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
