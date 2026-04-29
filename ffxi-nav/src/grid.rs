//! 2D occupancy-grid navigation.
//!
//! Maps world (`Vec3.x`, `Vec3.z`) coordinates onto a row-major boolean
//! grid loaded from a greyscale PNG. Pathfinding uses A* with 8-connected
//! neighbors; cardinal cost is 100 and diagonal cost is 141 (≈ √2 · 100,
//! kept integral so the `pathfinding` crate's default ordering is happy).
//!
//! `Vec3.y` is intentionally ignored at this stage — cliff-aware (3D)
//! navigation is Phase 10b. Two points that share `(x, z)` but differ in
//! `y` resolve to the same grid cell.

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use glam::{Vec2, Vec3};
use pathfinding::prelude::astar;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::NavMesh;

/// Cost of a cardinal (N/S/E/W) move; chosen integral so the diagonal
/// cost (≈ √2 · cardinal) can also stay integral without truncation
/// destroying the heuristic-admissibility invariant for A*.
const COST_CARDINAL: u32 = 100;
/// Cost of a diagonal move. 141 ≈ √2 · 100; very slightly over-estimates
/// √2 (which is 141.421…), which is harmless for A* — the heuristic
/// stays admissible because we use a Chebyshev-style lower bound below.
const COST_DIAGONAL: u32 = 141;

/// Errors produced when loading, building, or persisting a [`GridNav`].
#[derive(Debug, Error)]
pub enum NavError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("image decode error: {0}")]
    Decode(#[from] image::ImageError),

    #[error("bincode error: {0}")]
    BincodeDecode(#[from] bincode::Error),

    #[error("coordinate is outside the grid")]
    OutOfBounds,

    #[error("image has zero width or height")]
    EmptyImage,
}

/// 2D occupancy grid backed by a flat row-major `Vec<bool>`.
///
/// `walkable[row * width + col] == true` means the cell at `(col, row)`
/// is traversable. Conversion between world `(x, z)` and grid `(col,
/// row)` uses `origin_world` (the world coordinate of the grid's `(0,
/// 0)` cell) and `cell_size` (world units per cell edge).
#[derive(Debug, Clone)]
pub struct GridNav {
    width: u32,
    height: u32,
    walkable: Vec<bool>,
    origin_world: Vec2,
    cell_size: f32,
}

/// On-disk representation of [`GridNav`]. Kept separate so we don't have
/// to derive `Serialize`/`Deserialize` on the public type — this gives
/// us room to add non-serializable fields (e.g. an A* scratch buffer)
/// later without breaking the cache format.
#[derive(Debug, Serialize, Deserialize)]
struct GridNavOnDisk {
    width: u32,
    height: u32,
    walkable: Vec<bool>,
    origin_world: [f32; 2],
    cell_size: f32,
}

impl GridNav {
    /// Build a grid from a greyscale PNG. Pixels with luminance strictly
    /// less than `threshold` are walkable; pixels at or above the
    /// threshold are blocked. (Convention: dark = walkable, light =
    /// wall, matching how navmesh sketches are usually drawn.)
    pub fn from_png(
        path: &Path,
        threshold: u8,
        origin_world: Vec2,
        cell_size: f32,
    ) -> Result<Self, NavError> {
        let img = image::open(path)?.to_luma8();
        let (w, h) = img.dimensions();
        if w == 0 || h == 0 {
            return Err(NavError::EmptyImage);
        }
        let walkable = img.pixels().map(|p| p.0[0] < threshold).collect();
        Ok(Self::from_walkable(w, h, walkable, origin_world, cell_size))
    }

    /// Build a grid from a pre-computed walkability mask. Panics if
    /// `walkable.len() != width * height`; this is a programmer error,
    /// not a runtime condition.
    pub fn from_walkable(
        width: u32,
        height: u32,
        walkable: Vec<bool>,
        origin_world: Vec2,
        cell_size: f32,
    ) -> Self {
        assert_eq!(
            walkable.len(),
            (width as usize) * (height as usize),
            "walkable length must equal width * height",
        );
        Self {
            width,
            height,
            walkable,
            origin_world,
            cell_size,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn cell_size(&self) -> f32 {
        self.cell_size
    }
    pub fn origin_world(&self) -> Vec2 {
        self.origin_world
    }

    /// Persist this grid to disk via bincode. Format is field-by-field
    /// (see [`GridNavOnDisk`]); not stable across major versions of
    /// `ffxi-nav`.
    pub fn save_cache(&self, path: &Path) -> Result<(), NavError> {
        let on_disk = GridNavOnDisk {
            width: self.width,
            height: self.height,
            walkable: self.walkable.clone(),
            origin_world: [self.origin_world.x, self.origin_world.y],
            cell_size: self.cell_size,
        };
        let f = File::create(path)?;
        let mut w = BufWriter::new(f);
        bincode::serialize_into(&mut w, &on_disk)?;
        Ok(())
    }

    /// Load a grid previously written with [`Self::save_cache`].
    pub fn load_cache(path: &Path) -> Result<Self, NavError> {
        let f = File::open(path)?;
        let r = BufReader::new(f);
        let on_disk: GridNavOnDisk = bincode::deserialize_from(r)?;
        Ok(Self::from_walkable(
            on_disk.width,
            on_disk.height,
            on_disk.walkable,
            Vec2::new(on_disk.origin_world[0], on_disk.origin_world[1]),
            on_disk.cell_size,
        ))
    }

    /// `true` if `(col, row)` is inside the grid and marked walkable.
    fn is_walkable(&self, col: i32, row: i32) -> bool {
        if col < 0 || row < 0 || col >= self.width as i32 || row >= self.height as i32 {
            return false;
        }
        let idx = (row as usize) * (self.width as usize) + (col as usize);
        self.walkable[idx]
    }

    /// World `(x, z)` → grid `(col, row)`. Rounds to the nearest cell.
    fn world_to_cell(&self, x: f32, z: f32) -> (i32, i32) {
        let col = ((x - self.origin_world.x) / self.cell_size).round() as i32;
        let row = ((z - self.origin_world.y) / self.cell_size).round() as i32;
        (col, row)
    }

    /// Grid `(col, row)` → world `(x, y=0, z)`. `y` is left at zero
    /// because Phase 10a is height-agnostic; callers that need a real
    /// `y` should sample their own heightmap.
    fn cell_to_world(&self, col: i32, row: i32) -> Vec3 {
        let x = self.origin_world.x + (col as f32) * self.cell_size;
        let z = self.origin_world.y + (row as f32) * self.cell_size;
        Vec3::new(x, 0.0, z)
    }

    /// 8-connected neighbors of `(col, row)` whose cells are walkable,
    /// each paired with the cost of the move into that neighbor.
    fn neighbors(&self, &(col, row): &(i32, i32)) -> Vec<((i32, i32), u32)> {
        let mut out = Vec::with_capacity(8);
        for (dc, dr) in [
            (-1, -1),
            (0, -1),
            (1, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (0, 1),
            (1, 1),
        ] {
            let nc = col + dc;
            let nr = row + dr;
            if !self.is_walkable(nc, nr) {
                continue;
            }
            // Forbid corner-cutting through walls: a diagonal move is
            // only legal if both adjacent cardinal cells are also
            // walkable. Without this, paths squeeze through diagonal
            // gaps the agent physically can't fit through.
            if dc != 0
                && dr != 0
                && (!self.is_walkable(col + dc, row) || !self.is_walkable(col, row + dr))
            {
                continue;
            }
            let cost = if dc == 0 || dr == 0 {
                COST_CARDINAL
            } else {
                COST_DIAGONAL
            };
            out.push(((nc, nr), cost));
        }
        out
    }

    /// Octile-distance heuristic — admissible lower bound on the path
    /// cost given our 8-connected cost model. Prefer this over Manhattan
    /// (which over-estimates with diagonals) and over Euclidean (which
    /// under-estimates by being too loose).
    fn heuristic(&self, &(col, row): &(i32, i32), goal: &(i32, i32)) -> u32 {
        let dx = (col - goal.0).unsigned_abs();
        let dy = (row - goal.1).unsigned_abs();
        let (min, max) = if dx < dy { (dx, dy) } else { (dy, dx) };
        COST_DIAGONAL * min + COST_CARDINAL * (max - min)
    }
}

impl NavMesh for GridNav {
    fn path(&self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>> {
        let start = self.world_to_cell(from.x, from.z);
        let goal = self.world_to_cell(to.x, to.z);
        if !self.is_walkable(start.0, start.1) || !self.is_walkable(goal.0, goal.1) {
            return None;
        }
        let (cells, _cost) = astar(
            &start,
            |c| self.neighbors(c),
            |c| self.heuristic(c, &goal),
            |c| *c == goal,
        )?;
        Some(
            cells
                .into_iter()
                .map(|(c, r)| self.cell_to_world(c, r))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an `n × n` grid where every cell is walkable.
    fn open_grid(n: u32) -> GridNav {
        let cells = (n * n) as usize;
        GridNav::from_walkable(n, n, vec![true; cells], Vec2::ZERO, 1.0)
    }

    #[test]
    fn path_through_open_returns_straight_segment() {
        let nav = open_grid(10);
        let start = Vec3::new(0.0, 0.0, 0.0);
        let goal = Vec3::new(9.0, 0.0, 9.0);
        let path = nav.path(start, goal).expect("path exists in open grid");
        assert!(path.len() >= 2, "path must contain start and goal");
        assert_eq!(path.first().copied(), Some(Vec3::new(0.0, 0.0, 0.0)));
        assert_eq!(path.last().copied(), Some(Vec3::new(9.0, 0.0, 9.0)));

        // Monotonic progress: every step should strictly decrease the
        // remaining distance to the goal. We compare squared distance
        // in the (x, z) plane to dodge the floating-point sqrt.
        let dist2 =
            |p: Vec3| (p.x - goal.x).powi(2) + (p.z - goal.z).powi(2);
        let mut prev = dist2(path[0]);
        for w in &path[1..] {
            let d = dist2(*w);
            assert!(
                d < prev,
                "expected monotonic progress toward goal: {prev} -> {d}",
            );
            prev = d;
        }
    }

    #[test]
    fn path_around_obstacle_finds_route() {
        // 10×10 with column 5 walled off for rows 0..7; gap at rows 7..10.
        // We pass the start at (0, 0) and goal at (9, 0); the only way
        // through is to detour southward (higher row index) to use the gap.
        let w: u32 = 10;
        let h: u32 = 10;
        let mut walkable = vec![true; (w * h) as usize];
        for row in 0..7 {
            walkable[(row * w as usize) + 5] = false;
        }
        let nav = GridNav::from_walkable(w, h, walkable, Vec2::ZERO, 1.0);

        let start = Vec3::new(0.0, 0.0, 0.0);
        let goal = Vec3::new(9.0, 0.0, 0.0);
        let path = nav.path(start, goal).expect("path must exist");

        // The gap is at rows >= 7; every viable route must touch one of
        // those rows at least once.
        let routed_through_gap = path.iter().any(|p| p.z >= 7.0);
        assert!(
            routed_through_gap,
            "path should detour through the row-7+ gap; got {path:?}",
        );
    }

    #[test]
    fn cache_roundtrips_to_disk() {
        let nav = open_grid(8);
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("nav.bin");
        nav.save_cache(&path).expect("save cache");
        let loaded = GridNav::load_cache(&path).expect("load cache");

        let from = Vec3::new(0.0, 0.0, 0.0);
        let to = Vec3::new(7.0, 0.0, 7.0);
        let original = nav.path(from, to).expect("path on original");
        let restored = loaded.path(from, to).expect("path on restored");

        assert_eq!(
            original, restored,
            "round-tripped grid must produce identical paths",
        );
        assert_eq!(loaded.width(), nav.width());
        assert_eq!(loaded.height(), nav.height());
        assert_eq!(loaded.cell_size(), nav.cell_size());
        assert_eq!(loaded.origin_world(), nav.origin_world());
    }
}
