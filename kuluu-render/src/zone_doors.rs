#![cfg(not(target_arch = "wasm32"))]

//! Door swing. A door's FourCC is at once the `BlockID` of its `_`/`@` MZB
//! placement group, the name of the zone-DAT directory holding its `open`/`clos`
//! routines, and the key those two are joined on; the server sends only which
//! state it is in, on the entity animation byte (`enum ANIMATIONTYPE`,
//! vendor/server/src/map/entities/baseentity.h).

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task};

use ffxi_dat::chunk::{walk_tree, ChunkNode};
use ffxi_dat::kind::ChunkKind;
use ffxi_dat::mzb::{self, MmbPlacement};
use ffxi_dat::scheduler::{Scheduler, StageKind, MODEL_TRANSFORM_SUBCHUNK_SLOTS};
use ffxi_dat::sep::Sep;
use ffxi_dat::DatRoot;
use ffxi_proto::decode::{animation, DoorId};
use kuluu_snapshot::EntityLook;

use crate::dat_mzb::placement_bevy_transform;
use crate::scene::TrackedEntities;
use crate::scheduler_runtime::{ActionAssets, ActiveScheduler, SchedulerStageEvent, ROUTINE_FPS};
use crate::snapshot::{effective_zone_file_id, SceneState};

// The two door states LSB broadcasts (`ANIMATION_OPEN_DOOR` = 8 /
// `ANIMATION_CLOSE_DOOR` = 9, set e.g. in vendor/server/src/map/transport.cpp)
// each name the zone-DAT Scheduler the client runs for it — Southern San d'Oria's
// Chocobo Stables door is `/t_sa/door/_6ey/{open,clos}`.
const ROUTINE_OPEN: [u8; 4] = *b"open";
const ROUTINE_CLOSE: [u8; 4] = *b"clos";

// The same two poses as a routine to arrive on rather than swing through. Which
// is what retail uses them for is our inference, not a citation: a door already
// open when it comes into view has nothing to animate from. Most are brief
// rather than instant — 5346 of 7524 transform stages corpus-wide run 2 frames,
// 2032 run 0 — so they are played, not assigned.
const ROUTINE_OPEN_ON_ARRIVAL: [u8; 4] = *b"into";
const ROUTINE_CLOSE_ON_ARRIVAL: [u8; 4] = *b"intc";

/// A leaf's routine-driven displacement from its authored MZB placement pose —
/// what a 0x0C/0x0D stage's `final_value` targets. Zero is the authored pose,
/// which is why retail keeps a per-slot copy of the placement's TRS to rebuild
/// from (research/XIClient `UnderscoreAtStruct::InitMatrix`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DoorPose {
    /// Radians per FFXI axis, added to the placement's authored Euler triple.
    pub rotation: Vec3,
    /// Yalms per FFXI axis, added to the placement's authored translation.
    pub translation: Vec3,
}

/// One drawn placement of a `_`/`@` FourCC group, tagged with the slot a
/// Scheduler stage addresses it by.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct ZoneDoorLeaf {
    /// The group's `BlockID`, little-endian — the same four bytes as the door
    /// entity's [`DoorId`] and the zone-DAT directory name.
    pub four_cc: u32,

    /// `UnderscoreAtStruct::Subchunks` slot, which is what a 0x0C/0x0D stage's
    /// index operand selects.
    pub subchunk: u32,

    base_scale: Vec3,
    base_rot: Vec3,
    base_trans: Vec3,
    world_offset: Vec3,
}

pub type DoorLeafKey = (u32, u32);

impl ZoneDoorLeaf {
    pub fn new(subchunk: u32, placement: &MmbPlacement) -> Self {
        Self {
            four_cc: placement.block_id,
            subchunk,
            base_scale: Vec3::from_array(placement.scale),
            base_rot: Vec3::from_array(placement.rot),
            base_trans: Vec3::from_array(placement.trans),
            world_offset: Vec3::ZERO,
        }
    }

    /// The offset `LoadMzbRequest` spawns this zone's geometry at, so a re-posed
    /// leaf lands where the un-posed one did.
    pub fn with_world_offset(self, world_offset: Vec3) -> Self {
        Self {
            world_offset,
            ..self
        }
    }

    pub fn key(&self) -> DoorLeafKey {
        (self.four_cc, self.subchunk)
    }

    /// The leaf's world matrix under `pose`. [`DoorPose::default`] reproduces the
    /// matrix the placement spawned with, bit for bit.
    pub fn posed_transform(&self, pose: DoorPose) -> Mat4 {
        Mat4::from_translation(self.world_offset)
            * placement_bevy_transform(
                self.base_scale,
                self.base_rot + pose.rotation,
                self.base_trans + pose.translation,
            )
    }
}

/// Placement index -> subchunk slot, for every member of a `_`/`@` group.
pub fn leaf_slots(placements: &[MmbPlacement]) -> HashMap<usize, u32> {
    let mut out = HashMap::new();
    for group in mzb::underscore_at_groups(placements) {
        for (slot, placement_idx) in group.subchunks.iter().enumerate() {
            out.insert(*placement_idx, slot as u32);
        }
    }
    out
}

/// A door NPC entity, tagged once its FourCC has been matched to a routine set.
#[derive(Component, Debug, Clone, Copy)]
pub struct ZoneDoorNpc {
    pub four_cc: u32,

    /// Last animation byte acted on. The server repeats the byte on every 0x0E,
    /// so only a change is an event.
    pub animation: u8,
}

/// One zone-DAT door directory: every Scheduler it holds (so a routine's 0x03
/// sub-routine calls resolve against its siblings) and its Sep sounds.
#[derive(Debug, Default, Clone)]
pub struct DoorDir {
    pub routines: Vec<Scheduler>,
    pub seps: HashMap<[u8; 4], Sep>,
}

impl DoorDir {
    fn has(&self, name: &[u8; 4]) -> bool {
        self.routines.iter().any(|s| &s.name == name)
    }

    /// The routine for `animation`, and whether it is the on-arrival twin.
    fn routine_for(&self, animation: u8, on_arrival: bool) -> Option<[u8; 4]> {
        let (arrival, animated) = match animation {
            animation::OPEN_DOOR => (ROUTINE_OPEN_ON_ARRIVAL, ROUTINE_OPEN),
            animation::CLOSE_DOOR => (ROUTINE_CLOSE_ON_ARRIVAL, ROUTINE_CLOSE),
            _ => return None,
        };
        if on_arrival && self.has(&arrival) {
            return Some(arrival);
        }
        self.has(&animated).then_some(animated)
    }
}

#[derive(Debug, Clone, Copy)]
struct LeafTween {
    from: Vec3,
    to: Vec3,
    elapsed_frames: f32,
    duration_frames: f32,
}

impl LeafTween {
    fn value(&self) -> Vec3 {
        if self.duration_frames <= 0.0 {
            return self.to;
        }
        self.from.lerp(
            self.to,
            (self.elapsed_frames / self.duration_frames).clamp(0.0, 1.0),
        )
    }

    fn finished(&self) -> bool {
        self.elapsed_frames >= self.duration_frames
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct LeafMotion {
    pose: DoorPose,
    rotation: Option<LeafTween>,
    translation: Option<LeafTween>,
}

impl LeafMotion {
    fn animating(&self) -> bool {
        self.rotation.is_some() || self.translation.is_some()
    }

    /// Begin a stage: it interpolates from wherever the leaf is *now* to
    /// `target` over `duration_frames`, which is how a routine chains two stages
    /// on one slot (Mea's `_pmd` lift creeps 1 yalm, then runs the last 9).
    fn start(&mut self, kind: StageKind, target: Vec3, duration_frames: f32) {
        let tween = LeafTween {
            from: self.axis(kind),
            to: target,
            elapsed_frames: 0.0,
            duration_frames,
        };
        match kind {
            StageKind::ModelRotation => self.rotation = Some(tween),
            StageKind::ModelTranslation => self.translation = Some(tween),
            _ => return,
        }
        self.advance(0.0);
    }

    fn snap(&mut self, kind: StageKind, target: Vec3) {
        self.start(kind, target, 0.0);
    }

    fn axis(&self, kind: StageKind) -> Vec3 {
        match kind {
            StageKind::ModelRotation => self.pose.rotation,
            _ => self.pose.translation,
        }
    }

    fn advance(&mut self, frames: f32) {
        if let Some(t) = &mut self.rotation {
            t.elapsed_frames += frames;
            self.pose.rotation = t.value();
            if t.finished() {
                self.rotation = None;
            }
        }
        if let Some(t) = &mut self.translation {
            t.elapsed_frames += frames;
            self.pose.translation = t.value();
            if t.finished() {
                self.translation = None;
            }
        }
    }
}

#[derive(Resource, Default)]
pub struct ZoneDoors {
    /// DAT file the routines came from; a zone warp keeps `AppPhase::InGame`, so
    /// this — not an `OnExit` — is what scopes the state to one zone (same
    /// pattern as `MzbCollisionGeometry::source_file_id`).
    source_file_id: Option<u32>,
    dirs: HashMap<u32, DoorDir>,
    leaves: HashMap<DoorLeafKey, LeafMotion>,
    load: Option<Task<HashMap<u32, DoorDir>>>,
}

impl ZoneDoors {
    pub fn pose(&self, key: DoorLeafKey) -> DoorPose {
        self.leaves.get(&key).map(|m| m.pose).unwrap_or_default()
    }

    pub fn dir(&self, four_cc: u32) -> Option<&DoorDir> {
        self.dirs.get(&four_cc)
    }

    /// Return one group's leaves to their authored placement pose.
    fn rest_group(&mut self, four_cc: u32) {
        self.leaves.retain(|(group, _), _| *group != four_cc);
    }

    fn clear_zone_state(&mut self) {
        self.dirs.clear();
        self.leaves.clear();
        self.load = None;
    }
}

/// The FourCC as the four characters that name its DAT directory, for logs.
fn door_label(four_cc: u32) -> String {
    DoorId::new(four_cc.to_le_bytes()).map_or_else(|| four_cc.to_string(), |id| id.to_string())
}

fn door_four_cc(look: Option<&EntityLook>) -> Option<u32> {
    match look {
        Some(EntityLook::Door { door_id, .. }) => DoorId::new((*door_id)?).map(DoorId::block_id),
        _ => None,
    }
}

/// Every door directory in a zone DAT, keyed by its FourCC read little-endian —
/// the byte order `MmbPlacement::block_id` and [`DoorId::block_id`] both use.
///
/// A directory qualifies by holding a routine one of the two animation bytes can
/// name; nothing else in a zone DAT does.
pub fn door_dirs(bytes: &[u8]) -> HashMap<u32, DoorDir> {
    fn walk(node: &ChunkNode<'_>, out: &mut HashMap<u32, DoorDir>) {
        for child in &node.children {
            if child.children.is_empty() {
                continue;
            }
            let mut dir = DoorDir::default();
            for entry in &child.children {
                let c = &entry.chunk;
                match ChunkKind::from_u8(c.kind) {
                    Some(ChunkKind::Scheduler) => {
                        if let Ok(s) = Scheduler::parse_in_dir(child.chunk.name, c.name, c.data) {
                            dir.routines.push(s);
                        }
                    }
                    Some(ChunkKind::Sep) => {
                        if let Ok(s) = Sep::parse(c.name, c.data) {
                            dir.seps.insert(c.name, s);
                        }
                    }
                    _ => {}
                }
            }
            if dir.has(&ROUTINE_OPEN) || dir.has(&ROUTINE_CLOSE) {
                out.insert(u32::from_le_bytes(child.chunk.name), dir);
            }
            walk(child, out);
        }
    }
    let mut out = HashMap::new();
    walk(&walk_tree(bytes), &mut out);
    out
}

fn load_door_dirs(file_id: u32) -> HashMap<u32, DoorDir> {
    let bytes = DatRoot::from_env_or_default()
        .ok()
        .and_then(|root| {
            let loc = root.resolve(file_id).ok()?;
            std::fs::read(loc.path_under(&root)).ok()
        })
        .unwrap_or_default();
    door_dirs(&bytes)
}

pub fn sync_zone_door_dirs(scene_state: Res<SceneState>, mut doors: ResMut<ZoneDoors>) {
    let current = effective_zone_file_id(&scene_state.snapshot);
    if current != doors.source_file_id {
        doors.source_file_id = current;
        doors.clear_zone_state();
        if let Some(file_id) = current {
            doors.load =
                Some(AsyncComputeTaskPool::get().spawn(async move { load_door_dirs(file_id) }));
        }
    }

    let Some(task) = &mut doors.load else { return };
    let Some(dirs) = future::block_on(future::poll_once(task)) else {
        return;
    };
    info!(
        "zone_doors: DAT {:?} → {} animated door group(s)",
        doors.source_file_id,
        dirs.len()
    );
    doors.dirs = dirs;
    doors.load = None;
}

/// Starts a door's routine when the server's animation byte for it changes.
///
/// The first sighting is not a change: the byte is the door's *state*, so a door
/// that is already open when it comes into view takes the on-arrival pose
/// directly rather than swinging (and without its 0x0B sound).
pub fn trigger_zone_doors(
    scene_state: Res<SceneState>,
    tracked: Res<TrackedEntities>,
    mut doors: ResMut<ZoneDoors>,
    mut q_npc: Query<&mut ZoneDoorNpc>,
    mut commands: Commands,
) {
    if doors.dirs.is_empty() {
        return;
    }
    for wire in &scene_state.snapshot.entities {
        let Some(four_cc) = door_four_cc(wire.look.as_ref()) else {
            continue;
        };
        let Some(&entity) = tracked.by_id.get(&wire.id) else {
            continue;
        };
        let on_arrival = match q_npc.get_mut(entity) {
            Ok(mut npc) => {
                if npc.animation == wire.animation {
                    continue;
                }
                npc.animation = wire.animation;
                false
            }
            Err(_) => {
                commands.entity(entity).try_insert(ZoneDoorNpc {
                    four_cc,
                    animation: wire.animation,
                });
                true
            }
        };

        if on_arrival {
            // Retail rebuilds `UnderscoreAtStructs` from the placement table when
            // the zone loads, so a group the server does not report as open is at
            // its authored pose — including after a relog into a zone this
            // resource still holds swung leaves for.
            doors.rest_group(four_cc);
        }
        let Some(dir) = doors.dirs.get(&four_cc) else {
            continue;
        };
        let Some(routine) = dir.routine_for(wire.animation, on_arrival) else {
            continue;
        };
        let label = door_label(four_cc);
        if on_arrival {
            let offsets = final_offsets(dir, &routine);
            let applied = offsets.len();
            for (subchunk, kind, value) in offsets {
                doors
                    .leaves
                    .entry((four_cc, subchunk))
                    .or_default()
                    .snap(kind, value);
            }
            debug!(
                "zone_doors: {label} arrives at {} — snapped {applied} slot(s)",
                String::from_utf8_lossy(&routine)
            );
            continue;
        }
        let Some(active) = ActiveScheduler::from_main(&dir.routines, &routine) else {
            continue;
        };
        commands
            .entity(entity)
            .try_insert(active)
            .try_insert(ActionAssets {
                seps: dir.seps.clone(),
                ..Default::default()
            });
        info!(
            "zone_doors: {label} runs {}",
            String::from_utf8_lossy(&routine)
        );
    }
}

/// The pose a routine ends at, per addressed slot — what an on-arrival door
/// takes directly. Later stages on the same slot supersede earlier ones.
fn final_offsets(dir: &DoorDir, routine: &[u8; 4]) -> Vec<(u32, StageKind, Vec3)> {
    let Some(active) = ActiveScheduler::from_main(&dir.routines, routine) else {
        return Vec::new();
    };
    let mut out: Vec<(u32, StageKind, Vec3)> = Vec::new();
    for timed in &active.stages {
        let Some(m) = timed.stage.model_transform else {
            continue;
        };
        if m.subchunk >= MODEL_TRANSFORM_SUBCHUNK_SLOTS {
            continue;
        }
        let entry = (
            m.subchunk,
            timed.stage.kind,
            Vec3::from_array(m.final_value),
        );
        match out
            .iter_mut()
            .find(|(s, k, _)| *s == entry.0 && *k == entry.1)
        {
            Some(existing) => existing.2 = entry.2,
            None => out.push(entry),
        }
    }
    out
}

pub fn apply_zone_door_stages(
    mut events: MessageReader<SchedulerStageEvent>,
    q_npc: Query<&ZoneDoorNpc>,
    mut doors: ResMut<ZoneDoors>,
) {
    for ev in events.read() {
        let Some(m) = ev.stage.stage.model_transform else {
            continue;
        };
        // Retail stores four subchunks per group and draws only those, so a stage
        // addressing a fifth reaches nothing.
        if m.subchunk >= MODEL_TRANSFORM_SUBCHUNK_SLOTS {
            continue;
        }
        let Ok(npc) = q_npc.get(ev.actor) else {
            continue;
        };
        doors
            .leaves
            .entry((npc.four_cc, m.subchunk))
            .or_default()
            .start(
                ev.stage.stage.kind,
                Vec3::from_array(m.final_value),
                ev.stage.stage.duration_frames as f32,
            );
    }
}

/// Advances the live tweens on the routine clock, then poses every leaf that is
/// currently spawned.
///
/// The pose lives in the resource rather than on the leaf entity because MMB
/// placements stream by distance: a door opened out of streaming range has no
/// leaf entity to hold state, and must still spawn open when the player walks up.
pub fn animate_zone_door_leaves(
    time: Res<Time>,
    mut doors: ResMut<ZoneDoors>,
    mut q: Query<(&ZoneDoorLeaf, &mut Transform)>,
) {
    if doors.leaves.values().any(LeafMotion::animating) {
        let frames = time.delta_secs() * ROUTINE_FPS;
        for motion in doors.leaves.values_mut() {
            motion.advance(frames);
        }
    }
    for (leaf, mut transform) in &mut q {
        let posed = Transform::from_matrix(leaf.posed_transform(doors.pose(leaf.key())));
        if *transform != posed {
            *transform = posed;
        }
    }
}

pub struct ZoneDoorsPlugin;

impl Plugin for ZoneDoorsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneDoors>().add_systems(
            Update,
            (
                sync_zone_door_dirs,
                trigger_zone_doors.before(crate::scheduler_runtime::tick_active_schedulers),
                apply_zone_door_stages.after(crate::scheduler_runtime::tick_active_schedulers),
                animate_zone_door_leaves,
            )
                .chain(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::scheduler::{ModelTransform, SchedulerStage, TimedStage};

    const SSANDY_ZONE_DAT: u32 = 330;
    const SSANDY_STABLES_DOOR: [u8; 4] = *b"_6ey";
    // `/t_sa/door/_6ey/open` stores 1.3962256 rad; the exact 80 deg would be
    // 1.3962634. Nothing downstream may hardcode the angle — this bound only
    // pins that the DAT still says "about 80 degrees".
    const SSANDY_STABLES_SWING_DEG: f32 = 80.0;
    const SWING_DEG_TOLERANCE: f32 = 0.01;

    fn placement(block_id: &[u8; 4], rot_y: f32, scale_z: f32) -> MmbPlacement {
        MmbPlacement {
            id: [0; 16],
            trans: [0.0; 3],
            rot: [0.0, rot_y, 0.0],
            scale: [1.0, 1.0, scale_z],
            block_id: u32::from_le_bytes(*block_id),
            lod_near: 0.0,
            lod_mid: 0.0,
            lod_far: 0.0,
            special_effects: 0,
            area_resource_id: 0,
            sub_area_link: 0,
            light_references: [0; mzb::LIGHT_REFERENCE_COUNT],
        }
    }

    fn rotation_stage(subchunk: u32, y: f32, delay: u16, duration: u16) -> TimedStage {
        TimedStage {
            frame: 0,
            stage: SchedulerStage {
                kind: StageKind::ModelRotation,
                raw_type: 0x0D,
                delay_frames: delay,
                duration_frames: duration,
                id: [0; 4],
                max_loops: 0,
                transition_in: 0,
                transition_out: 0,
                model_transform: Some(ModelTransform {
                    final_value: [0.0, y, 0.0],
                    subchunk,
                }),
                screen_color: None,
                random_group: None,
                local_dir: ffxi_dat::scheduler::NO_LOCAL_DIR,
            },
        }
    }

    fn door_entity(id: u32, animation: u8) -> kuluu_snapshot::Entity {
        kuluu_snapshot::Entity {
            id,
            act_index: 7,
            kind: kuluu_snapshot::EntityKind::Other,
            name: None,
            pos: kuluu_snapshot::Vec3::default(),
            heading: 0,
            hp_pct: None,
            bt_target_id: 0,
            face_target: 0,
            name_vis: None,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: Some(EntityLook::Door {
                size: 2,
                door_id: Some(SSANDY_STABLES_DOOR),
            }),
            animation,
            animationsub: 0,
            mount: None,
            status: 0,
            char_flags: kuluu_snapshot::CharFlags::default(),
        }
    }

    // A door's own `open`, in the shape the DAT ships it: one stage per leaf,
    // the second delayed behind the first, both swinging over the same 70 frames.
    fn swing_dir(swing: f32) -> DoorDir {
        DoorDir {
            routines: vec![
                Scheduler {
                    name: ROUTINE_OPEN,
                    stages: vec![
                        rotation_stage(0, swing, 0, SWING_FRAMES),
                        rotation_stage(1, swing, SWING_FRAMES, SWING_FRAMES),
                    ],
                },
                Scheduler {
                    name: ROUTINE_CLOSE,
                    stages: vec![
                        rotation_stage(0, 0.0, 0, SWING_FRAMES),
                        rotation_stage(1, 0.0, SWING_FRAMES, SWING_FRAMES),
                    ],
                },
            ],
            seps: HashMap::new(),
        }
    }

    const SWING_FRAMES: u16 = 70;
    const DOOR_ENTITY_ID: u32 = 0x1701234;

    fn step(app: &mut App, frames: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(frames / ROUTINE_FPS));
        app.update();
    }

    fn see(app: &mut App, animation: u8) {
        app.world_mut()
            .resource_mut::<SceneState>()
            .snapshot
            .entities = vec![door_entity(DOOR_ENTITY_ID, animation)];
        step(app, 0.0);
    }

    /// How far the leaf has swung, as the distance its local +X corner has
    /// travelled from the shut pose. Measured on the matrix rather than on a
    /// decomposed yaw because the FFXI->Bevy flip is baked into it.
    fn swept(app: &App, leaf: Entity, rest: Mat4) -> f32 {
        let m = app
            .world()
            .entity(leaf)
            .get::<Transform>()
            .unwrap()
            .to_matrix();
        m.transform_point3(Vec3::X)
            .distance(rest.transform_point3(Vec3::X))
    }

    /// The whole join, driven through the registered systems: an animation-byte
    /// change on a door NPC has to reach the Transform of the MMB placements its
    /// FourCC names.
    #[test]
    fn an_animation_byte_change_swings_the_tagged_leaves() {
        let swing = SSANDY_STABLES_SWING_DEG.to_radians();
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<SceneState>()
            .init_resource::<TrackedEntities>()
            .init_resource::<ZoneDoors>()
            .add_message::<SchedulerStageEvent>()
            .add_systems(
                Update,
                (
                    trigger_zone_doors,
                    crate::scheduler_runtime::tick_active_schedulers,
                    apply_zone_door_stages,
                    animate_zone_door_leaves,
                )
                    .chain(),
            );
        app.world_mut()
            .resource_mut::<ZoneDoors>()
            .dirs
            .insert(u32::from_le_bytes(SSANDY_STABLES_DOOR), swing_dir(swing));

        let npc = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<TrackedEntities>()
            .by_id
            .insert(DOOR_ENTITY_ID, npc);
        let shut: Vec<Mat4> = (0..2)
            .map(|slot| {
                ZoneDoorLeaf::new(slot, &placement(&SSANDY_STABLES_DOOR, 0.0, 1.0))
                    .posed_transform(DoorPose::default())
            })
            .collect();
        let open_swing = ZoneDoorLeaf::new(0, &placement(&SSANDY_STABLES_DOOR, 0.0, 1.0))
            .posed_transform(DoorPose {
                rotation: Vec3::new(0.0, swing, 0.0),
                ..Default::default()
            })
            .transform_point3(Vec3::X)
            .distance(shut[0].transform_point3(Vec3::X));
        let leaves: Vec<Entity> = (0..2)
            .map(|slot| {
                let leaf = ZoneDoorLeaf::new(slot, &placement(&SSANDY_STABLES_DOOR, 0.0, 1.0));
                app.world_mut()
                    .spawn((leaf, Transform::from_matrix(shut[slot as usize])))
                    .id()
            })
            .collect();

        see(&mut app, animation::CLOSE_DOOR);
        assert_eq!(swept(&app, leaves[0], shut[0]), 0.0, "arrives shut");

        // The routine is inserted through Commands and its frame-0 stages are read
        // back by the two consumers after it, so the swing is live on the frame
        // the byte changed.
        see(&mut app, animation::OPEN_DOOR);
        step(&mut app, SWING_FRAMES as f32 / 2.0);
        for (slot, leaf) in leaves.iter().enumerate() {
            let half = swept(&app, *leaf, shut[slot]);
            assert!(
                half > 0.0 && half < open_swing,
                "leaf {slot} is mid-swing half way through the stage, got {half} of {open_swing}"
            );
        }

        step(&mut app, SWING_FRAMES as f32 / 2.0);
        for (slot, leaf) in leaves.iter().enumerate() {
            assert!(
                (swept(&app, *leaf, shut[slot]) - open_swing).abs() < 1e-4,
                "both leaves end fully open — leaf 1's delay gates the stages after \
                 it, not itself"
            );
        }

        see(&mut app, animation::CLOSE_DOOR);
        step(&mut app, SWING_FRAMES as f32);
        for (slot, leaf) in leaves.iter().enumerate() {
            assert!(
                swept(&app, *leaf, shut[slot]) < 1e-4,
                "closing returns leaf {slot} to the authored placement pose"
            );
        }
    }

    #[test]
    fn wire_door_id_and_placement_block_id_are_the_same_key() {
        let leaf = ZoneDoorLeaf::new(0, &placement(&SSANDY_STABLES_DOOR, 0.0, 1.0));
        let from_wire = DoorId::new(SSANDY_STABLES_DOOR).expect("printable FourCC");
        assert_eq!(leaf.four_cc, from_wire.block_id());
        assert_eq!(leaf.four_cc.to_le_bytes(), SSANDY_STABLES_DOOR);
    }

    #[test]
    fn leaf_slots_number_group_members_in_placement_order() {
        let placements = [
            placement(b"door", 0.0, 1.0),
            placement(&SSANDY_STABLES_DOOR, 0.0, 1.0),
            placement(b"_zzz", 0.0, 1.0),
            placement(&SSANDY_STABLES_DOOR, 0.0, -1.0),
        ];
        let slots = leaf_slots(&placements);
        assert_eq!(slots.get(&1), Some(&0));
        assert_eq!(slots.get(&3), Some(&1));
        assert_eq!(slots.get(&2), Some(&0), "each FourCC numbers from 0");
        assert_eq!(slots.get(&0), None, "no leading _/@ is not a group");
    }

    #[test]
    fn rest_pose_reproduces_the_placement_transform() {
        let p = placement(&SSANDY_STABLES_DOOR, std::f32::consts::FRAC_PI_2, -1.0);
        let leaf = ZoneDoorLeaf::new(1, &p);
        assert_eq!(
            leaf.posed_transform(DoorPose::default()),
            placement_bevy_transform(
                Vec3::from_array(p.scale),
                Vec3::from_array(p.rot),
                Vec3::from_array(p.trans),
            ),
            "a closed door must render exactly where the un-posed placement did"
        );
    }

    #[test]
    fn pose_offsets_the_authored_rotation_rather_than_replacing_it() {
        let authored = std::f32::consts::FRAC_PI_2;
        let swing = SSANDY_STABLES_SWING_DEG.to_radians();
        let leaf = ZoneDoorLeaf::new(0, &placement(&SSANDY_STABLES_DOOR, authored, 1.0));
        let open = leaf.posed_transform(DoorPose {
            rotation: Vec3::new(0.0, swing, 0.0),
            ..Default::default()
        });
        let expected =
            placement_bevy_transform(Vec3::ONE, Vec3::new(0.0, authored + swing, 0.0), Vec3::ZERO);
        assert!(
            (open - expected)
                .to_cols_array()
                .iter()
                .all(|v| v.abs() < 1e-5),
            "open pose is authored + swing, not swing alone"
        );
    }

    #[test]
    fn mirrored_leaf_keeps_the_stage_sign() {
        let swing = SSANDY_STABLES_SWING_DEG.to_radians();
        let pose = DoorPose {
            rotation: Vec3::new(0.0, swing, 0.0),
            ..Default::default()
        };
        // The second Southern San d'Oria leaf is the first mirrored through
        // scale.z; one positive stage value swings both outward, so the renderer
        // must not negate it per leaf.
        let mirrored = ZoneDoorLeaf::new(1, &placement(&SSANDY_STABLES_DOOR, 0.0, -1.0));
        let plain = ZoneDoorLeaf::new(0, &placement(&SSANDY_STABLES_DOOR, 0.0, 1.0));
        let mirrored_yaw = mirrored.posed_transform(pose) * Vec3::X.extend(0.0);
        let plain_yaw = plain.posed_transform(pose) * Vec3::X.extend(0.0);
        assert!(
            (mirrored_yaw - plain_yaw).length() < 1e-5,
            "the mirror lives in the placement's scale, not in the stage value"
        );
    }

    #[test]
    fn rotation_interpolates_over_the_stage_duration() {
        const DURATION: f32 = 70.0;
        let swing = SSANDY_STABLES_SWING_DEG.to_radians();
        let mut motion = LeafMotion::default();
        motion.start(
            StageKind::ModelRotation,
            Vec3::new(0.0, swing, 0.0),
            DURATION,
        );
        assert_eq!(motion.pose.rotation, Vec3::ZERO, "frame 0 is the rest pose");

        motion.advance(DURATION / 2.0);
        assert!((motion.pose.rotation.y - swing / 2.0).abs() < 1e-5);
        assert!(motion.animating());

        motion.advance(DURATION / 2.0);
        assert!((motion.pose.rotation.y - swing).abs() < 1e-5);
        assert!(!motion.animating(), "the tween ends with the stage");

        motion.advance(DURATION);
        assert!(
            (motion.pose.rotation.y - swing).abs() < 1e-5,
            "an ended stage holds its final value"
        );
    }

    #[test]
    fn a_stage_resumes_from_wherever_the_leaf_is() {
        const DURATION: f32 = 70.0;
        let swing = SSANDY_STABLES_SWING_DEG.to_radians();
        let mut motion = LeafMotion::default();
        motion.start(
            StageKind::ModelRotation,
            Vec3::new(0.0, swing, 0.0),
            DURATION,
        );
        motion.advance(DURATION / 2.0);

        motion.start(StageKind::ModelRotation, Vec3::ZERO, DURATION);
        assert!((motion.pose.rotation.y - swing / 2.0).abs() < 1e-5);
        motion.advance(DURATION / 2.0);
        assert!((motion.pose.rotation.y - swing / 4.0).abs() < 1e-5);
    }

    #[test]
    fn a_zero_duration_stage_snaps() {
        let mut motion = LeafMotion::default();
        motion.start(StageKind::ModelTranslation, Vec3::new(0.0, 10.0, 0.0), 0.0);
        assert_eq!(motion.pose.translation, Vec3::new(0.0, 10.0, 0.0));
        assert!(!motion.animating());
    }

    #[test]
    fn rotation_and_translation_stages_do_not_share_a_track() {
        let mut motion = LeafMotion::default();
        motion.snap(StageKind::ModelRotation, Vec3::new(0.0, 1.0, 0.0));
        motion.snap(StageKind::ModelTranslation, Vec3::new(0.0, 3.0, 0.0));
        assert_eq!(motion.pose.rotation, Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(motion.pose.translation, Vec3::new(0.0, 3.0, 0.0));
    }

    #[test]
    fn later_stages_win_the_on_arrival_pose() {
        let dir = DoorDir {
            routines: vec![Scheduler {
                name: ROUTINE_OPEN,
                stages: vec![
                    rotation_stage(0, 0.5, 0, 60),
                    rotation_stage(0, 1.25, 0, 60),
                    rotation_stage(1, 0.75, 0, 60),
                ],
            }],
            seps: HashMap::new(),
        };
        let finals = final_offsets(&dir, &ROUTINE_OPEN);
        assert_eq!(finals.len(), 2);
        assert_eq!(
            finals[0],
            (0, StageKind::ModelRotation, Vec3::new(0.0, 1.25, 0.0))
        );
        assert_eq!(
            finals[1],
            (1, StageKind::ModelRotation, Vec3::new(0.0, 0.75, 0.0))
        );
    }

    #[test]
    fn a_stage_past_the_fourth_slot_addresses_nothing() {
        let dir = DoorDir {
            routines: vec![Scheduler {
                name: ROUTINE_OPEN,
                stages: vec![rotation_stage(MODEL_TRANSFORM_SUBCHUNK_SLOTS, 1.0, 0, 60)],
            }],
            seps: HashMap::new(),
        };
        assert!(final_offsets(&dir, &ROUTINE_OPEN).is_empty());
    }

    #[test]
    fn the_animation_byte_picks_the_routine() {
        let animated = DoorDir {
            routines: vec![
                Scheduler {
                    name: ROUTINE_OPEN,
                    stages: Vec::new(),
                },
                Scheduler {
                    name: ROUTINE_CLOSE,
                    stages: Vec::new(),
                },
            ],
            seps: HashMap::new(),
        };
        assert_eq!(
            animated.routine_for(animation::OPEN_DOOR, false),
            Some(ROUTINE_OPEN)
        );
        assert_eq!(
            animated.routine_for(animation::CLOSE_DOOR, false),
            Some(ROUTINE_CLOSE)
        );
        assert_eq!(
            animated.routine_for(animation::OPEN_DOOR, true),
            Some(ROUTINE_OPEN),
            "no on-arrival twin falls back to the animated routine"
        );
        assert_eq!(animated.routine_for(animation::NONE, false), None);
        assert_eq!(animated.routine_for(animation::ATTACK, true), None);

        let mut with_twins = animated.clone();
        with_twins.routines.push(Scheduler {
            name: ROUTINE_OPEN_ON_ARRIVAL,
            stages: Vec::new(),
        });
        assert_eq!(
            with_twins.routine_for(animation::OPEN_DOOR, true),
            Some(ROUTINE_OPEN_ON_ARRIVAL)
        );
        assert_eq!(
            with_twins.routine_for(animation::OPEN_DOOR, false),
            Some(ROUTINE_OPEN),
            "a state change still swings"
        );
    }

    #[test]
    fn resting_a_group_leaves_the_other_doors_alone() {
        let stables = u32::from_le_bytes(SSANDY_STABLES_DOOR);
        let other = u32::from_le_bytes(*b"_6e1");
        let mut doors = ZoneDoors::default();
        for key in [(stables, 0), (stables, 1), (other, 0)] {
            doors
                .leaves
                .entry(key)
                .or_default()
                .snap(StageKind::ModelRotation, Vec3::new(0.0, 1.4, 0.0));
        }
        doors.rest_group(stables);
        assert_eq!(doors.pose((stables, 0)), DoorPose::default());
        assert_eq!(doors.pose((stables, 1)), DoorPose::default());
        assert_eq!(doors.pose((other, 0)).rotation.y, 1.4);
    }

    #[test]
    fn a_door_look_without_a_four_cc_joins_nothing() {
        assert_eq!(
            door_four_cc(Some(&EntityLook::Door {
                size: 2,
                door_id: Some(SSANDY_STABLES_DOOR),
            })),
            Some(u32::from_le_bytes(SSANDY_STABLES_DOOR))
        );
        assert_eq!(
            door_four_cc(Some(&EntityLook::Door {
                size: 2,
                door_id: None,
            })),
            None
        );
        assert_eq!(door_four_cc(Some(&EntityLook::Transport { size: 3 })), None);
    }

    #[test]
    fn real_dat_southern_sandoria_stables_door_swings_both_leaves() {
        let Some(bytes) = crate::weather_particles::tests::zone_dat(SSANDY_ZONE_DAT) else {
            return;
        };
        let dirs = door_dirs(&bytes);
        let dir = dirs
            .get(&u32::from_le_bytes(SSANDY_STABLES_DOOR))
            .expect("_6ey is an animated door directory");

        let open = final_offsets(dir, &ROUTINE_OPEN);
        assert_eq!(open.len(), 2, "two leaves are addressed");
        for (slot, (subchunk, kind, value)) in open.iter().enumerate() {
            assert_eq!(*subchunk, slot as u32);
            assert_eq!(*kind, StageKind::ModelRotation);
            assert!(
                (value.y.to_degrees().abs() - SSANDY_STABLES_SWING_DEG).abs() < SWING_DEG_TOLERANCE,
                "leaf {slot} swings about Y by ~{SSANDY_STABLES_SWING_DEG} deg, got {value:?}"
            );
            assert_eq!(value.x, 0.0);
            assert_eq!(value.z, 0.0);
        }

        for (_, _, value) in final_offsets(dir, &ROUTINE_CLOSE) {
            assert_eq!(
                value,
                Vec3::ZERO,
                "closing returns to the authored placement pose"
            );
        }

        assert_eq!(dir.seps.len(), 2, "the open and close cues live in the dir");
        let sounds: Vec<u32> = {
            let active = ActiveScheduler::from_main(&dir.routines, &ROUTINE_OPEN).unwrap();
            active
                .stages
                .iter()
                .filter(|t| t.stage.kind == StageKind::SoundOnTarget)
                .filter_map(|t| dir.seps.get(&t.stage.id).map(|s| s.se_id))
                .collect()
        };
        assert_eq!(sounds.len(), 1, "opening plays one cue: {sounds:?}");
    }

    /// The tagging site: only `_`/`@` group members may carry a leaf tag, and the
    /// generator water sheets — the other `ZoneMmbSpawn` constructor — may not.
    #[test]
    fn real_dat_zone_build_tags_exactly_the_group_members() {
        if DatRoot::from_env_or_default().is_err() {
            eprintln!("skipping: no FFXI install");
            return;
        }
        AsyncComputeTaskPool::get_or_init(Default::default);
        let build = crate::dat_mzb::build_zone_mmb_spawns(SSANDY_ZONE_DAT, None, None)
            .expect("Southern San d'Oria builds");

        let stables = build
            .spawns
            .iter()
            .filter_map(|s| s.door)
            .filter(|d| d.four_cc == u32::from_le_bytes(SSANDY_STABLES_DOOR))
            .count();
        assert_eq!(stables, 2, "both stables leaves reach the renderer tagged");
        assert!(
            build
                .spawns
                .iter()
                .filter(|s| s.water.is_some())
                .all(|s| s.door.is_none()),
            "a generator water sheet has no placement record to belong to a group"
        );
        assert!(
            build
                .spawns
                .iter()
                .filter_map(|s| s.door)
                .all(|d| d.subchunk < MODEL_TRANSFORM_SUBCHUNK_SLOTS),
            "retail keeps four subchunks per group, so no leaf may be tagged past the fourth"
        );
    }

    #[test]
    fn real_dat_door_leaves_are_tagged_by_their_placement_group() {
        let Some(bytes) = crate::weather_particles::tests::zone_dat(SSANDY_ZONE_DAT) else {
            return;
        };
        let chunks: Vec<_> = ffxi_dat::walk(&bytes).filter_map(Result::ok).collect();
        let mzb_chunk = chunks
            .iter()
            .find(|c| c.kind == ffxi_dat::ChunkKind::Mzb as u8)
            .expect("zone DAT ships an MZB");
        let plain = mzb::decrypt(mzb_chunk.data).expect("decrypt");
        let header = mzb::MzbHeader::parse(&plain).expect("header");
        let placements = mzb::parse_mmb_placements(&plain, &header).expect("placements");

        let slots = leaf_slots(&placements);
        let leaves: Vec<ZoneDoorLeaf> = slots
            .iter()
            .map(|(idx, slot)| ZoneDoorLeaf::new(*slot, &placements[*idx]))
            .filter(|l| l.four_cc == u32::from_le_bytes(SSANDY_STABLES_DOOR))
            .collect();
        assert_eq!(leaves.len(), 2, "the stables door draws two leaves");

        let mut addressed: Vec<u32> = leaves.iter().map(|l| l.subchunk).collect();
        addressed.sort_unstable();
        assert_eq!(
            addressed,
            vec![0, 1],
            "the slots the open routine addresses are the slots the MZB group fills"
        );
        assert!(
            leaves.iter().any(|l| l.base_scale.z < 0.0),
            "the second leaf is the first mirrored"
        );
    }
}
