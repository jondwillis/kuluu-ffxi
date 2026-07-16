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

pub const DEFAULT_WORLD_DRAW_DISTANCE: f32 = 80.0;
pub const DEFAULT_MOB_DRAW_DISTANCE: f32 = 50.0;

pub const MMB_LOAD_DISTANCE_MARGIN: f32 = 1.25;

const MZB_MATERIAL_PALETTE: [[f32; 3]; 16] = [
    [0.85, 0.55, 0.40],
    [0.75, 0.65, 0.45],
    [0.50, 0.65, 0.55],
    [0.55, 0.70, 0.75],
    [0.65, 0.55, 0.75],
    [0.80, 0.65, 0.55],
    [0.65, 0.60, 0.50],
    [0.55, 0.55, 0.60],
    [0.70, 0.50, 0.45],
    [0.45, 0.55, 0.50],
    [0.60, 0.70, 0.40],
    [0.50, 0.45, 0.40],
    [0.75, 0.70, 0.50],
    [0.55, 0.60, 0.65],
    [0.45, 0.50, 0.55],
    [0.65, 0.65, 0.55],
];

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneGeomMode {
    #[default]
    Off,

    Collision,

    All,

    Camera,
}

impl ZoneGeomMode {
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

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraCollisionSource {
    #[default]
    Mzb,

    Mmb,

    Both,
}

impl CameraCollisionSource {
    pub fn cycle(self) -> Self {
        match self {
            Self::Mzb => Self::Mmb,
            Self::Mmb => Self::Both,
            Self::Both => Self::Mzb,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mzb => "mzb",
            Self::Mmb => "mmb",
            Self::Both => "both",
        }
    }

    pub fn uses_mzb(self) -> bool {
        matches!(self, Self::Mzb | Self::Both)
    }

    pub fn uses_mmb(self) -> bool {
        matches!(self, Self::Mmb | Self::Both)
    }
}

#[derive(Resource, Debug, Clone, Copy)]
pub struct DrawDistance {
    pub world: f32,
    pub mob: f32,

    pub zone_geom_mode: ZoneGeomMode,

    pub camera_collision_source: CameraCollisionSource,
}

impl Default for DrawDistance {
    fn default() -> Self {
        Self {
            world: DEFAULT_WORLD_DRAW_DISTANCE,
            mob: DEFAULT_MOB_DRAW_DISTANCE,
            zone_geom_mode: ZoneGeomMode::default(),
            camera_collision_source: CameraCollisionSource::default(),
        }
    }
}

#[derive(Component)]
pub struct MzbCollisionMesh;

#[derive(Resource, Default)]
pub struct MzbCollisionGeometry {
    pub positions: Vec<Vec3>,

    pub indices: Vec<u32>,

    pub cell_index: std::collections::HashMap<(i32, i32), Vec<u32>>,

    /// DAT file the triangles came from. Grounding against a zone the player
    /// is no longer in sticks entities to the wrong surface (the nearest-floor
    /// snap is a fixed point), so the auto-loader clears this resource the
    /// moment the effective zone DAT changes instead of waiting for the new
    /// load to land.
    pub source_file_id: Option<u32>,
}

#[derive(Clone)]
pub struct LoadedZoneGeom {
    pub submeshes: Arc<Vec<MzbSubMesh>>,
    pub instances: Arc<Vec<MzbInstance>>,

    pub mmb_spawns: Result<Vec<ZoneMmbSpawn>, String>,
}

#[derive(Resource, Default)]
pub struct LoadMzbInFlight {
    pub tasks: std::collections::HashMap<u32, (Vec<LoadMzbRequest>, Task<LoadedZoneGeom>)>,
}

#[derive(Resource, Default)]
pub struct ZoneGeomCache {
    pub entries: VecDeque<(u32, LoadedZoneGeom)>,
}

pub const ZONE_GEOM_CACHE_CAP: usize = 4;

impl ZoneGeomCache {
    fn get_and_promote(&mut self, file_id: u32) -> Option<LoadedZoneGeom> {
        let pos = self.entries.iter().position(|(id, _)| *id == file_id)?;
        let entry = self.entries.remove(pos)?;
        let geom = entry.1.clone();
        self.entries.push_front(entry);
        Some(geom)
    }

    fn insert(&mut self, file_id: u32, geom: LoadedZoneGeom) {
        if let Some(pos) = self.entries.iter().position(|(id, _)| *id == file_id) {
            self.entries.remove(pos);
        }
        self.entries.push_front((file_id, geom));
        while self.entries.len() > ZONE_GEOM_CACHE_CAP {
            self.entries.pop_back();
        }
    }
}

pub const MZB_GRID_CELL: f32 = 8.0;

pub const FLOOR_NORMAL_MIN: f32 = 0.5;

impl MzbCollisionGeometry {
    pub fn tri_count(&self) -> usize {
        self.indices.len() / 3
    }

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

    /// Floor (near-flat normal) in this column whose height is closest to
    /// `ref_y`. Unlike [`Self::ground_raycast`]'s one-sided `ceiling` cutoff,
    /// "nearest" is a fixed point under grounding (a grounded entity's nearest
    /// floor is the floor it stands on), so it doesn't oscillate when the
    /// reference Y wobbles near a cutoff, and it picks the entity's own level in
    /// a multi-floor building instead of the floor above.
    pub fn ground_nearest(&self, xz: Vec2, ref_y: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        self.for_each_hit_in_column(xz, |hit_y, normal| {
            if normal.y.abs() < FLOOR_NORMAL_MIN {
                return;
            }
            best = Some(match best {
                Some(prev) if (prev - ref_y).abs() <= (hit_y - ref_y).abs() => prev,
                _ => hit_y,
            });
        });
        best
    }

    pub fn ground_raycast_all(&self, xz: Vec2) -> Vec<(f32, Vec3)> {
        let mut hits: Vec<(f32, Vec3)> = Vec::new();
        self.for_each_hit_in_column(xz, |hit_y, normal| hits.push((hit_y, normal)));
        hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        hits
    }

    fn for_each_hit_in_column(&self, xz: Vec2, mut visit: impl FnMut(f32, Vec3)) {
        const RAY_ORIGIN_Y: f32 = 1000.0;
        let orig = Vec3::new(xz.x, RAY_ORIGIN_Y, xz.y);
        let dir = Vec3::new(0.0, -1.0, 0.0);

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

#[derive(Component)]
pub struct MzbNonCollisionMesh;

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

#[derive(Component)]
pub struct MzbOverlay;

#[derive(Component)]
pub struct AutoMzbOverlay;

#[derive(Component)]
pub struct WaterPlane;

// Actual water footprint: the water-material submesh triangles flattened to the
// surface height (world XZ preserved), NOT a bounding box — a box unions
// disconnected ponds and floods the dry paths between them. The depth test clips
// the flattened surface to wherever terrain sits below it.
pub struct WaterSpec {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub min: Vec3,
    pub max: Vec3,
    pub parent: Entity,
    pub auto_loaded: bool,
}

// MZB load computes per-placement water footprints (CPU-side) and queues them
// here; spawn_zone_water streams them in distance-gated and nearest-first, like
// the MMB visual models (process_load_mmb_requests), instead of spawning the whole
// zone's water at once. Cleared on zone change in auto_load_zone_geometry_system.
#[derive(Resource, Default)]
pub struct PendingWaterSpawns {
    pub specs: std::collections::VecDeque<WaterSpec>,
}

// World units covered by one repeat of the water ripple texture. Vanilla water
// mesh UVs are baked as world XZ / WATER_TEX_TILE, so ripple size and scroll
// speed are world-sized and pond-independent, and a single material is shared
// by every pond (see ZoneWaterMaterial).
const WATER_TEX_TILE: f32 = 16.0;

// Scroll velocity in world units/sec (XZ). Gentle drift; sub-tile per second.
const WATER_SCROLL_WORLD: Vec2 = Vec2::new(0.55, 0.35);

#[derive(Message, Debug, Clone, Copy)]
pub struct LoadMzbRequest {
    pub file_id: u32,

    pub chunk_idx: Option<usize>,
    pub world_pos: Vec3,

    pub auto_loaded: bool,
}

pub struct MzbSubMesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,

    pub tri_material: Vec<u8>,

    pub flags: u16,
}

pub struct MzbInstance {
    pub submesh_idx: usize,
    pub bevy_transform: Transform,

    pub water_height_bevy: Option<f32>,
}

pub fn load_mzb_placed(
    file_id: u32,
    chunk_idx: Option<usize>,
) -> Result<(Vec<MzbSubMesh>, Vec<MzbInstance>), String> {
    let (header, plain, _chunks) = load_decrypted(file_id, chunk_idx)?;

    let placements =
        mzb::parse_placements(&plain, &header).map_err(|e| format!("MZB parse_placements: {e}"))?;

    if placements.is_empty() {
        let meshes =
            mzb::parse_meshes(&plain, &header).map_err(|e| format!("MZB parse_meshes: {e}"))?;

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

    let mut unique_offsets: Vec<u32> = Vec::new();
    let mut offset_to_idx: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    for p in &placements {
        if let std::collections::hash_map::Entry::Vacant(e) = offset_to_idx.entry(p.geometry_offset)
        {
            e.insert(unique_offsets.len());
            unique_offsets.push(p.geometry_offset);
        }
    }

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
            continue;
        };

        let m_native = Mat4::from_cols_array(&p.transform);

        let to_bevy = Mat4::from_cols(
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, -1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, -1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        );
        let m_bevy = to_bevy * m_native;

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

#[derive(Debug, Clone, Copy)]
pub struct ZoneMmbSpawn {
    pub chunk_idx: usize,
    pub bevy_transform: Mat4,
}

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

                Some((idx, hdr.zone_mesh_name()))
            });
        }
    });
    let mut mmb_names: Vec<String> = Vec::with_capacity(parsed.len());
    let mut mmb_indices: Vec<usize> = Vec::with_capacity(parsed.len());
    for entry in parsed.into_iter().flatten() {
        mmb_indices.push(entry.0);
        mmb_names.push(entry.1);
    }

    use std::collections::HashMap;
    let mut name_to_locals: HashMap<&str, Vec<usize>> = HashMap::new();
    for (local, name) in mmb_names.iter().enumerate() {
        if !name.is_empty() {
            name_to_locals.entry(name.as_str()).or_default().push(local);
        }
    }

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

    let mut rr_cursor: HashMap<&str, usize> = HashMap::new();
    let mut out = Vec::with_capacity(placements.len());
    for p in &placements {
        let id = p.id_str().trim_end_matches('\0').trim_end();
        let Some(matches) = name_to_locals.get(id) else {
            continue;
        };
        let cursor = rr_cursor.entry(id).or_insert(0);
        let local_idx = matches[*cursor % matches.len()];
        *cursor += 1;
        let chunk_idx = mmb_indices[local_idx];

        let m_ffxi = Mat4::from_scale_rotation_translation(
            Vec3::new(p.scale[0], p.scale[1], p.scale[2]),
            Quat::from_euler(EulerRot::XYZ, p.rot[0], p.rot[1], p.rot[2]),
            Vec3::new(p.trans[0], p.trans[1], p.trans[2]),
        );

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

    let diag_enabled = match std::env::var("FFXI_DIAG_ZONE_GEOM") {
        Ok(s) if s == "*" || s == "all" || s.eq_ignore_ascii_case("any") => true,
        Ok(s) => s.parse::<u32>().ok() == Some(file_id),
        _ => false,
    };
    if diag_enabled {
        use std::collections::HashMap;

        let mut name_counts: HashMap<&str, u32> = HashMap::new();
        for n in &mmb_names {
            *name_counts.entry(n.trim_end()).or_insert(0) += 1;
        }
        let mut dup_names: Vec<(&str, u32)> = name_counts
            .iter()
            .filter(|(_, &c)| c > 1)
            .map(|(&n, &c)| (n, c))
            .collect();
        dup_names.sort_by_key(|x| std::cmp::Reverse(x.1));

        let mut placement_id_counts: HashMap<String, u32> = HashMap::new();
        let mut bucket0: Vec<String> = Vec::new();
        let mut bucket1: u32 = 0;
        let mut bucket_many: Vec<(String, usize)> = Vec::new();
        for p in &placements {
            let id = p.id_str().trim_end_matches('\0').trim_end().to_string();
            *placement_id_counts.entry(id.clone()).or_insert(0) += 1;
            let matches_len = name_to_locals.get(id.as_str()).map_or(0, |v| v.len());
            match matches_len {
                0 => bucket0.push(id),
                1 => bucket1 += 1,
                n => bucket_many.push((id, n)),
            }
        }

        let mut roundrobin_smoke: Vec<(String, u32, usize)> = Vec::new();
        for (id, count) in &placement_id_counts {
            if *count < 2 {
                continue;
            }
            let m = name_to_locals.get(id.as_str()).map_or(0, |v| v.len());
            if m > 1 {
                roundrobin_smoke.push((id.clone(), *count, m));
            }
        }
        roundrobin_smoke.sort_by_key(|x| std::cmp::Reverse(x.1));

        let mut unmatched_unique: HashMap<String, u32> = HashMap::new();
        for id in &bucket0 {
            *unmatched_unique.entry(id.clone()).or_insert(0) += 1;
        }
        let mut um_list: Vec<(String, u32)> = unmatched_unique.into_iter().collect();
        um_list.sort_by_key(|x| std::cmp::Reverse(x.1));

        info!(
            target: "ffxi_viewer_core::dat_mzb::diag",
            file_id,
            placements = placements.len(),
            spawned = out.len(),
            mmb_names = mmb_names.len(),
            distinct_names = name_to_locals.len(),
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

pub fn kick_load_mzb_tasks(
    mut events: MessageReader<LoadMzbRequest>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    draw: Res<DrawDistance>,
    mut collision_geometry: ResMut<MzbCollisionGeometry>,
    mut load_mmb_tx: MessageWriter<crate::dat_mmb::LoadMmbRequest>,
    mut pending_water: ResMut<PendingWaterSpawns>,
    mut in_flight: ResMut<LoadMzbInFlight>,
    mut cache: ResMut<ZoneGeomCache>,
) {
    let init_vis = compute_init_visibility(draw.zone_geom_mode);
    for req in events.read() {
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
                &mut pending_water,
                init_vis,
                true,
            );
            continue;
        }

        if let Some((reqs, _)) = in_flight.tasks.get_mut(&req.file_id) {
            reqs.push(*req);
            continue;
        }

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

pub fn poll_load_mzb_tasks(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    draw: Res<DrawDistance>,
    mut collision_geometry: ResMut<MzbCollisionGeometry>,
    mut load_mmb_tx: MessageWriter<crate::dat_mmb::LoadMmbRequest>,
    mut pending_water: ResMut<PendingWaterSpawns>,
    mut in_flight: ResMut<LoadMzbInFlight>,
    mut cache: ResMut<ZoneGeomCache>,
) {
    let init_vis = compute_init_visibility(draw.zone_geom_mode);

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
                &mut pending_water,
                init_vis,
                false,
            );
        }
    }
}

fn compute_init_visibility(mode: ZoneGeomMode) -> (Visibility, Visibility) {
    match mode {
        ZoneGeomMode::Off => (Visibility::Hidden, Visibility::Hidden),
        ZoneGeomMode::Collision | ZoneGeomMode::Camera => {
            (Visibility::Inherited, Visibility::Hidden)
        }
        ZoneGeomMode::All => (Visibility::Inherited, Visibility::Inherited),
    }
}

// Flat water tint — linear-space conversion of the old StandardMaterial
// placeholder srgba(0.20, 0.30, 0.31, 0.40). The procedural ripple texture
// modulates it (a stand-in until the retail scrolling water texture set
// (MMB 0x8000 section) is parsed); unlike the old PBR material, this runs the
// FFXI zone lighting model, so ponds track zone time-of-day/weather light
// like the terrain.
const WATER_TINT: Vec4 = Vec4::new(0.033, 0.073, 0.078, 0.40);

/// Shared handle for the vanilla water-surface material, so `scroll_water_uv`
/// can integrate `uv_offset` on one asset instead of per-spawn clones.
#[derive(Resource, Default)]
pub struct ZoneWaterMaterial(pub Option<Handle<crate::ffxi_zone_material::FfxiZoneMaterial>>);

fn simple_water_material(texture: Handle<Image>) -> crate::ffxi_zone_material::FfxiZoneMaterial {
    crate::ffxi_zone_material::FfxiZoneMaterial::new(
        Some(texture),
        crate::skinned_ffxi_material::FfxiMaterialFlags {
            // (has_texture, blend-emit [0x8000 translucent path], unused,
            // discard threshold — 0 so the cutout test never fires).
            flags: Vec4::new(1.0, 1.0, 0.0, 0.0),
        },
        WATER_TINT,
        Vec4::ZERO,
        AlphaMode::Blend,
        crate::ffxi_zone_material::FfxiZoneMaterialKey {
            // The old StandardMaterial was double-sided (cull_mode: None).
            back_face_culling: false,
            mirrored: false,
            // Bounding-box plane is near-coplanar with the sloped pond bed at
            // the shoreline; the decal polygon-offset pulls the surface toward
            // the camera so it wins the depth test there (replaces the old
            // constant `depth_bias: 1000.0`).
            z_bias_level: 1,
            depth_write: false,
        },
    )
}

/// Scrolls the shared water material's UVs. Runs on the single cached asset in
/// [`ZoneWaterMaterial`]; every water plane in the zone shares it.
pub fn scroll_water_uv(
    time: Res<Time>,
    water_mat: Res<ZoneWaterMaterial>,
    mut materials: ResMut<Assets<crate::ffxi_zone_material::FfxiZoneMaterial>>,
) {
    let Some(handle) = water_mat.0.as_ref() else {
        return;
    };
    // get_mut_untracked: uv_offset flows to the GPU through the persistent
    // buffers in upload_zone_material_buffers; marking the asset Modified here
    // would needlessly rebuild its bind group every frame (same pattern as
    // zone_clouds).
    if let Some(material) = materials.get_mut_untracked(handle) {
        // fract() keeps the offset small so f32 precision holds over long
        // sessions; the texture repeats, so a whole-tile jump is invisible.
        let t = time.elapsed_secs();
        let uv = Vec2::new(
            (t * WATER_SCROLL_WORLD.x / WATER_TEX_TILE).fract(),
            (t * WATER_SCROLL_WORLD.y / WATER_TEX_TILE).fract(),
        );
        material.uv_offset = Vec4::new(uv.x, uv.y, 0.0, 0.0);
    }
}

// Tileable procedural ripple: low-contrast sum of integer-frequency sine bands
// so opposite edges match exactly. Stands in until the DAT-sourced water
// texture set is parsed; the scroll mechanism (uv_transform animation) is the
// part that carries over.
fn water_ripple_image() -> Image {
    use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    const N: usize = 64;
    let mut data = Vec::with_capacity(N * N * 4);
    let tau = std::f32::consts::TAU;
    for y in 0..N {
        for x in 0..N {
            let u = x as f32 / N as f32;
            let v = y as f32 / N as f32;
            // Integer wave vectors -> exact tiling at the texture border.
            let w = (tau * (3.0 * u + v)).sin()
                + (tau * (u - 2.0 * v)).sin()
                + 0.5 * (tau * (5.0 * u + 4.0 * v)).sin();
            // w in [-2.5, 2.5] -> luma around 1.0 with subtle modulation, so
            // the material's base_color tint still sets the overall look.
            let l = 1.0 + 0.10 * (w / 2.5);
            let b = (l.clamp(0.0, 1.0) * 255.0) as u8;
            data.extend_from_slice(&[b, b, b, 255]);
        }
    }
    let mut img = Image::new(
        Extent3d {
            width: N as u32,
            height: N as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..ImageSamplerDescriptor::linear()
    });
    img
}

// `world_tile_uvs`: the vanilla FFXI water material wants world XZ /
// WATER_TEX_TILE baked into the mesh, so the shared material's ripples are
// world-sized and continuous across ponds. bevy_water (enhanced) instead wants
// UVs normalised over the footprint bounds, so its `coord_offset`/
// `coord_scale` recover world coords for a continuous, world-scaled wave
// field.
fn build_water_surface_mesh(spec: &WaterSpec, world_tile_uvs: bool) -> Mesh {
    let dx = (spec.max.x - spec.min.x).max(0.01);
    let dz = (spec.max.z - spec.min.z).max(0.01);
    let uvs: Vec<[f32; 2]> = if world_tile_uvs {
        spec.positions
            .iter()
            .map(|p| [p[0] / WATER_TEX_TILE, p[2] / WATER_TEX_TILE])
            .collect()
    } else {
        spec.positions
            .iter()
            .map(|p| [(p[0] - spec.min.x) / dx, (p[2] - spec.min.z) / dz])
            .collect()
    };
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, spec.positions.clone());
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_NORMAL,
        vec![[0.0, 1.0, 0.0]; spec.positions.len()],
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    // Neutral 0.5 vertex colour: the zone shader's XIM `2 · vertexColor`
    // overbright convention makes 0.5 the identity, leaving the water colour
    // entirely to WATER_TINT.
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_COLOR,
        vec![[0.5, 0.5, 0.5, 1.0]; spec.positions.len()],
    );
    mesh.insert_indices(Indices::U32(spec.indices.clone()));
    mesh
}

// Drains water footprints queued by the MZB load and spawns one surface each:
// the vanilla translucent plane, or — when built with `enhanced-water` and the
// GraphicsSettings toggle is on — bevy_water's animated material on the same
// footprint mesh. Reads the setting at drain time, so a toggle change takes
// effect on the next zone (re)load.
fn water_dist_sq_xz(spec: &WaterSpec, self_pos: Vec3) -> f32 {
    let cx = 0.5 * (spec.min.x + spec.max.x);
    let cz = 0.5 * (spec.min.z + spec.max.z);
    let dx = cx - self_pos.x;
    let dz = cz - self_pos.z;
    dx * dx + dz * dz
}

pub fn spawn_zone_water(
    mut commands: Commands,
    mut pending: ResMut<PendingWaterSpawns>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<crate::ffxi_zone_material::FfxiZoneMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut water_mat: ResMut<ZoneWaterMaterial>,
    settings: Res<crate::graphics::GraphicsSettings>,
    draw: Res<DrawDistance>,
    self_q: Query<&GlobalTransform, With<IsSelf>>,
    #[cfg(feature = "enhanced-water")] mut water_materials: ResMut<
        Assets<crate::water_enhanced::StandardWaterMaterial>,
    >,
) {
    if pending.specs.is_empty() {
        return;
    }

    let self_pos = self_q.single().ok().map(|t| t.translation());
    if let Some(self_pos) = self_pos {
        pending.specs.make_contiguous().sort_by(|a, b| {
            water_dist_sq_xz(a, self_pos).total_cmp(&water_dist_sq_xz(b, self_pos))
        });
    }
    let load_radius = settings.view_distance * MMB_LOAD_DISTANCE_MARGIN;
    let load_radius_sq = load_radius * load_radius;
    let enhanced = cfg!(feature = "enhanced-water") && settings.enhanced_water;
    let simple_mat = water_mat
        .0
        .get_or_insert_with(|| {
            materials.add(simple_water_material(images.add(water_ripple_image())))
        })
        .clone();

    // Water surfaces are visual, non-collision zone geometry: honor the current
    // ZoneGeomMode at spawn time (a zone loaded while geom is Off/Collision must
    // not show water), and tag MzbNonCollisionMesh so apply_zone_geom_visibility
    // toggles them with the rest of the non-collision meshes.
    let (_, water_vis) = compute_init_visibility(draw.zone_geom_mode);

    const WATER_SPAWN_BUDGET: usize = 32;
    let mut spawned = 0usize;
    let mut retained: std::collections::VecDeque<WaterSpec> =
        std::collections::VecDeque::with_capacity(pending.specs.len());

    while let Some(spec) = pending.specs.pop_front() {
        if let Some(self_pos) = self_pos {
            // Sorted nearest-first, so the first out-of-range spec means the rest
            // are too — retain them all for when the player moves closer.
            if water_dist_sq_xz(&spec, self_pos) > load_radius_sq {
                retained.push_back(spec);
                retained.append(&mut pending.specs);
                break;
            }
        }
        if spawned >= WATER_SPAWN_BUDGET {
            retained.push_back(spec);
            continue;
        }
        spawned += 1;

        let mesh = Mesh3d(meshes.add(build_water_surface_mesh(&spec, !enhanced)));
        let mut e;
        #[cfg(feature = "enhanced-water")]
        if enhanced {
            let mat = crate::water_enhanced::pond_water_material(
                &mut water_materials,
                spec.min,
                spec.max,
            );
            e = commands.spawn((
                MzbOverlay,
                WaterPlane,
                MzbNonCollisionMesh,
                mesh,
                mat,
                Transform::IDENTITY,
                water_vis,
                bevy::light::NotShadowReceiver,
                ChildOf(spec.parent),
            ));
            if spec.auto_loaded {
                e.insert(AutoMzbOverlay);
            }
            continue;
        }

        e = commands.spawn((
            MzbOverlay,
            WaterPlane,
            MzbNonCollisionMesh,
            mesh,
            MeshMaterial3d(simple_mat.clone()),
            Transform::IDENTITY,
            water_vis,
            bevy::light::NotShadowReceiver,
            ChildOf(spec.parent),
        ));
        if spec.auto_loaded {
            e.insert(AutoMzbOverlay);
        }
    }
    pending.specs = retained;
    let _ = enhanced;
}

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
    pending_water: &mut PendingWaterSpawns,
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

        mesh.compute_smooth_normals();

        if let Some(normals) = mesh
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .and_then(|a| a.as_float3())
        {
            let colors: Vec<[f32; 4]> = normals
                .iter()
                .zip(vert_mat.iter())
                .map(|(n, &m)| {
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

    let collision_verts = collision_positions.len();
    let collision_tris = collision_indices.len() / 3;
    let noncollision_verts = noncollision_positions.len();
    let noncollision_tris = noncollision_indices.len() / 3;

    collision_geometry.positions = collision_positions
        .iter()
        .map(|p| Vec3::new(p[0], p[1], p[2]))
        .collect();
    collision_geometry.indices = collision_indices.clone();

    collision_geometry.cell_index =
        build_cell_index(&collision_geometry.positions, &collision_geometry.indices);
    collision_geometry.source_file_id = Some(req.file_id);

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

    // One localized footprint per water-material placement (NOT merged by height),
    // so spawn_zone_water can distance-gate each like an MMB placement. Merging the
    // whole zone's water into one mesh would make it un-streamable.
    let mut water_added = 0usize;
    for inst in instances.iter() {
        let Some(h_bevy) = inst.water_height_bevy else {
            continue;
        };
        let sub = &submeshes[inst.submesh_idx];
        if sub.positions.is_empty() || sub.indices.is_empty() {
            continue;
        }

        let mut positions: Vec<[f32; 3]> = Vec::with_capacity(sub.positions.len());
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for v in &sub.positions {
            let p = inst
                .bevy_transform
                .transform_point(Vec3::new(v[0], v[1], v[2]));
            // Flatten to the flat water surface; keep world XZ to follow the
            // actual shoreline rather than a bounding box.
            let flat = [p.x, h_bevy, p.z];
            min = min.min(Vec3::from_array(flat));
            max = max.max(Vec3::from_array(flat));
            positions.push(flat);
        }
        pending_water.specs.push_back(WaterSpec {
            positions,
            indices: sub.indices.clone(),
            min,
            max,
            parent,
            auto_loaded: req.auto_loaded,
        });
        water_added += 1;
    }
    if water_added > 0 {
        push_system_msg(
            toasts,
            format!(
                "/load_mzb {}: {} water surface{} queued",
                req.file_id,
                water_added,
                if water_added == 1 { "" } else { "s" },
            ),
        );
    }

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

#[derive(Resource, Default)]
pub struct LastAutoLoadedZone {
    pub file_id: Option<u32>,
}

pub fn auto_load_zone_geometry_system(
    scene_state: Res<SceneState>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    mut last: ResMut<LastAutoLoadedZone>,
    mut commands: Commands,
    mut load_tx: MessageWriter<LoadMzbRequest>,
    auto_q: Query<Entity, With<AutoMzbOverlay>>,
    mut mzb_in_flight: ResMut<LoadMzbInFlight>,
    mut mmb_queue: ResMut<crate::dat_mmb::MmbLoadQueue>,
    mut mmb_in_flight: ResMut<crate::dat_mmb::MmbLoadInFlight>,
    mut pending_water: ResMut<PendingWaterSpawns>,
    mut collision_geometry: ResMut<MzbCollisionGeometry>,
) {
    let current = crate::snapshot::effective_zone_file_id(&scene_state.snapshot);
    if current == last.file_id {
        return;
    }

    for e in auto_q.iter() {
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.despawn();
        }
    }

    // Keeping the old zone's triangles until the new load lands grounds
    // entities against geometry they're not in: entering a Mog House snapped
    // the player onto a city surface at the MH-origin column, and the
    // nearest-floor snap then resolved that stuck Y to the MH model's roof.
    if collision_geometry.source_file_id != current {
        *collision_geometry = MzbCollisionGeometry::default();
    }

    if !mzb_in_flight.tasks.is_empty() {
        mzb_in_flight.tasks.clear();
    }
    mmb_queue
        .pending
        .retain(|r| !(r.entity_id.is_none() && r.world_transform.is_some()));
    mmb_in_flight.tasks.clear();
    // Drop any old-zone water footprints still queued for streaming; the spawned
    // ones go with the despawned AutoMzbOverlay parent above.
    pending_water.specs.clear();
    last.file_id = current;

    match current {
        Some(file_id) => {
            load_tx.write(LoadMzbRequest {
                file_id,
                chunk_idx: None,
                world_pos: Vec3::ZERO,
                auto_loaded: true,
            });

            let zone_label = scene_state
                .snapshot
                .zone_id
                .map_or_else(|| "?".to_string(), |z| z.to_string());
            let myroom_label = scene_state
                .snapshot
                .myroom
                .map(|m| format!(" (Mog House model {})", m.model))
                .unwrap_or_default();
            push_system_msg(
                &mut toasts,
                format!("auto-load: zone {zone_label}{myroom_label} -> DAT file {file_id}"),
            );
        }
        None => {
            let Some(zone_id) = scene_state.snapshot.zone_id else {
                return;
            };
            push_system_msg(
                &mut toasts,
                format!("auto-load: no DAT mapping for zone {zone_id} (Phase 11b table pending)"),
            );
        }
    }
}

pub fn cull_mzb_by_distance(
    draw: Res<DrawDistance>,
    self_q: Query<&GlobalTransform, With<IsSelf>>,

    mut mzb_q: Query<(&GlobalTransform, &mut Visibility), (With<MzbOverlay>, With<Mesh3d>)>,
) {
    let Ok(self_t) = self_q.single() else {
        return;
    };
    let self_pos = self_t.translation();
    let cull_sq = draw.world * draw.world;

    for (mzb_t, mut vis) in mzb_q.iter_mut() {
        let mzb_pos = mzb_t.translation();

        let dx = mzb_pos.x - self_pos.x;
        let dz = mzb_pos.z - self_pos.z;
        let d_sq = dx * dx + dz * dz;
        let want = if d_sq > cull_sq {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };

        if *vis != want {
            *vis = want;
        }
    }
}

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

#[cfg(test)]
mod ground_tests {
    use super::*;

    fn floor_at(h: f32) -> ([Vec3; 4], [u32; 6]) {
        (
            [
                Vec3::new(-4.0, h, -4.0),
                Vec3::new(4.0, h, -4.0),
                Vec3::new(4.0, h, 4.0),
                Vec3::new(-4.0, h, 4.0),
            ],
            [0, 1, 2, 0, 2, 3],
        )
    }

    fn two_floors(low: f32, high: f32) -> MzbCollisionGeometry {
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for h in [low, high] {
            let (verts, idx) = floor_at(h);
            let base = positions.len() as u32;
            positions.extend_from_slice(&verts);
            indices.extend(idx.iter().map(|i| base + i));
        }
        MzbCollisionGeometry {
            positions,
            indices,
            cell_index: std::collections::HashMap::new(),
            source_file_id: None,
        }
    }

    #[test]
    fn ground_nearest_is_fixed_point() {
        let geom = two_floors(0.0, 4.0);
        let g = geom.ground_nearest(Vec2::ZERO, 0.0).unwrap();
        assert_eq!(g, 0.0, "a grounded entity's nearest floor is its own floor");
        assert_eq!(
            geom.ground_nearest(Vec2::ZERO, g).unwrap(),
            g,
            "re-running on the result is stable (no per-frame oscillation)"
        );
    }

    #[test]
    fn ground_nearest_picks_own_level_not_floor_above() {
        let geom = two_floors(0.0, 4.0);
        assert_eq!(
            geom.ground_nearest(Vec2::ZERO, 4.3).unwrap(),
            4.0,
            "standing on the upper floor stays on it, not snapped down"
        );
        assert_eq!(
            geom.ground_nearest(Vec2::ZERO, 0.3).unwrap(),
            0.0,
            "near the lower floor stays low, not pulled up to the floor above"
        );
    }

    #[test]
    fn ground_nearest_grounds_entity_below_floor() {
        let geom = two_floors(0.0, 4.0);
        assert_eq!(
            geom.ground_nearest(Vec2::ZERO, -50.0).unwrap(),
            0.0,
            "a pathing entity sent a flat reference Y far below ground still snaps up"
        );
    }
}
