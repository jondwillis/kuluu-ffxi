#pragma once

#include "DetourAlloc.h"
#include "DetourAssert.h"
#include "DetourCommon.h"
#include "DetourNavMesh.h"
#include "DetourNavMeshBuilder.h"
#include "DetourNavMeshQuery.h"
#include "DetourNode.h"
#include "DetourStatus.h"

#include "rust/cxx.h"
#include "recastnavigation-rs/src/utils.h"

using c_void = void;

static_assert(sizeof(dtStatus) == sizeof(unsigned int), "dtStatus size");
static_assert(sizeof(dtNavMeshCreateParams) == SIZE_32_64(140, 208), "dtNavMeshCreateParams size");

static_assert(sizeof(dtPolyRef) == sizeof(unsigned int), "dtPolyRef size");
static_assert(sizeof(dtTileRef) == sizeof(unsigned int), "dtTileRef size");
static_assert(sizeof(dtPoly) == 32, "dtPoly size");
static_assert(sizeof(dtPolyDetail) == 12, "dtPolyDetail size");
static_assert(sizeof(dtLink) == 12, "dtLink size");
static_assert(sizeof(dtBVNode) == 16, "dtBVNode size");
static_assert(sizeof(dtOffMeshConnection) == 36, "dtOffMeshConnection size");
static_assert(sizeof(dtMeshHeader) == 100, "dtMeshHeader size");
static_assert(sizeof(dtNavMeshParams) == 28, "dtNavMeshParams size");
static_assert(sizeof(dtMeshTile) == SIZE_32_64(60, 104), "dtNavMesh size");

static_assert(sizeof(dtQueryFilter) == 260, "dtQueryFilter size");
static_assert(sizeof(dtRaycastHit) == SIZE_32_64(36, 48), "dtQueryFilter size");

//
// dtPoly
//

inline void dtp_setArea(dtPoly& poly, unsigned char a) { poly.setArea(a); }
inline void dtp_setType(dtPoly& poly, unsigned char t) { poly.setType(t); }
inline unsigned char dtp_getArea(const dtPoly& poly) { return poly.getArea(); }
inline unsigned char dtp_getType(const dtPoly& poly) { return poly.getType(); }

//
// dtMeshTile
//

inline dtStatus dtmt_storeTileState(const dtNavMesh& navMesh, dtPolyRef ref, unsigned char* data, const int maxDataSize) {
    const dtMeshTile* tile = navMesh.getTileByRef(ref);
    if (tile == nullptr) {
        return DT_FAILURE | DT_INVALID_PARAM;
    }
    return navMesh.storeTileState(tile, data, maxDataSize);
}

inline dtStatus dtmt_restoreTileState(dtNavMesh& navMesh, dtPolyRef ref, const unsigned char* data, const int maxDataSize) {
    const dtMeshTile* tile = navMesh.getTileByRef(ref);
    if (tile == nullptr) {
        return DT_FAILURE | DT_INVALID_PARAM;
    }
    return navMesh.restoreTileState(const_cast<dtMeshTile*>(tile), data, maxDataSize);
}

//
// dtQueryFilter
//

inline bool dtqf_passFilter(const dtQueryFilter& filter, const dtPolyRef ref, const dtMeshTile* tile, const dtPoly* poly) {
    return filter.passFilter(ref, tile, poly);
}

inline float dtqf_getCost(const dtQueryFilter& filter, const float* pa, const float* pb,
    const dtPolyRef prevRef, const dtMeshTile* prevTile, const dtPoly* prevPoly,
    const dtPolyRef curRef, const dtMeshTile* curTile, const dtPoly* curPoly,
    const dtPolyRef nextRef, const dtMeshTile* nextTile, const dtPoly* nextPoly
) {
    return filter.getCost(pa, pb, prevRef, prevTile, prevPoly, curRef, curTile, curPoly, nextRef, nextTile, nextPoly);
}

//
// dtNavMeshQuery
//

inline dtStatus dtnmq_findRandomPoint(const dtNavMeshQuery& query,
    const dtQueryFilter* filter, rust::Fn<float()> frand, dtPolyRef* randomRef, float* randomPt) {
    return query.findRandomPoint(filter, frand, randomRef, randomPt);
}

inline dtStatus dtnmq_findRandomPointAroundCircle(const dtNavMeshQuery& query, dtPolyRef startRef, const float* centerPos,
    const float maxRadius, const dtQueryFilter* filter, rust::Fn<float()> frand, dtPolyRef* randomRef, float* randomPt) {
    return query.findRandomPointAroundCircle(startRef, centerPos, maxRadius, filter, frand, randomRef, randomPt);
}
