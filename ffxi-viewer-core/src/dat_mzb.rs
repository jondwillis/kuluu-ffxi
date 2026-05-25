//! MZB zone overlay: load a real FFXI zone mesh-library from a DAT
//! file, decode the grid-cell placement table, and spawn the resulting
//! instanced geometry as two merged meshes (collision + non-collision).
//!
//! Pattern mirrors [`crate::dat_mmb`] — slash-command dispatcher fires
//! [`LoadMzbRequest`]; [`kick_load_mzb_tasks`] hands the parse off to a
//! background `AsyncComputeTaskPool` task; [`poll_load_mzb_tasks`]
//! picks up the result and runs the main-thread spawn.
//! Placement transforms are decoded from the MZB header and applied
//! per-instance inside [`load_mzb_placed`]; the resulting submeshes
//! land at correct world coordinates relative to the zone origin.
//! Small indoor zones with no grid records fall back to "spawn the
//! library at origin" (see the placements-empty branch of
//! `load_mzb_placed`).
//!
//! Known gaps:
//!   - Material textures: MZB stores 4-bit `material_id` per triangle
//!     but the `material_id → TIM texture name` lookup table lives in
//!     FFXI's runtime material engine which we don't decode. We render
//!     a 16-color muted palette tint instead (see `MZB_MATERIAL_PALETTE`).
//!   - Water planes: lotus extracts `water_height` per grid cell in
//!     `parseGridMesh` and spawns flat alpha quads. Not yet decoded
//!     on our side.
//!   - Generator chunks (kind 0x05) referenced from MZB: ambient VFX
//!     spawners (fountain spray, torch glow). Requires baseline
//!     particle renderer.
//!
//! Native-only for the same reason as `dat_mmb.rs`: `ffxi-dat::DatRoot`
//! does sync `fs::read` of the user's local install.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::VecDeque;
use std::fs;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use ffxi_dat::mmb::MmbHeader;
use ffxi_dat::{mmb, mzb, walk, ChunkKind, DatRoot};

use crate::components::{IsSelf, WorldEntity};
use crate::snapshot::SceneState;
use ffxi_viewer_wire::EntityKind;

/// Default cull distances (yalms). Operator-tunable at runtime via the
/// `/drawdistance setworld N` / `/drawdistance setmob N` slash commands
/// (Ashita / Windower convention). Stored in [`DrawDistance`] as a
/// `Resource` so the cull systems can read the live value each frame
/// without rebuilding the systems.
pub const DEFAULT_WORLD_DRAW_DISTANCE: f32 = 80.0;
pub const DEFAULT_MOB_DRAW_DISTANCE: f32 = 50.0;

/// 16-color palette for the 4-bit `material` id decoded from MZB
/// triangle index high bits (see [`ffxi_dat::mzb::MzbTriangleInfo`] and
/// `vendor/xi-tinkerer/crates/dats/src/formats/zone_data/mesh_block.rs:51-77`).
///
/// Without ground-truth `material_id → texture name` mapping (FFXI's
/// runtime maps these to TIMs via the engine's material table, which
/// we don't yet decode), each material gets a distinct muted RGB tint
/// instead of a real texture. Walls/floors/stairs/sand each render in
/// their own subdued hue — operator-readable without going rainbow.
///
/// Values are linear sRGB scalars in `[0, 1]`. They're multiplied
/// per-vertex with a `shade` factor (`0.4..1.0` from the upward normal
/// component) inside [`spawn_mzb_overlay`] before being baked
/// into `ATTRIBUTE_COLOR`. The material's WHITE base color lets the
/// vertex color drive the final pixel.
const MZB_MATERIAL_PALETTE: [[f32; 3]; 16] = [
    [0.85, 0.55, 0.40], // 0
    [0.75, 0.65, 0.45], // 1
    [0.50, 0.65, 0.55], // 2
    [0.55, 0.70, 0.75], // 3
    [0.65, 0.55, 0.75], // 4
    [0.80, 0.65, 0.55], // 5
    [0.65, 0.60, 0.50], // 6
    [0.55, 0.55, 0.60], // 7
    [0.70, 0.50, 0.45], // 8
    [0.45, 0.55, 0.50], // 9
    [0.60, 0.70, 0.40], // 10
    [0.50, 0.45, 0.40], // 11
    [0.75, 0.70, 0.50], // 12
    [0.55, 0.60, 0.65], // 13
    [0.45, 0.50, 0.55], // 14
    [0.65, 0.65, 0.55], // 15
];

/// Runtime-tunable cull distances. World controls MZB overlay
/// entities, mob controls non-PC entity capsules (mobs/NPCs/pets).
/// PCs are never culled by distance — party members and other PCs
/// stay visible regardless so the operator can still target them.
/// `/zonegeom` tri-state. **Default is `All`** because the MZB grid-cell
/// placement layer is the *primary visible-geometry source* for all
/// zones — collision and decoration both. Dungeon zones (Bastok Mines,
/// tunnels) reference *stub* MMBs (`kabe-atariyou`-family) with
/// `pieces=0`, so without grid-cell content their walls/floors would
/// be invisible. Cities have both layers populated. `Collision` shows
/// only LoS-blockers; `Off` hides MZB entirely for a clean MMB-only
/// view.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneGeomMode {
    /// Hide all MZB geometry. Operator opt-out for clean MMB-only view.
    #[default]
    Off,
    /// Show only LoS-blocking (collision) meshes — flag bit 0 == 0.
    Collision,
    /// Show both collision and non-collision (decorative) meshes.
    /// Default — the full visible-geometry layer.
    All,
    /// Camera-collision debug overlay. MZB collision visible (same as
    /// `Collision`), plus the client crate's `draw_camera_collision_debug`
    /// system layers gizmos over the top: each `CollisionBvh`'s root AABB
    /// as a green wirebox, and the live player→camera ray as a red line.
    /// Lets the operator see exactly what the camera push-in raycast tests
    /// against. Diagnostic only — has no effect on actual collision math.
    Camera,
}

impl ZoneGeomMode {
    /// `toggle`-cycle order: Collision → All → Camera → Off → Collision.
    /// Skips no states; lets a single keybind walk the full tri-state +
    /// debug overlay.
    pub fn cycle(self) -> Self {
        match self {
            Self::Collision => Self::All,
            Self::All => Self::Camera,
            Self::Camera => Self::Off,
            Self::Off => Self::Collision,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Collision => "collision",
            Self::All => "all",
            Self::Camera => "camera",
        }
    }
}

#[derive(Resource, Debug, Clone, Copy)]
pub struct DrawDistance {
    pub world: f32,
    pub mob: f32,
    /// `/zonegeom` setting — bundled into this resource so the
    /// text-input dispatcher stays under Bevy's 16 `SystemParam` limit.
    pub zone_geom_mode: ZoneGeomMode,
}

impl Default for DrawDistance {
    fn default() -> Self {
        Self {
            world: DEFAULT_WORLD_DRAW_DISTANCE,
            mob: DEFAULT_MOB_DRAW_DISTANCE,
            zone_geom_mode: ZoneGeomMode::default(),
        }
    }
}

/// Sub-marker for the merged collision mesh (flag bit 0 == 0). Lets
/// `apply_zone_geom_visibility` toggle collision vs non-collision
/// independently.
#[derive(Component)]
pub struct MzbCollisionMesh;

/// CPU-side copy of the merged MZB **collision** triangle soup (already
/// transformed into Bevy world coordinates). Owned as a `Resource` so
/// the client-side ground-snap and `/debug heights` paths can do
/// per-tick raycasts without walking the Bevy `Assets<Mesh>` storage.
///
/// Dropped + replaced each `LoadMzbRequest` consumption, so zone
/// transitions stay correct.
#[derive(Resource, Default)]
pub struct MzbCollisionGeometry {
    pub positions: Vec<Vec3>,
    /// Flat triangle indices: `tri_i` = `(indices[i*3], indices[i*3+1], indices[i*3+2])`.
    pub indices: Vec<u32>,
    /// XZ uniform-grid index: `(cell_x, cell_z) → tri_index` (where
    /// `tri_index * 3 ..= tri_index * 3 + 2` slices [`indices`]). A
    /// triangle is inserted into every cell its 2D AABB overlaps so a
    /// downward raycast at the entity's XZ only scans the local cell
    /// rather than the whole zone. Empty when no MZB is loaded.
    pub cell_index: std::collections::HashMap<(i32, i32), Vec<u32>>,
}

/// One background MZB load completion: the same triple returned by
/// [`load_mzb_placed`] plus the zone-MMB placement list from
/// [`build_zone_mmb_spawns`]. Wrapped in `Arc`s so [`ZoneGeomCache`]
/// and the spawn step can share ownership without cloning the bulky
/// inner Vecs — `submeshes` for a populated city zone can run tens of
/// thousands of entries.
#[derive(Clone)]
pub struct LoadedZoneGeom {
    pub submeshes: Arc<Vec<MzbSubMesh>>,
    pub instances: Arc<Vec<MzbInstance>>,
    /// Zone-MMB placement table (`build_zone_mmb_spawns` result). `Err`
    /// gets surfaced as a chat toast at spawn time, matching the
    /// pre-background behaviour. `Ok(Vec::new())` for zones with no
    /// MMB placement table.
    pub mmb_spawns: Result<Vec<ZoneMmbSpawn>, String>,
}

/// In-flight background MZB-load tasks, keyed by `file_id`. Populated
/// by [`kick_load_mzb_tasks`], drained by [`poll_load_mzb_tasks`] as
/// each task reports `Ready`. The `Vec<LoadMzbRequest>` carries every
/// request that arrived for the same `file_id` while the task was in
/// flight, so a burst of duplicate auto-loads coalesces to one parse.
///
/// Dropped wholesale by `OnExit(AppPhase::InGame)` cleanup — pending
/// tasks are cancelled when the `Task` is dropped, so a /logout
/// mid-zone-load doesn't leak background work into the next session.
#[derive(Resource, Default)]
pub struct LoadMzbInFlight {
    pub tasks: std::collections::HashMap<u32, (Vec<LoadMzbRequest>, Task<LoadedZoneGeom>)>,
}

/// Bounded LRU of recently-loaded zone geometry. Zone transitions in
/// FFXI cycle through a small working set (La Theine ↔ Tahrongi ↔
/// Selbina ↔ Mhaura, plus three character-house zones), so a 4-entry
/// cache covers the common pattern of "user re-enters the previous
/// zone". A cache hit skips file I/O + XOR decrypt + parse + bake
/// entirely; the spawn step still runs (it owns the GPU asset upload
/// + ECS spawns, which can't be cached because handles are scoped to
/// the current session's `Assets<...>` storage).
///
/// `Arc<...>` lets the cache and the spawn step share the inner Vecs
/// without copying — the spawn step iterates `&[MzbSubMesh]` /
/// `&[MzbInstance]` so a borrow is sufficient. Also drained in
/// `despawn_ingame_entities`: zone-geometry cache entries reference
/// session-scoped data and a stale entry on relogin could otherwise
/// short-circuit a fresh parse on the next zone-in.
#[derive(Resource, Default)]
pub struct ZoneGeomCache {
    /// Front of the deque = most-recently-used. Pushes to front,
    /// evicts from back when length exceeds [`ZONE_GEOM_CACHE_CAP`].
    pub entries: VecDeque<(u32, LoadedZoneGeom)>,
}

/// Maximum number of cached zone geometries. Sized for the typical
/// FFXI cycle: one outdoor zone + adjacent zones + a player house —
/// four entries cover the round-trip without growing the cache
/// indefinitely on long play sessions.
pub const ZONE_GEOM_CACHE_CAP: usize = 4;

impl ZoneGeomCache {
    /// Cache hit: returns a clone of the `Arc` triple (cheap — atomic
    /// refcount bumps, no Vec copies) and promotes the entry to MRU.
    fn get_and_promote(&mut self, file_id: u32) -> Option<LoadedZoneGeom> {
        let pos = self.entries.iter().position(|(id, _)| *id == file_id)?;
        let entry = self.entries.remove(pos)?;
        let geom = entry.1.clone();
        self.entries.push_front(entry);
        Some(geom)
    }

    /// Insert a freshly-parsed geometry at the MRU position; evict the
    /// LRU entry when over capacity.
    fn insert(&mut self, file_id: u32, geom: LoadedZoneGeom) {
        // De-dupe: if the same file_id is already present (e.g. two
        // back-to-back kicks before the first poll fired), replace it
        // in place rather than pushing a second copy.
        if let Some(pos) = self.entries.iter().position(|(id, _)| *id == file_id) {
            self.entries.remove(pos);
        }
        self.entries.push_front((file_id, geom));
        while self.entries.len() > ZONE_GEOM_CACHE_CAP {
            self.entries.pop_back();
        }
    }
}

/// Width (yalms) of one XZ cell in [`MzbCollisionGeometry::cell_index`].
/// 8 yalms is large enough that typical wall/floor tris (1–4 yalms
/// across) drop into one or two cells, small enough that a populated
/// outdoor zone keeps each cell's tri list to tens of entries.
pub const MZB_GRID_CELL: f32 = 8.0;

/// Minimum **|normal.y|** (in Bevy world frame) for a triangle to count
/// as a walkable horizontal surface in
/// [`MzbCollisionGeometry::ground_raycast`]. `0.5` corresponds to a
/// 60° max slope, matching the legacy FFXI stair-gradient cap so
/// ramps through steep dungeon stairs stay eligible, while vertical
/// walls (`|n.y|` ≈ 0) are excluded.
///
/// The test is on the **absolute value** of `n.y` because FFXI's MZB
/// winding convention emits floor triangles with `n.y ≈ -1` after the
/// `(x, -y, -z)` axis flip in `load_mzb_placed` — the cross-product
/// of the two edges as wound on disk points away from the visible
/// top of the surface. Filtering by `n.y > 0` would reject every
/// floor in the dataset; testing `|n.y|` keeps both authoring
/// conventions (CCW and CW) eligible. Overhead geometry the entity
/// shouldn't snap to is still excluded by the caller's `ceiling_y`
/// bound — see [`MzbCollisionGeometry::ground_raycast`].
pub const FLOOR_NORMAL_MIN: f32 = 0.5;

impl MzbCollisionGeometry {
    /// Number of triangles backing this geometry.
    pub fn tri_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Cast a ray straight down (Bevy −Y) at (`xz.x`, `xz.y`) and
    /// return the **highest** Y of any **upward-facing** triangle hit
    /// at or below `ceiling_y`. `None` when no qualifying triangle
    /// sits in the column.
    ///
    /// "Horizontal" is `|normal.y| ≥ FLOOR_NORMAL_MIN` — without that
    /// filter the raycast would happily snap the entity onto a
    /// vertical wall edge, because vertical-wall hits can still
    /// satisfy `hit_y ≤ ceiling_y` if the entity is already elevated.
    /// FFXI's MZB floor winding produces `n.y ≈ -1` in Bevy frame
    /// (see [`FLOOR_NORMAL_MIN`]), so the test uses the absolute
    /// value of `n.y`.
    ///
    /// `ceiling_y` continues to filter overhead floor-like geometry —
    /// arches, gate roofs, second-floor surfaces the player is walking
    /// *under*. Of the remaining candidates, we still pick the highest
    /// (multi-level-building case: player on 2nd floor → snap to the
    /// 2nd floor, not the basement).
    pub fn ground_raycast(&self, xz: Vec2, ceiling_y: f32) -> Option<f32> {
        let mut best_y: Option<f32> = None;
        self.for_each_hit_in_column(xz, |hit_y, normal| {
            if normal.y.abs() < FLOOR_NORMAL_MIN || hit_y > ceiling_y {
                return;
            }
            best_y = Some(match best_y {
                Some(prev) if prev > hit_y => prev,
                _ => hit_y,
            });
        });
        best_y
    }

    /// Return every triangle the downward raycast hits at (`xz`),
    /// sorted by `hit_y` descending. Unfiltered — both upward- and
    /// downward-facing tris are included so the diagnostic
    /// (`/debug heights`) can show *why* a tri was rejected by
    /// [`ground_raycast`] (normal too low, above ceiling, etc.).
    pub fn ground_raycast_all(&self, xz: Vec2) -> Vec<(f32, Vec3)> {
        let mut hits: Vec<(f32, Vec3)> = Vec::new();
        self.for_each_hit_in_column(xz, |hit_y, normal| hits.push((hit_y, normal)));
        hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        hits
    }

    /// Common driver: scan only the cell at (`xz`) (or every tri when
    /// the cell index is empty, i.e. legacy callers before the grid
    /// was built), and invoke `visit(hit_y, normal)` for each tri the
    /// ray intersects. Normal is in Bevy world frame, normalized.
    fn for_each_hit_in_column(&self, xz: Vec2, mut visit: impl FnMut(f32, Vec3)) {
        const RAY_ORIGIN_Y: f32 = 1000.0;
        let orig = Vec3::new(xz.x, RAY_ORIGIN_Y, xz.y);
        let dir = Vec3::new(0.0, -1.0, 0.0);

        // Pull the candidate tri-index list from the spatial grid when
        // available. Falls back to a full scan if the resource was
        // populated without the index (defensive — every code path
        // that fills `positions/indices` now also fills `cell_index`,
        // but we don't want a partial-load to crash the raycast).
        if !self.cell_index.is_empty() {
            let cell = (
                (xz.x / MZB_GRID_CELL).floor() as i32,
                (xz.y / MZB_GRID_CELL).floor() as i32,
            );
            if let Some(tri_ids) = self.cell_index.get(&cell) {
                for &tri_id in tri_ids {
                    self.visit_tri(orig, dir, tri_id as usize, &mut visit);
                }
            }
            return;
        }

        for tri_id in 0..(self.indices.len() / 3) {
            self.visit_tri(orig, dir, tri_id, &mut visit);
        }
    }

    fn visit_tri(&self, orig: Vec3, dir: Vec3, tri_id: usize, visit: &mut impl FnMut(f32, Vec3)) {
        let base = tri_id * 3;
        let v0 = self.positions[self.indices[base] as usize];
        let v1 = self.positions[self.indices[base + 1] as usize];
        let v2 = self.positions[self.indices[base + 2] as usize];
        if let Some(t) = ray_tri_intersect(orig, dir, v0, v1, v2) {
            let hit_y = orig.y + t * dir.y;
            let normal = (v1 - v0).cross(v2 - v0).normalize_or_zero();
            visit(hit_y, normal);
        }
    }
}

/// Build the XZ cell index from the merged collision positions +
/// indices. Each triangle is inserted into every grid cell its 2D AABB
/// (XZ projection) overlaps. Cheap to call once at zone load; the
/// `HashMap` allocation stays around as long as the zone is loaded.
fn build_cell_index(
    positions: &[Vec3],
    indices: &[u32],
) -> std::collections::HashMap<(i32, i32), Vec<u32>> {
    let mut idx: std::collections::HashMap<(i32, i32), Vec<u32>> = std::collections::HashMap::new();
    for (tri_id, tri) in indices.chunks_exact(3).enumerate() {
        let v0 = positions[tri[0] as usize];
        let v1 = positions[tri[1] as usize];
        let v2 = positions[tri[2] as usize];
        let min_x = v0.x.min(v1.x).min(v2.x);
        let max_x = v0.x.max(v1.x).max(v2.x);
        let min_z = v0.z.min(v1.z).min(v2.z);
        let max_z = v0.z.max(v1.z).max(v2.z);
        let cx0 = (min_x / MZB_GRID_CELL).floor() as i32;
        let cx1 = (max_x / MZB_GRID_CELL).floor() as i32;
        let cz0 = (min_z / MZB_GRID_CELL).floor() as i32;
        let cz1 = (max_z / MZB_GRID_CELL).floor() as i32;
        for cz in cz0..=cz1 {
            for cx in cx0..=cx1 {
                idx.entry((cx, cz)).or_default().push(tri_id as u32);
            }
        }
    }
    idx
}

/// Möller–Trumbore ray-triangle intersection. Returns `t` (≥ ε), or
/// `None` if the ray misses.
fn ray_tri_intersect(orig: Vec3, dir: Vec3, v0: Vec3, v1: Vec3, v2: Vec3) -> Option<f32> {
    const EPS: f32 = 1e-7;
    let e1 = v1 - v0;
    let e2 = v2 - v0;
    let h = dir.cross(e2);
    let a = e1.dot(h);
    if a.abs() < EPS {
        return None;
    }
    let f = 1.0 / a;
    let s = orig - v0;
    let u = f * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = f * dir.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = f * e2.dot(q);
    if t > EPS {
        Some(t)
    } else {
        None
    }
}

/// Sub-marker for the merged non-collision (decorative) mesh.
#[derive(Component)]
pub struct MzbNonCollisionMesh;

/// Propagate `DrawDistance.zone_geom_mode` onto the MZB overlay tree.
/// Only writes when the resource has changed — skips per-frame iteration
/// cost when the operator hasn't touched the toggle.
pub fn apply_zone_geom_visibility(
    draw: Res<DrawDistance>,
    mut q_collision: Query<&mut Visibility, (With<MzbCollisionMesh>, Without<MzbNonCollisionMesh>)>,
    mut q_noncollision: Query<
        &mut Visibility,
        (With<MzbNonCollisionMesh>, Without<MzbCollisionMesh>),
    >,
) {
    if !draw.is_changed() {
        return;
    }
    let (want_collision, want_noncollision) = match draw.zone_geom_mode {
        ZoneGeomMode::Off => (Visibility::Hidden, Visibility::Hidden),
        ZoneGeomMode::Collision => (Visibility::Inherited, Visibility::Hidden),
        ZoneGeomMode::All => (Visibility::Inherited, Visibility::Inherited),
        // Camera-debug mode shows the same MZB collision layer as
        // `Collision`, then the client crate's gizmo system layers
        // BVH AABBs + the active raycast on top. Non-collision (decor)
        // stays hidden so the gizmos read clearly.
        ZoneGeomMode::Camera => (Visibility::Inherited, Visibility::Hidden),
    };
    for mut v in q_collision.iter_mut() {
        if *v != want_collision {
            *v = want_collision;
        }
    }
    for mut v in q_noncollision.iter_mut() {
        if *v != want_noncollision {
            *v = want_noncollision;
        }
    }
}

/// Marker for overlay entities spawned by this module. Includes both
/// `/load_mzb`-loaded and auto-loaded-on-zone-change entities — the
/// finer-grained [`AutoMzbOverlay`] marker is added in *addition* on
/// auto-loaded ones so the zone-change watcher can despawn them
/// without clobbering the operator's manual loads.
#[derive(Component)]
pub struct MzbOverlay;

/// Sub-marker added on top of [`MzbOverlay`] when the entity was
/// spawned by the auto-load-on-zone-change system. Lets that system
/// recognize "its own" entities for despawn-on-next-zone, leaving
/// `/load_mzb` manual loads alone.
#[derive(Component)]
pub struct AutoMzbOverlay;

/// Spawn-a-zone-mesh-library-at-position request. `world_pos` is
/// already in Bevy coordinates — the parser pre-applies `ffxi_to_bevy`
/// so this system stays axis-agnostic.
#[derive(Message, Debug, Clone, Copy)]
pub struct LoadMzbRequest {
    pub file_id: u32,
    /// Optional explicit chunk index. `None` means "scan for the first
    /// kind=0x1C (MZB) chunk in the file", matching the convenience
    /// behavior of `examples/dat-mzb-probe.rs`. Zone-bundle DATs
    /// usually have exactly one MZB.
    pub chunk_idx: Option<usize>,
    pub world_pos: Vec3,
    /// `true` for auto-load-on-zone-change requests — the spawn code
    /// tags the resulting entities with [`AutoMzbOverlay`] so the
    /// zone-change watcher can identify them on the next change.
    /// `/load_mzb` slash command always sets this `false`.
    pub auto_loaded: bool,
}

/// Pure-data Bevy-ready bake of one MZB library mesh.
pub struct MzbSubMesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    /// One per triangle (`indices.len() / 3` entries). Carries the 4-bit
    /// `material` id (0..15) decoded from the index high bits in
    /// `ffxi_dat::mzb::parse_one_mesh`. Used by the renderer to
    /// partition the merged geometry into one sub-mesh per material so
    /// each can carry its own color/texture. Cross-ref:
    /// `vendor/xi-tinkerer/crates/dats/src/formats/zone_data/mesh_block.rs:51-77`.
    pub tri_material: Vec<u8>,
    /// Per-mesh flag from the MZB record header. Bit 0 = does NOT
    /// block LoS (visual-only / non-collision). Surface so the caller
    /// can colorize collision vs non-collision geometry distinctly.
    pub flags: u16,
}

/// One instance of a baked submesh in world space. The submesh
/// referenced by `submesh_idx` is in the matching `Vec<MzbSubMesh>`
/// returned alongside this list. `bevy_transform` is already in Bevy
/// world coordinates (MZB matrix decomposed and re-mapped through
/// `ffxi_to_bevy`).
pub struct MzbInstance {
    pub submesh_idx: usize,
    pub bevy_transform: Transform,
    /// MZB-Y of the water surface at this placement's grid cell, when
    /// the vis-entry records one. Bevy-space (axis-flipped from the
    /// MZB native value). The renderer spawns a flat alpha quad at
    /// this Y, sized to the geometry's XZ extent. `None` for dry
    /// placements.
    pub water_height_bevy: Option<f32>,
}

/// Load + decrypt + parse the MZB chunk of `file_id`. Returns:
///   - `submeshes`: one entry per unique `geometry_offset` referenced
///     by any placement (deduped). Each is the bare library geometry
///     in MZB-local space — no instance transform applied.
///   - `instances`: one entry per placement, with `submesh_idx`
///     pointing into `submeshes` and a Bevy-space `Transform`.
///
/// Fallback: when the MZB has no placements at all (e.g. small
/// indoor zones with no grid), returns every library mesh as its own
/// submesh with a single identity placement each — same behavior as
/// the pre-Phase-9b "spawn at origin" path.
pub fn load_mzb_placed(
    file_id: u32,
    chunk_idx: Option<usize>,
) -> Result<(Vec<MzbSubMesh>, Vec<MzbInstance>), String> {
    let (header, plain, _chunks) = load_decrypted(file_id, chunk_idx)?;

    let placements =
        mzb::parse_placements(&plain, &header).map_err(|e| format!("MZB parse_placements: {e}"))?;

    if placements.is_empty() {
        // Fallback: no grid placements decoded — spawn the library at
        // origin (old behavior). Bake every library mesh in parallel.
        let meshes =
            mzb::parse_meshes(&plain, &header).map_err(|e| format!("MZB parse_meshes: {e}"))?;
        // Parallel bake: each `bake_submesh` is pure CPU and independent.
        // Use `AsyncComputeTaskPool::scope` so the work joins before
        // returning — matches the function's "load, parse, bake" sync
        // contract (callers expect owned Vecs back).
        let pool = AsyncComputeTaskPool::get();
        let baked: Vec<Option<MzbSubMesh>> = pool.scope(|s| {
            for m in &meshes {
                s.spawn(async move {
                    if m.vertices.is_empty() || m.triangles.is_empty() {
                        None
                    } else {
                        Some(bake_submesh(m))
                    }
                });
            }
        });
        let mut submeshes = Vec::with_capacity(baked.len());
        let mut instances = Vec::with_capacity(baked.len());
        for sub in baked.into_iter().flatten() {
            let idx = submeshes.len();
            submeshes.push(sub);
            instances.push(MzbInstance {
                submesh_idx: idx,
                bevy_transform: Transform::IDENTITY,
                water_height_bevy: None,
            });
        }
        return Ok((submeshes, instances));
    }

    // Dedupe by geometry_offset, then bake the unique offsets in parallel.
    // The placement loop below references baked entries by `submesh_idx`,
    // so the order of submeshes is fixed by the order we first see each
    // unique offset — preserve that to keep the per-instance indices stable.
    let mut unique_offsets: Vec<u32> = Vec::new();
    let mut offset_to_idx: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    for p in &placements {
        if let std::collections::hash_map::Entry::Vacant(e) = offset_to_idx.entry(p.geometry_offset)
        {
            e.insert(unique_offsets.len());
            unique_offsets.push(p.geometry_offset);
        }
    }
    // Parallel parse+bake of each unique geometry_offset. Empty/bad
    // records bake to `None`; the placement loop below filters them.
    // `pool.scope` blocks until every spawned task is done — we get
    // back a Vec aligned 1:1 with `unique_offsets`.
    let pool = AsyncComputeTaskPool::get();
    let baked: Vec<Option<MzbSubMesh>> = pool.scope(|s| {
        let plain_ref = &plain;
        for &offset in &unique_offsets {
            s.spawn(async move {
                let m = mzb::parse_mesh_at(plain_ref, offset as usize).ok()?;
                if m.vertices.is_empty() || m.triangles.is_empty() {
                    return None;
                }
                Some(bake_submesh(&m))
            });
        }
    });

    // Compact `baked` into the dense `submeshes` Vec by remapping the
    // unique-offset index → dense index (skipping bad records). Then the
    // placement loop walks `offset_to_idx → unique_idx → dense_idx`.
    let mut submeshes: Vec<MzbSubMesh> = Vec::with_capacity(baked.len());
    let mut unique_to_dense: Vec<Option<usize>> = Vec::with_capacity(baked.len());
    for sub in baked {
        match sub {
            Some(s) => {
                unique_to_dense.push(Some(submeshes.len()));
                submeshes.push(s);
            }
            None => unique_to_dense.push(None),
        }
    }
    let mut instances: Vec<MzbInstance> = Vec::with_capacity(placements.len());

    for p in placements {
        let Some(&unique_idx) = offset_to_idx.get(&p.geometry_offset) else {
            continue;
        };
        let Some(idx) = unique_to_dense[unique_idx] else {
            continue; // bad/empty record — skipped at bake time
        };

        // MZB coordinate convention — empirical, cross-checked against
        // `navmesh_overlay.rs:41-43`: xiNavmeshes (built from these MZBs
        // by FFXI-NavMesh-Builder) are stored in Detour-standard Y-up
        // coords, differing from Bevy only in z-handedness. That tells
        // us MZB-derived geometry is Y-up too, NOT Z-up like FFXI
        // server-side wire coords. So `p_swap` from the agent's first
        // pass (assumed FFXI Z-up) was over-rotating by 90°. Drop the
        // axis swap and apply only a z-flip for handedness.
        //
        // Matrix layout on disk: column-major. `m[0..4]` is column 0,
        // `m[12..15]` is the translation column.
        let m_native = Mat4::from_cols_array(&p.transform);
        // FFXI client native convention: Y-down (height grows toward
        // negative Y), Z forward. Bevy: Y-up, Z back. Transform is
        // therefore `Bevy = (x, -y, -z)`. Both MZB and the xiNavmesh
        // share this — flipping all three pipelines (entity wire here
        // via `ffxi_to_bevy`, MZB here, navmesh in `navmesh_overlay`)
        // keeps the scene self-consistent.
        let to_bevy = Mat4::from_cols(
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, -1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, -1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        );
        let m_bevy = to_bevy * m_native;
        // Apply the same Y-flip the matrix gets to the water height —
        // both live in MZB-native (Y-down) coords and need to land in
        // Bevy (Y-up) frame before rendering.
        let water_height_bevy = p.water_height.map(|h| -h);
        instances.push(MzbInstance {
            submesh_idx: idx,
            bevy_transform: Transform::from_matrix(m_bevy),
            water_height_bevy,
        });
    }

    Ok((submeshes, instances))
}

fn bake_submesh(m: &mzb::MzbMesh) -> MzbSubMesh {
    // Vertices stay in FFXI-local mesh space; the per-instance
    // Transform handles MZB matrix + ffxi_to_bevy together.
    let positions: Vec<[f32; 3]> = m.vertices.iter().map(|v| v.pos).collect();
    let indices: Vec<u32> = m
        .triangles
        .iter()
        .flat_map(|t| [t[0], t[1], t[2]])
        .collect();
    let tri_material: Vec<u8> = m.tri_info.iter().map(|t| t.material).collect();
    MzbSubMesh {
        positions,
        indices,
        tri_material,
        flags: m.flags,
    }
}

fn load_decrypted(
    file_id: u32,
    chunk_idx: Option<usize>,
) -> Result<(mzb::MzbHeader, Vec<u8>, ()), String> {
    let root =
        DatRoot::from_env_or_default().map_err(|e| format!("DatRoot::from_env_or_default: {e}"))?;
    let location = root
        .resolve(file_id)
        .map_err(|e| format!("resolve({file_id}): {e}"))?;
    let path = location.path_under(root.root());
    let bytes = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();

    let (idx, chunk) = match chunk_idx {
        Some(i) => (
            i,
            chunks
                .get(i)
                .ok_or_else(|| format!("chunk_idx {i} out of range ({} chunks)", chunks.len()))?,
        ),
        None => chunks
            .iter()
            .enumerate()
            .find(|(_, c)| c.kind == ChunkKind::Mzb as u8)
            .ok_or_else(|| {
                format!(
                    "no MZB (kind 0x1C) chunk in file_id {file_id} ({} chunks)",
                    chunks.len()
                )
            })?,
    };
    if chunk.kind != ChunkKind::Mzb as u8 {
        return Err(format!(
            "chunk[{idx}] kind=0x{:02X} ({:?}), not an MZB",
            chunk.kind,
            ChunkKind::label(chunk.kind),
        ));
    }

    let plain = mzb::decrypt(chunk.data).map_err(|e| format!("MZB decrypt: {e}"))?;
    let header = mzb::MzbHeader::parse(&plain).map_err(|e| format!("MZB header: {e}"))?;
    Ok((header, plain, ()))
}

/// One resolved zone-MMB placement: which MMB chunk to instance, and
/// a 4×4 Bevy-space transform. The transform is `to_bevy * M_ffxi`
/// where `M_ffxi` is built from the placement record's
/// trans/rot/scale (FFXI native, Y-down) — see
/// [`build_zone_mmb_spawns`] for the math. MMB local-space vertices
/// stay in FFXI-native coords; the entity transform alone does the
/// axis flip.
#[derive(Debug, Clone, Copy)]
pub struct ZoneMmbSpawn {
    pub chunk_idx: usize,
    pub bevy_transform: Mat4,
}

/// Resolve the zone's MMB-placement table (inside the MZB chunk body)
/// to concrete `(chunk_idx, transform)` entries ready to dispatch as
/// `LoadMmbRequest`s. Skips placements whose name doesn't resolve to an
/// MMB asset_name in the same DAT (zone "ground tile" MZB-internal
/// names like `d_x04z16` resolve via the zone-prefix rule).
pub fn build_zone_mmb_spawns(
    file_id: u32,
    chunk_idx: Option<usize>,
) -> Result<Vec<ZoneMmbSpawn>, String> {
    let root =
        DatRoot::from_env_or_default().map_err(|e| format!("DatRoot::from_env_or_default: {e}"))?;
    let location = root
        .resolve(file_id)
        .map_err(|e| format!("resolve({file_id}): {e}"))?;
    let path = location.path_under(root.root());
    let bytes = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();

    // Index every MMB chunk's asset_name → chunk_idx. Skip MMBs whose
    // header fails to parse (we already log this elsewhere).
    //
    // Parallel pre-pass: decrypt + parse each MMB header in parallel
    // (each is an independent ~kilobyte XOR + struct parse). City DATs
    // carry dozens-to-hundreds of MMB chunks; on the main thread this
    // is the second-largest portion of zone-load wall time after the
    // MZB itself.
    let pool = AsyncComputeTaskPool::get();
    let mmb_chunk_refs: Vec<(usize, &[u8])> = chunks
        .iter()
        .enumerate()
        .filter(|(_, c)| c.kind == ChunkKind::Mmb as u8)
        .map(|(idx, c)| (idx, c.data))
        .collect();
    let parsed: Vec<Option<(usize, String)>> = pool.scope(|s| {
        for (idx, data) in &mmb_chunk_refs {
            let idx = *idx;
            let data = *data;
            s.spawn(async move {
                let dec = mmb::decrypt(data).ok()?;
                let hdr = MmbHeader::parse(&dec).ok()?;
                Some((idx, hdr.asset_name_str().trim_end().to_string()))
            });
        }
    });
    let mut mmb_names: Vec<String> = Vec::with_capacity(parsed.len());
    let mut mmb_indices: Vec<usize> = Vec::with_capacity(parsed.len());
    for entry in parsed.into_iter().flatten() {
        mmb_indices.push(entry.0);
        mmb_names.push(entry.1);
    }
    let zone_prefix = mzb::infer_zone_prefix(&mmb_names);

    // Locate MZB chunk and parse the placement table.
    let (_, mzb_chunk) = match chunk_idx {
        Some(i) => (
            i,
            chunks
                .get(i)
                .ok_or_else(|| format!("chunk_idx {i} out of range ({} chunks)", chunks.len()))?,
        ),
        None => chunks
            .iter()
            .enumerate()
            .find(|(_, c)| c.kind == ChunkKind::Mzb as u8)
            .ok_or_else(|| {
                format!(
                    "no MZB chunk in file_id {file_id} ({} chunks)",
                    chunks.len()
                )
            })?,
    };
    let plain = mzb::decrypt(mzb_chunk.data).map_err(|e| format!("MZB decrypt: {e}"))?;
    let header = mzb::MzbHeader::parse(&plain).map_err(|e| format!("MZB header: {e}"))?;
    let placements = mzb::parse_mmb_placements(&plain, &header)
        .map_err(|e| format!("MZB parse_mmb_placements: {e}"))?;

    // Round-robin pairing for placement_id → chunk_idx (replaces the
    // historical singular `resolve_mmb_index` call that collapsed
    // every variant onto the first match).
    //
    // Some placement ids resolve to *multiple* MMB chunks — typically
    // because the placement name (e.g. `cube`, `water`, `saku`) matches
    // a family of variants in the chunk stream. Bastok Mines has
    // `cube` × 75 placements pointing at 2 distinct `cube` MMBs and
    // `water` × 24 pointing at 2 `water` MMBs. With the singular
    // resolver, all 75 cube placements bound to the first match and
    // the second variant never rendered (visible in-game as a missing
    // family of stair / pillar / fence pieces). The plural resolver
    // returns every match; we pair them round-robin by maintaining a
    // per-id counter that advances each time we consume a placement.
    // Policy: wrap modulo the match count when N placements exceed N
    // matches (FFXI authoring assumes "place N copies of variant"
    // wraps cleanly when N exceeds the variant set).
    //
    // Cited bug location for the singular collapse:
    // `ffxi-dat/src/mzb.rs:820-829` documents the pairing requirement;
    // this call site is what actually consumes the indices vec.
    use std::collections::HashMap;
    let mut rr_cursor: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::with_capacity(placements.len());
    for p in &placements {
        let name = p.id_str().trim_end_matches('\0');
        let trimmed = name.trim_end();
        let matches = mzb::resolve_mmb_indices(trimmed, &zone_prefix, &mmb_names);
        if matches.is_empty() {
            continue;
        }
        let cursor = rr_cursor.entry(trimmed.to_string()).or_insert(0);
        let local_idx = matches[*cursor % matches.len()];
        *cursor += 1;
        let chunk_idx = mmb_indices[local_idx];
        // Build the FFXI-native placement matrix `M_ffxi` from the
        // record's trans/rot/scale. Conventions per the reference: rot
        // is XYZ Euler radians in FFXI's Y-down frame. Apply scale
        // first, then rotate, then translate (S-R-T composition).
        // Euler order: XYZ (rotate-X, then Y, then Z). This matched
        // the runtime "Image #5" pass where bridge/walls rendered
        // correctly. YXZ was tried and made it worse, so we're sticking
        // with XYZ. Some MMBs remain visually wrong; the suspected
        // cause is not Euler order but per-MMB issues (clod-style
        // sub-records being mis-parsed as vertex data — task #18).
        let m_ffxi = Mat4::from_scale_rotation_translation(
            Vec3::new(p.scale[0], p.scale[1], p.scale[2]),
            Quat::from_euler(EulerRot::XYZ, p.rot[0], p.rot[1], p.rot[2]),
            Vec3::new(p.trans[0], p.trans[1], p.trans[2]),
        );
        // Same axis-flip we use for MZB merged instancing (Y-down →
        // Y-up with z-handedness flip). Vertex data inside the MMB
        // stays in FFXI-local coords; this matrix carries the entire
        // placement-then-flip so meshes render correctly oriented.
        let to_bevy = Mat4::from_cols(
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, -1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, -1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        );
        let bevy_transform = to_bevy * m_ffxi;
        out.push(ZoneMmbSpawn {
            chunk_idx,
            bevy_transform,
        });
    }

    // DIAG-zonegeom: remove after fix. Diagnostic gated on
    // `FFXI_DIAG_ZONE_GEOM=<file_id>` (e.g. `334` for Bastok Mines) or
    // `=all` / `=*` to dump every zone load. Surfaces enough state to
    // discriminate the three plausible causes of "MMBs in this zone
    // don't all spawn" from the plan
    // (~/.claude/plans/some-zones-still-have-composed-wind.md):
    //   (A) round-robin pairing — placement_count_per_id > 1 AND
    //       available_matches > 1 → singular resolver collapses siblings.
    //   (B) parser drops MMBs at decode — placements match 1:1 here but
    //       process_load_mmb_requests reports 0-submesh MMBs.
    //   (C) grid-cell MeshPlacement list — many placements report 0
    //       matches AND mmb_names is rich (placement ids live in a
    //       different table we don't iterate).
    let diag_enabled = match std::env::var("FFXI_DIAG_ZONE_GEOM") {
        Ok(s) if s == "*" || s == "all" || s.eq_ignore_ascii_case("any") => true,
        Ok(s) => s.parse::<u32>().ok() == Some(file_id),
        _ => false,
    };
    if diag_enabled {
        use std::collections::HashMap;

        // Duplicate analysis over the MMB chunk-stream asset names.
        let mut name_counts: HashMap<&str, u32> = HashMap::new();
        for n in &mmb_names {
            *name_counts.entry(n.trim_end()).or_insert(0) += 1;
        }
        let mut dup_names: Vec<(&str, u32)> = name_counts
            .iter()
            .filter(|(_, &c)| c > 1)
            .map(|(&n, &c)| (n, c))
            .collect();
        dup_names.sort_by(|a, b| b.1.cmp(&a.1));

        // Per-placement match-count buckets.
        let mut placement_id_counts: HashMap<String, u32> = HashMap::new();
        let mut bucket0: Vec<String> = Vec::new();
        let mut bucket1: u32 = 0;
        let mut bucket_many: Vec<(String, usize)> = Vec::new();
        for p in &placements {
            let id = p.id_str().trim_end_matches('\0').trim_end().to_string();
            *placement_id_counts.entry(id.clone()).or_insert(0) += 1;
            let matches = mzb::resolve_mmb_indices(&id, &zone_prefix, &mmb_names);
            match matches.len() {
                0 => bucket0.push(id),
                1 => bucket1 += 1,
                n => bucket_many.push((id, n)),
            }
        }

        // Round-robin smoke: ids with placement_count>1 AND available
        // matches>1. If non-empty, the singular resolver is leaving
        // siblings on the table.
        let mut roundrobin_smoke: Vec<(String, u32, usize)> = Vec::new();
        for (id, count) in &placement_id_counts {
            if *count < 2 {
                continue;
            }
            let m = mzb::resolve_mmb_indices(id, &zone_prefix, &mmb_names).len();
            if m > 1 {
                roundrobin_smoke.push((id.clone(), *count, m));
            }
        }
        roundrobin_smoke.sort_by(|a, b| b.1.cmp(&a.1));

        // Compact unmatched / ambiguous lists (dedup by id).
        let mut unmatched_unique: HashMap<String, u32> = HashMap::new();
        for id in &bucket0 {
            *unmatched_unique.entry(id.clone()).or_insert(0) += 1;
        }
        let mut um_list: Vec<(String, u32)> = unmatched_unique.into_iter().collect();
        um_list.sort_by(|a, b| b.1.cmp(&a.1));

        info!(
            target: "ffxi_viewer_core::dat_mzb::diag",
            file_id,
            placements = placements.len(),
            spawned = out.len(),
            mmb_names = mmb_names.len(),
            zone_prefix = %zone_prefix,
            dup_asset_names = dup_names.len(),
            match0 = bucket0.len(),
            match1 = bucket1,
            match_many = bucket_many.len(),
            roundrobin_smoke = roundrobin_smoke.len(),
            "DIAG-zonegeom summary",
        );
        if !dup_names.is_empty() {
            let head: Vec<&(&str, u32)> = dup_names.iter().take(20).collect();
            info!(
                target: "ffxi_viewer_core::dat_mzb::diag",
                "DIAG-zonegeom duplicate mmb asset_names (top 20): {head:?}",
            );
        }
        if !um_list.is_empty() {
            let head: Vec<&(String, u32)> = um_list.iter().take(20).collect();
            info!(
                target: "ffxi_viewer_core::dat_mzb::diag",
                "DIAG-zonegeom unmatched placement ids (id × count, top 20): {head:?}",
            );
        }
        if !roundrobin_smoke.is_empty() {
            let head: Vec<&(String, u32, usize)> = roundrobin_smoke.iter().take(20).collect();
            info!(
                target: "ffxi_viewer_core::dat_mzb::diag",
                "DIAG-zonegeom round-robin smoke (id, placement_count, matches, top 20): {head:?}",
            );
        }

        // Transform extents. If MMBs look "missing" while parse + texture
        // are healthy, the bug is almost always positional: scale shrinks
        // toward zero, or translations land far outside the navmesh AABB.
        // Decompose each `bevy_transform` via `to_scale_rotation_translation`.
        if !out.is_empty() {
            let mut tx_min = Vec3::splat(f32::INFINITY);
            let mut tx_max = Vec3::splat(f32::NEG_INFINITY);
            let mut sc_min = Vec3::splat(f32::INFINITY);
            let mut sc_max = Vec3::splat(f32::NEG_INFINITY);
            let mut tiny_scale: Vec<(usize, [f32; 3])> = Vec::new();
            let mut sample: Vec<(usize, [f32; 3], [f32; 3])> = Vec::new();
            for sp in out.iter() {
                let (scale, _rot, trans) = sp.bevy_transform.to_scale_rotation_translation();
                tx_min = tx_min.min(trans);
                tx_max = tx_max.max(trans);
                sc_min = sc_min.min(scale);
                sc_max = sc_max.max(scale);
                if scale.length() < 1e-3 {
                    tiny_scale.push((sp.chunk_idx, [scale.x, scale.y, scale.z]));
                }
                if sample.len() < 5 {
                    sample.push((
                        sp.chunk_idx,
                        [trans.x, trans.y, trans.z],
                        [scale.x, scale.y, scale.z],
                    ));
                }
            }
            info!(
                target: "ffxi_viewer_core::dat_mzb::diag",
                tx_min = ?[tx_min.x, tx_min.y, tx_min.z],
                tx_max = ?[tx_max.x, tx_max.y, tx_max.z],
                sc_min = ?[sc_min.x, sc_min.y, sc_min.z],
                sc_max = ?[sc_max.x, sc_max.y, sc_max.z],
                tiny_scale_n = tiny_scale.len(),
                "DIAG-zonegeom transform extents (Bevy frame)",
            );
            if !tiny_scale.is_empty() {
                let head: Vec<&(usize, [f32; 3])> = tiny_scale.iter().take(10).collect();
                info!(
                    target: "ffxi_viewer_core::dat_mzb::diag",
                    "DIAG-zonegeom tiny-scale spawns (chunk_idx, scale.xyz, top 10): {head:?}",
                );
            }
            info!(
                target: "ffxi_viewer_core::dat_mzb::diag",
                "DIAG-zonegeom sample spawns (chunk_idx, trans.xyz, scale.xyz, first 5): {sample:?}",
            );
        }
    }

    Ok(out)
}

/// Load + decrypt + parse all meshes in the first (or specified) MZB
/// chunk of `file_id`. Returns ready-to-bake submeshes.
///
/// Kept for backward compatibility with the pre-Phase-9b "everything
/// at origin" path; new code should call [`load_mzb_placed`].
pub fn load_mzb(file_id: u32, chunk_idx: Option<usize>) -> Result<Vec<MzbSubMesh>, String> {
    let root =
        DatRoot::from_env_or_default().map_err(|e| format!("DatRoot::from_env_or_default: {e}"))?;
    let location = root
        .resolve(file_id)
        .map_err(|e| format!("resolve({file_id}): {e}"))?;
    let path = location.path_under(root.root());
    let bytes = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();

    let (idx, chunk) = match chunk_idx {
        Some(i) => (
            i,
            chunks
                .get(i)
                .ok_or_else(|| format!("chunk_idx {i} out of range ({} chunks)", chunks.len()))?,
        ),
        None => chunks
            .iter()
            .enumerate()
            .find(|(_, c)| c.kind == ChunkKind::Mzb as u8)
            .ok_or_else(|| {
                format!(
                    "no MZB (kind 0x1C) chunk in file_id {file_id} ({} chunks)",
                    chunks.len()
                )
            })?,
    };
    if chunk.kind != ChunkKind::Mzb as u8 {
        return Err(format!(
            "chunk[{idx}] kind=0x{:02X} ({:?}), not an MZB",
            chunk.kind,
            ChunkKind::label(chunk.kind),
        ));
    }

    let (_header, meshes) =
        mzb::parse_all(chunk.data).map_err(|e| format!("MZB parse_all: {e}"))?;

    let mut out = Vec::with_capacity(meshes.len());
    for m in meshes {
        if m.vertices.is_empty() || m.triangles.is_empty() {
            continue;
        }
        let positions: Vec<[f32; 3]> = m.vertices.iter().map(|v| v.pos).collect();
        let indices: Vec<u32> = m
            .triangles
            .iter()
            .flat_map(|t| [t[0], t[1], t[2]])
            .collect();
        let tri_material: Vec<u8> = m.tri_info.iter().map(|t| t.material).collect();
        out.push(MzbSubMesh {
            positions,
            indices,
            tri_material,
            flags: m.flags,
        });
    }
    Ok(out)
}

/// Phase-A kicker: drain incoming [`LoadMzbRequest`] events, look the
/// requested `file_id` up in [`ZoneGeomCache`], and either feed the
/// cached geometry directly into the spawn step (cache hit path,
/// happens in the same frame the kick runs) or hand a fresh
/// `AsyncComputeTaskPool` task that runs [`load_mzb_placed`] +
/// [`build_zone_mmb_spawns`] off the main thread.
///
/// Multiple requests for the same `file_id` arriving in one frame are
/// coalesced onto a single task — the request list is kept so each
/// request's `world_pos` / `auto_loaded` flag still drives its own
/// spawn at poll time.
///
/// The actual file read + XOR decrypt + chunk-walk + mesh parse + bake
/// all run inside the spawned task (see [`load_mzb_placed`] for the
/// internal Phase-B parallelism). The only main-thread cost here is
/// the kick itself plus, on cache hit, the spawn step (which is the
/// `Assets::add` + `commands.spawn` work that has to stay on the main
/// thread).
pub fn kick_load_mzb_tasks(
    mut events: MessageReader<LoadMzbRequest>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    draw: Res<DrawDistance>,
    mut collision_geometry: ResMut<MzbCollisionGeometry>,
    mut load_mmb_tx: MessageWriter<crate::dat_mmb::LoadMmbRequest>,
    mut in_flight: ResMut<LoadMzbInFlight>,
    mut cache: ResMut<ZoneGeomCache>,
) {
    let init_vis = compute_init_visibility(draw.zone_geom_mode);
    for req in events.read() {
        // Cache hit: skip the background task entirely and spawn the
        // overlay this frame. The poll system never sees this request.
        if let Some(geom) = cache.get_and_promote(req.file_id) {
            spawn_mzb_overlay(
                *req,
                &geom,
                &mut commands,
                &mut meshes,
                &mut materials,
                &mut toasts,
                &mut collision_geometry,
                &mut load_mmb_tx,
                init_vis,
                /*from_cache*/ true,
            );
            continue;
        }
        // Already a task in flight for this file_id — append the
        // request so the poll fires once per original request even when
        // multiple arrive while the parse is still running. Saves a
        // duplicate parse for the rare case where the auto-load watcher
        // + a manual `/load_mzb` fire in quick succession.
        if let Some((reqs, _)) = in_flight.tasks.get_mut(&req.file_id) {
            reqs.push(*req);
            continue;
        }
        // Cache miss + no in-flight: spawn a background parse task.
        // Inside the task we run the bulk of zone-load work that's
        // currently main-thread-blocking: file I/O, XOR decrypt, chunk
        // walk, MZB parse, per-submesh bake (parallelized in turn),
        // and the zone-MMB placement table.
        let file_id = req.file_id;
        let chunk_idx = req.chunk_idx;
        let pool = AsyncComputeTaskPool::get();
        let task = pool.spawn(async move {
            let (submeshes, instances) = match load_mzb_placed(file_id, chunk_idx) {
                Ok(s) => s,
                Err(msg) => {
                    return LoadedZoneGeom {
                        submeshes: Arc::new(Vec::new()),
                        instances: Arc::new(Vec::new()),
                        mmb_spawns: Err(msg),
                    };
                }
            };
            let mmb_spawns = build_zone_mmb_spawns(file_id, chunk_idx);
            LoadedZoneGeom {
                submeshes: Arc::new(submeshes),
                instances: Arc::new(instances),
                mmb_spawns,
            }
        });
        in_flight.tasks.insert(file_id, (vec![*req], task));
    }
}

/// Phase-A poller: scan in-flight tasks for completed parses. Each
/// completed task gets fed into the spawn step once per coalesced
/// request and then inserted into [`ZoneGeomCache`] for the next
/// time the player zones back.
///
/// Polling is non-blocking: `future::poll_once` returns `None` when
/// the task is still running and `Some(result)` when it's ready.
/// We retain unfinished tasks across frames.
pub fn poll_load_mzb_tasks(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    draw: Res<DrawDistance>,
    mut collision_geometry: ResMut<MzbCollisionGeometry>,
    mut load_mmb_tx: MessageWriter<crate::dat_mmb::LoadMmbRequest>,
    mut in_flight: ResMut<LoadMzbInFlight>,
    mut cache: ResMut<ZoneGeomCache>,
) {
    let init_vis = compute_init_visibility(draw.zone_geom_mode);
    // Single pass: `poll_once` advances each in-flight task's futures
    // state machine. When it returns `Some(output)` the task is done
    // and we've consumed its `Output` — so we capture the value here
    // and drop the `Task` afterwards via the `retain` filter. The
    // previous "poll twice" shape silently hung because the second
    // poll always returns `None` (a yielded result isn't re-yielded).
    let mut completed: Vec<(u32, Vec<LoadMzbRequest>, LoadedZoneGeom)> = Vec::new();
    in_flight.tasks.retain(|file_id, (reqs, task)| {
        match future::block_on(future::poll_once(task)) {
            Some(geom) => {
                completed.push((*file_id, std::mem::take(reqs), geom));
                false
            }
            None => true,
        }
    });
    for (file_id, reqs, geom) in completed {
        // Cache before spawning so a same-frame second kick for the
        // same file_id (e.g. user mashing /load_mzb) hits the cache.
        // Skip caching for empty results — the spawn step will surface
        // the same toast each kick, and a stale empty cache entry would
        // hide a real DAT fix from the next attempt.
        let cache_eligible = !geom.submeshes.is_empty() && !geom.instances.is_empty();
        if cache_eligible {
            cache.insert(file_id, geom.clone());
        }
        for req in reqs {
            spawn_mzb_overlay(
                req,
                &geom,
                &mut commands,
                &mut meshes,
                &mut materials,
                &mut toasts,
                &mut collision_geometry,
                &mut load_mmb_tx,
                init_vis,
                /*from_cache*/ false,
            );
        }
    }
}

fn compute_init_visibility(mode: ZoneGeomMode) -> (Visibility, Visibility) {
    // Capture current zonegeom mode so freshly-spawned merged meshes
    // start at the correct visibility. `apply_zone_geom_visibility`
    // only fires on `draw.is_changed()`, so a zone-in with the mode
    // untouched would otherwise leave non-collision meshes visible
    // even when the mode is `Collision`.
    match mode {
        ZoneGeomMode::Off => (Visibility::Hidden, Visibility::Hidden),
        ZoneGeomMode::Collision | ZoneGeomMode::Camera => {
            (Visibility::Inherited, Visibility::Hidden)
        }
        ZoneGeomMode::All => (Visibility::Inherited, Visibility::Inherited),
    }
}

/// Spawn each MZB submesh as its own child entity under a parent
/// transform at `world_pos`. Collision and non-collision meshes are
/// distinct colors so the operator can see which geometry actually
/// participates in LoS / pathing.
///
/// MZB carries vertex positions only — no normals per vertex. We let
/// Bevy compute flat normals from positions for shading. Collision
/// (flags bit 0 cleared) and non-collision (flags bit 0 set) get
/// different palettes so they're visually distinguishable when
/// stacked at the same origin.
///
/// Main-thread only: every `Assets::add` and `commands.spawn` lives
/// here. The CPU-bound parse + bake ran in the kicker's background
/// task; we only touch the inputs through `&[MzbSubMesh]` and
/// `&[MzbInstance]` so a cache-shared `Arc<Vec<...>>` works without
/// cloning.
#[allow(clippy::too_many_arguments)]
fn spawn_mzb_overlay(
    req: LoadMzbRequest,
    geom: &LoadedZoneGeom,
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    toasts: &mut MessageWriter<crate::snapshot::ToastEvent>,
    collision_geometry: &mut ResMut<MzbCollisionGeometry>,
    load_mmb_tx: &mut MessageWriter<crate::dat_mmb::LoadMmbRequest>,
    init_vis: (Visibility, Visibility),
    _from_cache: bool,
) {
    let (init_collision_vis, init_noncollision_vis) = init_vis;
    let submeshes: &[MzbSubMesh] = geom.submeshes.as_slice();
    let instances: &[MzbInstance] = geom.instances.as_slice();
    if submeshes.is_empty() || instances.is_empty() {
        push_system_msg(
            toasts,
            format!(
                "/load_mzb {}: 0 renderable meshes ({} submeshes, {} instances)",
                req.file_id,
                submeshes.len(),
                instances.len(),
            ),
        );
        return;
    }

    let n_submeshes = submeshes.len();
    let n_instances = instances.len();

    // Two-sided. MZB walls are mostly single-sided polygons (the
    // FFXI client lit them per-face from outside only), so backface
    // culling makes interior surfaces invisible and produces
    // "missing geometry" gaps. `cull_mode: None` doubles fragment
    // cost but the per-zone draw count is two batched meshes, so
    // the tradeoff is comfortably affordable.
    //
    // `base_color: WHITE` so the per-vertex `ATTRIBUTE_COLOR`
    // (material_palette × normal-shade, baked above) carries the
    // visible tint. This replaces the old single-color teal /
    // translucent-amber appearance with material-distinguishable
    // walls (stone, sand, wood, water etc. each gets its own hue
    // from the 4-bit material_id decoded from the MZB triangle
    // index high bits). Bevy's StandardMaterial multiplies the
    // vertex color through unchanged, so the palette × shade bake
    // survives PBR shading.
    let collision_mat = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        cull_mode: None,
        ..default()
    });
    let noncollision_mat = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        cull_mode: None,
        ..default()
    });

    let mut parent_spawn = commands.spawn((
        crate::components::InGameEntity,
        MzbOverlay,
        Transform::from_translation(req.world_pos),
        Visibility::default(),
    ));
    if req.auto_loaded {
        parent_spawn.insert(AutoMzbOverlay);
    }
    let parent = parent_spawn.id();

    // Phase 3 perf — bake every placement into exactly two big
    // meshes (collision / non-collision) so the per-frame ECS cost
    // is O(2) instead of O(7000+ entities). Vertex positions are
    // pre-transformed by the placement matrix at load time; the
    // resulting buffer is in Bevy world space. Trade-off: lose
    // per-instance despawn; whole-zone refresh on zone change is
    // the only mutation, which is exactly what the auto-load does
    // already.
    let mut collision_positions: Vec<[f32; 3]> = Vec::new();
    let mut collision_indices: Vec<u32> = Vec::new();
    let mut collision_tri_mat: Vec<u8> = Vec::new();
    let mut noncollision_positions: Vec<[f32; 3]> = Vec::new();
    let mut noncollision_indices: Vec<u32> = Vec::new();
    let mut noncollision_tri_mat: Vec<u8> = Vec::new();

    for inst in instances.iter() {
        let sub = &submeshes[inst.submesh_idx];
        let is_collision = sub.flags & 1 == 0;
        let (positions, indices, tri_mat) = if is_collision {
            (
                &mut collision_positions,
                &mut collision_indices,
                &mut collision_tri_mat,
            )
        } else {
            (
                &mut noncollision_positions,
                &mut noncollision_indices,
                &mut noncollision_tri_mat,
            )
        };
        let base = positions.len() as u32;
        for v in &sub.positions {
            let p = inst
                .bevy_transform
                .transform_point(Vec3::new(v[0], v[1], v[2]));
            positions.push([p.x, p.y, p.z]);
        }
        for &i in &sub.indices {
            indices.push(i + base);
        }
        tri_mat.extend_from_slice(&sub.tri_material);
    }

    // Spawn one child per non-empty merged mesh — usually both,
    // sometimes only collision (zones without decorative walls).
    // `is_collision` controls which sub-marker is attached so
    // `/zonegeom` can toggle the two channels independently.
    //
    // Per-vertex material tint: each vertex carries the 4-bit
    // `material` id (0..15) from the MZB triangle index high bits
    // (decoded in `ffxi_dat::mzb::parse_one_mesh`). We bake one of
    // 16 palette colors into the vertex color × `shade` from the
    // computed normal. Shared vertices that span material
    // boundaries get the LAST triangle's material — minor seam
    // bleed but acceptable for a coarse-mesh proxy.
    //
    // The 16-color palette is HSV-distributed with a fixed
    // saturation/value: distinct enough that operator-readable
    // walls, floors, stairs, sand etc. don't all blur together,
    // muted enough that the scene reads as solid (not rainbow).
    let spawn_merged = |commands: &mut Commands,
                        positions: Vec<[f32; 3]>,
                        indices: Vec<u32>,
                        tri_mat: Vec<u8>,
                        material: Handle<StandardMaterial>,
                        parent: bevy::ecs::entity::Entity,
                        auto_loaded: bool,
                        is_collision: bool,
                        init_vis: Visibility,
                        meshes: &mut ResMut<Assets<Mesh>>| {
        if positions.is_empty() || indices.is_empty() {
            return;
        }
        // Project per-triangle material id onto per-vertex slots.
        // Last-write-wins at shared vertices.
        let mut vert_mat: Vec<u8> = vec![0u8; positions.len()];
        for (tri_idx, tri) in indices.chunks_exact(3).enumerate() {
            let m = tri_mat.get(tri_idx).copied().unwrap_or(0);
            vert_mat[tri[0] as usize] = m;
            vert_mat[tri[1] as usize] = m;
            vert_mat[tri[2] as usize] = m;
        }
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_indices(Indices::U32(indices));
        // Smooth normals: indexed meshes share vertices and
        // can't host per-face normals; smooth shading reads
        // fine on the coarse MZB walls.
        mesh.compute_smooth_normals();
        // Vertex color = material_palette[mat] × shade(n.y).
        // StandardMaterial multiplies vertex color × base_color,
        // so this gives us a per-vertex shading + material tint
        // bake on top of whatever PBR shading the scene lights add.
        if let Some(normals) = mesh
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .and_then(|a| a.as_float3())
        {
            let colors: Vec<[f32; 4]> = normals
                .iter()
                .zip(vert_mat.iter())
                .map(|(n, &m)| {
                    // n.y in [-1, +1]. Up-facing → shade=1.0,
                    // down-facing → shade=0.4. Linear lerp.
                    let shade = 0.4 + 0.6 * (n[1] * 0.5 + 0.5);
                    let pal = MZB_MATERIAL_PALETTE[(m & 0x0F) as usize];
                    [pal[0] * shade, pal[1] * shade, pal[2] * shade, 1.0]
                })
                .collect();
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        }
        let mut child = commands.spawn((
            MzbOverlay,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            init_vis,
            ChildOf(parent),
        ));
        if is_collision {
            child.insert(MzbCollisionMesh);
        } else {
            child.insert(MzbNonCollisionMesh);
        }
        if auto_loaded {
            child.insert(AutoMzbOverlay);
        }
    };
    // Capture merge stats before we move the buffers into spawn_merged.
    let collision_verts = collision_positions.len();
    let collision_tris = collision_indices.len() / 3;
    let noncollision_verts = noncollision_positions.len();
    let noncollision_tris = noncollision_indices.len() / 3;

    // Stash the collision geometry in a CPU-side resource so the
    // ground-snap and `/debug heights` paths can do per-tick
    // raycasts without walking the Bevy `Assets<Mesh>` storage.
    // This replaces zone-N's geometry wholesale — on a zone change
    // the new `LoadMzbRequest` lands here and overwrites.
    collision_geometry.positions = collision_positions
        .iter()
        .map(|p| Vec3::new(p[0], p[1], p[2]))
        .collect();
    collision_geometry.indices = collision_indices.clone();
    // XZ grid index for O(cell) raycasts. The snap fires per-frame
    // for every entity in the zone, so the prior O(tris) linear
    // scan tanked FPS in populated outdoor zones — see the
    // historical comment in `snap_entities_to_navmesh_system`.
    collision_geometry.cell_index =
        build_cell_index(&collision_geometry.positions, &collision_geometry.indices);

    spawn_merged(
        commands,
        collision_positions,
        collision_indices,
        collision_tri_mat,
        collision_mat,
        parent,
        req.auto_loaded,
        true,
        init_collision_vis,
        meshes,
    );
    spawn_merged(
        commands,
        noncollision_positions,
        noncollision_indices,
        noncollision_tri_mat,
        noncollision_mat,
        parent,
        req.auto_loaded,
        false,
        init_noncollision_vis,
        meshes,
    );

    let total_verts = collision_verts + noncollision_verts;
    let total_tris = collision_tris + noncollision_tris;
    push_system_msg(
            toasts,
            format!(
                "/load_mzb {}: {n_submeshes} submeshes, {n_instances} placements → merged {total_verts} verts / {total_tris} tris ({collision_verts}v {collision_tris}t collision, {noncollision_verts}v {noncollision_tris}t non-collision)",
                req.file_id,
            ),
        );

    // Water planes: group placements with `water_height_bevy` set
    // by height (rounded to the nearest mm so float-noise duplicates
    // don't fragment a single lake into N near-identical groups),
    // then spawn one flat alpha quad per group sized to the union
    // of the placements' geometry XZ extents.
    //
    // Lotus's pipeline animates a 30-frame water texture; we ship a
    // static teal-blue alpha quad instead — the per-placement extent
    // is what fixes "lakes render as collision boxes underwater."
    // Texture animation lands when the particle/animated-texture
    // pipeline does.
    let mut water_groups: std::collections::HashMap<i32, (Vec3, Vec3)> =
        std::collections::HashMap::new();
    for inst in instances.iter() {
        let Some(h_bevy) = inst.water_height_bevy else {
            continue;
        };
        let sub = &submeshes[inst.submesh_idx];
        if sub.positions.is_empty() {
            continue;
        }
        // World-space XZ bbox for this placement's geometry.
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for v in &sub.positions {
            let p = inst
                .bevy_transform
                .transform_point(Vec3::new(v[0], v[1], v[2]));
            min = min.min(p);
            max = max.max(p);
        }
        // Key by mm-rounded height — float noise on the parser
        // side would otherwise split one lake into 10 near-equal
        // groups.
        let key = (h_bevy * 1000.0).round() as i32;
        water_groups
            .entry(key)
            .and_modify(|(mn, mx)| {
                *mn = mn.min(min);
                *mx = mx.max(max);
            })
            .or_insert((min, max));
    }
    let water_count = water_groups.len();
    if water_count > 0 {
        let water_mat = materials.add(StandardMaterial {
            base_color: Color::srgba(0.20, 0.45, 0.70, 0.55),
            cull_mode: None,
            alpha_mode: AlphaMode::Blend,
            ..default()
        });
        for (height_key, (min, max)) in water_groups {
            let h = height_key as f32 / 1000.0;
            let dx = (max.x - min.x).max(0.01);
            let dz = (max.z - min.z).max(0.01);
            let cx = 0.5 * (min.x + max.x);
            let cz = 0.5 * (min.z + max.z);
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            );
            // Flat quad in the XZ plane, centered on (cx, h, cz).
            let hx = dx * 0.5;
            let hz = dz * 0.5;
            let positions: Vec<[f32; 3]> = vec![
                [cx - hx, h, cz - hz],
                [cx + hx, h, cz - hz],
                [cx + hx, h, cz + hz],
                [cx - hx, h, cz + hz],
            ];
            let normals: Vec<[f32; 3]> = vec![[0.0, 1.0, 0.0]; 4];
            let uvs: Vec<[f32; 2]> = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
            let indices: Vec<u32> = vec![0, 1, 2, 0, 2, 3];
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
            mesh.insert_indices(Indices::U32(indices));
            let mut child = commands.spawn((
                MzbOverlay,
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(water_mat.clone()),
                Transform::IDENTITY,
                init_noncollision_vis,
                ChildOf(parent),
                MzbNonCollisionMesh,
            ));
            if req.auto_loaded {
                child.insert(AutoMzbOverlay);
            }
        }
        push_system_msg(
            toasts,
            format!(
                "/load_mzb {}: {} water plane{} spawned",
                req.file_id,
                water_count,
                if water_count == 1 { "" } else { "s" },
            ),
        );
    }

    // Also instance the zone's visual MMBs at their MZB-placement
    // transforms. This is the "textured visual world" half of the
    // zone — MZB merged meshes above are collision-only, MMBs are
    // the per-prop textured walls/buildings/floor tiles.
    //
    // The spawn-table parse already ran inside the background task
    // (see `kick_load_mzb_tasks` → `LoadedZoneGeom.mmb_spawns`);
    // here we just consume the `Result` and fire the per-placement
    // events. On a cache hit the same `Arc<Result<...>>` is reused
    // — no re-parse cost on zone re-entry.
    match &geom.mmb_spawns {
        Ok(spawns) => {
            let n = spawns.len();
            let offset = Mat4::from_translation(req.world_pos);
            for s in spawns {
                load_mmb_tx.write(crate::dat_mmb::LoadMmbRequest {
                    file_id: req.file_id,
                    chunk_idx: s.chunk_idx,
                    world_pos: Vec3::ZERO,
                    entity_id: None,
                    world_transform: Some(offset * s.bevy_transform),
                });
            }
            push_system_msg(
                toasts,
                format!(
                    "/load_mzb {}: queued {n} visual MMB placements",
                    req.file_id
                ),
            );
        }
        Err(msg) => {
            push_system_msg(
                toasts,
                format!("/load_mzb {}: zone-MMB spawn: {msg}", req.file_id),
            );
        }
    }
}

/// Last zone_id the auto-load watcher fired for. `None` until the
/// player first zones in. Tracked separately from the snapshot so we
/// can detect transitions without depending on Bevy's `Res<...>`
/// change-detection (which would fire on every snapshot replacement
/// regardless of whether zone_id actually changed).
#[derive(Resource, Default)]
pub struct LastAutoLoadedZone {
    pub zone_id: Option<u16>,
}

/// Watch [`SceneState::snapshot::zone_id`] for changes. On every
/// transition (None → Some, Some(A) → Some(B), Some(A) → None):
///   1. Despawn every entity tagged [`AutoMzbOverlay`] (preserving the
///      operator's manual `/load_mzb` loads).
///   2. If the new zone has a known DAT file_id mapping, fire a
///      [`LoadMzbRequest`] at FFXI world origin with
///      `auto_loaded: true`.
///
/// Zones without a known mapping fall through quietly — the previous
/// zone's auto-load is still despawned (so we don't leave stale
/// geometry from zone A floating in zone B), but no new request is
/// fired. The chat HUD gets a one-line note so the operator can tell
/// the difference between "mapping missing" and "auto-load broken".
pub fn auto_load_zone_geometry_system(
    scene_state: Res<SceneState>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    mut last: ResMut<LastAutoLoadedZone>,
    mut commands: Commands,
    mut load_tx: MessageWriter<LoadMzbRequest>,
    auto_q: Query<Entity, With<AutoMzbOverlay>>,
) {
    let current = scene_state.snapshot.zone_id;
    if current == last.zone_id {
        return;
    }
    // Transition detected — despawn previous auto-load even if we
    // don't end up firing a new one (covers the Some(A) → None
    // "logout / charselect" case).
    for e in auto_q.iter() {
        commands.entity(e).despawn();
    }
    last.zone_id = current;
    let Some(zone_id) = current else { return };

    match ffxi_dat::zone_dat::zone_id_to_mzb_file_id(zone_id) {
        Some(file_id) => {
            // FFXI world origin = Bevy origin: `ffxi_to_bevy(0,0,0)`
            // = `Vec3::ZERO`. MZB vertex data is already in zone-local
            // space (which IS the zone's coordinate frame).
            load_tx.write(LoadMzbRequest {
                file_id,
                chunk_idx: None,
                world_pos: Vec3::ZERO,
                auto_loaded: true,
            });
            // Distinguish auto-load from manual `/load_mzb` in chat.
            push_system_msg(
                &mut toasts,
                format!("auto-load: zone {zone_id} -> DAT file {file_id}"),
            );
        }
        None => {
            push_system_msg(
                &mut toasts,
                format!("auto-load: no DAT mapping for zone {zone_id} (Phase 11b table pending)"),
            );
        }
    }
}

/// Distance-LOD culling for MZB overlay entities (Phase #1 of the
/// three-pass MZB perf plan). Hides any `MzbOverlay` entity whose
/// translation is more than [`MZB_CULL_DISTANCE`] yalms (squared
/// distance for cheap comparison) from the player's world transform.
///
/// Falls through quietly if no `IsSelf` entity is present (e.g. before
/// the first `EntityUpserted` for self).  We use horizontal distance
/// only — vertical offsets in multi-story zones shouldn't make a
/// building "disappear" because it's a few yalms above the camera.
pub fn cull_mzb_by_distance(
    draw: Res<DrawDistance>,
    self_q: Query<&GlobalTransform, With<IsSelf>>,
    // Filter on `Mesh3d` so we only touch the *child* placement
    // entities, not the zone-wide parent. The parent lives at the
    // FFXI world origin (often hundreds of yalms from the player),
    // so without this filter the parent ends up Hidden and every
    // child inherits the same — entire zone disappears.
    mut mzb_q: Query<(&GlobalTransform, &mut Visibility), (With<MzbOverlay>, With<Mesh3d>)>,
) {
    let Ok(self_t) = self_q.single() else {
        return;
    };
    let self_pos = self_t.translation();
    let cull_sq = draw.world * draw.world;

    for (mzb_t, mut vis) in mzb_q.iter_mut() {
        let mzb_pos = mzb_t.translation();
        // Horizontal-only distance — multi-story zones shouldn't drop
        // a building because the player is climbing stairs above it.
        let dx = mzb_pos.x - self_pos.x;
        let dz = mzb_pos.z - self_pos.z;
        let d_sq = dx * dx + dz * dz;
        let want = if d_sq > cull_sq {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        // Skip the write if visibility already matches — Bevy's change
        // detection is per-field, and a no-op write would still tick
        // mutation flags and force the renderer to re-extract.
        if *vis != want {
            *vis = want;
        }
    }
}

/// Distance-cull non-PC entities (mobs, NPCs, pets, other) beyond
/// `DrawDistance.mob` yalms from the player. PCs are never culled —
/// party members, raid mates, and other PCs in the zone stay visible
/// so the operator can target them regardless of camera distance.
///
/// Horizontal-only distance (same rationale as the now-removed Phase 1
/// MZB cull): multi-story zones shouldn't drop a mob because the
/// camera is on a different floor.
pub fn cull_entities_by_distance(
    draw: Res<DrawDistance>,
    self_q: Query<&GlobalTransform, With<IsSelf>>,
    mut ent_q: Query<(&WorldEntity, &GlobalTransform, &mut Visibility), Without<IsSelf>>,
) {
    let Ok(self_t) = self_q.single() else {
        return;
    };
    let self_pos = self_t.translation();
    let cull_sq = draw.mob * draw.mob;

    for (ent, ent_t, mut vis) in ent_q.iter_mut() {
        // Always-visible PCs — covers party members, alliance, random
        // bystanders. Same convention as Ashita's drawdistance addon.
        if matches!(ent.kind, EntityKind::Pc) {
            if *vis != Visibility::Inherited {
                *vis = Visibility::Inherited;
            }
            continue;
        }
        let p = ent_t.translation();
        let dx = p.x - self_pos.x;
        let dz = p.z - self_pos.z;
        let want = if dx * dx + dz * dz > cull_sq {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        if *vis != want {
            *vis = want;
        }
    }
}

fn push_system_msg(toasts: &mut MessageWriter<crate::snapshot::ToastEvent>, text: String) {
    toasts.write(crate::snapshot::ToastEvent::debug(text));
}
