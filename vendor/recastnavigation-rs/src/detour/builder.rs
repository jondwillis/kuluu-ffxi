use cxx::{type_id, ExternType};
use static_assertions::const_assert_eq;
use std::fmt::{self, Debug, Formatter};
use std::mem;

use crate::base::XError;
use crate::detour::base::{DtAABB, DtBuf};
use crate::detour::mesh::DT_VERTS_PER_POLYGON;

#[cxx::bridge]
pub(crate) mod ffi {
    unsafe extern "C++" {
        include!("recastnavigation-rs/src/detour/detour-ffi.h");

        type dtNavMeshCreateParams = crate::detour::builder::CxxDtNavMeshCreateParams;

        unsafe fn dtCreateNavMeshData(
            params: *mut dtNavMeshCreateParams,
            outData: *mut *mut u8,
            outDataSize: *mut i32,
        ) -> bool;
        unsafe fn dtNavMeshHeaderSwapEndian(data: *mut u8, dataSize: i32) -> bool;
        unsafe fn dtNavMeshDataSwapEndian(data: *mut u8, dataSize: i32) -> bool;
    }
}

// Represents the source data used to build a navigation mesh tile.
#[derive(Default)]
pub struct DtNavMeshCreateParams<'t> {
    pub verts: Option<&'t [[u16; 3]]>,
    pub polys: Option<&'t [u16]>,
    pub poly_flags: Option<&'t [u16]>,
    pub poly_areas: Option<&'t [u8]>,
    pub nvp: usize,

    pub detail_meshes: Option<&'t [[u32; 4]]>,
    pub detail_verts: Option<&'t [[f32; 3]]>,
    pub detail_tris: Option<&'t [[u8; 4]]>,

    pub off_mesh_con_verts: Option<&'t [DtAABB]>,
    pub off_mesh_con_rad: Option<&'t [f32]>,
    pub off_mesh_con_flags: Option<&'t [u16]>,
    pub off_mesh_con_areas: Option<&'t [u8]>,
    pub off_mesh_con_dir: Option<&'t [u8]>,
    pub off_mesh_con_user_id: Option<&'t [u32]>,

    pub user_id: u32,
    pub tile_x: i32,
    pub tile_y: i32,
    pub tile_layer: i32,
    pub bmin: [f32; 3],
    pub bmax: [f32; 3],

    pub walkable_height: f32,
    pub walkable_radius: f32,
    pub walkable_climb: f32,
    pub cs: f32,
    pub ch: f32,

    pub build_bv_tree: bool,
}

impl Debug for DtNavMeshCreateParams<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let cp = CxxDtNavMeshCreateParams::from(self);
        return write!(f, "{:?}", cp);
    }
}

fn unpack_ptr<T>(v: Option<&[T]>) -> *const T {
    return v.map(|v| v.as_ptr()).unwrap_or(std::ptr::null());
}

fn unpack_len<T>(v: Option<&[T]>) -> i32 {
    return v.map(|v| v.len() as i32).unwrap_or(0);
}

#[repr(C)]
#[derive(Debug)]
pub(crate) struct CxxDtNavMeshCreateParams {
    verts: *const u16,
    vert_count: i32,
    polys: *const u16,
    poly_flags: *const u16,
    poly_areas: *const u8,
    poly_count: i32,
    nvp: i32,

    detail_meshes: *const u32,
    detail_verts: *const f32,
    detail_verts_count: i32,
    detail_tris: *const u8,
    detail_tri_count: i32,

    off_mesh_con_verts: *const f32,
    off_mesh_con_rad: *const f32,
    off_mesh_con_flags: *const u16,
    off_mesh_con_areas: *const u8,
    off_mesh_con_dir: *const u8,
    off_mesh_con_user_id: *const u32,
    off_mesh_con_count: i32,

    user_id: u32,
    tile_x: i32,
    tile_y: i32,
    tile_layer: i32,
    bmin: [f32; 3],
    bmax: [f32; 3],

    walkable_height: f32,
    walkable_radius: f32,
    walkable_climb: f32,
    cs: f32,
    ch: f32,

    build_bv_tree: bool,
}

#[cfg(target_pointer_width = "32")]
const_assert_eq!(mem::size_of::<CxxDtNavMeshCreateParams>(), 140);

#[cfg(target_pointer_width = "64")]
const_assert_eq!(mem::size_of::<CxxDtNavMeshCreateParams>(), 208);

unsafe impl ExternType for CxxDtNavMeshCreateParams {
    type Id = type_id!("dtNavMeshCreateParams");
    type Kind = cxx::kind::Trivial;
}

impl CxxDtNavMeshCreateParams {
    fn from(params: &DtNavMeshCreateParams<'_>) -> CxxDtNavMeshCreateParams {
        return CxxDtNavMeshCreateParams {
            verts: unpack_ptr(params.verts) as *const _,
            vert_count: unpack_len(params.verts),
            polys: unpack_ptr(params.polys),
            poly_flags: unpack_ptr(params.poly_flags),
            poly_areas: unpack_ptr(params.poly_areas),
            poly_count: unpack_len(params.poly_flags),
            nvp: params.nvp as i32,

            detail_meshes: unpack_ptr(params.detail_meshes) as *const _,
            detail_verts: unpack_ptr(params.detail_verts) as *const _,
            detail_verts_count: unpack_len(params.detail_verts),
            detail_tris: unpack_ptr(params.detail_tris) as *const _,
            detail_tri_count: unpack_len(params.detail_tris),

            off_mesh_con_verts: unpack_ptr(params.off_mesh_con_verts) as *const _,
            off_mesh_con_rad: unpack_ptr(params.off_mesh_con_rad),
            off_mesh_con_flags: unpack_ptr(params.off_mesh_con_flags),
            off_mesh_con_areas: unpack_ptr(params.off_mesh_con_areas),
            off_mesh_con_dir: unpack_ptr(params.off_mesh_con_dir),
            off_mesh_con_user_id: unpack_ptr(params.off_mesh_con_user_id),
            off_mesh_con_count: unpack_len(params.off_mesh_con_verts),

            user_id: params.user_id,
            tile_x: params.tile_x,
            tile_y: params.tile_y,
            tile_layer: params.tile_layer,
            bmin: params.bmin,
            bmax: params.bmax,

            walkable_height: params.walkable_height,
            walkable_radius: params.walkable_radius,
            walkable_climb: params.walkable_climb,
            cs: params.cs,
            ch: params.ch,

            build_bv_tree: params.build_bv_tree,
        };
    }
}

pub fn dt_create_nav_mesh_data(params: &mut DtNavMeshCreateParams) -> Result<DtBuf, XError> {
    let mut cp = CxxDtNavMeshCreateParams::from(params);

    if cp.vert_count < 3 || cp.vert_count >= 0xFFFF {
        return Err(XError::InvalidParam);
    }

    if cp.poly_count < 1 {
        return Err(XError::InvalidParam);
    }
    if unpack_len(params.polys) != cp.poly_count * 2 * cp.nvp {
        return Err(XError::InvalidParam);
    }
    if unpack_len(params.poly_flags) != cp.poly_count {
        return Err(XError::InvalidParam);
    }
    if unpack_len(params.poly_areas) != cp.poly_count {
        return Err(XError::InvalidParam);
    }
    if cp.nvp < 3 || cp.nvp > DT_VERTS_PER_POLYGON as i32 {
        return Err(XError::InvalidParam);
    }

    if cp.detail_meshes.is_null() {
        if unpack_len(params.detail_meshes) != cp.poly_count {
            return Err(XError::InvalidParam);
        }
    }

    if unpack_len(params.off_mesh_con_rad) != cp.off_mesh_con_count {
        return Err(XError::InvalidParam);
    }
    if unpack_len(params.off_mesh_con_flags) != cp.off_mesh_con_count {
        return Err(XError::InvalidParam);
    }
    if unpack_len(params.off_mesh_con_areas) != cp.off_mesh_con_count {
        return Err(XError::InvalidParam);
    }
    if unpack_len(params.off_mesh_con_dir) != cp.off_mesh_con_count {
        return Err(XError::InvalidParam);
    }
    if unpack_len(params.off_mesh_con_user_id) != cp.off_mesh_con_count {
        return Err(XError::InvalidParam);
    }

    unsafe {
        let mut buf = DtBuf::default();
        let res = ffi::dtCreateNavMeshData((&mut cp) as *mut _, &mut buf.data, &mut buf.size);
        if !res {
            return Err(XError::Failed);
        }
        return Ok(buf);
    }
}

pub fn dt_nav_mesh_header_swap_endian(buf: &mut DtBuf) -> Result<(), XError> {
    let res = unsafe { ffi::dtNavMeshHeaderSwapEndian(buf.data, buf.size) };
    if !res {
        return Err(XError::Failed);
    }
    return Ok(());
}

pub fn dt_nav_mesh_data_swap_endian(buf: &mut DtBuf) -> Result<(), XError> {
    let res = unsafe { ffi::dtNavMeshDataSwapEndian(buf.data, buf.size) };
    if !res {
        return Err(XError::Failed);
    }
    return Ok(());
}
