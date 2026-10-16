//! Mog House furniture: the shelf record a piece comes from, the furniture DAT's property and plant
//! tables, which model a piece draws, where it stands, and when the house shows it
//! (.agents/skills/retail-observe/references/2026-10-15-mog-house-arrange.md Part A).

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::ops::{Range, RangeFrom, RangeInclusive};

use ffxi_proto::map::container::{LOC_MOGSAFE, LOC_MOGSAFE2};

use crate::datid::DatId;
use crate::sep::Sep;
use crate::texture::{QualifiedTextureName, NAMESPACE_LEN};
use crate::{walk, ChunkKind, DatRoot};

/// Bytes of item Attr an s2c 0x020 record carries
/// (vendor/server/src/map/packets/s2c/0x020_item_attr.h).
pub const ATTR_LEN: usize = 24;

/// The Attr byte holding the installed bit and the layer
/// (vendor/server/src/map/items/exdata/furniture.h Furniture).
pub const ATTR_FLAGS_BYTE: usize = 1;
/// LSB `Exdata::Furniture::Installed`: placed in the room rather than kept on the shelf.
pub const ATTR_INSTALLED_BIT: u8 = 0x40;
/// The layer bits the shelf scan compares with the current floor.
pub const ATTR_LAYER_MASK: u8 = 0b11;
/// LSB `X`, `Z`, `Y`, `Rotation`, which its accessors read back as column, level and row
/// (vendor/server/src/map/items/item_furnishing.cpp getCol, getLevel, getRow).
pub const ATTR_COL_BYTE: usize = 6;
pub const ATTR_LEVEL_BYTE: usize = 7;
pub const ATTR_ROW_BYTE: usize = 8;
pub const ATTR_ROTATION_BYTE: usize = 9;
/// vendor/server/src/map/items/exdata/flower_pot.h FlowerPot Step, Kind.
pub const ATTR_FLOWERPOT_STEP_BYTE: usize = 0;
pub const ATTR_FLOWERPOT_KIND_BYTE: usize = 4;
/// vendor/server/src/map/items/exdata/mannequin.h Mannequin Race, Pose.
pub const ATTR_MANNEQUIN_RACE_BYTE: usize = 18;
pub const ATTR_MANNEQUIN_POSE_BYTE: usize = 19;

/// Item DAT kinds ([`crate::item_dat::ItemStatic::item_type`]) the shelf scan makes pieces of.
pub const KIND_FURNISHING: u8 = 10;
pub const KIND_FLOWERPOT: u8 = 12;
pub const KIND_MANNEQUIN: u8 = 14;

pub fn is_furniture_kind(kind: u8) -> bool {
    matches!(kind, KIND_FURNISHING | KIND_FLOWERPOT | KIND_MANNEQUIN)
}

/// The shelves the scan walks, in its order.
pub const SHELVES: [u8; 2] = [LOC_MOGSAFE, LOC_MOGSAFE2];
pub const FIRST_SHELF_SLOT: u8 = 1;
pub const LAST_SHELF_SLOT: u8 = 80;
/// Seats per shelf in the piece table: a second-shelf slot sits this far past the first shelf's.
pub const SEATS_PER_SHELF: usize = 81;

pub fn is_shelf_slot(slot: u8) -> bool {
    (FIRST_SHELF_SLOT..=LAST_SHELF_SLOT).contains(&slot)
}

/// A shelf slot's seat: one stable index per piece across both shelves, ordered first shelf then second.
pub fn seat_index(container: u8, slot: u8) -> usize {
    let base = if container == LOC_MOGSAFE2 {
        SEATS_PER_SHELF
    } else {
        0
    };
    base + usize::from(slot)
}

const SUB_MAP_FLOOR_SHIFT: u32 = 1;

/// The Mog House floor (0 = 1F, 1 = 2F) from s2c 0x00A `MyroomSubMapNumber`, which LSB sends as 2 upstairs
/// (vendor/server/src/map/packets/s2c/0x00a_login.cpp).
pub fn floor_of_sub_map(sub_map_number: u8) -> u8 {
    (sub_map_number >> SUB_MAP_FLOOR_SHIFT) & ATTR_LAYER_MASK
}

pub const FLOOR_1F: u8 = 0;
pub const FLOOR_2F: u8 = 1;

/// The Mog Garden, where the shelf scan does not run (vendor/server/data/enums/zone.yaml mog_garden).
pub const MOG_GARDEN_ZONE: u16 = 280;

/// A piece's place in the room as the Attr stores it: grid column (or wall slot), stack level, grid row,
/// quarter turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FurniturePose {
    pub col: i8,
    pub level: i8,
    pub row: i8,
    pub rot: i8,
}

/// The Attr fields a placed piece is drawn from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FurnitureAttr {
    pub installed: bool,
    pub layer: u8,
    pub pose: FurniturePose,
    pub flowerpot_step: u8,
    pub flowerpot_kind: u8,
    pub mannequin_race: u8,
    pub mannequin_pose: u8,
}

impl FurnitureAttr {
    pub fn decode(attr: &[u8; ATTR_LEN]) -> Self {
        Self {
            installed: attr[ATTR_FLAGS_BYTE] & ATTR_INSTALLED_BIT != 0,
            layer: attr[ATTR_FLAGS_BYTE] & ATTR_LAYER_MASK,
            pose: FurniturePose {
                col: attr[ATTR_COL_BYTE] as i8,
                level: attr[ATTR_LEVEL_BYTE] as i8,
                row: attr[ATTR_ROW_BYTE] as i8,
                rot: attr[ATTR_ROTATION_BYTE] as i8,
            },
            flowerpot_step: attr[ATTR_FLOWERPOT_STEP_BYTE],
            flowerpot_kind: attr[ATTR_FLOWERPOT_KIND_BYTE],
            mannequin_race: attr[ATTR_MANNEQUIN_RACE_BYTE],
            mannequin_pose: attr[ATTR_MANNEQUIN_POSE_BYTE],
        }
    }
}

pub const FURNITURE_DAT_FILE_ID: u32 = 32922;
pub const PROPERTY_CHUNK_KIND: u8 = 0x59;
pub const PLANT_CHUNK_KIND: u8 = 0x5B;
pub const PROPERTY_RECORD_LEN: usize = 8;

/// The furniture DAT's sound chunks (kind [`ChunkKind::Sep`]): a step that moved, a floor-piece
/// rotation, and an accepted placement.
pub const STEP_SOUND_NAME: [u8; 4] = *b"8058";
pub const ROTATE_SOUND_NAME: [u8; 4] = *b"8059";
pub const PLACED_SOUND_NAME: [u8; 4] = *b"8060";

/// Item ids from here up read the property record [`REMAPPED_ITEM_ID_SHIFT`] lower.
const REMAPPED_ITEM_IDS: RangeFrom<u16> = 0xE00..;
const REMAPPED_ITEM_ID_SHIFT: u16 = 0xC00;

const PROPERTY_FLAGS: usize = 0;
const PROPERTY_MODEL: Range<usize> = 2..4;
const PROPERTY_WIDTH: usize = 4;
const PROPERTY_DEPTH: usize = 5;
const PROPERTY_HEIGHT: Range<usize> = 6..8;

pub const FLAG_WALL_HUNG: u8 = 1 << 0;
pub const FLAG_SURFACE: u8 = 1 << 1;
pub const FLAG_STACKABLE: u8 = 1 << 2;
pub const FLAG_FLOWERPOT: u8 = 1 << 3;

/// Hundredths of a world unit per stack level: a piece standing on a surface sits at height / 10.
pub const HEIGHT_PER_LEVEL: u16 = 10;

pub fn property_record_index(item_id: u16) -> usize {
    if REMAPPED_ITEM_IDS.contains(&item_id) {
        usize::from(item_id - REMAPPED_ITEM_ID_SHIFT)
    } else {
        usize::from(item_id)
    }
}

/// One 8-byte furniture property record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FurnitureProperty {
    pub flags: u8,
    pub model: u16,
    /// Cells along x at rot 0.
    pub width: u8,
    /// Cells along z at rot 0.
    pub depth: u8,
    /// Surface top in hundredths of a world unit.
    pub height: u16,
}

impl FurnitureProperty {
    pub fn from_record(record: &[u8]) -> Option<Self> {
        let record = record.get(..PROPERTY_RECORD_LEN)?;
        Some(Self {
            flags: record[PROPERTY_FLAGS],
            model: u16::from_le_bytes(record[PROPERTY_MODEL].try_into().ok()?),
            width: record[PROPERTY_WIDTH],
            depth: record[PROPERTY_DEPTH],
            height: u16::from_le_bytes(record[PROPERTY_HEIGHT].try_into().ok()?),
        })
    }

    pub fn to_record(&self) -> [u8; PROPERTY_RECORD_LEN] {
        let mut record = [0u8; PROPERTY_RECORD_LEN];
        record[PROPERTY_FLAGS] = self.flags;
        record[PROPERTY_MODEL].copy_from_slice(&self.model.to_le_bytes());
        record[PROPERTY_WIDTH] = self.width;
        record[PROPERTY_DEPTH] = self.depth;
        record[PROPERTY_HEIGHT].copy_from_slice(&self.height.to_le_bytes());
        record
    }

    pub fn wall_hung(&self) -> bool {
        self.flags & FLAG_WALL_HUNG != 0
    }

    pub fn surface(&self) -> bool {
        self.flags & FLAG_SURFACE != 0
    }

    pub fn stackable(&self) -> bool {
        self.flags & FLAG_STACKABLE != 0
    }

    pub fn flowerpot(&self) -> bool {
        self.flags & FLAG_FLOWERPOT != 0
    }

    /// (width, depth) after `rot` quarter turns: an odd turn swaps them.
    pub fn footprint(&self, rot: i8) -> (u8, u8) {
        if rot & 1 != 0 {
            (self.depth, self.width)
        } else {
            (self.width, self.depth)
        }
    }

    /// The level a piece standing on this surface takes.
    pub fn surface_level(&self) -> u16 {
        self.height / HEIGHT_PER_LEVEL
    }
}

/// The property table: the payload of the furniture DAT's first property chunk.
#[derive(Debug, Clone, Default)]
pub struct PropertyTable {
    payload: Vec<u8>,
}

impl PropertyTable {
    pub fn from_payload(payload: &[u8]) -> Self {
        Self {
            payload: payload.to_vec(),
        }
    }

    pub fn get(&self, item_id: u16) -> Option<FurnitureProperty> {
        let start = property_record_index(item_id).checked_mul(PROPERTY_RECORD_LEN)?;
        FurnitureProperty::from_record(self.payload.get(start..)?)
    }

    /// A table holding exactly these items' records, every other record zero.
    pub fn from_items(items: &[(u16, FurnitureProperty)]) -> Self {
        let len = items
            .iter()
            .map(|(item_id, _)| property_record_index(*item_id) + 1)
            .max()
            .unwrap_or(0);
        let mut payload = vec![0u8; len * PROPERTY_RECORD_LEN];
        for (item_id, property) in items {
            let at = property_record_index(*item_id) * PROPERTY_RECORD_LEN;
            payload[at..at + PROPERTY_RECORD_LEN].copy_from_slice(&property.to_record());
        }
        Self { payload }
    }
}

/// Plant table value for "this step grows no model".
pub const NO_PLANT_MODEL: u32 = 999;

const WORD_LEN: usize = 4;
const PLANT_ROW_OFFSETS_AT: usize = WORD_LEN;
const PLANT_ROW_VALUES_AT: usize = WORD_LEN;

/// The plant table: the payload of the furniture DAT's first plant chunk.
#[derive(Debug, Clone, Default)]
pub struct PlantTable {
    payload: Vec<u8>,
}

impl PlantTable {
    pub fn from_payload(payload: &[u8]) -> Self {
        Self {
            payload: payload.to_vec(),
        }
    }

    /// A table laid out the way [`Self::value`] reads it: a row count word, one offset word per row, and each
    /// row a word of its own followed by its values.
    pub fn from_rows(rows: &[&[u32]]) -> Self {
        let word = |value: usize| u32::try_from(value).unwrap_or(u32::MAX).to_le_bytes();
        let header = PLANT_ROW_OFFSETS_AT + WORD_LEN * rows.len();
        let mut offsets = Vec::new();
        let mut body = Vec::new();
        for row in rows {
            offsets.extend_from_slice(&word(header + body.len()));
            body.extend_from_slice(&word(row.len()));
            for value in *row {
                body.extend_from_slice(&value.to_le_bytes());
            }
        }
        let mut payload = word(rows.len()).to_vec();
        payload.extend_from_slice(&offsets);
        payload.extend_from_slice(&body);
        Self { payload }
    }

    fn word(&self, at: usize) -> Option<u32> {
        let bytes = self.payload.get(at..at.checked_add(WORD_LEN)?)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    }

    pub fn value(&self, row: usize, column: usize) -> Option<u32> {
        let row_offset =
            self.word(PLANT_ROW_OFFSETS_AT.checked_add(WORD_LEN.checked_mul(row)?)?)?;
        let at = usize::try_from(row_offset)
            .ok()?
            .checked_add(PLANT_ROW_VALUES_AT)?
            .checked_add(WORD_LEN.checked_mul(column)?)?;
        self.word(at)
    }

    /// The model number a pot grows at this Attr step and kind; `None` while nothing is planted or the
    /// table names no model.
    pub fn plant_model(&self, flowerpot_kind: u8, flowerpot_step: u8) -> Option<u16> {
        if flowerpot_step == 0 {
            return None;
        }
        let row = usize::from(flowerpot_kind.saturating_sub(1));
        let value = self.value(row, usize::from(flowerpot_step))?;
        if value == NO_PLANT_MODEL {
            return None;
        }
        u16::try_from(value).ok()
    }
}

/// The sound ids of the furniture DAT's three sound chunks; `None` for a chunk the file lacks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FurnitureSounds {
    pub step: Option<u32>,
    pub rotate: Option<u32>,
    pub placed: Option<u32>,
}

/// The two furniture tables of DAT [`FURNITURE_DAT_FILE_ID`] and its sound chunks.
#[derive(Debug, Clone, Default)]
pub struct FurnitureDat {
    pub properties: PropertyTable,
    pub plants: Option<PlantTable>,
    pub sounds: FurnitureSounds,
}

impl FurnitureDat {
    /// `None` when the file has no property chunk.
    pub fn parse(file: &[u8]) -> Option<Self> {
        let mut properties = None;
        let mut plants = None;
        let mut sounds = FurnitureSounds::default();
        for chunk in walk(file).filter_map(Result::ok) {
            if chunk.kind == PROPERTY_CHUNK_KIND && properties.is_none() {
                properties = Some(PropertyTable::from_payload(chunk.data));
            } else if chunk.kind == PLANT_CHUNK_KIND && plants.is_none() {
                plants = Some(PlantTable::from_payload(chunk.data));
            } else if ChunkKind::from_u8(chunk.kind) == Some(ChunkKind::Sep) {
                let slot = match chunk.name {
                    STEP_SOUND_NAME => &mut sounds.step,
                    ROTATE_SOUND_NAME => &mut sounds.rotate,
                    PLACED_SOUND_NAME => &mut sounds.placed,
                    _ => continue,
                };
                if slot.is_none() {
                    *slot = Sep::parse(chunk.name, chunk.data).ok().map(|sep| sep.se_id);
                }
            }
        }
        Some(Self {
            properties: properties?,
            plants,
            sounds,
        })
    }

    pub fn open(root: &DatRoot) -> Option<Self> {
        let location = root.resolve(FURNITURE_DAT_FILE_ID).ok()?;
        Self::parse(&std::fs::read(location.path_under(root)).ok()?)
    }
}

/// Model numbers drawn as a client-made NPC with the standard look of that model id instead of a file.
pub const NPC_LOOK_MODELS: RangeInclusive<u16> = 264..=299;

const LOW_MODELS: Range<u16> = 0..0x100;
const LOW_MODEL_FILE_BASE: u32 = 0x809F;
const MID_MODELS: Range<u16> = 0x100..0x200;
const MID_MODEL_FILE_BASE: u32 = 0xC7EF;
const HIGH_MODELS: Range<u16> = 0xE00..0x1000;
const HIGH_MODEL_FILE_BASE: u32 = 0xE75B;

/// What a property-table model number draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FurnitureModel {
    /// One static model file, by file id.
    File(u32),
    /// An NPC with `EntityLook::Standard { modelid }`.
    NpcLook(u16),
    /// No model.
    Absent,
}

pub fn furniture_model(model: u16) -> FurnitureModel {
    if NPC_LOOK_MODELS.contains(&model) {
        return FurnitureModel::NpcLook(model);
    }
    let file = |base: u32| FurnitureModel::File(base + u32::from(model));
    if LOW_MODELS.contains(&model) {
        file(LOW_MODEL_FILE_BASE)
    } else if MID_MODELS.contains(&model) {
        file(MID_MODEL_FILE_BASE)
    } else if HIGH_MODELS.contains(&model) {
        file(HIGH_MODEL_FILE_BASE)
    } else if model >= HIGH_MODELS.end {
        FurnitureModel::File(LOW_MODEL_FILE_BASE)
    } else {
        FurnitureModel::Absent
    }
}

pub const MESH_NAME_FUNE: [u8; 4] = *b"fune";
pub const MESH_NAME_UNDERSCORE_FUN: [u8; 4] = *b"_fun";
const CUTOUT_NAME_PREFIX: u8 = b'_';

/// Which rule chose a static model's one mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticMeshRule {
    /// The first MMB chunk named `fune`.
    Fune,
    /// No `fune`: the first named `_fun`.
    UnderscoreFun,
    /// Neither: the first MMB chunk of any name.
    FirstMmb,
}

/// The one MMB a static model file draws. `chunk_index` counts every chunk the file walk yields, the
/// index the MMB loader takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticMeshPick {
    pub chunk_index: usize,
    pub name: [u8; 4],
    pub rule: StaticMeshRule,
}

impl StaticMeshPick {
    /// The whole piece is alpha tested against [`ALPHA_TEST_REF`].
    pub fn alpha_tested(&self) -> bool {
        match self.rule {
            StaticMeshRule::Fune => false,
            StaticMeshRule::UnderscoreFun => true,
            StaticMeshRule::FirstMmb => self.name[0] == CUTOUT_NAME_PREFIX,
        }
    }

    /// The drawn rotation eases toward the piece's rotation instead of following it at once.
    pub fn eases_rotation(&self) -> bool {
        matches!(
            self.rule,
            StaticMeshRule::Fune | StaticMeshRule::UnderscoreFun
        )
    }

    /// Only a mesh picked by the last rule has its file's schedulers started (which ones is untraced).
    pub fn starts_schedulers(&self) -> bool {
        self.rule == StaticMeshRule::FirstMmb
    }
}

pub fn pick_static_mesh(file: &[u8]) -> Option<StaticMeshPick> {
    let mut fune = None;
    let mut underscore_fun = None;
    let mut first = None;
    for (chunk_index, chunk) in walk(file).filter_map(Result::ok).enumerate() {
        if ChunkKind::from_u8(chunk.kind) != Some(ChunkKind::Mmb) {
            continue;
        }
        let pick = |rule| StaticMeshPick {
            chunk_index,
            name: chunk.name,
            rule,
        };
        if fune.is_none() && chunk.name == MESH_NAME_FUNE {
            fune = Some(pick(StaticMeshRule::Fune));
        }
        if underscore_fun.is_none() && chunk.name == MESH_NAME_UNDERSCORE_FUN {
            underscore_fun = Some(pick(StaticMeshRule::UnderscoreFun));
        }
        if first.is_none() {
            first = Some(pick(StaticMeshRule::FirstMmb));
        }
    }
    fune.or(underscore_fun).or(first)
}

/// A pixel of an alpha-tested piece is kept while its final alpha byte is above this.
pub const ALPHA_TEST_REF: u8 = 0x60;

/// The texture a static-model part binds: the file's own IMG with the same 16 name bytes, else any loaded
/// texture with the same 16, else any loaded texture whose last 8 match; `None` draws the part untextured.
/// `loaded` is searched in the order given.
pub fn match_part_texture<T: Clone>(
    part: &QualifiedTextureName,
    own: &[(QualifiedTextureName, T)],
    loaded: &[(QualifiedTextureName, T)],
) -> Option<T> {
    let local = &part[NAMESPACE_LEN..];
    own.iter()
        .find(|(name, _)| name == part)
        .or_else(|| loaded.iter().find(|(name, _)| name == part))
        .or_else(|| {
            loaded
                .iter()
                .find(|(name, _)| &name[NAMESPACE_LEN..] == local)
        })
        .map(|(_, texture)| texture.clone())
}

/// Each frame tick the drawn rotation closes this share of what is left.
const ROTATION_EASE_SHARE: f32 = 1.0 / 8.0;

/// `angle` wrapped into [-π, π).
pub fn wrap_angle(angle: f32) -> f32 {
    (angle + PI).rem_euclid(TAU) - PI
}

/// Frame ticks an update steps: its elapsed ticks rounded up, at least one.
pub fn update_ticks(elapsed_ticks: f32) -> i32 {
    elapsed_ticks.ceil().max(1.0) as i32
}

/// The drawn yaw after `ticks` frame ticks of easing toward `target`, the difference wrapped to ±π.
pub fn ease_yaw(drawn: f32, target: f32, ticks: i32) -> f32 {
    let remaining = wrap_angle(target - drawn) * (1.0 - ROTATION_EASE_SHARE).powi(ticks);
    wrap_angle(target - remaining)
}

/// The yaw of a piece turned `rot` quarter turns.
pub fn piece_yaw(rot: i8) -> f32 {
    f32::from(rot) * FRAC_PI_2
}

/// World units per floor cell.
pub const CELL: f32 = 0.6;
pub const HALF_CELL: f32 = CELL / 2.0;
/// World units per stack level.
pub const LEVEL_STEP: f32 = 0.1;
/// Floor columns on either floor.
pub const FLOOR_COLUMNS: i8 = 20;

/// One floor's grid: the cell the room origin falls in and the row count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FloorGrid {
    pub origin_col: i8,
    pub origin_row: i8,
    pub rows: i8,
}

pub const GRID_1F: FloorGrid = FloorGrid {
    origin_col: 10,
    origin_row: 12,
    rows: 24,
};

pub const GRID_2F: FloorGrid = FloorGrid {
    origin_col: 10,
    origin_row: 13,
    rows: 26,
};

pub fn floor_grid(floor: u8) -> Option<FloorGrid> {
    match floor {
        FLOOR_1F => Some(GRID_1F),
        FLOOR_2F => Some(GRID_2F),
        _ => None,
    }
}

/// A floor piece's model origin in FFXI world coordinates (y down): the centre of its rotated footprint,
/// whose minimum corner is (col, row).
pub fn floor_piece_point(grid: FloorGrid, pose: FurniturePose, footprint: (u8, u8)) -> [f32; 3] {
    let (width, depth) = footprint;
    [
        f32::from(i16::from(pose.col) - i16::from(grid.origin_col)) * CELL
            + f32::from(width) * HALF_CELL,
        -f32::from(pose.level) * LEVEL_STEP,
        f32::from(i16::from(pose.row) - i16::from(grid.origin_row)) * CELL
            + f32::from(depth) * HALF_CELL,
    ]
}

/// A wall slot's point and the rotation a piece stepped into it takes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WallSlot {
    pub point: [f32; 3],
    pub rotation: i8,
}

/// s2c 0x00A `MyroomMapNumber` of LSB's San d'Oria home house, whose 1F walls stand closer in.
pub const SAN_DORIA_HOME_HOUSE: u16 = 289;
/// LSB's three home-nation houses (vendor/server/src/map/packets/s2c/0x00a_login.cpp GetMogHouseModelID).
pub const HOME_NATION_HOUSES: RangeInclusive<u16> = SAN_DORIA_HOME_HOUSE..=291;

const WALL_Y_1F: f32 = -3.0;
const WALL_X_1F: f32 = 6.5;
const WALL_X_1F_SAN_DORIA: f32 = 6.2325;
/// (x sign, z, slot rotation) per 1F slot.
const WALL_SLOTS_1F: [(f32, f32, i8); 5] = [
    (1.0, 4.55, 1),
    (1.0, -4.8, 1),
    (-1.0, -4.8, 3),
    (-1.0, 0.0, 3),
    (-1.0, 4.55, 3),
];

/// 2F slot heights, h 0..3.
const WALL_HEIGHTS_2F: [f32; 4] = [-5.6, -3.2, -2.5, -1.8];
const SIDE_WALL_COLUMNS_2F: [f32; 9] = [-6.2, -5.15, -4.1, -1.0, 0.0, 1.0, 4.1, 5.15, 6.2];
const NORTH_WALL_COLUMNS_2F: [f32; 3] = [-4.0, 0.0, 4.0];
const SOUTH_WALL_COLUMNS_2F: [f32; 2] = [-4.0, 4.0];

/// The four walls of the 2F room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wall2F {
    West,
    North,
    East,
    South,
}

/// One 2F wall: its first slot, the column positions along it, where it stands across, and the
/// rotation its slots give.
struct WallRun {
    wall: Wall2F,
    base: u8,
    columns: &'static [f32],
    across: f32,
    columns_run_along_x: bool,
    rotation: i8,
}

const WALLS_2F: [WallRun; 4] = [
    WallRun {
        wall: Wall2F::West,
        base: 0,
        columns: &SIDE_WALL_COLUMNS_2F,
        across: -6.2,
        columns_run_along_x: false,
        rotation: 3,
    },
    WallRun {
        wall: Wall2F::North,
        base: 36,
        columns: &NORTH_WALL_COLUMNS_2F,
        across: 8.0,
        columns_run_along_x: true,
        rotation: 0,
    },
    WallRun {
        wall: Wall2F::East,
        base: 48,
        columns: &SIDE_WALL_COLUMNS_2F,
        across: 6.2,
        columns_run_along_x: false,
        rotation: 1,
    },
    WallRun {
        wall: Wall2F::South,
        base: 84,
        columns: &SOUTH_WALL_COLUMNS_2F,
        across: -8.0,
        columns_run_along_x: true,
        rotation: 2,
    },
];

/// Height steps per 2F wall column.
pub const WALL_HEIGHTS_2F_COUNT: u8 = WALL_HEIGHTS_2F.len() as u8;

impl Wall2F {
    fn run(self) -> &'static WallRun {
        WALLS_2F
            .iter()
            .find(|run| run.wall == self)
            .unwrap_or(&WALLS_2F[0])
    }

    pub fn columns(self) -> u8 {
        self.run().columns.len() as u8
    }

    pub fn last_column(self) -> u8 {
        self.columns() - 1
    }
}

/// A 2F wall slot as the wall it hangs on, its column along that wall and its height step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WallPlace2F {
    pub wall: Wall2F,
    pub column: u8,
    pub height: u8,
}

/// Slot = wall base + heights per column × column + height.
pub fn wall_place_2f(slot: i8) -> Option<WallPlace2F> {
    let slot = u8::try_from(slot).ok()?;
    WALLS_2F.iter().find_map(|run| {
        let offset = slot.checked_sub(run.base)?;
        let column = offset / WALL_HEIGHTS_2F_COUNT;
        (usize::from(column) < run.columns.len()).then_some(WallPlace2F {
            wall: run.wall,
            column,
            height: offset % WALL_HEIGHTS_2F_COUNT,
        })
    })
}

pub fn wall_slot_2f(place: WallPlace2F) -> i8 {
    let slot = place.wall.run().base + WALL_HEIGHTS_2F_COUNT * place.column + place.height;
    slot as i8
}

/// Whether the wall a slot hangs on runs along x (its slots differ in x) rather than along z; the
/// room interior lies off it along the other axis.
pub fn wall_runs_along_x(floor: u8, slot: i8) -> Option<bool> {
    match floor {
        FLOOR_1F => usize::try_from(slot)
            .ok()
            .filter(|&slot| slot < WALL_SLOTS_1F.len())
            .map(|_| false),
        FLOOR_2F => wall_place_2f(slot).map(|place| place.wall.run().columns_run_along_x),
        _ => None,
    }
}

pub const WALL_SLOTS_1F_COUNT: usize = WALL_SLOTS_1F.len();
pub const WALL_SLOTS_2F_COUNT: usize =
    (2 * SIDE_WALL_COLUMNS_2F.len() + NORTH_WALL_COLUMNS_2F.len() + SOUTH_WALL_COLUMNS_2F.len())
        * WALL_HEIGHTS_2F.len();

pub fn wall_slot(floor: u8, slot: i8, myroom_map_number: u16) -> Option<WallSlot> {
    let slot = u8::try_from(slot).ok()?;
    match floor {
        FLOOR_1F => {
            let (sign, z, rotation) = *WALL_SLOTS_1F.get(usize::from(slot))?;
            let wall_x = if myroom_map_number == SAN_DORIA_HOME_HOUSE {
                WALL_X_1F_SAN_DORIA
            } else {
                WALL_X_1F
            };
            Some(WallSlot {
                point: [sign * wall_x, WALL_Y_1F, z],
                rotation,
            })
        }
        FLOOR_2F => {
            let heights = WALL_HEIGHTS_2F.len();
            WALLS_2F.iter().find_map(|wall| {
                let offset = usize::from(slot.checked_sub(wall.base)?);
                let along = *wall.columns.get(offset / heights)?;
                let y = WALL_HEIGHTS_2F[offset % heights];
                let point = if wall.columns_run_along_x {
                    [along, y, wall.across]
                } else {
                    [wall.across, y, along]
                };
                Some(WallSlot {
                    point,
                    rotation: wall.rotation,
                })
            })
        }
        _ => None,
    }
}

/// A placed piece's model origin in FFXI world coordinates (y down); `None` off the two floors or for a
/// wall slot that does not exist.
pub fn piece_point(
    floor: u8,
    property: &FurnitureProperty,
    pose: FurniturePose,
    myroom_map_number: u16,
) -> Option<[f32; 3]> {
    if property.wall_hung() {
        wall_slot(floor, pose.col, myroom_map_number).map(|slot| slot.point)
    } else {
        floor_grid(floor).map(|grid| floor_piece_point(grid, pose, property.footprint(pose.rot)))
    }
}

/// How far above its pot a plant model stands.
pub const PLANT_RAISE: f32 = 0.3;

pub fn plant_point(pot: [f32; 3]) -> [f32; 3] {
    [pot[0], pot[1] - PLANT_RAISE, pot[2]]
}

/// Whether this house draws its furniture (and lets it be arranged): 2F, or 1F of a home-nation house,
/// while the character is in a Mog House at all.
pub fn house_is_arrangeable(floor: u8, myroom_map_number: u16, in_mog_house: bool) -> bool {
    in_mog_house
        && (floor == FLOOR_2F
            || (floor == FLOOR_1F && HOME_NATION_HOUSES.contains(&myroom_map_number)))
}

/// Room NPCs with this standard-look model id show only while a [`STATUE_ITEMS`] piece is placed on either
/// floor of an arrangeable house.
pub const STATUE_NPC_MODEL: u16 = 1847;
/// LSB's Nanaa Mihgo statue, statue II and Orchestrion.
pub const STATUE_ITEMS: [u16; 3] = [286, 287, 426];

pub fn statue_npcs_shown(arrangeable: bool, statue_placed: bool) -> bool {
    arrangeable && statue_placed
}

const MANNEQUIN_FAMILY: [u8; 2] = *b"mq";
const MOTION_FAMILY_WILDCARD: u8 = b'?';
const MANNEQUIN_POSE_DIGIT_MASK: u8 = 0b111;
const MANNEQUIN_POSE_VARIANT_SHIFT: u32 = 3;
/// The pose whose digit comes from a dressed item instead of the pose byte.
pub const MANNEQUIN_DRESSED_POSE: u8 = 8;
/// That digit when no dressed item supplies one.
pub const MANNEQUIN_DRESSED_DIGIT_DEFAULT: u8 = 6;
pub const MANNEQUIN_DRESSED_VARIANT: u8 = 1;

/// A mannequin's idle motion: the `mq<digit>?` family and its variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MannequinIdle {
    pub family: DatId,
    pub variant: u8,
}

/// `dressed_digit` is the item-info byte pose 8 reads from a dressed item, which nothing supplies yet.
pub fn mannequin_idle(pose: u8, dressed_digit: Option<u8>) -> MannequinIdle {
    let (digit, variant) = if pose == MANNEQUIN_DRESSED_POSE {
        (
            dressed_digit.unwrap_or(MANNEQUIN_DRESSED_DIGIT_DEFAULT),
            MANNEQUIN_DRESSED_VARIANT,
        )
    } else {
        (
            pose & MANNEQUIN_POSE_DIGIT_MASK,
            pose >> MANNEQUIN_POSE_VARIANT_SHIFT,
        )
    };
    MannequinIdle {
        family: DatId([
            MANNEQUIN_FAMILY[0],
            MANNEQUIN_FAMILY[1],
            b'0'.wrapping_add(digit),
            MOTION_FAMILY_WILDCARD,
        ]),
        variant,
    }
}

/// The scene light a Mog House switches on for every piece.
pub const MOG_HOUSE_PIECE_LIGHT: [u8; 4] = *b"l_01";

#[cfg(test)]
mod tests {
    use super::*;

    const CHUNK_HEADER_LEN: usize = 16;
    const CHUNK_SIZE_SHIFT: u32 = 7;
    const DIR_KIND: u8 = 0x01;
    const DIR_END_KIND: u8 = 0x00;
    const EPSILON: f32 = 1e-5;

    const FURNITURE_DAT_FILE_ID_PINNED: u32 = 0x809A;
    const LOW_MODEL_FILE_BASE_PINNED: u32 = 0x809F;
    const MID_MODEL_FILE_BASE_PINNED: u32 = 0xC7EF;
    const HIGH_MODEL_FILE_BASE_PINNED: u32 = 0xE75B;
    const REMAPPED_ITEM_ID_PINNED: u16 = 0xE00;
    const REMAPPED_ITEM_ID_SHIFT_PINNED: u16 = 0xC00;
    const HIGH_MODELS_START_PINNED: u16 = 0xE00;
    const MODEL_NUMBER_TOP_PINNED: u16 = 0x1000;

    fn chunk(name: &[u8; 4], kind: u8, body: &[u8]) -> Vec<u8> {
        let mut body = body.to_vec();
        body.resize(body.len().next_multiple_of(CHUNK_HEADER_LEN), 0);
        let total = CHUNK_HEADER_LEN + body.len();
        let mut out = name.to_vec();
        out.extend_from_slice(
            &(u32::from(kind) | (((total / CHUNK_HEADER_LEN) as u32) << CHUNK_SIZE_SHIFT))
                .to_le_bytes(),
        );
        out.extend_from_slice(&[0u8; 8]);
        out.extend_from_slice(&body);
        out
    }

    fn dir(name: &[u8; 4], children: &[u8]) -> Vec<u8> {
        let mut out = chunk(name, DIR_KIND, &[]);
        out.extend_from_slice(children);
        out.extend_from_slice(&chunk(b"end ", DIR_END_KIND, &[]));
        out
    }

    fn record(flags: u8, model: u16, width: u8, depth: u8, height: u16) -> [u8; 8] {
        FurnitureProperty {
            flags,
            model,
            width,
            depth,
            height,
        }
        .to_record()
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < EPSILON)
    }

    #[test]
    fn the_record_values_are_pinned() {
        assert_eq!(FURNITURE_DAT_FILE_ID, FURNITURE_DAT_FILE_ID_PINNED);
        assert_eq!(LOW_MODEL_FILE_BASE, LOW_MODEL_FILE_BASE_PINNED);
        assert_eq!(MID_MODEL_FILE_BASE, MID_MODEL_FILE_BASE_PINNED);
        assert_eq!(HIGH_MODEL_FILE_BASE, HIGH_MODEL_FILE_BASE_PINNED);
        assert_eq!(REMAPPED_ITEM_IDS.start, REMAPPED_ITEM_ID_PINNED);
        assert_eq!(REMAPPED_ITEM_ID_SHIFT, REMAPPED_ITEM_ID_SHIFT_PINNED);
        assert_eq!(HIGH_MODELS.start, HIGH_MODELS_START_PINNED);
        assert_eq!(HIGH_MODELS.end, MODEL_NUMBER_TOP_PINNED);
        assert_eq!(WALL_SLOTS_2F_COUNT, 92);
    }

    #[test]
    fn property_records_are_indexed_by_item_id_with_the_high_ids_shifted_down() {
        let below = REMAPPED_ITEM_IDS.start - 1;
        assert_eq!(property_record_index(below), usize::from(below));
        let high = REMAPPED_ITEM_IDS.start;
        assert_eq!(
            property_record_index(high),
            usize::from(high - REMAPPED_ITEM_ID_SHIFT)
        );

        let direct = 3u16;
        let shifted = property_record_index(high);
        let mut payload = vec![0u8; PROPERTY_RECORD_LEN * (shifted + 1)];
        let wardrobe = record(FLAG_SURFACE | FLAG_STACKABLE, 0x12, 2, 1, 105);
        let at = PROPERTY_RECORD_LEN * usize::from(direct);
        payload[at..at + PROPERTY_RECORD_LEN].copy_from_slice(&wardrobe);
        let at = PROPERTY_RECORD_LEN * shifted;
        payload[at..at + PROPERTY_RECORD_LEN].copy_from_slice(&record(
            FLAG_WALL_HUNG,
            0x300,
            1,
            1,
            0,
        ));
        let table = PropertyTable::from_payload(&payload);

        let p = table.get(direct).unwrap();
        assert_eq!((p.model, p.width, p.depth, p.height), (0x12, 2, 1, 105));
        assert!(p.surface() && p.stackable() && !p.wall_hung() && !p.flowerpot());
        assert_eq!(p.surface_level(), 10, "height / 10, truncated");
        assert!(table.get(high).unwrap().wall_hung());
        let past = u16::try_from(shifted + 1).unwrap();
        assert_eq!(table.get(past), None, "past the payload");
    }

    #[test]
    fn a_table_built_from_items_reads_back_through_the_record_index() {
        let low = FurnitureProperty::from_record(&record(FLAG_FLOWERPOT, 9, 1, 1, 30)).unwrap();
        let high = FurnitureProperty::from_record(&record(FLAG_WALL_HUNG, 4, 2, 1, 0)).unwrap();
        let table = PropertyTable::from_items(&[(5, low), (REMAPPED_ITEM_IDS.start, high)]);
        assert_eq!(table.get(5), Some(low));
        assert_eq!(table.get(REMAPPED_ITEM_IDS.start), Some(high));
        assert_eq!(table.get(6).map(|p| p.model), Some(0));
    }

    #[test]
    fn an_odd_turn_swaps_the_footprint() {
        let p = FurnitureProperty::from_record(&record(0, 0, 3, 1, 0)).unwrap();
        assert_eq!(p.footprint(0), (3, 1));
        assert_eq!(p.footprint(1), (1, 3));
        assert_eq!(p.footprint(2), (3, 1));
        assert_eq!(p.footprint(3), (1, 3));
        assert_eq!(p.footprint(-1), (1, 3));
    }

    #[test]
    fn the_plant_table_reads_row_kind_minus_one_column_step() {
        let table = PlantTable::from_rows(&[
            &[NO_PLANT_MODEL, 40, 41, NO_PLANT_MODEL],
            &[NO_PLANT_MODEL, 50, 51, 52],
        ]);
        assert_eq!(table.value(1, 3), Some(52));
        assert_eq!(table.plant_model(1, 1), Some(40));
        assert_eq!(table.plant_model(2, 2), Some(51));
        assert_eq!(table.plant_model(0, 2), Some(41), "kind 0 clamps to row 0");
        assert_eq!(table.plant_model(1, 3), None, "999 grows no model");
        assert_eq!(table.plant_model(2, 0), None, "nothing planted");
        assert_eq!(table.plant_model(2, 9), None, "past the row");
    }

    #[test]
    fn the_dat_takes_the_first_property_and_plant_chunks() {
        let mut first = vec![0u8; PROPERTY_RECORD_LEN * 2];
        first[PROPERTY_RECORD_LEN..].copy_from_slice(&record(0, 7, 1, 1, 0));
        let mut second = first.clone();
        second[PROPERTY_RECORD_LEN..].copy_from_slice(&record(0, 9, 1, 1, 0));
        let mut file = chunk(b"furn", PROPERTY_CHUNK_KIND, &first);
        file.extend(chunk(b"furn", PROPERTY_CHUNK_KIND, &second));
        file.extend(chunk(
            b"plnt",
            PLANT_CHUNK_KIND,
            &PlantTable::from_rows(&[&[NO_PLANT_MODEL, 3]]).payload,
        ));
        let dat = FurnitureDat::parse(&file).unwrap();
        assert_eq!(dat.properties.get(1).unwrap().model, 7);
        assert_eq!(dat.plants.unwrap().plant_model(1, 1), Some(3));
        assert!(FurnitureDat::parse(&chunk(b"plnt", PLANT_CHUNK_KIND, &[])).is_none());
    }

    #[test]
    fn the_dat_reads_its_step_rotate_and_placed_sound_chunks() {
        const SE_ID_AT: usize = 8;
        let sep = |name: &[u8; 4], se_id: u32| {
            let mut body = [0u8; CHUNK_HEADER_LEN];
            body[SE_ID_AT..SE_ID_AT + 4].copy_from_slice(&se_id.to_le_bytes());
            chunk(name, ChunkKind::Sep as u8, &body)
        };
        let mut file = chunk(b"furn", PROPERTY_CHUNK_KIND, &record(0, 1, 1, 1, 0));
        file.extend(sep(&STEP_SOUND_NAME, 11));
        file.extend(sep(b"9999", 12));
        file.extend(sep(&PLACED_SOUND_NAME, 13));
        file.extend(sep(&STEP_SOUND_NAME, 14));
        let sounds = FurnitureDat::parse(&file).unwrap().sounds;
        assert_eq!(
            sounds,
            FurnitureSounds {
                step: Some(11),
                rotate: None,
                placed: Some(13),
            },
            "the first chunk of each name; an absent chunk plays nothing"
        );
    }

    #[test]
    fn second_floor_slots_name_their_wall_column_and_height() {
        for slot in 0..WALL_SLOTS_2F_COUNT as i8 {
            let place = wall_place_2f(slot).unwrap();
            assert_eq!(wall_slot_2f(place), slot);
            assert!(place.height < WALL_HEIGHTS_2F_COUNT);
            assert!(place.column < place.wall.columns());
            let along_x = matches!(place.wall, Wall2F::North | Wall2F::South);
            assert_eq!(wall_runs_along_x(FLOOR_2F, slot), Some(along_x));
        }
        assert_eq!(wall_place_2f(WALL_SLOTS_2F_COUNT as i8), None);
        assert_eq!(wall_place_2f(-1), None);
        assert_eq!(
            wall_place_2f(36 + 4 + 2),
            Some(WallPlace2F {
                wall: Wall2F::North,
                column: 1,
                height: 2,
            })
        );
        assert_eq!(Wall2F::South.last_column(), 1);
        assert_eq!(wall_runs_along_x(FLOOR_1F, 4), Some(false));
        assert_eq!(wall_runs_along_x(FLOOR_1F, 5), None);
    }

    #[test]
    fn model_numbers_map_to_file_ids_by_band() {
        use FurnitureModel::*;
        assert_eq!(furniture_model(0), File(LOW_MODEL_FILE_BASE_PINNED));
        assert_eq!(
            furniture_model(LOW_MODELS.end - 1),
            File(LOW_MODEL_FILE_BASE_PINNED + u32::from(LOW_MODELS.end - 1))
        );
        assert_eq!(
            furniture_model(MID_MODELS.start),
            File(MID_MODEL_FILE_BASE_PINNED + u32::from(MID_MODELS.start))
        );
        assert_eq!(
            furniture_model(*NPC_LOOK_MODELS.start() - 1),
            File(MID_MODEL_FILE_BASE_PINNED + u32::from(*NPC_LOOK_MODELS.start() - 1))
        );
        for n in NPC_LOOK_MODELS {
            assert_eq!(furniture_model(n), NpcLook(n));
        }
        assert_eq!(
            furniture_model(*NPC_LOOK_MODELS.end() + 1),
            File(MID_MODEL_FILE_BASE_PINNED + u32::from(*NPC_LOOK_MODELS.end() + 1))
        );
        assert_eq!(furniture_model(MID_MODELS.end), Absent);
        assert_eq!(furniture_model(HIGH_MODELS_START_PINNED - 1), Absent);
        assert_eq!(
            furniture_model(HIGH_MODELS_START_PINNED),
            File(HIGH_MODEL_FILE_BASE_PINNED + u32::from(HIGH_MODELS_START_PINNED))
        );
        assert_eq!(
            furniture_model(MODEL_NUMBER_TOP_PINNED - 1),
            File(HIGH_MODEL_FILE_BASE_PINNED + u32::from(MODEL_NUMBER_TOP_PINNED - 1))
        );
        assert_eq!(
            furniture_model(MODEL_NUMBER_TOP_PINNED),
            File(LOW_MODEL_FILE_BASE_PINNED)
        );
        assert_eq!(furniture_model(u16::MAX), File(LOW_MODEL_FILE_BASE_PINNED));
    }

    fn mmb(name: &[u8; 4]) -> Vec<u8> {
        chunk(name, ChunkKind::Mmb as u8, &[0u8; CHUNK_HEADER_LEN])
    }

    fn img() -> Vec<u8> {
        chunk(b"tex0", ChunkKind::Img as u8, &[0u8; CHUNK_HEADER_LEN])
    }

    #[test]
    fn the_mesh_is_fune_then_underscore_fun_then_the_first_mmb() {
        let mut file = img();
        file.extend(mmb(b"aaaa"));
        file.extend(dir(b"mode", &[mmb(b"_fun"), mmb(b"fune")].concat()));
        file.extend(mmb(b"fune"));
        let pick = pick_static_mesh(&file).unwrap();
        assert_eq!(pick.rule, StaticMeshRule::Fune);
        assert_eq!(
            pick.chunk_index, 4,
            "the nested fune is first in file order"
        );
        assert!(!pick.alpha_tested());
        assert!(pick.eases_rotation());
        assert!(!pick.starts_schedulers());

        let mut file = img();
        file.extend(mmb(b"aaaa"));
        file.extend(mmb(b"_fun"));
        file.extend(mmb(b"_fun"));
        let pick = pick_static_mesh(&file).unwrap();
        assert_eq!(
            (pick.rule, pick.chunk_index),
            (StaticMeshRule::UnderscoreFun, 2)
        );
        assert!(pick.alpha_tested() && pick.eases_rotation());

        let mut file = img();
        file.extend(mmb(b"_abc"));
        file.extend(mmb(b"bbbb"));
        let pick = pick_static_mesh(&file).unwrap();
        assert_eq!((pick.rule, pick.chunk_index), (StaticMeshRule::FirstMmb, 1));
        assert!(pick.alpha_tested(), "rule (c) with a leading underscore");
        assert!(!pick.eases_rotation());
        assert!(pick.starts_schedulers());

        let mut plain = img();
        plain.extend(mmb(b"bbbb"));
        assert!(!pick_static_mesh(&plain).unwrap().alpha_tested());
        assert_eq!(pick_static_mesh(&img()), None);
    }

    fn name(namespace: &[u8; 8], local: &[u8; 8]) -> QualifiedTextureName {
        let mut n = [0u8; 16];
        n[..NAMESPACE_LEN].copy_from_slice(namespace);
        n[NAMESPACE_LEN..].copy_from_slice(local);
        n
    }

    #[test]
    fn a_part_texture_resolves_own_file_then_full_name_then_local_name() {
        let part = name(b"furnitur", b"table_01");
        let own = [(name(b"furnitur", b"table_01"), "own")];
        let loaded = [
            (name(b"zone____", b"table_01"), "local"),
            (name(b"furnitur", b"table_01"), "loaded"),
        ];
        assert_eq!(match_part_texture(&part, &own, &loaded), Some("own"));
        assert_eq!(match_part_texture(&part, &[], &loaded), Some("loaded"));
        assert_eq!(match_part_texture(&part, &[], &loaded[..1]), Some("local"));
        let stranger = [(name(b"furnitur", b"chair_01"), "x")];
        assert_eq!(match_part_texture(&part, &stranger, &stranger), None);
    }

    #[test]
    fn the_drawn_rotation_closes_an_eighth_per_tick_the_short_way_round() {
        assert_eq!(update_ticks(0.0), 1);
        assert_eq!(update_ticks(1.2), 2);
        assert_eq!(update_ticks(2.0), 2);

        let one = ease_yaw(0.0, FRAC_PI_2, 1);
        assert!((one - FRAC_PI_2 * ROTATION_EASE_SHARE).abs() < EPSILON);
        let two = ease_yaw(0.0, FRAC_PI_2, 2);
        assert!((two - ease_yaw(one, FRAC_PI_2, 1)).abs() < EPSILON);

        let across = ease_yaw(piece_yaw(3), piece_yaw(0), 1);
        assert!(
            (across - wrap_angle(piece_yaw(3) + FRAC_PI_2 * ROTATION_EASE_SHARE)).abs() < EPSILON,
            "3 -> 0 turns forward a quarter, not back three quarters"
        );
        assert!(wrap_angle(ease_yaw(1.0, 1.0, 1) - 1.0).abs() < EPSILON);
    }

    #[test]
    fn floor_pieces_centre_on_their_rotated_footprint() {
        let pose = FurniturePose {
            col: 10,
            level: 0,
            row: 12,
            rot: 0,
        };
        assert!(close(
            floor_piece_point(GRID_1F, pose, (1, 1)),
            [0.3, 0.0, 0.3]
        ));
        let bed = FurnitureProperty::from_record(&record(0, 0, 3, 2, 0)).unwrap();
        let p = piece_point(FLOOR_1F, &bed, pose, 0).unwrap();
        assert!(close(p, [0.9, 0.0, 0.6]));
        let turned = FurniturePose { rot: 1, ..pose };
        let p = piece_point(FLOOR_1F, &bed, turned, 0).unwrap();
        assert!(close(p, [0.6, 0.0, 0.9]), "rot 1 swaps width and depth");

        let raised = FurniturePose {
            col: 0,
            level: 10,
            row: 0,
            rot: 2,
        };
        let p = piece_point(FLOOR_2F, &bed, raised, 0).unwrap();
        assert!(close(p, [-6.0 + 0.9, -1.0, -7.8 + 0.6]), "2F origin row 13");
        assert!((piece_yaw(2) - PI).abs() < EPSILON);
        assert!(piece_point(2, &bed, pose, 0).is_none());
    }

    #[test]
    fn first_floor_wall_slots_and_the_san_doria_walls() {
        let expected = [
            ([6.5, -3.0, 4.55], 1),
            ([6.5, -3.0, -4.8], 1),
            ([-6.5, -3.0, -4.8], 3),
            ([-6.5, -3.0, 0.0], 3),
            ([-6.5, -3.0, 4.55], 3),
        ];
        for (slot, (point, rotation)) in expected.iter().enumerate() {
            let s = wall_slot(FLOOR_1F, slot as i8, 0).unwrap();
            assert!(close(s.point, *point), "slot {slot}");
            assert_eq!(s.rotation, *rotation);
            let sandy = wall_slot(FLOOR_1F, slot as i8, SAN_DORIA_HOME_HOUSE).unwrap();
            assert!((sandy.point[0].abs() - WALL_X_1F_SAN_DORIA).abs() < EPSILON);
            assert_eq!(sandy.point[0].signum(), point[0].signum());
            assert!((sandy.point[2] - point[2]).abs() < EPSILON);
        }
        assert!(wall_slot(FLOOR_1F, WALL_SLOTS_1F_COUNT as i8, 0).is_none());
        assert!(wall_slot(FLOOR_1F, -1, 0).is_none());

        let mirror = FurnitureProperty::from_record(&record(FLAG_WALL_HUNG, 0, 3, 1, 0)).unwrap();
        let hung = FurniturePose {
            col: 3,
            level: 0,
            row: 0,
            rot: 3,
        };
        let p = piece_point(FLOOR_1F, &mirror, hung, 0).unwrap();
        assert!(close(p, [-6.5, -3.0, 0.0]), "no footprint offset on a wall");
    }

    #[test]
    fn second_floor_wall_slots_run_four_heights_per_column() {
        let at = |slot: i8| wall_slot(FLOOR_2F, slot, 0).unwrap();
        assert!(close(at(0).point, [-6.2, -5.6, -6.2]));
        assert_eq!(at(0).rotation, 3);
        assert!(close(at(4 * 4 + 3).point, [-6.2, -1.8, 0.0]));
        assert!(close(at(36 + 4 + 1).point, [0.0, -3.2, 8.0]));
        assert_eq!(at(36).rotation, 0);
        assert!(close(at(48 + 4 * 8 + 2).point, [6.2, -2.5, 6.2]));
        assert_eq!(at(48).rotation, 1);
        assert!(close(at(84 + 4).point, [4.0, -5.6, -8.0]));
        assert_eq!(at(91).rotation, 2);
        assert!(wall_slot(FLOOR_2F, WALL_SLOTS_2F_COUNT as i8, 0).is_none());

        let mut next = 0usize;
        for wall in &WALLS_2F {
            assert_eq!(usize::from(wall.base), next, "walls tile the slot range");
            next += wall.columns.len() * WALL_HEIGHTS_2F.len();
        }
        assert_eq!(next, WALL_SLOTS_2F_COUNT);
    }

    #[test]
    fn a_plant_stands_point_three_above_its_pot() {
        assert!(close(plant_point([1.0, -0.5, 2.0]), [1.0, -0.8, 2.0]));
    }

    #[test]
    fn only_2f_or_a_home_nation_1f_is_arrangeable() {
        assert!(house_is_arrangeable(FLOOR_2F, 0, true));
        for house in HOME_NATION_HOUSES {
            assert!(house_is_arrangeable(FLOOR_1F, house, true));
        }
        assert!(!house_is_arrangeable(
            FLOOR_1F,
            *HOME_NATION_HOUSES.end() + 1,
            true
        ));
        assert!(!house_is_arrangeable(
            FLOOR_1F,
            *HOME_NATION_HOUSES.start() - 1,
            true
        ));
        assert!(!house_is_arrangeable(FLOOR_2F, 0, false));
        assert!(statue_npcs_shown(true, true));
        assert!(!statue_npcs_shown(true, false));
        assert!(!statue_npcs_shown(false, true));
    }

    #[test]
    fn mannequin_idles_take_the_pose_digit_and_variant() {
        let idle = mannequin_idle(2, None);
        assert_eq!(idle.family, DatId::from_str("mq2?"));
        assert_eq!(idle.variant, 0);
        let idle = mannequin_idle(MANNEQUIN_DRESSED_POSE, None);
        assert_eq!(idle.family, DatId::from_str("mq6?"));
        assert_eq!(idle.variant, MANNEQUIN_DRESSED_VARIANT);
        assert_eq!(
            mannequin_idle(MANNEQUIN_DRESSED_POSE, Some(3)).family,
            DatId::from_str("mq3?")
        );
        let idle = mannequin_idle(0b1_0101, None);
        assert_eq!((idle.family, idle.variant), (DatId::from_str("mq5?"), 2));
    }

    #[test]
    fn the_attr_decodes_lsb_furniture_flowerpot_and_mannequin_bytes() {
        let mut attr = [0u8; ATTR_LEN];
        attr[ATTR_FLOWERPOT_STEP_BYTE] = 3;
        attr[ATTR_FLAGS_BYTE] = ATTR_INSTALLED_BIT | FLOOR_2F;
        attr[ATTR_FLOWERPOT_KIND_BYTE] = 2;
        attr[ATTR_COL_BYTE] = 5;
        attr[ATTR_LEVEL_BYTE] = 2;
        attr[ATTR_ROW_BYTE] = (-3i8) as u8;
        attr[ATTR_ROTATION_BYTE] = 1;
        attr[ATTR_MANNEQUIN_RACE_BYTE] = 4;
        attr[ATTR_MANNEQUIN_POSE_BYTE] = 9;
        let a = FurnitureAttr::decode(&attr);
        assert!(a.installed);
        assert_eq!(a.layer, FLOOR_2F);
        assert_eq!(
            a.pose,
            FurniturePose {
                col: 5,
                level: 2,
                row: -3,
                rot: 1
            }
        );
        assert_eq!((a.flowerpot_step, a.flowerpot_kind), (3, 2));
        assert_eq!((a.mannequin_race, a.mannequin_pose), (4, 9));
        attr[ATTR_FLAGS_BYTE] = FLOOR_2F;
        assert!(!FurnitureAttr::decode(&attr).installed);
    }

    #[test]
    fn seats_put_the_second_shelf_after_the_first() {
        assert_eq!(seat_index(LOC_MOGSAFE, 7), 7);
        assert_eq!(seat_index(LOC_MOGSAFE2, 7), SEATS_PER_SHELF + 7);
        assert!(is_shelf_slot(FIRST_SHELF_SLOT) && is_shelf_slot(LAST_SHELF_SLOT));
        assert!(!is_shelf_slot(0) && !is_shelf_slot(LAST_SHELF_SLOT + 1));
        assert_eq!(floor_of_sub_map(0), FLOOR_1F);
        assert_eq!(floor_of_sub_map(2), FLOOR_2F);
        assert!(is_furniture_kind(KIND_FLOWERPOT) && !is_furniture_kind(KIND_FLOWERPOT + 1));
    }
}
