# Detour `.nav` binary format (LSB / FFXI)

Reference for a future Rust-native parser. Today `ffxi-nav` only
reads PNG occupancy bitmaps via `GridNav`; the LSB submodule
(`server/navmeshes/`) ships ~300 Recast/Detour binary files that
this crate does **not** yet consume. Operators wanting cliff-aware
pathfinding hand-trace PNGs into `~/.config/ffxi-mcp/heightmaps/`
in the meantime.

This doc lays out the on-disk layout so the next implementer can
write a parser without re-reverse-engineering it.

## Provenance

LSB stores all zone navigation data as serialized
`recastnavigation/Detour/dtNavMesh` instances. The serialization
header is custom to LSB (it wraps the per-tile `dtNavMesh::tile`
serialization with a small `NavMeshSetHeader` and per-tile
`NavMeshTileHeader`). Authoritative reader: `server/src/map/navmesh.cpp`,
function `CNavMesh::load`.

## File layout

All multi-byte fields are little-endian on x86 / x86_64 (host
order at write time; LSB never byte-swaps these).

```
+--------------------------------------------+
| NavMeshSetHeader  (40 bytes)               |
|   magic        : i32  = 0x5453454D ("MSET")|
|   version      : i32  = 1                  |
|   numTiles     : i32                       |
|   params       : dtNavMeshParams (32 bytes)|
+--------------------------------------------+
| Tile 0:                                    |
|   NavMeshTileHeader (12 bytes)             |
|     tileRef    : u64                       |
|     dataSize   : i32                       |
|   Tile payload : dataSize bytes            |
+--------------------------------------------+
| Tile 1: …                                  |
| …                                          |
+--------------------------------------------+
```

### `dtNavMeshParams` (32 bytes)

```
orig[3]    : f32  // world-space origin of the tile grid
tileWidth  : f32  // size of one tile along X
tileHeight : f32  // size of one tile along Z
maxTiles   : i32
maxPolys   : i32
```

`orig` is in **Detour** coordinates. LSB maintains a runtime axis
flip between FFXI and Detour space (`y *= -1; z *= -1`); the
serialized values are pre-flip Detour-side, so a Rust reader
needs to apply `ToFFXIPos` (negate Y and Z) before any value
is compared against an FFXI position from the wire.

### Tile payload

Each tile begins with a `dtMeshHeader` (104 bytes) whose first
field is `magic = 'D' 'N' 'A' 'V'` (`DNAV` LE = `0x56414E44`).
The header declares vertex / poly / link / detail-mesh counts,
which together index into the variable-length arrays that
follow:

1. `dtPoly[polyCount]` — connectivity table
2. `dtLink[maxLinkCount]` — adjacency
3. `dtPolyDetail[polyCount]` — detail-mesh metadata
4. `f32 vertices[3 * vertCount]`
5. `f32 detailVerts[3 * detailVertCount]`
6. `u8 detailTris[4 * detailTriCount]`
7. `dtBVNode[bvNodeCount]` — bounding-volume tree
8. `dtOffMeshConnection[offMeshConCount]` — jump links

Field-by-field byte counts are in `Detour/Source/DetourNavMesh.cpp::dtCreateNavMeshData`
upstream. No alignment padding within a tile.

## Naming convention

LSB ships the files name-keyed in the SQL table
(`server/sql/zone_settings.sql`). Most zones look like
`Rabao.nav`, `West_Sarutabaruta.nav`. Three legacy
zones use numeric filenames: `133.nav`, `229.nav`, `49.nav`.

A Rust reader resolves a zone id by:

1. Look up `zone_id → zone_name` from `zone_names::zone_name`.
2. Try `<zone_name>.nav` in the navmesh directory.
3. Fall back to `<zone_id>.nav`.

That two-pass strategy is what `reactor::detour_navmesh_path`
already does for existence-checking; a real reader plugs into
the same call site.

## What a parser still has to implement

The format above gets you the bytes. To answer `findPath(from, to)`
you also need:

1. `dtNavMeshQuery` initialization (allocates an A* working set;
   sized via `init(navmesh, MAX_NAV_NODES)`).
2. `findNearestPoly` — projects the world-space start/end onto
   the nearest mesh polygon within a search half-extent. LSB
   uses `extents = (10, 20, 10)` (yalms) for nearest-poly with
   `verticalLimit = 5.0`.
3. `findPath` — A* over the polygon graph. Returns a polygon
   ref sequence.
4. `findStraightPath` — converts the polygon sequence into a
   yalm-space line via the funnel algorithm.

These are all in `Detour/Source/DetourNavMeshQuery.cpp`. Either
ship a Rust port of those routines or FFI to a built copy of
`recastnavigation`. Pure-Rust port is preferable (no cmake
build dependency) and is realistically ~600-800 LoC for the
subset LSB uses.

## Coordinate flips between Detour and FFXI

Detour stores `y` as up; FFXI exposes `y` as down (height grows
*downward* in `position_t`). LSB has six small helpers in
`navmesh.cpp` that flip Y and Z:

```cpp
void CNavMesh::ToFFXIPos(position_t* out)   { out->y *= -1; out->z *= -1; }
void CNavMesh::ToDetourPos(position_t* out) { out->y *= -1; out->z *= -1; }
```

Any Rust port must apply the same flip on every position that
crosses the boundary between FFXI wire space and Detour storage
space. Skipping this turns "go upstairs" into "fall through the
floor."

## Where to start

A small skeleton would land like:

```rust
// ffxi-nav/src/detour.rs (future)

pub struct DetourNav {
    params: DtNavMeshParams,
    tiles: Vec<DtTile>,
}

impl DetourNav {
    pub fn from_path(path: &Path) -> Result<Self, NavError> {
        // 1. Read header, validate magic == 'TESM' / version == 1.
        // 2. Loop numTiles: NavMeshTileHeader + dataSize bytes.
        // 3. Parse each tile payload: dtMeshHeader + verts/polys/links/...
        // ...
    }
}

impl NavMesh for DetourNav {
    fn path(&self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>> {
        // 1. Apply Y/Z flip → Detour space.
        // 2. findNearestPoly(start), findNearestPoly(end).
        // 3. A* on the poly graph.
        // 4. Funnel → straight-path.
        // 5. Apply inverse Y/Z flip → FFXI space, return.
    }
}
```

The `NavMesh` trait already exists in `lib.rs`; plug `DetourNav`
in alongside `GridNav` and update `reactor::default_load_navmesh`
to prefer it when present.

## See also

- `server/src/map/navmesh.cpp` — LSB's reader / query glue
- `recastnavigation/Detour/Include/DetourNavMesh.h` — `dtMeshHeader`,
  `dtPoly`, `dtLink`, `dtMeshTile`, `dtNavMeshParams`
- `recastnavigation/Detour/Source/DetourNavMeshQuery.cpp` —
  `findNearestPoly`, `findPath`, `findStraightPath`
- `ffxi-nav/src/zone_names.rs` — id → name lookup
