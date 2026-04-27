use cxx::{type_id, ExternType};
use static_assertions::const_assert_eq;
use std::ptr;
use std::{mem, pin::Pin};

use crate::base::XError;
use crate::detour::base::DtAABB;
use crate::detour::mesh::{DtMeshTile, DtNavMesh, DtPoly, DtPolyRef, DT_MAX_AREAS};

#[cxx::bridge]
pub(crate) mod ffi {
    #[repr(u32)]
    enum dtStraightPathFlags {
        DT_STRAIGHTPATH_START = 0x01,
        DT_STRAIGHTPATH_END = 0x02,
        DT_STRAIGHTPATH_OFFMESH_CONNECTION = 0x04,
    }

    #[repr(u32)]
    enum dtStraightPathOptions {
        DT_STRAIGHTPATH_AREA_CROSSINGS = 0x01,
        DT_STRAIGHTPATH_ALL_CROSSINGS = 0x02,
    }

    #[repr(u32)]
    enum dtFindPathOptions {
        DT_FINDPATH_ANY_ANGLE = 0x02,
    }

    unsafe extern "C++" {
        include!("recastnavigation-rs/src/detour/detour-ffi.h");

        type dtStraightPathFlags;
        type dtStraightPathOptions;
        type dtFindPathOptions;

        type dtStatus = crate::detour::base::ffi::dtStatus;
        type dtPolyRef = crate::detour::mesh::ffi::dtPolyRef;
        type dtPoly = crate::detour::mesh::ffi::dtPoly;
        type dtMeshTile = crate::detour::mesh::ffi::dtMeshTile;
        type dtNavMesh = crate::detour::mesh::ffi::dtNavMesh;

        type dtQueryFilter = crate::detour::query::DtQueryFilter;
        pub unsafe fn dtqf_passFilter(
            filter: &dtQueryFilter,
            re: dtPolyRef,
            tile: *const dtMeshTile,
            poly: *const dtPoly,
        ) -> bool;
        pub unsafe fn dtqf_getCost(
            filter: &dtQueryFilter,
            pa: *const f32,
            pb: *const f32,
            prevRef: dtPolyRef,
            prevTile: *const dtMeshTile,
            prevPoly: *const dtPoly,
            curRef: dtPolyRef,
            curTile: *const dtMeshTile,
            curPoly: *const dtPoly,
            nextRef: dtPolyRef,
            nextTile: *const dtMeshTile,
            nextPoly: *const dtPoly,
        ) -> f32;

        type dtRaycastHit = crate::detour::query::DtRaycastHit;

        type dtNavMeshQuery;
        pub fn dtAllocNavMeshQuery() -> *mut dtNavMeshQuery;
        pub unsafe fn dtFreeNavMeshQuery(query: *mut dtNavMeshQuery);
        // TODO: dtNavMesh owner;
        pub unsafe fn init(self: Pin<&mut dtNavMeshQuery>, nav: *const dtNavMesh, maxNodes: i32) -> dtStatus;
        pub unsafe fn findPath(
            self: &dtNavMeshQuery,
            startRef: dtPolyRef,
            endRef: dtPolyRef,
            startPos: *const f32,
            endPos: *const f32,
            filter: *const dtQueryFilter,
            path: *mut dtPolyRef,
            pathCount: *mut i32,
            maxPath: i32,
        ) -> dtStatus;
        pub unsafe fn findStraightPath(
            self: &dtNavMeshQuery,
            startPos: *const f32,
            endPos: *const f32,
            path: *const dtPolyRef,
            pathSize: i32,
            straightPath: *mut f32,
            straightPathFlags: *mut u8,
            straightPathRefs: *mut dtPolyRef,
            straightPathCount: *mut i32,
            maxStraightPath: i32,
            options: i32,
        ) -> dtStatus;
        pub unsafe fn initSlicedFindPath(
            self: Pin<&mut dtNavMeshQuery>,
            startRef: dtPolyRef,
            endRef: dtPolyRef,
            startPos: *const f32,
            endPos: *const f32,
            filter: *const dtQueryFilter,
            options: u32,
        ) -> dtStatus;
        pub unsafe fn updateSlicedFindPath(
            self: Pin<&mut dtNavMeshQuery>,
            maxIter: i32,
            doneIters: *mut i32,
        ) -> dtStatus;
        pub unsafe fn finalizeSlicedFindPath(
            self: Pin<&mut dtNavMeshQuery>,
            path: *mut dtPolyRef,
            pathCount: *mut i32,
            maxPath: i32,
        ) -> dtStatus;
        pub unsafe fn finalizeSlicedFindPathPartial(
            self: Pin<&mut dtNavMeshQuery>,
            existing: *const dtPolyRef,
            existingSize: i32,
            path: *mut dtPolyRef,
            pathCount: *mut i32,
            maxPath: i32,
        ) -> dtStatus;
        pub unsafe fn findPolysAroundCircle(
            self: &dtNavMeshQuery,
            startRef: dtPolyRef,
            centerPos: *const f32,
            radius: f32,
            filter: *const dtQueryFilter,
            resultRef: *mut dtPolyRef,
            resultParent: *mut dtPolyRef,
            resultCost: *mut f32,
            resultCount: *mut i32,
            maxResult: i32,
        ) -> dtStatus;
        pub unsafe fn findPolysAroundShape(
            self: &dtNavMeshQuery,
            startRef: dtPolyRef,
            verts: *const f32,
            nverts: i32,
            filter: *const dtQueryFilter,
            resultRef: *mut dtPolyRef,
            resultParent: *mut dtPolyRef,
            resultCost: *mut f32,
            resultCount: *mut i32,
            maxResult: i32,
        ) -> dtStatus;
        pub unsafe fn getPathFromDijkstraSearch(
            self: &dtNavMeshQuery,
            endRef: dtPolyRef,
            path: *mut dtPolyRef,
            pathCount: *mut i32,
            maxPath: i32,
        ) -> dtStatus;
        #[rust_name = "findNearestPoly1"]
        pub unsafe fn findNearestPoly(
            self: &dtNavMeshQuery,
            center: *const f32,
            halfExtents: *const f32,
            filter: *const dtQueryFilter,
            nearestRef: *mut dtPolyRef,
            nearestPt: *mut f32,
        ) -> dtStatus;
        #[rust_name = "findNearestPoly2"]
        pub unsafe fn findNearestPoly(
            self: &dtNavMeshQuery,
            center: *const f32,
            halfExtents: *const f32,
            filter: *const dtQueryFilter,
            nearestRef: *mut dtPolyRef,
            nearestPt: *mut f32,
            isOverPoly: *mut bool,
        ) -> dtStatus;
        pub unsafe fn queryPolygons(
            self: &dtNavMeshQuery,
            center: *const f32,
            halfExtents: *const f32,
            filter: *const dtQueryFilter,
            polys: *mut dtPolyRef,
            polyCount: *mut i32,
            maxPolys: i32,
        ) -> dtStatus;
        pub unsafe fn findLocalNeighbourhood(
            self: &dtNavMeshQuery,
            startRef: dtPolyRef,
            centerPos: *const f32,
            radius: f32,
            filter: *const dtQueryFilter,
            resultRef: *mut dtPolyRef,
            resultParent: *mut dtPolyRef,
            resultCount: *mut i32,
            maxResult: i32,
        ) -> dtStatus;
        pub unsafe fn moveAlongSurface(
            self: &dtNavMeshQuery,
            startRef: dtPolyRef,
            startPos: *const f32,
            endPos: *const f32,
            filter: *const dtQueryFilter,
            resultPos: *mut f32,
            visited: *mut dtPolyRef,
            visitedCount: *mut i32,
            maxVisitedSize: i32,
        ) -> dtStatus;
        #[rust_name = "raycast1"]
        pub unsafe fn raycast(
            self: &dtNavMeshQuery,
            startRef: dtPolyRef,
            startPos: *const f32,
            endPos: *const f32,
            filter: *const dtQueryFilter,
            t: *mut f32,
            hitNormal: *mut f32,
            path: *mut dtPolyRef,
            pathCount: *mut i32,
            maxPath: i32,
        ) -> dtStatus;
        #[rust_name = "raycast2"]
        pub unsafe fn raycast(
            self: &dtNavMeshQuery,
            startRef: dtPolyRef,
            startPos: *const f32,
            endPos: *const f32,
            filter: *const dtQueryFilter,
            options: u32,
            hit: *mut dtRaycastHit,
            prevRef: dtPolyRef,
        ) -> dtStatus;
        pub unsafe fn findDistanceToWall(
            self: &dtNavMeshQuery,
            startRef: dtPolyRef,
            centerPos: *const f32,
            maxRadius: f32,
            filter: *const dtQueryFilter,
            hitDist: *mut f32,
            hitPos: *mut f32,
            hitNormal: *mut f32,
        ) -> dtStatus;
        pub unsafe fn getPolyWallSegments(
            self: &dtNavMeshQuery,
            re: dtPolyRef,
            filter: *const dtQueryFilter,
            segmentVerts: *mut f32,
            segmentRefs: *mut dtPolyRef,
            segmentCount: *mut i32,
            maxSegments: i32,
        ) -> dtStatus;
        pub unsafe fn closestPointOnPoly(
            self: &dtNavMeshQuery,
            re: dtPolyRef,
            pos: *const f32,
            closest: *mut f32,
            posOverPoly: *mut bool,
        ) -> dtStatus;
        pub unsafe fn closestPointOnPolyBoundary(
            self: &dtNavMeshQuery,
            re: dtPolyRef,
            pos: *const f32,
            closest: *mut f32,
        ) -> dtStatus;
        pub unsafe fn getPolyHeight(
            self: &dtNavMeshQuery,
            re: dtPolyRef,
            pos: *const f32,
            height: *mut f32,
        ) -> dtStatus;
        pub unsafe fn isValidPolyRef(self: &dtNavMeshQuery, re: dtPolyRef, filter: *const dtQueryFilter) -> bool;
        pub unsafe fn isInClosedList(self: &dtNavMeshQuery, re: dtPolyRef) -> bool;
        // pub unsafe fn getNodePool(self: &dtNavMeshQuery) -> *mut dtNodePool;
        pub unsafe fn getAttachedNavMesh(self: &dtNavMeshQuery) -> *const dtNavMesh;
        pub unsafe fn dtnmq_findRandomPoint(
            query: &dtNavMeshQuery,
            filter: *const dtQueryFilter,
            frand: unsafe extern "C" fn() -> f32,
            randomRef: *mut dtPolyRef,
            randomPt: *mut f32,
        ) -> dtStatus;
        pub unsafe fn dtnmq_findRandomPointAroundCircle(
            query: &dtNavMeshQuery,
            startRef: dtPolyRef,
            centerPos: *const f32,
            maxRadius: f32,
            filter: *const dtQueryFilter,
            frand: unsafe extern "C" fn() -> f32,
            randomRef: *mut dtPolyRef,
            randomPt: *mut f32,
        ) -> dtStatus;
    }
}

pub type DtStraightPathFlags = u8;
pub const DT_STRAIGHTPATH_START: DtStraightPathFlags = ffi::dtStraightPathFlags::DT_STRAIGHTPATH_START.repr as u8;
pub const DT_STRAIGHTPATH_END: DtStraightPathFlags = ffi::dtStraightPathFlags::DT_STRAIGHTPATH_END.repr as u8;
pub const DT_STRAIGHTPATH_OFFMESH_CONNECTION: DtStraightPathFlags =
    ffi::dtStraightPathFlags::DT_STRAIGHTPATH_OFFMESH_CONNECTION.repr as u8;

pub type DtStraightPathOptions = u32;
pub const DT_STRAIGHTPATH_AREA_CROSSINGS: DtStraightPathOptions =
    ffi::dtStraightPathOptions::DT_STRAIGHTPATH_AREA_CROSSINGS.repr;
pub const DT_STRAIGHTPATH_ALL_CROSSINGS: DtStraightPathOptions =
    ffi::dtStraightPathOptions::DT_STRAIGHTPATH_ALL_CROSSINGS.repr;

#[repr(C)]
#[derive(Debug, Clone)]
pub struct DtQueryFilter {
    pub area_cost: [f32; DT_MAX_AREAS],
    pub include_flags: u16,
    pub exclude_flags: u16,
}

const_assert_eq!(mem::size_of::<DtQueryFilter>(), 260);

unsafe impl ExternType for DtQueryFilter {
    type Id = type_id!("dtQueryFilter");
    type Kind = cxx::kind::Trivial;
}

impl Default for DtQueryFilter {
    fn default() -> DtQueryFilter {
        return DtQueryFilter {
            area_cost: [1.0; DT_MAX_AREAS],
            include_flags: 0xffff,
            exclude_flags: 0,
        };
    }
}

impl DtQueryFilter {
    pub fn pass_filter(&self, re: DtPolyRef, tile: DtMeshTile, poly: &DtPoly) -> bool {
        return unsafe { ffi::dtqf_passFilter(self, re, tile.as_ptr(), poly) };
    }

    pub fn get_cost(
        &self,
        pa: &[f32; 3],
        pb: &[f32; 3],
        prev_re: DtPolyRef,
        prev_tile: &DtMeshTile,
        prev_poly: &DtPoly,
        cur_re: DtPolyRef,
        cur_tile: &DtMeshTile,
        cur_poly: &DtPoly,
        next_re: DtPolyRef,
        next_tile: &DtMeshTile,
        next_poly: &DtPoly,
    ) -> f32 {
        return unsafe {
            ffi::dtqf_getCost(
                self,
                pa.as_ptr(),
                pb.as_ptr(),
                prev_re,
                prev_tile.as_ptr(),
                prev_poly as *const _,
                cur_re,
                cur_tile.as_ptr(),
                cur_poly as *const _,
                next_re,
                next_tile.as_ptr(),
                next_poly as *const _,
            )
        };
    }
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct DtRaycastHit {
    pub t: f32,
    pub hit_normal: [f32; 3],
    pub hit_edge_index: i32,
    path: *mut DtPolyRef,
    path_count: i32,
    pub max_path: i32,
    pub path_cost: f32,
}

#[cfg(target_pointer_width = "64")]
const_assert_eq!(mem::size_of::<DtRaycastHit>(), 48);

#[cfg(target_pointer_width = "32")]
const_assert_eq!(mem::size_of::<DtRaycastHit>(), 36);

unsafe impl ExternType for DtRaycastHit {
    type Id = type_id!("dtRaycastHit");
    type Kind = cxx::kind::Trivial;
}

impl Default for DtRaycastHit {
    fn default() -> DtRaycastHit {
        return DtRaycastHit {
            t: 0.0,
            hit_normal: [0.0; 3],
            hit_edge_index: 0,
            path: ptr::null_mut(),
            path_count: 0,
            max_path: 0,
            path_cost: 0.0,
        };
    }
}

impl DtRaycastHit {
    pub fn path(&self) -> &[DtPolyRef] {
        return unsafe { std::slice::from_raw_parts(self.path, self.path_count as usize) };
    }

    pub fn path_mut(&mut self) -> &mut [DtPolyRef] {
        return unsafe { std::slice::from_raw_parts_mut(self.path, self.path_count as usize) };
    }

    pub fn path_count(&self) -> i32 {
        return self.path_count;
    }
}

#[derive(Debug)]
pub struct DtNavMeshQuery(*mut ffi::dtNavMeshQuery);

impl Drop for DtNavMeshQuery {
    fn drop(&mut self) {
        unsafe { ffi::dtFreeNavMeshQuery(self.0) };
    }
}

impl DtNavMeshQuery {
    pub fn new() -> DtNavMeshQuery {
        return DtNavMeshQuery(ffi::dtAllocNavMeshQuery());
    }

    fn inner(&self) -> &ffi::dtNavMeshQuery {
        return unsafe { &*self.0 };
    }

    fn inner_mut(&mut self) -> Pin<&mut ffi::dtNavMeshQuery> {
        return unsafe { Pin::new_unchecked(&mut *self.0) };
    }

    pub fn as_ptr(&self) -> *const ffi::dtNavMeshQuery {
        return self.0;
    }

    pub fn as_mut_ptr(&mut self) -> *mut ffi::dtNavMeshQuery {
        return self.0;
    }

    pub fn init(&mut self, nav: &DtNavMesh, max_nodes: usize) -> Result<(), XError> {
        return unsafe { self.inner_mut().init(nav.as_ptr(), max_nodes as i32) }.to_result();
    }

    pub fn with_mesh(nav: &DtNavMesh, max_nodes: usize) -> Result<DtNavMeshQuery, XError> {
        let mut query = DtNavMeshQuery::new();
        query.init(nav, max_nodes)?;
        return Ok(query);
    }

    pub fn find_path(
        &self,
        start_ref: DtPolyRef,
        end_ref: DtPolyRef,
        start_pos: &[f32; 3],
        end_pos: &[f32; 3],
        filter: &DtQueryFilter,
        path: &mut [DtPolyRef],
    ) -> Result<usize, XError> {
        let mut path_count = 0;
        unsafe {
            self.inner().findPath(
                start_ref,
                end_ref,
                start_pos.as_ptr(),
                end_pos.as_ptr(),
                filter,
                path.as_mut_ptr(),
                &mut path_count,
                path.len() as i32,
            )
        }
        .to_result()?;
        return Ok(path_count as usize);
    }

    pub fn find_straight_path(
        &self,
        start_pos: &[f32; 3],
        end_pos: &[f32; 3],
        path: &[DtPolyRef],
        straight_path: &mut [[f32; 3]],
        straight_path_flags: Option<&mut [DtStraightPathFlags]>,
        straight_path_refs: Option<&mut [DtPolyRef]>,
        options: DtStraightPathOptions,
    ) -> Result<usize, XError> {
        let mut max_result = straight_path.len();

        let mut straight_path_flags_ptr = ptr::null_mut();
        if let Some(straight_path_flags) = straight_path_flags {
            max_result = usize::min(straight_path_flags.len(), max_result);
            straight_path_flags_ptr = straight_path_flags.as_mut_ptr();
        }

        let mut straight_path_refs_ptr = ptr::null_mut();
        if let Some(straight_path_refs) = straight_path_refs {
            max_result = usize::min(straight_path_refs.len(), max_result);
            straight_path_refs_ptr = straight_path_refs.as_mut_ptr();
        }

        let mut straight_path_count = 0;
        unsafe {
            self.inner().findStraightPath(
                start_pos.as_ptr(),
                end_pos.as_ptr(),
                path.as_ptr(),
                path.len() as i32,
                straight_path.as_mut_ptr() as *mut _,
                straight_path_flags_ptr,
                straight_path_refs_ptr,
                &mut straight_path_count,
                max_result as i32,
                options as i32,
            )
        }
        .to_result()?;
        return Ok(straight_path_count as usize);
    }

    pub fn init_sliced_find_path(
        &mut self,
        start_ref: DtPolyRef,
        end_ref: DtPolyRef,
        start_pos: &[f32; 3],
        end_pos: &[f32; 3],
        filter: &DtQueryFilter,
        any_angle: bool,
    ) -> Result<(), XError> {
        let mut options = 0;
        if any_angle {
            options = ffi::dtFindPathOptions::DT_FINDPATH_ANY_ANGLE.repr;
        }
        return unsafe {
            self.inner_mut().initSlicedFindPath(
                start_ref,
                end_ref,
                start_pos.as_ptr(),
                end_pos.as_ptr(),
                filter,
                options,
            )
        }
        .to_result();
    }

    pub fn update_sliced_find_path(&mut self, max_iter: usize) -> Result<usize, XError> {
        let mut done_iters = 0;
        unsafe { self.inner_mut().updateSlicedFindPath(max_iter as i32, &mut done_iters) }.to_result()?;
        return Ok(done_iters as usize);
    }

    pub fn finalize_sliced_find_path(&mut self, path: &mut [DtPolyRef]) -> Result<usize, XError> {
        let mut path_count = 0;
        unsafe {
            self.inner_mut()
                .finalizeSlicedFindPath(path.as_mut_ptr(), &mut path_count, path.len() as i32)
        }
        .to_result()?;
        return Ok(path_count as usize);
    }

    pub fn finalize_sliced_find_path_partial(
        &mut self,
        existing: &[DtPolyRef],
        path: &mut [DtPolyRef],
    ) -> Result<usize, XError> {
        let mut path_count = 0;
        unsafe {
            self.inner_mut().finalizeSlicedFindPathPartial(
                existing.as_ptr(),
                existing.len() as i32,
                path.as_mut_ptr(),
                &mut path_count,
                path.len() as i32,
            )
        }
        .to_result()?;
        return Ok(path_count as usize);
    }

    pub fn find_polys_around_circle(
        &self,
        start_ref: DtPolyRef,
        center_pos: &[f32; 3],
        radius: f32,
        filter: &DtQueryFilter,
        result_ref: Option<&mut [DtPolyRef]>,
        result_parent: Option<&mut [DtPolyRef]>,
        result_cost: Option<&mut [f32]>,
    ) -> Result<usize, XError> {
        let mut max_result = usize::MAX;

        let mut result_ref_ptr = ptr::null_mut();
        if let Some(result_ref) = result_ref {
            max_result = usize::min(result_ref.len(), max_result);
            result_ref_ptr = result_ref.as_mut_ptr();
        }

        let mut result_parent_ptr = ptr::null_mut();
        if let Some(result_parent) = result_parent {
            max_result = usize::min(result_parent.len(), max_result);
            result_parent_ptr = result_parent.as_mut_ptr();
        }

        let mut result_cost_ptr = ptr::null_mut();
        if let Some(result_cost) = result_cost {
            max_result = usize::min(result_cost.len(), max_result);
            result_cost_ptr = result_cost.as_mut_ptr();
        }

        if max_result == usize::MAX {
            return Ok(0);
        }

        let mut result_count = 0;
        unsafe {
            self.inner().findPolysAroundCircle(
                start_ref,
                center_pos.as_ptr(),
                radius,
                filter,
                result_ref_ptr,
                result_parent_ptr,
                result_cost_ptr,
                &mut result_count,
                max_result as i32,
            )
        }
        .to_result()?;
        return Ok(result_count as usize);
    }

    pub fn find_polys_around_shape(
        &self,
        start_ref: DtPolyRef,
        verts: &[[f32; 3]],
        filter: &DtQueryFilter,
        result_ref: Option<&mut [DtPolyRef]>,
        result_parent: Option<&mut [DtPolyRef]>,
        result_cost: Option<&mut [f32]>,
    ) -> Result<usize, XError> {
        let mut max_result = usize::MAX;

        let mut result_ref_ptr = ptr::null_mut();
        if let Some(result_ref) = result_ref {
            max_result = usize::min(result_ref.len(), max_result);
            result_ref_ptr = result_ref.as_mut_ptr();
        }

        let mut result_parent_ptr = ptr::null_mut();
        if let Some(result_parent) = result_parent {
            max_result = usize::min(result_parent.len(), max_result);
            result_parent_ptr = result_parent.as_mut_ptr();
        }

        let mut result_cost_ptr = ptr::null_mut();
        if let Some(result_cost) = result_cost {
            max_result = usize::min(result_cost.len(), max_result);
            result_cost_ptr = result_cost.as_mut_ptr();
        }

        if max_result == usize::MAX {
            return Ok(0);
        }

        let mut result_count = 0;
        unsafe {
            self.inner().findPolysAroundShape(
                start_ref,
                verts.as_ptr() as *const _,
                verts.len() as i32,
                filter,
                result_ref_ptr,
                result_parent_ptr,
                result_cost_ptr,
                &mut result_count,
                max_result as i32,
            )
        }
        .to_result()?;
        return Ok(result_count as usize);
    }

    pub fn get_path_from_dijkstra_search(&self, end_ref: DtPolyRef, path: &mut [DtPolyRef]) -> Result<usize, XError> {
        let mut path_count = 0;
        unsafe {
            self.inner()
                .getPathFromDijkstraSearch(end_ref, path.as_mut_ptr(), &mut path_count, path.len() as i32)
        }
        .to_result()?;
        return Ok(path_count as usize);
    }

    /// Returns `Result<(nearest_ref: DtPolyRef, nearest_pt: [f32; 3])>`
    pub fn find_nearest_poly_1(
        &self,
        center: &[f32; 3],
        half_extents: &[f32; 3],
        filter: &DtQueryFilter,
    ) -> Result<(DtPolyRef, [f32; 3]), XError> {
        let mut nearest_ref = DtPolyRef::default();
        let mut nearest_pt = [0.0; 3];
        unsafe {
            self.inner().findNearestPoly1(
                center.as_ptr(),
                half_extents.as_ptr(),
                filter,
                &mut nearest_ref,
                nearest_pt.as_mut_ptr(),
            )
        }
        .to_result()?;
        return Ok((nearest_ref, nearest_pt));
    }

    /// Returns `Result<(nearest_ref: DtPolyRef, nearest_pt: [f32; 3], is_over_poly: bool)>`
    pub fn find_nearest_poly_2(
        &self,
        center: &[f32; 3],
        half_extents: &[f32; 3],
        filter: &DtQueryFilter,
    ) -> Result<(DtPolyRef, [f32; 3], bool), XError> {
        let mut nearest_ref = DtPolyRef::default();
        let mut nearest_pt = [0.0; 3];
        let mut is_over_poly = false;
        unsafe {
            self.inner().findNearestPoly2(
                center.as_ptr(),
                half_extents.as_ptr(),
                filter,
                &mut nearest_ref,
                nearest_pt.as_mut_ptr(),
                &mut is_over_poly,
            )
        }
        .to_result()?;
        return Ok((nearest_ref, nearest_pt, is_over_poly));
    }

    pub fn query_polygons(
        &self,
        center: &[f32; 3],
        half_extents: &[f32; 3],
        filter: &DtQueryFilter,
        polys: &mut [DtPolyRef],
    ) -> Result<usize, XError> {
        let mut poly_count = 0;
        unsafe {
            self.inner().queryPolygons(
                center.as_ptr(),
                half_extents.as_ptr(),
                filter,
                polys.as_mut_ptr(),
                &mut poly_count,
                polys.len() as i32,
            )
        }
        .to_result()?;
        return Ok(poly_count as usize);
    }

    // TODO: query_polygons with dtPolyQuery

    pub fn find_local_neighbourhood(
        &self,
        start_ref: DtPolyRef,
        center_pos: &[f32; 3],
        radius: f32,
        filter: &DtQueryFilter,
        result_ref: &mut [DtPolyRef],
        result_parent: Option<&mut [DtPolyRef]>,
    ) -> Result<usize, XError> {
        let mut max_result = 0;
        let mut result_parent_ptr = ptr::null_mut();
        if let Some(result_parent) = result_parent {
            max_result = result_parent.len();
            result_parent_ptr = result_parent.as_mut_ptr();
        }

        let mut result_count = 0;
        unsafe {
            self.inner().findLocalNeighbourhood(
                start_ref,
                center_pos.as_ptr(),
                radius,
                filter,
                result_ref.as_mut_ptr(),
                result_parent_ptr,
                &mut result_count,
                max_result as i32,
            )
        }
        .to_result()?;
        return Ok(result_count as usize);
    }

    /// Returns `Result<(result_pos: [f32; 3], visited_count: usize)>`
    pub fn move_along_surface(
        &self,
        start_ref: DtPolyRef,
        start_pos: &[f32; 3],
        end_pos: &[f32; 3],
        filter: &DtQueryFilter,
        visited: &mut [DtPolyRef],
    ) -> Result<([f32; 3], usize), XError> {
        let mut visited_count = 0;
        let mut result_pos = [0.0; 3];
        unsafe {
            self.inner().moveAlongSurface(
                start_ref,
                start_pos.as_ptr(),
                end_pos.as_ptr(),
                filter,
                result_pos.as_mut_ptr(),
                visited.as_mut_ptr(),
                &mut visited_count,
                visited.len() as i32,
            )
        }
        .to_result()?;
        return Ok((result_pos, visited_count as usize));
    }

    /// Returns `Result<(t: f32, hit_normal: [f32; 3], path_count: usize)>`
    pub fn raycast_1(
        &self,
        start_ref: DtPolyRef,
        start_pos: &[f32; 3],
        end_pos: &[f32; 3],
        filter: &DtQueryFilter,
        path: Option<&mut [DtPolyRef]>,
    ) -> Result<(f32, [f32; 3], usize), XError> {
        let mut max_path = 0;
        let mut path_ptr = ptr::null_mut();
        if let Some(path) = path {
            max_path = path.len();
            path_ptr = path.as_mut_ptr();
        }

        let mut t = 0.0;
        let mut hit_normal = [0.0; 3];
        let mut path_count = 0;
        unsafe {
            self.inner().raycast1(
                start_ref,
                start_pos.as_ptr(),
                end_pos.as_ptr(),
                filter,
                &mut t,
                hit_normal.as_mut_ptr(),
                path_ptr,
                &mut path_count,
                max_path as i32,
            )
        }
        .to_result()?;
        return Ok((t, hit_normal, path_count as usize));
    }

    pub fn raycast_2(
        &self,
        start_ref: DtPolyRef,
        start_pos: &[f32; 3],
        end_pos: &[f32; 3],
        filter: &DtQueryFilter,
        options: DtStraightPathFlags,
        prev_ref: Option<DtPolyRef>,
    ) -> Result<DtRaycastHit, XError> {
        let mut hit = DtRaycastHit::default();
        unsafe {
            self.inner().raycast2(
                start_ref,
                start_pos.as_ptr(),
                end_pos.as_ptr(),
                filter,
                options as u32,
                &mut hit,
                prev_ref.unwrap_or(DtPolyRef::default()),
            )
        }
        .to_result()?;
        return Ok(hit);
    }

    /// Returns `Result<(hit_dist: f32, hit_pos: [f32; 3], hit_normal: [f32; 3])>`
    pub fn find_distance_to_wall(
        &self,
        start_ref: DtPolyRef,
        center_pos: &[f32; 3],
        max_radius: f32,
        filter: &DtQueryFilter,
    ) -> Result<(f32, [f32; 3], [f32; 3]), XError> {
        let mut hit_dist = 0.0;
        let mut hit_pos = [0.0; 3];
        let mut hit_normal = [0.0; 3];
        unsafe {
            self.inner().findDistanceToWall(
                start_ref,
                center_pos.as_ptr(),
                max_radius,
                filter,
                &mut hit_dist,
                hit_pos.as_mut_ptr(),
                hit_normal.as_mut_ptr(),
            )
        }
        .to_result()?;
        return Ok((hit_dist, hit_pos, hit_normal));
    }

    pub fn get_poly_wall_segments(
        &self,
        re: DtPolyRef,
        filter: &DtQueryFilter,
        segment_verts: &mut [DtAABB],
        segment_refs: Option<&mut [DtPolyRef]>,
    ) -> Result<usize, XError> {
        let mut max_segments = segment_verts.len();

        let mut segment_refs_ptr = ptr::null_mut();
        if let Some(segment_refs) = segment_refs {
            max_segments = segment_refs.len();
            segment_refs_ptr = segment_refs.as_mut_ptr();
        }

        let mut segment_count = 0;
        unsafe {
            self.inner().getPolyWallSegments(
                re,
                filter,
                segment_verts.as_mut_ptr() as *mut _,
                segment_refs_ptr,
                &mut segment_count,
                max_segments as i32,
            )
        }
        .to_result()?;
        return Ok(segment_count as usize);
    }

    /// Returns `Result<(closest: [f32; 3], pos_over_poly: bool)>`
    pub fn closest_point_on_poly(&self, re: DtPolyRef, pos: &[f32; 3]) -> Result<([f32; 3], bool), XError> {
        let mut closest: [f32; 3] = [0.0; 3];
        let mut pos_over_poly = false;
        unsafe {
            self.inner()
                .closestPointOnPoly(re, pos.as_ptr(), closest.as_mut_ptr(), &mut pos_over_poly)
        }
        .to_result()?;
        return Ok((closest, pos_over_poly));
    }

    pub fn closest_point_on_poly_boundary(&self, re: DtPolyRef, pos: &[f32; 3]) -> Result<[f32; 3], XError> {
        let mut closest: [f32; 3] = [0.0; 3];
        unsafe {
            self.inner()
                .closestPointOnPolyBoundary(re, pos.as_ptr(), closest.as_mut_ptr())
        }
        .to_result()?;
        return Ok(closest);
    }

    pub fn get_poly_height(&self, re: DtPolyRef, pos: &[f32; 3]) -> Result<f32, XError> {
        let mut height = 0.0;
        unsafe { self.inner().getPolyHeight(re, pos.as_ptr(), &mut height) }.to_result()?;
        return Ok(height);
    }

    pub fn is_valid_poly_ref(&self, re: DtPolyRef, filter: &DtQueryFilter) -> bool {
        return unsafe { self.inner().isValidPolyRef(re, filter) };
    }

    pub fn is_in_closed_list(&self, re: DtPolyRef) -> bool {
        return unsafe { self.inner().isInClosedList(re) };
    }

    // pub fn get_node_pool(&self) -> &DtNodePool {
    //     return unsafe { &*self.inner().getNodePool() };
    // }

    // pub fn get_attached_nav_mesh(&self) -> &DtNavMesh {
    //     return unsafe { DtNavMesh::from_ptr(self.inner().getAttachedNavMesh()) };
    // }

    /// Returns `Result<(random_ref: DtPolyRef, random_pt: [f32; 3])>`
    pub fn find_random_point(
        &self,
        filter: &DtQueryFilter,
        frand: fn() -> f32,
    ) -> Result<(DtPolyRef, [f32; 3]), XError> {
        let mut random_ref = DtPolyRef::default();
        let mut random_pt = [0.0; 3];
        unsafe { ffi::dtnmq_findRandomPoint(self.inner(), filter, frand, &mut random_ref, random_pt.as_mut_ptr()) }
            .to_result()?;
        return Ok((random_ref, random_pt));
    }

    /// Returns `Result<(random_ref: DtPolyRef, random_pt: [f32; 3])>`
    pub fn find_random_point_around_circle(
        &self,
        start_ref: DtPolyRef,
        center_pos: &[f32; 3],
        max_radius: f32,
        filter: &DtQueryFilter,
        frand: fn() -> f32,
    ) -> Result<(DtPolyRef, [f32; 3]), XError> {
        let mut random_ref = DtPolyRef::default();
        let mut random_pt = [0.0; 3];
        unsafe {
            ffi::dtnmq_findRandomPointAroundCircle(
                self.inner(),
                start_ref,
                center_pos.as_ptr(),
                max_radius,
                filter,
                frand,
                &mut random_ref,
                random_pt.as_mut_ptr(),
            )
        }
        .to_result()?;
        return Ok((random_ref, random_pt));
    }
}
