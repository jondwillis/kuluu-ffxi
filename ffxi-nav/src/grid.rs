use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use glam::{Vec2, Vec3};
use pathfinding::prelude::astar;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::NavMesh;

const COST_CARDINAL: u32 = 100;

const COST_DIAGONAL: u32 = 141;

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

#[derive(Debug, Clone)]
pub struct GridNav {
    width: u32,
    height: u32,
    walkable: Vec<bool>,
    origin_world: Vec2,
    cell_size: f32,
}

#[derive(Debug, Serialize, Deserialize)]
struct GridNavOnDisk {
    width: u32,
    height: u32,
    walkable: Vec<bool>,
    origin_world: [f32; 2],
    cell_size: f32,
}

impl GridNav {
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

    fn is_walkable(&self, col: i32, row: i32) -> bool {
        if col < 0 || row < 0 || col >= self.width as i32 || row >= self.height as i32 {
            return false;
        }
        let idx = (row as usize) * (self.width as usize) + (col as usize);
        self.walkable[idx]
    }

    fn world_to_cell(&self, x: f32, z: f32) -> (i32, i32) {
        let col = ((x - self.origin_world.x) / self.cell_size).round() as i32;
        let row = ((z - self.origin_world.y) / self.cell_size).round() as i32;
        (col, row)
    }

    fn cell_to_world(&self, col: i32, row: i32) -> Vec3 {
        let x = self.origin_world.x + (col as f32) * self.cell_size;
        let z = self.origin_world.y + (row as f32) * self.cell_size;
        Vec3::new(x, 0.0, z)
    }

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

        let dist2 = |p: Vec3| (p.x - goal.x).powi(2) + (p.z - goal.z).powi(2);
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
