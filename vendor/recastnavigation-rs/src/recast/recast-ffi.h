#pragma once

#include <memory>
#include "Recast.h"
#include "RecastAlloc.h"
#include "RecastAssert.h"

#include "rust/cxx.h"
#include "recastnavigation-rs/src/utils.h"

inline std::unique_ptr<rcContext> rcNewContext(bool state) { return std::make_unique<rcContext>(state); }

static_assert(sizeof(rcConfig) == 92, "rcConfig size");
static_assert(sizeof(rcSpan) == SIZE_32_64(8, 16), "rcSpan size");
static_assert(sizeof(rcSpanPool) == SIZE_32_64(16388, 32776), "rcSpanPool size");
static_assert(sizeof(rcHeightfield) == SIZE_32_64(52, 64), "rcHeightfield size");
static_assert(sizeof(rcCompactCell) == 4, "rcCompactCell size");
static_assert(sizeof(rcCompactSpan) == 8, "rcCompactSpan size");
static_assert(sizeof(rcCompactHeightfield) == SIZE_32_64(76, 96), "rcCompactHeightfield size");
static_assert(sizeof(rcHeightfieldLayer) == SIZE_32_64(76, 88), "rcHeightfieldLayer size");
static_assert(sizeof(rcHeightfieldLayerSet) == SIZE_32_64(8, 16), "rcHeightfieldLayerSet size");
static_assert(sizeof(rcContour) == SIZE_32_64(20, 32), "rcContour size");
static_assert(sizeof(rcContourSet) == SIZE_32_64(56, 64), "rcContourSet size");
static_assert(sizeof(rcPolyMesh) == SIZE_32_64(76, 96), "rcPolyMesh size");
static_assert(sizeof(rcPolyMeshDetail) == SIZE_32_64(24, 40), "rcPolyMeshDetail size");
