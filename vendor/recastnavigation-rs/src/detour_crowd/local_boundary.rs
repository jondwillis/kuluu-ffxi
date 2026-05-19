use cxx::{type_id, ExternType};
use std::fmt::{self, Debug, Formatter};
use std::pin::Pin;

use crate::detour::{DtAABB, DtNavMeshQuery, DtPolyRef, DtQueryFilter};

#[cxx::bridge]
pub(crate) mod ffi {
    unsafe extern "C++" {
        include!("recastnavigation-rs/src/detour_crowd/crowd-ffi.h");

        type dtPolyRef = crate::detour::DtPolyRef;
        type dtQueryFilter = crate::detour::DtQueryFilter;
        type dtNavMeshQuery = crate::detour::query::ffi::dtNavMeshQuery;

        type dtLocalBoundary = crate::detour_crowd::local_boundary::CxxDtLocalBoundary;
        unsafe fn dtlb_reset(lb: Pin<&mut dtLocalBoundary>);
        unsafe fn dtlb_update(
            lb: Pin<&mut dtLocalBoundary>,
            re: dtPolyRef,
            pos: *const f32,
            collision_query_range: f32,
            navquery: *mut dtNavMeshQuery,
            filter: *const dtQueryFilter,
        );
        unsafe fn dtlb_isValid(
            lb: &dtLocalBoundary,
            navquery: *mut dtNavMeshQuery,
            filter: *const dtQueryFilter,
        ) -> bool;
        pub fn dtlb_getCenter(lb: &dtLocalBoundary) -> *const f32;
        pub fn dtlb_getSegmentCount(lb: &dtLocalBoundary) -> i32;
        pub fn dtlb_getSegment(lb: &dtLocalBoundary, i: i32) -> *const f32;
    }
}

#[repr(C, align(4))]
pub struct CxxDtLocalBoundary([u8; 308]);

unsafe impl ExternType for CxxDtLocalBoundary {
    type Id = type_id!("dtLocalBoundary");
    type Kind = cxx::kind::Trivial;
}

#[derive(Debug)]
pub struct DtLocalBoundary(CxxDtLocalBoundary);

impl Debug for CxxDtLocalBoundary {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        return f
            .debug_tuple("CxxDtLocalBoundary")
            .field(&(self as *const CxxDtLocalBoundary))
            .finish();
    }
}

impl DtLocalBoundary {
    fn inner(&self) -> &ffi::dtLocalBoundary {
        return &self.0;
    }

    fn inner_mut(&mut self) -> Pin<&mut ffi::dtLocalBoundary> {
        return unsafe { Pin::new_unchecked(&mut self.0) };
    }

    pub fn reset(&mut self) {
        unsafe { ffi::dtlb_reset(self.inner_mut()) };
    }

    pub fn update(
        &mut self,
        re: DtPolyRef,
        pos: &[f32; 3],
        collision_query_range: f32,
        navquery: &mut DtNavMeshQuery,
        filter: &DtQueryFilter,
    ) {
        unsafe {
            ffi::dtlb_update(
                self.inner_mut(),
                re,
                pos.as_ptr(),
                collision_query_range,
                navquery.as_mut_ptr(),
                filter,
            );
        }
    }

    pub fn is_valid(&self, navquery: &mut DtNavMeshQuery, filter: &DtQueryFilter) -> bool {
        return unsafe { ffi::dtlb_isValid(self.inner(), navquery.as_mut_ptr(), filter) };
    }

    pub fn center(&self) -> &[f32; 3] {
        return unsafe { &*(ffi::dtlb_getCenter(self.inner()) as *const [f32; 3]) };
    }

    pub fn segment(&self, i: usize) -> Option<&DtAABB> {
        if i >= ffi::dtlb_getSegmentCount(self.inner()) as usize {
            return None;
        }
        let seg = unsafe { &*(ffi::dtlb_getSegment(self.inner(), i as i32) as *const DtAABB) };
        return Some(seg);
    }
}
