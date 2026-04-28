//! Navigation and pathfinding primitives for the FFXI agent harness.
//!
//! Phase 10a: 2D occupancy-grid pathfinding via [`grid::GridNav`].
//! Phase 10b will add cliff-aware (3D) navigation; this crate's [`NavMesh`]
//! trait is the seam where richer implementations will plug in.

use glam::Vec3;

pub mod grid;

pub use grid::{GridNav, NavError};

/// Trait implemented by any navigable representation of a zone.
///
/// Implementations are expected to be cheap to query (sub-millisecond for
/// reasonable grid sizes) and may be cached to disk between runs.
pub trait NavMesh {
    /// Find a path of waypoints from `from` to `to`, or `None` if no
    /// route exists. The first waypoint may be coincident with `from`;
    /// callers should skip-step it if they already started moving.
    fn path(&self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>>;
}
