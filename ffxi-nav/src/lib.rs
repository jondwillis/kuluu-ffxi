pub use glam;
use glam::Vec3;

pub mod grid;
pub mod zone_names;
pub mod zonelines;

pub use grid::{GridNav, NavError};
pub use zone_names::zone_name;
pub use zonelines::{to_pos_for_line, zone_lines_for, ZoneLine};

pub trait NavMesh {
    fn path(&self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>>;
}
