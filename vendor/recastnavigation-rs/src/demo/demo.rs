use cxx::{let_cxx_string, type_id, ExternType, UniquePtr};
use static_assertions::const_assert_eq;
use std::mem;
use std::pin::Pin;
use std::slice;

use crate::base::XError;
use crate::detour::DtNavMesh;

#[allow(dead_code)]
#[cxx::bridge]
pub(crate) mod ffi {
    unsafe extern "C++" {
        include!("recastnavigation-rs/src/demo/demo-ffi.h");

        type dtNavMesh = crate::detour::mesh::ffi::dtNavMesh;

        type rcMeshLoaderObj;
        fn load(self: Pin<&mut rcMeshLoaderObj>, fileName: &CxxString) -> bool;
        fn rcNewMeshLoaderObj() -> UniquePtr<rcMeshLoaderObj>;
        fn getVerts(self: &rcMeshLoaderObj) -> *const f32;
        fn getNormals(self: &rcMeshLoaderObj) -> *const f32;
        fn getTris(self: &rcMeshLoaderObj) -> *const i32;
        fn getVertCount(self: &rcMeshLoaderObj) -> i32;
        fn getTriCount(self: &rcMeshLoaderObj) -> i32;
        fn getFileName(self: &rcMeshLoaderObj) -> &CxxString;

        unsafe fn loadNavMesh(path: &str) -> *mut dtNavMesh;
        unsafe fn saveNavMesh(mesh: *const dtNavMesh, path: &str);

        type rcChunkyTriMeshNode = crate::demo::demo::RcChunkyTriMeshNode;
        type rcChunkyTriMesh = crate::demo::demo::CxxRcChunkyTriMesh;

        unsafe fn rcctm_new() -> *mut rcChunkyTriMesh;
        unsafe fn rcctm_delete(cm: *mut rcChunkyTriMesh);
        unsafe fn rcCreateChunkyTriMesh(
            verts: *const f32,
            tris: *const i32,
            ntris: i32,
            tris_per_chunk: i32,
            cm: *mut rcChunkyTriMesh,
        ) -> bool;
        unsafe fn rcGetChunksOverlappingRect(
            cm: *const rcChunkyTriMesh,
            bmin: *const f32,
            bmax: *const f32,
            ids: *mut i32,
            max_ids: i32,
        ) -> i32;
        unsafe fn rcGetChunksOverlappingSegment(
            cm: *const rcChunkyTriMesh,
            p: *const f32,
            q: *const f32,
            ids: *mut i32,
            max_ids: i32,
        ) -> i32;
    }
}

pub struct RcMeshLoaderObj(UniquePtr<ffi::rcMeshLoaderObj>);

impl Default for RcMeshLoaderObj {
    fn default() -> Self {
        return RcMeshLoaderObj::new();
    }
}

impl RcMeshLoaderObj {
    pub fn new() -> RcMeshLoaderObj {
        return RcMeshLoaderObj(ffi::rcNewMeshLoaderObj());
    }

    pub fn load(&mut self, file_name: &str) -> bool {
        let_cxx_string!(file_name = file_name);
        return self.0.pin_mut().load(&file_name);
    }

    pub fn get_verts(&self) -> &[[f32; 3]] {
        return unsafe { slice::from_raw_parts(self.0.getVerts() as *const _, self.get_vert_count() as usize) };
    }

    pub fn get_normals(&self) -> &[[f32; 3]] {
        return unsafe { slice::from_raw_parts(self.0.getNormals() as *const _, self.get_vert_count() as usize) };
    }

    pub fn get_tris(&self) -> &[[i32; 3]] {
        return unsafe { slice::from_raw_parts(self.0.getTris() as *const _, self.get_tri_count() as usize) };
    }

    pub fn get_vert_count(&self) -> i32 {
        return self.0.getVertCount();
    }

    pub fn get_tri_count(&self) -> i32 {
        return self.0.getTriCount();
    }

    pub fn get_file_name(&self) -> &str {
        return self.0.getFileName().to_str().unwrap_or("");
    }
}

pub fn load_nav_mesh(path: &str) -> Result<DtNavMesh, XError> {
    unsafe {
        let mesh = ffi::loadNavMesh(path);
        if mesh.is_null() {
            return Err(XError::Failed);
        }
        return Ok(DtNavMesh::from_ptr(mesh));
    };
}

pub fn save_nav_mesh(mesh: &DtNavMesh, path: &str) {
    unsafe { ffi::saveNavMesh(mesh.as_ptr(), path) };
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct RcChunkyTriMeshNode {
    pub bmin: [f32; 2],
    pub bmax: [f32; 2],
    pub i: i32,
    pub n: i32,
}

const_assert_eq!(mem::size_of::<RcChunkyTriMeshNode>(), 24);

unsafe impl ExternType for RcChunkyTriMeshNode {
    type Id = type_id!("rcChunkyTriMeshNode");
    type Kind = cxx::kind::Trivial;
}

#[repr(C)]
#[derive(Debug, Clone)]
pub(crate) struct CxxRcChunkyTriMesh {
    nodes: *mut RcChunkyTriMeshNode,
    nnodes: i32,
    tris: *mut i32,
    ntris: i32,
    max_tris_per_chunk: i32,
}

#[cfg(target_pointer_width = "64")]
const_assert_eq!(mem::size_of::<CxxRcChunkyTriMesh>(), 32);

#[cfg(target_pointer_width = "32")]
const_assert_eq!(mem::size_of::<CxxRcChunkyTriMesh>(), 20);

unsafe impl ExternType for CxxRcChunkyTriMesh {
    type Id = type_id!("rcChunkyTriMesh");
    type Kind = cxx::kind::Trivial;
}

#[derive(Debug)]
pub struct RcChunkyTriMesh(*mut CxxRcChunkyTriMesh);

impl Drop for RcChunkyTriMesh {
    fn drop(&mut self) {
        unsafe { ffi::rcctm_delete(self.0) };
        self.0 = std::ptr::null_mut();
    }
}

impl RcChunkyTriMesh {
    fn inner(&self) -> &CxxRcChunkyTriMesh {
        return unsafe { &*self.0 };
    }

    fn inner_mut(&mut self) -> Pin<&mut CxxRcChunkyTriMesh> {
        return unsafe { Pin::new_unchecked(&mut *self.0) };
    }

    pub fn new() -> RcChunkyTriMesh {
        return RcChunkyTriMesh(unsafe { ffi::rcctm_new() });
    }

    pub fn nodes(&self) -> &[RcChunkyTriMeshNode] {
        return unsafe { slice::from_raw_parts(self.inner().nodes, self.inner().nnodes as usize) };
    }

    pub fn nodes_mut(&mut self) -> &mut [RcChunkyTriMeshNode] {
        return unsafe { slice::from_raw_parts_mut(self.inner_mut().nodes, self.inner().nnodes as usize) };
    }

    pub fn tris(&self) -> &[[i32; 3]] {
        return unsafe { slice::from_raw_parts(self.inner().tris as *const _, self.inner().ntris as usize) };
    }

    pub fn tris_mut(&mut self) -> &mut [[i32; 3]] {
        return unsafe { slice::from_raw_parts_mut(self.inner_mut().tris as *mut _, self.inner().ntris as usize) };
    }

    pub fn max_tris_per_chunk(&self) -> usize {
        return self.inner().max_tris_per_chunk as usize;
    }
}

pub fn rc_create_chunky_tri_mesh(
    cm: &mut RcChunkyTriMesh,
    verts: &[[f32; 3]],
    tris: &[[i32; 3]],
    tris_per_chunk: i32,
) -> Result<(), XError> {
    let verts_ptr = verts.as_ptr() as *const f32;
    let tris_ptr = tris.as_ptr() as *const i32;
    let ntris = tris.len() as i32;
    let result = unsafe { ffi::rcCreateChunkyTriMesh(verts_ptr, tris_ptr, ntris, tris_per_chunk, cm.0) };
    if !result {
        return Err(XError::Failed);
    }
    return Ok(());
}

pub fn rc_get_chunks_overlapping_rect(
    cm: &RcChunkyTriMesh,
    bmin: &[f32; 2],
    bmax: &[f32; 2],
    ids: &mut [i32],
) -> usize {
    let n = unsafe {
        ffi::rcGetChunksOverlappingRect(cm.0, bmin.as_ptr(), bmax.as_ptr(), ids.as_mut_ptr(), ids.len() as i32)
    };
    return n as usize;
}

pub fn rc_get_chunks_overlapping_segment(cm: &RcChunkyTriMesh, p: &[f32; 2], q: &[f32; 2], ids: &mut [i32]) -> usize {
    let n =
        unsafe { ffi::rcGetChunksOverlappingSegment(cm.0, p.as_ptr(), q.as_ptr(), ids.as_mut_ptr(), ids.len() as i32) };
    return n as usize;
}
