use cxx::{type_id, ExternType, UniquePtr};
use static_assertions::const_assert_eq;
use std::fmt::{self, Debug, Formatter};
use std::mem;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::slice;

use crate::base::XError;

pub const RC_SPAN_HEIGHT_BITS: u32 = 13;
pub const RC_SPAN_MAX_HEIGHT: u32 = (1 << RC_SPAN_HEIGHT_BITS) - 1;
pub const RC_SPANS_PER_POOL: usize = 2048;

pub const RC_BORDER_REG: u16 = 0x8000;
pub const RC_MULTIPLE_REGS: u16 = 0;
pub const RC_BORDER_VERTEX: i32 = 0x10000;
pub const RC_AREA_BORDER: i32 = 0x20000;
pub const RC_CONTOUR_REG_MASK: i32 = 0xffff;
pub const RC_MESH_NULL_IDX: u16 = 0xffff;
pub const RC_NULL_AREA: u8 = 0;
pub const RC_WALKABLE_AREA: u8 = 63;
pub const RC_NOT_CONNECTED: i32 = 0x3f;

#[allow(dead_code)]
#[cxx::bridge]
pub(crate) mod ffi {
    // Recast log categories.
    #[repr(u32)]
    enum rcLogCategory {
        RC_LOG_PROGRESS = 1,
        RC_LOG_WARNING,
        RC_LOG_ERROR,
    }

    // Recast performance timer categories.
    #[repr(u32)]
    enum rcTimerLabel {
        RC_TIMER_TOTAL,
        RC_TIMER_TEMP,
        RC_TIMER_RASTERIZE_TRIANGLES,
        RC_TIMER_BUILD_COMPACTHEIGHTFIELD,
        RC_TIMER_BUILD_CONTOURS,
        RC_TIMER_BUILD_CONTOURS_TRACE,
        RC_TIMER_BUILD_CONTOURS_SIMPLIFY,
        RC_TIMER_FILTER_BORDER,
        RC_TIMER_FILTER_WALKABLE,
        RC_TIMER_MEDIAN_AREA,
        RC_TIMER_FILTER_LOW_OBSTACLES,
        RC_TIMER_BUILD_POLYMESH,
        RC_TIMER_MERGE_POLYMESH,
        RC_TIMER_ERODE_AREA,
        RC_TIMER_MARK_BOX_AREA,
        RC_TIMER_MARK_CYLINDER_AREA,
        RC_TIMER_MARK_CONVEXPOLY_AREA,
        RC_TIMER_BUILD_DISTANCEFIELD,
        RC_TIMER_BUILD_DISTANCEFIELD_DIST,
        RC_TIMER_BUILD_DISTANCEFIELD_BLUR,
        RC_TIMER_BUILD_REGIONS,
        RC_TIMER_BUILD_REGIONS_WATERSHED,
        RC_TIMER_BUILD_REGIONS_EXPAND,
        RC_TIMER_BUILD_REGIONS_FLOOD,
        RC_TIMER_BUILD_REGIONS_FILTER,
        RC_TIMER_BUILD_LAYERS,
        RC_TIMER_BUILD_POLYMESHDETAIL,
        RC_TIMER_MERGE_POLYMESHDETAIL,
        RC_MAX_TIMERS,
    }

    #[repr(i32)]
    enum rcBuildContoursFlags {
        RC_CONTOUR_TESS_WALL_EDGES = 0x01,
        RC_CONTOUR_TESS_AREA_EDGES = 0x02,
    }

    unsafe extern "C++" {
        include!("recastnavigation-rs/src/recast/recast-ffi.h");

        //
        // enums
        //

        type rcLogCategory;
        type rcTimerLabel;
        type rcBuildContoursFlags;

        //
        // structs
        //

        type rcContext;
        fn rcNewContext(state: bool) -> UniquePtr<rcContext>;
        fn enableLog(self: Pin<&mut rcContext>, state: bool);
        fn resetLog(self: Pin<&mut rcContext>);
        // fn log(self: Pin<&mut rcContext>, category: rcLogCategory, format: &str);
        fn enableTimer(self: Pin<&mut rcContext>, state: bool);
        fn resetTimers(self: Pin<&mut rcContext>);
        fn startTimer(self: Pin<&mut rcContext>, label: rcTimerLabel);
        fn stopTimer(self: Pin<&mut rcContext>, label: rcTimerLabel);
        fn getAccumulatedTime(self: &rcContext, label: rcTimerLabel) -> i32;

        type rcConfig = crate::recast::recast::RcConfig;

        type rcSpan = crate::recast::recast::RcSpan;
        type rcSpanPool = crate::recast::recast::RcSpanPool;

        type rcHeightfield = crate::recast::recast::CxxRcHeightfield;
        unsafe fn rcAllocHeightfield() -> *mut rcHeightfield;
        unsafe fn rcFreeHeightField(heightfield: *mut rcHeightfield);

        type rcCompactCell = crate::recast::recast::RcCompactCell;
        type rcCompactSpan = crate::recast::recast::RcCompactSpan;

        type rcCompactHeightfield = crate::recast::recast::CxxRcCompactHeightfield;
        unsafe fn rcAllocCompactHeightfield() -> *mut rcCompactHeightfield;
        unsafe fn rcFreeCompactHeightfield(compactHeightfield: *mut rcCompactHeightfield);

        type rcHeightfieldLayer = crate::recast::recast::RcHeightfieldLayer;

        type rcHeightfieldLayerSet = crate::recast::recast::CxxRcHeightfieldLayerSet;
        unsafe fn rcAllocHeightfieldLayerSet() -> *mut rcHeightfieldLayerSet;
        unsafe fn rcFreeHeightfieldLayerSet(layerSet: *mut rcHeightfieldLayerSet);

        type rcContour = crate::recast::recast::RcContour;
        type rcContourSet = crate::recast::recast::CxxRcContourSet;
        unsafe fn rcAllocContourSet() -> *mut rcContourSet;
        unsafe fn rcFreeContourSet(contourSet: *mut rcContourSet);

        type rcPolyMesh = crate::recast::recast::CxxRcPolyMesh;
        unsafe fn rcAllocPolyMesh() -> *mut rcPolyMesh;
        unsafe fn rcFreePolyMesh(polyMesh: *mut rcPolyMesh);

        type rcPolyMeshDetail = crate::recast::recast::CxxRcPolyMeshDetail;
        unsafe fn rcAllocPolyMeshDetail() -> *mut rcPolyMeshDetail;
        unsafe fn rcFreePolyMeshDetail(detailMesh: *mut rcPolyMeshDetail);

        //
        // functions
        //

        unsafe fn rcCalcBounds(verts: *const f32, numVerts: i32, minBounds: *mut f32, maxBounds: *mut f32);
        unsafe fn rcCalcGridSize(
            minBounds: *const f32,
            maxBounds: *const f32,
            cellSize: f32,
            sizeX: *mut i32,
            sizeZ: *mut i32,
        );
        unsafe fn rcCreateHeightfield(
            context: *mut rcContext,
            heightfield: Pin<&mut rcHeightfield>,
            sizeX: i32,
            sizeZ: i32,
            minBounds: *const f32,
            maxBounds: *const f32,
            cellSize: f32,
            cellHeight: f32,
        ) -> bool;
        unsafe fn rcMarkWalkableTriangles(
            context: *mut rcContext,
            walkableSlopeAngle: f32,
            verts: *const f32,
            numVerts: i32,
            tris: *const i32,
            numTris: i32,
            triAreaIDs: *mut u8,
        );
        unsafe fn rcClearUnwalkableTriangles(
            context: *mut rcContext,
            walkableSlopeAngle: f32,
            verts: *const f32,
            numVerts: i32,
            tris: *const i32,
            numTris: i32,
            triAreaIDs: *mut u8,
        );
        unsafe fn rcAddSpan(
            context: *mut rcContext,
            heightfield: Pin<&mut rcHeightfield>,
            x: i32,
            z: i32,
            spanMin: u16,
            spanMax: u16,
            areaID: u8,
            flagMergeThreshold: i32,
        ) -> bool;
        unsafe fn rcRasterizeTriangle(
            context: *mut rcContext,
            v0: *const f32,
            v1: *const f32,
            v2: *const f32,
            areaID: u8,
            heightfield: Pin<&mut rcHeightfield>,
            flagMergeThreshold: i32,
        ) -> bool;
        #[rust_name = "rcRasterizeTriangles1"]
        unsafe fn rcRasterizeTriangles(
            context: *mut rcContext,
            verts: *const f32,
            numVerts: i32,
            tris: *const i32,
            triAreaIDs: *const u8,
            numTris: i32,
            heightfield: Pin<&mut rcHeightfield>,
            flagMergeThreshold: i32,
        ) -> bool;
        #[rust_name = "rcRasterizeTriangles2"]
        unsafe fn rcRasterizeTriangles(
            context: *mut rcContext,
            verts: *const f32,
            numVerts: i32,
            tris: *const u16,
            triAreaIDs: *const u8,
            numTris: i32,
            heightfield: Pin<&mut rcHeightfield>,
            flagMergeThreshold: i32,
        ) -> bool;
        #[rust_name = "rcRasterizeTriangles3"]
        unsafe fn rcRasterizeTriangles(
            context: *mut rcContext,
            verts: *const f32,
            triAreaIDs: *const u8,
            numTris: i32,
            heightfield: Pin<&mut rcHeightfield>,
            flagMergeThreshold: i32,
        ) -> bool;
        unsafe fn rcFilterLowHangingWalkableObstacles(
            context: *mut rcContext,
            walkableClimb: i32,
            heightfield: Pin<&mut rcHeightfield>,
        );
        unsafe fn rcFilterLedgeSpans(
            context: *mut rcContext,
            walkableHeight: i32,
            walkableClimb: i32,
            heightfield: Pin<&mut rcHeightfield>,
        );
        unsafe fn rcFilterWalkableLowHeightSpans(
            context: *mut rcContext,
            walkableHeight: i32,
            heightfield: Pin<&mut rcHeightfield>,
        );
        unsafe fn rcGetHeightFieldSpanCount(context: *mut rcContext, heightfield: &rcHeightfield) -> i32;
        unsafe fn rcBuildCompactHeightfield(
            context: *mut rcContext,
            walkableHeight: i32,
            walkableClimb: i32,
            heightfield: &rcHeightfield,
            compactHeightfield: Pin<&mut rcCompactHeightfield>,
        ) -> bool;
        unsafe fn rcErodeWalkableArea(
            context: *mut rcContext,
            erosionRadius: i32,
            compactHeightfield: Pin<&mut rcCompactHeightfield>,
        ) -> bool;
        unsafe fn rcMedianFilterWalkableArea(
            context: *mut rcContext,
            compactHeightfield: Pin<&mut rcCompactHeightfield>,
        ) -> bool;
        unsafe fn rcMarkBoxArea(
            context: *mut rcContext,
            boxMinBounds: *const f32,
            boxMaxBounds: *const f32,
            areaId: u8,
            compactHeightfield: Pin<&mut rcCompactHeightfield>,
        );
        unsafe fn rcMarkConvexPolyArea(
            context: *mut rcContext,
            verts: *const f32,
            numVerts: i32,
            minY: f32,
            maxY: f32,
            areaId: u8,
            compactHeightfield: Pin<&mut rcCompactHeightfield>,
        );
        unsafe fn rcOffsetPoly(
            verts: *const f32,
            numVerts: i32,
            offset: f32,
            outVerts: *mut f32,
            maxOutVerts: i32,
        ) -> i32;
        unsafe fn rcMarkCylinderArea(
            context: *mut rcContext,
            position: *const f32,
            radius: f32,
            height: f32,
            areaId: u8,
            compactHeightfield: Pin<&mut rcCompactHeightfield>,
        );
        unsafe fn rcBuildDistanceField(ctx: *mut rcContext, chf: Pin<&mut rcCompactHeightfield>) -> bool;
        unsafe fn rcBuildRegions(
            ctx: *mut rcContext,
            chf: Pin<&mut rcCompactHeightfield>,
            borderSize: i32,
            minRegionArea: i32,
            mergeRegionArea: i32,
        ) -> bool;
        unsafe fn rcBuildLayerRegions(
            ctx: *mut rcContext,
            chf: Pin<&mut rcCompactHeightfield>,
            borderSize: i32,
            minRegionArea: i32,
        ) -> bool;
        unsafe fn rcBuildRegionsMonotone(
            ctx: *mut rcContext,
            chf: Pin<&mut rcCompactHeightfield>,
            borderSize: i32,
            minRegionArea: i32,
            mergeRegionArea: i32,
        ) -> bool;
        unsafe fn rcBuildHeightfieldLayers(
            ctx: *mut rcContext,
            chf: &rcCompactHeightfield,
            borderSize: i32,
            walkableHeight: i32,
            lset: Pin<&mut rcHeightfieldLayerSet>,
        ) -> bool;
        unsafe fn rcBuildContours(
            ctx: *mut rcContext,
            chf: &rcCompactHeightfield,
            maxError: f32,
            maxEdgeLen: i32,
            cset: Pin<&mut rcContourSet>,
            buildFlags: i32,
        ) -> bool;
        unsafe fn rcBuildPolyMesh(
            ctx: *mut rcContext,
            cset: &rcContourSet,
            nvp: i32,
            mesh: Pin<&mut rcPolyMesh>,
        ) -> bool;
        unsafe fn rcMergePolyMeshes(
            ctx: *mut rcContext,
            meshes: *const *const rcPolyMesh,
            nmeshes: i32,
            mesh: Pin<&mut rcPolyMesh>,
        ) -> bool;
        unsafe fn rcBuildPolyMeshDetail(
            ctx: *mut rcContext,
            mesh: &rcPolyMesh,
            chf: &rcCompactHeightfield,
            sampleDist: f32,
            sampleMaxError: f32,
            dmesh: Pin<&mut rcPolyMeshDetail>,
        ) -> bool;
        unsafe fn rcCopyPolyMesh(ctx: *mut rcContext, src: &rcPolyMesh, dst: Pin<&mut rcPolyMesh>) -> bool;
        unsafe fn rcMergePolyMeshDetails(
            ctx: *mut rcContext,
            meshes: *const *const rcPolyMeshDetail,
            nmeshes: i32,
            mesh: Pin<&mut rcPolyMeshDetail>,
        ) -> bool;
    }
}

pub type RcLogCategory = ffi::rcLogCategory;
pub type RcTimerLabel = ffi::rcTimerLabel;
pub type RcBuildContoursFlags = ffi::rcBuildContoursFlags;

//
// RcContext
//

pub struct RcContext(UniquePtr<ffi::rcContext>);

impl Debug for RcContext {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        return f
            .debug_tuple("RcContext")
            .field(&unsafe { mem::transmute_copy::<_, *const ffi::rcContext>(&self.0) })
            .finish();
    }
}

impl RcContext {
    pub fn new(state: bool) -> RcContext {
        return RcContext(ffi::rcNewContext(state));
    }

    #[inline]
    pub fn enable_log(&mut self, state: bool) {
        self.0.pin_mut().enableLog(state);
    }

    #[inline]
    pub fn reset_log(&mut self) {
        self.0.pin_mut().resetLog();
    }

    #[inline]
    pub fn enable_timer(&mut self, state: bool) {
        self.0.pin_mut().enableTimer(state);
    }

    #[inline]
    pub fn reset_timers(&mut self) {
        self.0.pin_mut().resetTimers();
    }

    #[inline]
    pub fn start_timer(&mut self, label: RcTimerLabel) {
        self.0.pin_mut().startTimer(label);
    }

    #[inline]
    pub fn stop_timer(&mut self, label: RcTimerLabel) {
        self.0.pin_mut().stopTimer(label);
    }

    #[inline]
    pub fn get_accumulated_time(&self, label: RcTimerLabel) -> i32 {
        return self.0.getAccumulatedTime(label);
    }
}

//
// RcConfig
//

#[repr(C)]
#[derive(Debug, Default, Clone)]
pub struct RcConfig {
    // The width of the field along the x-axis. [Limit: >= 0] [Units: vx]
    pub width: i32,

    // The height of the field along the z-axis. [Limit: >= 0] [Units: vx]
    pub height: i32,

    // The width/height size of tile's on the xz-plane. [Limit: >= 0] [Units: vx]
    pub tile_size: i32,

    // The size of the non-navigable border around the heightfield. [Limit: >=0] [Units: vx]
    pub border_size: i32,

    // The xz-plane cell size to use for fields. [Limit: > 0] [Units: wu]
    pub cs: f32,

    // The y-axis cell size to use for fields. [Limit: > 0] [Units: wu]
    pub ch: f32,

    // The minimum bounds of the field's AABB. [(x, y, z)] [Units: wu]
    pub bmin: [f32; 3],

    // The maximum bounds of the field's AABB. [(x, y, z)] [Units: wu]
    pub bmax: [f32; 3],

    // The maximum slope that is considered walkable. [Limits: 0 <= value < 90] [Units: Degrees]
    pub walkable_slope_angle: f32,

    // Minimum floor to 'ceiling' height that will still allow the floor area to
    // be considered walkable. [Limit: >= 3] [Units: vx]
    pub walkable_height: i32,

    // Maximum ledge height that is considered to still be traversable. [Limit: >=0] [Units: vx]
    pub walkable_climb: i32,

    // The distance to erode/shrink the walkable area of the heightfield away from
    // obstructions.  [Limit: >=0] [Units: vx]
    pub walkable_radius: i32,

    // The maximum allowed length for contour edges along the border of the mesh. [Limit: >=0] [Units: vx]
    pub max_edge_len: i32,

    // The maximum distance a simplified contour's border edges should deviate
    // the original raw contour. [Limit: >=0] [Units: vx]
    pub max_simplification_error: f32,

    // The minimum number of cells allowed to form isolated island areas. [Limit: >=0] [Units: vx]
    pub min_region_area: i32,

    // Any regions with a span count smaller than this value will, if possible,
    // be merged with larger regions. [Limit: >=0] [Units: vx]
    pub merge_region_area: i32,

    // The maximum number of vertices allowed for polygons generated during the
    // contour to polygon conversion process. [Limit: >= 3]
    pub max_verts_per_poly: i32,

    // Sets the sampling distance to use when generating the detail mesh.
    // (For height detail only.) [Limits: 0 or >= 0.9] [Units: wu]
    pub detail_sample_dist: f32,

    // The maximum distance the detail mesh surface should deviate from heightfield
    // data. (For height detail only.) [Limit: >=0] [Units: wu]
    pub detail_sample_max_error: f32,
}

const_assert_eq!(mem::size_of::<RcConfig>(), 92);

unsafe impl ExternType for RcConfig {
    type Id = type_id!("rcConfig");
    type Kind = cxx::kind::Trivial;
}

//
// RcSpan
//

#[repr(C)]
#[derive(Clone)]
pub struct RcSpan {
    pub bits: u32,
    next: *const RcSpan,
}

#[cfg(target_pointer_width = "64")]
const_assert_eq!(mem::size_of::<RcSpan>(), 16);

#[cfg(target_pointer_width = "32")]
const_assert_eq!(mem::size_of::<RcSpan>(), 8);

unsafe impl ExternType for RcSpan {
    type Id = type_id!("rcSpan");
    type Kind = cxx::kind::Trivial;
}

const RC_SPAN_SMIN_MASK: u32 = RC_SPAN_MAX_HEIGHT;
const RC_SPAN_SMAX_OFF: u32 = RC_SPAN_HEIGHT_BITS;
const RC_SPAN_SMAX_MASK: u32 = RC_SPAN_MAX_HEIGHT << RC_SPAN_SMAX_OFF;
const RC_SPAN_AREA_OFF: u32 = RC_SPAN_HEIGHT_BITS * 2;
const RC_SPAN_AREA_MASK: u32 = !(RC_SPAN_SMIN_MASK | RC_SPAN_SMAX_MASK);

impl RcSpan {
    #[inline]
    pub fn smin(&self) -> u32 {
        return self.bits & RC_SPAN_SMIN_MASK;
    }

    #[inline]
    pub fn set_smin(&mut self, smin: u32) {
        self.bits = (self.bits & !RC_SPAN_SMIN_MASK) | (smin & RC_SPAN_SMIN_MASK);
    }

    #[inline]
    pub fn smax(&self) -> u32 {
        return (self.bits & RC_SPAN_SMAX_MASK) >> RC_SPAN_SMAX_OFF;
    }

    #[inline]
    pub fn set_smax(&mut self, smax: u32) {
        self.bits = (self.bits & !RC_SPAN_SMAX_MASK) | ((smax << RC_SPAN_SMAX_OFF) | RC_SPAN_SMAX_MASK);
    }

    #[inline]
    pub fn area(&self) -> u32 {
        return self.bits >> RC_SPAN_AREA_OFF;
    }

    #[inline]
    pub fn set_area(&mut self, area: u32) {
        self.bits = (self.bits & !RC_SPAN_AREA_MASK) | (area << RC_SPAN_AREA_OFF);
    }

    #[inline]
    pub fn next(&self) -> Option<&RcSpan> {
        if self.next.is_null() {
            return None;
        } else {
            return Some(unsafe { &*self.next });
        }
    }
}

impl Debug for RcSpan {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        return f
            .debug_struct("RcSpan")
            .field("smin", &self.smin())
            .field("smax", &self.smax())
            .field("area", &self.area())
            .field("next", &self.next())
            .finish();
    }
}

//
// RcSpanPool
//

#[repr(C)]
#[derive(Debug, Clone)]
pub struct RcSpanPool {
    next: *const RcSpanPool,
    pub items: [RcSpan; RC_SPANS_PER_POOL],
}

#[cfg(target_pointer_width = "64")]
const_assert_eq!(mem::size_of::<RcSpanPool>(), 32776);

#[cfg(target_pointer_width = "32")]
const_assert_eq!(mem::size_of::<RcSpanPool>(), 16388);

unsafe impl ExternType for RcSpanPool {
    type Id = type_id!("rcSpanPool");
    type Kind = cxx::kind::Trivial;
}

//
// RcHeightfield
//

#[repr(C)]
#[derive(Debug)]
pub struct CxxRcHeightfield {
    width: i32,
    height: i32,
    pub bmin: [f32; 3],
    pub bmax: [f32; 3],
    pub cs: f32,
    pub ch: f32,
    spans: *mut *mut RcSpan,
    pools: *mut RcSpanPool,
    freelist: *mut RcSpan,
}

#[cfg(target_pointer_width = "64")]
const_assert_eq!(mem::size_of::<CxxRcHeightfield>(), 64);

#[cfg(target_pointer_width = "32")]
const_assert_eq!(mem::size_of::<CxxRcHeightfield>(), 52);

unsafe impl ExternType for CxxRcHeightfield {
    type Id = type_id!("rcHeightfield");
    type Kind = cxx::kind::Trivial;
}

pub struct RcHeightfield(*mut CxxRcHeightfield);

impl Deref for RcHeightfield {
    type Target = CxxRcHeightfield;

    fn deref(&self) -> &Self::Target {
        return self.inner();
    }
}

impl DerefMut for RcHeightfield {
    fn deref_mut(&mut self) -> &mut Self::Target {
        return self.inner_mut().get_mut();
    }
}

impl Drop for RcHeightfield {
    fn drop(&mut self) {
        unsafe { ffi::rcFreeHeightField(self.0) };
    }
}

impl RcHeightfield {
    #[inline]
    pub fn new() -> RcHeightfield {
        return RcHeightfield(unsafe { ffi::rcAllocHeightfield() });
    }

    #[inline]
    fn inner(&self) -> &ffi::rcHeightfield {
        return unsafe { &*self.0 };
    }

    #[inline]
    fn inner_mut(&mut self) -> Pin<&mut ffi::rcHeightfield> {
        return unsafe { Pin::new_unchecked(&mut *self.0) };
    }

    #[inline]
    pub fn as_ptr(&self) -> *const ffi::rcHeightfield {
        return self.0;
    }

    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut ffi::rcHeightfield {
        return self.0;
    }

    #[inline]
    pub fn width(&self) -> i32 {
        return self.width;
    }

    #[inline]
    pub fn height(&self) -> i32 {
        return self.height;
    }

    #[inline]
    pub fn get_span(&self, idx: usize) -> Option<&RcSpan> {
        if idx >= (self.width() * self.height()) as usize {
            return None;
        }
        let span = unsafe { *self.spans.add(idx) };
        if span.is_null() {
            return None;
        }
        return Some(unsafe { &*span });
    }

    #[inline]
    pub fn get_span_mut(&mut self, idx: usize) -> Option<&mut RcSpan> {
        if idx >= (self.width() * self.height()) as usize {
            return None;
        }
        let span = unsafe { *self.spans.add(idx) };
        if span.is_null() {
            return None;
        }
        return Some(unsafe { &mut *span });
    }
}

impl Debug for RcHeightfield {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        return self.inner().fmt(f);
    }
}

//
// RcCompactCell
//

#[repr(C)]
#[derive(Clone)]
pub struct RcCompactCell {
    bits: u32,
}

const_assert_eq!(mem::size_of::<RcCompactCell>(), 4);

unsafe impl ExternType for RcCompactCell {
    type Id = type_id!("rcCompactCell");
    type Kind = cxx::kind::Trivial;
}

const RC_COMPACT_CELL_INDEX_MASK: u32 = 0xFFFFFF;
const RC_COMPACT_CELL_COUNT_OFF: u32 = 24;
const RC_COMPACT_CELL_COUNT_MASK: u32 = 0xFF;

impl RcCompactCell {
    #[inline]
    pub fn index(&self) -> u32 {
        return self.bits & RC_COMPACT_CELL_INDEX_MASK;
    }

    #[inline]
    pub fn set_index(&mut self, index: u32) {
        self.bits = (self.bits & !RC_COMPACT_CELL_INDEX_MASK) | (index & RC_COMPACT_CELL_INDEX_MASK);
    }

    #[inline]
    pub fn count(&self) -> u32 {
        return self.bits >> RC_COMPACT_CELL_COUNT_OFF;
    }

    #[inline]
    pub fn set_count(&mut self, count: u32) {
        self.bits = (self.bits & !RC_COMPACT_CELL_COUNT_MASK) | (count << RC_COMPACT_CELL_COUNT_OFF);
    }
}

impl Debug for RcCompactCell {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        return f
            .debug_struct("RcCompactCell")
            .field("index", &self.index())
            .field("count", &self.count())
            .finish();
    }
}

//
// RcCompactSpan
//

#[repr(C)]
#[derive(Clone)]
pub struct RcCompactSpan {
    pub y: u16,
    pub reg: u16,
    bits: u32,
}

const_assert_eq!(mem::size_of::<RcCompactSpan>(), 8);

unsafe impl ExternType for RcCompactSpan {
    type Id = type_id!("rcCompactSpan");
    type Kind = cxx::kind::Trivial;
}

const RC_COMPACT_SPAN_CON_MASK: u32 = 0xFFFFFF;
const RC_COMPACT_SPAN_H_OFF: u32 = 24;
const RC_COMPACT_SPAN_H_MASK: u32 = 0xFF;

impl RcCompactSpan {
    #[inline]
    pub fn con(&self) -> u32 {
        return self.bits & RC_COMPACT_SPAN_CON_MASK;
    }

    #[inline]
    pub fn set_con(&mut self, con: u32) {
        self.bits = (self.bits & !RC_COMPACT_SPAN_CON_MASK) | (con & RC_COMPACT_SPAN_CON_MASK);
    }

    #[inline]
    pub fn h(&self) -> u32 {
        return self.bits >> RC_COMPACT_SPAN_H_OFF;
    }

    #[inline]
    pub fn set_h(&mut self, h: u32) {
        self.bits = (self.bits & !RC_COMPACT_SPAN_H_MASK) | (h << RC_COMPACT_SPAN_H_OFF);
    }
}

impl Debug for RcCompactSpan {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        return f
            .debug_struct("RcCompactSpan")
            .field("y", &self.y)
            .field("reg", &self.reg)
            .field("con", &self.con())
            .field("h", &self.h())
            .finish();
    }
}

//
// RcCompactHeightfield
//

#[repr(C)]
#[derive(Debug)]
pub struct CxxRcCompactHeightfield {
    width: i32,
    height: i32,
    span_count: i32,
    pub walkable_height: i32,
    pub walkable_climb: i32,
    pub border_size: i32,
    pub max_distance: u16,
    pub max_regions: u16,
    pub bmin: [f32; 3],
    pub bmax: [f32; 3],
    pub cs: f32,
    pub ch: f32,
    cells: *mut RcCompactCell,
    spans: *mut RcCompactSpan,
    dist: *mut u16,
    areas: *mut u8,
}

#[cfg(target_pointer_width = "64")]
const_assert_eq!(mem::size_of::<CxxRcCompactHeightfield>(), 96);

#[cfg(target_pointer_width = "32")]
const_assert_eq!(mem::size_of::<CxxRcCompactHeightfield>(), 76);

unsafe impl ExternType for CxxRcCompactHeightfield {
    type Id = type_id!("rcCompactHeightfield");
    type Kind = cxx::kind::Trivial;
}

pub struct RcCompactHeightfield(*mut ffi::rcCompactHeightfield);

impl Deref for RcCompactHeightfield {
    type Target = CxxRcCompactHeightfield;

    fn deref(&self) -> &Self::Target {
        return self.inner();
    }
}

impl DerefMut for RcCompactHeightfield {
    fn deref_mut(&mut self) -> &mut Self::Target {
        return self.inner_mut().get_mut();
    }
}

impl Drop for RcCompactHeightfield {
    fn drop(&mut self) {
        unsafe { ffi::rcFreeCompactHeightfield(self.0) };
    }
}

impl RcCompactHeightfield {
    #[inline]
    pub fn new() -> RcCompactHeightfield {
        return RcCompactHeightfield(unsafe { ffi::rcAllocCompactHeightfield() });
    }

    #[inline]
    fn inner(&self) -> &ffi::rcCompactHeightfield {
        return unsafe { &*self.0 };
    }

    #[inline]
    fn inner_mut(&mut self) -> Pin<&mut ffi::rcCompactHeightfield> {
        return unsafe { Pin::new_unchecked(&mut *self.0) };
    }

    #[inline]
    pub fn as_ptr(&self) -> *const ffi::rcCompactHeightfield {
        return self.0;
    }

    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut ffi::rcCompactHeightfield {
        return self.0;
    }

    #[inline]
    pub fn width(&self) -> i32 {
        return self.width;
    }

    #[inline]
    pub fn height(&self) -> i32 {
        return self.height;
    }

    #[inline]
    pub fn span_count(&self) -> usize {
        return self.span_count as usize;
    }

    #[inline]
    pub fn cells(&self) -> &[RcCompactCell] {
        return unsafe { slice::from_raw_parts(self.cells, (self.width() * self.height()) as usize) };
    }

    #[inline]
    pub fn cells_mut(&mut self) -> &mut [RcCompactCell] {
        return unsafe { slice::from_raw_parts_mut(self.cells, (self.width() * self.height()) as usize) };
    }

    #[inline]
    pub fn spans(&self) -> &[RcCompactSpan] {
        return unsafe { slice::from_raw_parts(self.spans, self.span_count()) };
    }

    #[inline]
    pub fn spans_mut(&mut self) -> &mut [RcCompactSpan] {
        return unsafe { slice::from_raw_parts_mut(self.spans, self.span_count()) };
    }

    #[inline]
    pub fn dist(&self) -> &[u16] {
        let dist_ptr = self.dist;
        let mut dist_len = self.span_count();
        if dist_ptr.is_null() {
            dist_len = 0;
        }
        return unsafe { slice::from_raw_parts(dist_ptr, dist_len) };
    }

    #[inline]
    pub fn dist_mut(&mut self) -> &mut [u16] {
        let dist_ptr = self.dist;
        let mut dist_len = self.span_count();
        if dist_ptr.is_null() {
            dist_len = 0;
        }
        return unsafe { slice::from_raw_parts_mut(dist_ptr, dist_len) };
    }

    #[inline]
    pub fn areas(&self) -> &[u8] {
        return unsafe { slice::from_raw_parts(self.areas, self.span_count()) };
    }

    #[inline]
    pub fn areas_mut(&mut self) -> &mut [u8] {
        return unsafe { slice::from_raw_parts_mut(self.areas, self.span_count()) };
    }
}

impl Debug for RcCompactHeightfield {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        return self.inner().fmt(f);
    }
}

//
// RcHeightfieldLayer
//

#[repr(C)]
#[derive(Debug)]
pub struct RcHeightfieldLayer {
    pub bmin: [f32; 3], // The minimum bounds in world space. [(x, y, z)]
    pub bmax: [f32; 3], // The maximum bounds in world space. [(x, y, z)]
    pub cs: f32,        // The size of each cell. (On the xz-plane.)
    pub ch: f32,        // The height of each cell. (The minimum increment along the y-axis.)
    width: i32,         // The width of the heightfield. (Along the x-axis in cell units.)
    height: i32,        // The height of the heightfield. (Along the z-axis in cell units.)
    pub minx: i32,      // The minimum x-bounds of usable data.
    pub maxx: i32,      // The maximum x-bounds of usable data.
    pub miny: i32,      // The minimum y-bounds of usable data. (Along the z-axis.)
    pub maxy: i32,      // The maximum y-bounds of usable data. (Along the z-axis.)
    pub hmin: i32,      // The minimum height bounds of usable data. (Along the y-axis.)
    pub hmax: i32,      // The maximum height bounds of usable data. (Along the y-axis.)
    heights: *mut u8,   // The heightfield. [Size: width * height]
    areas: *mut u8,     // Area ids. [Size: Same as #heights]
    cons: *mut u8,      // Packed neighbor connection information. [Size: Same as #heights]
}

#[cfg(target_pointer_width = "64")]
const_assert_eq!(mem::size_of::<RcHeightfieldLayer>(), 88);

#[cfg(target_pointer_width = "32")]
const_assert_eq!(mem::size_of::<RcHeightfieldLayer>(), 76);

unsafe impl ExternType for RcHeightfieldLayer {
    type Id = type_id!("rcHeightfieldLayer");
    type Kind = cxx::kind::Trivial;
}

impl RcHeightfieldLayer {
    #[inline]
    pub fn width(&self) -> i32 {
        return self.width;
    }

    #[inline]
    pub fn height(&self) -> i32 {
        return self.height;
    }

    #[inline]
    pub fn heights(&self) -> &[u8] {
        return unsafe { slice::from_raw_parts(self.heights, (self.width * self.height) as usize) };
    }

    #[inline]
    pub fn heights_mut(&mut self) -> &mut [u8] {
        return unsafe { slice::from_raw_parts_mut(self.heights, (self.width * self.height) as usize) };
    }

    #[inline]
    pub fn areas(&self) -> &[u8] {
        return unsafe { slice::from_raw_parts(self.areas, self.height as usize) };
    }

    #[inline]
    pub fn areas_mut(&mut self) -> &mut [u8] {
        return unsafe { slice::from_raw_parts_mut(self.areas, self.height as usize) };
    }

    #[inline]
    pub fn cons(&self) -> &[u8] {
        return unsafe { slice::from_raw_parts(self.cons, self.height as usize) };
    }

    #[inline]
    pub fn cons_mut(&mut self) -> &mut [u8] {
        return unsafe { slice::from_raw_parts_mut(self.cons, self.height as usize) };
    }
}

//
// RcHeightfieldLayerSet
//

#[repr(C)]
#[derive(Debug)]
pub struct CxxRcHeightfieldLayerSet {
    layers: *mut ffi::rcHeightfieldLayer,
    nlayers: i32,
}

#[cfg(target_pointer_width = "64")]
const_assert_eq!(mem::size_of::<CxxRcHeightfieldLayerSet>(), 16);

#[cfg(target_pointer_width = "32")]
const_assert_eq!(mem::size_of::<CxxRcHeightfieldLayerSet>(), 8);

unsafe impl ExternType for CxxRcHeightfieldLayerSet {
    type Id = type_id!("rcHeightfieldLayerSet");
    type Kind = cxx::kind::Trivial;
}

pub struct RcHeightfieldLayerSet(*mut ffi::rcHeightfieldLayerSet);

impl Deref for RcHeightfieldLayerSet {
    type Target = CxxRcHeightfieldLayerSet;

    fn deref(&self) -> &Self::Target {
        return self.inner();
    }
}

impl DerefMut for RcHeightfieldLayerSet {
    fn deref_mut(&mut self) -> &mut Self::Target {
        return self.inner_mut().get_mut();
    }
}

impl Drop for RcHeightfieldLayerSet {
    fn drop(&mut self) {
        unsafe { ffi::rcFreeHeightfieldLayerSet(self.0) };
    }
}

impl RcHeightfieldLayerSet {
    #[inline]
    pub fn new() -> RcHeightfieldLayerSet {
        return RcHeightfieldLayerSet(unsafe { ffi::rcAllocHeightfieldLayerSet() });
    }

    #[inline]
    fn inner(&self) -> &ffi::rcHeightfieldLayerSet {
        return unsafe { &*self.0 };
    }

    #[inline]
    fn inner_mut(&mut self) -> Pin<&mut ffi::rcHeightfieldLayerSet> {
        return unsafe { Pin::new_unchecked(&mut *self.0) };
    }

    #[inline]
    pub fn as_ptr(&self) -> *const ffi::rcHeightfieldLayerSet {
        return self.0;
    }

    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut ffi::rcHeightfieldLayerSet {
        return self.0;
    }

    #[inline]
    pub fn layers(&self) -> &[RcHeightfieldLayer] {
        return unsafe { slice::from_raw_parts(self.layers, self.nlayers()) };
    }

    #[inline]
    pub fn layers_mut(&mut self) -> &mut [RcHeightfieldLayer] {
        return unsafe { slice::from_raw_parts_mut(self.layers, self.nlayers()) };
    }

    #[inline]
    pub fn nlayers(&self) -> usize {
        return self.inner().nlayers as usize;
    }
}

impl Debug for RcHeightfieldLayerSet {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        return self.inner().fmt(f);
    }
}

//
// RcContour
//

#[repr(C)]
#[derive(Debug)]
pub struct RcContour {
    verts: *mut [i32; 4],
    nverts: i32,
    rverts: *mut [i32; 4],
    nrverts: i32,
    pub reg: u16,
    pub area: u8,
}

#[cfg(target_pointer_width = "64")]
const_assert_eq!(mem::size_of::<RcContour>(), 32);

#[cfg(target_pointer_width = "32")]
const_assert_eq!(mem::size_of::<RcContour>(), 20);

unsafe impl ExternType for RcContour {
    type Id = type_id!("rcContour");
    type Kind = cxx::kind::Trivial;
}

impl RcContour {
    #[inline]
    pub fn nverts(&self) -> usize {
        return self.nverts as usize;
    }

    #[inline]
    pub fn verts(&self) -> &[[i32; 4]] {
        return unsafe { slice::from_raw_parts(self.verts, self.nverts()) };
    }

    #[inline]
    pub fn verts_mut(&mut self) -> &mut [[i32; 4]] {
        return unsafe { slice::from_raw_parts_mut(self.verts, self.nverts()) };
    }

    #[inline]
    pub fn nrverts(&self) -> usize {
        return self.nrverts as usize;
    }

    #[inline]
    pub fn rverts(&self) -> &[[i32; 4]] {
        return unsafe { slice::from_raw_parts(self.rverts, self.nrverts()) };
    }

    #[inline]
    pub fn rverts_mut(&mut self) -> &mut [[i32; 4]] {
        return unsafe { slice::from_raw_parts_mut(self.rverts, self.nrverts()) };
    }
}

//
// RcContourSet
//

#[repr(C)]
#[derive(Debug)]
pub struct CxxRcContourSet {
    conts: *mut RcContour,
    nconts: i32,
    pub bmin: [f32; 3],
    pub bmax: [f32; 3],
    pub cs: f32,
    pub ch: f32,
    pub width: i32,
    pub height: i32,
    pub border_size: i32,
    pub max_error: f32,
}

#[cfg(target_pointer_width = "64")]
const_assert_eq!(mem::size_of::<CxxRcContourSet>(), 64);

#[cfg(target_pointer_width = "32")]
const_assert_eq!(mem::size_of::<CxxRcContourSet>(), 56);

unsafe impl ExternType for CxxRcContourSet {
    type Id = type_id!("rcContourSet");
    type Kind = cxx::kind::Trivial;
}

pub struct RcContourSet(*mut ffi::rcContourSet);

impl Deref for RcContourSet {
    type Target = CxxRcContourSet;

    fn deref(&self) -> &Self::Target {
        return self.inner();
    }
}

impl DerefMut for RcContourSet {
    fn deref_mut(&mut self) -> &mut Self::Target {
        return self.inner_mut().get_mut();
    }
}

impl Drop for RcContourSet {
    fn drop(&mut self) {
        unsafe { ffi::rcFreeContourSet(self.0) };
    }
}

impl RcContourSet {
    #[inline]
    pub fn new() -> RcContourSet {
        return RcContourSet(unsafe { ffi::rcAllocContourSet() });
    }

    #[inline]
    fn inner(&self) -> &ffi::rcContourSet {
        return unsafe { &*self.0 };
    }

    #[inline]
    fn inner_mut(&mut self) -> Pin<&mut ffi::rcContourSet> {
        return unsafe { Pin::new_unchecked(&mut *self.0) };
    }

    #[inline]
    pub fn as_ptr(&self) -> *const ffi::rcContourSet {
        return self.0;
    }

    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut ffi::rcContourSet {
        return self.0;
    }

    #[inline]
    pub fn conts(&self) -> &[RcContour] {
        return unsafe { slice::from_raw_parts(self.inner().conts, self.nconts()) };
    }

    #[inline]
    pub fn conts_mut(&mut self) -> &mut [RcContour] {
        return unsafe { slice::from_raw_parts_mut(self.inner_mut().conts, self.nconts()) };
    }

    #[inline]
    pub fn nconts(&self) -> usize {
        return self.inner().nconts as usize;
    }
}

impl Debug for RcContourSet {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        return self.inner().fmt(f);
    }
}

//
// RcPolyMesh
//

#[repr(C)]
#[derive(Debug)]
pub struct CxxRcPolyMesh {
    verts: *mut [u16; 3],
    polys: *mut u16,
    regs: *mut u16,
    flags: *mut u16,
    areas: *mut u8,
    nverts: i32,
    npolys: i32,
    maxpolys: i32,
    nvp: i32,
    pub bmin: [f32; 3],
    pub bmax: [f32; 3],
    pub cs: f32,
    pub ch: f32,
    pub border_size: i32,
    pub max_edge_error: f32,
}

#[cfg(target_pointer_width = "64")]
const_assert_eq!(mem::size_of::<CxxRcPolyMesh>(), 96);

#[cfg(target_pointer_width = "32")]
const_assert_eq!(mem::size_of::<CxxRcPolyMesh>(), 76);

unsafe impl ExternType for CxxRcPolyMesh {
    type Id = type_id!("rcPolyMesh");
    type Kind = cxx::kind::Trivial;
}

pub struct RcPolyMesh(*mut ffi::rcPolyMesh);

impl Deref for RcPolyMesh {
    type Target = CxxRcPolyMesh;

    fn deref(&self) -> &Self::Target {
        return self.inner();
    }
}

impl DerefMut for RcPolyMesh {
    fn deref_mut(&mut self) -> &mut Self::Target {
        return self.inner_mut().get_mut();
    }
}

impl Drop for RcPolyMesh {
    fn drop(&mut self) {
        unsafe { ffi::rcFreePolyMesh(self.0) };
    }
}

impl RcPolyMesh {
    #[inline]
    pub fn new() -> RcPolyMesh {
        return RcPolyMesh(unsafe { ffi::rcAllocPolyMesh() });
    }

    #[inline]
    fn inner(&self) -> &ffi::rcPolyMesh {
        return unsafe { &*self.0 };
    }

    #[inline]
    fn inner_mut(&mut self) -> Pin<&mut ffi::rcPolyMesh> {
        return unsafe { Pin::new_unchecked(&mut *self.0) };
    }

    #[inline]
    pub fn as_ptr(&self) -> *const ffi::rcPolyMesh {
        return self.0;
    }

    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut ffi::rcPolyMesh {
        return self.0;
    }

    #[inline]
    pub fn verts(&self) -> &[[u16; 3]] {
        return unsafe { slice::from_raw_parts(self.verts, self.nverts()) };
    }

    #[inline]
    pub fn verts_mut(&mut self) -> &mut [[u16; 3]] {
        return unsafe { slice::from_raw_parts_mut(self.verts, self.nverts()) };
    }

    #[inline]
    pub fn polys(&self) -> &[u16] {
        return unsafe { slice::from_raw_parts(self.polys, self.npolys() * 2 * self.nvp()) };
    }

    #[inline]
    pub fn polys_mut(&mut self) -> &mut [u16] {
        return unsafe { slice::from_raw_parts_mut(self.polys, self.npolys() * 2 * self.nvp()) };
    }

    #[inline]
    pub fn regs(&self) -> &[u16] {
        return unsafe { slice::from_raw_parts(self.regs, self.npolys()) };
    }

    #[inline]
    pub fn regs_mut(&mut self) -> &mut [u16] {
        return unsafe { slice::from_raw_parts_mut(self.regs, self.npolys()) };
    }

    #[inline]
    pub fn flags(&self) -> &[u16] {
        return unsafe { slice::from_raw_parts(self.flags, self.npolys()) };
    }

    #[inline]
    pub fn flags_mut(&mut self) -> &mut [u16] {
        return unsafe { slice::from_raw_parts_mut(self.flags, self.npolys()) };
    }

    #[inline]
    pub fn areas(&self) -> &[u8] {
        return unsafe { slice::from_raw_parts(self.areas, self.npolys()) };
    }

    #[inline]
    pub fn areas_mut(&mut self) -> &mut [u8] {
        return unsafe { slice::from_raw_parts_mut(self.areas, self.npolys()) };
    }

    #[inline]
    pub fn nverts(&self) -> usize {
        return self.nverts as usize;
    }

    #[inline]
    pub fn npolys(&self) -> usize {
        return self.npolys as usize;
    }

    #[inline]
    pub fn maxpolys(&self) -> usize {
        return self.maxpolys as usize;
    }

    #[inline]
    pub fn nvp(&self) -> usize {
        return self.nvp as usize;
    }
}

impl Debug for RcPolyMesh {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        return self.inner().fmt(f);
    }
}

//
// RcPolyMeshDetail
//

#[repr(C)]
#[derive(Debug)]
pub struct CxxRcPolyMeshDetail {
    meshes: *mut [u32; 4],
    verts: *mut [f32; 3],
    tris: *mut [u8; 4],
    nmeshes: i32,
    nverts: i32,
    ntris: i32,
}

#[cfg(target_pointer_width = "64")]
const_assert_eq!(mem::size_of::<CxxRcPolyMeshDetail>(), 40);

#[cfg(target_pointer_width = "32")]
const_assert_eq!(mem::size_of::<CxxRcPolyMeshDetail>(), 24);

unsafe impl ExternType for CxxRcPolyMeshDetail {
    type Id = type_id!("rcPolyMeshDetail");
    type Kind = cxx::kind::Trivial;
}

pub struct RcPolyMeshDetail(*mut ffi::rcPolyMeshDetail);

impl Deref for RcPolyMeshDetail {
    type Target = CxxRcPolyMeshDetail;

    fn deref(&self) -> &Self::Target {
        return self.inner();
    }
}

impl DerefMut for RcPolyMeshDetail {
    fn deref_mut(&mut self) -> &mut Self::Target {
        return self.inner_mut().get_mut();
    }
}

impl Drop for RcPolyMeshDetail {
    fn drop(&mut self) {
        unsafe { ffi::rcFreePolyMeshDetail(self.0) };
    }
}

impl RcPolyMeshDetail {
    #[inline]
    pub fn new() -> RcPolyMeshDetail {
        return RcPolyMeshDetail(unsafe { ffi::rcAllocPolyMeshDetail() });
    }

    #[inline]
    fn inner(&self) -> &ffi::rcPolyMeshDetail {
        return unsafe { &*self.0 };
    }

    #[inline]
    fn inner_mut(&mut self) -> Pin<&mut ffi::rcPolyMeshDetail> {
        return unsafe { Pin::new_unchecked(&mut *self.0) };
    }

    #[inline]
    pub fn as_ptr(&self) -> *const ffi::rcPolyMeshDetail {
        return self.0;
    }

    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut ffi::rcPolyMeshDetail {
        return self.0;
    }

    #[inline]
    pub fn meshes(&self) -> &[[u32; 4]] {
        return unsafe { slice::from_raw_parts(self.meshes, self.nmeshes()) };
    }

    #[inline]
    pub fn meshes_mut(&mut self) -> &mut [[u32; 4]] {
        return unsafe { slice::from_raw_parts_mut(self.meshes, self.nmeshes()) };
    }

    #[inline]
    pub fn verts(&self) -> &[[f32; 3]] {
        return unsafe { slice::from_raw_parts(self.verts, self.nverts()) };
    }

    #[inline]
    pub fn verts_mut(&mut self) -> &mut [[f32; 3]] {
        return unsafe { slice::from_raw_parts_mut(self.verts, self.nverts()) };
    }

    #[inline]
    pub fn tris(&self) -> &[[u8; 4]] {
        return unsafe { slice::from_raw_parts(self.tris, self.ntris()) };
    }

    #[inline]
    pub fn tris_mut(&mut self) -> &mut [[u8; 4]] {
        return unsafe { slice::from_raw_parts_mut(self.tris, self.ntris()) };
    }

    #[inline]
    pub fn nmeshes(&self) -> usize {
        return self.nmeshes as usize;
    }

    #[inline]
    pub fn nverts(&self) -> usize {
        return self.nverts as usize;
    }

    #[inline]
    pub fn ntris(&self) -> usize {
        return self.ntris as usize;
    }
}

impl Debug for RcPolyMeshDetail {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        return self.inner().fmt(f);
    }
}

//
// functions
//

pub fn rc_calc_bounds(verts: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut min_bounds = [0.0; 3];
    let mut max_bounds = [0.0; 3];
    unsafe {
        ffi::rcCalcBounds(
            verts.as_ptr() as *const _,
            verts.len() as i32,
            min_bounds.as_mut_ptr(),
            max_bounds.as_mut_ptr(),
        );
    }
    return (min_bounds, max_bounds);
}

pub fn rc_calc_grid_size(min_bounds: &[f32; 3], max_bounds: &[f32; 3], cell_size: f32) -> (i32, i32) {
    let mut size_x = 0;
    let mut size_z = 0;
    unsafe {
        ffi::rcCalcGridSize(
            min_bounds.as_ptr(),
            max_bounds.as_ptr(),
            cell_size,
            &mut size_x,
            &mut size_z,
        );
    }
    return (size_x, size_z);
}

pub fn rc_create_heightfield(
    context: &mut RcContext,
    heightfield: &mut RcHeightfield,
    size_x: i32,
    size_z: i32,
    min_bounds: &[f32; 3],
    max_bounds: &[f32; 3],
    cell_size: f32,
    cell_height: f32,
) -> Result<(), XError> {
    let res = unsafe {
        ffi::rcCreateHeightfield(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            heightfield.inner_mut(),
            size_x,
            size_z,
            min_bounds.as_ptr(),
            max_bounds.as_ptr(),
            cell_size,
            cell_height,
        )
    };
    return if res { Ok(()) } else { Err(XError::Failed) };
}

pub fn rc_mark_walkable_triangles(
    context: &mut RcContext,
    walkable_slope_angle: f32,
    verts: &[[f32; 3]],
    tris: &[[i32; 3]],
    tri_area_ids: &mut [u8],
) -> Result<(), XError> {
    if tri_area_ids.len() < tris.len() {
        return Err(XError::InvalidParam);
    }
    unsafe {
        ffi::rcMarkWalkableTriangles(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            walkable_slope_angle,
            verts.as_ptr() as *const f32,
            verts.len() as i32,
            tris.as_ptr() as *const i32,
            tris.len() as i32,
            tri_area_ids.as_mut_ptr(),
        );
    }
    return Ok(());
}

pub fn rc_clear_unwalkable_triangles(
    context: &mut RcContext,
    walkable_slope_angle: f32,
    verts: &[[f32; 3]],
    tris: &[[i32; 3]],
    tri_area_ids: &mut [u8],
) -> Result<(), XError> {
    if tri_area_ids.len() < tris.len() {
        return Err(XError::InvalidParam);
    }
    unsafe {
        ffi::rcClearUnwalkableTriangles(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            walkable_slope_angle,
            verts.as_ptr() as *const f32,
            verts.len() as i32,
            tris.as_ptr() as *const i32,
            tris.len() as i32,
            tri_area_ids.as_mut_ptr(),
        );
    }
    return Ok(());
}

pub fn rc_add_span(
    context: &mut RcContext,
    heightfield: &mut RcHeightfield,
    x: i32,
    z: i32,
    span_min: u16,
    span_max: u16,
    area_id: u8,
    flag_merge_threshold: i32,
) -> Result<(), XError> {
    let res = unsafe {
        ffi::rcAddSpan(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            heightfield.inner_mut(),
            x,
            z,
            span_min,
            span_max,
            area_id,
            flag_merge_threshold,
        )
    };
    return if res { Ok(()) } else { Err(XError::Failed) };
}

pub fn rc_rasterize_triangle(
    context: &mut RcContext,
    v0: &[f32; 3],
    v1: &[f32; 3],
    v2: &[f32; 3],
    area_id: u8,
    heightfield: &mut RcHeightfield,
    flag_merge_threshold: i32,
) -> Result<(), XError> {
    let res = unsafe {
        ffi::rcRasterizeTriangle(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            v0.as_ptr(),
            v1.as_ptr(),
            v2.as_ptr(),
            area_id,
            heightfield.inner_mut(),
            flag_merge_threshold,
        )
    };
    return if res { Ok(()) } else { Err(XError::Failed) };
}

pub fn rc_rasterize_triangles_1(
    context: &mut RcContext,
    verts: &[[f32; 3]],
    tris: &[[i32; 3]],
    tri_area_ids: &[u8],
    heightfield: &mut RcHeightfield,
    flag_merge_threshold: i32,
) -> Result<bool, XError> {
    if tri_area_ids.len() < tris.len() {
        return Err(XError::InvalidParam);
    }
    let res = unsafe {
        ffi::rcRasterizeTriangles1(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            verts.as_ptr() as *const f32,
            verts.len() as i32,
            tris.as_ptr() as *const i32,
            tri_area_ids.as_ptr(),
            tris.len() as i32,
            heightfield.inner_mut(),
            flag_merge_threshold,
        )
    };
    return Ok(res);
}

pub fn rc_rasterize_triangles_2(
    context: &mut RcContext,
    verts: &[[f32; 3]],
    tris: &[[u16; 3]],
    tri_area_ids: &[u8],
    heightfield: &mut RcHeightfield,
    flag_merge_threshold: i32,
) -> Result<bool, XError> {
    if tri_area_ids.len() < tris.len() {
        return Err(XError::InvalidParam);
    }
    let res = unsafe {
        ffi::rcRasterizeTriangles2(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            verts.as_ptr() as *const f32,
            verts.len() as i32,
            tris.as_ptr() as *const u16,
            tri_area_ids.as_ptr(),
            tris.len() as i32,
            heightfield.inner_mut(),
            flag_merge_threshold,
        )
    };
    return Ok(res);
}

pub fn rc_rasterize_triangles_3(
    context: &mut RcContext,
    verts: &[[f32; 3]],
    tri_area_ids: &[u8],
    heightfield: &mut RcHeightfield,
    flag_merge_threshold: i32,
) -> Result<bool, XError> {
    if tri_area_ids.len() < verts.len() {
        return Err(XError::InvalidParam);
    }
    let res = unsafe {
        ffi::rcRasterizeTriangles3(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            verts.as_ptr() as *const f32,
            tri_area_ids.as_ptr(),
            tri_area_ids.len() as i32,
            heightfield.inner_mut(),
            flag_merge_threshold,
        )
    };
    return Ok(res);
}

pub fn rc_filter_low_hanging_walkable_obstacles(
    context: &mut RcContext,
    walkable_climb: i32,
    heightfield: &mut RcHeightfield,
) {
    unsafe {
        ffi::rcFilterLowHangingWalkableObstacles(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            walkable_climb,
            heightfield.inner_mut(),
        );
    }
}

pub fn rc_filter_ledge_spans(
    context: &mut RcContext,
    walkable_height: i32,
    walkable_climb: i32,
    heightfield: &mut RcHeightfield,
) {
    unsafe {
        ffi::rcFilterLedgeSpans(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            walkable_height,
            walkable_climb,
            heightfield.inner_mut(),
        );
    }
}

pub fn rc_filter_walkable_low_height_spans(
    context: &mut RcContext,
    walkable_height: i32,
    heightfield: &mut RcHeightfield,
) {
    unsafe {
        ffi::rcFilterWalkableLowHeightSpans(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            walkable_height,
            heightfield.inner_mut(),
        );
    }
}

pub fn rc_get_height_field_span_count(context: &mut RcContext, heightfield: &RcHeightfield) -> i32 {
    unsafe {
        return ffi::rcGetHeightFieldSpanCount(context.0.pin_mut().get_unchecked_mut() as *mut _, heightfield.inner());
    }
}

pub fn rc_build_compact_heightfield(
    context: &mut RcContext,
    walkable_height: i32,
    walkable_climb: i32,
    heightfield: &RcHeightfield,
    compact_heightfield: &mut RcCompactHeightfield,
) -> Result<(), XError> {
    let res = unsafe {
        ffi::rcBuildCompactHeightfield(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            walkable_height,
            walkable_climb,
            heightfield.inner(),
            compact_heightfield.inner_mut(),
        )
    };
    return if res { Ok(()) } else { Err(XError::Failed) };
}

pub fn rc_erode_walkable_area(
    context: &mut RcContext,
    erosion_radius: i32,
    compact_heightfield: &mut RcCompactHeightfield,
) -> Result<(), XError> {
    let res = unsafe {
        ffi::rcErodeWalkableArea(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            erosion_radius,
            compact_heightfield.inner_mut(),
        )
    };
    return if res { Ok(()) } else { Err(XError::Failed) };
}

pub fn rc_median_filter_walkable_area(
    context: &mut RcContext,
    compact_heightfield: &mut RcCompactHeightfield,
) -> Result<(), XError> {
    let res = unsafe {
        ffi::rcMedianFilterWalkableArea(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            compact_heightfield.inner_mut(),
        )
    };
    return if res { Ok(()) } else { Err(XError::Failed) };
}

pub fn rc_mark_box_area(
    context: &mut RcContext,
    box_min_bounds: &[f32; 3],
    box_max_bounds: &[f32; 3],
    area_id: u8,
    compact_heightfield: &mut RcCompactHeightfield,
) {
    unsafe {
        ffi::rcMarkBoxArea(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            box_min_bounds.as_ptr(),
            box_max_bounds.as_ptr(),
            area_id,
            compact_heightfield.inner_mut(),
        );
    }
}

pub fn rc_offset_poly(
    verts: &[[f32; 3]],
    num_verts: i32,
    offset: f32,
    out_verts: &mut [[f32; 3]],
    max_out_verts: i32,
) -> i32 {
    unsafe {
        return ffi::rcOffsetPoly(
            verts.as_ptr() as *const f32,
            num_verts,
            offset,
            out_verts.as_mut_ptr() as *mut f32,
            max_out_verts,
        );
    }
}

pub fn rc_mark_convex_poly_area(
    context: &mut RcContext,
    verts: &[[f32; 3]],
    min_y: f32,
    max_y: f32,
    area_id: u8,
    compact_heightfield: &mut RcCompactHeightfield,
) {
    unsafe {
        ffi::rcMarkConvexPolyArea(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            verts.as_ptr() as *const f32,
            verts.len() as i32,
            min_y,
            max_y,
            area_id,
            compact_heightfield.inner_mut(),
        );
    }
}

pub fn rc_mark_cylinder_area(
    context: &mut RcContext,
    position: &[f32; 3],
    radius: f32,
    height: f32,
    area_id: u8,
    compact_heightfield: &mut RcCompactHeightfield,
) {
    unsafe {
        ffi::rcMarkCylinderArea(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            position.as_ptr(),
            radius,
            height,
            area_id,
            compact_heightfield.inner_mut(),
        );
    }
}

pub fn rc_build_distance_field(context: &mut RcContext, chf: &mut RcCompactHeightfield) -> Result<(), XError> {
    let res = unsafe { ffi::rcBuildDistanceField(context.0.pin_mut().get_unchecked_mut() as *mut _, chf.inner_mut()) };
    return if res { Ok(()) } else { Err(XError::Failed) };
}

pub fn rc_build_regions(
    context: &mut RcContext,
    chf: &mut RcCompactHeightfield,
    border_size: i32,
    min_region_area: i32,
    merge_region_area: i32,
) -> Result<(), XError> {
    let res = unsafe {
        ffi::rcBuildRegions(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            chf.inner_mut(),
            border_size,
            min_region_area,
            merge_region_area,
        )
    };
    return if res { Ok(()) } else { Err(XError::Failed) };
}

pub fn rc_build_layer_regions(
    context: &mut RcContext,
    chf: &mut RcCompactHeightfield,
    border_size: i32,
    min_region_area: i32,
) -> Result<(), XError> {
    let res = unsafe {
        ffi::rcBuildLayerRegions(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            chf.inner_mut(),
            border_size,
            min_region_area,
        )
    };
    return if res { Ok(()) } else { Err(XError::Failed) };
}

pub fn rc_build_regions_monotone(
    context: &mut RcContext,
    chf: &mut RcCompactHeightfield,
    border_size: i32,
    min_region_area: i32,
    merge_region_area: i32,
) -> Result<(), XError> {
    let res = unsafe {
        ffi::rcBuildRegionsMonotone(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            chf.inner_mut(),
            border_size,
            min_region_area,
            merge_region_area,
        )
    };
    return if res { Ok(()) } else { Err(XError::Failed) };
}

pub fn rc_build_heightfield_layers(
    context: &mut RcContext,
    chf: &RcCompactHeightfield,
    border_size: i32,
    walkable_height: i32,
    lset: &mut RcHeightfieldLayerSet,
) -> Result<(), XError> {
    let res = unsafe {
        ffi::rcBuildHeightfieldLayers(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            chf.inner(),
            border_size,
            walkable_height,
            lset.inner_mut(),
        )
    };
    return if res { Ok(()) } else { Err(XError::Failed) };
}

pub fn rc_build_contours(
    context: &mut RcContext,
    chf: &RcCompactHeightfield,
    max_error: f32,
    max_edge_len: i32,
    cset: &mut RcContourSet,
    build_flags: RcBuildContoursFlags,
) -> Result<(), XError> {
    let res = unsafe {
        ffi::rcBuildContours(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            chf.inner(),
            max_error,
            max_edge_len,
            cset.inner_mut(),
            build_flags.repr,
        )
    };
    return if res { Ok(()) } else { Err(XError::Failed) };
}

pub fn rc_build_poly_mesh(
    context: &mut RcContext,
    cset: &RcContourSet,
    nvp: i32,
    mesh: &mut RcPolyMesh,
) -> Result<(), XError> {
    let res = unsafe {
        ffi::rcBuildPolyMesh(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            cset.inner(),
            nvp,
            mesh.inner_mut(),
        )
    };
    return if res { Ok(()) } else { Err(XError::Failed) };
}

pub fn rc_merge_poly_meshes(context: &mut RcContext, meshes: &[&RcPolyMesh], mesh: &mut RcPolyMesh) -> bool {
    let tmp_meshes: Vec<_> = meshes.iter().map(|m| m.as_ptr()).collect();
    unsafe {
        return ffi::rcMergePolyMeshes(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            tmp_meshes.as_ptr(),
            meshes.len() as i32,
            mesh.inner_mut(),
        );
    }
}

pub fn rc_build_poly_mesh_detail(
    context: &mut RcContext,
    mesh: &RcPolyMesh,
    chf: &RcCompactHeightfield,
    sample_dist: f32,
    sample_max_error: f32,
    dmesh: &mut RcPolyMeshDetail,
) -> Result<(), XError> {
    let res = unsafe {
        ffi::rcBuildPolyMeshDetail(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            mesh.inner(),
            chf.inner(),
            sample_dist,
            sample_max_error,
            dmesh.inner_mut(),
        )
    };
    return if res { Ok(()) } else { Err(XError::Failed) };
}

pub fn rc_copy_poly_mesh(context: &mut RcContext, src: &RcPolyMesh, dst: &mut RcPolyMesh) -> Result<(), XError> {
    let res = unsafe {
        ffi::rcCopyPolyMesh(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            src.inner(),
            dst.inner_mut(),
        )
    };
    return if res { Ok(()) } else { Err(XError::Failed) };
}

pub fn rc_merge_poly_mesh_details(
    context: &mut RcContext,
    meshes: &[&RcPolyMeshDetail],
    mesh: &mut RcPolyMeshDetail,
) -> bool {
    let tmp_meshes: Vec<_> = meshes.iter().map(|m| m.as_ptr()).collect();
    unsafe {
        return ffi::rcMergePolyMeshDetails(
            context.0.pin_mut().get_unchecked_mut() as *mut _,
            tmp_meshes.as_ptr(),
            meshes.len() as i32,
            mesh.inner_mut(),
        );
    }
}
