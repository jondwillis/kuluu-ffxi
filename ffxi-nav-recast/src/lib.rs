pub mod fetch;

use std::path::{Path, PathBuf};

use ffxi_nav::{glam::Vec3, NavMesh};
use recastnavigation_rs::demo::load_nav_mesh;
use recastnavigation_rs::detour::{
    DtNavMesh, DtNavMeshQuery, DtPolyRef, DtQueryFilter, DtStraightPathOptions,
};
use thiserror::Error;

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

pub struct RecastNavMesh {
    _mesh: DtNavMesh,
    query: DtNavMeshQuery,
}

unsafe impl Send for RecastNavMesh {}

const MAX_NAV_POLYS: usize = 1024;

const POLY_PICK_EXT: [f32; 3] = [2.0, 4.0, 2.0];

impl RecastNavMesh {
    pub fn from_path(path: &Path) -> Result<Self, LoadError> {
        let path_str = path.to_string_lossy();
        let mesh = load_nav_mesh(&path_str).map_err(|e| LoadError::Recast {
            path: path.to_path_buf(),
            reason: format!("{e:?}"),
        })?;
        let query = DtNavMeshQuery::with_mesh(&mesh, MAX_NAV_POLYS)
            .map_err(|e| LoadError::Query(format!("{e:?}")))?;

        let mut tile_count = 0usize;
        let mut bmin = [f32::INFINITY; 3];
        let mut bmax = [f32::NEG_INFINITY; 3];
        for tile_idx in 0..mesh.max_tiles() {
            let Some(tile) = mesh.get_tile(tile_idx) else {
                continue;
            };
            let Some(header) = tile.header() else {
                continue;
            };
            tile_count += 1;
            for axis in 0..3 {
                bmin[axis] = bmin[axis].min(header.bmin[axis]);
                bmax[axis] = bmax[axis].max(header.bmax[axis]);
            }
        }
        tracing::info!(
            path = %path_str, tile_count,
            detour_bmin = ?bmin, detour_bmax = ?bmax,
            "RecastNavMesh loaded"
        );
        Ok(Self { _mesh: mesh, query })
    }

    pub fn for_zone(zone_id: u16) -> Result<Self, LoadError> {
        let path = fetch(zone_id)?.ok_or(LoadError::NotAvailable(zone_id))?;
        Self::from_path(&path)
    }

    pub fn polygon_edges_detour(&self) -> Vec<([f32; 3], [f32; 3])> {
        let mut out = Vec::new();
        for tile_idx in 0..self._mesh.max_tiles() {
            let Some(tile) = self._mesh.get_tile(tile_idx) else {
                continue;
            };
            let Some(header) = tile.header() else {
                continue;
            };
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

    pub fn polygon_edges_caller(&self) -> Vec<(Vec3, Vec3)> {
        self.polygon_edges_detour()
            .into_iter()
            .map(|(a, b)| (detour_to_ffxi(a), detour_to_ffxi(b)))
            .collect()
    }

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

    /// Coverage-check search extent for [`Self::slide_along`]'s off-mesh
    /// fallback: wide enough to say "there is truly no navmesh data near
    /// this destination", not just "no directly-connected polygon".
    const COVERAGE_CHECK_EXT: [f32; 3] = [15.0, 100.0, 15.0];

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

        let mut visited = vec![DtPolyRef::default(); 16];
        let (result_d, _n_visited) = self
            .query
            .move_along_surface(start_ref, &snapped_start, &end, &filter, &mut visited)
            .ok()?;

        // move_along_surface clamps to the connected-polygon boundary
        // whenever `end` isn't reachable, and that's indistinguishable from
        // its result alone between "a real wall/edge" and "LSB's xiNavmeshes
        // bake (built for mob pathing, not full player-walkable coverage)
        // simply never generated data out here". Treating the latter as a
        // wall regresses walkable retail space (e.g. decorative plazas mobs
        // never path through), so: if nothing exists near the *destination*
        // even under a generous search, there's no data at all — return
        // None so the caller's off-mesh fallback lets the raw move through
        // instead of clamping to this edge.
        let has_nearby_coverage = self
            .query
            .find_nearest_poly_1(&end, &Self::COVERAGE_CHECK_EXT, &filter)
            .ok()
            .is_some_and(|(poly_ref, _)| !poly_ref.is_null());
        if !has_nearby_coverage {
            return None;
        }

        Some(detour_to_ffxi(result_d))
    }
}

impl NavMesh for RecastNavMesh {
    fn path(&self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>> {
        let filter = DtQueryFilter::default();
        let start = ffxi_to_detour(from);
        let end = ffxi_to_detour(to);

        let endpoint_ext = [POLY_PICK_EXT[0], 100.0, POLY_PICK_EXT[2]];

        let (start_ref, start_pt) =
            match self
                .query
                .find_nearest_poly_1(&start, &endpoint_ext, &filter)
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::info!(
                        from_ffxi = ?from, start_detour = ?start, err = ?e,
                        "RecastNavMesh::path — find_nearest_poly failed for START"
                    );
                    return None;
                }
            };
        let (end_ref, end_pt) = match self.query.find_nearest_poly_1(&end, &endpoint_ext, &filter) {
            Ok(r) => r,
            Err(e) => {
                tracing::info!(
                    to_ffxi = ?to, end_detour = ?end, err = ?e,
                    "RecastNavMesh::path — find_nearest_poly failed for END"
                );
                return None;
            }
        };
        if start_ref.is_null() || end_ref.is_null() {
            tracing::info!(
                from_ffxi = ?from, to_ffxi = ?to,
                start_detour = ?start, start_snapped = ?start_pt,
                end_detour = ?end, end_snapped = ?end_pt,
                start_ref_null = start_ref.is_null(),
                end_ref_null = end_ref.is_null(),
                "RecastNavMesh::path — null poly ref (start or end off-mesh even with 100-yalm vertical tolerance)"
            );
            return None;
        }

        let d_start = ((start[0] - start_pt[0]).powi(2)
            + (start[1] - start_pt[1]).powi(2)
            + (start[2] - start_pt[2]).powi(2))
        .sqrt();
        let d_end = ((end[0] - end_pt[0]).powi(2)
            + (end[1] - end_pt[1]).powi(2)
            + (end[2] - end_pt[2]).powi(2))
        .sqrt();
        tracing::debug!(
            from_ffxi = ?from, to_ffxi = ?to,
            start_snap_dist_yalms = d_start, end_snap_dist_yalms = d_end,
            "RecastNavMesh::path — endpoints snapped"
        );

        let mut polys = vec![DtPolyRef::default(); MAX_NAV_POLYS];
        let n_polys = self
            .query
            .find_path(start_ref, end_ref, &start_pt, &end_pt, &filter, &mut polys)
            .ok()?;
        if n_polys == 0 {
            tracing::info!(
                from_ffxi = ?from, to_ffxi = ?to,
                "RecastNavMesh::path — find_path returned 0 polys (endpoints in disconnected components — no walkable route)"
            );
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
            tracing::info!(
                from_ffxi = ?from, to_ffxi = ?to, n_polys,
                "RecastNavMesh::path — find_straight_path returned 0 (polys connected but string-pull failed)"
            );
            return None;
        }
        tracing::debug!(
            from_ffxi = ?from, to_ffxi = ?to, n_polys, n_straight,
            "RecastNavMesh::path — path computed"
        );

        Some(
            straight[..n_straight]
                .iter()
                .map(|d| detour_to_ffxi(*d))
                .collect(),
        )
    }
}

#[inline]
pub fn ffxi_to_detour(v: Vec3) -> [f32; 3] {
    [v.x, -v.z, -v.y]
}

#[inline]
pub fn detour_to_ffxi(d: [f32; 3]) -> Vec3 {
    Vec3::new(d[0], -d[2], -d[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_links() {
        let _ = std::any::TypeId::of::<DtNavMesh>();
    }

    #[test]
    fn coord_transform_round_trip() {
        let v = Vec3::new(1.5, -2.25, 3.75);
        let round_trip = detour_to_ffxi(ffxi_to_detour(v));
        assert_eq!(round_trip, v);
    }

    #[test]
    fn height_axis_lands_at_detour_y() {
        let v = Vec3::new(0.0, 0.0, 42.0);
        let d = ffxi_to_detour(v);
        assert_eq!(d, [0.0, -42.0, 0.0]);
    }

    #[test]
    fn north_axis_negated_into_detour_z() {
        let v = Vec3::new(0.0, 33.0, 0.0);
        let d = ffxi_to_detour(v);
        assert_eq!(d, [0.0, 0.0, -33.0]);
    }
}
