//! Recast/Detour-backed `NavMesh` impl, sourced from
//! `LandSandBoat/xiNavmeshes`. Replaces the 2D `GridNav` fallback when a
//! `.nav` file is available for the current zone.
//!
//! ## Stage 1 progress
//!
//! - `fetch`: download + cache `.nav` files from upstream xiNavmeshes.
//! - `RecastNavMesh`: load + path-find via `recastnavigation-rs`.
//!
//! ## Coordinate system
//!
//! The single load-bearing decision in this crate is how to translate
//! between caller coords (what `NavMesh::path` accepts/returns) and
//! Detour-internal coords (what the navmesh was built in). LSB's
//! C++ `CNavMesh::ToDetourPos` is `(x, -y, -z)` — an involution. We
//! mirror that in [`ffxi_to_detour`] / [`detour_to_ffxi`]; see the
//! TODO there for the empirical confirmation step.

pub mod fetch;

use std::path::{Path, PathBuf};

use ffxi_nav::{glam::Vec3, NavMesh};
use recastnavigation_rs::demo::load_nav_mesh;
use recastnavigation_rs::detour::{
    DtNavMesh, DtNavMeshQuery, DtPolyRef, DtQueryFilter, DtStraightPathOptions,
};
use thiserror::Error;
use tracing::warn;

pub use fetch::{cache_dir, fetch, FetchError};

#[derive(Debug, Error)]
pub enum LoadError {
    #[error("fetch failed: {0}")]
    Fetch(#[from] FetchError),
    #[error("no upstream navmesh for zone {0}")]
    NotAvailable(u16),
    #[error("recastnavigation rejected `{path}`: {reason}")]
    Recast { path: PathBuf, reason: String },
    #[error("could not initialize DtNavMeshQuery: {0}")]
    Query(String),
}

/// A loaded Detour navmesh + paired query object.
///
/// `recastnavigation-rs` keeps the C++ `dtNavMesh` heap-allocated; we
/// hold both the mesh and a single query bound to it. The query's
/// `findPath` is thread-safe per the Detour docs only if you don't
/// mutate the mesh — which we never do — but we still keep one query
/// per `RecastNavMesh` instance to keep the `&self` API clean.
pub struct RecastNavMesh {
    _mesh: DtNavMesh,
    query: DtNavMeshQuery,
}

// SAFETY: Detour's threading model (per RecastNavigation's API docs):
//   * `dtNavMesh` is safe for concurrent **reads** (no writers); we
//     never mutate after construction, so it's `Send` and `Sync`.
//   * `dtNavMeshQuery` keeps mutable internal state per call; it's
//     `Send` (single-owner moves are fine) but **not** `Sync` (no
//     concurrent calls from multiple threads).
// `Reactor` only ever holds one `RecastNavMesh` per task, so `Send`
// alone is sufficient — `path()` takes `&self` but Detour serializes
// the work inside the query, which means callers must not share the
// nav across tokio tasks. Rust's `!Sync` enforces that.
unsafe impl Send for RecastNavMesh {}

/// How many polys/waypoints a single `path` call can return. Matches
/// LSB's `MAX_NAV_POLYS = 1024` (see `server/src/map/navmesh.cpp`).
const MAX_NAV_POLYS: usize = 1024;

/// Half-extents passed to `findNearestPoly`. LSB uses
/// `polyPickExt = {2.0, 4.0, 2.0}` — a 4-yalm-wide / 8-yalm-tall search
/// box. We adopt the same so our agent's "snap to navmesh" tolerance
/// matches the server's mob-pathing tolerance.
const POLY_PICK_EXT: [f32; 3] = [2.0, 4.0, 2.0];

impl RecastNavMesh {
    /// Construct from a path on disk. The file must be in the standard
    /// Detour `NAVMESHSET_MAGIC` format (what xiNavmeshes ships).
    pub fn from_path(path: &Path) -> Result<Self, LoadError> {
        let path_str = path.to_string_lossy();
        let mesh = load_nav_mesh(&path_str).map_err(|e| LoadError::Recast {
            path: path.to_path_buf(),
            reason: format!("{e:?}"),
        })?;
        let query = DtNavMeshQuery::with_mesh(&mesh, MAX_NAV_POLYS)
            .map_err(|e| LoadError::Query(format!("{e:?}")))?;
        Ok(Self {
            _mesh: mesh,
            query,
        })
    }

    /// Convenience: fetch (download-and-cache) + load in one call.
    pub fn for_zone(zone_id: u16) -> Result<Self, LoadError> {
        let path = fetch(zone_id)?.ok_or(LoadError::NotAvailable(zone_id))?;
        Self::from_path(&path)
    }

    /// Polygon outline edges in **raw Detour space** — exactly what's
    /// stored in the `.nav` file, with no coord transform applied.
    ///
    /// Caller decides how to project these into world coords. Path-
    /// finding goes through [`detour_to_ffxi`] (involution; round-trip
    /// safe). Rendering goes through a different transform that depends
    /// on which axis convention the navmesh was generated in — see the
    /// `detour_to_bevy` helper in
    /// `ffxi-client/src/view_native/navmesh_overlay.rs`.
    ///
    /// Returns the **navigation-level polygons** (the convex polys A*
    /// walks across), not the dense `detail_tris` sub-mesh. The detail
    /// mesh is noisier and covers the same ground.
    pub fn polygon_edges_detour(&self) -> Vec<([f32; 3], [f32; 3])> {
        let mut out = Vec::new();
        for tile_idx in 0..self._mesh.max_tiles() {
            let Some(tile) = self._mesh.get_tile(tile_idx) else {
                continue;
            };
            let Some(header) = tile.header() else { continue };
            let verts = tile.verts();
            let polys = tile.polys();
            let poly_count = header.poly_count as usize;
            for poly in polys.iter().take(poly_count) {
                let nverts = poly.vert_count as usize;
                if nverts < 2 {
                    continue;
                }
                for edge in 0..nverts {
                    let a_idx = poly.verts[edge] as usize;
                    let b_idx = poly.verts[(edge + 1) % nverts] as usize;
                    if a_idx >= verts.len() || b_idx >= verts.len() {
                        continue;
                    }
                    out.push((verts[a_idx], verts[b_idx]));
                }
            }
        }
        out
    }

    /// Convenience for callers who want polygon edges in
    /// **path-finding-caller-space** (post-[`detour_to_ffxi`]). Useful
    /// for debug-printing waypoints alongside the polygon graph that
    /// produced them. Don't use this for *rendering* — rendering needs
    /// a different transform; see `polygon_edges_detour`.
    pub fn polygon_edges_caller(&self) -> Vec<(Vec3, Vec3)> {
        self.polygon_edges_detour()
            .into_iter()
            .map(|(a, b)| (detour_to_ffxi(a), detour_to_ffxi(b)))
            .collect()
    }

    /// Navmesh height at a 2D ground-plane location. `z_hint` is the
    /// rough current height (used to disambiguate stacked layers in
    /// caves / multi-level structures). Returns FFXI-space `z` of the
    /// nearest poly, or `None` if no poly is within the search box.
    ///
    /// The vertical search range is 100 yalms — generous enough to
    /// catch a player who's mid-fall or temporarily clipped above
    /// the floor, without snapping to a totally different level.
    pub fn nearest_height_at(&self, x_ffxi: f32, y_ffxi: f32, z_hint: f32) -> Option<f32> {
        let filter = DtQueryFilter::default();
        let center = ffxi_to_detour(Vec3::new(x_ffxi, y_ffxi, z_hint));
        let half_ext = [POLY_PICK_EXT[0], 100.0, POLY_PICK_EXT[2]];
        let (poly, snapped) = self
            .query
            .find_nearest_poly_1(&center, &half_ext, &filter)
            .ok()?;
        if poly.is_null() {
            return None;
        }
        Some(detour_to_ffxi(snapped).z)
    }

    /// Wall-slide: try to move from `from` to `to`; if the line would
    /// exit the navmesh, project along the nearest poly edge and
    /// return the slid endpoint. Returns `None` if the start position
    /// isn't near any poly (player off-mesh — caller should pass the
    /// move through unchanged).
    ///
    /// `from` and `to` are in **caller-space** (same as `path()`).
    /// Internally we go through `ffxi_to_detour` and back, mirroring
    /// LSB's `CNavMesh::ToDetourPos` round-trip.
    ///
    /// Detour's `moveAlongSurface` is documented for short moves only
    /// (one or a few polys). It's perfect for per-tick WASD step
    /// dispatch (~5 yalms/sec at 60 Hz = ~0.08 yalm/tick) but should
    /// not be used for arbitrary teleports — for those, find_path is
    /// the right tool.
    pub fn slide_along(&self, from: Vec3, to: Vec3) -> Option<Vec3> {
        let filter = DtQueryFilter::default();
        let start = ffxi_to_detour(from);
        let end = ffxi_to_detour(to);

        let (start_ref, snapped_start) = self
            .query
            .find_nearest_poly_1(&start, &POLY_PICK_EXT, &filter)
            .ok()?;
        if start_ref.is_null() {
            return None;
        }

        // 16 visited polys is plenty for a single-tick step. If the
        // step is large enough to cross more than that, we fall back
        // to the unclamped move (move_along_surface will return
        // partial-success in that case, which we treat as "slide as
        // far as we tracked").
        let mut visited = vec![DtPolyRef::default(); 16];
        let (result_d, _n_visited) = self
            .query
            .move_along_surface(start_ref, &snapped_start, &end, &filter, &mut visited)
            .ok()?;
        Some(detour_to_ffxi(result_d))
    }
}

impl NavMesh for RecastNavMesh {
    fn path(&self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>> {
        let filter = DtQueryFilter::default();
        let start = ffxi_to_detour(from);
        let end = ffxi_to_detour(to);

        let (start_ref, start_pt) = self
            .query
            .find_nearest_poly_1(&start, &POLY_PICK_EXT, &filter)
            .ok()?;
        let (end_ref, end_pt) = self
            .query
            .find_nearest_poly_1(&end, &POLY_PICK_EXT, &filter)
            .ok()?;
        if start_ref.is_null() || end_ref.is_null() {
            warn!("findNearestPoly returned null for start or end");
            return None;
        }

        let mut polys = vec![DtPolyRef::default(); MAX_NAV_POLYS];
        let n_polys = self
            .query
            .find_path(start_ref, end_ref, &start_pt, &end_pt, &filter, &mut polys)
            .ok()?;
        if n_polys == 0 {
            return None;
        }

        let mut straight = vec![[0.0_f32; 3]; MAX_NAV_POLYS];
        let n_straight = self
            .query
            .find_straight_path(
                &start_pt,
                &end_pt,
                &polys[..n_polys],
                &mut straight,
                None,
                None,
                DtStraightPathOptions::default(),
            )
            .ok()?;
        if n_straight == 0 {
            return None;
        }

        Some(
            straight[..n_straight]
                .iter()
                .map(|d| detour_to_ffxi(*d))
                .collect(),
        )
    }
}

/// Convert a caller-space `Vec3` (FFXI z-up: x east, y north, z up)
/// to Detour-space `[f32; 3]` (Detour y-up: d.x east, d.y up, d.z
/// north). The mapping is a **y/z swap, no sign flips**, derived from
/// matching three empirical signals:
///
///   1. xiNavmeshes are produced by RecastNavigation in its standard
///      y-up convention (`d.y` is the up axis).
///   2. The Bevy renderer aligns the overlay using
///      `(d.x, d.y, -d.z)`, which only works if `d.y` is height.
///   3. FFXI's protocol packets put height in `z`, ground in `(x, y)`.
///
/// The earlier `(x, -y, -z)` involution path-found *correctly* (Detour
/// is orientation-agnostic for graph search) but produced waypoints
/// with bogus heights and broke `move_along_surface` (it queried for
/// polys at coordinates that didn't match where xiNavmeshes had put
/// them). The correct y/z swap makes both heights and short-step
/// queries work.
#[inline]
pub fn ffxi_to_detour(v: Vec3) -> [f32; 3] {
    [v.x, v.z, v.y]
}

/// Inverse of [`ffxi_to_detour`]. Y/z swap (Detour `d.y` height →
/// FFXI `z`, Detour `d.z` ground → FFXI `y`). Not an involution.
#[inline]
pub fn detour_to_ffxi(d: [f32; 3]) -> Vec3 {
    Vec3::new(d[0], d[2], d[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_links() {
        let _ = std::any::TypeId::of::<DtNavMesh>();
    }

    /// Round-trip: `detour_to_ffxi(ffxi_to_detour(v)) == v`. They're
    /// inverses (a y/z swap composed with itself is identity), even
    /// though neither alone is an involution.
    #[test]
    fn coord_transform_round_trip() {
        let v = Vec3::new(1.5, -2.25, 3.75);
        let round_trip = detour_to_ffxi(ffxi_to_detour(v));
        assert_eq!(round_trip, v);
    }

    /// The height axis (FFXI `z`) lands at Detour `d[1]` after the
    /// forward transform — that's what makes `move_along_surface`
    /// queries actually find polys, since xiNavmeshes stores them in
    /// y-up Detour-standard convention.
    #[test]
    fn height_axis_lands_at_detour_y() {
        let v = Vec3::new(0.0, 0.0, 42.0); // pure-height vector
        let d = ffxi_to_detour(v);
        assert_eq!(d, [0.0, 42.0, 0.0]);
    }
}
