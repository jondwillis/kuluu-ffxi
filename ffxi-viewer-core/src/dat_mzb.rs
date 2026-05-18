//! MZB debug overlay: load a real FFXI zone mesh-library from a DAT
//! file and spawn every mesh in the library at a chosen world position.
//!
//! Pattern mirrors [`crate::dat_mmb`] — slash-command dispatcher fires
//! [`LoadMzbRequest`], [`process_load_mzb_requests`] consumes it. The
//! mesh-library geometry sits near the origin in MZB's own coordinate
//! frame; spatial-index decode (grid/quadtree/placement transforms)
//! lives in Phase 9b. Without those, the overlay shows mesh data as
//! though every mesh is at the library origin — useful for visual
//! validation of the parser even before placement is wired.
//!
//! Native-only for the same reason as `dat_mmb.rs`: `ffxi-dat::DatRoot`
//! does sync `fs::read` of the user's local install.

#![cfg(not(target_arch = "wasm32"))]

use std::fs;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
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

/// Runtime-tunable cull distances. World controls MZB overlay
/// entities, mob controls non-PC entity capsules (mobs/NPCs/pets).
/// PCs are never culled by distance — party members and other PCs
/// stay visible regardless so the operator can still target them.
/// `/zonegeom` tri-state. `Off` is the default once MMB placements
/// render the textured visual world — the MZB collision mesh overlaps
/// MMB walls geometrically and z-fights, so we hide it. Operators can
/// `/zonegeom collision` to see the LoS-blocking proxy when debugging
/// nav/path issues, or `/zonegeom all` for both layers.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneGeomMode {
    /// Hide all MZB geometry. Default — MMBs supply the visual layer.
    #[default]
    Off,
    /// Show only LoS-blocking (collision) meshes — flag bit 0 == 0.
    Collision,
    /// Show both collision and non-collision (decorative) meshes.
    All,
}

impl ZoneGeomMode {
    /// `toggle`-cycle order: Collision → All → Off → Collision.
    /// Skips no states; lets a single keybind walk the full tri-state.
    pub fn cycle(self) -> Self {
        match self {
            Self::Collision => Self::All,
            Self::All => Self::Off,
            Self::Off => Self::Collision,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Collision => "collision",
            Self::All => "all",
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
}

impl MzbCollisionGeometry {
    /// Number of triangles backing this geometry.
    pub fn tri_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Cast a ray straight down (Bevy −Y) at (`xz.x`, `xz.y`) from a
    /// y high above any plausible zone height, and return the **highest**
    /// Y of any triangle the ray hits **at or below `ceiling_y`**.
    /// `None` if the column at `xz` has no qualifying triangle.
    ///
    /// `ceiling_y` filters out overhead geometry — arches, gate tops,
    /// eaves, bridges the player is walking *under*. Without it, "highest
    /// hit" warps the player onto the gate roof in South San d'Oria
    /// every time they pass through the entrance.
    ///
    /// Of the surfaces below the ceiling, we still pick the highest:
    /// that's the multi-level-building case (player on 2nd floor — snap
    /// to the 2nd floor, not the basement).
    ///
    /// Caller policy: pass `player.y + step_tolerance` as the ceiling
    /// so small step-ups (curbs, ramps) still work. The
    /// `snap_entities_to_navmesh_system` caller uses 2 yalms — bigger
    /// than any real step, smaller than any real arch.
    pub fn ground_raycast(&self, xz: Vec2, ceiling_y: f32) -> Option<f32> {
        const RAY_ORIGIN_Y: f32 = 1000.0;
        let orig = Vec3::new(xz.x, RAY_ORIGIN_Y, xz.y);
        let dir = Vec3::new(0.0, -1.0, 0.0);
        let mut best_y: Option<f32> = None;
        for tri in self.indices.chunks_exact(3) {
            let v0 = self.positions[tri[0] as usize];
            let v1 = self.positions[tri[1] as usize];
            let v2 = self.positions[tri[2] as usize];
            if let Some(t) = ray_tri_intersect(orig, dir, v0, v1, v2) {
                let hit_y = orig.y + t * dir.y;
                if hit_y > ceiling_y {
                    // Overhead geometry (gate top, arch, eave). Don't
                    // let it pull the player onto its roof.
                    continue;
                }
                best_y = Some(match best_y {
                    Some(prev) if prev > hit_y => prev,
                    _ => hit_y,
                });
            }
        }
        best_y
    }
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
        // origin (old behavior).
        let meshes =
            mzb::parse_meshes(&plain, &header).map_err(|e| format!("MZB parse_meshes: {e}"))?;
        let mut submeshes = Vec::new();
        let mut instances = Vec::new();
        for m in meshes {
            if m.vertices.is_empty() || m.triangles.is_empty() {
                continue;
            }
            let sub = bake_submesh(&m);
            let idx = submeshes.len();
            submeshes.push(sub);
            instances.push(MzbInstance {
                submesh_idx: idx,
                bevy_transform: Transform::IDENTITY,
            });
        }
        return Ok((submeshes, instances));
    }

    // Dedupe by geometry_offset.
    let mut offset_to_idx: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    let mut submeshes: Vec<MzbSubMesh> = Vec::new();
    let mut instances: Vec<MzbInstance> = Vec::new();

    for p in placements {
        let idx = if let Some(&i) = offset_to_idx.get(&p.geometry_offset) {
            i
        } else {
            let m = match mzb::parse_mesh_at(&plain, p.geometry_offset as usize) {
                Ok(m) => m,
                Err(_) => continue, // skip bad records — same posture as MMB
            };
            if m.vertices.is_empty() || m.triangles.is_empty() {
                continue;
            }
            let i = submeshes.len();
            submeshes.push(bake_submesh(&m));
            offset_to_idx.insert(p.geometry_offset, i);
            i
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
        instances.push(MzbInstance {
            submesh_idx: idx,
            bevy_transform: Transform::from_matrix(m_bevy),
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
    MzbSubMesh {
        positions,
        indices,
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
    let mut mmb_names: Vec<String> = Vec::new();
    let mut mmb_indices: Vec<usize> = Vec::new();
    for (idx, c) in chunks.iter().enumerate() {
        if c.kind != ChunkKind::Mmb as u8 {
            continue;
        }
        let dec = match mmb::decrypt(c.data) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let hdr = match MmbHeader::parse(&dec) {
            Ok(h) => h,
            Err(_) => continue,
        };
        mmb_names.push(hdr.asset_name_str().trim_end().to_string());
        mmb_indices.push(idx);
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

    let mut out = Vec::with_capacity(placements.len());
    for p in &placements {
        let name = p.id_str().trim_end_matches('\0');
        let trimmed = name.trim_end();
        let Some(local_idx) = mzb::resolve_mmb_index(trimmed, &zone_prefix, &mmb_names) else {
            continue;
        };
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
        out.push(MzbSubMesh {
            positions,
            indices,
            flags: m.flags,
        });
    }
    Ok(out)
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
pub fn process_load_mzb_requests(
    mut events: MessageReader<LoadMzbRequest>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut scene_state: ResMut<SceneState>,
    draw: Res<DrawDistance>,
    mut collision_geometry: ResMut<MzbCollisionGeometry>,
    mut load_mmb_tx: MessageWriter<crate::dat_mmb::LoadMmbRequest>,
) {
    // Capture current zonegeom mode so freshly-spawned merged meshes
    // start at the correct visibility. `apply_zone_geom_visibility`
    // only fires on `draw.is_changed()`, so a zone-in with the mode
    // untouched would otherwise leave non-collision meshes visible
    // even when the mode is `Collision`.
    let (init_collision_vis, init_noncollision_vis) = match draw.zone_geom_mode {
        ZoneGeomMode::Off => (Visibility::Hidden, Visibility::Hidden),
        ZoneGeomMode::Collision => (Visibility::Inherited, Visibility::Hidden),
        ZoneGeomMode::All => (Visibility::Inherited, Visibility::Inherited),
    };
    for req in events.read() {
        let (submeshes, instances) = match load_mzb_placed(req.file_id, req.chunk_idx) {
            Ok(s) => s,
            Err(msg) => {
                push_system_msg(
                    &mut scene_state,
                    format!("/load_mzb {}: {msg}", req.file_id),
                );
                continue;
            }
        };
        if submeshes.is_empty() || instances.is_empty() {
            push_system_msg(
                &mut scene_state,
                format!(
                    "/load_mzb {}: 0 renderable meshes ({} submeshes, {} instances)",
                    req.file_id,
                    submeshes.len(),
                    instances.len(),
                ),
            );
            continue;
        }

        let n_submeshes = submeshes.len();
        let n_instances = instances.len();

        // Unlit + two-sided. MZB walls are mostly single-sided polygons
        // (the FFXI client originally lit them per-face from outside
        // only), so backface culling makes interior surfaces invisible
        // and produces visible "missing geometry" gaps. `cull_mode: None`
        // doubles fragment cost but at the post-unlit perf baseline that
        // tradeoff is comfortably affordable.
        let collision_mat = materials.add(StandardMaterial {
            base_color: Color::srgb(0.30, 0.65, 0.65),
            unlit: true,
            cull_mode: None,
            ..default()
        });
        let noncollision_mat = materials.add(StandardMaterial {
            // Translucent amber: visual-only walls (LoS does NOT block).
            // The earlier opaque switch was a perf experiment from when
            // we were CPU-bound; now that we're GPU-comfortable at ~100
            // FPS, the see-through alpha is worth its sorted-pass cost
            // because it lets the operator see *through* visual walls
            // when the camera ends up inside a building.
            base_color: Color::srgba(0.75, 0.55, 0.30, 0.45),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
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
        let mut noncollision_positions: Vec<[f32; 3]> = Vec::new();
        let mut noncollision_indices: Vec<u32> = Vec::new();

        for inst in &instances {
            let sub = &submeshes[inst.submesh_idx];
            let is_collision = sub.flags & 1 == 0;
            let (positions, indices) = if is_collision {
                (&mut collision_positions, &mut collision_indices)
            } else {
                (&mut noncollision_positions, &mut noncollision_indices)
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
        }

        // Spawn one child per non-empty merged mesh — usually both,
        // sometimes only collision (zones without decorative walls).
        // `is_collision` controls which sub-marker is attached so
        // `/zonegeom` can toggle the two channels independently.
        let spawn_merged = |commands: &mut Commands,
                            positions: Vec<[f32; 3]>,
                            indices: Vec<u32>,
                            material: Handle<StandardMaterial>,
                            parent: bevy::ecs::entity::Entity,
                            auto_loaded: bool,
                            is_collision: bool,
                            init_vis: Visibility,
                            meshes: &mut ResMut<Assets<Mesh>>| {
            if positions.is_empty() || indices.is_empty() {
                return;
            }
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            );
            let n_positions = positions.len();
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_indices(Indices::U32(indices));
            // Smooth normals: indexed meshes share vertices and
            // can't host per-face normals; smooth shading reads
            // fine on the coarse MZB walls.
            mesh.compute_smooth_normals();
            // Bake fake "sun lighting" into vertex colors using the
            // computed normal's y component: faces pointing up =
            // bright, faces pointing down = dim. The unlit material
            // multiplies vertex color × base_color, so this gives us
            // free shading at zero per-pixel cost. Cheap proxy for
            // PBR lighting that runs at vertex-shader scale (`O(N)`
            // baked-once, not `O(fragments)` per frame).
            if let Some(normals) = mesh
                .attribute(Mesh::ATTRIBUTE_NORMAL)
                .and_then(|a| a.as_float3())
            {
                let colors: Vec<[f32; 4]> = normals
                    .iter()
                    .map(|n| {
                        // n.y in [-1, +1]. Up-facing → shade=1.0,
                        // down-facing → shade=0.4. Linear lerp.
                        let shade = 0.4 + 0.6 * (n[1] * 0.5 + 0.5);
                        [shade, shade, shade, 1.0]
                    })
                    .collect();
                mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
            }
            let _ = n_positions;
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

        spawn_merged(
            &mut commands,
            collision_positions,
            collision_indices,
            collision_mat,
            parent,
            req.auto_loaded,
            true,
            init_collision_vis,
            &mut meshes,
        );
        spawn_merged(
            &mut commands,
            noncollision_positions,
            noncollision_indices,
            noncollision_mat,
            parent,
            req.auto_loaded,
            false,
            init_noncollision_vis,
            &mut meshes,
        );

        let total_verts = collision_verts + noncollision_verts;
        let total_tris = collision_tris + noncollision_tris;
        push_system_msg(
            &mut scene_state,
            format!(
                "/load_mzb {}: {n_submeshes} submeshes, {n_instances} placements → merged {total_verts} verts / {total_tris} tris ({collision_verts}v {collision_tris}t collision, {noncollision_verts}v {noncollision_tris}t non-collision)",
                req.file_id,
            ),
        );

        // Also instance the zone's visual MMBs at their MZB-placement
        // transforms. This is the "textured visual world" half of the
        // zone — MZB merged meshes above are collision-only, MMBs are
        // the per-prop textured walls/buildings/floor tiles.
        match build_zone_mmb_spawns(req.file_id, req.chunk_idx) {
            Ok(spawns) => {
                let n = spawns.len();
                let offset = Mat4::from_translation(req.world_pos);
                for s in &spawns {
                    load_mmb_tx.write(crate::dat_mmb::LoadMmbRequest {
                        file_id: req.file_id,
                        chunk_idx: s.chunk_idx,
                        world_pos: Vec3::ZERO,
                        entity_id: None,
                        world_transform: Some(offset * s.bevy_transform),
                    });
                }
                push_system_msg(
                    &mut scene_state,
                    format!(
                        "/load_mzb {}: queued {n} visual MMB placements",
                        req.file_id
                    ),
                );
            }
            Err(msg) => {
                push_system_msg(
                    &mut scene_state,
                    format!("/load_mzb {}: zone-MMB spawn: {msg}", req.file_id),
                );
            }
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
    mut scene_state: ResMut<SceneState>,
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
                &mut scene_state,
                format!("auto-load: zone {zone_id} -> DAT file {file_id}"),
            );
        }
        None => {
            push_system_msg(
                &mut scene_state,
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

fn push_system_msg(scene_state: &mut SceneState, text: String) {
    use ffxi_viewer_wire::{ChatChannel, ChatLine};
    scene_state.push_local_toast(ChatLine {
        channel: ChatChannel::Debug,
        sender: "client".into(),
        text,
        server_ts: 0,
        local_seq: 0,
    });
}
