//! Placed Mog House furniture: the client object the shelf scan makes for each placed piece, drawn as one
//! static model, an NPC look or a mannequin where the floor grid or wall slot puts it, and shown only in a
//! house that can be arranged (.agents/skills/retail-observe/references/2026-10-15-mog-house-arrange.md
//! Part A).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use bevy::prelude::*;
use bevy::tasks::{futures_lite::future, AsyncComputeTaskPool, Task};
use ffxi_dat::furniture::{
    self as law, FurnitureAttr, FurnitureDat, FurnitureModel, FurniturePose, StaticMeshPick,
};
use ffxi_dat::DatRoot;
use kuluu_snapshot::{EntityLook, SceneSnapshot};

use crate::components::{InGameEntity, WorldEntity};
use crate::dat_mmb::{LoadMmbRequest, StaticModelPart, StaticModelTarget};
use crate::dat_mzb::{placement_bevy_transform, LoadMzbInFlight, ZoneAreaMap, ZONE_SLOT_MAIN};
use crate::dat_root::SharedDatRoot;
use crate::ffxi_actor_render::{ActorSubject, FfxiRenderActor, FfxiRenderRoot, LoadActorRequest};
use crate::ffxi_zone_material::{FfxiZoneMaterial, ZoneLightBindings, ZoneMaterialTouched};
use crate::hud::item_dat_root::{ItemDatRoot, ItemIconCache};
use crate::scene::{furniture_actor_id, TrackedEntities};
use crate::snapshot::SceneState;

/// The house the pieces belong to. Any change (zone-in, floor, house) rebuilds every piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MyroomRoom {
    pub zone_generation: u64,
    pub zone_id: Option<u16>,
    /// s2c 0x00A `MyroomMapNumber`.
    pub map_number: u16,
    pub floor: u8,
}

impl MyroomRoom {
    /// The house the snapshot stands in; none outside a Mog House and in the Mog Garden, where the shelf
    /// scan does not run.
    pub fn of(snap: &SceneSnapshot) -> Option<Self> {
        let room = snap.myroom?;
        if snap.zone_id == Some(law::MOG_GARDEN_ZONE) {
            return None;
        }
        Some(Self {
            zone_generation: snap.zone_generation,
            zone_id: snap.zone_id,
            map_number: room.model,
            floor: law::floor_of_sub_map(room.sub_map),
        })
    }

    pub fn arrangeable(&self, snap: &SceneSnapshot) -> bool {
        law::house_is_arrangeable(
            self.floor,
            self.map_number,
            crate::hud::unity_fp::in_mog_house(snap),
        )
    }
}

/// Every piece's root entity by seat ([`law::seat_index`]).
#[derive(Resource, Default)]
pub struct MyroomPieces {
    by_seat: BTreeMap<usize, Entity>,
    room: Option<MyroomRoom>,
}

impl MyroomPieces {
    pub fn entity(&self, seat: usize) -> Option<Entity> {
        self.by_seat.get(&seat).copied()
    }

    /// Seat order: first shelf slots, then second shelf slots.
    pub fn seats(&self) -> impl Iterator<Item = (usize, Entity)> + '_ {
        self.by_seat.iter().map(|(seat, entity)| (*seat, *entity))
    }

    pub fn room(&self) -> Option<MyroomRoom> {
        self.room
    }
}

/// One placed piece: the client object for a shelf seat.
#[derive(Component, Debug, Clone)]
pub struct MyroomPiece {
    pub seat: usize,
    pub container: u8,
    pub slot: u8,
    pub item_no: u16,
    /// Item DAT kind: furnishing, flowerpot or mannequin.
    pub kind: u8,
    /// Where the piece is drawn: the record's pose when the piece was made. The shelf scan leaves it
    /// alone (A8), so an editor moves a piece by setting it.
    pub pose: FurniturePose,
    /// A record for this seat has shown the installed bit; only then does a cleared bit destroy the piece.
    pub seen_installed: bool,
    pub flowerpot_step: u8,
    pub flowerpot_kind: u8,
    pub mannequin_race: u8,
    pub mannequin_pose: u8,
    /// Hidden whatever the house allows.
    pub editor_hidden: bool,
    plant: Option<Entity>,
    plant_step: Option<u8>,
}

impl MyroomPiece {
    pub fn new(container: u8, slot: u8, item_no: u16, kind: u8, attr: &FurnitureAttr) -> Self {
        Self {
            seat: law::seat_index(container, slot),
            container,
            slot,
            item_no,
            kind,
            pose: attr.pose,
            seen_installed: attr.installed,
            flowerpot_step: attr.flowerpot_step,
            flowerpot_kind: attr.flowerpot_kind,
            mannequin_race: attr.mannequin_race,
            mannequin_pose: attr.mannequin_pose,
            editor_hidden: false,
            plant: None,
            plant_step: None,
        }
    }

    /// The plant model entity a flowerpot carries.
    pub fn plant(&self) -> Option<Entity> {
        self.plant
    }

    fn held(&self) -> HeldPiece {
        HeldPiece {
            item_no: self.item_no,
            seen_installed: self.seen_installed,
            flowerpot_step: self.flowerpot_step,
        }
    }
}

/// The colour byte that means 1.0.
const TINT_BYTE_ONE: f32 = 128.0;

/// The colour a piece's static model is multiplied by.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct MyroomPieceTint(pub Vec4);

impl Default for MyroomPieceTint {
    fn default() -> Self {
        Self(Vec4::ONE)
    }
}

impl MyroomPieceTint {
    /// From (R, G, B, A) colour bytes.
    pub fn from_bytes(rgba: [u8; 4]) -> Self {
        Self(Vec4::from_array(rgba.map(|b| f32::from(b) / TINT_BYTE_ONE)))
    }
}

/// The plant a flowerpot grows: a child of the pot's root.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct MyroomPlant {
    pub pot_seat: usize,
    pub model: u16,
}

/// The entity a static model's submeshes hang from: FFXI model space under the piece root.
#[derive(Component, Debug, Clone, Copy)]
pub struct StaticModelFrame;

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum ModelState {
    #[default]
    Pending,
    File {
        file_id: u32,
        frame: Entity,
        requested: bool,
    },
    Actor(u32),
    Absent,
}

/// What a piece or plant draws, and the rotation it is drawn at.
#[derive(Component, Debug, Default)]
pub struct PieceModel {
    state: ModelState,
    drawn_yaw: Option<f32>,
    eases: bool,
}

impl PieceModel {
    fn file(file_id: u32, frame: Entity) -> Self {
        Self {
            state: ModelState::File {
                file_id,
                frame,
                requested: false,
            },
            ..default()
        }
    }

    pub fn file_id(&self) -> Option<u32> {
        match self.state {
            ModelState::File { file_id, .. } => Some(file_id),
            _ => None,
        }
    }

    pub fn actor_id(&self) -> Option<u32> {
        match self.state {
            ModelState::Actor(id) => Some(id),
            _ => None,
        }
    }

    pub fn drawn_yaw(&self) -> Option<f32> {
        self.drawn_yaw
    }

    fn step_yaw(&mut self, target: f32, ticks: i32) -> f32 {
        let yaw = match self.drawn_yaw {
            Some(drawn) if self.eases => law::ease_yaw(drawn, target, ticks),
            _ => target,
        };
        self.drawn_yaw = Some(yaw);
        yaw
    }
}

/// The furniture DAT's tables, read once per install.
#[derive(Resource, Default)]
pub struct MyroomFurnitureTables {
    dat: Option<Arc<FurnitureDat>>,
    loading: Option<Task<Option<FurnitureDat>>>,
    unavailable: bool,
}

impl MyroomFurnitureTables {
    pub fn with(dat: FurnitureDat) -> Self {
        Self {
            dat: Some(Arc::new(dat)),
            ..default()
        }
    }

    pub fn get(&self) -> Option<&Arc<FurnitureDat>> {
        self.dat.as_ref()
    }
}

/// Which mesh each static model file draws, read once per file.
#[derive(Resource, Default)]
pub struct StaticModelPicks {
    picks: HashMap<u32, Option<StaticMeshPick>>,
    loading: HashMap<u32, Task<Option<StaticMeshPick>>>,
}

impl StaticModelPicks {
    pub fn insert(&mut self, file_id: u32, pick: Option<StaticMeshPick>) {
        self.picks.insert(file_id, pick);
    }

    fn poll(
        &mut self,
        file_id: u32,
        root: Option<&Arc<DatRoot>>,
    ) -> Option<Option<StaticMeshPick>> {
        if let Some(pick) = self.picks.get(&file_id) {
            return Some(*pick);
        }
        if let Some(task) = self.loading.get_mut(&file_id) {
            let pick = future::block_on(future::poll_once(task))?;
            self.loading.remove(&file_id);
            self.picks.insert(file_id, pick);
            return Some(pick);
        }
        let root = root?.clone();
        self.loading.insert(
            file_id,
            AsyncComputeTaskPool::get().spawn(async move { read_static_mesh_pick(&root, file_id) }),
        );
        None
    }
}

fn read_static_mesh_pick(root: &DatRoot, file_id: u32) -> Option<StaticMeshPick> {
    let location = root.resolve(file_id).ok()?;
    law::pick_static_mesh(&std::fs::read(location.path_under(root)).ok()?)
}

/// Item DAT kinds by item id, filled from the install's item table as the scan meets them, and the
/// Storage slots a placed piece adds.
#[derive(Resource, Default)]
pub struct MyroomItemKinds {
    known: HashMap<u16, Option<u8>>,
    storage: HashMap<u16, u32>,
}

impl MyroomItemKinds {
    pub fn insert(&mut self, item_no: u16, kind: Option<u8>) {
        self.known.insert(item_no, kind);
    }

    pub fn insert_storage(&mut self, item_no: u16, storage: u32) {
        self.storage.insert(item_no, storage);
    }

    /// The item's DAT kind and its storage slot count, `None` until its kind is known.
    pub fn furnishing(
        &mut self,
        item_no: u16,
        table: Option<&ffxi_dat::item_dat::ItemTable>,
    ) -> Option<(u8, u32)> {
        let kind = self.kind(item_no, table)?;
        if let Some(storage) = self.storage.get(&item_no) {
            return Some((kind, *storage));
        }
        let Some(table) = table else {
            return Some((kind, 0));
        };
        let storage = table
            .lookup(item_no)
            .map_or(0, |record| record.storage_slots);
        self.storage.insert(item_no, storage);
        Some((kind, storage))
    }

    fn kind(&mut self, item_no: u16, table: Option<&ffxi_dat::item_dat::ItemTable>) -> Option<u8> {
        if let Some(kind) = self.known.get(&item_no) {
            return *kind;
        }
        let kind = table?.lookup(item_no).map(|record| record.item_type);
        self.known.insert(item_no, kind);
        kind
    }
}

/// One Mog House shelf record as the scan reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShelfRecord {
    pub container: u8,
    pub slot: u8,
    pub item_no: u16,
    pub attr: Option<[u8; law::ATTR_LEN]>,
}

/// Both shelves' records in scan order: the first shelf's slots, then the second's.
pub fn shelf_records(snap: &SceneSnapshot) -> Vec<ShelfRecord> {
    law::SHELVES
        .iter()
        .flat_map(|&container| {
            let mut records: Vec<ShelfRecord> = snap
                .container(container)
                .map(|view| {
                    view.items
                        .iter()
                        .map(|item| ShelfRecord {
                            container,
                            slot: item.index,
                            item_no: item.item_no,
                            attr: item.extdata,
                        })
                        .collect()
                })
                .unwrap_or_default();
            records.sort_by_key(|record| record.slot);
            records
        })
        .collect()
}

/// What the scan knows of a piece it already holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldPiece {
    pub item_no: u16,
    pub seen_installed: bool,
    pub flowerpot_step: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanAction {
    Create {
        container: u8,
        slot: u8,
        item_no: u16,
        kind: u8,
        attr: FurnitureAttr,
    },
    Destroy {
        seat: usize,
    },
    SeenInstalled {
        seat: usize,
    },
    Replant {
        seat: usize,
        flowerpot_step: u8,
        flowerpot_kind: u8,
    },
}

/// One pass of the shelf scan (A8): a piece is made for a seat only when none exists, from a furniture-kind
/// record installed on this floor; an existing piece keeps its pose; a piece seen installed is destroyed when
/// its record shows the bit clear; a pot is replanted when its step changes. A seat whose record is gone or
/// now holds another item loses its piece. `kind_of` answers `None` until it can, and such a record waits.
pub fn scan_shelf_records(
    records: &[ShelfRecord],
    floor: u8,
    held: &BTreeMap<usize, HeldPiece>,
    kind_of: &mut dyn FnMut(u16) -> Option<u8>,
) -> Vec<ScanAction> {
    let mut actions = Vec::new();
    let mut seen = HashSet::new();
    for record in records {
        if !law::is_shelf_slot(record.slot) {
            continue;
        }
        let seat = law::seat_index(record.container, record.slot);
        seen.insert(seat);
        let attr = record.attr.as_ref().map(FurnitureAttr::decode);
        match held.get(&seat) {
            Some(piece) => {
                if piece.item_no != record.item_no {
                    actions.push(ScanAction::Destroy { seat });
                    continue;
                }
                let Some(attr) = attr else {
                    continue;
                };
                if !attr.installed {
                    if piece.seen_installed {
                        actions.push(ScanAction::Destroy { seat });
                    }
                    continue;
                }
                if !piece.seen_installed {
                    actions.push(ScanAction::SeenInstalled { seat });
                }
                if attr.flowerpot_step != piece.flowerpot_step {
                    actions.push(ScanAction::Replant {
                        seat,
                        flowerpot_step: attr.flowerpot_step,
                        flowerpot_kind: attr.flowerpot_kind,
                    });
                }
            }
            None => {
                let Some(attr) = attr.filter(|attr| attr.installed && attr.layer == floor) else {
                    continue;
                };
                let Some(kind) =
                    kind_of(record.item_no).filter(|kind| law::is_furniture_kind(*kind))
                else {
                    continue;
                };
                actions.push(ScanAction::Create {
                    container: record.container,
                    slot: record.slot,
                    item_no: record.item_no,
                    kind,
                    attr,
                });
            }
        }
    }
    actions.extend(
        held.keys()
            .filter(|seat| !seen.contains(*seat))
            .map(|&seat| ScanAction::Destroy { seat }),
    );
    actions
}

/// Makes the client object for one seat. The shelf scan calls it for every new placed record; an editor
/// placing a piece that is not installed yet calls it the same way.
pub fn spawn_piece(
    commands: &mut Commands,
    pieces: &mut MyroomPieces,
    piece: MyroomPiece,
) -> Entity {
    let seat = piece.seat;
    let entity = commands
        .spawn((
            piece,
            MyroomPieceTint::default(),
            PieceModel::default(),
            Transform::default(),
            Visibility::Hidden,
            InGameEntity,
        ))
        .id();
    if let Some(previous) = pieces.by_seat.insert(seat, entity) {
        commands.entity(previous).try_despawn();
    }
    entity
}

/// Removes the client object for one seat: the scan for a record gone or put away, the editor for a new
/// piece it never placed.
pub fn destroy_piece(
    commands: &mut Commands,
    pieces: &mut MyroomPieces,
    tracked: &mut TrackedEntities,
    seat: usize,
) {
    let Some(entity) = pieces.by_seat.remove(&seat) else {
        return;
    };
    commands.entity(entity).try_despawn();
    let actor = furniture_actor_id(seat);
    if tracked.by_id.get(&actor) == Some(&entity) {
        tracked.by_id.remove(&actor);
    }
}

pub fn scan_myroom_shelves(
    mut commands: Commands,
    state: Res<SceneState>,
    mut pieces: ResMut<MyroomPieces>,
    mut kinds: ResMut<MyroomItemKinds>,
    item_root: Option<Res<ItemDatRoot>>,
    icon_cache: Option<ResMut<ItemIconCache>>,
    mut q_pieces: Query<&mut MyroomPiece>,
    mut tracked: ResMut<TrackedEntities>,
) {
    let snap = &state.snapshot;
    let room = MyroomRoom::of(snap);
    if pieces.room != room {
        let seats: Vec<usize> = pieces.by_seat.keys().copied().collect();
        for seat in seats {
            destroy_piece(&mut commands, &mut pieces, &mut tracked, seat);
        }
        pieces.room = room;
    }
    let Some(room) = room else {
        return;
    };
    let held: BTreeMap<usize, HeldPiece> = pieces
        .by_seat
        .iter()
        .filter_map(|(seat, entity)| q_pieces.get(*entity).ok().map(|p| (*seat, p.held())))
        .collect();
    let table = match (item_root, icon_cache) {
        (Some(root), Some(mut cache)) => cache.table(&root),
        _ => None,
    };
    let actions = scan_shelf_records(&shelf_records(snap), room.floor, &held, &mut |item| {
        kinds.kind(item, table.as_deref())
    });
    for action in actions {
        match action {
            ScanAction::Create {
                container,
                slot,
                item_no,
                kind,
                attr,
            } => {
                spawn_piece(
                    &mut commands,
                    &mut pieces,
                    MyroomPiece::new(container, slot, item_no, kind, &attr),
                );
            }
            ScanAction::Destroy { seat } => {
                destroy_piece(&mut commands, &mut pieces, &mut tracked, seat);
            }
            ScanAction::SeenInstalled { seat } => {
                if let Some(mut piece) = pieces.entity(seat).and_then(|e| q_pieces.get_mut(e).ok())
                {
                    piece.seen_installed = true;
                }
            }
            ScanAction::Replant {
                seat,
                flowerpot_step,
                flowerpot_kind,
            } => {
                let Some(mut piece) = pieces.entity(seat).and_then(|e| q_pieces.get_mut(e).ok())
                else {
                    continue;
                };
                if let Some(plant) = piece.plant.take() {
                    commands.entity(plant).try_despawn();
                }
                piece.flowerpot_step = flowerpot_step;
                piece.flowerpot_kind = flowerpot_kind;
                piece.plant_step = None;
            }
        }
    }
}

/// FFXI model space to Bevy: the basis every placed model's matrix carries.
fn ffxi_basis() -> Mat4 {
    placement_bevy_transform(Vec3::ONE, Vec3::ZERO, Vec3::ZERO)
}

/// An FFXI world point (y down) in Bevy space, the way pieces stand.
pub fn ffxi_world_to_bevy(point: [f32; 3]) -> Vec3 {
    ffxi_basis().transform_point3(Vec3::from_array(point))
}

/// A Bevy point as an FFXI world point.
pub fn bevy_to_ffxi_world(point: Vec3) -> [f32; 3] {
    ffxi_basis().inverse().transform_point3(point).to_array()
}

/// A piece root: its FFXI world point in Bevy space, turned by its FFXI yaw the way an entity heading
/// turns. With [`static_frame_transform`] under it, it is the placed-model matrix of the same point and yaw.
pub fn piece_root_transform(point: [f32; 3], yaw: f32) -> Transform {
    Transform {
        translation: ffxi_basis().transform_point3(Vec3::from_array(point)),
        rotation: Quat::from_rotation_y(-yaw),
        scale: Vec3::ONE,
    }
}

pub fn static_frame_transform() -> Transform {
    Transform::from_matrix(ffxi_basis())
}

fn relative_to(parent: &Transform, world: &Transform) -> Transform {
    Transform::from_matrix(parent.to_matrix().inverse() * world.to_matrix())
}

/// Puts every piece at its grid cell or wall slot (A3, A4) and every plant above its pot (A5); a static
/// model picked by name eases its drawn rotation toward the piece's, everything else turns at once.
pub fn pose_myroom_pieces(
    time: Res<Time>,
    tables: Res<MyroomFurnitureTables>,
    pieces: Res<MyroomPieces>,
    mut q_pieces: Query<(&MyroomPiece, &mut PieceModel, &mut Transform), Without<MyroomPlant>>,
    mut q_plants: Query<
        (&mut PieceModel, &mut Transform),
        (With<MyroomPlant>, Without<MyroomPiece>),
    >,
) {
    let (Some(dat), Some(room)) = (tables.get(), pieces.room()) else {
        return;
    };
    let ticks = law::update_ticks(time.delta_secs() * crate::scheduler_runtime::ROUTINE_FPS);
    for (piece, mut model, mut transform) in &mut q_pieces {
        let Some(property) = dat.properties.get(piece.item_no) else {
            continue;
        };
        let Some(point) = law::piece_point(room.floor, &property, piece.pose, room.map_number)
        else {
            continue;
        };
        let target = law::piece_yaw(piece.pose.rot);
        let yaw = model.step_yaw(target, ticks);
        *transform = piece_root_transform(point, yaw);
        let Some((mut plant_model, mut plant_transform)) =
            piece.plant.and_then(|plant| q_plants.get_mut(plant).ok())
        else {
            continue;
        };
        let world =
            piece_root_transform(law::plant_point(point), plant_model.step_yaw(target, ticks));
        *plant_transform = relative_to(&transform, &world);
    }
}

fn piece_light_bindings() -> ZoneLightBindings {
    let mut bindings = ZoneLightBindings::default();
    bindings[0] = Some(u32::from_le_bytes(law::MOG_HOUSE_PIECE_LIGHT));
    bindings
}

fn spawn_frame(commands: &mut Commands, parent: Entity) -> Entity {
    commands
        .spawn((
            StaticModelFrame,
            static_frame_transform(),
            Visibility::Inherited,
            ChildOf(parent),
        ))
        .id()
}

/// The dressed look a mannequin wears comes from a per-slot record the client keeps for the shelf entry,
/// which is untraced (A6), so a mannequin is built with no equipment parts yet.
fn mannequin_equipment() -> Vec<u32> {
    Vec::new()
}

fn dispatch_actor(
    entity: Entity,
    seat: usize,
    subject: ActorSubject,
    tracked: &mut TrackedEntities,
    actors: &mut MessageWriter<LoadActorRequest>,
) -> ModelState {
    let id = furniture_actor_id(seat);
    tracked.by_id.insert(id, entity);
    actors.write(LoadActorRequest {
        entity_id: id,
        subject,
    });
    ModelState::Actor(id)
}

fn resolve_piece_model(
    commands: &mut Commands,
    entity: Entity,
    piece: &MyroomPiece,
    dat: &FurnitureDat,
    tracked: &mut TrackedEntities,
    actors: &mut MessageWriter<LoadActorRequest>,
) -> ModelState {
    if piece.kind == law::KIND_MANNEQUIN {
        let subject = ActorSubject::Pc {
            race: piece.mannequin_race,
            mounted: false,
            equipment: mannequin_equipment(),
            body: None,
            main_weapon: None,
            sub_weapon: None,
            sub_model: None,
        };
        return dispatch_actor(entity, piece.seat, subject, tracked, actors);
    }
    match dat
        .properties
        .get(piece.item_no)
        .map(|property| law::furniture_model(property.model))
    {
        Some(FurnitureModel::File(file_id)) => ModelState::File {
            file_id,
            frame: spawn_frame(commands, entity),
            requested: false,
        },
        Some(FurnitureModel::NpcLook(modelid)) => {
            let subject = ActorSubject::Npc {
                file_id: crate::look_resolver::npc_dat_id(modelid),
                graph_size: 0,
            };
            dispatch_actor(entity, piece.seat, subject, tracked, actors)
        }
        Some(FurnitureModel::Absent) | None => ModelState::Absent,
    }
}

fn spawn_plant(
    commands: &mut Commands,
    pot: Entity,
    piece: &MyroomPiece,
    dat: &FurnitureDat,
) -> Option<Entity> {
    let model = dat
        .plants
        .as_ref()?
        .plant_model(piece.flowerpot_kind, piece.flowerpot_step)?;
    let FurnitureModel::File(file_id) = law::furniture_model(model) else {
        return None;
    };
    let plant = commands
        .spawn((
            MyroomPlant {
                pot_seat: piece.seat,
                model,
            },
            MyroomPieceTint::default(),
            Transform::default(),
            Visibility::Inherited,
            ChildOf(pot),
        ))
        .id();
    let frame = spawn_frame(commands, plant);
    commands
        .entity(plant)
        .insert(PieceModel::file(file_id, frame));
    Some(plant)
}

fn request_static_model(
    model: &mut PieceModel,
    at: Vec3,
    picks: &mut StaticModelPicks,
    root: Option<&Arc<DatRoot>>,
    areas: &ZoneAreaMap,
    requests: &mut MessageWriter<LoadMmbRequest>,
) {
    let ModelState::File {
        file_id,
        frame,
        requested: false,
    } = model.state
    else {
        return;
    };
    let Some(pick) = picks.poll(file_id, root) else {
        return;
    };
    let Some(pick) = pick else {
        model.state = ModelState::Absent;
        return;
    };
    model.eases = pick.eases_rotation();
    model.state = ModelState::File {
        file_id,
        frame,
        requested: true,
    };
    requests.write(LoadMmbRequest {
        area_id: areas.area_at(at).unwrap_or_default(),
        light_bindings: piece_light_bindings(),
        voyage_backdrop: false,
        file_id,
        chunk_idx: pick.chunk_index,
        world_pos: at,
        entity_id: None,
        world_transform: None,
        water: None,
        lod: None,
        door: None,
        slot: ZONE_SLOT_MAIN,
        sub_area_link: ffxi_dat::mzb::NO_SUB_AREA_LINK,
        static_model: Some(StaticModelTarget {
            frame,
            alpha_test: pick.alpha_tested(),
        }),
    });
}

/// Gives each new piece its model (A2): one static model file, an NPC look for the model numbers that name
/// one, a character for a mannequin (A6), and a pot's plant (A5). Static models wait for the room's own
/// geometry so the area and light they bind exist.
pub fn request_myroom_models(
    mut commands: Commands,
    tables: Res<MyroomFurnitureTables>,
    mut picks: ResMut<StaticModelPicks>,
    dat_root: Res<SharedDatRoot>,
    mzb: Res<LoadMzbInFlight>,
    areas: Res<ZoneAreaMap>,
    mut tracked: ResMut<TrackedEntities>,
    mut q_pieces: Query<
        (Entity, &mut MyroomPiece, &mut PieceModel, &Transform),
        Without<MyroomPlant>,
    >,
    mut q_plants: Query<&mut PieceModel, (With<MyroomPlant>, Without<MyroomPiece>)>,
    mut requests: MessageWriter<LoadMmbRequest>,
    mut actors: MessageWriter<LoadActorRequest>,
) {
    let Some(dat) = tables.get().cloned() else {
        return;
    };
    let root = dat_root.get().cloned();
    let room_loaded = !mzb.any_pending();
    for (entity, mut piece, mut model, transform) in &mut q_pieces {
        if model.state == ModelState::Pending {
            model.state = resolve_piece_model(
                &mut commands,
                entity,
                &piece,
                &dat,
                &mut tracked,
                &mut actors,
            );
        }
        if !room_loaded {
            continue;
        }
        let at = transform.translation;
        request_static_model(
            &mut model,
            at,
            &mut picks,
            root.as_ref(),
            &areas,
            &mut requests,
        );
        let pot = dat
            .properties
            .get(piece.item_no)
            .is_some_and(|property| property.flowerpot());
        if pot && piece.plant.is_none() && piece.plant_step != Some(piece.flowerpot_step) {
            piece.plant_step = Some(piece.flowerpot_step);
            piece.plant = spawn_plant(&mut commands, entity, &piece, &dat);
        }
        if let Some(mut plant_model) = piece.plant.and_then(|plant| q_plants.get_mut(plant).ok()) {
            request_static_model(
                &mut plant_model,
                at,
                &mut picks,
                root.as_ref(),
                &areas,
                &mut requests,
            );
        }
    }
}

/// A7: pieces (and the plants under them) draw only in an arrangeable house, and the statue NPCs only
/// while one of their items is placed on either floor of one.
pub fn apply_myroom_visibility(
    state: Res<SceneState>,
    pieces: Res<MyroomPieces>,
    tracked: Res<TrackedEntities>,
    mut q_pieces: Query<(&MyroomPiece, &mut Visibility)>,
    mut q_world: Query<&mut Visibility, (With<WorldEntity>, Without<MyroomPiece>)>,
) {
    let snap = &state.snapshot;
    let Some(room) = pieces.room() else {
        return;
    };
    let arrangeable = room.arrangeable(snap);
    for (piece, mut visibility) in &mut q_pieces {
        visibility.set_if_neq(if arrangeable && !piece.editor_hidden {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
    let statue_placed = shelf_records(snap).iter().any(|record| {
        law::STATUE_ITEMS.contains(&record.item_no)
            && record
                .attr
                .is_some_and(|attr| FurnitureAttr::decode(&attr).installed)
    });
    if law::statue_npcs_shown(arrangeable, statue_placed) {
        return;
    }
    let statue_look = Some(EntityLook::Standard {
        modelid: law::STATUE_NPC_MODEL,
    });
    for entity in snap.entities.iter().filter(|e| e.look == statue_look) {
        if let Some(mut visibility) = tracked
            .by_id
            .get(&entity.id)
            .and_then(|bevy_entity| q_world.get_mut(*bevy_entity).ok())
        {
            visibility.set_if_neq(Visibility::Hidden);
        }
    }
}

/// Writes each piece's [`MyroomPieceTint`] into its static model's materials.
pub fn apply_myroom_tints(
    q_roots: Query<(Ref<MyroomPieceTint>, &Children)>,
    q_frames: Query<&Children, With<StaticModelFrame>>,
    q_parts: Query<Ref<StaticModelPart>>,
    mut materials: ResMut<Assets<FfxiZoneMaterial>>,
    mut touched: ResMut<ZoneMaterialTouched>,
) {
    for (tint, children) in &q_roots {
        for frame in children.iter().filter_map(|child| q_frames.get(child).ok()) {
            for part in frame.iter().filter_map(|child| q_parts.get(child).ok()) {
                if !tint.is_changed() && !part.is_added() {
                    continue;
                }
                if let Some(material) = materials.get_mut_untracked(&part.material) {
                    material.tint = tint.0;
                    touched.mark(part.material.id());
                }
            }
        }
    }
}

/// A6: a mannequin stands in the `mq<digit>?` idle its pose names.
pub fn drive_mannequin_idles(
    q_pieces: Query<(&MyroomPiece, &FfxiRenderRoot)>,
    mut q_actors: Query<&mut FfxiRenderActor>,
) {
    for (piece, root) in &q_pieces {
        if piece.kind != law::KIND_MANNEQUIN {
            continue;
        }
        let family = law::mannequin_idle(piece.mannequin_pose, None).family;
        if let Ok(mut actor) = q_actors.get_mut(root.0) {
            if actor.idle_family() != Some(family) {
                actor.set_idle_family(Some(family));
            }
        }
    }
}

pub fn load_myroom_tables(
    state: Res<SceneState>,
    dat_root: Res<SharedDatRoot>,
    mut tables: ResMut<MyroomFurnitureTables>,
) {
    if tables.dat.is_some() || tables.unavailable {
        return;
    }
    if let Some(task) = tables.loading.as_mut() {
        let Some(result) = future::block_on(future::poll_once(task)) else {
            return;
        };
        tables.loading = None;
        match result {
            Some(dat) => tables.dat = Some(Arc::new(dat)),
            None => {
                warn!(
                    "furniture DAT {} is unreadable: placed furniture will not draw",
                    law::FURNITURE_DAT_FILE_ID
                );
                tables.unavailable = true;
            }
        }
        return;
    }
    if state.snapshot.myroom.is_none() {
        return;
    }
    let Some(root) = dat_root.get().cloned() else {
        return;
    };
    tables.loading =
        Some(AsyncComputeTaskPool::get().spawn(async move { FurnitureDat::open(&root) }));
}

pub struct MyroomFurniturePlugin;

impl Plugin for MyroomFurniturePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MyroomPieces>()
            .init_resource::<MyroomFurnitureTables>()
            .init_resource::<StaticModelPicks>()
            .init_resource::<MyroomItemKinds>()
            .add_systems(
                Update,
                (
                    load_myroom_tables,
                    scan_myroom_shelves,
                    pose_myroom_pieces,
                    request_myroom_models,
                    apply_myroom_visibility,
                    apply_myroom_tints,
                    drive_mannequin_idles,
                )
                    .chain()
                    .after(crate::dat_mmb::process_load_mmb_requests)
                    .after(crate::scene::sync_entities_system),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::message::Messages;
    use bevy::ecs::system::RunSystemOnce;
    use ffxi_dat::furniture::{
        FurnitureProperty, PlantTable, PropertyTable, StaticMeshRule, FLAG_FLOWERPOT,
        FLAG_WALL_HUNG, KIND_FLOWERPOT, KIND_FURNISHING, KIND_MANNEQUIN,
    };
    use ffxi_proto::decode::ServerLoginMyroom;
    use ffxi_proto::map::container::{LOC_MOGSAFE, LOC_MOGSAFE2};
    use kuluu_snapshot::{ContainerView, EntityKind, InventoryItem, MyRoom};

    const TABLE: u16 = 5;
    const POT: u16 = 6;
    const NPC_PIECE: u16 = 7;
    const MANNEQUIN: u16 = 8;
    const MIRROR: u16 = 9;
    const NOT_FURNITURE: u16 = 10;
    const TABLE_MODEL: u16 = 3;
    const POT_MODEL: u16 = 4;
    const MIRROR_MODEL: u16 = 11;
    const SPROUT_MODEL: u32 = 20;
    const BLOOM_MODEL: u32 = 21;
    const MANNEQUIN_RACE: u8 = 4;
    const STATUE_NPC_ID: u32 = 100;
    const EPSILON: f32 = 1e-4;

    fn pose(col: i8, row: i8, rot: i8) -> FurniturePose {
        FurniturePose {
            col,
            level: 0,
            row,
            rot,
        }
    }

    fn attr(installed: bool, layer: u8, pose: FurniturePose) -> [u8; law::ATTR_LEN] {
        let mut a = [0u8; law::ATTR_LEN];
        a[law::ATTR_FLAGS_BYTE] = layer
            | if installed {
                law::ATTR_INSTALLED_BIT
            } else {
                0
            };
        a[law::ATTR_COL_BYTE] = pose.col as u8;
        a[law::ATTR_LEVEL_BYTE] = pose.level as u8;
        a[law::ATTR_ROW_BYTE] = pose.row as u8;
        a[law::ATTR_ROTATION_BYTE] = pose.rot as u8;
        a
    }

    fn item(container: u8, slot: u8, item_no: u16, attr: [u8; law::ATTR_LEN]) -> InventoryItem {
        InventoryItem {
            container,
            index: slot,
            item_no,
            quantity: 1,
            locked: false,
            unselectable: false,
            charges_remaining: None,
            next_use_vana_ts: None,
            use_delay_end_vana_ts: None,
            ready: None,
            extdata: Some(attr),
        }
    }

    fn property(flags: u8, model: u16, width: u8, depth: u8) -> FurnitureProperty {
        FurnitureProperty {
            flags,
            model,
            width,
            depth,
            height: 0,
        }
    }

    fn dat() -> FurnitureDat {
        FurnitureDat {
            properties: PropertyTable::from_items(&[
                (TABLE, property(0, TABLE_MODEL, 2, 1)),
                (POT, property(FLAG_FLOWERPOT, POT_MODEL, 1, 1)),
                (NPC_PIECE, property(0, *law::NPC_LOOK_MODELS.start(), 1, 1)),
                (MANNEQUIN, property(0, 0, 1, 1)),
                (MIRROR, property(FLAG_WALL_HUNG, MIRROR_MODEL, 1, 1)),
            ]),
            plants: Some(PlantTable::from_rows(&[&[
                law::NO_PLANT_MODEL,
                SPROUT_MODEL,
                BLOOM_MODEL,
            ]])),
            sounds: Default::default(),
        }
    }

    fn kinds() -> MyroomItemKinds {
        let mut kinds = MyroomItemKinds::default();
        for (item, kind) in [
            (TABLE, KIND_FURNISHING),
            (POT, KIND_FLOWERPOT),
            (NPC_PIECE, KIND_FURNISHING),
            (MANNEQUIN, KIND_MANNEQUIN),
            (MIRROR, KIND_FURNISHING),
            (NOT_FURNITURE, KIND_FURNISHING - 1),
        ] {
            kinds.insert(item, Some(kind));
        }
        kinds
    }

    fn snapshot(sub_map: u8, map_number: u16, items: Vec<InventoryItem>) -> SceneSnapshot {
        let containers = law::SHELVES
            .iter()
            .map(|&id| ContainerView {
                id,
                capacity: u16::from(law::LAST_SHELF_SLOT),
                items: items
                    .iter()
                    .filter(|i| i.container == id)
                    .copied()
                    .collect(),
            })
            .collect();
        SceneSnapshot {
            myroom: Some(MyRoom {
                model: map_number,
                sub_map,
                door_open: false,
            }),
            login_state: ServerLoginMyroom::LOGIN_STATE_MYROOM,
            containers,
            ..default()
        }
    }

    fn upstairs(items: Vec<InventoryItem>) -> SceneSnapshot {
        snapshot(ServerLoginMyroom::SUB_MAP_2F, 0, items)
    }

    fn app(snap: SceneSnapshot) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(SceneState {
                snapshot: snap,
                ..default()
            })
            .insert_resource(MyroomFurnitureTables::with(dat()))
            .insert_resource(kinds())
            .init_resource::<MyroomPieces>()
            .init_resource::<StaticModelPicks>()
            .init_resource::<TrackedEntities>()
            .init_resource::<LoadMzbInFlight>()
            .init_resource::<ZoneAreaMap>()
            .init_resource::<SharedDatRoot>()
            .add_message::<LoadMmbRequest>()
            .add_message::<LoadActorRequest>()
            .add_systems(
                Update,
                (
                    scan_myroom_shelves,
                    pose_myroom_pieces,
                    request_myroom_models,
                    apply_myroom_visibility,
                )
                    .chain(),
            );
        app
    }

    fn set_snapshot(app: &mut App, snap: SceneSnapshot) {
        app.world_mut().resource_mut::<SceneState>().snapshot = snap;
    }

    fn seats(app: &App) -> Vec<usize> {
        app.world()
            .resource::<MyroomPieces>()
            .seats()
            .map(|(seat, _)| seat)
            .collect()
    }

    fn piece_entity(app: &App, seat: usize) -> Entity {
        app.world().resource::<MyroomPieces>().entity(seat).unwrap()
    }

    fn piece(app: &App, seat: usize) -> MyroomPiece {
        app.world()
            .get::<MyroomPiece>(piece_entity(app, seat))
            .unwrap()
            .clone()
    }

    fn mmb_requests(app: &mut App) -> Vec<LoadMmbRequest> {
        app.world_mut()
            .resource_mut::<Messages<LoadMmbRequest>>()
            .drain()
            .collect()
    }

    fn actor_requests(app: &mut App) -> Vec<LoadActorRequest> {
        app.world_mut()
            .resource_mut::<Messages<LoadActorRequest>>()
            .drain()
            .collect()
    }

    fn table_file() -> u32 {
        let FurnitureModel::File(file_id) = law::furniture_model(TABLE_MODEL) else {
            panic!("the table model names a file");
        };
        file_id
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < EPSILON
    }

    #[test]
    fn the_scan_makes_a_piece_per_placed_record_and_drops_it_when_the_bit_clears() {
        let records = vec![
            item(
                LOC_MOGSAFE,
                3,
                TABLE,
                attr(true, law::FLOOR_2F, pose(1, 1, 0)),
            ),
            item(
                LOC_MOGSAFE,
                4,
                TABLE,
                attr(false, law::FLOOR_2F, pose(1, 1, 0)),
            ),
            item(
                LOC_MOGSAFE,
                5,
                TABLE,
                attr(true, law::FLOOR_1F, pose(1, 1, 0)),
            ),
            item(
                LOC_MOGSAFE,
                6,
                NOT_FURNITURE,
                attr(true, law::FLOOR_2F, pose(1, 1, 0)),
            ),
            item(
                LOC_MOGSAFE2,
                2,
                POT,
                attr(true, law::FLOOR_2F, pose(4, 4, 0)),
            ),
        ];
        let mut app = app(upstairs(records.clone()));
        app.update();
        let second_shelf_seat = law::seat_index(LOC_MOGSAFE2, 2);
        assert_eq!(seats(&app), vec![3, second_shelf_seat]);
        let table = piece_entity(&app, 3);
        assert_eq!(piece(&app, 3).item_no, TABLE);
        assert!(piece(&app, 3).seen_installed);

        let mut put_away = records;
        put_away[0] = item(
            LOC_MOGSAFE,
            3,
            TABLE,
            attr(false, law::FLOOR_2F, pose(1, 1, 0)),
        );
        set_snapshot(&mut app, upstairs(put_away));
        app.update();
        assert_eq!(seats(&app), vec![second_shelf_seat]);
        assert!(
            app.world().get_entity(table).is_err(),
            "the piece is despawned"
        );
    }

    #[test]
    fn a_piece_keeps_its_own_pose_when_a_later_record_moves_it() {
        let mut app = app(upstairs(vec![item(
            LOC_MOGSAFE,
            3,
            TABLE,
            attr(true, law::FLOOR_2F, pose(1, 1, 0)),
        )]));
        app.update();
        app.update();
        let before = *app.world().get::<Transform>(piece_entity(&app, 3)).unwrap();
        set_snapshot(
            &mut app,
            upstairs(vec![item(
                LOC_MOGSAFE,
                3,
                TABLE,
                attr(true, law::FLOOR_2F, pose(9, 9, 2)),
            )]),
        );
        app.update();
        assert_eq!(piece(&app, 3).pose, pose(1, 1, 0));
        let after = *app.world().get::<Transform>(piece_entity(&app, 3)).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn a_piece_never_seen_installed_survives_until_its_record_shows_it_installed() {
        let unplaced = attr(false, law::FLOOR_2F, pose(2, 2, 0));
        let mut app = app(upstairs(vec![item(LOC_MOGSAFE, 4, TABLE, unplaced)]));
        app.update();
        assert!(seats(&app).is_empty());
        let new_piece = MyroomPiece::new(
            LOC_MOGSAFE,
            4,
            TABLE,
            KIND_FURNISHING,
            &FurnitureAttr::decode(&unplaced),
        );
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands, mut pieces: ResMut<MyroomPieces>| {
                    spawn_piece(&mut commands, &mut pieces, new_piece.clone());
                },
            )
            .unwrap();
        app.update();
        assert_eq!(
            seats(&app),
            vec![4],
            "an uninstalled record keeps a new piece"
        );
        assert!(!piece(&app, 4).seen_installed);

        let placed = attr(true, law::FLOOR_2F, pose(2, 2, 0));
        set_snapshot(
            &mut app,
            upstairs(vec![item(LOC_MOGSAFE, 4, TABLE, placed)]),
        );
        app.update();
        assert!(piece(&app, 4).seen_installed);

        set_snapshot(
            &mut app,
            upstairs(vec![item(LOC_MOGSAFE, 4, TABLE, unplaced)]),
        );
        app.update();
        assert!(seats(&app).is_empty(), "seen installed, then cleared");
    }

    #[test]
    fn leaving_the_house_or_changing_floor_rebuilds_every_piece() {
        let records = vec![
            item(
                LOC_MOGSAFE,
                3,
                TABLE,
                attr(true, law::FLOOR_2F, pose(1, 1, 0)),
            ),
            item(
                LOC_MOGSAFE,
                7,
                TABLE,
                attr(true, law::FLOOR_1F, pose(1, 1, 0)),
            ),
        ];
        let mut app = app(upstairs(records.clone()));
        app.update();
        assert_eq!(seats(&app), vec![3]);
        let upstairs_piece = piece_entity(&app, 3);

        set_snapshot(&mut app, snapshot(0, 0, records.clone()));
        app.update();
        assert_eq!(seats(&app), vec![7], "1F draws only the 1F record");
        assert!(app.world().get_entity(upstairs_piece).is_err());

        let mut outside = snapshot(0, 0, records);
        outside.myroom = None;
        set_snapshot(&mut app, outside);
        app.update();
        assert!(seats(&app).is_empty());
        assert_eq!(app.world().resource::<MyroomPieces>().room(), None);
    }

    #[test]
    fn pieces_draw_only_in_an_arrangeable_house() {
        let record = |layer| {
            vec![item(
                LOC_MOGSAFE,
                3,
                TABLE,
                attr(true, layer, pose(1, 1, 0)),
            )]
        };
        let visibility = |app: &App| *app.world().get::<Visibility>(piece_entity(app, 3)).unwrap();
        let home = *law::HOME_NATION_HOUSES.start();

        let mut app = app(snapshot(0, home - 1, record(law::FLOOR_1F)));
        app.update();
        app.update();
        assert_eq!(
            visibility(&app),
            Visibility::Hidden,
            "a rent-a-room hides furniture"
        );

        set_snapshot(&mut app, snapshot(0, home, record(law::FLOOR_1F)));
        app.update();
        assert_eq!(
            visibility(&app),
            Visibility::Inherited,
            "a home-nation 1F shows it"
        );

        set_snapshot(&mut app, upstairs(record(law::FLOOR_2F)));
        app.update();
        assert_eq!(visibility(&app), Visibility::Inherited);

        let mut not_home = upstairs(record(law::FLOOR_2F));
        not_home.login_state = ServerLoginMyroom::LOGIN_STATE_GAME;
        set_snapshot(&mut app, not_home);
        app.update();
        assert_eq!(visibility(&app), Visibility::Hidden);

        set_snapshot(&mut app, upstairs(record(law::FLOOR_2F)));
        app.update();
        let entity = piece_entity(&app, 3);
        app.world_mut()
            .get_mut::<MyroomPiece>(entity)
            .unwrap()
            .editor_hidden = true;
        app.update();
        assert_eq!(visibility(&app), Visibility::Hidden);
    }

    #[test]
    fn a_static_model_piece_asks_for_its_picked_mesh_under_its_frame() {
        let mut app = app(upstairs(vec![item(
            LOC_MOGSAFE,
            3,
            TABLE,
            attr(true, law::FLOOR_2F, pose(1, 1, 0)),
        )]));
        let pick = StaticMeshPick {
            chunk_index: 4,
            name: law::MESH_NAME_UNDERSCORE_FUN,
            rule: StaticMeshRule::UnderscoreFun,
        };
        app.world_mut()
            .resource_mut::<StaticModelPicks>()
            .insert(table_file(), Some(pick));
        app.update();
        app.update();
        let requests = mmb_requests(&mut app);
        assert_eq!(requests.len(), 1);
        let request = requests[0];
        assert_eq!((request.file_id, request.chunk_idx), (table_file(), 4));
        let target = request.static_model.expect("a static-model request");
        assert!(target.alpha_test, "a `_fun` mesh is alpha tested");
        assert_eq!(
            request.light_bindings[0],
            Some(u32::from_le_bytes(law::MOG_HOUSE_PIECE_LIGHT))
        );
        let root = piece_entity(&app, 3);
        assert_eq!(
            app.world()
                .get::<ChildOf>(target.frame)
                .map(ChildOf::parent),
            Some(root)
        );
        assert!(app.world().get::<StaticModelFrame>(target.frame).is_some());
        let model = app.world().get::<PieceModel>(root).unwrap();
        assert_eq!(model.file_id(), Some(table_file()));
        assert!(model.eases);
        app.update();
        assert!(mmb_requests(&mut app).is_empty(), "one request per piece");
    }

    #[test]
    fn npc_look_pieces_and_mannequins_become_client_actors() {
        let mut mannequin = attr(true, law::FLOOR_2F, pose(5, 5, 0));
        mannequin[law::ATTR_MANNEQUIN_RACE_BYTE] = MANNEQUIN_RACE;
        let mut app = app(upstairs(vec![
            item(
                LOC_MOGSAFE,
                3,
                NPC_PIECE,
                attr(true, law::FLOOR_2F, pose(1, 1, 0)),
            ),
            item(LOC_MOGSAFE, 4, MANNEQUIN, mannequin),
        ]));
        app.update();
        app.update();
        let requests = actor_requests(&mut app);
        assert_eq!(requests.len(), 2);
        let npc = requests
            .iter()
            .find(|r| r.entity_id == crate::scene::furniture_actor_id(3))
            .unwrap();
        assert!(matches!(
            npc.subject,
            ActorSubject::Npc { file_id, graph_size: 0 }
                if file_id == crate::look_resolver::npc_dat_id(*law::NPC_LOOK_MODELS.start())
        ));
        let dressed = requests
            .iter()
            .find(|r| r.entity_id == crate::scene::furniture_actor_id(4))
            .unwrap();
        assert!(matches!(
            &dressed.subject,
            ActorSubject::Pc { race: MANNEQUIN_RACE, equipment, .. } if equipment.is_empty()
        ));
        let tracked = app.world().resource::<TrackedEntities>();
        assert_eq!(
            tracked.by_id.get(&crate::scene::furniture_actor_id(3)),
            Some(&piece_entity(&app, 3))
        );

        set_snapshot(&mut app, upstairs(Vec::new()));
        app.update();
        assert!(app.world().resource::<TrackedEntities>().by_id.is_empty());
    }

    fn plant_of(app: &mut App, seat: usize) -> Option<(Entity, MyroomPlant)> {
        let plant = piece(app, seat).plant()?;
        Some((plant, *app.world().get::<MyroomPlant>(plant)?))
    }

    #[test]
    fn a_pot_regrows_its_plant_when_its_step_changes() {
        let pot = |step| {
            let mut a = attr(true, law::FLOOR_2F, pose(3, 3, 1));
            a[law::ATTR_FLOWERPOT_STEP_BYTE] = step;
            a[law::ATTR_FLOWERPOT_KIND_BYTE] = 1;
            vec![item(LOC_MOGSAFE, 3, POT, a)]
        };
        let mut app = app(upstairs(pot(1)));
        app.update();
        app.update();
        app.update();
        let (sprout, plant) = plant_of(&mut app, 3).expect("step 1 grows a plant");
        assert_eq!(u32::from(plant.model), SPROUT_MODEL);
        assert_eq!(
            app.world().get::<ChildOf>(sprout).map(ChildOf::parent),
            Some(piece_entity(&app, 3))
        );

        set_snapshot(&mut app, upstairs(pot(2)));
        app.update();
        app.update();
        app.update();
        let (bloom, plant) = plant_of(&mut app, 3).expect("step 2 grows a plant");
        assert_eq!(u32::from(plant.model), BLOOM_MODEL);
        assert!(
            app.world().get_entity(sprout).is_err(),
            "the old plant is gone"
        );
        assert_ne!(sprout, bloom);

        let pot_root = *app.world().get::<Transform>(piece_entity(&app, 3)).unwrap();
        let local = *app.world().get::<Transform>(bloom).unwrap();
        let world = pot_root.to_matrix() * local.to_matrix();
        let pot_point = law::piece_point(
            law::FLOOR_2F,
            &dat().properties.get(POT).unwrap(),
            pose(3, 3, 1),
            0,
        )
        .unwrap();
        let expected = piece_root_transform(law::plant_point(pot_point), law::piece_yaw(1));
        assert!(close(world.w_axis.truncate(), expected.translation));

        set_snapshot(&mut app, upstairs(pot(0)));
        app.update();
        app.update();
        assert!(plant_of(&mut app, 3).is_none(), "nothing planted, no plant");
    }

    #[test]
    fn the_piece_root_and_frame_compose_to_the_placed_model_matrix() {
        for (point, yaw) in [
            ([0.3, 0.0, 0.3], 0.0),
            ([-6.2325, -3.0, 0.0], law::piece_yaw(3)),
            ([1.5, -1.0, -7.2], law::piece_yaw(1)),
        ] {
            let composed =
                piece_root_transform(point, yaw).to_matrix() * static_frame_transform().to_matrix();
            let placed = placement_bevy_transform(
                Vec3::ONE,
                Vec3::new(0.0, yaw, 0.0),
                Vec3::from_array(point),
            );
            assert!(composed.abs_diff_eq(placed, EPSILON), "{point:?} {yaw}");
        }
    }

    #[test]
    fn floor_and_wall_pieces_stand_where_the_grid_puts_them() {
        let sandy = law::SAN_DORIA_HOME_HOUSE;
        let mut app = app(snapshot(
            0,
            sandy,
            vec![
                item(
                    LOC_MOGSAFE,
                    3,
                    TABLE,
                    attr(true, law::FLOOR_1F, pose(4, 6, 1)),
                ),
                item(
                    LOC_MOGSAFE,
                    4,
                    MIRROR,
                    attr(true, law::FLOOR_1F, pose(3, 0, 3)),
                ),
            ],
        ));
        app.update();
        app.update();
        let at = |app: &App, seat| {
            *app.world()
                .get::<Transform>(piece_entity(app, seat))
                .unwrap()
        };
        let table_point = law::piece_point(
            law::FLOOR_1F,
            &dat().properties.get(TABLE).unwrap(),
            pose(4, 6, 1),
            sandy,
        )
        .unwrap();
        let table = at(&app, 3);
        assert_eq!(table, piece_root_transform(table_point, law::piece_yaw(1)));
        let slot = law::wall_slot(law::FLOOR_1F, 3, sandy).unwrap();
        assert_eq!(
            at(&app, 4),
            piece_root_transform(slot.point, law::piece_yaw(3))
        );
    }

    #[test]
    fn a_named_mesh_eases_its_turn_and_any_other_follows_at_once() {
        let mut app = app(upstairs(vec![item(
            LOC_MOGSAFE,
            3,
            TABLE,
            attr(true, law::FLOOR_2F, pose(1, 1, 0)),
        )]));
        app.world_mut().resource_mut::<StaticModelPicks>().insert(
            table_file(),
            Some(StaticMeshPick {
                chunk_index: 1,
                name: law::MESH_NAME_FUNE,
                rule: StaticMeshRule::Fune,
            }),
        );
        app.update();
        app.update();
        app.update();
        let entity = piece_entity(&app, 3);
        app.world_mut()
            .get_mut::<MyroomPiece>(entity)
            .unwrap()
            .pose
            .rot = 1;
        app.update();
        let drawn = app
            .world()
            .get::<PieceModel>(entity)
            .unwrap()
            .drawn_yaw()
            .unwrap();
        assert!(
            drawn > 0.0 && drawn < law::piece_yaw(1),
            "eased, drawn {drawn}"
        );

        let mut instant = app_with_pick(StaticMeshRule::FirstMmb);
        let entity = piece_entity(&instant, 3);
        instant
            .world_mut()
            .get_mut::<MyroomPiece>(entity)
            .unwrap()
            .pose
            .rot = 1;
        instant.update();
        let drawn = instant
            .world()
            .get::<PieceModel>(entity)
            .unwrap()
            .drawn_yaw()
            .unwrap();
        assert!((drawn - law::piece_yaw(1)).abs() < EPSILON);
    }

    fn app_with_pick(rule: StaticMeshRule) -> App {
        let mut app = app(upstairs(vec![item(
            LOC_MOGSAFE,
            3,
            TABLE,
            attr(true, law::FLOOR_2F, pose(1, 1, 0)),
        )]));
        app.world_mut().resource_mut::<StaticModelPicks>().insert(
            table_file(),
            Some(StaticMeshPick {
                chunk_index: 1,
                name: *b"abcd",
                rule,
            }),
        );
        app.update();
        app.update();
        app.update();
        app
    }

    fn statue_npc() -> kuluu_snapshot::Entity {
        kuluu_snapshot::Entity {
            id: STATUE_NPC_ID,
            act_index: 1,
            kind: EntityKind::Npc,
            name: None,
            pos: kuluu_snapshot::Vec3::default(),
            heading: 0,
            hp_pct: None,
            bt_target_id: 0,
            face_target: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: Some(EntityLook::Standard {
                modelid: law::STATUE_NPC_MODEL,
            }),
            animation: 0,
            animationsub: 0,
            mount: None,
            status: 0,
            char_flags: default(),
            monstrosity: false,
            name_vis: None,
        }
    }

    #[test]
    fn the_statue_npc_shows_only_while_a_statue_is_placed_on_either_floor() {
        let with_statue = |installed: bool| {
            let mut snap = upstairs(vec![item(
                LOC_MOGSAFE,
                3,
                law::STATUE_ITEMS[0],
                attr(installed, law::FLOOR_1F, pose(1, 1, 0)),
            )]);
            snap.entities = vec![statue_npc()];
            snap
        };
        let mut app = app(with_statue(false));
        let npc = app
            .world_mut()
            .spawn((
                WorldEntity {
                    id: STATUE_NPC_ID,
                    act_index: 1,
                    kind: EntityKind::Npc,
                },
                Visibility::Inherited,
            ))
            .id();
        app.world_mut()
            .resource_mut::<TrackedEntities>()
            .by_id
            .insert(STATUE_NPC_ID, npc);
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(npc),
            Some(&Visibility::Hidden)
        );

        set_snapshot(&mut app, with_statue(true));
        *app.world_mut().get_mut::<Visibility>(npc).unwrap() = Visibility::Inherited;
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(npc),
            Some(&Visibility::Inherited),
            "a statue placed downstairs shows the NPC upstairs"
        );
    }

    #[test]
    fn a_piece_tint_reaches_its_static_model_materials() {
        let mut app = App::new();
        app.init_resource::<Assets<FfxiZoneMaterial>>()
            .init_resource::<ZoneMaterialTouched>()
            .add_systems(Update, apply_myroom_tints);
        let material = app
            .world_mut()
            .resource_mut::<Assets<FfxiZoneMaterial>>()
            .add(crate::dat_mmb::static_part_material(
                None,
                0,
                false,
                Default::default(),
                0,
            ));
        let root = app.world_mut().spawn(MyroomPieceTint::default()).id();
        let frame = app
            .world_mut()
            .spawn((StaticModelFrame, ChildOf(root)))
            .id();
        app.world_mut().spawn((
            StaticModelPart {
                file_id: u32::from(TABLE),
                texture_name: Default::default(),
                material: material.clone(),
                textured: false,
            },
            ChildOf(frame),
        ));
        app.update();
        let dimmed = MyroomPieceTint::from_bytes([0x60, 0x60, 0x60, 0x80]);
        assert_eq!(dimmed.0.w, 1.0, "0x80 is one");
        *app.world_mut().get_mut::<MyroomPieceTint>(root).unwrap() = dimmed;
        app.update();
        let materials = app.world().resource::<Assets<FfxiZoneMaterial>>();
        assert_eq!(materials.get(&material).unwrap().tint, dimmed.0);
    }

    #[test]
    fn a_mannequin_actor_idles_in_its_pose_family() {
        const POSE: u8 = 2;
        let mut app = App::new();
        app.add_systems(Update, drive_mannequin_idles);
        let actor = app
            .world_mut()
            .spawn(crate::ffxi_actor_render::render_actor_stub(
                crate::scene::furniture_actor_id(4),
            ))
            .id();
        let mut bytes = attr(true, law::FLOOR_2F, pose(1, 1, 0));
        bytes[law::ATTR_MANNEQUIN_POSE_BYTE] = POSE;
        let piece = MyroomPiece::new(
            LOC_MOGSAFE,
            4,
            MANNEQUIN,
            KIND_MANNEQUIN,
            &FurnitureAttr::decode(&bytes),
        );
        app.world_mut().spawn((piece, FfxiRenderRoot(actor)));
        app.update();
        assert_eq!(
            app.world()
                .get::<FfxiRenderActor>(actor)
                .unwrap()
                .idle_family(),
            Some(law::mannequin_idle(POSE, None).family)
        );
    }

    #[test]
    fn the_pure_scan_waits_for_kinds_and_drops_swapped_or_missing_seats() {
        let placed = attr(true, law::FLOOR_2F, pose(1, 1, 0));
        let record = |slot, item_no| ShelfRecord {
            container: LOC_MOGSAFE,
            slot,
            item_no,
            attr: Some(placed),
        };
        let none = BTreeMap::new();
        assert!(
            scan_shelf_records(&[record(3, TABLE)], law::FLOOR_2F, &none, &mut |_| None).is_empty()
        );
        let out_of_range = [record(0, TABLE), record(law::LAST_SHELF_SLOT + 1, TABLE)];
        assert!(
            scan_shelf_records(&out_of_range, law::FLOOR_2F, &none, &mut |_| Some(
                KIND_FURNISHING
            ))
            .is_empty()
        );

        let held = BTreeMap::from([
            (
                3,
                HeldPiece {
                    item_no: TABLE,
                    seen_installed: true,
                    flowerpot_step: 0,
                },
            ),
            (
                5,
                HeldPiece {
                    item_no: TABLE,
                    seen_installed: true,
                    flowerpot_step: 0,
                },
            ),
        ]);
        let actions = scan_shelf_records(&[record(3, MIRROR)], law::FLOOR_2F, &held, &mut |_| {
            Some(KIND_FURNISHING)
        });
        assert_eq!(
            actions,
            vec![
                ScanAction::Destroy { seat: 3 },
                ScanAction::Destroy { seat: 5 }
            ]
        );
    }
}
