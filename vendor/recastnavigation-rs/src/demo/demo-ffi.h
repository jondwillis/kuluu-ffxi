#pragma once

#include <memory>
#include "DetourNavMesh.h"
#include "MeshLoaderObj.h"
#include "ChunkyTriMesh.h"

#include "rust/cxx.h"
#include "recastnavigation-rs/src/utils.h"

inline std::unique_ptr<rcMeshLoaderObj> rcNewMeshLoaderObj()  {
    return std::make_unique<rcMeshLoaderObj>();
}

dtNavMesh* loadNavMesh(rust::Str path);
void saveNavMesh(const dtNavMesh* mesh, rust::Str path);

inline rcChunkyTriMesh* rcctm_new() {
    return new rcChunkyTriMesh();
}

inline void rcctm_delete(rcChunkyTriMesh* cm) {
    if (cm != nullptr) {
        delete cm;
    }
}
