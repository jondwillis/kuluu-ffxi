#![cfg(not(target_arch = "wasm32"))]

//! Lift platforms. The server only says which leg a lift is on, on the
//! platform entity's animation byte, and when it began, in the 0x0E name block
//! (vendor/server/src/map/transports/elevator_handler.cpp `start`); it never
//! moves the platform or anyone standing on it. The platform's `@` FourCC is at
//! once its MZB placement group, the zone-DAT directory holding its `mv01` /
//! `mv10` routines and the tag of the RID box that states its two floor heights
//! (research/xim/src/jsMain/kotlin/xim/poc/Actor.kt updateElevatorDisplay,
//! ZoneDrawer.kt, Scene.kt checkElevatorInteraction).
//!
//! The platform entity is bound to its shaft by position, not by the FourCC the
//! 0x0E name block carries. A lift NPC stands at the centre of its RID box, and
//! the server's label can name the other shaft: LSB's Metalworks data tags the
//! platform inside the DAT's `@6l1` box `@6l0` and vice versa
//! (vendor/server/data/zones/metalworks/npcs.yaml against zone DAT 337), so a
//! label-bound platform follows the other shaft's leg, which runs in the
//! opposite phase. The label only decides when no box contains the entity.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task};

use ffxi_dat::chunk::{walk_tree, ChunkNode};
use ffxi_dat::generator::Generator;
use ffxi_dat::kind::ChunkKind;
use ffxi_dat::scheduler::{Scheduler, StageKind};
use ffxi_dat::sep::Sep;
use ffxi_dat::zone_interaction::{self, ZoneInteraction};
use ffxi_dat::DatRoot;
use ffxi_proto::decode::animation;
use ffxi_vocab::transport::MODEL_ELEVATOR;
use kuluu_snapshot::EntityLook;

use crate::scene::TrackedEntities;
use crate::scheduler_runtime::{enqueue_routine, ActionAssets, ActiveScheduler, ROUTINE_FPS};
use crate::snapshot::{effective_zone_file_id, SceneState};
use crate::vana_time::{VanaClock, EARTH_EPOCH_UNIX};
use crate::zone_doors::ZoneDoors;

/// `mv<from><to>`: 0 is the RID entry's first floor height (`elevator_bottom_y`),
/// 1 its second. `ELEVATOR_UP` plays `mv01`.
const ROUTINE_UP: [u8; 4] = *b"mv01";
const ROUTINE_DOWN: [u8; 4] = *b"mv10";

/// How far a rider's feet may be from the platform floor and still be carried.
/// The RID box spans the whole shaft, so height is what tells a rider on the
/// platform from someone on the landing above an empty shaft. XIM snaps every
/// actor inside the box; a window around the platform is our inference.
pub const RIDE_CAPTURE_YALMS: f32 = 1.5;

#[derive(Debug, Default, Clone)]
pub struct ShaftDir {
    pub routines: Vec<Scheduler>,
    pub seps: HashMap<[u8; 4], Sep>,
    pub generators: HashMap<[u8; 4], Generator>,
}

impl ShaftDir {
    fn has(&self, name: &[u8; 4]) -> bool {
        self.routines.iter().any(|s| &s.name == name)
    }

    /// The routine's travel stage length in seconds, when it states one.
    pub fn travel_secs(&self, routine: &[u8; 4]) -> Option<f32> {
        let scheduler = self.routines.iter().find(|s| &s.name == routine)?;
        scheduler
            .stages
            .iter()
            .find(|t| t.stage.kind == StageKind::ElevatorTravel)
            .map(|t| f32::from(t.stage.duration_frames) / ROUTINE_FPS)
    }
}

#[derive(Debug, Clone)]
pub struct Shaft {
    pub rect: ZoneInteraction,
    pub dir: ShaftDir,
}

#[derive(Resource, Default)]
pub struct ZoneElevators {
    /// Scopes the state to one zone DAT the way `ZoneDoors::source_file_id` does:
    /// a zone warp keeps `AppPhase::InGame`, so no `OnExit` runs.
    source_file_id: Option<u32>,
    shafts: HashMap<u32, Shaft>,
    /// Current platform height per FourCC, FFXI-native y (grows down).
    heights: HashMap<u32, f32>,
    load: Option<Task<HashMap<u32, Shaft>>>,
}

impl ZoneElevators {
    pub fn from_dat(bytes: &[u8]) -> Self {
        Self {
            shafts: shafts(bytes),
            ..Self::default()
        }
    }

    pub fn shaft(&self, four_cc: u32) -> Option<&Shaft> {
        self.shafts.get(&four_cc)
    }

    /// The shaft a platform entity drives: the one whose RID box holds the
    /// entity's wire position, else the one its wire FourCC names. `wire` is
    /// (x, y, z) as the position packets carry them, z the vertical.
    pub fn shaft_for(&self, wire: [f32; 3], labelled: Option<u32>) -> Option<u32> {
        let native = [wire[0], wire[2], wire[1]];
        self.shafts
            .iter()
            .find_map(|(four_cc, shaft)| shaft.rect.contains(native).then_some(*four_cc))
            .or_else(|| labelled.filter(|cc| self.shafts.contains_key(cc)))
    }

    pub fn insert_shaft(&mut self, shaft: Shaft) {
        self.shafts.insert(shaft.rect.rect_id(), shaft);
    }

    pub fn set_height(&mut self, four_cc: u32, y: f32) {
        self.heights.insert(four_cc, y);
    }

    pub fn height(&self, four_cc: u32) -> Option<f32> {
        self.heights.get(&four_cc).copied()
    }

    /// The platform height under a rider, or `None` when the wire position is
    /// not on any lift. `wire` is (x, y, z) as c2s 0x015 carries them: z is the
    /// vertical, growing down.
    pub fn ride_height(&self, wire: [f32; 3]) -> Option<f32> {
        let native = [wire[0], wire[2], wire[1]];
        self.shafts.iter().find_map(|(four_cc, shaft)| {
            let y = self.height(*four_cc)?;
            (shaft.rect.contains(native) && (native[1] - y).abs() <= RIDE_CAPTURE_YALMS)
                .then_some(y)
        })
    }

    fn clear_zone_state(&mut self) {
        self.shafts.clear();
        self.heights.clear();
        self.load = None;
    }
}

/// Where a platform is `elapsed_secs` into the leg `animation` names, between the
/// RID entry's two floor heights; `None` when the byte is not a lift leg. A leg
/// with no known length, or one that has run its course, rests at its destination.
pub fn platform_height(
    rect: &ZoneInteraction,
    animation: u8,
    elapsed_secs: Option<f32>,
    travel_secs: Option<f32>,
) -> Option<f32> {
    let (from, to) = match animation {
        animation::ELEVATOR_UP => (rect.elevator_bottom_y, rect.elevator_top_y),
        animation::ELEVATOR_DOWN => (rect.elevator_top_y, rect.elevator_bottom_y),
        _ => return None,
    };
    let progress = match (elapsed_secs, travel_secs) {
        (Some(elapsed), Some(travel)) if travel > 0.0 => (elapsed / travel).clamp(0.0, 1.0),
        _ => 1.0,
    };
    Some(from + (to - from) * progress)
}

pub fn routine_for(animation: u8) -> Option<[u8; 4]> {
    match animation {
        animation::ELEVATOR_UP => Some(ROUTINE_UP),
        animation::ELEVATOR_DOWN => Some(ROUTINE_DOWN),
        _ => None,
    }
}

fn shaft_dirs(bytes: &[u8]) -> HashMap<u32, ShaftDir> {
    fn walk(node: &ChunkNode<'_>, out: &mut HashMap<u32, ShaftDir>) {
        for child in &node.children {
            if child.children.is_empty() {
                continue;
            }
            let mut dir = ShaftDir::default();
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
                    Some(ChunkKind::Generator) => {
                        if let Ok(Some(g)) = Generator::parse(c.name, c.data) {
                            dir.generators.insert(c.name, g);
                        }
                    }
                    _ => {}
                }
            }
            if dir.has(&ROUTINE_UP) || dir.has(&ROUTINE_DOWN) {
                out.insert(u32::from_le_bytes(child.chunk.name), dir);
            }
            walk(child, out);
        }
    }
    let mut out = HashMap::new();
    walk(&walk_tree(bytes), &mut out);
    out
}

/// Every lift in a zone DAT: an `@`-tagged RID entry joined to the routine
/// directory of the same FourCC. A shaft with either half missing has no
/// motion the client can play, so it is left out.
pub fn shafts(bytes: &[u8]) -> HashMap<u32, Shaft> {
    let mut dirs = shaft_dirs(bytes);
    zone_interaction::from_dat(bytes)
        .unwrap_or_default()
        .into_iter()
        .filter(ZoneInteraction::is_elevator)
        .filter_map(|rect| {
            let dir = dirs.remove(&rect.rect_id())?;
            Some((rect.rect_id(), Shaft { rect, dir }))
        })
        .collect()
}

fn load_shafts(root: &DatRoot, file_id: u32) -> HashMap<u32, Shaft> {
    let bytes = root
        .resolve(file_id)
        .ok()
        .and_then(|loc| std::fs::read(loc.path_under(root)).ok())
        .unwrap_or_default();
    shafts(&bytes)
}

pub fn sync_zone_elevators(
    scene_state: Res<SceneState>,
    mut lifts: ResMut<ZoneElevators>,
    dat_root: Res<crate::dat_root::SharedDatRoot>,
) {
    let Some(root) = dat_root.get() else {
        return;
    };
    let current = effective_zone_file_id(&scene_state.snapshot);
    if current != lifts.source_file_id {
        lifts.source_file_id = current;
        lifts.clear_zone_state();
        if let Some(file_id) = current {
            let root = root.clone();
            lifts.load =
                Some(AsyncComputeTaskPool::get().spawn(async move { load_shafts(&root, file_id) }));
        }
    }
    let Some(task) = &mut lifts.load else { return };
    let Some(data) = future::block_on(future::poll_once(task)) else {
        return;
    };
    info!(
        "elevators: DAT {:?} -> {} lift shaft(s)",
        lifts.source_file_id,
        data.len()
    );
    lifts.shafts = data;
    lifts.load = None;
}

/// A lift platform entity, tagged once its FourCC matched a shaft.
#[derive(Component, Debug, Clone, Copy)]
pub struct ElevatorNpc {
    pub four_cc: u32,
    /// Last animation byte acted on; the server repeats it on every 0x0E.
    pub animation: u8,
}

fn shaft_label(four_cc: u32) -> String {
    String::from_utf8_lossy(&four_cc.to_le_bytes()).into_owned()
}

/// Poses every platform from the wire's leg + timestamp, and starts the leg's
/// routine (its sounds and effects) when the byte changes. A platform first
/// seen mid-leg takes its computed height silently, as a door first seen open
/// takes its pose.
pub fn drive_elevators(
    scene_state: Res<SceneState>,
    tracked: Res<TrackedEntities>,
    clock: Res<VanaClock>,
    mut lifts: ResMut<ZoneElevators>,
    mut doors: ResMut<ZoneDoors>,
    mut q_npc: Query<&mut ElevatorNpc>,
    mut commands: Commands,
) {
    if lifts.shafts.is_empty() {
        return;
    }
    let now = clock.earth_unix_now() - EARTH_EPOCH_UNIX as f64;
    let lifts = &mut *lifts;
    for wire in &scene_state.snapshot.entities {
        let Some(EntityLook::Transport {
            size: MODEL_ELEVATOR,
            model_id,
            animation_start,
            travel_secs,
        }) = wire.look
        else {
            continue;
        };
        let Some(four_cc) = lifts.shaft_for([wire.pos.x, wire.pos.y, wire.pos.z], model_id) else {
            continue;
        };
        let Some(shaft) = lifts.shafts.get(&four_cc) else {
            continue;
        };
        let Some(routine) = routine_for(wire.animation) else {
            continue;
        };
        let travel = shaft
            .dir
            .travel_secs(&routine)
            .or(travel_secs.map(f32::from));
        let elapsed = animation_start.map(|start| (now - f64::from(start)).max(0.0) as f32);
        let Some(height) = platform_height(&shaft.rect, wire.animation, elapsed, travel) else {
            continue;
        };
        lifts.heights.insert(four_cc, height);
        doors.set_platform_height(four_cc, height);

        let Some(&entity) = tracked.by_id.get(&wire.id) else {
            continue;
        };
        let changed = match q_npc.get_mut(entity) {
            Ok(mut npc) => {
                if npc.animation == wire.animation {
                    continue;
                }
                npc.animation = wire.animation;
                true
            }
            Err(_) => {
                commands.entity(entity).try_insert(ElevatorNpc {
                    four_cc,
                    animation: wire.animation,
                });
                false
            }
        };
        if !changed {
            continue;
        }
        let Some(active) = ActiveScheduler::from_main(&shaft.dir.routines, &routine) else {
            continue;
        };
        enqueue_routine(&mut commands, entity, active.with_target(Some(entity)));
        commands.entity(entity).try_insert_if_new(ActionAssets {
            seps: shaft.dir.seps.clone(),
            generators: shaft.dir.generators.clone(),
            ..Default::default()
        });
        info!(
            "elevators: {} runs {} over {:?}s",
            shaft_label(four_cc),
            String::from_utf8_lossy(&routine),
            travel
        );
    }
}

pub struct ElevatorsPlugin;

impl Plugin for ElevatorsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneElevators>().add_systems(
            Update,
            (sync_zone_elevators, drive_elevators)
                .chain()
                .before(crate::scheduler_runtime::tick_active_schedulers),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::scheduler::{SchedulerStage, TimedStage};

    const SHAFT: [u8; 4] = *b"@6l0";
    const LOWER_FLOOR_Y: f32 = 1.962;
    const UPPER_FLOOR_Y: f32 = -9.983;
    const TRAVEL_FRAMES: u16 = 480;

    fn shaft_rect() -> ZoneInteraction {
        ZoneInteraction {
            position: [-56.0, -13.1, -12.0],
            rect_class: 0,
            orientation: [0.0; 3],
            size: [5.15, 31.475, 5.15],
            source_id: ffxi_dat::datid::DatId(SHAFT),
            dest_id: None,
            param: 0,
            terrain_flags: 0,
            map_id: 0,
            elevator_bottom_y: LOWER_FLOOR_Y,
            elevator_top_y: UPPER_FLOOR_Y,
        }
    }

    fn travel_stage(duration: u16) -> TimedStage {
        TimedStage {
            frame: 0,
            stage: SchedulerStage {
                stage_words: ffxi_dat::scheduler::SYNTHESIZED_STAGE_WORDS,
                kind: StageKind::ElevatorTravel,
                raw_type: 0x1D,
                delay_frames: duration,
                duration_frames: duration,
                id: [0; 4],
                max_loops: 0,
                transition_in: 0,
                transition_out: 0,
                model_transform: None,
                follow_points: None,
                screen_color: None,
                actor_fade: None,
                idle_transition_time: None,
                flinch_duration: None,
                model_visibility: None,
                spell_effect: None,
                random_group: None,
                sound_range: None,
                control_flow: None,
                actor_rotation: None,
                animation_mode: None,
                turn_toward_step_degrees: None,
                local_dir: ffxi_dat::scheduler::NO_LOCAL_DIR,
            },
        }
    }

    fn shaft_dir() -> ShaftDir {
        ShaftDir {
            routines: vec![
                Scheduler {
                    name: ROUTINE_UP,
                    stages: vec![travel_stage(TRAVEL_FRAMES)],
                },
                Scheduler {
                    name: ROUTINE_DOWN,
                    stages: vec![travel_stage(TRAVEL_FRAMES)],
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn elevator_state_contract() {
        let rect = shaft_rect();
        let dir = shaft_dir();
        let travel = dir.travel_secs(&ROUTINE_UP);
        assert_eq!(travel, Some(TRAVEL_FRAMES as f32 / ROUTINE_FPS));
        assert_eq!(dir.travel_secs(b"mv00"), None);

        // `elevator_up` runs ev0 -> ev1, so the FFXI y decreases; `elevator_down` runs back.
        let near = |a: Option<f32>, b: f32| (a.unwrap() - b).abs() < 1e-4;
        let up = |elapsed| platform_height(&rect, animation::ELEVATOR_UP, Some(elapsed), travel);
        assert!(near(up(0.0), LOWER_FLOOR_Y));
        assert!(near(up(4.0), (LOWER_FLOOR_Y + UPPER_FLOOR_Y) / 2.0));
        assert!(near(up(8.0), UPPER_FLOOR_Y));
        assert!(
            near(up(500.0), UPPER_FLOOR_Y),
            "a finished leg rests at its floor"
        );
        let down =
            |elapsed| platform_height(&rect, animation::ELEVATOR_DOWN, Some(elapsed), travel);
        assert!(near(down(0.0), UPPER_FLOOR_Y));
        assert!(near(down(8.0), LOWER_FLOOR_Y));
        assert!(
            near(
                platform_height(&rect, animation::ELEVATOR_UP, None, travel),
                UPPER_FLOOR_Y
            ),
            "no timestamp: the platform is where the leg ends"
        );
        assert!(
            near(
                platform_height(&rect, animation::ELEVATOR_UP, Some(1.0), None),
                UPPER_FLOOR_Y
            ),
            "no travel length: nothing to interpolate over"
        );
        assert_eq!(
            platform_height(&rect, animation::OPEN_DOOR, Some(1.0), travel),
            None
        );
        assert_eq!(routine_for(animation::ELEVATOR_UP), Some(ROUTINE_UP));
        assert_eq!(routine_for(animation::ELEVATOR_DOWN), Some(ROUTINE_DOWN));
        assert_eq!(routine_for(animation::NONE), None);

        // A wire travel byte only stands in when the routine states none.
        let wire_travel = Some(f32::from(7u8));
        assert_eq!(
            dir.travel_secs(&ROUTINE_UP).or(wire_travel),
            Some(8.0),
            "the DAT's 480 frames win over LSB's rounded seconds"
        );
        assert_eq!(
            ShaftDir::default().travel_secs(&ROUTINE_UP).or(wire_travel),
            Some(7.0)
        );

        // Riding: a wire position inside the shaft box within reach of the platform
        // is carried; the landing above an empty shaft is not.
        let four_cc = u32::from_le_bytes(SHAFT);
        let mut lifts = ZoneElevators::default();
        lifts.shafts.insert(four_cc, Shaft { rect, dir });
        assert_eq!(
            lifts.ride_height([-56.0, -12.0, LOWER_FLOOR_Y]),
            None,
            "no height yet"
        );
        lifts.heights.insert(four_cc, LOWER_FLOOR_Y);
        let at = |lifts: &ZoneElevators, x, z_h, y_v| lifts.ride_height([x, z_h, y_v]);
        assert_eq!(
            at(&lifts, -56.0, -12.0, LOWER_FLOOR_Y + 0.5),
            Some(LOWER_FLOOR_Y)
        );
        assert_eq!(
            at(&lifts, -56.0, -12.0, LOWER_FLOOR_Y - RIDE_CAPTURE_YALMS),
            Some(LOWER_FLOOR_Y)
        );
        assert_eq!(
            at(&lifts, -56.0, -12.0, UPPER_FLOOR_Y),
            None,
            "upper landing, platform below"
        );
        assert_eq!(
            at(&lifts, -56.0, 12.0, LOWER_FLOOR_Y),
            None,
            "the other shaft"
        );
        assert_eq!(
            at(&lifts, -50.0, -12.0, LOWER_FLOOR_Y),
            None,
            "outside the box"
        );
        lifts.set_height(four_cc, -4.0);
        assert_eq!(
            at(&lifts, -56.0, -12.0, -3.9),
            Some(-4.0),
            "carried mid-shaft"
        );
        assert_eq!(
            at(&lifts, -56.0, -12.0, LOWER_FLOOR_Y),
            None,
            "left behind at the floor"
        );

        // Binding: the box holding the platform entity names its shaft; the wire
        // FourCC only decides for an entity standing in no box.
        const OTHER_SHAFT: [u8; 4] = *b"@6l1";
        let other_cc = u32::from_le_bytes(OTHER_SHAFT);
        let mut other = shaft_rect();
        other.position[2] = -other.position[2];
        other.source_id = ffxi_dat::datid::DatId(OTHER_SHAFT);
        lifts.insert_shaft(Shaft {
            rect: other,
            dir: ShaftDir::default(),
        });
        let centre_of =
            |rect: &ZoneInteraction| [rect.position[0], rect.position[2], rect.position[1]];
        assert_eq!(
            lifts.shaft_for(centre_of(&rect), Some(other_cc)),
            Some(four_cc),
            "an entity labelled for the other shaft drives the one it stands in"
        );
        assert_eq!(
            lifts.shaft_for(centre_of(&rect), None),
            Some(four_cc),
            "no label needed inside a box"
        );
        assert_eq!(
            lifts.shaft_for([0.0, 0.0, 0.0], Some(other_cc)),
            Some(other_cc),
            "outside every box the label decides"
        );
        assert_eq!(
            lifts.shaft_for([0.0, 0.0, 0.0], Some(u32::from_le_bytes(*b"@zzz"))),
            None,
            "a label naming no shaft binds nothing"
        );
        assert_eq!(lifts.shaft_for([0.0, 0.0, 0.0], None), None);
    }

    #[test]
    fn a_shaft_needs_both_its_rect_and_its_routines() {
        assert!(shafts(&[]).is_empty());
        assert!(ZoneElevators::from_dat(&[])
            .shaft(u32::from_le_bytes(SHAFT))
            .is_none());
    }

    #[test]
    fn installed_metalworks_lifts_join_rect_to_routines() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        const METALWORKS_ZONE_DAT: u32 = 337;
        let lifts = ZoneElevators {
            shafts: load_shafts(&root, METALWORKS_ZONE_DAT),
            ..Default::default()
        };
        for tag in [b"@6l0", b"@6l1"] {
            let shaft = lifts.shaft(u32::from_le_bytes(*tag)).expect("lift shaft");
            assert_eq!(shaft.dir.travel_secs(&ROUTINE_UP), Some(8.0));
            assert_eq!(shaft.dir.travel_secs(&ROUTINE_DOWN), Some(8.0));
            assert!(shaft.rect.elevator_top_y < shaft.rect.elevator_bottom_y);
        }
        // LSB's two Metalworks platform NPCs (vendor/server/data/zones/metalworks/npcs.yaml
        // @6l0 at (-56.006, -13.1, 12.014), @6l1 at (-55.978, -13.1, -12.02)) each stand
        // in the box the DAT gives the other name.
        let lsb_6l0 = [-56.006, 12.014, -13.1];
        let lsb_6l1 = [-55.978, -12.02, -13.1];
        assert_eq!(
            lifts.shaft_for(lsb_6l0, Some(u32::from_le_bytes(*b"@6l0"))),
            Some(u32::from_le_bytes(*b"@6l1"))
        );
        assert_eq!(
            lifts.shaft_for(lsb_6l1, Some(u32::from_le_bytes(*b"@6l1"))),
            Some(u32::from_le_bytes(*b"@6l0"))
        );
    }
}
