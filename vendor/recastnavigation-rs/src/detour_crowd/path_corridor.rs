use cxx::{type_id, ExternType};
use std::fmt::{self, Debug, Formatter};
use std::pin::Pin;

use crate::detour::{DtNavMeshQuery, DtPolyRef, DtQueryFilter};

#[cxx::bridge]
pub(crate) mod ffi {
    unsafe extern "C++" {
        include!("recastnavigation-rs/src/detour_crowd/crowd-ffi.h");

        type dtPolyRef = crate::detour::DtPolyRef;
        type dtQueryFilter = crate::detour::DtQueryFilter;
        type dtNavMeshQuery = crate::detour::query::ffi::dtNavMeshQuery;

        type dtPathCorridor = crate::detour_crowd::path_corridor::CxxDtPathCorridor;
        fn init(self: Pin<&mut dtPathCorridor>, maxPath: i32) -> bool;
        unsafe fn reset(self: Pin<&mut dtPathCorridor>, rer: dtPolyRef, pos: *const f32);
        unsafe fn findCorners(
            self: Pin<&mut dtPathCorridor>,
            cornerVerts: *mut f32,
            cornerFlags: *mut u8,
            cornerPolys: *mut dtPolyRef,
            maxCorners: i32,
            navquery: *mut dtNavMeshQuery,
            filter: *const dtQueryFilter,
        ) -> i32;
        unsafe fn optimizePathVisibility(
            self: Pin<&mut dtPathCorridor>,
            next: *const f32,
            pathOptimizationRange: f32,
            navquery: *mut dtNavMeshQuery,
            filter: *const dtQueryFilter,
        );
        unsafe fn optimizePathTopology(
            self: Pin<&mut dtPathCorridor>,
            navquery: *mut dtNavMeshQuery,
            filter: *const dtQueryFilter,
        ) -> bool;
        unsafe fn moveOverOffmeshConnection(
            self: Pin<&mut dtPathCorridor>,
            offMeshConRef: dtPolyRef,
            res: *mut dtPolyRef,
            startPos: *mut f32,
            endPos: *mut f32,
            navquery: *mut dtNavMeshQuery,
        ) -> bool;
        unsafe fn fixPathStart(self: Pin<&mut dtPathCorridor>, safeRef: dtPolyRef, safePos: *const f32) -> bool;
        unsafe fn trimInvalidPath(
            self: Pin<&mut dtPathCorridor>,
            safeRef: dtPolyRef,
            safePos: *const f32,
            navquery: *mut dtNavMeshQuery,
            filter: *const dtQueryFilter,
        ) -> bool;
        unsafe fn isValid(
            self: Pin<&mut dtPathCorridor>,
            maxLookAhead: i32,
            navquery: *mut dtNavMeshQuery,
            filter: *const dtQueryFilter,
        ) -> bool;
        unsafe fn movePosition(
            self: Pin<&mut dtPathCorridor>,
            npos: *const f32,
            navquery: *mut dtNavMeshQuery,
            filter: *const dtQueryFilter,
        ) -> bool;
        unsafe fn moveTargetPosition(
            self: Pin<&mut dtPathCorridor>,
            npos: *const f32,
            navquery: *mut dtNavMeshQuery,
            filter: *const dtQueryFilter,
        ) -> bool;
        unsafe fn setCorridor(self: Pin<&mut dtPathCorridor>, target: *const f32, polys: *const dtPolyRef, npath: i32);
        fn getPos(self: &dtPathCorridor) -> *const f32;
        fn getTarget(self: &dtPathCorridor) -> *const f32;
        fn getFirstPoly(self: &dtPathCorridor) -> dtPolyRef;
        fn getLastPoly(self: &dtPathCorridor) -> dtPolyRef;
        fn getPath(self: &dtPathCorridor) -> *const dtPolyRef;
        fn getPathCount(self: &dtPathCorridor) -> i32;

        unsafe fn dtMergeCorridorStartMoved(
            path: *mut dtPolyRef,
            npath: i32,
            maxPath: i32,
            visited: *const dtPolyRef,
            nvisited: i32,
        ) -> i32;
        unsafe fn dtMergeCorridorEndMoved(
            path: *mut dtPolyRef,
            npath: i32,
            maxPath: i32,
            visited: *const dtPolyRef,
            nvisited: i32,
        ) -> i32;
        unsafe fn dtMergeCorridorStartShortcut(
            path: *mut dtPolyRef,
            npath: i32,
            maxPath: i32,
            visited: *const dtPolyRef,
            nvisited: i32,
        ) -> i32;
    }
}

#[repr(C, align(8))]
pub struct CxxDtPathCorridor([u8; 40]);

unsafe impl ExternType for CxxDtPathCorridor {
    type Id = type_id!("dtPathCorridor");
    type Kind = cxx::kind::Trivial;
}

pub struct DtPathCorridor(CxxDtPathCorridor);

impl Debug for DtPathCorridor {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        return f
            .debug_tuple("DtPathCorridor")
            .field(&(self as *const DtPathCorridor))
            .finish();
    }
}

impl DtPathCorridor {
    fn inner(&self) -> &ffi::dtPathCorridor {
        return &self.0;
    }

    fn inner_mut(&mut self) -> Pin<&mut ffi::dtPathCorridor> {
        return unsafe { Pin::new_unchecked(&mut self.0) };
    }

    pub fn init(&mut self, max_path: i32) -> bool {
        return self.inner_mut().init(max_path);
    }

    pub fn reset(&mut self, rer: DtPolyRef, pos: &[f32; 3]) {
        unsafe { self.inner_mut().reset(rer, pos.as_ptr()) };
    }

    pub fn find_corners(
        &mut self,
        corner_verts: &mut [[f32; 3]],
        corner_flags: &mut [u8],
        corner_polys: &mut [DtPolyRef],
        navquery: &mut DtNavMeshQuery,
        filter: &DtQueryFilter,
    ) -> i32 {
        let max_corners = [corner_verts.len(), corner_flags.len(), corner_polys.len()]
            .into_iter()
            .min()
            .unwrap_or(0);
        return unsafe {
            self.inner_mut().findCorners(
                corner_verts.as_mut_ptr() as *mut f32,
                corner_flags.as_mut_ptr(),
                corner_polys.as_mut_ptr(),
                max_corners as i32,
                navquery.as_mut_ptr(),
                filter,
            )
        };
    }

    pub fn optimize_path_visibility(
        &mut self,
        next: &[f32; 3],
        path_optimization_range: f32,
        navquery: &mut DtNavMeshQuery,
        filter: &DtQueryFilter,
    ) {
        unsafe {
            self.inner_mut().optimizePathVisibility(
                next.as_ptr(),
                path_optimization_range,
                navquery.as_mut_ptr(),
                filter,
            )
        };
    }

    pub fn optimize_path_topology(&mut self, navquery: &mut DtNavMeshQuery, filter: &DtQueryFilter) -> bool {
        return unsafe { self.inner_mut().optimizePathTopology(navquery.as_mut_ptr(), filter) };
    }

    pub fn move_over_offmesh_connection(
        &mut self,
        off_mesh_con_ref: DtPolyRef,
        res: &mut DtPolyRef,
        start_pos: &mut [f32; 3],
        end_pos: &mut [f32; 3],
        navquery: &mut DtNavMeshQuery,
    ) -> bool {
        return unsafe {
            self.inner_mut().moveOverOffmeshConnection(
                off_mesh_con_ref,
                res,
                start_pos.as_mut_ptr(),
                end_pos.as_mut_ptr(),
                navquery.as_mut_ptr(),
            )
        };
    }

    pub fn fix_path_start(&mut self, safe_ref: DtPolyRef, safe_pos: &[f32; 3]) -> bool {
        return unsafe { self.inner_mut().fixPathStart(safe_ref, safe_pos.as_ptr()) };
    }

    pub fn trim_invalid_path(
        &mut self,
        safe_ref: DtPolyRef,
        safe_pos: &[f32; 3],
        navquery: &mut DtNavMeshQuery,
        filter: &DtQueryFilter,
    ) -> bool {
        return unsafe {
            self.inner_mut()
                .trimInvalidPath(safe_ref, safe_pos.as_ptr(), navquery.as_mut_ptr(), filter)
        };
    }

    pub fn is_valid(&mut self, max_look_ahead: i32, navquery: &mut DtNavMeshQuery, filter: &DtQueryFilter) -> bool {
        return unsafe { self.inner_mut().isValid(max_look_ahead, navquery.as_mut_ptr(), filter) };
    }

    pub fn move_position(&mut self, npos: &[f32; 3], navquery: &mut DtNavMeshQuery, filter: &DtQueryFilter) -> bool {
        return unsafe {
            self.inner_mut()
                .movePosition(npos.as_ptr(), navquery.as_mut_ptr(), filter)
        };
    }

    pub fn move_target_position(
        &mut self,
        npos: &[f32; 3],
        navquery: &mut DtNavMeshQuery,
        filter: &DtQueryFilter,
    ) -> bool {
        return unsafe {
            self.inner_mut()
                .moveTargetPosition(npos.as_ptr(), navquery.as_mut_ptr(), filter)
        };
    }

    pub fn set_corridor(&mut self, target: &[f32; 3], polys: &[DtPolyRef], npath: i32) {
        unsafe { self.inner_mut().setCorridor(target.as_ptr(), polys.as_ptr(), npath) };
    }

    pub fn pos(&self) -> &[f32; 3] {
        return unsafe { &*(self.inner().getPos() as *const [f32; 3]) };
    }

    pub fn target(&self) -> &[f32; 3] {
        return unsafe { &*(self.inner().getTarget() as *const [f32; 3]) };
    }

    pub fn first_poly(&self) -> DtPolyRef {
        return self.inner().getFirstPoly();
    }

    pub fn last_poly(&self) -> DtPolyRef {
        return self.inner().getLastPoly();
    }

    pub fn path(&self) -> &[DtPolyRef] {
        return unsafe { std::slice::from_raw_parts(self.inner().getPath(), self.inner().getPathCount() as usize) };
    }
}

pub fn merge_corridor_start_moved(path: &mut [DtPolyRef], npath: usize, visited: &[DtPolyRef]) -> usize {
    return unsafe {
        ffi::dtMergeCorridorStartMoved(
            path.as_mut_ptr(),
            npath as i32,
            path.len() as i32,
            visited.as_ptr(),
            visited.len() as i32,
        ) as usize
    };
}

pub fn merge_corridor_end_moved(path: &mut [DtPolyRef], npath: usize, visited: &[DtPolyRef]) -> usize {
    return unsafe {
        ffi::dtMergeCorridorEndMoved(
            path.as_mut_ptr(),
            npath as i32,
            path.len() as i32,
            visited.as_ptr(),
            visited.len() as i32,
        ) as usize
    };
}

pub fn merge_corridor_start_shortcut(path: &mut [DtPolyRef], npath: usize, visited: &[DtPolyRef]) -> usize {
    return unsafe {
        ffi::dtMergeCorridorStartShortcut(
            path.as_mut_ptr(),
            npath as i32,
            path.len() as i32,
            visited.as_ptr(),
            visited.len() as i32,
        ) as usize
    };
}
