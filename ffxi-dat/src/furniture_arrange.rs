//! The Mog House arrange editor's law: the highlight pulse, selection by direction, moving and
//! turning a piece, the occupancy grid that decides where it may be placed, the overlay drawn while
//! moving, the arrange camera, the furniture list filter and the menu grey-outs
//! (.agents/skills/retail-observe/references/2026-10-15-mog-house-arrange.md Part B).
//!
//! Positions are FFXI world coordinates (y grows downward); cells and wall slots are those of
//! [`crate::furniture`].

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use crate::furniture::{
    floor_grid, piece_point, wall_place_2f, wall_runs_along_x, wall_slot, wall_slot_2f, FloorGrid,
    FurniturePose, FurnitureProperty, Wall2F, WallPlace2F, CELL, FLOOR_1F, FLOOR_2F, FLOOR_COLUMNS,
    HALF_CELL, HEIGHT_PER_LEVEL, KIND_FLOWERPOT, KIND_FURNISHING, KIND_MANNEQUIN, LEVEL_STEP,
    WALL_HEIGHTS_2F_COUNT, WALL_SLOTS_1F_COUNT, WALL_SLOTS_2F_COUNT,
};

/// (R, G, B, A) colour bytes on the record's scale, where [`TINT_NORMAL`]'s bytes are 1.0.
pub type ColorBytes = [u8; 4];

const FULL: u8 = 0x80;
const DIM: u8 = 0x60;

/// Every piece's tint on entering and after a placement.
pub const TINT_NORMAL: ColorBytes = [FULL; 4];
/// Every piece's tint while one is being moved.
pub const TINT_DIMMED: ColorBytes = [DIM, DIM, DIM, FULL];
/// A furniture-list row for a piece placed on this floor.
pub const LIST_PLACED_TINT: ColorBytes = [0x50, 0x90, 0x50, FULL];
/// Overlay colours while moving.
pub const OVERLAY_FREE_FLOOR: ColorBytes = [DIM, DIM, 0xFF, FULL];
pub const OVERLAY_FREE_SURFACE: ColorBytes = [DIM, 0xFF, DIM, FULL];
pub const OVERLAY_FITS: ColorBytes = [0xFF, FULL, FULL, FULL];
pub const OVERLAY_REFUSED: ColorBytes = [DIM, DIM, DIM, FULL];
pub const OVERLAY_WALL_SLOT: ColorBytes = OVERLAY_FREE_FLOOR;
pub const OVERLAY_WALL_CURRENT: ColorBytes = OVERLAY_FITS;

const PULSE_START: i32 = 0x80;
const PULSE_STEP_PER_TICK: i32 = 8;
const PULSE_LOW: i32 = 0x20;
const PULSE_RESTART: i32 = 0x40;
const PULSE_CAP: i32 = 0xFF;
/// The highlighted piece's alpha byte, and a mannequin's.
pub const PULSE_ALPHA: u8 = 0x40;
pub const PULSE_ALPHA_MANNEQUIN: u8 = FULL;

/// The highlight grey level: it falls a fixed step per frame tick, restarts low and rising once it
/// drops below its floor, and turns to fall again at its cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pulse {
    level: i32,
    rising: bool,
}

impl Default for Pulse {
    fn default() -> Self {
        Self {
            level: PULSE_START,
            rising: false,
        }
    }
}

impl Pulse {
    /// One client update of `ticks` frame ticks.
    pub fn advance(&mut self, ticks: u32) {
        let step = PULSE_STEP_PER_TICK.saturating_mul(i32::try_from(ticks).unwrap_or(i32::MAX));
        if self.rising {
            self.level = self.level.saturating_add(step);
            if self.level >= PULSE_CAP {
                self.level = PULSE_CAP;
                self.rising = false;
            }
        } else {
            self.level = self.level.saturating_sub(step);
            if self.level < PULSE_LOW {
                self.level = PULSE_RESTART;
                self.rising = true;
            }
        }
    }

    pub fn level(&self) -> u8 {
        self.level.clamp(0, PULSE_CAP) as u8
    }

    pub fn tint(&self, alpha: u8) -> ColorBytes {
        let g = self.level();
        [g, g, g, alpha]
    }
}

/// Keypad 5 cycles the arrange camera through these views.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CameraView {
    #[default]
    TopDown,
    /// The eye south of the focus, looking north.
    FromSouth,
    /// The eye north of the focus, looking south; direction presses flip.
    FromNorth,
}

impl CameraView {
    pub fn next(self) -> Self {
        match self {
            Self::TopDown => Self::FromSouth,
            Self::FromSouth => Self::FromNorth,
            Self::FromNorth => Self::TopDown,
        }
    }

    pub fn flips_presses(self) -> bool {
        self == Self::FromNorth
    }
}

/// The camera focal length layout mode sets (the normal camera starts at 350).
pub const LAYOUT_FOCAL_LENGTH: f32 = 250.0;

const FOCUS_REACH_X: f32 = 4.0;
const FOCUS_REACH_Z_1F: f32 = 5.0;
const FOCUS_REACH_Z_2F: f32 = 6.0;
const FOCUS_RISE: f32 = 0.3;
const TOP_DOWN_EYE_RISE: f32 = 5.0;
const TOP_DOWN_EYE_BACK: f32 = 0.1;
const SIDE_EYE_SPREAD: f32 = 0.7;
const SIDE_EYE_RISE: f32 = 4.5;
const SIDE_EYE_BACK: f32 = 4.0;
const SIDE_EYE_REACH_Z: f32 = 5.2;
const CAMERA_EASE_SHARE: f32 = 0.3;
const TOP_DOWN_DISTANCE_1F: f32 = 7.0;
const TOP_DOWN_DISTANCE_2F: f32 = 9.0;

/// Where the camera looks: the selected piece's drawn point held inside the room, raised 0.3 (at
/// floor height in the top-down view); the room origin without a selection.
pub fn focus_target(selected: Option<[f32; 3]>, view: CameraView, floor: u8) -> [f32; 3] {
    let Some([x, y, z]) = selected else {
        return [0.0, -FOCUS_RISE, 0.0];
    };
    let reach_z = if floor == FLOOR_2F {
        FOCUS_REACH_Z_2F
    } else {
        FOCUS_REACH_Z_1F
    };
    let fy = match view {
        CameraView::TopDown => -FOCUS_RISE,
        _ => y - FOCUS_RISE,
    };
    [
        x.clamp(-FOCUS_REACH_X, FOCUS_REACH_X),
        fy,
        z.clamp(-reach_z, reach_z),
    ]
}

/// Where the eye heads for a focus target in each view.
pub fn eye_target(focus: [f32; 3], view: CameraView) -> [f32; 3] {
    let [fx, _, fz] = focus;
    match view {
        CameraView::TopDown => [fx, -TOP_DOWN_EYE_RISE, fz - TOP_DOWN_EYE_BACK],
        CameraView::FromSouth => [
            SIDE_EYE_SPREAD * fx,
            -SIDE_EYE_RISE,
            (fz - SIDE_EYE_BACK).max(-SIDE_EYE_REACH_Z),
        ],
        CameraView::FromNorth => [
            SIDE_EYE_SPREAD * fx,
            -SIDE_EYE_RISE,
            (fz + SIDE_EYE_BACK).min(SIDE_EYE_REACH_Z),
        ],
    }
}

fn top_down_distance(floor: u8) -> f32 {
    if floor == FLOOR_2F {
        TOP_DOWN_DISTANCE_2F
    } else {
        TOP_DOWN_DISTANCE_1F
    }
}

fn lerp3(from: [f32; 3], to: [f32; 3], share: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| from[i] + (to[i] - from[i]) * share)
}

/// The arrange camera's eye and focus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraRig {
    pub eye: [f32; 3],
    pub focus: [f32; 3],
}

impl CameraRig {
    /// One client update: eye and focus each close 0.3 of the way to their targets; top-down, the eye
    /// then sits at the floor's fixed distance from the focus along their line.
    pub fn update(&mut self, selected: Option<[f32; 3]>, view: CameraView, floor: u8) {
        let focus = focus_target(selected, view, floor);
        let eye = eye_target(focus, view);
        self.eye = lerp3(self.eye, eye, CAMERA_EASE_SHARE);
        self.focus = lerp3(self.focus, focus, CAMERA_EASE_SHARE);
        if view == CameraView::TopDown {
            let offset = [0, 1, 2].map(|i| self.eye[i] - self.focus[i]);
            let length = offset.iter().map(|v| v * v).sum::<f32>().sqrt();
            if length > f32::EPSILON {
                let scale = top_down_distance(floor) / length;
                self.eye = [0, 1, 2].map(|i| self.focus[i] + offset[i] * scale);
            }
        }
    }

    /// `/layoutctrl type1` reverses floor steps while the eye is not south of the focus.
    pub fn reverses_type1_presses(&self) -> bool {
        self.eye[2] >= self.focus[2]
    }
}

/// An arrow press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    Up,
    Right,
    Down,
    Left,
}

impl Press {
    pub const ALL: [Press; 4] = [Press::Up, Press::Right, Press::Down, Press::Left];

    /// (dx, dz): Right (1, 0), Left (-1, 0), Up (0, 1), Down (0, -1). On the floor grid the same pair
    /// is (columns, rows).
    pub fn delta(self) -> (i8, i8) {
        match self {
            Press::Up => (0, 1),
            Press::Right => (1, 0),
            Press::Down => (0, -1),
            Press::Left => (-1, 0),
        }
    }

    fn index(self) -> usize {
        match self {
            Press::Up => 0,
            Press::Right => 1,
            Press::Down => 2,
            Press::Left => 3,
        }
    }
}

/// A floor piece's step for a press; `reversed` turns all four round.
pub fn floor_step(press: Press, reversed: bool) -> (i8, i8) {
    let (dcol, drow) = press.delta();
    if reversed {
        (-dcol, -drow)
    } else {
        (dcol, drow)
    }
}

const SAME_SPOT: f32 = 0.1;
const FLOOR_PIECE_PREFERENCE: f32 = 0.1;
const QUARTER_TURNS: i8 = 4;

/// One drawn piece as the selection rule sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavPiece {
    pub seat: usize,
    pub point: [f32; 3],
    pub level: i8,
    pub wall_hung: bool,
    pub rot: i8,
    /// Rotated footprint.
    pub footprint: (u8, u8),
    /// Something stands on it.
    pub carries: bool,
}

/// A piece's (x, z) for the selection rule: a wall piece is pushed by its height toward its rotation.
pub fn nav_point(piece: &NavPiece) -> (f32, f32) {
    let [x, y, z] = piece.point;
    if !piece.wall_hung {
        return (x, z);
    }
    let h = -y;
    match piece.rot.rem_euclid(QUARTER_TURNS) {
        0 => (x, z + h),
        1 => (x, z - h),
        2 => (x + h, z),
        _ => (x - h, z),
    }
}

/// The piece a press of (dx, dz) selects from `selected`, if any: one standing on the selected
/// floor piece's spot at once; else the nearest piece past it in that direction, a floor piece
/// winning a near tie.
pub fn navigate(selected: &NavPiece, pieces: &[NavPiece], (dx, dz): (i8, i8)) -> Option<usize> {
    let (px, pz) = nav_point(selected);
    let (tx, tz) = if selected.carries || selected.wall_hung {
        (0.0, 0.0)
    } else {
        (
            f32::from(selected.footprint.0) * HALF_CELL,
            f32::from(selected.footprint.1) * HALF_CELL,
        )
    };
    let mut best: Option<(f32, usize)> = None;
    let mut best_floor: Option<(f32, usize)> = None;
    for piece in pieces.iter().filter(|piece| piece.seat != selected.seat) {
        let (qx, qz) = nav_point(piece);
        let (ex, ez) = (qx - px, qz - pz);
        if selected.level == 0 && ex.abs() + ez.abs() < SAME_SPOT {
            return Some(piece.seat);
        }
        if dx == 0 && dz == 0 {
            continue;
        }
        let horizontal = dx == 0 || (ex * f32::from(dx) > tx && ex.abs() >= ez.abs());
        let vertical = dz == 0 || (ez * f32::from(dz) > tz && ez.abs() >= ex.abs());
        if !horizontal || !vertical {
            continue;
        }
        let d2 = ex * ex + ez * ez;
        if best.is_none_or(|(b, _)| d2 < b) {
            best = Some((d2, piece.seat));
        }
        if piece.level == 0 && best_floor.is_none_or(|(b, _)| d2 < b) {
            best_floor = Some((d2, piece.seat));
        }
    }
    let (best_d2, best_seat) = best?;
    match best_floor {
        Some((d2, seat)) if d2 - best_d2 <= FLOOR_PIECE_PREFERENCE => Some(seat),
        _ => Some(best_seat),
    }
}

/// A floor anchor held so the footprint stays inside the floor.
pub fn clamp_anchor(grid: FloorGrid, footprint: (u8, u8), col: i16, row: i16) -> (i8, i8) {
    let max_col = (i16::from(FLOOR_COLUMNS) - i16::from(footprint.0)).max(0);
    let max_row = (i16::from(grid.rows) - i16::from(footprint.1)).max(0);
    (col.clamp(0, max_col) as i8, row.clamp(0, max_row) as i8)
}

/// 1F wall slot → the slot an (Up, Right, Down, Left) press steps to.
const WALL_STEPS_1F: [[i8; 4]; WALL_SLOTS_1F_COUNT] = [
    [0, 0, 1, 4],
    [0, 1, 1, 2],
    [3, 1, 2, 2],
    [4, 0, 2, 3],
    [4, 0, 3, 4],
];

fn wall_step_2f(place: WallPlace2F, press: Press) -> WallPlace2F {
    use Wall2F::{East, North, South, West};
    let at = |wall, column, height| WallPlace2F {
        wall,
        column,
        height,
    };
    let top = WALL_HEIGHTS_2F_COUNT - 1;
    let (c, h, last) = (place.column, place.height, place.wall.last_column());
    let up_height = h.saturating_sub(1);
    match (place.wall, press) {
        (West, Press::Up) if c < last => at(West, c + 1, h),
        (West, Press::Up) => at(North, 0, h),
        (West, Press::Right) if h < top => at(West, c, h + 1),
        (West, Press::Right) => at(East, c, top),
        (West, Press::Down) if c > 0 => at(West, c - 1, h),
        (West, Press::Down) => at(South, 0, h),
        (West, Press::Left) => at(West, c, up_height),
        (East, Press::Up) if c < last => at(East, c + 1, h),
        (East, Press::Up) => at(North, North.last_column(), h),
        (East, Press::Right) => at(East, c, up_height),
        (East, Press::Down) if c > 0 => at(East, c - 1, h),
        (East, Press::Down) => at(South, South.last_column(), h),
        (East, Press::Left) if h < top => at(East, c, h + 1),
        (East, Press::Left) => at(West, c, top),
        (North, Press::Up) => at(North, c, up_height),
        (North, Press::Right) if c < last => at(North, c + 1, h),
        (North, Press::Right) => at(East, East.last_column(), h),
        (North, Press::Down) if h < top => at(North, c, h + 1),
        (North, Press::Down) if c == 0 => at(South, 0, top),
        (North, Press::Down) if c == last => at(South, South.last_column(), top),
        (North, Press::Down) => place,
        (North, Press::Left) if c > 0 => at(North, c - 1, h),
        (North, Press::Left) => at(West, West.last_column(), h),
        (South, Press::Up) if h < top => at(South, c, h + 1),
        (South, Press::Up) if c == 0 => at(North, 0, top),
        (South, Press::Up) => at(North, North.last_column(), top),
        (South, Press::Right) if c < last => at(South, c + 1, h),
        (South, Press::Right) => at(East, 0, h),
        (South, Press::Down) => at(South, c, up_height),
        (South, Press::Left) if c > 0 => at(South, c - 1, h),
        (South, Press::Left) => at(West, 0, h),
    }
}

/// The wall slot a press steps a wall piece to; the same slot when it does not move.
pub fn wall_step(floor: u8, slot: i8, press: Press) -> i8 {
    match floor {
        FLOOR_1F => usize::try_from(slot)
            .ok()
            .and_then(|slot| WALL_STEPS_1F.get(slot))
            .map_or(slot, |steps| steps[press.index()]),
        FLOOR_2F => {
            wall_place_2f(slot).map_or(slot, |place| wall_slot_2f(wall_step_2f(place, press)))
        }
        _ => slot,
    }
}

/// One quarter turn back: rot - 1, mod 4.
pub fn turned_back(rot: i8) -> i8 {
    (rot.rem_euclid(QUARTER_TURNS) - 1).rem_euclid(QUARTER_TURNS)
}

/// Where a piece standing on a turned surface lands: parent anchor + (W' - drow - d, dcol), from
/// its old offset (dcol, drow), the parent's width after the turn and its own depth before its turn.
pub fn turned_child_anchor(
    parent: (i8, i8),
    parent_width_after: u8,
    offset: (i8, i8),
    child_depth_before: u8,
) -> (i8, i8) {
    let col = i16::from(parent.0) + i16::from(parent_width_after)
        - i16::from(offset.1)
        - i16::from(child_depth_before);
    let row = i16::from(parent.1) + i16::from(offset.0);
    (col as i8, row as i8)
}

/// The 1F floor area no piece may cover.
pub const BLOCKED_1F_COLUMNS: RangeInclusive<i8> = 6..=13;
pub const BLOCKED_1F_ROWS: RangeInclusive<i8> = 0..=17;
/// At most this many pieces belong to one surface.
pub const MAX_PIECES_ON_SURFACE: usize = 32;

/// One floor cell of the occupancy grid.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Cell {
    #[default]
    Free,
    /// Under a surface piece: the floor is taken, its top (at the surface's height in hundredths)
    /// free until a piece stands there.
    Surface {
        piece: usize,
        height: u16,
        top: Option<usize>,
    },
    /// Under any other floor piece.
    Covered { piece: usize },
    /// The 1F area no piece may cover.
    Blocked,
}

/// A piece the editor holds: its seat, property record and the pose it is drawn at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrangePiece {
    pub seat: usize,
    pub property: FurnitureProperty,
    pub pose: FurniturePose,
}

impl ArrangePiece {
    pub fn footprint(&self) -> (u8, u8) {
        self.property.footprint(self.pose.rot)
    }

    /// Footprint cells, scanned column then row.
    pub fn cells(&self) -> impl Iterator<Item = (i16, i16)> {
        let (w, d) = self.footprint();
        let (col, row) = (i16::from(self.pose.col), i16::from(self.pose.row));
        (0..i16::from(w)).flat_map(move |dc| (0..i16::from(d)).map(move |dr| (col + dc, row + dr)))
    }
}

/// The level a stack level of a surface `height` (hundredths) gives a piece standing on it.
fn level_on(height: u16) -> i8 {
    i8::try_from(height / HEIGHT_PER_LEVEL).unwrap_or(i8::MAX)
}

/// World units of a surface height in hundredths.
fn surface_top(height: u16) -> f32 {
    f32::from(height) * LEVEL_STEP / f32::from(HEIGHT_PER_LEVEL)
}

/// Which floor cells and wall slots are taken, and what stands on what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occupancy {
    grid: FloorGrid,
    floor: u8,
    cells: Vec<Cell>,
    walls: BTreeMap<i8, usize>,
    carried: BTreeMap<usize, Vec<usize>>,
}

impl Occupancy {
    pub fn new(floor: u8) -> Option<Self> {
        let grid = floor_grid(floor)?;
        let len = usize::from(FLOOR_COLUMNS.unsigned_abs()) * usize::from(grid.rows.unsigned_abs());
        let mut occupancy = Self {
            grid,
            floor,
            cells: vec![Cell::Free; len],
            walls: BTreeMap::new(),
            carried: BTreeMap::new(),
        };
        occupancy.clear();
        Some(occupancy)
    }

    pub fn grid(&self) -> FloorGrid {
        self.grid
    }

    fn index(&self, col: i16, row: i16) -> Option<usize> {
        let inside = (0..i16::from(FLOOR_COLUMNS)).contains(&col)
            && (0..i16::from(self.grid.rows)).contains(&row);
        inside.then(|| (row as usize) * usize::from(FLOOR_COLUMNS.unsigned_abs()) + col as usize)
    }

    pub fn cell(&self, col: i16, row: i16) -> Option<Cell> {
        self.index(col, row).map(|i| self.cells[i])
    }

    /// Empties the grid back to its blocked area alone.
    pub fn clear(&mut self) {
        let blocked = self.floor == FLOOR_1F;
        for row in 0..i16::from(self.grid.rows) {
            for col in 0..i16::from(FLOOR_COLUMNS) {
                let Some(i) = self.index(col, row) else {
                    continue;
                };
                self.cells[i] = if blocked
                    && BLOCKED_1F_COLUMNS.contains(&(col as i8))
                    && BLOCKED_1F_ROWS.contains(&(row as i8))
                {
                    Cell::Blocked
                } else {
                    Cell::Free
                };
            }
        }
        self.walls.clear();
        self.carried.clear();
    }

    /// Takes a piece's cells or wall slot, then rebuilds what stands on what.
    pub fn register(&mut self, piece: &ArrangePiece) {
        if piece.property.wall_hung() {
            self.walls.insert(piece.pose.col, piece.seat);
        } else if piece.pose.level > 0 {
            for (col, row) in piece.cells() {
                if let Some(Cell::Surface { top, .. }) =
                    self.index(col, row).map(|i| &mut self.cells[i])
                {
                    *top = Some(piece.seat);
                }
            }
        } else {
            for (col, row) in piece.cells() {
                let Some(i) = self.index(col, row) else {
                    continue;
                };
                if self.cells[i] == Cell::Blocked {
                    continue;
                }
                self.cells[i] = if piece.property.surface() {
                    Cell::Surface {
                        piece: piece.seat,
                        height: piece.property.height,
                        top: None,
                    }
                } else {
                    Cell::Covered { piece: piece.seat }
                };
            }
        }
        self.rebuild_carried();
    }

    /// Frees everything a seat holds.
    pub fn unregister(&mut self, seat: usize) {
        for cell in &mut self.cells {
            match cell {
                Cell::Covered { piece } | Cell::Surface { piece, .. } if *piece == seat => {
                    *cell = Cell::Free;
                }
                Cell::Surface { top, .. } if *top == Some(seat) => *top = None,
                _ => {}
            }
        }
        self.walls.retain(|_, held| *held != seat);
        self.rebuild_carried();
    }

    fn rebuild_carried(&mut self) {
        let mut carried: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for cell in &self.cells {
            if let Cell::Surface {
                piece,
                top: Some(top),
                ..
            } = *cell
            {
                let on = carried.entry(piece).or_default();
                if !on.contains(&top) && on.len() < MAX_PIECES_ON_SURFACE {
                    on.push(top);
                }
            }
        }
        self.carried = carried;
    }

    /// The pieces standing on a surface piece.
    pub fn carried_by(&self, seat: usize) -> &[usize] {
        self.carried.get(&seat).map_or(&[], Vec::as_slice)
    }

    /// Whether Place would accept the piece where it stands: a wall piece's slot empty; a floor
    /// piece's every cell inside the grid and free; a raised piece's every cell a free surface top of
    /// one surface piece.
    pub fn fits(&self, piece: &ArrangePiece) -> bool {
        if piece.property.wall_hung() {
            let slot_exists = match self.floor {
                FLOOR_1F => usize::try_from(piece.pose.col).is_ok_and(|s| s < WALL_SLOTS_1F_COUNT),
                _ => usize::try_from(piece.pose.col).is_ok_and(|s| s < WALL_SLOTS_2F_COUNT),
            };
            return slot_exists && !self.walls.contains_key(&piece.pose.col);
        }
        if piece.pose.level > 0 {
            let mut surface = None;
            return piece.cells().all(|(col, row)| match self.cell(col, row) {
                Some(Cell::Surface {
                    piece: under,
                    top: None,
                    ..
                }) => *surface.get_or_insert(under) == under,
                _ => false,
            });
        }
        piece
            .cells()
            .all(|(col, row)| self.cell(col, row) == Some(Cell::Free))
    }

    /// A stackable piece's level over its footprint: the last cell (column then row) with a surface
    /// under it gives height / 10; `None` with no surface under it.
    pub fn level_under(&self, piece: &ArrangePiece) -> Option<i8> {
        piece
            .cells()
            .filter_map(|(col, row)| match self.cell(col, row) {
                Some(Cell::Surface { height, .. }) => Some(level_on(height)),
                _ => None,
            })
            .last()
    }

    /// Free floor cells, row by row.
    pub fn free_floor_cells(&self) -> Vec<(i16, i16)> {
        self.scan(|cell| (cell == Cell::Free).then_some(()))
            .into_iter()
            .map(|(col, row, ())| (col, row))
            .collect()
    }

    /// Free surface tops and their surface heights in hundredths.
    pub fn free_surface_tops(&self) -> Vec<(i16, i16, u16)> {
        self.scan(|cell| match cell {
            Cell::Surface {
                height, top: None, ..
            } => Some(height),
            _ => None,
        })
    }

    fn scan<T>(&self, mut pick: impl FnMut(Cell) -> Option<T>) -> Vec<(i16, i16, T)> {
        let mut out = Vec::new();
        for row in 0..i16::from(self.grid.rows) {
            for col in 0..i16::from(FLOOR_COLUMNS) {
                if let Some(value) = self.cell(col, row).and_then(&mut pick) {
                    out.push((col, row, value));
                }
            }
        }
        out
    }

    /// The surface height under a cell in hundredths, 0 on the bare floor.
    pub fn cell_height(&self, col: i16, row: i16) -> u16 {
        match self.cell(col, row) {
            Some(Cell::Surface { height, .. }) => height,
            _ => 0,
        }
    }
}

/// A piece being moved and the pieces it carries along.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveSession {
    pub seat: usize,
    pub carried: Vec<usize>,
}

/// What a quarter turn did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turned {
    Floor,
    Wall,
}

/// One floor of a Mog House as the editor holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Room {
    pub floor: u8,
    pub map_number: u16,
    pieces: BTreeMap<usize, ArrangePiece>,
    occupancy: Occupancy,
}

/// A new piece's start anchor on the floor when no floor piece is selected.
pub const NEW_PIECE_ANCHOR: (i8, i8) = (9, 12);

impl Room {
    pub fn new(floor: u8, map_number: u16) -> Option<Self> {
        Some(Self {
            floor,
            map_number,
            pieces: BTreeMap::new(),
            occupancy: Occupancy::new(floor)?,
        })
    }

    pub fn piece(&self, seat: usize) -> Option<&ArrangePiece> {
        self.pieces.get(&seat)
    }

    /// Pieces in seat order.
    pub fn pieces(&self) -> impl Iterator<Item = &ArrangePiece> {
        self.pieces.values()
    }

    pub fn first_seat(&self) -> Option<usize> {
        self.pieces.keys().next().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    pub fn occupancy(&self) -> &Occupancy {
        &self.occupancy
    }

    /// Adds a drawn piece and registers it.
    pub fn insert(&mut self, piece: ArrangePiece) {
        self.pieces.insert(piece.seat, piece);
        self.occupancy.register(&piece);
    }

    /// Adds a piece that is not registered yet (one about to be moved).
    pub fn insert_unregistered(&mut self, piece: ArrangePiece) {
        self.pieces.insert(piece.seat, piece);
    }

    pub fn remove(&mut self, seat: usize) -> Option<ArrangePiece> {
        self.occupancy.unregister(seat);
        self.pieces.remove(&seat)
    }

    /// Put away all: the grid forgets every piece.
    pub fn clear_occupancy(&mut self) {
        self.occupancy.clear();
    }

    /// A piece's drawn point.
    pub fn point(&self, seat: usize) -> Option<[f32; 3]> {
        let piece = self.pieces.get(&seat)?;
        piece_point(self.floor, &piece.property, piece.pose, self.map_number)
    }

    pub fn nav_piece(&self, seat: usize) -> Option<NavPiece> {
        let piece = self.pieces.get(&seat)?;
        Some(NavPiece {
            seat,
            point: self.point(seat)?,
            level: piece.pose.level,
            wall_hung: piece.property.wall_hung(),
            rot: piece.pose.rot,
            footprint: piece.footprint(),
            carries: !self.occupancy.carried_by(seat).is_empty(),
        })
    }

    /// The selection a direction press moves to from `from`.
    pub fn navigate(&self, from: usize, delta: (i8, i8)) -> Option<usize> {
        let selected = self.nav_piece(from)?;
        let others: Vec<NavPiece> = self
            .pieces
            .keys()
            .filter_map(|&seat| self.nav_piece(seat))
            .collect();
        navigate(&selected, &others, delta)
    }

    /// The drawn piece nearest a point in (x, z), seat order breaking ties.
    pub fn nearest(&self, reference: (f32, f32)) -> Option<usize> {
        let mut best: Option<(f32, usize)> = None;
        for &seat in self.pieces.keys() {
            let Some([x, _, z]) = self.point(seat) else {
                continue;
            };
            let d2 = (x - reference.0).powi(2) + (z - reference.1).powi(2);
            if best.is_none_or(|(b, _)| d2 < b) {
                best = Some((d2, seat));
            }
        }
        best.map(|(_, seat)| seat)
    }

    /// Starts moving a piece: it and everything on it leave the grid.
    pub fn begin_move(&mut self, seat: usize) -> MoveSession {
        let carried = self.occupancy.carried_by(seat).to_vec();
        self.occupancy.unregister(seat);
        for &child in &carried {
            self.occupancy.unregister(child);
        }
        MoveSession { seat, carried }
    }

    /// A floor pose stepped by (columns, rows): the anchor held inside the floor; a pose that would not
    /// change is returned unchanged with `false`; otherwise level 0, a stackable piece raised onto the
    /// surface under it.
    fn stepped(
        &self,
        property: &FurnitureProperty,
        pose: FurniturePose,
        delta: (i8, i8),
    ) -> (FurniturePose, bool) {
        let grid = self.occupancy.grid();
        let footprint = property.footprint(pose.rot);
        let (col, row) = clamp_anchor(
            grid,
            footprint,
            i16::from(pose.col) + i16::from(delta.0),
            i16::from(pose.row) + i16::from(delta.1),
        );
        if (col, row) == (pose.col, pose.row) {
            return (pose, false);
        }
        let mut stepped = FurniturePose {
            col,
            row,
            level: 0,
            ..pose
        };
        if property.stackable() {
            let probe = ArrangePiece {
                seat: usize::MAX,
                property: *property,
                pose: stepped,
            };
            stepped.level = self.occupancy.level_under(&probe).unwrap_or(0);
        }
        (stepped, true)
    }

    fn move_floor(&mut self, session: &MoveSession, delta: (i8, i8)) -> bool {
        let Some(piece) = self.pieces.get(&session.seat).copied() else {
            return false;
        };
        let (pose, moved) = self.stepped(&piece.property, piece.pose, delta);
        if !moved {
            return false;
        }
        let shift = (
            pose.col.saturating_sub(piece.pose.col),
            pose.row.saturating_sub(piece.pose.row),
        );
        for child in &session.carried {
            if let Some(child) = self.pieces.get_mut(child) {
                child.pose.col = child.pose.col.saturating_add(shift.0);
                child.pose.row = child.pose.row.saturating_add(shift.1);
            }
        }
        if let Some(piece) = self.pieces.get_mut(&session.seat) {
            piece.pose = pose;
        }
        true
    }

    /// One step of the moving piece; `reversed` is the `/layoutctrl` reversal for floor pieces.
    /// Returns whether it moved.
    pub fn step(&mut self, session: &MoveSession, press: Press, reversed: bool) -> bool {
        let Some(piece) = self.pieces.get(&session.seat).copied() else {
            return false;
        };
        if !piece.property.wall_hung() {
            return self.move_floor(session, floor_step(press, reversed));
        }
        let slot = wall_step(self.floor, piece.pose.col, press);
        if slot == piece.pose.col {
            return false;
        }
        let rotation = wall_slot(self.floor, slot, self.map_number).map(|s| s.rotation);
        if let Some(piece) = self.pieces.get_mut(&session.seat) {
            piece.pose.col = slot;
            if let Some(rotation) = rotation {
                piece.pose.rot = rotation;
            }
        }
        true
    }

    /// A quarter turn back of the moving piece. A floor piece keeps its anchor, carries what stands on
    /// it round with it, is held back inside the floor and re-levels when stackable; a wall piece only
    /// changes its stored rotation.
    pub fn turn(&mut self, session: &MoveSession) -> Option<Turned> {
        let piece = self.pieces.get(&session.seat).copied()?;
        let rot = turned_back(piece.pose.rot);
        if piece.property.wall_hung() {
            self.pieces.get_mut(&session.seat)?.pose.rot = rot;
            return Some(Turned::Wall);
        }
        let width_after = piece.property.footprint(rot).0;
        let anchor = (piece.pose.col, piece.pose.row);
        for child in &session.carried {
            let Some(child) = self.pieces.get_mut(child) else {
                continue;
            };
            let offset = (
                child.pose.col.saturating_sub(anchor.0),
                child.pose.row.saturating_sub(anchor.1),
            );
            let depth_before = child.footprint().1;
            let (col, row) = turned_child_anchor(anchor, width_after, offset, depth_before);
            child.pose.col = col;
            child.pose.row = row;
            child.pose.rot = turned_back(child.pose.rot);
        }
        self.pieces.get_mut(&session.seat)?.pose.rot = rot;
        self.move_floor(session, (0, 0));
        if piece.property.stackable() {
            let turned = *self.pieces.get(&session.seat)?;
            let level = self.occupancy.level_under(&turned).unwrap_or(0);
            self.pieces.get_mut(&session.seat)?.pose.level = level;
        }
        Some(Turned::Floor)
    }

    /// Whether Place would accept the moving piece where it stands.
    pub fn fits(&self, session: &MoveSession) -> bool {
        self.pieces
            .get(&session.seat)
            .is_some_and(|piece| self.occupancy.fits(piece))
    }

    /// Place: registers the piece and what it carries and returns every seat to send, in seat order;
    /// `None` when the spot is refused.
    pub fn place(&mut self, session: &MoveSession) -> Option<Vec<usize>> {
        if !self.fits(session) {
            return None;
        }
        let mut seats = vec![session.seat];
        seats.extend(&session.carried);
        for seat in &seats {
            if let Some(piece) = self.pieces.get(seat).copied() {
                self.occupancy.register(&piece);
            }
        }
        seats.sort_unstable();
        Some(seats)
    }

    /// Cancel of a placed piece: it and what it carries go back to their recorded poses and the grid.
    pub fn restore(
        &mut self,
        session: &MoveSession,
        recorded: impl Fn(usize) -> Option<FurniturePose>,
    ) {
        let mut seats = vec![session.seat];
        seats.extend(&session.carried);
        for seat in seats {
            let Some(piece) = self.pieces.get_mut(&seat) else {
                continue;
            };
            if let Some(pose) = recorded(seat) {
                piece.pose = pose;
            }
            let piece = *piece;
            self.occupancy.register(&piece);
        }
    }

    /// A new piece's start: a wall piece at slot 0 with that slot's rotation; a floor piece at the
    /// selected floor piece's anchor and level (else [`NEW_PIECE_ANCHOR`]), stepped back by half its
    /// footprint.
    pub fn new_piece_pose(
        &self,
        property: &FurnitureProperty,
        selected: Option<usize>,
    ) -> FurniturePose {
        if property.wall_hung() {
            let rot = wall_slot(self.floor, 0, self.map_number).map_or(0, |slot| slot.rotation);
            return FurniturePose {
                rot,
                ..FurniturePose::default()
            };
        }
        let start = selected
            .and_then(|seat| self.pieces.get(&seat))
            .filter(|piece| !piece.property.wall_hung())
            .map_or(
                FurniturePose {
                    col: NEW_PIECE_ANCHOR.0,
                    row: NEW_PIECE_ANCHOR.1,
                    ..FurniturePose::default()
                },
                |piece| FurniturePose {
                    rot: 0,
                    ..piece.pose
                },
            );
        let (w, d) = property.footprint(0);
        let back = (-((w / 2) as i8), -((d / 2) as i8));
        self.stepped(property, start, back).0
    }

    /// The overlay drawn while `session` moves.
    pub fn overlay(&self, session: &MoveSession) -> Vec<OverlayQuad> {
        let Some(piece) = self.pieces.get(&session.seat) else {
            return Vec::new();
        };
        if piece.property.wall_hung() {
            return wall_overlay(self.floor, self.map_number, piece.pose.col);
        }
        floor_overlay(&self.occupancy, piece, self.occupancy.fits(piece))
    }
}

/// One flat overlay quad, corners in order round it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayQuad {
    pub corners: [[f32; 3]; 4],
    pub color: ColorBytes,
}

const OVERLAY_FLOOR_RISE: f32 = 0.04;
const OVERLAY_SURFACE_RISE: f32 = 0.06;
const OVERLAY_INSET: f32 = 0.05;
const WALL_OVERLAY_STANDOFF: f32 = 0.2;
/// (width along the wall, height) of a wall slot's overlay rectangle.
const WALL_OVERLAY_SIZE_1F: (f32, f32) = (3.3, 4.0);
const WALL_OVERLAY_SIZE_2F: (f32, f32) = (1.0, 0.6);
const WALL_OVERLAY_CURRENT_SCALE: f32 = 0.9;

fn cell_quad(
    grid: FloorGrid,
    col: i16,
    row: i16,
    rise: f32,
    inset: f32,
    color: ColorBytes,
) -> OverlayQuad {
    let x0 = f32::from(col - i16::from(grid.origin_col)) * CELL + inset;
    let z0 = f32::from(row - i16::from(grid.origin_row)) * CELL + inset;
    let (x1, z1) = (x0 + CELL - 2.0 * inset, z0 + CELL - 2.0 * inset);
    let y = -rise;
    OverlayQuad {
        corners: [[x0, y, z0], [x1, y, z0], [x1, y, z1], [x0, y, z1]],
        color,
    }
}

/// A floor piece's overlay: free floor cells, free surface tops for a stackable piece, then its
/// footprint coloured by whether Place would accept it.
pub fn floor_overlay(occupancy: &Occupancy, piece: &ArrangePiece, fits: bool) -> Vec<OverlayQuad> {
    let grid = occupancy.grid();
    let mut quads: Vec<OverlayQuad> = occupancy
        .free_floor_cells()
        .into_iter()
        .map(|(col, row)| cell_quad(grid, col, row, OVERLAY_FLOOR_RISE, 0.0, OVERLAY_FREE_FLOOR))
        .collect();
    if piece.property.stackable() {
        quads.extend(
            occupancy
                .free_surface_tops()
                .into_iter()
                .map(|(col, row, height)| {
                    cell_quad(
                        grid,
                        col,
                        row,
                        surface_top(height) + OVERLAY_SURFACE_RISE,
                        0.0,
                        OVERLAY_FREE_SURFACE,
                    )
                }),
        );
    }
    let color = if fits { OVERLAY_FITS } else { OVERLAY_REFUSED };
    for (col, row) in piece.cells() {
        if occupancy.cell(col, row).is_none() {
            continue;
        }
        let rise = surface_top(occupancy.cell_height(col, row)) + OVERLAY_SURFACE_RISE;
        quads.push(cell_quad(grid, col, row, rise, 0.0, color));
        if fits {
            quads.push(cell_quad(grid, col, row, rise, OVERLAY_INSET, color));
        }
    }
    quads
}

fn wall_quad(
    floor: u8,
    slot: i8,
    map_number: u16,
    scale: f32,
    color: ColorBytes,
) -> Option<OverlayQuad> {
    let [x, y, z] = wall_slot(floor, slot, map_number)?.point;
    let along_x = wall_runs_along_x(floor, slot)?;
    let (width, height) = if floor == FLOOR_2F {
        WALL_OVERLAY_SIZE_2F
    } else {
        WALL_OVERLAY_SIZE_1F
    };
    let (half_w, half_h) = (width * scale / 2.0, height * scale / 2.0);
    let (top, bottom) = (y - half_h, y + half_h);
    let corners = if along_x {
        let z = z - z.signum() * WALL_OVERLAY_STANDOFF;
        [
            [x - half_w, top, z],
            [x + half_w, top, z],
            [x + half_w, bottom, z],
            [x - half_w, bottom, z],
        ]
    } else {
        let x = x - x.signum() * WALL_OVERLAY_STANDOFF;
        [
            [x, top, z - half_w],
            [x, top, z + half_w],
            [x, bottom, z + half_w],
            [x, bottom, z - half_w],
        ]
    };
    Some(OverlayQuad { corners, color })
}

/// A wall piece's overlay: every slot of the floor 0.2 off its wall, the current one in the fitting
/// colour with a 0.9-scaled copy.
pub fn wall_overlay(floor: u8, map_number: u16, current: i8) -> Vec<OverlayQuad> {
    let count = if floor == FLOOR_2F {
        WALL_SLOTS_2F_COUNT
    } else {
        WALL_SLOTS_1F_COUNT
    };
    let mut quads = Vec::new();
    for slot in (0..count).filter_map(|slot| i8::try_from(slot).ok()) {
        if slot == current {
            continue;
        }
        quads.extend(wall_quad(floor, slot, map_number, 1.0, OVERLAY_WALL_SLOT));
    }
    quads.extend(wall_quad(
        floor,
        current,
        map_number,
        1.0,
        OVERLAY_WALL_CURRENT,
    ));
    quads.extend(wall_quad(
        floor,
        current,
        map_number,
        WALL_OVERLAY_CURRENT_SCALE,
        OVERLAY_WALL_CURRENT,
    ));
    quads
}

/// How the furniture list shows a shelf record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listed {
    PlacedHere,
    Unplaced,
}

/// The furniture list's row for a record: furnishings and mannequins, flowerpots only on 1F; a piece
/// placed on the other floor is left out.
pub fn furniture_list_row(kind: u8, installed: bool, layer: u8, floor: u8) -> Option<Listed> {
    let listed = kind == KIND_FURNISHING
        || kind == KIND_MANNEQUIN
        || (kind == KIND_FLOWERPOT && floor == FLOOR_1F);
    if !listed {
        return None;
    }
    match (installed, layer == floor) {
        (false, _) => Some(Listed::Unplaced),
        (true, true) => Some(Listed::PlacedHere),
        (true, false) => None,
    }
}

pub const MAX_PLACED_FLOWERPOTS: usize = 10;
pub const MAX_PLACED_MANNEQUINS: usize = 8;

/// Why a furniture-list pick is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickRefusal {
    Flowerpots,
    Mannequins,
}

pub fn pick_refusal(
    kind: u8,
    placed_flowerpots: usize,
    placed_mannequins: usize,
) -> Option<PickRefusal> {
    if kind == KIND_FLOWERPOT && placed_flowerpots >= MAX_PLACED_FLOWERPOTS {
        Some(PickRefusal::Flowerpots)
    } else if kind == KIND_MANNEQUIN && placed_mannequins >= MAX_PLACED_MANNEQUINS {
        Some(PickRefusal::Mannequins)
    } else {
        None
    }
}

/// Put away all is grey while the Storage list holds more than one entry.
pub fn put_away_all_greyed(storage_entries: usize) -> bool {
    storage_entries > 1
}

/// Put away is grey while Storage's free slots are fewer than the storage value of the piece and
/// everything on it.
pub fn put_away_greyed(storage_free: i64, storage_value: u64) -> bool {
    storage_free < i64::try_from(storage_value).unwrap_or(i64::MAX)
}

/// After a placed piece disappears, the piece nearest the reference point becomes the selection,
/// unless it already is the selection, which then clears.
pub fn selection_after_removal(nearest: Option<usize>, current: Option<usize>) -> Option<usize> {
    nearest.filter(|&seat| Some(seat) != current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::furniture::{FLAG_STACKABLE, FLAG_SURFACE, FLAG_WALL_HUNG, GRID_1F, GRID_2F};

    const EPSILON: f32 = 1e-4;

    fn property(flags: u8, width: u8, depth: u8, height: u16) -> FurnitureProperty {
        FurnitureProperty {
            flags,
            model: 0,
            width,
            depth,
            height,
        }
    }

    fn pose(col: i8, row: i8, level: i8, rot: i8) -> FurniturePose {
        FurniturePose {
            col,
            level,
            row,
            rot,
        }
    }

    fn piece(seat: usize, property: FurnitureProperty, pose: FurniturePose) -> ArrangePiece {
        ArrangePiece {
            seat,
            property,
            pose,
        }
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < EPSILON)
    }

    #[test]
    fn the_pulse_falls_restarts_rising_and_turns_at_its_cap() {
        let mut pulse = Pulse::default();
        assert_eq!(pulse.level(), 0x80);
        pulse.advance(2);
        assert_eq!(pulse.level(), 0x80 - 16, "8 per tick");
        let mut pulse = Pulse::default();
        pulse.advance(12);
        assert_eq!(pulse.level(), 0x20, "0x20 is not below 0x20");
        pulse.advance(1);
        assert_eq!(pulse.level(), 0x40, "below 0x20 restarts at 0x40");
        pulse.advance(1);
        assert_eq!(pulse.level(), 0x48, "now rising");
        pulse.advance(100);
        assert_eq!(pulse.level(), 0xFF, "capped");
        pulse.advance(1);
        assert_eq!(pulse.level(), 0xFF - 8, "falling again");
        assert_eq!(pulse.tint(PULSE_ALPHA), [0xF7, 0xF7, 0xF7, PULSE_ALPHA]);
        assert_eq!(PULSE_ALPHA_MANNEQUIN, TINT_NORMAL[3]);
    }

    #[test]
    fn views_cycle_and_only_from_north_flips_presses() {
        let view = CameraView::default();
        assert_eq!(view, CameraView::TopDown);
        assert_eq!(view.next(), CameraView::FromSouth);
        assert_eq!(view.next().next(), CameraView::FromNorth);
        assert_eq!(view.next().next().next(), CameraView::TopDown);
        assert!(CameraView::FromNorth.flips_presses());
        assert!(!CameraView::FromSouth.flips_presses() && !CameraView::TopDown.flips_presses());
    }

    #[test]
    fn camera_targets_clamp_the_focus_and_place_the_eye_per_view() {
        assert!(close(
            focus_target(None, CameraView::FromSouth, FLOOR_1F),
            [0.0, -0.3, 0.0]
        ));
        let far = Some([9.0, -1.0, -9.0]);
        assert!(close(
            focus_target(far, CameraView::TopDown, FLOOR_1F),
            [4.0, -0.3, -5.0]
        ));
        assert!(close(
            focus_target(far, CameraView::FromSouth, FLOOR_2F),
            [4.0, -1.3, -6.0]
        ));
        let focus = [2.0, -0.3, 3.0];
        assert!(close(
            eye_target(focus, CameraView::TopDown),
            [2.0, -5.0, 2.9]
        ));
        assert!(close(
            eye_target(focus, CameraView::FromSouth),
            [1.4, -4.5, -1.0]
        ));
        assert!(close(
            eye_target(focus, CameraView::FromNorth),
            [1.4, -4.5, 5.2]
        ));
        assert!(close(
            eye_target([0.0, -0.3, -3.0], CameraView::FromSouth),
            [0.0, -4.5, -5.2]
        ));
    }

    #[test]
    fn the_rig_eases_a_third_and_holds_the_top_down_distance() {
        let mut rig = CameraRig {
            eye: [0.0, -4.5, -4.0],
            focus: [0.0, -0.3, 0.0],
        };
        rig.update(None, CameraView::FromSouth, FLOOR_1F);
        let eye = eye_target(
            focus_target(None, CameraView::FromSouth, FLOOR_1F),
            CameraView::FromSouth,
        );
        assert!(close(rig.eye, lerp3([0.0, -4.5, -4.0], eye, 0.3)));
        assert!(!rig.reverses_type1_presses());

        let mut rig = CameraRig {
            eye: [0.0, -2.0, 0.0],
            focus: [0.0, -0.3, 0.0],
        };
        rig.update(None, CameraView::TopDown, FLOOR_2F);
        let reach: f32 = [0, 1, 2]
            .map(|i| rig.eye[i] - rig.focus[i])
            .iter()
            .map(|v| v * v)
            .sum::<f32>()
            .sqrt();
        assert!((reach - TOP_DOWN_DISTANCE_2F).abs() < EPSILON);

        let north = CameraRig {
            eye: [0.0, -4.5, 3.0],
            focus: [0.0, -0.3, 3.0],
        };
        assert!(north.reverses_type1_presses(), "level counts as north");
    }

    #[test]
    fn presses_step_the_floor_grid_and_reverse_together() {
        assert_eq!(floor_step(Press::Up, false), (0, 1));
        assert_eq!(floor_step(Press::Right, false), (1, 0));
        assert_eq!(floor_step(Press::Down, false), (0, -1));
        assert_eq!(floor_step(Press::Left, false), (-1, 0));
        for press in Press::ALL {
            let (c, r) = floor_step(press, false);
            assert_eq!(floor_step(press, true), (-c, -r));
        }
    }

    fn nav(seat: usize, point: [f32; 3]) -> NavPiece {
        NavPiece {
            seat,
            point,
            level: 0,
            wall_hung: false,
            rot: 0,
            footprint: (1, 1),
            carries: false,
        }
    }

    #[test]
    fn a_wall_piece_is_pushed_by_its_height_toward_its_rotation() {
        let wall = NavPiece {
            wall_hung: true,
            ..nav(1, [1.0, -3.0, 2.0])
        };
        let at = |rot| nav_point(&NavPiece { rot, ..wall });
        assert_eq!(at(0), (1.0, 5.0));
        assert_eq!(at(1), (1.0, -1.0));
        assert_eq!(at(2), (4.0, 2.0));
        assert_eq!(at(3), (-2.0, 2.0));
        assert_eq!(at(-1), (-2.0, 2.0));
        assert_eq!(nav_point(&nav(2, [1.0, -3.0, 2.0])), (1.0, 2.0));
    }

    #[test]
    fn selection_takes_the_nearest_piece_past_the_threshold_in_the_pressed_direction() {
        let selected = NavPiece {
            footprint: (2, 1),
            ..nav(1, [0.0, 0.0, 0.0])
        };
        let pieces = [
            selected,
            nav(2, [0.5, 0.0, 0.0]),
            nav(3, [1.0, 0.0, 0.2]),
            nav(4, [3.0, 0.0, 0.0]),
            nav(5, [-2.0, 0.0, 0.0]),
            nav(6, [0.1, 0.0, 2.0]),
        ];
        assert_eq!(
            navigate(&selected, &pieces, Press::Right.delta()),
            Some(3),
            "0.5 is not past the half width 0.6"
        );
        assert_eq!(navigate(&selected, &pieces, Press::Left.delta()), Some(5));
        assert_eq!(navigate(&selected, &pieces, Press::Up.delta()), Some(6));
        assert_eq!(navigate(&selected, &pieces, Press::Down.delta()), None);

        let carrying = NavPiece {
            carries: true,
            ..selected
        };
        assert_eq!(
            navigate(&carrying, &pieces, Press::Right.delta()),
            Some(2),
            "no threshold for a piece something stands on"
        );
        let both = navigate(&selected, &pieces, (1, 1));
        assert_eq!(both, None, "two arrows need |dx| = |dz| exactly");
    }

    #[test]
    fn selection_jumps_to_a_piece_on_the_same_spot_and_prefers_floor_pieces_in_a_near_tie() {
        let selected = nav(1, [0.0, 0.0, 0.0]);
        let raised = NavPiece {
            level: 10,
            ..nav(2, [0.05, -1.0, 0.0])
        };
        assert_eq!(
            navigate(&selected, &[selected, raised], (0, 0)),
            Some(2),
            "|dx| + |dz| < 0.1 wins at once"
        );
        let on_top = NavPiece {
            level: 5,
            ..selected
        };
        assert_eq!(navigate(&on_top, &[on_top, raised], (0, 0)), None);

        let near_raised = NavPiece {
            level: 10,
            ..nav(3, [1.0, -1.0, 0.0])
        };
        let floor = nav(4, [1.04, 0.0, 0.0]);
        assert_eq!(
            navigate(&selected, &[selected, near_raised, floor], (1, 0)),
            Some(4)
        );
        let far_floor = nav(5, [1.5, 0.0, 0.0]);
        assert_eq!(
            navigate(&selected, &[selected, near_raised, far_floor], (1, 0)),
            Some(3)
        );
    }

    #[test]
    fn anchors_clamp_inside_the_floor() {
        assert_eq!(clamp_anchor(GRID_1F, (2, 3), -4, 30), (0, 21));
        assert_eq!(clamp_anchor(GRID_2F, (1, 1), 25, 25), (19, 25));
        assert_eq!(clamp_anchor(GRID_1F, (1, 1), 5, 5), (5, 5));
    }

    #[test]
    fn first_floor_wall_slots_step_by_table() {
        let expect: [(i8, [i8; 4]); 5] = [
            (0, [0, 0, 1, 4]),
            (1, [0, 1, 1, 2]),
            (2, [3, 1, 2, 2]),
            (3, [4, 0, 2, 3]),
            (4, [4, 0, 3, 4]),
        ];
        for (slot, steps) in expect {
            for (press, to) in Press::ALL.into_iter().zip(steps) {
                assert_eq!(
                    wall_step(FLOOR_1F, slot, press),
                    to,
                    "slot {slot} {press:?}"
                );
            }
        }
    }

    fn place(wall: Wall2F, column: u8, height: u8) -> i8 {
        wall_slot_2f(WallPlace2F {
            wall,
            column,
            height,
        })
    }

    #[test]
    fn second_floor_wall_slots_step_along_and_round_the_walls() {
        use Wall2F::*;
        let step = |from, press| wall_place_2f(wall_step(FLOOR_2F, from, press)).unwrap();
        let at = |wall, column, height| WallPlace2F {
            wall,
            column,
            height,
        };
        assert_eq!(step(place(West, 3, 1), Press::Up), at(West, 4, 1));
        assert_eq!(step(place(West, 8, 1), Press::Up), at(North, 0, 1));
        assert_eq!(step(place(West, 2, 3), Press::Right), at(East, 2, 3));
        assert_eq!(step(place(West, 0, 2), Press::Down), at(South, 0, 2));
        assert_eq!(step(place(West, 4, 0), Press::Left), at(West, 4, 0));
        assert_eq!(step(place(East, 8, 2), Press::Up), at(North, 2, 2));
        assert_eq!(step(place(East, 5, 0), Press::Right), at(East, 5, 0));
        assert_eq!(step(place(East, 0, 1), Press::Down), at(South, 1, 1));
        assert_eq!(step(place(East, 6, 3), Press::Left), at(West, 6, 3));
        assert_eq!(step(place(North, 1, 0), Press::Up), at(North, 1, 0));
        assert_eq!(step(place(North, 2, 1), Press::Right), at(East, 8, 1));
        assert_eq!(step(place(North, 0, 3), Press::Down), at(South, 0, 3));
        assert_eq!(step(place(North, 1, 3), Press::Down), at(North, 1, 3));
        assert_eq!(step(place(North, 2, 3), Press::Down), at(South, 1, 3));
        assert_eq!(step(place(North, 0, 2), Press::Left), at(West, 8, 2));
        assert_eq!(step(place(South, 0, 3), Press::Up), at(North, 0, 3));
        assert_eq!(step(place(South, 1, 3), Press::Up), at(North, 2, 3));
        assert_eq!(step(place(South, 1, 2), Press::Right), at(East, 0, 2));
        assert_eq!(step(place(South, 0, 0), Press::Down), at(South, 0, 0));
        assert_eq!(step(place(South, 0, 1), Press::Left), at(West, 0, 1));
        assert_eq!(step(place(South, 0, 1), Press::Right), at(South, 1, 1));
    }

    #[test]
    fn a_turn_goes_back_a_quarter_and_carries_a_child_round_its_parent() {
        assert_eq!(turned_back(0), 3);
        assert_eq!(turned_back(3), 2);
        assert_eq!(turned_child_anchor((4, 6), 2, (1, 0), 1), (5, 7));
        assert_eq!(turned_child_anchor((4, 6), 3, (0, 2), 1), (4, 6));
    }

    fn room(floor: u8) -> Room {
        Room::new(floor, 0).unwrap()
    }

    #[test]
    fn the_first_floor_grid_blocks_its_middle_rectangle() {
        let occupancy = Occupancy::new(FLOOR_1F).unwrap();
        assert_eq!(occupancy.cell(6, 0), Some(Cell::Blocked));
        assert_eq!(occupancy.cell(13, 17), Some(Cell::Blocked));
        assert_eq!(occupancy.cell(14, 17), Some(Cell::Free));
        assert_eq!(occupancy.cell(6, 18), Some(Cell::Free));
        assert_eq!(occupancy.cell(20, 0), None);
        let upstairs = Occupancy::new(FLOOR_2F).unwrap();
        assert_eq!(upstairs.cell(6, 0), Some(Cell::Free));
        assert!(Occupancy::new(2).is_none());
    }

    #[test]
    fn place_validity_follows_the_cell_rules() {
        let table = property(FLAG_SURFACE, 2, 2, 105);
        let chair = property(0, 1, 1, 0);
        let vase = property(FLAG_STACKABLE, 1, 1, 0);
        let mut occupancy = Occupancy::new(FLOOR_1F).unwrap();
        occupancy.register(&piece(1, table, pose(1, 1, 0, 0)));
        occupancy.register(&piece(2, chair, pose(4, 4, 0, 0)));

        assert!(occupancy.fits(&piece(3, chair, pose(3, 1, 0, 0))));
        assert!(
            !occupancy.fits(&piece(3, chair, pose(2, 2, 0, 0))),
            "under a surface"
        );
        assert!(
            !occupancy.fits(&piece(3, chair, pose(4, 4, 0, 0))),
            "under a piece"
        );
        assert!(
            !occupancy.fits(&piece(3, chair, pose(7, 3, 0, 0))),
            "blocked"
        );
        assert!(
            !occupancy.fits(&piece(3, chair, pose(20, 0, 0, 0))),
            "outside"
        );

        let on_table = piece(3, vase, pose(2, 2, 10, 0));
        assert!(occupancy.fits(&on_table));
        assert!(
            !occupancy.fits(&piece(3, vase, pose(4, 4, 10, 0))),
            "no surface"
        );
        occupancy.register(&on_table);
        assert!(
            !occupancy.fits(&piece(4, vase, pose(2, 2, 10, 0))),
            "top taken"
        );
        assert_eq!(occupancy.carried_by(1), &[3]);

        let wide = property(FLAG_STACKABLE, 2, 1, 0);
        occupancy.register(&piece(5, table, pose(3, 1, 0, 0)));
        assert!(
            !occupancy.fits(&piece(4, wide, pose(2, 1, 10, 0))),
            "two different surfaces"
        );
        assert!(occupancy.fits(&piece(4, wide, pose(3, 1, 10, 0))));

        let mirror = property(FLAG_WALL_HUNG, 1, 1, 0);
        occupancy.register(&piece(6, mirror, pose(2, 0, 0, 3)));
        assert!(!occupancy.fits(&piece(7, mirror, pose(2, 0, 0, 3))));
        assert!(occupancy.fits(&piece(7, mirror, pose(3, 0, 0, 3))));
        assert!(
            !occupancy.fits(&piece(7, mirror, pose(5, 0, 0, 3))),
            "no slot 5 on 1F"
        );

        occupancy.unregister(1);
        assert_eq!(occupancy.cell(1, 1), Some(Cell::Free));
        assert!(occupancy.carried_by(1).is_empty());
        occupancy.unregister(6);
        assert!(occupancy.fits(&piece(7, mirror, pose(2, 0, 0, 3))));
    }

    #[test]
    fn a_surface_carries_at_most_thirty_two_pieces() {
        let wide = property(FLAG_SURFACE, 20, 2, 50);
        let cup = property(FLAG_STACKABLE, 1, 1, 0);
        let mut occupancy = Occupancy::new(FLOOR_2F).unwrap();
        occupancy.register(&piece(0, wide, pose(0, 0, 0, 0)));
        let mut seat = 1;
        for row in 0..2 {
            for col in 0..20 {
                occupancy.register(&piece(seat, cup, pose(col, row, 5, 0)));
                seat += 1;
            }
        }
        assert_eq!(occupancy.carried_by(0).len(), MAX_PIECES_ON_SURFACE);
    }

    fn table_room() -> (Room, FurnitureProperty, FurnitureProperty) {
        let table = property(FLAG_SURFACE, 2, 1, 105);
        let vase = property(FLAG_STACKABLE, 1, 1, 0);
        let mut room = room(FLOOR_2F);
        room.insert(piece(1, table, pose(4, 4, 0, 0)));
        room.insert(piece(2, vase, pose(5, 4, 10, 0)));
        (room, table, vase)
    }

    #[test]
    fn moving_a_surface_carries_what_stands_on_it_and_clamps_at_the_wall() {
        let (mut room, ..) = table_room();
        let session = room.begin_move(1);
        assert_eq!(session.carried, vec![2]);
        assert!(room.occupancy().carried_by(1).is_empty(), "unregistered");
        assert!(room.step(&session, Press::Right, false));
        assert_eq!(room.piece(1).unwrap().pose, pose(5, 4, 0, 0));
        assert_eq!(room.piece(2).unwrap().pose, pose(6, 4, 10, 0), "level kept");
        assert!(
            room.step(&session, Press::Up, true),
            "reversed: Up is row - 1"
        );
        assert_eq!(room.piece(1).unwrap().pose, pose(5, 3, 0, 0));
        for _ in 0..30 {
            room.step(&session, Press::Right, false);
        }
        assert_eq!(room.piece(1).unwrap().pose.col, 18, "20 - width 2");
        assert!(
            !room.step(&session, Press::Right, false),
            "held at the wall"
        );
        assert_eq!(room.piece(2).unwrap().pose.col, 19);
    }

    #[test]
    fn a_stackable_piece_rises_onto_a_surface_and_back_down() {
        let (mut room, ..) = table_room();
        let cup = property(FLAG_STACKABLE, 1, 1, 0);
        room.insert(piece(3, cup, pose(3, 4, 0, 0)));
        let session = room.begin_move(3);
        assert!(room.step(&session, Press::Right, false));
        assert_eq!(
            room.piece(3).unwrap().pose,
            pose(4, 4, 10, 0),
            "height 105 / 10"
        );
        assert!(room.fits(&session), "free top of the table");
        assert!(room.step(&session, Press::Up, false));
        assert_eq!(room.piece(3).unwrap().pose.level, 0, "off the table");
        let chair = property(0, 1, 1, 0);
        room.insert(piece(4, chair, pose(1, 1, 0, 0)));
        let session = room.begin_move(4);
        room.step(&session, Press::Right, false);
        room.step(&session, Press::Right, false);
        room.step(&session, Press::Right, false);
        assert_eq!(room.piece(4).unwrap().pose.level, 0, "only stackables rise");
    }

    #[test]
    fn a_turned_surface_swaps_its_footprint_and_carries_its_children_round() {
        let (mut room, ..) = table_room();
        let session = room.begin_move(1);
        assert_eq!(room.turn(&session), Some(Turned::Floor));
        let table = room.piece(1).unwrap();
        assert_eq!(table.pose, pose(4, 4, 0, 3));
        assert_eq!(table.footprint(), (1, 2));
        assert_eq!(
            room.piece(2).unwrap().pose,
            pose(4, 5, 10, 3),
            "anchor + (W' - drow - d, dcol) = (4 + 1 - 0 - 1, 4 + 1)"
        );
    }

    #[test]
    fn a_turn_at_the_far_wall_clamps_back_inside() {
        let long = property(0, 1, 3, 0);
        let mut room = room(FLOOR_1F);
        room.insert(piece(1, long, pose(19, 0, 0, 0)));
        let session = room.begin_move(1);
        room.turn(&session);
        assert_eq!(room.piece(1).unwrap().pose, pose(17, 0, 0, 3));
    }

    /// Attr bytes far outside the room step and turn without overflowing: the anchor comes back
    /// inside the floor and the rotation keeps its quarter.
    #[test]
    fn malformed_attr_bytes_step_and_turn_without_overflow() {
        assert_eq!(
            turned_back(i8::MIN),
            turned_back(i8::MIN.rem_euclid(QUARTER_TURNS))
        );
        let mut room = room(FLOOR_2F);
        room.insert(piece(
            1,
            property(0, 1, 1, 0),
            pose(i8::MIN, i8::MAX, 0, i8::MIN),
        ));
        let session = room.begin_move(1);
        assert!(room.step(&session, Press::Right, false));
        assert_eq!(room.turn(&session), Some(Turned::Floor));
        let held = room.piece(1).unwrap().pose;
        assert!(
            room.occupancy()
                .cell(held.col.into(), held.row.into())
                .is_some(),
            "back inside the floor"
        );
    }

    #[test]
    fn wall_pieces_step_by_slot_take_its_rotation_and_turn_in_place() {
        let mirror = property(FLAG_WALL_HUNG, 1, 1, 0);
        let mut room = room(FLOOR_1F);
        room.insert(piece(1, mirror, pose(0, 0, 0, 1)));
        let session = room.begin_move(1);
        assert!(!room.step(&session, Press::Up, false), "slot 0 Up stays");
        assert!(room.step(&session, Press::Left, false));
        assert_eq!(
            room.piece(1).unwrap().pose,
            pose(4, 0, 0, 3),
            "slot 4 rotation"
        );
        assert_eq!(room.turn(&session), Some(Turned::Wall));
        assert_eq!(room.piece(1).unwrap().pose, pose(4, 0, 0, 2));
        assert!(room.fits(&session));
        assert_eq!(room.place(&session), Some(vec![1]));
    }

    #[test]
    fn place_registers_the_piece_and_its_load_or_refuses() {
        let (mut room, ..) = table_room();
        let chair = property(0, 1, 1, 0);
        room.insert(piece(9, chair, pose(8, 4, 0, 0)));
        let session = room.begin_move(1);
        room.step(&session, Press::Right, false);
        room.step(&session, Press::Right, false);
        room.step(&session, Press::Right, false);
        assert!(
            !room.fits(&session),
            "the table's right cell is on the chair"
        );
        assert_eq!(room.place(&session), None);
        room.step(&session, Press::Right, false);
        assert_eq!(room.piece(1).unwrap().pose.col, 8);
        assert_eq!(room.place(&session), None, "still on the chair");
        room.step(&session, Press::Up, false);
        assert_eq!(room.place(&session), Some(vec![1, 2]));
        assert_eq!(room.occupancy().carried_by(1), &[2]);
    }

    #[test]
    fn cancel_puts_the_piece_and_its_load_back_where_their_records_say() {
        let (mut room, ..) = table_room();
        let recorded = |seat| match seat {
            1 => Some(pose(4, 4, 0, 0)),
            2 => Some(pose(5, 4, 10, 0)),
            _ => None,
        };
        let session = room.begin_move(1);
        room.step(&session, Press::Up, false);
        room.turn(&session);
        room.restore(&session, recorded);
        assert_eq!(room.piece(1).unwrap().pose, pose(4, 4, 0, 0));
        assert_eq!(room.piece(2).unwrap().pose, pose(5, 4, 10, 0));
        assert_eq!(room.occupancy().carried_by(1), &[2]);
    }

    #[test]
    fn a_new_piece_starts_half_its_footprint_back_from_the_selection_or_the_room_spot() {
        let (room, ..) = table_room();
        let bed = property(0, 3, 2, 0);
        assert_eq!(
            room.new_piece_pose(&bed, None),
            pose(NEW_PIECE_ANCHOR.0 - 1, NEW_PIECE_ANCHOR.1 - 1, 0, 0)
        );
        assert_eq!(room.new_piece_pose(&bed, Some(1)), pose(3, 3, 0, 0));
        let cup = property(FLAG_STACKABLE, 1, 1, 0);
        assert_eq!(
            room.new_piece_pose(&cup, Some(2)),
            pose(5, 4, 10, 0),
            "no step: the selection's level stays"
        );
        let mirror = property(FLAG_WALL_HUNG, 1, 1, 0);
        let downstairs = Room::new(FLOOR_1F, 0).unwrap();
        assert_eq!(
            downstairs.new_piece_pose(&mirror, Some(1)),
            pose(0, 0, 0, 1)
        );
        let corner = property(0, 4, 4, 0);
        let mut low = Room::new(FLOOR_1F, 0).unwrap();
        low.insert(piece(1, property(0, 1, 1, 0), pose(0, 0, 0, 2)));
        assert_eq!(
            low.new_piece_pose(&corner, Some(1)),
            pose(0, 0, 0, 0),
            "clamped"
        );
    }

    #[test]
    fn the_floor_overlay_paints_free_cells_surface_tops_and_the_footprint() {
        let (mut room, ..) = table_room();
        let cup = property(FLAG_STACKABLE, 1, 1, 0);
        room.insert_unregistered(piece(3, cup, pose(4, 4, 10, 0)));
        let session = MoveSession {
            seat: 3,
            carried: Vec::new(),
        };
        let quads = room.overlay(&session);
        let free_cells = room.occupancy().free_floor_cells().len();
        let colors = |color| quads.iter().filter(|q| q.color == color).count();
        assert_eq!(colors(OVERLAY_FREE_FLOOR), free_cells);
        assert_eq!(colors(OVERLAY_FREE_SURFACE), 1, "the table's other top");
        assert_eq!(colors(OVERLAY_FITS), 2, "footprint plus its inset copy");
        let top = quads
            .iter()
            .find(|q| q.color == OVERLAY_FITS)
            .unwrap()
            .corners[0];
        assert!(
            (top[1] + (1.05 + 0.06)).abs() < EPSILON,
            "0.06 above the 1.05 top"
        );
        let first = quads[0].corners;
        assert!((first[0][1] + 0.04).abs() < EPSILON);
        assert!((first[1][0] - first[0][0] - CELL).abs() < EPSILON);

        room.insert_unregistered(piece(3, cup, pose(4, 4, 10, 0)));
        let chair = property(0, 1, 1, 0);
        room.insert_unregistered(piece(4, chair, pose(4, 4, 0, 0)));
        let refused = room.overlay(&MoveSession {
            seat: 4,
            carried: Vec::new(),
        });
        assert_eq!(
            refused
                .iter()
                .filter(|q| q.color == OVERLAY_REFUSED)
                .count(),
            1
        );
        assert!(refused.iter().all(|q| q.color != OVERLAY_FREE_SURFACE));
    }

    #[test]
    fn the_wall_overlay_shows_every_slot_and_doubles_the_current_one() {
        let quads = wall_overlay(FLOOR_1F, 0, 2);
        assert_eq!(quads.len(), WALL_SLOTS_1F_COUNT + 1);
        let current: Vec<_> = quads
            .iter()
            .filter(|q| q.color == OVERLAY_WALL_CURRENT)
            .collect();
        assert_eq!(current.len(), 2);
        let corners = current[0].corners;
        assert!(
            (corners[0][0] - (-6.5 + 0.2)).abs() < EPSILON,
            "0.2 off the west wall"
        );
        assert!((corners[1][2] - corners[0][2] - 3.3).abs() < EPSILON);
        assert!((corners[2][1] - corners[1][1] - 4.0).abs() < EPSILON);
        let scaled = current[1].corners;
        assert!((scaled[1][2] - scaled[0][2] - 3.3 * 0.9).abs() < EPSILON);

        let upstairs = wall_overlay(FLOOR_2F, 0, place(Wall2F::North, 1, 2));
        assert_eq!(upstairs.len(), WALL_SLOTS_2F_COUNT + 1);
        let north = upstairs
            .iter()
            .find(|q| q.color == OVERLAY_WALL_CURRENT)
            .unwrap()
            .corners;
        assert!(
            (north[0][2] - (8.0 - 0.2)).abs() < EPSILON,
            "0.2 off the north wall"
        );
        assert!((north[1][0] - north[0][0] - 1.0).abs() < EPSILON);
    }

    #[test]
    fn the_furniture_list_shows_furniture_kinds_not_placed_upstairs_or_down() {
        assert_eq!(
            furniture_list_row(KIND_FURNISHING, false, FLOOR_2F, FLOOR_1F),
            Some(Listed::Unplaced)
        );
        assert_eq!(
            furniture_list_row(KIND_MANNEQUIN, true, FLOOR_2F, FLOOR_2F),
            Some(Listed::PlacedHere)
        );
        assert_eq!(
            furniture_list_row(KIND_FURNISHING, true, FLOOR_1F, FLOOR_2F),
            None
        );
        assert_eq!(
            furniture_list_row(KIND_FLOWERPOT, false, FLOOR_1F, FLOOR_1F),
            Some(Listed::Unplaced)
        );
        assert_eq!(
            furniture_list_row(KIND_FLOWERPOT, false, FLOOR_1F, FLOOR_2F),
            None,
            "flowerpots only on 1F"
        );
        assert_eq!(furniture_list_row(KIND_FURNISHING - 1, false, 0, 0), None);
    }

    #[test]
    fn picks_over_the_limits_are_refused_and_menu_rows_grey_by_storage() {
        assert_eq!(
            pick_refusal(KIND_FLOWERPOT, MAX_PLACED_FLOWERPOTS, 0),
            Some(PickRefusal::Flowerpots)
        );
        assert_eq!(
            pick_refusal(KIND_FLOWERPOT, MAX_PLACED_FLOWERPOTS - 1, 0),
            None
        );
        assert_eq!(
            pick_refusal(KIND_MANNEQUIN, 0, MAX_PLACED_MANNEQUINS),
            Some(PickRefusal::Mannequins)
        );
        assert_eq!(
            pick_refusal(
                KIND_FURNISHING,
                MAX_PLACED_FLOWERPOTS,
                MAX_PLACED_MANNEQUINS
            ),
            None
        );
        assert!(!put_away_all_greyed(1));
        assert!(put_away_all_greyed(2));
        assert!(put_away_greyed(2, 3));
        assert!(!put_away_greyed(3, 3));
    }

    #[test]
    fn after_a_removal_the_nearest_piece_is_selected_unless_it_already_was() {
        let (room, ..) = table_room();
        let near_table = room.nearest((1.0, 1.0));
        assert_eq!(near_table, Some(2));
        assert_eq!(selection_after_removal(near_table, Some(7)), Some(2));
        assert_eq!(selection_after_removal(near_table, Some(2)), None);
        assert_eq!(selection_after_removal(None, Some(2)), None);
    }
}
