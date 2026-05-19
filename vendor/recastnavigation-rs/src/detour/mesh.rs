use cxx::{type_id, ExternType};
use static_assertions::const_assert_eq;
use std::fmt::{self, Debug, Formatter};
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::{mem, ptr};

use crate::base::XError;
use crate::detour::base::{DtAABB, DtBuf};

pub const DT_VERTS_PER_POLYGON: usize = 6;

pub const DT_NAVMESH_MAGIC: u32 = (('D' as u32) << 24) | (('N' as u32) << 16) | (('A' as u32) << 8) | ('V' as u32);
pub const DT_NAVMESH_VERSION: u32 = 7;
pub const DT_NAVMESH_STATE_MAGIC: u32 =
    (('D' as u32) << 24) | (('N' as u32) << 16) | (('M' as u32) << 8) | ('S' as u32);
pub const DT_NAVMESH_STATE_VERSION: u32 = 1;

pub const DT_EXT_LINK: u16 = 0x8000;
pub const DT_NULL_LINK: u32 = 0xffffffff;

pub const DT_OFFMESH_CON_BIDIR: u32 = 1;

pub const DT_MAX_AREAS: usize = 64;

pub const DT_RAY_CAST_LIMIT_PROPORTIONS: f32 = 50.0;

const DT_TILE_FREE_DATA: i32 = 1;

#[cxx::bridge]
pub(crate) mod ffi {
    #[repr(u32)]
    enum dtPolyTypes {
        DT_POLYTYPE_GROUND = 0,
        DT_POLYTYPE_OFFMESH_CONNECTION = 1,
    }

    unsafe extern "C++" {
        include!("recastnavigation-rs/src/detour/detour-ffi.h");

        type dtStatus = crate::detour::base::ffi::dtStatus;

        type dtPolyRef = crate::detour::mesh::DtPolyRef;
        type dtTileRef = crate::detour::mesh::DtTileRef;

        type dtPolyTypes;

        type dtPoly = crate::detour::mesh::DtPoly;
        fn dtp_setArea(poly: Pin<&mut dtPoly>, a: u8);
        fn dtp_setType(poly: Pin<&mut dtPoly>, t: u8);
        fn dtp_getArea(poly: &dtPoly) -> u8;
        fn dtp_getType(poly: &dtPoly) -> u8;

        type dtPolyDetail = crate::detour::mesh::DtPolyDetail;
        type dtLink = crate::detour::mesh::DtLink;
        type dtBVNode = crate::detour::mesh::DtBVNode;
        type dtOffMeshConnection = crate::detour::mesh::DtOffMeshConnection;
        type dtMeshHeader = crate::detour::mesh::DtMeshHeader;
        type dtNavMeshParams = crate::detour::mesh::DtNavMeshParams;

        type dtMeshTile = crate::detour::mesh::CxxDtMeshTile;

        fn dtGetDetailTriEdgeFlags(triFlags: u8, edgeIndex: i32) -> i32;

        type dtNavMesh;
        fn dtAllocNavMesh() -> *mut dtNavMesh;
        unsafe fn dtFreeNavMesh(ptr: *mut dtNavMesh);
        #[rust_name = "init_with_params"]
        unsafe fn init(self: Pin<&mut dtNavMesh>, params: *const dtNavMeshParams) -> dtStatus;
        #[rust_name = "init_with_data"]
        unsafe fn init(self: Pin<&mut dtNavMesh>, data: *mut u8, dataSize: i32, flags: i32) -> dtStatus;
        fn getParams(self: &dtNavMesh) -> *const dtNavMeshParams;
        unsafe fn addTile(
            self: Pin<&mut dtNavMesh>,
            data: *mut u8,
            dataSize: i32,
            flags: i32,
            lastRef: dtTileRef,
            result: *mut dtTileRef,
        ) -> dtStatus;
        unsafe fn removeTile(
            self: Pin<&mut dtNavMesh>,
            re: dtTileRef,
            data: *mut *mut u8,
            dataSize: *mut i32,
        ) -> dtStatus;
        unsafe fn calcTileLoc(self: &dtNavMesh, pos: *const f32, tx: *mut i32, ty: *mut i32);
        unsafe fn getTileAt(self: &dtNavMesh, x: i32, y: i32, layer: i32) -> *const dtMeshTile;
        unsafe fn getTilesAt(self: &dtNavMesh, x: i32, y: i32, tiles: *mut *const dtMeshTile, maxTiles: i32) -> i32;
        fn getTileRefAt(self: &dtNavMesh, x: i32, y: i32, layer: i32) -> dtTileRef;
        unsafe fn getTileRef(self: &dtNavMesh, tile: *const dtMeshTile) -> dtTileRef;
        unsafe fn getTileByRef(self: &dtNavMesh, re: dtTileRef) -> *const dtMeshTile;
        fn getMaxTiles(self: &dtNavMesh) -> i32;
        unsafe fn getTile(self: &dtNavMesh, i: i32) -> *const dtMeshTile;
        unsafe fn getTileAndPolyByRef(
            self: &dtNavMesh,
            re: dtPolyRef,
            tile: *mut *const dtMeshTile,
            poly: *mut *const dtPoly,
        ) -> dtStatus;
        unsafe fn getTileAndPolyByRefUnsafe(
            self: &dtNavMesh,
            re: dtPolyRef,
            tile: *mut *const dtMeshTile,
            poly: *mut *const dtPoly,
        );
        fn isValidPolyRef(self: &dtNavMesh, re: dtPolyRef) -> bool;
        unsafe fn getPolyRefBase(self: &dtNavMesh, tile: *const dtMeshTile) -> dtPolyRef;
        unsafe fn getOffMeshConnectionPolyEndPoints(
            self: &dtNavMesh,
            prevRef: dtPolyRef,
            polyRef: dtPolyRef,
            startPos: *mut f32,
            endPos: *mut f32,
        ) -> dtStatus;
        fn getOffMeshConnectionByRef(self: &dtNavMesh, re: dtPolyRef) -> *const dtOffMeshConnection;
        fn setPolyFlags(self: Pin<&mut dtNavMesh>, re: dtPolyRef, flags: u16) -> dtStatus;
        unsafe fn getPolyFlags(self: &dtNavMesh, re: dtPolyRef, resultFlags: *mut u16) -> dtStatus;
        fn setPolyArea(self: Pin<&mut dtNavMesh>, re: dtPolyRef, area: u8) -> dtStatus;
        unsafe fn getPolyArea(self: &dtNavMesh, re: dtPolyRef, resultArea: *mut u8) -> dtStatus;
        unsafe fn getTileStateSize(self: &dtNavMesh, tile: *const dtMeshTile) -> i32;
        fn encodePolyId(self: &dtNavMesh, salt: u32, it: u32, ip: u32) -> dtPolyRef;
        unsafe fn decodePolyId(self: &dtNavMesh, re: dtPolyRef, salt: &mut u32, it: &mut u32, ip: &mut u32);
        fn decodePolyIdSalt(self: &dtNavMesh, re: dtPolyRef) -> u32;
        fn decodePolyIdTile(self: &dtNavMesh, re: dtPolyRef) -> u32;
        fn decodePolyIdPoly(self: &dtNavMesh, re: dtPolyRef) -> u32;
        unsafe fn dtmt_storeTileState(navMesh: &dtNavMesh, re: dtTileRef, data: *mut u8, maxDataSize: i32) -> dtStatus;
        unsafe fn dtmt_restoreTileState(
            navMesh: Pin<&mut dtNavMesh>,
            re: dtTileRef,
            data: *const u8,
            maxDataSize: i32,
        ) -> dtStatus;
    }
}

pub type DtPolyTypes = ffi::dtPolyTypes;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize))]
pub struct DtPolyRef(pub u32);

unsafe impl ExternType for DtPolyRef {
    type Id = type_id!("dtPolyRef");
    type Kind = cxx::kind::Trivial;
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DtTileRef(pub u32);

unsafe impl ExternType for DtTileRef {
    type Id = type_id!("dtTileRef");
    type Kind = cxx::kind::Trivial;
}

impl DtPolyRef {
    pub fn is_null(&self) -> bool {
        return self.0 == 0;
    }
}

//
// DtPoly
//

#[repr(C)]
#[derive(Debug, Clone)]
pub struct DtPoly {
    pub first_link: u32,
    pub verts: [u16; DT_VERTS_PER_POLYGON],
    pub neis: [u16; DT_VERTS_PER_POLYGON],
    pub flags: u16,
    pub vert_count: u8,
    pub area_and_type: u8,
}

const_assert_eq!(mem::size_of::<DtPoly>(), 32);

unsafe impl ExternType for DtPoly {
    type Id = type_id!("dtPoly");
    type Kind = cxx::kind::Trivial;
}

impl DtPoly {
    pub fn set_area(&mut self, a: u8) {
        return ffi::dtp_setArea(Pin::new(self), a);
    }

    pub fn set_type(&mut self, t: DtPolyTypes) {
        return ffi::dtp_setType(Pin::new(self), t.repr as u32 as u8);
    }

    pub fn area(&self) -> u8 {
        return ffi::dtp_getArea(self);
    }

    pub fn typ(&self) -> DtPolyTypes {
        let t = ffi::dtp_getType(self) as u32;
        if t == DtPolyTypes::DT_POLYTYPE_OFFMESH_CONNECTION.repr as u32 {
            return DtPolyTypes::DT_POLYTYPE_OFFMESH_CONNECTION;
        } else {
            return DtPolyTypes::DT_POLYTYPE_GROUND;
        };
    }
}

//
// DtPolyDetail
//

#[repr(C)]
#[derive(Debug, Clone)]
pub struct DtPolyDetail {
    pub vert_base: u32,
    pub tri_base: u32,
    pub vert_count: u8,
    pub tri_count: u8,
}

const_assert_eq!(mem::size_of::<DtPolyDetail>(), 12);

unsafe impl ExternType for DtPolyDetail {
    type Id = type_id!("dtPolyDetail");
    type Kind = cxx::kind::Trivial;
}

//
// DtLink
//

#[repr(C)]
#[derive(Debug, Clone)]
pub struct DtLink {
    pub re: DtPolyRef,
    pub next: u32,
    pub edge: u8,
    pub side: u8,
    pub bmin: u8,
    pub bmax: u8,
}

const_assert_eq!(mem::size_of::<DtLink>(), 12);

unsafe impl ExternType for DtLink {
    type Id = type_id!("dtLink");
    type Kind = cxx::kind::Trivial;
}

//
// DtBVNode
//

#[repr(C)]
#[derive(Debug, Clone)]
pub struct DtBVNode {
    pub bmin: [u16; 3],
    pub bmax: [u16; 3],
    pub i: i32,
}

const_assert_eq!(mem::size_of::<DtBVNode>(), 16);

unsafe impl ExternType for DtBVNode {
    type Id = type_id!("dtBVNode");
    type Kind = cxx::kind::Trivial;
}

//
// DtOffMeshConnection
//

#[repr(C)]
#[derive(Debug, Clone)]
pub struct DtOffMeshConnection {
    pub pos: DtAABB,
    pub rad: f32,
    pub poly: u16,
    pub flags: u8,
    pub side: u8,
    pub user_id: u32,
}

const_assert_eq!(mem::size_of::<DtOffMeshConnection>(), 36);

unsafe impl ExternType for DtOffMeshConnection {
    type Id = type_id!("dtOffMeshConnection");
    type Kind = cxx::kind::Trivial;
}

//
// DtMeshHeader
//

#[repr(C)]
#[derive(Debug, Clone, PartialEq)]
pub struct DtMeshHeader {
    pub magic: i32,
    pub version: i32,
    pub x: i32,
    pub y: i32,
    pub layer: i32,
    pub user_id: u32,
    pub poly_count: i32,
    pub vert_count: i32,
    pub max_link_count: i32,
    pub detail_mesh_count: i32,
    pub detail_vert_count: i32,
    pub detail_tri_count: i32,
    pub bv_node_count: i32,
    pub off_mesh_con_count: i32,
    pub off_mesh_base: i32,
    pub walkable_height: f32,
    pub walkable_radius: f32,
    pub walkable_climb: f32,
    pub bmin: [f32; 3],
    pub bmax: [f32; 3],
    pub bv_quant_factor: f32,
}

const_assert_eq!(mem::size_of::<DtMeshHeader>(), 100);

unsafe impl ExternType for DtMeshHeader {
    type Id = type_id!("dtMeshHeader");
    type Kind = cxx::kind::Trivial;
}

//
// DtMeshTile
//

#[repr(C)]
#[derive(Debug)]
pub struct CxxDtMeshTile {
    pub salt: u32,
    pub links_free_list: u32,
    header: *mut ffi::dtMeshHeader,
    polys: *mut ffi::dtPoly,
    verts: *mut [f32; 3],
    links: *mut ffi::dtLink,
    detail_meshes: *mut ffi::dtPolyDetail,
    detail_verts: *mut [f32; 3],
    detail_tris: *mut [u8; 4],
    bv_tree: *mut ffi::dtBVNode,
    off_mesh_cons: *mut ffi::dtOffMeshConnection,
    data: *mut u8,
    data_size: i32,
    _flags: i32,
    next: *mut ffi::dtMeshTile,
}

#[cfg(target_pointer_width = "64")]
const_assert_eq!(mem::size_of::<CxxDtMeshTile>(), 104);

#[cfg(target_pointer_width = "32")]
const_assert_eq!(mem::size_of::<CxxDtMeshTile>(), 60);

unsafe impl ExternType for CxxDtMeshTile {
    type Id = type_id!("dtMeshTile");
    type Kind = cxx::kind::Trivial;
}

pub struct DtMeshTile(CxxDtMeshTile);

impl Deref for DtMeshTile {
    type Target = CxxDtMeshTile;

    fn deref(&self) -> &Self::Target {
        return self.inner();
    }
}

impl DerefMut for DtMeshTile {
    fn deref_mut(&mut self) -> &mut Self::Target {
        return self.inner_mut().get_mut();
    }
}

impl Debug for DtMeshTile {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        return self.inner().fmt(f);
    }
}

impl DtMeshTile {
    fn inner(&self) -> &ffi::dtMeshTile {
        return &self.0;
    }

    fn inner_mut(&mut self) -> Pin<&mut ffi::dtMeshTile> {
        return unsafe { Pin::new_unchecked(&mut self.0) };
    }

    pub fn as_ptr(&self) -> *const ffi::dtMeshTile {
        return &self.0;
    }

    pub fn as_mut_ptr(&mut self) -> *mut ffi::dtMeshTile {
        return &mut self.0;
    }

    pub fn header(&self) -> Option<&DtMeshHeader> {
        unsafe {
            if self.header.is_null() {
                return None;
            } else {
                return Some(&*self.header);
            }
        };
    }

    pub fn header_mut(&mut self) -> Option<&mut DtMeshHeader> {
        unsafe {
            if self.header.is_null() {
                return None;
            } else {
                return Some(&mut *self.header);
            }
        };
    }

    pub fn polys(&self) -> &[DtPoly] {
        let poly_count = self.header().map_or(0, |h| h.poly_count as usize);
        return unsafe { std::slice::from_raw_parts(self.polys, poly_count) };
    }

    pub fn polys_mut(&mut self) -> &mut [DtPoly] {
        let poly_count = self.header().map_or(0, |h| h.poly_count as usize);
        return unsafe { std::slice::from_raw_parts_mut(self.polys, poly_count) };
    }

    pub fn verts(&self) -> &[[f32; 3]] {
        let vert_count = self.header().map_or(0, |h| h.vert_count as usize);
        return unsafe { std::slice::from_raw_parts(self.verts, vert_count) };
    }

    pub fn verts_mut(&mut self) -> &mut [[f32; 3]] {
        let vert_count = self.header().map_or(0, |h| h.vert_count as usize);
        return unsafe { std::slice::from_raw_parts_mut(self.verts, vert_count) };
    }

    pub fn links(&self) -> &[DtLink] {
        let link_count = self.header().map_or(0, |h| h.max_link_count as usize);
        return unsafe { std::slice::from_raw_parts(self.links, link_count) };
    }

    pub fn links_mut(&mut self) -> &mut [DtLink] {
        let link_count = self.header().map_or(0, |h| h.max_link_count as usize);
        return unsafe { std::slice::from_raw_parts_mut(self.links, link_count) };
    }

    pub fn detail_meshes(&self) -> &[DtPolyDetail] {
        let detail_mesh_count = self.header().map_or(0, |h| h.detail_mesh_count as usize);
        return unsafe { std::slice::from_raw_parts(self.detail_meshes, detail_mesh_count) };
    }

    pub fn detail_meshes_mut(&mut self) -> &mut [DtPolyDetail] {
        let detail_mesh_count = self.header().map_or(0, |h| h.detail_mesh_count as usize);
        return unsafe { std::slice::from_raw_parts_mut(self.detail_meshes, detail_mesh_count) };
    }

    pub fn detail_verts(&self) -> &[[f32; 3]] {
        let detail_vert_count = self.header().map_or(0, |h| h.detail_vert_count as usize);
        return unsafe { std::slice::from_raw_parts(self.detail_verts, detail_vert_count) };
    }

    pub fn detail_verts_mut(&mut self) -> &mut [[f32; 3]] {
        let detail_vert_count = self.header().map_or(0, |h| h.detail_vert_count as usize);
        return unsafe { std::slice::from_raw_parts_mut(self.detail_verts, detail_vert_count) };
    }

    pub fn detail_tris(&self) -> &[[u8; 4]] {
        let detail_tri_count = self.header().map_or(0, |h| h.detail_tri_count as usize);
        return unsafe { std::slice::from_raw_parts(self.detail_tris, detail_tri_count) };
    }

    pub fn detail_tris_mut(&mut self) -> &mut [[u8; 4]] {
        let detail_tri_count = self.header().map_or(0, |h| h.detail_tri_count as usize);
        return unsafe { std::slice::from_raw_parts_mut(self.detail_tris, detail_tri_count) };
    }

    pub fn bv_tree(&self) -> &[DtBVNode] {
        let bv_node_count = self.header().map_or(0, |h| h.bv_node_count as usize);
        return unsafe { std::slice::from_raw_parts(self.bv_tree, bv_node_count) };
    }

    pub fn bv_tree_mut(&mut self) -> &mut [DtBVNode] {
        let bv_node_count = self.header().map_or(0, |h| h.bv_node_count as usize);
        return unsafe { std::slice::from_raw_parts_mut(self.bv_tree, bv_node_count) };
    }

    pub fn off_mesh_cons(&self) -> &[DtOffMeshConnection] {
        let off_mesh_con_count = self.header().map_or(0, |h| h.off_mesh_con_count as usize);
        return unsafe { std::slice::from_raw_parts(self.off_mesh_cons, off_mesh_con_count) };
    }

    pub fn off_mesh_cons_mut(&mut self) -> &mut [DtOffMeshConnection] {
        let off_mesh_con_count = self.header().map_or(0, |h| h.off_mesh_con_count as usize);
        return unsafe { std::slice::from_raw_parts_mut(self.off_mesh_cons, off_mesh_con_count) };
    }

    pub fn data(&self) -> &[u8] {
        return unsafe { std::slice::from_raw_parts(self.data, self.data_size()) };
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        return unsafe { std::slice::from_raw_parts_mut(self.data, self.data_size()) };
    }

    pub fn data_size(&self) -> usize {
        return self.data_size as usize;
    }

    pub fn next(&self) -> Option<&DtMeshTile> {
        if self.next.is_null() {
            return None;
        } else {
            return Some(unsafe { mem::transmute(self.next) });
        }
    }

    pub fn next_mut(&mut self) -> Option<&mut DtMeshTile> {
        if self.next.is_null() {
            return None;
        } else {
            return Some(unsafe { mem::transmute(self.next) });
        }
    }
}

pub fn get_detail_tri_edge_flags(tri_flags: u8, edge_index: i32) -> i32 {
    return ffi::dtGetDetailTriEdgeFlags(tri_flags, edge_index);
}

//
// DtNavMeshParams
//

#[repr(C)]
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DtNavMeshParams {
    pub orig: [f32; 3],
    pub tile_width: f32,
    pub tile_height: f32,
    pub max_tiles: i32,
    pub max_polys: i32,
}

const_assert_eq!(mem::size_of::<DtNavMeshParams>(), 28);

unsafe impl ExternType for DtNavMeshParams {
    type Id = type_id!("dtNavMeshParams");
    type Kind = cxx::kind::Trivial;
}

//
// DtNavMesh
//

#[derive(Debug)]
pub struct DtNavMesh(*mut ffi::dtNavMesh);

impl Drop for DtNavMesh {
    fn drop(&mut self) {
        unsafe { ffi::dtFreeNavMesh(self.0) };
        self.0 = std::ptr::null_mut();
    }
}

impl DtNavMesh {
    pub fn new() -> DtNavMesh {
        return DtNavMesh(ffi::dtAllocNavMesh());
    }

    pub(crate) unsafe fn from_ptr(ptr: *mut ffi::dtNavMesh) -> DtNavMesh {
        return DtNavMesh(ptr);
    }

    fn inner(&self) -> &ffi::dtNavMesh {
        return unsafe { &*self.0 };
    }

    fn inner_mut(&mut self) -> Pin<&mut ffi::dtNavMesh> {
        return unsafe { Pin::new_unchecked(&mut *self.0) };
    }

    pub fn as_ptr(&self) -> *const ffi::dtNavMesh {
        return self.0;
    }

    pub fn as_mut_ptr(&mut self) -> *mut ffi::dtNavMesh {
        return self.0;
    }

    pub fn with_params(params: &DtNavMeshParams) -> Result<DtNavMesh, XError> {
        let mut mesh = DtNavMesh::new();
        unsafe { mesh.inner_mut().init_with_params(params) }.to_result()?;
        return Ok(mesh);
    }

    pub fn with_data(buf: DtBuf) -> Result<DtNavMesh, XError> {
        let mut mesh = DtNavMesh::new();
        unsafe {
            mesh.inner_mut()
                .init_with_data(buf.data, buf.len() as i32, DT_TILE_FREE_DATA)
        }
        .to_result()?;
        mem::forget(buf);
        return Ok(mesh);
    }

    pub fn params(&self) -> &DtNavMeshParams {
        return unsafe { &*self.inner().getParams() };
    }

    pub fn add_tile(&mut self, buf: DtBuf, last_ref: DtTileRef) -> Result<DtTileRef, XError> {
        let mut re = DtTileRef::default();
        unsafe {
            self.inner_mut()
                .addTile(buf.data, buf.len() as i32, DT_TILE_FREE_DATA, last_ref, &mut re)
        }
        .to_result()?;
        mem::forget(buf);
        return Ok(re);
    }

    pub fn remove_tile(&mut self, re: DtTileRef) -> Result<(), XError> {
        return unsafe { self.inner_mut().removeTile(re, ptr::null_mut(), ptr::null_mut()) }.to_result();
    }

    pub fn calc_tile_loc(&self, pos: &[f32; 3]) -> [i32; 2] {
        let mut t: [i32; 2] = [0, 0];
        unsafe { self.inner().calcTileLoc(pos.as_ptr(), &mut t[0], &mut t[1]) };
        return t;
    }

    pub fn get_tile_at(&self, x: i32, y: i32, layer: i32) -> Option<&DtMeshTile> {
        let tile = unsafe { self.inner().getTileAt(x, y, layer) };
        if tile.is_null() {
            return None;
        } else {
            return Some(unsafe { mem::transmute(tile) });
        }
    }

    pub fn get_tiles_at<'a, 'b: 'a>(&'b self, x: i32, y: i32, tiles: &mut [Option<&'a DtMeshTile>]) -> usize {
        let count = unsafe {
            self.inner()
                .getTilesAt(x, y, tiles.as_mut_ptr() as *mut _, tiles.len() as i32)
        };
        return count as usize;
    }

    pub fn get_tile_ref_at(&self, x: i32, y: i32, layer: i32) -> DtTileRef {
        return self.inner().getTileRefAt(x, y, layer);
    }

    pub fn get_tile_ref(&self, tile: &DtMeshTile) -> DtTileRef {
        return unsafe { self.inner().getTileRef(tile.inner()) };
    }

    pub fn get_tile_by_ref(&self, re: DtTileRef) -> Option<&DtMeshTile> {
        let tile = unsafe { self.inner().getTileByRef(re) };
        if tile.is_null() {
            return None;
        } else {
            return Some(unsafe { mem::transmute(tile) });
        }
    }

    pub fn max_tiles(&self) -> i32 {
        return self.inner().getMaxTiles();
    }

    pub fn get_tile(&self, i: i32) -> Option<&DtMeshTile> {
        let tile = unsafe { self.inner().getTile(i) };
        if tile.is_null() {
            return None;
        } else {
            return Some(unsafe { mem::transmute(tile) });
        }
    }

    pub fn get_tile_and_poly_by_ref(&self, re: DtPolyRef) -> Result<(&DtMeshTile, &DtPoly), XError> {
        let mut tile = std::ptr::null();
        let mut poly = std::ptr::null();
        unsafe { self.inner().getTileAndPolyByRef(re, &mut tile, &mut poly) }.to_result()?;
        return Ok(unsafe { (mem::transmute(tile), &*poly) });
    }

    pub unsafe fn get_tile_and_poly_by_ref_unsafe(&self, re: DtPolyRef) -> (&DtMeshTile, &DtPoly) {
        let mut tile = std::ptr::null();
        let mut poly = std::ptr::null();
        unsafe { self.inner().getTileAndPolyByRefUnsafe(re, &mut tile, &mut poly) };
        return (mem::transmute(tile), unsafe { &*poly });
    }

    pub fn is_valid_poly_ref(&self, re: DtPolyRef) -> bool {
        return self.inner().isValidPolyRef(re);
    }

    pub unsafe fn get_poly_ref_base(&self, tile: &DtMeshTile) -> DtPolyRef {
        return self.inner().getPolyRefBase(tile.inner());
    }

    pub fn get_off_mesh_connection_poly_end_points(
        &self,
        prev_ref: DtPolyRef,
        poly_ref: DtPolyRef,
    ) -> Result<([f32; 3], [f32; 3]), XError> {
        let mut start_pos = [0.0; 3];
        let mut end_pos = [0.0; 3];
        unsafe {
            self.inner()
                .getOffMeshConnectionPolyEndPoints(prev_ref, poly_ref, &mut start_pos[0], &mut end_pos[0])
        }
        .to_result()?;
        return Ok((start_pos, end_pos));
    }

    pub fn get_off_mesh_connection_by_ref(&self, re: DtPolyRef) -> &DtOffMeshConnection {
        return unsafe { &*self.inner().getOffMeshConnectionByRef(re) };
    }

    pub fn set_poly_flags(&mut self, re: DtPolyRef, flags: u16) -> Result<(), XError> {
        return self.inner_mut().setPolyFlags(re, flags).to_result();
    }

    pub fn get_poly_flags(&self, re: DtPolyRef) -> Result<u16, XError> {
        let mut flags = 0;
        unsafe { self.inner().getPolyFlags(re, &mut flags) }.to_result()?;
        return Ok(flags);
    }

    pub fn set_poly_area(&mut self, re: DtPolyRef, area: u8) -> Result<(), XError> {
        return self.inner_mut().setPolyArea(re, area).to_result();
    }

    pub fn get_poly_area(&self, re: DtPolyRef) -> Result<u8, XError> {
        let mut area = 0;
        unsafe { self.inner().getPolyArea(re, &mut area) }.to_result()?;
        return Ok(area);
    }

    pub unsafe fn get_tile_state_size(&self, tile: &DtMeshTile) -> usize {
        return self.inner().getTileStateSize(tile.inner()) as usize;
    }

    pub unsafe fn store_tile_state(&self, re: DtTileRef, data: &mut [u8]) -> Result<(), XError> {
        return unsafe { ffi::dtmt_storeTileState(self.inner(), re, data.as_mut_ptr(), data.len() as i32) }.to_result();
    }

    pub unsafe fn restore_tile_state(&mut self, re: DtTileRef, data: &[u8]) -> Result<(), XError> {
        return unsafe { ffi::dtmt_restoreTileState(self.inner_mut(), re, data.as_ptr(), data.len() as i32) }
            .to_result();
    }

    pub fn encode_poly_id(&self, salt: u32, it: u32, ip: u32) -> DtPolyRef {
        return self.inner().encodePolyId(salt, it, ip);
    }

    pub unsafe fn decode_poly_id(&self, re: DtPolyRef) -> (u32, u32, u32) {
        let mut salt = 0;
        let mut it = 0;
        let mut ip = 0;
        self.inner().decodePolyId(re, &mut salt, &mut it, &mut ip);
        return (salt, it, ip);
    }

    pub fn decode_poly_id_salt(&self, re: DtPolyRef) -> u32 {
        return self.inner().decodePolyIdSalt(re);
    }

    pub fn decode_poly_id_tile(&self, re: DtPolyRef) -> u32 {
        return self.inner().decodePolyIdTile(re);
    }

    pub fn decode_poly_id_poly(&self, re: DtPolyRef) -> u32 {
        return self.inner().decodePolyIdPoly(re);
    }
}
