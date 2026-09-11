use std::collections::HashMap;

use crate::{DatError, Result};

use crate::mmb::keys::KEY_TABLE;

#[derive(Debug, thiserror::Error)]
pub enum MzbError {
    #[error("MZB body too small: need at least {needed} bytes, got {actual}")]
    TooSmall { needed: usize, actual: usize },
    #[error("MZB collision-data offset {offset} out of range (body is {len} bytes)")]
    CollisionDataOutOfRange { offset: usize, len: usize },
    #[error("MZB placement table ends at {end}, past the {len}-byte body")]
    PlacementTableOutOfRange { end: usize, len: usize },
    #[error("MZB mesh-data offset {offset} out of range (body is {len} bytes)")]
    MeshDataOutOfRange { offset: usize, len: usize },
    #[error("MZB mesh record at {pos} has crossed offsets (verts={verts}, normals={normals}, tris={tris})")]
    CrossedOffsets {
        pos: usize,
        verts: usize,
        normals: usize,
        tris: usize,
    },
}

impl From<MzbError> for DatError {
    fn from(e: MzbError) -> Self {
        DatError::Mzb(format!("{e}"))
    }
}

/// research/XIClient/src/XIClient/include/Resource/Derived/ZoneBlockFormat.h ZoneBlockHeader
/// — `ZoneBlockHeader`, a packed 0x20-byte struct.
const HDR_SIZE_AND_VERSION: usize = 0x00;
const HDR_CHUNK_COUNT_AND_DECRYPT_INDEX: usize = 0x04;
const HDR_COLLISION_DATA_OFFSET: usize = 0x08;
const HDR_TERRAIN_SCALE_X: usize = 0x0C;
const HDR_TERRAIN_SCALE_Z: usize = 0x0D;
const HDR_TERRAIN_UNITS_X: usize = 0x0E;
const HDR_TERRAIN_UNITS_Z: usize = 0x0F;
const HDR_QUADTREE_OR_GROUP_COUNT: usize = 0x10;
const HDR_GROUP_LIST_OFFSET: usize = 0x14;
const HDR_LIGHTING_OFFSET: usize = 0x18;
const HDR_SUBSTRUCTURE_TYPE: usize = 0x1C;
const HDR_COLLISION_FLAGS: usize = 0x1D;
pub const MZB_HEADER_LEN: usize = 0x20;

/// Low 24 bits of the two packed header dwords; the top byte is the format
/// version / decrypt-table index respectively (ZoneBlockFormat.h ZoneBlockHeader).
const HDR_COUNT_MASK: u32 = 0x00FF_FFFF;

/// research/XIClient/src/XIClient/source/Resource/Derived/ZoneBlockResource.cpp ZoneBlockResource::Decrypt
/// — `if (GetFormatVersion() < 27) return;`, i.e. only version 27 files carry
/// the pass-1 XOR at all.
const ENCRYPTED_MIN_VERSION: u8 = 27;

/// ZoneBlockResource.cpp ZoneBlockResource::Decrypt — "the first 8 bytes are never encrypted", so the
/// pass-1 region is `[8, 8 + encryptedByteCount)`.
const ENCRYPTED_REGION_START: usize = 8;
const DECRYPT_INDEX_XOR: u8 = 0xFF;

/// Pass 2 XORs the name of every placement record.
/// research/cexi-docs/zone/format.md "ZoneDef placement (`0x1C`)" — 0x64-byte records start at 0x20.
pub const PLACEMENT_RECORD_LEN: usize = 0x64;
const PLACEMENT_NAME_LEN: usize = 16;
const PLACEMENT_NAME_XOR: u8 = 0x55;

pub fn decrypt_in_place(data: &mut [u8]) -> Result<()> {
    if data.len() < ENCRYPTED_REGION_START {
        return Err(MzbError::TooSmall {
            needed: ENCRYPTED_REGION_START,
            actual: data.len(),
        }
        .into());
    }

    let encrypted_byte_count = (u32::from_le_bytes([
        data[HDR_SIZE_AND_VERSION],
        data[HDR_SIZE_AND_VERSION + 1],
        data[HDR_SIZE_AND_VERSION + 2],
        data[HDR_SIZE_AND_VERSION + 3],
    ]) & HDR_COUNT_MASK) as usize;
    let node_count = (u32::from_le_bytes([
        data[HDR_CHUNK_COUNT_AND_DECRYPT_INDEX],
        data[HDR_CHUNK_COUNT_AND_DECRYPT_INDEX + 1],
        data[HDR_CHUNK_COUNT_AND_DECRYPT_INDEX + 2],
        data[HDR_CHUNK_COUNT_AND_DECRYPT_INDEX + 3],
    ]) & HDR_COUNT_MASK) as usize;
    let version = data[HDR_SIZE_AND_VERSION + 3];
    let decrypt_index = data[HDR_CHUNK_COUNT_AND_DECRYPT_INDEX + 3];

    if version >= ENCRYPTED_MIN_VERSION {
        let mut key: i32 = KEY_TABLE[(decrypt_index ^ DECRYPT_INDEX_XOR) as usize] as i32;
        let mut key_count: i32 = 0;
        // ZoneBlockResource.cpp ZoneBlockResource::Decrypt — `position` counts from 0 while the run
        // it inverts starts at byte 8, so the encrypted region is
        // [8, 8 + encryptedByteCount) and both loop bound and skip test compare
        // against the count, not against the region's end offset.
        let mut position = 0usize;

        while position < encrypted_byte_count {
            let piece_len = (((key >> 4) & 7) as usize) + 16;
            if (key & 1) == 1 && position + piece_len < encrypted_byte_count {
                let start = ENCRYPTED_REGION_START.saturating_add(position);
                let end = start.saturating_add(piece_len).min(data.len());
                if start < end {
                    for b in &mut data[start..end] {
                        *b ^= 0xFF;
                    }
                }
            }
            key_count = key_count.wrapping_add(1);
            key = key.wrapping_add(key_count);
            position = position.saturating_add(piece_len);
        }
    }

    for i in 0..node_count {
        let base = MZB_HEADER_LEN.saturating_add(i.saturating_mul(PLACEMENT_RECORD_LEN));
        let end = base.saturating_add(PLACEMENT_NAME_LEN);
        if end > data.len() {
            break;
        }
        for b in &mut data[base..end] {
            *b ^= PLACEMENT_NAME_XOR;
        }
    }

    Ok(())
}

pub fn decrypt(data: &[u8]) -> Result<Vec<u8>> {
    let mut buf = data.to_vec();
    decrypt_in_place(&mut buf)?;
    Ok(buf)
}

/// World units spanned by one placement-grid sub-block. A zone block is divided
/// into `block_width / MZB_SUB_BLOCK_SIZE` sub-blocks per axis
/// (research/xim ZoneDefParser.kt: `subBlocksX = blockWidth / 4`). Most zones
/// ship 40-unit blocks (10 sub-blocks); Port Jeuno ships 80 (20).
pub const MZB_SUB_BLOCK_SIZE: u32 = 4;

/// ZoneRenderer.cpp ZoneRenderer::OpenMzb — below this the 0x10 dword is `GroupListCount` and
/// there is no quadtree; from 21 on it is `QuadTreeOffset`. Corroborated by the
/// shipped data: every version 17/18/20 MZB has 0 there, every version 23+ one a
/// live offset.
const QUADTREE_MIN_VERSION: u8 = 21;

/// ZoneRenderer.cpp ZoneRenderer::OpenMzb — the header's lighting section and the
/// per-placement `LightReferences` only exist from version 18 on; older files
/// get zeroed references.
const LIGHT_BINDING_MIN_VERSION: u8 = 18;

/// CollisionManager.cpp CollisionManager::PrepareData sets the collision-object record stride to 128
/// at version <= 0x19 and 192 above it. 192 is `sizeof(CollisionObjectData)`
/// and 128 is its two leading `Matrix4`s, so everything past them — including
/// the water-height word in `flags` — exists only above the split.
const LEGACY_COLLISION_OBJECT_MAX_VERSION: u8 = 0x19;

#[derive(Debug, Clone, Copy)]
pub struct MzbHeader {
    pub decode_length: u32,
    pub node_count: u32,
    pub version: u8,
    pub key_index: u8,
    pub zone_blocks_x: u8,
    pub zone_blocks_z: u8,

    pub block_width: u8,
    pub block_length: u8,

    /// Header 0x08. Zero is legal and means the zone ships no collision section
    /// at all — retail guards the whole collision path on it
    /// (ZoneRenderer.cpp ZoneRenderer::OpenMzb). The 15 moving-vehicle zones are all zero.
    pub collision_data_offset: u32,

    /// Header 0x10, a version-dependent union: see [`Self::quadtree_offset`] and
    /// [`Self::group_list_count`].
    pub quadtree_or_group_count: u32,

    pub group_list_offset: u32,
    pub lighting_offset: u32,

    /// Header 0x1C. `ZoneType = SubstructureType + 1` (ZoneRenderer.cpp ZoneRenderer::OpenMzb).
    pub substructure_type: u8,
    pub collision_flags: u8,
}

impl MzbHeader {
    /// Placement-grid cell counts: zone blocks × sub-blocks per block.
    pub fn grid_cells_x(&self) -> usize {
        (self.zone_blocks_x as usize)
            .saturating_mul((self.block_width as u32 / MZB_SUB_BLOCK_SIZE) as usize)
    }

    pub fn grid_cells_z(&self) -> usize {
        (self.zone_blocks_z as usize)
            .saturating_mul((self.block_length as u32 / MZB_SUB_BLOCK_SIZE) as usize)
    }

    pub fn has_collision_data(&self) -> bool {
        self.collision_data_offset != 0
    }

    pub fn quadtree_offset(&self) -> Option<u32> {
        (self.version >= QUADTREE_MIN_VERSION).then_some(self.quadtree_or_group_count)
    }

    pub fn group_list_count(&self) -> Option<u32> {
        (self.version < QUADTREE_MIN_VERSION).then_some(self.quadtree_or_group_count)
    }

    pub fn has_light_bindings(&self) -> bool {
        self.lighting_offset != 0 && self.version >= LIGHT_BINDING_MIN_VERSION
    }

    fn has_collision_object_tail(&self) -> bool {
        self.version > LEGACY_COLLISION_OBJECT_MAX_VERSION
    }
}

impl MzbHeader {
    pub fn parse(body: &[u8]) -> Result<Self> {
        if body.len() < MZB_HEADER_LEN {
            return Err(MzbError::TooSmall {
                needed: MZB_HEADER_LEN,
                actual: body.len(),
            }
            .into());
        }

        let dword = |o: usize| u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);

        Ok(Self {
            decode_length: dword(HDR_SIZE_AND_VERSION) & HDR_COUNT_MASK,
            node_count: dword(HDR_CHUNK_COUNT_AND_DECRYPT_INDEX) & HDR_COUNT_MASK,
            version: body[HDR_SIZE_AND_VERSION + 3],
            key_index: body[HDR_CHUNK_COUNT_AND_DECRYPT_INDEX + 3],
            zone_blocks_x: body[HDR_TERRAIN_SCALE_X],
            zone_blocks_z: body[HDR_TERRAIN_SCALE_Z],
            block_width: body[HDR_TERRAIN_UNITS_X],
            block_length: body[HDR_TERRAIN_UNITS_Z],
            collision_data_offset: dword(HDR_COLLISION_DATA_OFFSET),
            quadtree_or_group_count: dword(HDR_QUADTREE_OR_GROUP_COUNT),
            group_list_offset: dword(HDR_GROUP_LIST_OFFSET),
            lighting_offset: dword(HDR_LIGHTING_OFFSET),
            substructure_type: body[HDR_SUBSTRUCTURE_TYPE],
            collision_flags: body[HDR_COLLISION_FLAGS],
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MzbVertex {
    pub pos: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MzbNormal {
    pub n: [f32; 3],
}

/// Surface a collision triangle represents, from the [`MzbTriangleInfo::terrain`]
/// nibble. Names from research/xim ZoneDefParser.kt `TerrainType`; the mapping is
/// **measured**, not taken from XIM (tier 6): `dat-fishing-terrain-probe` scores
/// the nibble against LSB's independent `vendor/server/sql/fishing_area.sql`, and
/// water runs 0.3-4% of triangles zone-wide against 10-45% inside every radial
/// fishing cylinder. [`crate::footstep`] corroborates the 0..=10 span from the
/// shipped `fses` sound tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerrainType {
    Object,
    Path,
    Grass,
    Sand,
    Snow,
    Stone,
    Metal,
    Wood,
    ShallowWater,
    DeepWater,
    UnkA,
}

impl TerrainType {
    pub fn from_nibble(n: u8) -> Option<Self> {
        Some(match n {
            0 => Self::Object,
            1 => Self::Path,
            2 => Self::Grass,
            3 => Self::Sand,
            4 => Self::Snow,
            5 => Self::Stone,
            6 => Self::Metal,
            7 => Self::Wood,
            8 => Self::ShallowWater,
            9 => Self::DeepWater,
            10 => Self::UnkA,
            _ => return None,
        })
    }

    /// The surfaces a line can be cast into. Retail's client-side fishing gate
    /// accepts both depths (research/xim Scene.kt `canFish`).
    pub fn is_water(self) -> bool {
        matches!(self, Self::ShallowWater | Self::DeepWater)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MzbTriangleInfo {
    /// Terrain nibble: bit 15 of each of the four index words, low bit first.
    /// See [`TerrainType`].
    pub terrain: u8,

    pub is_invalid: bool,

    /// Third index word's `0x4000`. Feeds [`double_sided_skip`] — the chase
    /// camera and line-of-sight pass through, movement does not.
    pub camera_transparent: bool,
}

impl MzbTriangleInfo {
    pub fn terrain_type(&self) -> Option<TerrainType> {
        TerrainType::from_nibble(self.terrain)
    }
}

/// Third index word: `triangle->VertexIndex3 & 0x4000` in
/// research/XIClient/src/XIClient/include/World/Zone/Terrain/CollisionQuery.hpp
/// `DoubleSidedSkipPolicy::SkipTriangle`.
const TRI_CAMERA_TRANSPARENT: u16 = 0x4000;

/// Second index word, same bit position. research/cexi-docs/zone/collision.md "Technical notes"
/// claims player movement keys off it, but it measures 0 across Lower Jeuno,
/// Port Jeuno, Southern San d'Oria and West Ronfaure — if it gated blocking,
/// nothing in those zones would block. Parsed, unused, semantics unresolved.
const TRI_SECOND_WORD_FLAG: u16 = 0x4000;

const TRI_INDEX_MASK: u16 = 0x7FFF;
const TRI_FLAGGED_INDEX_MASK: u16 = 0x3FFF;

/// research/XIClient/src/XIClient/include/World/Zone/Terrain/CollisionQuery.hpp
/// `DoubleSidedSkipPolicy::SkipTriangle`:
/// `header->Flags != 0 && (triangle->VertexIndex3 & 0x4000) != 0`.
///
/// The whole `u16` is tested, not bit 0. Measured over Lower Jeuno, Port Jeuno,
/// Southern San d'Oria, West Ronfaure and two Mog House DATs, `flags` only ever
/// holds 0x0000 or 0x0001, so `!= 0` and `& 1` are indistinguishable on retail
/// data — this keeps the authoritative form. (In those same zones every
/// camera-transparent triangle already sits in a `flags != 0` mesh, so the mesh
/// gate never excludes anything; it is kept because retail tests it.)
///
/// Movement uses `BacksideCullingPolicy`, whose `SkipTriangle` is
/// unconditionally false — grounding must never consult this.
/// Corroborated: research/cexi-docs/zone/collision.md "Technical notes".
pub fn double_sided_skip(mesh_flags: u16, camera_transparent: bool) -> bool {
    mesh_flags != 0 && camera_transparent
}

#[derive(Debug, Clone)]
pub struct MzbMesh {
    pub vertices: Vec<MzbVertex>,
    pub normals: Vec<MzbNormal>,

    pub triangles: Vec<[u32; 3]>,

    pub triangle_normals: Vec<u32>,

    pub tri_info: Vec<MzbTriangleInfo>,

    pub flags: u16,
}

/// research/XIClient/src/XIClient/include/Resource/Derived/ZoneBlockFormat.h CollisionDataHeader
/// — `CollisionDataHeader`, the seven dwords the header's 0x08 offset points at.
const COLL_MESH_COUNT: usize = 0x00;
const COLL_MESH_DATA_OFFSET: usize = 0x04;
const COLL_GRID_DATA_OFFSET: usize = 0x10;
/// `CollisionDataHeader::SomeOffset` / `SomeCount` — the base and length of the
/// flat [`CollisionObjectData`](COLL_OBJECT_RECORD_LEN) array the grid cells point
/// into.
const COLL_OBJECT_ARRAY_OFFSET: usize = 0x14;
const COLL_OBJECT_ARRAY_COUNT: usize = 0x18;
/// Everything this parser reads from `CollisionDataHeader`, i.e. the whole struct.
const COLL_HEADER_READ_LEN: usize = COLL_OBJECT_ARRAY_COUNT + 4;

/// ZoneBlockFormat.h — `CollisionMeshHeader`, one per collision mesh.
const MESH_VERTEX_ARRAY: usize = 0x00;
const MESH_NORMAL_ARRAY: usize = 0x04;
const MESH_INDEX_ARRAY: usize = 0x08;
const MESH_TRIANGLE_COUNT: usize = 0x0C;
const MESH_FLAGS: usize = 0x0E;
/// Through `Flags`; the struct also carries two reserved dwords this parser
/// never reads.
const MESH_RECORD_LEN: usize = MESH_FLAGS + 2;
/// ZoneBlockFormat.h — `CollisionMeshTriangle`, four `unsigned short`.
const MESH_TRIANGLE_LEN: usize = 8;
/// `Common::Math::Vector3`, three `f32`.
const VEC3_LEN: usize = 12;

/// ZoneBlockFormat.h — `CollisionObjectData` opens with the object's
/// world matrix (`Common::Math::Matrix4 one`), followed by a second `Matrix4`
/// and a `Matrix3`.
const COLL_OBJECT_MATRIX_LEN: usize = 4 * 4 * 4;
const COLL_OBJECT_NORMAL_MATRIX_LEN: usize = 3 * 3 * 4;
/// `CollisionObjectData::flags`, past all three matrices. Only present above
/// [`LEGACY_COLLISION_OBJECT_MAX_VERSION`].
const COLL_OBJECT_FLAGS: usize = 2 * COLL_OBJECT_MATRIX_LEN + COLL_OBJECT_NORMAL_MATRIX_LEN;
/// `CollisionObjectData::something2`, six dwords past `flags`: the sub-area whose
/// interior replaces this object. `CollisionManager::KO_CharaCollision` and three
/// sibling walkers skip the object while that sub-area is the active one.
const COLL_OBJECT_SUB_AREA_LINK: usize = COLL_OBJECT_FLAGS + 0x18;
/// `something2` closes the record.
const COLL_OBJECT_RECORD_LEN: usize = COLL_OBJECT_SUB_AREA_LINK + 4;

/// Resolve the collision-data header, or `None` when the zone ships no collision
/// section (ZoneRenderer.cpp ZoneRenderer::OpenMzb guards the whole path on a non-zero offset).
fn collision_header(body: &[u8], header: &MzbHeader) -> Result<Option<usize>> {
    if !header.has_collision_data() {
        return Ok(None);
    }
    let off = header.collision_data_offset as usize;
    if off.saturating_add(COLL_HEADER_READ_LEN) > body.len() {
        return Err(MzbError::CollisionDataOutOfRange {
            offset: off,
            len: body.len(),
        }
        .into());
    }
    Ok(Some(off))
}

pub fn parse_meshes(body: &[u8], header: &MzbHeader) -> Result<Vec<MzbMesh>> {
    let Some(mt) = collision_header(body, header)? else {
        return Ok(Vec::new());
    };

    let mesh_count = u32::from_le_bytes([
        body[mt + COLL_MESH_COUNT],
        body[mt + COLL_MESH_COUNT + 1],
        body[mt + COLL_MESH_COUNT + 2],
        body[mt + COLL_MESH_COUNT + 3],
    ]) as usize;
    let mesh_data_offset = u32::from_le_bytes([
        body[mt + COLL_MESH_DATA_OFFSET],
        body[mt + COLL_MESH_DATA_OFFSET + 1],
        body[mt + COLL_MESH_DATA_OFFSET + 2],
        body[mt + COLL_MESH_DATA_OFFSET + 3],
    ]) as usize;

    if mesh_data_offset >= body.len() {
        return Err(MzbError::MeshDataOutOfRange {
            offset: mesh_data_offset,
            len: body.len(),
        }
        .into());
    }

    let mut out = Vec::with_capacity(mesh_count);
    let mut pos = mesh_data_offset;
    for _ in 0..mesh_count {
        if pos + MESH_RECORD_LEN > body.len() {
            break;
        }
        let mesh = parse_one_mesh(body, pos)?;

        let tri_off = u32::from_le_bytes([
            body[pos + MESH_INDEX_ARRAY],
            body[pos + MESH_INDEX_ARRAY + 1],
            body[pos + MESH_INDEX_ARRAY + 2],
            body[pos + MESH_INDEX_ARRAY + 3],
        ]) as usize;

        let tri_count = u16::from_le_bytes([
            body[pos + MESH_TRIANGLE_COUNT],
            body[pos + MESH_TRIANGLE_COUNT + 1],
        ]) as usize;
        out.push(mesh);
        let next = tri_off.saturating_add(tri_count.saturating_mul(MESH_TRIANGLE_LEN));
        if next <= pos || next >= body.len() {
            break;
        }
        pos = next;
    }
    Ok(out)
}

fn parse_one_mesh(body: &[u8], pos: usize) -> Result<MzbMesh> {
    if pos + MESH_RECORD_LEN > body.len() {
        return Err(MzbError::MeshDataOutOfRange {
            offset: pos,
            len: body.len(),
        }
        .into());
    }

    let dword = |o: usize| {
        u32::from_le_bytes([
            body[pos + o],
            body[pos + o + 1],
            body[pos + o + 2],
            body[pos + o + 3],
        ]) as usize
    };
    let word = |o: usize| u16::from_le_bytes([body[pos + o], body[pos + o + 1]]);

    let verts_off = dword(MESH_VERTEX_ARRAY);
    let norms_off = dword(MESH_NORMAL_ARRAY);
    let tris_off = dword(MESH_INDEX_ARRAY);

    let tri_count = word(MESH_TRIANGLE_COUNT) as usize;
    let flags = word(MESH_FLAGS);

    if verts_off >= body.len() || norms_off >= body.len() || tris_off >= body.len() {
        return Err(MzbError::MeshDataOutOfRange {
            offset: verts_off.max(norms_off).max(tris_off),
            len: body.len(),
        }
        .into());
    }
    if !(verts_off <= norms_off && norms_off <= tris_off) {
        return Err(MzbError::CrossedOffsets {
            pos,
            verts: verts_off,
            normals: norms_off,
            tris: tris_off,
        }
        .into());
    }

    let vert_count = (norms_off - verts_off) / VEC3_LEN;
    let norm_count = (tris_off - norms_off) / VEC3_LEN;

    let mut vertices = Vec::with_capacity(vert_count);
    for i in 0..vert_count {
        let o = verts_off + i * VEC3_LEN;
        if o + VEC3_LEN > body.len() {
            break;
        }
        vertices.push(MzbVertex {
            pos: [
                f32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]),
                f32::from_le_bytes([body[o + 4], body[o + 5], body[o + 6], body[o + 7]]),
                f32::from_le_bytes([body[o + 8], body[o + 9], body[o + 10], body[o + 11]]),
            ],
        });
    }

    let mut normals = Vec::with_capacity(norm_count);
    for i in 0..norm_count {
        let o = norms_off + i * VEC3_LEN;
        if o + VEC3_LEN > body.len() {
            break;
        }
        normals.push(MzbNormal {
            n: [
                f32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]),
                f32::from_le_bytes([body[o + 4], body[o + 5], body[o + 6], body[o + 7]]),
                f32::from_le_bytes([body[o + 8], body[o + 9], body[o + 10], body[o + 11]]),
            ],
        });
    }

    let mut triangles = Vec::with_capacity(tri_count);
    let mut triangle_normals = Vec::with_capacity(tri_count);
    let mut tri_info = Vec::with_capacity(tri_count);
    for i in 0..tri_count {
        let o = tris_off + i * MESH_TRIANGLE_LEN;
        if o + MESH_TRIANGLE_LEN > body.len() {
            break;
        }

        let v0_raw = u16::from_le_bytes([body[o], body[o + 1]]);
        let v1_raw = u16::from_le_bytes([body[o + 2], body[o + 3]]);
        let v2_raw = u16::from_le_bytes([body[o + 4], body[o + 5]]);
        let n0_raw = u16::from_le_bytes([body[o + 6], body[o + 7]]);
        let v0 = (v0_raw & TRI_INDEX_MASK) as u32;
        let v1 = (v1_raw & TRI_FLAGGED_INDEX_MASK) as u32;
        let v2 = (v2_raw & TRI_FLAGGED_INDEX_MASK) as u32;
        let n0 = (n0_raw & TRI_INDEX_MASK) as u32;
        let m0 = ((v0_raw >> 15) & 1) as u8;
        let m1 = ((v1_raw >> 15) & 1) as u8;
        let m2 = ((v2_raw >> 15) & 1) as u8;
        let m3 = ((n0_raw >> 15) & 1) as u8;
        let terrain = m0 | (m1 << 1) | (m2 << 2) | (m3 << 3);
        let is_invalid = (v1_raw & TRI_SECOND_WORD_FLAG) != 0;
        let camera_transparent = (v2_raw & TRI_CAMERA_TRANSPARENT) != 0;
        triangles.push([v0, v1, v2]);
        triangle_normals.push(n0);
        tri_info.push(MzbTriangleInfo {
            terrain,
            is_invalid,
            camera_transparent,
        });
    }

    Ok(MzbMesh {
        vertices,
        normals,
        triangles,
        triangle_normals,
        tri_info,
        flags,
    })
}

pub fn parse_all(encrypted_body: &[u8]) -> Result<(MzbHeader, Vec<MzbMesh>)> {
    let plain = decrypt(encrypted_body)?;
    let header = MzbHeader::parse(&plain)?;
    let meshes = parse_meshes(&plain, &header)?;
    Ok((header, meshes))
}

#[derive(Debug, Clone, Copy)]
pub struct MzbPlacement {
    pub geometry_offset: u32,

    pub transform: [f32; 16],

    pub doesnt_block_los: bool,

    pub flip_winding: bool,

    pub grid_x: u16,
    pub grid_y: u16,

    pub water_height: Option<f32>,

    /// `CollisionObjectData::something2`: the sub-area whose interior stands in
    /// for this object, `0` for ordinary zone collision. Feed it to
    /// [`MzbPlacement::collides_in`].
    pub sub_area_link: u32,

    /// Slot in the zone's collision-object array, which is index-parallel to
    /// [`parse_mmb_placements`]'s table. `None` on the legacy short record, whose
    /// stride this parser does not know.
    ///
    /// [`parse_placements`] emits one entry per (object, mesh) pair, so several
    /// entries share an index; dedupe on it before treating entries as objects.
    pub object_index: Option<u32>,
}

impl MzbPlacement {
    /// Whether this collision object is solid while `active_sub_area` is swapped
    /// in — the collision-side twin of [`MmbRenderType::classify`], sharing its
    /// predicate so the shell can never be drawn and solid at once.
    pub fn collides_in(&self, active_sub_area: Option<u32>) -> bool {
        !is_suppressed_placeholder(self.sub_area_link, active_sub_area)
    }
}

pub fn parse_mesh_at(body: &[u8], offset: usize) -> Result<MzbMesh> {
    parse_one_mesh(body, offset)
}

fn dword(body: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([body[off], body[off + 1], body[off + 2], body[off + 3]])
}

/// Every collision object's [`MzbPlacement::sub_area_link`], in
/// `CollisionDataHeader::SomeOffset` order — the array [`MzbPlacement::object_index`]
/// indexes, itself index-parallel to [`parse_mmb_placements`]. Empty on the legacy
/// short record, which carries no link.
///
/// This is the whole table; the grid walk [`parse_placements`] does reaches only the
/// objects some cell references, and in zone 289 that is six sub-areas short.
pub fn collision_object_sub_area_links(body: &[u8], header: &MzbHeader) -> Result<Vec<u32>> {
    let Some(mt) = collision_header(body, header)? else {
        return Ok(Vec::new());
    };
    if !header.has_collision_object_tail() {
        return Ok(Vec::new());
    }
    let base = dword(body, mt + COLL_OBJECT_ARRAY_OFFSET) as usize;
    let count = dword(body, mt + COLL_OBJECT_ARRAY_COUNT) as usize;
    if count == 0 {
        return Ok(Vec::new());
    }
    let end = base.saturating_add(count.saturating_mul(COLL_OBJECT_RECORD_LEN));
    if base == 0 || end > body.len() {
        return Err(MzbError::CollisionDataOutOfRange {
            offset: base,
            len: body.len(),
        }
        .into());
    }
    Ok((0..count)
        .map(|i| {
            dword(
                body,
                base + i * COLL_OBJECT_RECORD_LEN + COLL_OBJECT_SUB_AREA_LINK,
            )
        })
        .collect())
}

pub fn parse_placements(body: &[u8], header: &MzbHeader) -> Result<Vec<MzbPlacement>> {
    let Some(mt) = collision_header(body, header)? else {
        return Ok(Vec::new());
    };
    let grid_offset = u32::from_le_bytes([
        body[mt + COLL_GRID_DATA_OFFSET],
        body[mt + COLL_GRID_DATA_OFFSET + 1],
        body[mt + COLL_GRID_DATA_OFFSET + 2],
        body[mt + COLL_GRID_DATA_OFFSET + 3],
    ]) as usize;
    if grid_offset == 0 || grid_offset >= body.len() {
        return Ok(Vec::new());
    }
    let object_array_offset = u32::from_le_bytes([
        body[mt + COLL_OBJECT_ARRAY_OFFSET],
        body[mt + COLL_OBJECT_ARRAY_OFFSET + 1],
        body[mt + COLL_OBJECT_ARRAY_OFFSET + 2],
        body[mt + COLL_OBJECT_ARRAY_OFFSET + 3],
    ]) as usize;
    let object_array_count = u32::from_le_bytes([
        body[mt + COLL_OBJECT_ARRAY_COUNT],
        body[mt + COLL_OBJECT_ARRAY_COUNT + 1],
        body[mt + COLL_OBJECT_ARRAY_COUNT + 2],
        body[mt + COLL_OBJECT_ARRAY_COUNT + 3],
    ]) as usize;

    let gw = header.grid_cells_x();
    let gh = header.grid_cells_z();
    if gw == 0 || gh == 0 {
        return Ok(Vec::new());
    }

    let mut out: Vec<MzbPlacement> = Vec::new();

    for y in 0..gh {
        for x in 0..gw {
            let cell_ptr_off = grid_offset.saturating_add((y * gw + x) * 4);
            if cell_ptr_off + 4 > body.len() {
                continue;
            }
            let entry_off = u32::from_le_bytes([
                body[cell_ptr_off],
                body[cell_ptr_off + 1],
                body[cell_ptr_off + 2],
                body[cell_ptr_off + 3],
            ]) as usize;
            if entry_off == 0 || entry_off >= body.len() {
                continue;
            }

            let mut entries: Vec<u32> = Vec::new();
            let mut cur = entry_off;

            for _ in 0..4096 {
                if cur + 4 > body.len() {
                    break;
                }
                let v =
                    u32::from_le_bytes([body[cur], body[cur + 1], body[cur + 2], body[cur + 3]]);
                if v == 0 {
                    break;
                }
                entries.push(v);
                cur += 4;
            }
            if entries.is_empty() {
                continue;
            }

            let mut i = 1usize;
            while i + 1 < entries.len() {
                let mat_off = entries[i] as usize;
                let geo_off = entries[i + 1] as usize;
                i += 2;

                if mat_off == 0 || geo_off == 0 {
                    continue;
                }
                if mat_off + COLL_OBJECT_MATRIX_LEN > body.len()
                    || geo_off + MESH_RECORD_LEN > body.len()
                {
                    continue;
                }

                let mut m = [0.0f32; 16];
                for (k, slot) in m.iter_mut().enumerate() {
                    let o = mat_off + k * 4;
                    *slot = f32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
                }

                let det = m[0] * (m[5] * m[10] - m[9] * m[6]) - m[4] * (m[1] * m[10] - m[9] * m[2])
                    + m[8] * (m[1] * m[6] - m[5] * m[2]);

                let flags = u16::from_le_bytes([
                    body[geo_off + MESH_FLAGS],
                    body[geo_off + MESH_FLAGS + 1],
                ]);

                let has_tail = header.has_collision_object_tail()
                    && mat_off + COLL_OBJECT_RECORD_LEN <= body.len();

                let sub_area_link = if has_tail {
                    let o = mat_off + COLL_OBJECT_SUB_AREA_LINK;
                    u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]])
                } else {
                    0
                };

                let object_index = has_tail
                    .then(|| mat_off.checked_sub(object_array_offset))
                    .flatten()
                    .filter(|d| d % COLL_OBJECT_RECORD_LEN == 0)
                    .map(|d| d / COLL_OBJECT_RECORD_LEN)
                    .filter(|i| *i < object_array_count)
                    .map(|i| i as u32);

                let water_off = mat_off + COLL_OBJECT_FLAGS;
                let water_height =
                    if header.has_collision_object_tail() && water_off + 4 <= body.len() {
                        let raw = i32::from_le_bytes([
                            body[water_off],
                            body[water_off + 1],
                            body[water_off + 2],
                            body[water_off + 3],
                        ]);
                        let signed_26 = (raw.wrapping_shl(6)) >> 10;
                        if signed_26 == 0 {
                            None
                        } else {
                            Some(signed_26 as f32 / 1024.0)
                        }
                    } else {
                        None
                    };

                out.push(MzbPlacement {
                    geometry_offset: geo_off as u32,
                    transform: m,
                    doesnt_block_los: (flags & 1) != 0,
                    flip_winding: det < 0.0,
                    grid_x: x as u16,
                    grid_y: y as u16,
                    water_height,
                    sub_area_link,
                    object_index,
                });
            }
        }
    }

    Ok(out)
}

#[inline]
pub fn apply_placement(m: &[f32; 16], v: [f32; 3]) -> [f32; 3] {
    let (x, y, z) = (v[0], v[1], v[2]);
    [
        m[0] * x + m[4] * y + m[8] * z + m[12],
        m[1] * x + m[5] * y + m[9] * z + m[13],
        m[2] * x + m[6] * y + m[10] * z + m[14],
    ]
}

/// research/XIClient/src/XIClient/include/Resource/Derived/ZoneBlockFormat.h ZoneBlockHeader
/// — `PositionedMeshBlockData`, one 0x64-byte record per placed MMB.
/// Corroborated field-by-field by research/cexi-docs/zone/format.md "ZoneDef placement (`0x1C`)".
const PL_MESH_BLOCK_NAME: usize = 0x00;
const PL_TRANSLATION: usize = 0x10;
const PL_ROTATION: usize = 0x1C;
const PL_SCALING: usize = 0x28;
const PL_BLOCK_ID: usize = 0x34;
const PL_LOD_NEAR: usize = 0x38;
const PL_LOD_MID: usize = 0x3C;
const PL_LOD_FAR: usize = 0x40;
const PL_SPECIAL_EFFECTS: usize = 0x46;
const PL_AREA_RESOURCE_ID: usize = 0x4C;
const PL_SUB_AREA_LINK: usize = 0x50;
const PL_LIGHT_REFERENCES: usize = 0x54;
/// ZoneBlockFormat.h — `LIGHT_REFERENCE_COUNT`. Retail binds these four into
/// D3D light slots 2-5 (ZoneRenderer.cpp ZoneRenderer::SetLightIndices `SetLightIndices`).
pub const LIGHT_REFERENCE_COUNT: usize = 4;

/// FourCC naming an `XiArea`, stored little-endian at placement offset 0x4C
/// (ZoneBlockFormat.h PositionedMeshBlockData) and resolved by `XiArea::FindAreaByFourCC`
/// (XiArea.cpp XiArea::FindAreaByFourCC). `0` means "no area": retail's `FindAreaByFourCCAndGet*`
/// accessors short-circuit to the zone-wide environment (XiArea.cpp XiArea::FindAreaByFourCCAndGetAmbient,
/// :434, :284).
pub type AreaResourceId = u32;

/// The [`AreaResourceId`] a 4-byte DAT directory name denotes. Retail reaches an
/// area's own environment container by searching the zone container for this
/// FourCC (`SearchCurrentContainer(Rmp, fourCC)`, XiArea.cpp XiArea::XiArea), so the
/// placement field and the directory name are the same bytes.
pub fn area_resource_id_from_dir_name(name: &[u8; 4]) -> AreaResourceId {
    u32::from_le_bytes(*name)
}

#[derive(Debug, Clone, Copy)]
pub struct MmbPlacement {
    pub id: [u8; 16],
    pub trans: [f32; 3],

    pub rot: [f32; 3],
    pub scale: [f32; 3],

    /// FourCC. A non-zero `BlockID` makes retail classify the chunk
    /// [`MmbRenderType::Keyed`], which the normal pass never draws
    /// (ZoneRenderer.cpp ZoneRenderer::SetRenderTypes); see [`drawn_placements`] for the second pass
    /// that puts the `_`/`@` families back on screen.
    pub block_id: u32,

    /// Squared and compared against the squared camera distance to pick the
    /// high/mid/low mesh variant — see [`MmbLodThresholds`].
    pub lod_near: f32,
    pub lod_mid: f32,
    pub lod_far: f32,

    /// ZoneBlockFormat.h PositionedMeshBlockData — `SpecialEffects`, the third of the four flag
    /// bytes packed after the LOD triple (:92-95). Bit 0 is
    /// [`MmbPlacement::uses_lod_rendering`].
    pub special_effects: u8,

    /// FourCC of the area this chunk belongs to; drives per-area fog and the
    /// weather diffuse lights (ZoneRenderer.cpp ZoneRenderer::OpenMzb, :1133-1152). Read it through
    /// [`MmbPlacement::effective_area_resource_id`] — retail clears it for
    /// blocks outside the `_`/unkeyed families.
    pub area_resource_id: AreaResourceId,

    /// The sub-area (building interior) whose geometry replaces this placeholder,
    /// 0 when there is none. Retail hides the chunk while that sub-area is the
    /// active collision map — RenderType 1 (ZoneRenderer.cpp ZoneRenderer::SetRenderTypes,
    /// research/cexi-docs/zone/subareas.md "2. The placeholder link (`0x1C` object `0x50`)").
    pub sub_area_link: u32,

    /// 1-based indices into the header's light-binding table; 0 = unused. Zeroed
    /// below [`LIGHT_BINDING_MIN_VERSION`], as retail does
    /// (ZoneRenderer.cpp ZoneRenderer::OpenMzb).
    pub light_references: [u32; LIGHT_REFERENCE_COUNT],
}

/// ZoneLayoutData.cpp ZoneLayoutData::InitUnderscoreAtStructs, :85-86 — retail tests `(unsigned char)BlockID`, i.e.
/// the first character of the little-endian FourCC.
const BLOCK_ID_UNDERSCORE_GROUP: u8 = b'_';
const BLOCK_ID_AT_GROUP: u8 = b'@';

/// `UnderscoreAtStruct::Subchunks` is a fixed array of four
/// (research/XIClient/src/XIClient/include/World/Zone/Terrain/UnderscoreAtStruct.h); members past
/// the fourth are counted by `ZoneLayoutData::InitUnderscoreAtStructs` and then
/// dropped, and `SubchunkCount` is clamped to the array, so nothing can draw or
/// address them.
pub const UNDERSCORE_AT_GROUP_MAX_SUBCHUNKS: usize = 4;

impl MmbPlacement {
    pub fn id_str(&self) -> &str {
        let end = self
            .id
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.id.len());
        std::str::from_utf8(&self.id[..end]).unwrap_or("")
    }

    /// True when this placement joins an `UnderscoreAtStruct` group, which retail
    /// draws in its own pass at the tail of `RenderSubStruct`
    /// (ZoneRenderer.cpp ZoneRenderer::RenderSubStruct, :2240-2269) regardless of `RenderType`.
    pub fn in_underscore_at_group(&self) -> bool {
        let first = self.block_id.to_le_bytes()[0];
        self.block_id != 0 && (first == BLOCK_ID_UNDERSCORE_GROUP || first == BLOCK_ID_AT_GROUP)
    }

    /// ZoneBlockFormat.h — `PositionedMeshBlockData::UsesLodRendering`.
    pub fn uses_lod_rendering(&self) -> bool {
        self.special_effects & SPECIAL_EFFECTS_LOD_RENDERING != 0
    }

    pub fn lod_thresholds(&self) -> MmbLodThresholds {
        MmbLodThresholds::from_placement(self)
    }

    /// The [`AreaResourceId`] this placement is actually bound to.
    ///
    /// ZoneLayoutData.cpp ZoneLayoutData::BuildAreaResourceIDList (`BuildAreaResourceIDList`) — retail *zeroes*
    /// `AreaResourceID` in place on any block whose `BlockID` is both non-zero
    /// and not in the `_` family, so those blocks fall back to the zone-wide
    /// environment even though the record carries an id.
    pub fn effective_area_resource_id(&self) -> AreaResourceId {
        if self.area_resource_id == 0 {
            return 0;
        }
        let first = self.block_id.to_le_bytes()[0];
        if self.block_id == 0 || first == BLOCK_ID_UNDERSCORE_GROUP {
            self.area_resource_id
        } else {
            0
        }
    }
}

/// Distinct [`AreaResourceId`]s a zone's placements bind to, in first-seen order
/// — retail's `ZoneLayoutData::AreaResourceIDList` (ZoneLayoutData.cpp ZoneLayoutData::BuildAreaResourceIDList),
/// one `XiArea` per entry (ZoneRenderer.cpp ZoneRenderer::AllocateAndLinkAreas).
pub fn area_resource_ids(placements: &[MmbPlacement]) -> Vec<AreaResourceId> {
    let mut out: Vec<AreaResourceId> = Vec::new();
    for p in placements {
        let id = p.effective_area_resource_id();
        if id != 0 && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// ZoneBlockFormat.h UsesLodRendering — `SpecialEffects & 0x01`.
const SPECIAL_EFFECTS_LOD_RENDERING: u8 = 0x01;

/// ZoneRenderer.cpp ZoneRenderer::RenderChunk2 — which of the three mesh variants
/// `RenderChunk2` hands to the device for one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MmbLodLevel {
    High = 0,
    Medium = 1,
    Low = 2,
}

impl MmbLodLevel {
    pub const fn mask(self) -> u8 {
        1 << self as u8
    }
}

/// ZoneRenderer.cpp ZoneRenderer::OpenMzb nearDist — retail squares the three `Lod*Distance` floats once
/// while building the `PositionedMeshBlock` and compares them against the squared
/// camera distance, so no square root is taken per chunk per frame. Comparing our
/// linear distance against the raw floats would put every switch at the wrong
/// range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MmbLodThresholds {
    pub near_sq: f32,
    pub mid_sq: f32,
    pub far_sq: f32,
}

impl MmbLodThresholds {
    pub fn from_placement(p: &MmbPlacement) -> Self {
        let near_sq = p.lod_near * p.lod_near;
        Self {
            near_sq,
            // ZoneRenderer.cpp ZoneRenderer::OpenMzb — an authored mid *below* near collapses onto
            // near, emptying the medium band rather than inverting the comparison.
            mid_sq: if p.lod_near > p.lod_mid {
                near_sq
            } else {
                p.lod_mid * p.lod_mid
            },
            far_sq: p.lod_far * p.lod_far,
        }
    }

    /// ZoneRenderer.cpp ZoneRenderer::RenderChunk2. `camera_dist_sq` is measured from the camera eye
    /// to the placement translation (ZoneRenderer.cpp ZoneRenderer::RenderChunk2 val), not from the player.
    pub fn select(&self, camera_dist_sq: f32) -> MmbLodLevel {
        if camera_dist_sq <= self.mid_sq {
            if camera_dist_sq <= self.near_sq {
                MmbLodLevel::High
            } else {
                MmbLodLevel::Medium
            }
        } else {
            MmbLodLevel::Low
        }
    }
}

/// XiArea.cpp XiArea::GetAnotherSomething — `GetAnotherSomething(false)`, the per-zone scale retail
/// multiplies `FarThresholdSquared` by, is 1.0 before the registry graphics-config
/// draw-distance multipliers this client does not model.
pub const ZONE_LOD_FAR_SCALE: f32 = 1.0;

/// ZoneRenderer.cpp ZoneRenderer::RenderChunk, :1057-1064, :1071-1079 — the authored Lod far
/// distance doubles as the draw-distance cull, but only for chunks flagged
/// [`MmbPlacement::uses_lod_rendering`]; every other chunk is culled by the global
/// draw distance instead, which is why the placements authored with `lod_far == 0`
/// do not vanish.
pub fn beyond_lod_far_cull(camera_dist_sq: f32, thresholds: MmbLodThresholds) -> bool {
    camera_dist_sq > ZONE_LOD_FAR_SCALE * thresholds.far_sq
}

/// ZoneRenderer.cpp `InitializeMeshLOD` — a placement whose mesh name ends in
/// one of these swaps that last character to reach its siblings.
const MMB_LOD_SUFFIX_HIGH: u8 = b'h';
const MMB_LOD_SUFFIX_MEDIUM: u8 = b'm';
const MMB_LOD_SUFFIX_LOW: u8 = b'l';

/// The three mesh-block indices one placement can draw, as
/// `InitializeMeshLOD` leaves them. `None` means retail would have a null pointer
/// there and skip the draw entirely (ZoneRenderer.cpp ZoneRenderer::RenderChunk2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MmbLodSet {
    pub high: Option<usize>,
    pub medium: Option<usize>,
    pub low: Option<usize>,
}

impl MmbLodSet {
    pub fn get(&self, level: MmbLodLevel) -> Option<usize> {
        match level {
            MmbLodLevel::High => self.high,
            MmbLodLevel::Medium => self.medium,
            MmbLodLevel::Low => self.low,
        }
    }

    /// Which levels resolve to `index`, as an [`MmbLodLevel::mask`] bitmask.
    pub fn level_mask(&self, index: usize) -> u8 {
        let mut mask = 0;
        for level in [MmbLodLevel::High, MmbLodLevel::Medium, MmbLodLevel::Low] {
            if self.get(level) == Some(index) {
                mask |= level.mask();
            }
        }
        mask
    }

    pub fn distinct_indices(&self) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::with_capacity(3);
        for i in [self.high, self.medium, self.low].into_iter().flatten() {
            if !out.contains(&i) {
                out.push(i);
            }
        }
        out
    }
}

/// ZoneRenderer.cpp `InitializeMeshLOD`, re-expressed over a caller-supplied
/// name lookup so a consumer that resolves duplicate mesh names its own way keeps
/// doing so. The order matters: a found `m` sibling overwrites all three slots,
/// while `h` and `l` overwrite only their own slot and backfill the empty ones.
pub fn resolve_mmb_lod_set_with<F>(placement_id: &str, mut lookup: F) -> MmbLodSet
where
    F: FnMut(&str) -> Option<usize>,
{
    let id = placement_id.trim_end();
    let base = lookup(id);
    let mut set = MmbLodSet {
        high: base,
        medium: base,
        low: base,
    };

    // ZoneRenderer.cpp InitializeMeshLOD — `nameLength` is the index of the last character
    // above ' ', so a blank or single-character name returns before any sibling
    // lookup (:105-106).
    if id.len() < 2 {
        return set;
    }
    let last = id.as_bytes()[id.len() - 1];
    if !matches!(
        last,
        MMB_LOD_SUFFIX_HIGH | MMB_LOD_SUFFIX_MEDIUM | MMB_LOD_SUFFIX_LOW
    ) {
        return set;
    }

    let mut sibling = |suffix: u8| -> Option<usize> {
        let mut bytes = id.as_bytes().to_vec();
        *bytes.last_mut()? = suffix;
        lookup(&String::from_utf8(bytes).ok()?)
    };

    if let Some(m) = sibling(MMB_LOD_SUFFIX_MEDIUM) {
        set = MmbLodSet {
            high: Some(m),
            medium: Some(m),
            low: Some(m),
        };
    }
    if let Some(h) = sibling(MMB_LOD_SUFFIX_HIGH) {
        set.high = Some(h);
        set.medium = set.medium.or(Some(h));
        set.low = set.low.or(Some(h));
    }
    if let Some(l) = sibling(MMB_LOD_SUFFIX_LOW) {
        set.low = Some(l);
        set.high = set.high.or(Some(l));
        set.medium = set.medium.or(Some(l));
    }
    set
}

/// [`resolve_mmb_lod_set_with`] over the same name table [`resolve_mmb_index`] reads.
/// Siblings resolve by exact (or zone-prefixed) name only: retail's
/// `BlockManager.GetByName` is a name equality test, so the trailing-substring
/// ladder `resolve_mmb_indices` falls back on — a Kuluu affordance for placement ids
/// no MMB header spells out — must not be allowed to bind an unrelated mesh as a
/// LOD variant.
pub fn resolve_mmb_lod_set(
    placement_id: &str,
    zone_prefix: &str,
    mmb_asset_names: &[String],
) -> MmbLodSet {
    let id = placement_id.trim_end();
    resolve_mmb_lod_set_with(placement_id, |name| {
        if name == id {
            resolve_mmb_index(name, zone_prefix, mmb_asset_names)
        } else {
            resolve_mmb_index_exact(name, zone_prefix, mmb_asset_names)
        }
    })
}

fn resolve_mmb_index_exact(
    name: &str,
    zone_prefix: &str,
    mmb_asset_names: &[String],
) -> Option<usize> {
    let exact = mmb_asset_names
        .iter()
        .position(|n| n.trim_end() == name.trim_end());
    if exact.is_some() {
        return exact;
    }
    let mut prefixed = String::with_capacity(zone_prefix.len() + name.len());
    prefixed.push_str(zone_prefix);
    prefixed.push_str(name.trim_end());
    mmb_asset_names
        .iter()
        .position(|n| n.trim_end() == prefixed)
}

/// research/XIClient/src/XIClient/source/Rendering/ZoneRenderer.cpp ZoneRenderer::SetRenderTypes
/// — `ZoneRenderer::SetRenderTypes`. The static zone pass draws a chunk only when
/// `RenderType > 1` (ZoneRenderer.cpp ZoneRenderer::DrawHelper quadtree leaf, :2662 flat block list).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MmbRenderType {
    /// `BlockID != 0` — a FourCC-keyed chunk (doors `_*`, elevators `@*`, and the
    /// other RID/sub-model families). Retail hands these to the trigger/sub-model
    /// renderer, so the static pass skips them (ZoneRenderer.cpp ZoneRenderer::SetRenderTypes,
    /// ZoneLayoutData.cpp ZoneLayoutData::InitUnderscoreAtStructs).
    Keyed = 0,
    /// The chunk is the exterior placeholder for the sub-area that is currently
    /// active, so the interior DAT is standing in for it
    /// (ZoneRenderer.cpp ZoneRenderer::SetRenderTypes).
    SuppressedPlaceholder = 1,
    /// Ordinary static zone geometry (ZoneRenderer.cpp ZoneRenderer::SetRenderTypes).
    Static = 2,
}

/// ZoneRenderer.cpp ZoneRenderer::DrawHelper, :2662 — `RenderType > 1`.
const MMB_RENDER_TYPE_DRAW_MIN: u8 = 2;

/// Whether a `sub_area_link` names the sub-area currently swapped in, i.e. whether
/// its owner is the placeholder the interior is standing in for. Retail keeps the
/// active id in `CollisionManager::field_4`, sentinel `-1` for "none"
/// (ZoneRenderer.cpp ZoneRenderer::ZoneRenderer, :666), so a link of `0` — "not a placeholder"
/// (research/cexi-docs/zone/subareas.md "2. The placeholder link (`0x1C` object `0x50`)") — must never match, the way the
/// `-1` sentinel cannot.
///
/// The render pass ([`MmbRenderType::classify`]) and the collision pass
/// ([`MzbPlacement::collides_in`]) share this one predicate.
pub fn is_suppressed_placeholder(sub_area_link: u32, active_sub_area: Option<u32>) -> bool {
    active_sub_area.is_some_and(|a| a != NO_SUB_AREA_LINK && sub_area_link == a)
}

/// A [`MzbPlacement::sub_area_link`] / [`MmbPlacement::sub_area_link`] of `0`:
/// ordinary zone geometry, standing in for no interior
/// (research/cexi-docs/zone/subareas.md "2. The placeholder link (`0x1C` object `0x50`)").
pub const NO_SUB_AREA_LINK: u32 = 0;

impl MmbRenderType {
    /// `active_sub_area` is the sub-area currently swapped in, `None` when the
    /// player is in the open zone.
    pub fn classify(p: &MmbPlacement, active_sub_area: Option<u32>) -> Self {
        let mut rt = MmbRenderType::Static;
        if p.block_id != 0 {
            rt = MmbRenderType::Keyed;
        }
        if is_suppressed_placeholder(p.sub_area_link, active_sub_area) {
            rt = MmbRenderType::SuppressedPlaceholder;
        }
        rt
    }

    pub fn is_drawn(self) -> bool {
        self as u8 >= MMB_RENDER_TYPE_DRAW_MIN
    }
}

/// One `_`/`@` FourCC family of placements — retail's `UnderscoreAtStruct`
/// (research/XIClient/src/XIClient/include/World/Zone/Terrain/UnderscoreAtStruct.h).
///
/// A door entity's `DoorId` FourCC, the DAT directory holding its `open`/`clos`
/// Scheduler routines, and this `four_cc` are the same four bytes, so the group is
/// the join from a door entity to the leaves it swings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnderscoreAtGroup {
    pub four_cc: u32,

    /// Indices into the placement table, in `UnderscoreAtStruct::Subchunks` slot
    /// order — a Scheduler 0x0D stage addresses a leaf by that slot, so the order is
    /// load-bearing, not cosmetic. At most
    /// [`UNDERSCORE_AT_GROUP_MAX_SUBCHUNKS`] entries.
    pub subchunks: Vec<usize>,
}

impl UnderscoreAtGroup {
    /// The FourCC as the four characters that name the DAT directory. `BlockID` is
    /// read little-endian (`MmbPlacement::block_id`), so byte 0 is the leading
    /// `_`/`@`.
    pub fn four_cc_bytes(&self) -> [u8; 4] {
        self.four_cc.to_le_bytes()
    }
}

/// The zone's `_`/`@` FourCC groups, re-expressing
/// `ZoneLayoutData::InitUnderscoreAtStructs`
/// (research/XIClient/src/XIClient/source/World/Zone/Terrain/ZoneLayoutData.cpp).
///
/// Retail walks the placement table once to open a group at each FourCC's first
/// member, then walks it again per group collecting every placement with that
/// `BlockID`; `ZoneRenderer::OpenMzb` fills `positionedBlocks` one-for-one from the
/// record table, so both the group order and the subchunk order are placement-table
/// order.
pub fn underscore_at_groups(placements: &[MmbPlacement]) -> Vec<UnderscoreAtGroup> {
    let mut groups: Vec<UnderscoreAtGroup> = Vec::new();
    let mut group_of: HashMap<u32, usize> = HashMap::new();
    for (i, p) in placements.iter().enumerate() {
        if !p.in_underscore_at_group() {
            continue;
        }
        let slot = *group_of.entry(p.block_id).or_insert_with(|| {
            groups.push(UnderscoreAtGroup {
                four_cc: p.block_id,
                subchunks: Vec::with_capacity(UNDERSCORE_AT_GROUP_MAX_SUBCHUNKS),
            });
            groups.len() - 1
        });
        let group = &mut groups[slot];
        if group.subchunks.len() < UNDERSCORE_AT_GROUP_MAX_SUBCHUNKS {
            group.subchunks.push(i);
        }
    }
    groups
}

/// Per-placement visibility for one MZB, parallel to `placements`.
///
/// Retail reaches a placement through one of two passes, so neither alone is the
/// answer: the static pass keeps `RenderType > 1` (ZoneRenderer.cpp ZoneRenderer::DrawHelper, :2662),
/// and `DrawUnderscoreAtStructs` (ZoneRenderer.cpp ZoneRenderer::RenderSubStruct) then draws the first
/// [`UNDERSCORE_AT_GROUP_MAX_SUBCHUNKS`] members of every `_`/`@` FourCC group
/// without consulting `RenderType`. What is left invisible is therefore the
/// RenderType 0/1 chunks owned by some *other* subsystem — zone-line entrance
/// stand-ins (`en00`, `ent1`), event geometry (`ice1`, `cv10`) and the sub-area
/// placeholders.
pub fn drawn_placements(placements: &[MmbPlacement], active_sub_area: Option<u32>) -> Vec<bool> {
    let mut drawn: Vec<bool> = placements
        .iter()
        .map(|p| MmbRenderType::classify(p, active_sub_area).is_drawn())
        .collect();

    for group in underscore_at_groups(placements) {
        for i in group.subchunks {
            drawn[i] = true;
        }
    }
    drawn
}

pub fn parse_mmb_placements(body: &[u8], header: &MzbHeader) -> Result<Vec<MmbPlacement>> {
    let count = header.node_count as usize;
    if count == 0 {
        return Ok(Vec::new());
    }
    let table_end = MZB_HEADER_LEN.saturating_add(count.saturating_mul(PLACEMENT_RECORD_LEN));
    if table_end > body.len() {
        return Err(MzbError::PlacementTableOutOfRange {
            end: table_end,
            len: body.len(),
        }
        .into());
    }
    let has_lights = header.version >= LIGHT_BINDING_MIN_VERSION;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let off = MZB_HEADER_LEN + i * PLACEMENT_RECORD_LEN;
        let rec = &body[off..off + PLACEMENT_RECORD_LEN];
        let mut id = [0u8; PLACEMENT_NAME_LEN];
        id.copy_from_slice(&rec[PL_MESH_BLOCK_NAME..PL_MESH_BLOCK_NAME + PLACEMENT_NAME_LEN]);
        let f = |o: usize| f32::from_le_bytes([rec[o], rec[o + 1], rec[o + 2], rec[o + 3]]);
        let d = |o: usize| u32::from_le_bytes([rec[o], rec[o + 1], rec[o + 2], rec[o + 3]]);
        let mut light_references = [0u32; LIGHT_REFERENCE_COUNT];
        if has_lights {
            for (k, slot) in light_references.iter_mut().enumerate() {
                *slot = d(PL_LIGHT_REFERENCES + k * 4);
            }
        }
        out.push(MmbPlacement {
            id,
            trans: [
                f(PL_TRANSLATION),
                f(PL_TRANSLATION + 4),
                f(PL_TRANSLATION + 8),
            ],
            rot: [f(PL_ROTATION), f(PL_ROTATION + 4), f(PL_ROTATION + 8)],
            scale: [f(PL_SCALING), f(PL_SCALING + 4), f(PL_SCALING + 8)],
            block_id: d(PL_BLOCK_ID),
            lod_near: f(PL_LOD_NEAR),
            lod_mid: f(PL_LOD_MID),
            lod_far: f(PL_LOD_FAR),
            special_effects: rec[PL_SPECIAL_EFFECTS],
            area_resource_id: d(PL_AREA_RESOURCE_ID),
            sub_area_link: d(PL_SUB_AREA_LINK),
            light_references,
        });
    }
    Ok(out)
}

/// research/XIClient/src/XIClient/include/Resource/Derived/ZoneBlockFormat.h LightBindingEntry
/// — `LightBindingEntry` is `{ int LightID; ManagedLight* Light; char more[68]; }`.
/// Only `LightID` is authored; the rest is runtime state retail fills in place
/// after load, and it measures zero in the shipped files.
const LIGHT_BINDING_ENTRY_LEN: usize = 0x4C;

/// ZoneRenderer.cpp ZoneRenderer::GetOrAllocateLight (`SetupLightBindings`) walks the table for
/// `sizeof(LightPool) / sizeof(LightPool[0])` entries — ZoneRenderer.h sizes
/// `LightPool` at 256.
const LIGHT_BINDING_TABLE_MAX: usize = 256;

/// ZoneRenderer.cpp ZoneRenderer::UpdateBlockLightSettings — `(managedLight->LightID & 0xFF) == 99` drops the
/// binding. 99 is ASCII `c`, the first character of the little-endian FourCC and
/// the prefix of the character-light Generator names.
const LIGHT_ID_CHARACTER_PREFIX: u8 = b'c';

/// FourCC of a light-emitting Generator chunk (`LightID` in
/// Rendering/Light/ManagedLight.h ManagedLight LightID), little-endian like every other DAT FourCC.
pub type LightId = u32;

/// The zone's authored light table: `LightID`s in binding order, so a
/// placement's 1-based [`MmbPlacement::light_references`] index it directly.
/// Empty when the file ships no lighting section (ZoneRenderer.cpp ZoneRenderer::OpenMzb gates on
/// `LightingOffset != 0 && GetFormatVersion() >= 18`).
///
/// Retail reads a fixed [`LIGHT_BINDING_TABLE_MAX`] entries; we stop at the end
/// of the decrypted body instead, which is the same table for every shipped file
/// (measured: DAT 233 fills 251 of the 256 and the tail is zeroed).
pub fn parse_light_bindings(body: &[u8], header: &MzbHeader) -> Vec<LightId> {
    if !header.has_light_bindings() {
        return Vec::new();
    }
    let base = header.lighting_offset as usize;
    let mut out = Vec::new();
    for i in 0..LIGHT_BINDING_TABLE_MAX {
        let off = base.saturating_add(i * LIGHT_BINDING_ENTRY_LEN);
        if off + LIGHT_BINDING_ENTRY_LEN > body.len() {
            break;
        }
        out.push(u32::from_le_bytes([
            body[off],
            body[off + 1],
            body[off + 2],
            body[off + 3],
        ]));
    }
    while out.last() == Some(&0) {
        out.pop();
    }
    out
}

/// The lights retail binds into one chunk's four D3D slots
/// (ZoneRenderer.cpp ZoneRenderer::UpdateBlockLightSettings `UpdateBlockLightSettings`): slot `i` takes
/// `lightBindings[LightReferences[i] - 1]`, and a slot is left dark when
///
/// - the reference is 0 (`LightEnable(.., false)`),
/// - it points past the table (no `ManagedLight` was ever allocated), or
/// - the bound `LightID`'s low byte is 99 — the character-light prefix.
///
/// Static per chunk, so the set never changes with the camera.
pub fn resolve_chunk_lights(
    light_references: &[u32; LIGHT_REFERENCE_COUNT],
    bindings: &[LightId],
) -> [Option<LightId>; LIGHT_REFERENCE_COUNT] {
    let mut out = [None; LIGHT_REFERENCE_COUNT];
    for (slot, &reference) in light_references.iter().enumerate() {
        let Some(index) = (reference as usize).checked_sub(1) else {
            continue;
        };
        let Some(&light_id) = bindings.get(index) else {
            continue;
        };
        if light_id == 0 || light_id.to_le_bytes()[0] == LIGHT_ID_CHARACTER_PREFIX {
            continue;
        }
        out[slot] = Some(light_id);
    }
    out
}

pub fn resolve_mmb_index(
    placement_id: &str,
    zone_prefix: &str,
    mmb_asset_names: &[String],
) -> Option<usize> {
    resolve_mmb_indices(placement_id, zone_prefix, mmb_asset_names)
        .into_iter()
        .next()
}

pub fn resolve_mmb_indices(
    placement_id: &str,
    zone_prefix: &str,
    mmb_asset_names: &[String],
) -> Vec<usize> {
    let exact: Vec<usize> = mmb_asset_names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| (n.trim_end() == placement_id).then_some(i))
        .collect();
    if !exact.is_empty() {
        return exact;
    }

    let mut prefixed = String::with_capacity(zone_prefix.len() + placement_id.len());
    prefixed.push_str(zone_prefix);
    prefixed.push_str(placement_id);
    let pre: Vec<usize> = mmb_asset_names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| (n.trim_end() == prefixed).then_some(i))
        .collect();
    if !pre.is_empty() {
        return pre;
    }

    let id_bytes = placement_id.as_bytes();
    if id_bytes.len() >= 8 {
        let needle = &id_bytes[..8];
        let v: Vec<usize> = mmb_asset_names
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                let t = n.trim_end().as_bytes();
                (t.len() >= 8 && &t[t.len() - 8..] == needle).then_some(i)
            })
            .collect();
        if !v.is_empty() {
            return v;
        }
    }

    if placement_id.len() < 3 {
        return Vec::new();
    }
    mmb_asset_names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.trim_end().ends_with(placement_id).then_some(i))
        .collect()
}

pub fn infer_zone_prefix(mmb_asset_names: &[String]) -> String {
    let mut iter = mmb_asset_names.iter();
    let first = match iter.next() {
        Some(s) => s.as_str(),
        None => return String::new(),
    };
    let mut prefix_len = first.len().min(8);
    for name in iter {
        let cap = name.len().min(prefix_len);
        let common = first
            .as_bytes()
            .iter()
            .zip(name.as_bytes())
            .take(cap)
            .take_while(|(a, b)| a == b)
            .count();
        prefix_len = prefix_len.min(common);
        if prefix_len == 0 {
            break;
        }
    }
    first[..prefix_len].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRI_TERRAIN_BIT: u16 = 0x8000;
    const PLAINTEXT_VERSION: u8 = 0x10;
    const SYNTH_GEOMETRY_OFFSET: u32 = 0x40;
    const SPECIAL_EFFECTS_UNRELATED_BIT: u8 = 0x04;

    fn synth_mzb() -> Vec<u8> {
        let mut buf = vec![0u8; 0x8C];

        let decode_len = 0x8Cu32 | (0x10 << 24);
        buf[0..4].copy_from_slice(&decode_len.to_le_bytes());

        buf[4..8].copy_from_slice(&0u32.to_le_bytes());

        buf[8..12].copy_from_slice(&0x20u32.to_le_bytes());

        buf[0x20..0x24].copy_from_slice(&1u32.to_le_bytes());
        buf[0x24..0x28].copy_from_slice(&0x30u32.to_le_bytes());

        buf[0x30..0x34].copy_from_slice(&0x40u32.to_le_bytes());
        buf[0x34..0x38].copy_from_slice(&0x70u32.to_le_bytes());
        buf[0x38..0x3C].copy_from_slice(&0x7Cu32.to_le_bytes());

        buf[0x3C..0x40].copy_from_slice(&2u32.to_le_bytes());

        let verts: [[f32; 3]; 4] = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ];
        for (i, v) in verts.iter().enumerate() {
            let o = 0x40 + i * 12;
            buf[o..o + 4].copy_from_slice(&v[0].to_le_bytes());
            buf[o + 4..o + 8].copy_from_slice(&v[1].to_le_bytes());
            buf[o + 8..o + 12].copy_from_slice(&v[2].to_le_bytes());
        }

        buf[0x70..0x74].copy_from_slice(&0.0f32.to_le_bytes());
        buf[0x74..0x78].copy_from_slice(&1.0f32.to_le_bytes());
        buf[0x78..0x7C].copy_from_slice(&0.0f32.to_le_bytes());

        let tris: [[u16; 4]; 2] = [
            [TRI_TERRAIN_BIT, 1 | TRI_SECOND_WORD_FLAG, 2, 0],
            [
                0,
                2,
                3 | TRI_CAMERA_TRANSPARENT | TRI_TERRAIN_BIT,
                TRI_TERRAIN_BIT,
            ],
        ];
        for (i, t) in tris.iter().enumerate() {
            let o = 0x7C + i * 8;
            for (j, &val) in t.iter().enumerate() {
                buf[o + j * 2..o + j * 2 + 2].copy_from_slice(&val.to_le_bytes());
            }
        }

        buf
    }

    #[test]
    fn decrypt_plaintext_is_noop() {
        let orig = synth_mzb();
        let mut buf = orig.clone();
        decrypt_in_place(&mut buf).unwrap();
        assert_eq!(
            buf, orig,
            "version below ENCRYPTED_MIN_VERSION bypasses pass 1 entirely"
        );
    }

    #[test]
    fn header_parses_basic_fields() {
        let body = synth_mzb();
        let h = MzbHeader::parse(&body).unwrap();
        assert_eq!(h.version, 0x10);
        assert_eq!(h.key_index, 0x00);
        assert_eq!(h.node_count, 0);
        assert_eq!(h.collision_data_offset, 0x20);
    }

    // Each header dword is read from its own fixed offset. 0x18 in particular is
    // `LightingOffset`, not the placement count it used to be named after.
    #[test]
    fn header_fields_come_from_their_own_offsets() {
        let mut body = synth_mzb();
        body[0x08..0x0C].copy_from_slice(&0x1111_1111u32.to_le_bytes());
        body[0x10..0x14].copy_from_slice(&0x2222_2222u32.to_le_bytes());
        body[0x14..0x18].copy_from_slice(&0x3333_3333u32.to_le_bytes());
        body[0x18..0x1C].copy_from_slice(&0x4444_4444u32.to_le_bytes());
        body[0x1C] = 2;
        body[0x1D] = 0x0C;

        let h = MzbHeader::parse(&body).unwrap();
        assert_eq!(h.collision_data_offset, 0x1111_1111);
        assert_eq!(h.quadtree_or_group_count, 0x2222_2222);
        assert_eq!(h.group_list_offset, 0x3333_3333);
        assert_eq!(h.lighting_offset, 0x4444_4444);
        assert_eq!(h.substructure_type, 2);
        assert_eq!(h.collision_flags, 0x0C);
    }

    // The 15 moving-vehicle zones ship `CollisionDataOffset == 0`. Retail wraps
    // the whole collision path in `if (CollisionDataOffset != 0)`
    // (ZoneRenderer.cpp ZoneRenderer::OpenMzb), so zero is a legal state, not a parse failure — and
    // the terrain bytes at 0x0C..0x10 must never be re-read as the offset. The
    // 0x50505050 below is exactly the garbage the old probe loop produced for
    // zone 46's 80x80-block terrain.
    #[test]
    fn zero_collision_offset_is_legal_and_empty() {
        let mut body = synth_mzb();
        body[0x08..0x0C].copy_from_slice(&0u32.to_le_bytes());
        body[0x0C] = 0x50;
        body[0x0D] = 0x50;
        body[0x0E] = 0x50;
        body[0x0F] = 0x50;

        let h = MzbHeader::parse(&body).unwrap();
        assert_eq!(h.collision_data_offset, 0);
        assert!(!h.has_collision_data());
        assert!(
            parse_meshes(&body, &h).unwrap().is_empty(),
            "no collision section means no meshes, not an error"
        );
        assert_eq!(
            parse_placements(&body, &h).unwrap().len(),
            0,
            "no collision section means no placements, not an error"
        );
    }

    #[test]
    fn out_of_range_collision_offset_still_errors() {
        let mut body = synth_mzb();
        let past_end = (body.len() as u32) + 0x1000;
        body[0x08..0x0C].copy_from_slice(&past_end.to_le_bytes());
        let h = MzbHeader::parse(&body).unwrap();
        assert!(parse_meshes(&body, &h).is_err());
        assert!(parse_placements(&body, &h).is_err());
    }

    // ZoneRenderer.cpp ZoneRenderer::OpenMzb (quadtree) and :383/:518-523 (light bindings).
    #[test]
    fn header_unions_follow_the_format_version() {
        let mut body = synth_mzb();
        body[0x10..0x14].copy_from_slice(&0x1234u32.to_le_bytes());
        body[0x18..0x1C].copy_from_slice(&0x5678u32.to_le_bytes());

        body[3] = QUADTREE_MIN_VERSION - 1;
        let old = MzbHeader::parse(&body).unwrap();
        assert_eq!(old.quadtree_offset(), None);
        assert_eq!(old.group_list_count(), Some(0x1234));

        body[3] = QUADTREE_MIN_VERSION;
        let new = MzbHeader::parse(&body).unwrap();
        assert_eq!(new.quadtree_offset(), Some(0x1234));
        assert_eq!(new.group_list_count(), None);

        body[3] = LIGHT_BINDING_MIN_VERSION - 1;
        assert!(!MzbHeader::parse(&body).unwrap().has_light_bindings());
        body[3] = LIGHT_BINDING_MIN_VERSION;
        assert!(MzbHeader::parse(&body).unwrap().has_light_bindings());
        body[0x18..0x1C].copy_from_slice(&0u32.to_le_bytes());
        assert!(!MzbHeader::parse(&body).unwrap().has_light_bindings());
    }

    #[test]
    fn mesh_table_parses_and_indices_are_masked() {
        let body = synth_mzb();
        let h = MzbHeader::parse(&body).unwrap();
        let meshes = parse_meshes(&body, &h).unwrap();
        assert_eq!(meshes.len(), 1);

        let m = &meshes[0];
        assert_eq!(m.vertices.len(), 4);
        assert_eq!(m.normals.len(), 1);
        assert_eq!(m.triangles.len(), 2);

        assert_eq!(m.vertices[0].pos, [0.0, 0.0, 0.0]);
        assert_eq!(m.vertices[1].pos, [1.0, 0.0, 0.0]);

        assert_eq!(
            m.triangles[0],
            [0, 1, 2],
            "indices: v0 masked with 0x7FFF, v1/v2 with 0x3FFF"
        );
        assert_eq!(m.triangle_normals[0], 0);
        assert_eq!(
            m.tri_info[0].terrain, 0b0001,
            "terrain nibble from v0 top bit"
        );
        assert!(m.tri_info[0].is_invalid, "is_invalid from v1 bit 14");
        assert!(!m.tri_info[0].camera_transparent);

        assert_eq!(m.triangles[1], [0, 2, 3]);
        assert_eq!(
            m.tri_info[1].terrain, 0b1100,
            "terrain nibble composed from v2 + n0 top bits"
        );
        assert!(!m.tri_info[1].is_invalid);
        assert!(
            m.tri_info[1].camera_transparent,
            "camera_transparent from v2 bit 14"
        );
    }

    #[test]
    fn double_sided_skip_tests_the_whole_flags_word() {
        assert!(!double_sided_skip(0, true), "flags == 0 never skips");
        assert!(!double_sided_skip(1, false), "bit clear never skips");
        assert!(double_sided_skip(1, true));
        // The guard that matters: retail tests `Flags != 0`, not `Flags & 1`.
        // `doesnt_block_los` right next door uses `& 1`, so this pins the two
        // apart against a well-meaning "unification".
        assert!(
            double_sided_skip(2, true),
            "any nonzero flags value gates the skip, not just bit 0"
        );
    }

    #[test]
    fn pass2_node_xor_runs() {
        const FILL: u8 = 0xAA;
        let mut buf = vec![0u8; 0x20 + 0x64];
        buf[0..4].copy_from_slice(
            &((u32::from(PLAINTEXT_VERSION) << 24) | MZB_HEADER_LEN as u32).to_le_bytes(),
        );
        buf[4..8].copy_from_slice(&1u32.to_le_bytes());

        buf[8..12].copy_from_slice(&0x20u32.to_le_bytes());
        for b in &mut buf[0x20..0x30] {
            *b = FILL;
        }
        decrypt_in_place(&mut buf).unwrap();
        for b in &buf[0x20..0x30] {
            assert_eq!(
                *b,
                FILL ^ PLACEMENT_NAME_XOR,
                "pass 2 should XOR first 16 bytes of each node with 0x55"
            );
        }

        for b in &buf[0x30..0x20 + 0x64] {
            assert_eq!(*b, 0);
        }
    }

    #[test]
    fn pass1_xor_runs_when_version_is_encrypted() {
        let mut encrypted = vec![0u8; 64];
        encrypted[0..4].copy_from_slice(&((0x1Bu32 << 24) | 64).to_le_bytes());
        encrypted[4..8].copy_from_slice(&0u32.to_le_bytes());

        let original = encrypted.clone();
        let mut any_change = false;
        for seed in 0u8..=0xFF {
            let mut tmp = original.clone();
            tmp[7] = seed;
            decrypt_in_place(&mut tmp).unwrap();
            if tmp[8..] != original[8..] {
                any_change = true;
                break;
            }
        }
        assert!(
            any_change,
            "at least one key_index should produce a pass-1 XOR change"
        );
    }

    #[allow(clippy::identity_op)]
    fn synth_mzb_with_placement() -> Vec<u8> {
        let mut buf = vec![0u8; 0x260];

        let decode_len = (buf.len() as u32) | (0x10u32 << 24);
        buf[0..4].copy_from_slice(&decode_len.to_le_bytes());
        buf[4..8].copy_from_slice(&0u32.to_le_bytes());

        buf[8..12].copy_from_slice(&0x20u32.to_le_bytes());
        buf[0x0C] = 1;
        buf[0x0D] = 1;
        buf[0x0E] = 40;
        buf[0x0F] = 40;

        buf[0x20..0x24].copy_from_slice(&1u32.to_le_bytes());
        buf[0x24..0x28].copy_from_slice(&0x40u32.to_le_bytes());
        buf[0x30..0x34].copy_from_slice(&0x80u32.to_le_bytes());

        buf[0x40..0x44].copy_from_slice(&0x50u32.to_le_bytes());
        buf[0x44..0x48].copy_from_slice(&0x70u32.to_le_bytes());
        buf[0x48..0x4C].copy_from_slice(&0x7Cu32.to_le_bytes());
        buf[0x4C..0x50].copy_from_slice(&2u32.to_le_bytes());

        buf[0x80..0x84].copy_from_slice(&0x210u32.to_le_bytes());

        buf[0x210..0x214].copy_from_slice(&0xDEADu32.to_le_bytes());
        buf[0x214..0x218].copy_from_slice(&0x220u32.to_le_bytes());
        buf[0x218..0x21C].copy_from_slice(&SYNTH_GEOMETRY_OFFSET.to_le_bytes());
        buf[0x21C..0x220].copy_from_slice(&0u32.to_le_bytes());

        let mut m = [0.0f32; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        m[12] = 100.0;
        m[13] = 200.0;
        m[14] = 300.0;
        for (k, v) in m.iter().enumerate() {
            buf[0x220 + k * 4..0x220 + k * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }

        buf
    }

    #[test]
    fn placements_decode_one_cell() {
        let body = synth_mzb_with_placement();
        let h = MzbHeader::parse(&body).unwrap();
        assert_eq!(h.zone_blocks_x, 1);
        assert_eq!(h.zone_blocks_z, 1);
        assert_eq!(h.grid_cells_x(), 10);
        assert_eq!(h.grid_cells_z(), 10);

        let placements = parse_placements(&body, &h).unwrap();
        assert_eq!(
            placements.len(),
            1,
            "exactly one (mat,geo) pair in cell (0,0)"
        );
        let p = placements[0];
        assert_eq!(p.geometry_offset, SYNTH_GEOMETRY_OFFSET);
        assert_eq!(p.grid_x, 0);
        assert_eq!(p.grid_y, 0);
        assert!(
            !p.flip_winding,
            "identity rotation has positive determinant"
        );

        assert_eq!(p.transform[12], 100.0);
        assert_eq!(p.transform[13], 200.0);
        assert_eq!(p.transform[14], 300.0);

        let world = apply_placement(&p.transform, [0.0, 0.0, 0.0]);
        assert_eq!(world, [100.0, 200.0, 300.0]);

        let world = apply_placement(&p.transform, [1.0, 0.0, 0.0]);
        assert_eq!(world, [101.0, 200.0, 300.0]);
    }

    #[test]
    fn placements_empty_when_no_grid() {
        let body = synth_mzb();
        let h = MzbHeader::parse(&body).unwrap();
        let placements = parse_placements(&body, &h).unwrap();
        assert!(placements.is_empty(), "grid_width=0 → no placements");
    }

    // Port Jeuno (DAT 346) ships 80-unit zone blocks, so its placement grid is
    // 20 sub-blocks per block, not the 10 every other zone uses. Hardcoding 10
    // made every cell lookup read the wrong stride, parse_placements returned
    // empty, and the whole zone fell back to unplaced (identity) geometry — no
    // collision under the player anywhere in the zone.
    #[test]
    fn placements_grid_stride_follows_block_width() {
        let mut body = synth_mzb_with_placement();
        body[0x0E] = 80;
        body[0x0F] = 80;

        let h = MzbHeader::parse(&body).unwrap();
        assert_eq!(h.grid_cells_x(), 20);
        assert_eq!(h.grid_cells_z(), 20);

        // Move the only cell pointer from (0,0) to row 1, which sits at index 20
        // under the correct stride and index 10 under the old hardcoded one.
        let grid = 0x80usize;
        body[grid..grid + 4].copy_from_slice(&0u32.to_le_bytes());
        let row = 1usize;
        let cell = grid + row * h.grid_cells_x() * 4;
        body[cell..cell + 4].copy_from_slice(&0x210u32.to_le_bytes());

        let placements = parse_placements(&body, &h).unwrap();
        assert!(
            placements.iter().any(|p| p.grid_x == 0
                && p.grid_y == 1
                && p.geometry_offset == SYNTH_GEOMETRY_OFFSET),
            "row-1 cell must be reached at stride 20, got {:?}",
            placements
                .iter()
                .map(|p| (p.grid_x, p.grid_y))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn placement_flip_winding_on_negative_det() {
        let mut body = synth_mzb_with_placement();

        body[0x220..0x224].copy_from_slice(&(-1.0f32).to_le_bytes());
        let h = MzbHeader::parse(&body).unwrap();
        let placements = parse_placements(&body, &h).unwrap();
        assert_eq!(placements.len(), 1);
        assert!(
            placements[0].flip_winding,
            "negative determinant should set flip_winding"
        );
    }

    #[test]
    fn too_small_errors() {
        let small = vec![0u8; 4];
        let err = decrypt(&small).unwrap_err();
        assert!(matches!(err, DatError::Mzb(_)));
    }

    // `CollisionObjectData::flags` — where the section water height is packed —
    // sits 164 bytes into a record that only exists at full length above the
    // version split (CollisionManager.cpp CollisionManager::PrepareData).
    #[test]
    fn water_height_is_gated_on_the_collision_object_version() {
        let mut body = synth_mzb_with_placement();
        let mat_off = 0x220usize;
        let water_off = mat_off + COLL_OBJECT_FLAGS;
        body.resize(water_off + 4, 0);
        // 0x1000 decodes as ((0x1000 << 6) >> 10) / 1024 = 0.25.
        body[water_off..water_off + 4].copy_from_slice(&0x1000u32.to_le_bytes());

        body[3] = LEGACY_COLLISION_OBJECT_MAX_VERSION;
        let legacy = MzbHeader::parse(&body).unwrap();
        assert_eq!(
            parse_placements(&body, &legacy).unwrap()[0].water_height,
            None,
            "the legacy record is shorter than the flags field"
        );

        body[3] = LEGACY_COLLISION_OBJECT_MAX_VERSION + 1;
        let modern = MzbHeader::parse(&body).unwrap();
        assert_eq!(
            parse_placements(&body, &modern).unwrap()[0].water_height,
            Some(0.25)
        );
    }

    /// `CollisionObjectData::something2` closes the 0xC0-byte record, so it shares
    /// the version gate `flags` is under, and the object's slot is its distance from
    /// `CollisionDataHeader::SomeOffset` in whole records.
    #[test]
    fn collision_sub_area_link_and_object_index_come_from_the_record_tail() {
        const COLL_HEADER: usize = 0x20;
        const MAT_OFF: usize = 0x220;
        const LINK: u32 = 0x1CE;
        const OBJECT_COUNT: u32 = 4;

        let mut body = synth_mzb_with_placement();
        body.resize(MAT_OFF + COLL_OBJECT_RECORD_LEN, 0);

        let array_off = (MAT_OFF - COLL_OBJECT_RECORD_LEN) as u32;
        let o = COLL_HEADER + COLL_OBJECT_ARRAY_OFFSET;
        body[o..o + 4].copy_from_slice(&array_off.to_le_bytes());
        let o = COLL_HEADER + COLL_OBJECT_ARRAY_COUNT;
        body[o..o + 4].copy_from_slice(&OBJECT_COUNT.to_le_bytes());
        let o = MAT_OFF + COLL_OBJECT_SUB_AREA_LINK;
        body[o..o + 4].copy_from_slice(&LINK.to_le_bytes());

        body[3] = LEGACY_COLLISION_OBJECT_MAX_VERSION + 1;
        let modern = MzbHeader::parse(&body).unwrap();
        let p = parse_placements(&body, &modern).unwrap()[0];
        assert_eq!(p.sub_area_link, LINK);
        assert_eq!(p.object_index, Some(1));
        assert!(
            !p.collides_in(Some(LINK)),
            "the shell yields to its interior"
        );
        assert!(p.collides_in(Some(LINK + 1)));
        assert!(p.collides_in(None));

        body[3] = LEGACY_COLLISION_OBJECT_MAX_VERSION;
        let legacy = MzbHeader::parse(&body).unwrap();
        let p = parse_placements(&body, &legacy).unwrap()[0];
        assert_eq!(p.sub_area_link, 0);
        assert_eq!(p.object_index, None);
        assert!(p.collides_in(Some(LINK)));
    }

    /// A record whose slot would fall outside `SomeCount`, or land mid-record, is
    /// not an object of this array and must not be reported as one.
    #[test]
    fn an_object_index_outside_the_declared_array_is_none() {
        const COLL_HEADER: usize = 0x20;
        const MAT_OFF: usize = 0x220;

        let mut body = synth_mzb_with_placement();
        body.resize(MAT_OFF + COLL_OBJECT_RECORD_LEN, 0);
        body[3] = LEGACY_COLLISION_OBJECT_MAX_VERSION + 1;

        let set_array = |body: &mut Vec<u8>, off: u32, count: u32| {
            let o = COLL_HEADER + COLL_OBJECT_ARRAY_OFFSET;
            body[o..o + 4].copy_from_slice(&off.to_le_bytes());
            let o = COLL_HEADER + COLL_OBJECT_ARRAY_COUNT;
            body[o..o + 4].copy_from_slice(&count.to_le_bytes());
        };
        let index = |body: &Vec<u8>| {
            let h = MzbHeader::parse(body).unwrap();
            parse_placements(body, &h).unwrap()[0].object_index
        };

        set_array(&mut body, (MAT_OFF - COLL_OBJECT_RECORD_LEN) as u32, 1);
        assert_eq!(index(&body), None, "slot 1 of a 1-object array");

        set_array(&mut body, (MAT_OFF - COLL_OBJECT_RECORD_LEN + 4) as u32, 4);
        assert_eq!(index(&body), None, "not a whole number of records");

        set_array(&mut body, MAT_OFF as u32 + 4, 4);
        assert_eq!(index(&body), None, "record before the array base");
    }

    fn synth_mmb_placement_record(version: u8) -> Vec<u8> {
        let mut body = vec![0u8; MZB_HEADER_LEN + PLACEMENT_RECORD_LEN];
        let size_and_version = (body.len() as u32) | ((version as u32) << 24);
        body[0..4].copy_from_slice(&size_and_version.to_le_bytes());
        body[4..8].copy_from_slice(&1u32.to_le_bytes());

        let rec = MZB_HEADER_LEN;
        body[rec..rec + 4].copy_from_slice(b"blk\0");
        let put_f32 = |body: &mut Vec<u8>, o: usize, v: f32| {
            body[rec + o..rec + o + 4].copy_from_slice(&v.to_le_bytes());
        };
        let put_u32 = |body: &mut Vec<u8>, o: usize, v: u32| {
            body[rec + o..rec + o + 4].copy_from_slice(&v.to_le_bytes());
        };
        put_f32(&mut body, PL_TRANSLATION, 1.0);
        put_f32(&mut body, PL_TRANSLATION + 4, 2.0);
        put_f32(&mut body, PL_TRANSLATION + 8, 3.0);
        put_f32(&mut body, PL_ROTATION, 4.0);
        put_f32(&mut body, PL_SCALING, 5.0);
        put_u32(&mut body, PL_BLOCK_ID, 0xAABB_CCDD);
        put_f32(&mut body, PL_LOD_NEAR, 10.0);
        put_f32(&mut body, PL_LOD_MID, 20.0);
        put_f32(&mut body, PL_LOD_FAR, 30.0);
        body[rec + PL_SPECIAL_EFFECTS] =
            SPECIAL_EFFECTS_LOD_RENDERING | SPECIAL_EFFECTS_UNRELATED_BIT;
        put_u32(&mut body, PL_AREA_RESOURCE_ID, 0x1234_5678);
        put_u32(&mut body, PL_SUB_AREA_LINK, 0x1CE);
        for k in 0..LIGHT_REFERENCE_COUNT {
            put_u32(&mut body, PL_LIGHT_REFERENCES + k * 4, (k as u32) + 1);
        }
        body
    }

    #[test]
    fn mmb_placement_reads_the_whole_record() {
        let body = synth_mmb_placement_record(27);
        let h = MzbHeader::parse(&body).unwrap();
        let p = parse_mmb_placements(&body, &h).unwrap();
        assert_eq!(p.len(), 1);
        let p = p[0];
        assert_eq!(p.id_str(), "blk");
        assert_eq!(p.trans, [1.0, 2.0, 3.0]);
        assert_eq!(p.rot[0], 4.0);
        assert_eq!(p.scale[0], 5.0);
        assert_eq!(p.block_id, 0xAABB_CCDD);
        assert_eq!((p.lod_near, p.lod_mid, p.lod_far), (10.0, 20.0, 30.0));
        assert_eq!(
            p.special_effects,
            SPECIAL_EFFECTS_LOD_RENDERING | SPECIAL_EFFECTS_UNRELATED_BIT
        );
        assert!(p.uses_lod_rendering());
        assert_eq!(p.area_resource_id, 0x1234_5678);
        assert_eq!(p.sub_area_link, 0x1CE);
        assert_eq!(p.light_references, [1, 2, 3, 4]);
    }

    // ZoneRenderer.cpp ZoneRenderer::OpenMzb zeroes LightReferences below version 18 rather
    // than reading whatever those bytes hold.
    #[test]
    fn mmb_placement_light_references_need_version_18() {
        let body = synth_mmb_placement_record(LIGHT_BINDING_MIN_VERSION - 1);
        let h = MzbHeader::parse(&body).unwrap();
        let p = parse_mmb_placements(&body, &h).unwrap();
        assert_eq!(p[0].light_references, [0; LIGHT_REFERENCE_COUNT]);
        assert_eq!(
            p[0].block_id, 0xAABB_CCDD,
            "everything below 0x54 is version-independent"
        );
    }

    fn synth_light_binding_body(version: u8, light_ids: &[&[u8; 4]]) -> Vec<u8> {
        let table_at = MZB_HEADER_LEN + PLACEMENT_RECORD_LEN;
        let entries = light_ids.len().max(LIGHT_BINDING_TABLE_MAX);
        let mut body = vec![0u8; table_at + entries * LIGHT_BINDING_ENTRY_LEN];
        let size_and_version = (body.len() as u32) | ((version as u32) << 24);
        body[0..4].copy_from_slice(&size_and_version.to_le_bytes());
        body[4..8].copy_from_slice(&1u32.to_le_bytes());
        body[HDR_LIGHTING_OFFSET..HDR_LIGHTING_OFFSET + 4]
            .copy_from_slice(&(table_at as u32).to_le_bytes());
        for (i, id) in light_ids.iter().enumerate() {
            let off = table_at + i * LIGHT_BINDING_ENTRY_LEN;
            body[off..off + 4].copy_from_slice(*id);
        }
        body
    }

    #[test]
    fn light_binding_table_reads_ids_at_the_entry_stride() {
        let body = synth_light_binding_body(27, &[b"li12", b"l421", b"lmb0"]);
        let h = MzbHeader::parse(&body).unwrap();
        assert!(h.has_light_bindings());
        assert_eq!(
            parse_light_bindings(&body, &h),
            vec![
                u32::from_le_bytes(*b"li12"),
                u32::from_le_bytes(*b"l421"),
                u32::from_le_bytes(*b"lmb0"),
            ],
            "one LightID per 0x4C-byte LightBindingEntry, zero tail dropped"
        );
    }

    #[test]
    fn light_binding_table_needs_offset_and_version_18() {
        let mut body = synth_light_binding_body(LIGHT_BINDING_MIN_VERSION - 1, &[b"li12"]);
        let old = MzbHeader::parse(&body).unwrap();
        assert!(parse_light_bindings(&body, &old).is_empty());

        body[3] = LIGHT_BINDING_MIN_VERSION;
        let modern = MzbHeader::parse(&body).unwrap();
        assert_eq!(parse_light_bindings(&body, &modern).len(), 1);

        body[HDR_LIGHTING_OFFSET..HDR_LIGHTING_OFFSET + 4].copy_from_slice(&0u32.to_le_bytes());
        let no_section = MzbHeader::parse(&body).unwrap();
        assert!(parse_light_bindings(&body, &no_section).is_empty());
    }

    // ZoneRenderer.cpp ZoneRenderer::GetOrAllocateLight walks exactly LightPool-many entries; a file long
    // enough to hold more must not grow the table past the pool.
    #[test]
    fn light_binding_table_stops_at_the_light_pool_size() {
        let ids = vec![b"li12"; LIGHT_BINDING_TABLE_MAX + 8];
        let body = synth_light_binding_body(27, &ids);
        let h = MzbHeader::parse(&body).unwrap();
        assert_eq!(
            parse_light_bindings(&body, &h).len(),
            LIGHT_BINDING_TABLE_MAX
        );
    }

    #[test]
    fn chunk_light_references_are_one_based_table_indices() {
        let bindings = [
            u32::from_le_bytes(*b"li12"),
            u32::from_le_bytes(*b"l421"),
            u32::from_le_bytes(*b"lmb0"),
        ];
        assert_eq!(
            resolve_chunk_lights(&[3, 1, 0, 2], &bindings),
            [
                Some(bindings[2]),
                Some(bindings[0]),
                None,
                Some(bindings[1])
            ],
            "slot i takes bindings[refs[i] - 1]; reference 0 leaves the slot dark"
        );
    }

    #[test]
    fn chunk_light_reference_past_the_table_stays_dark() {
        let bindings = [u32::from_le_bytes(*b"li12")];
        assert_eq!(
            resolve_chunk_lights(&[2, 99, 1, 0], &bindings),
            [None, None, Some(bindings[0]), None],
            "no ManagedLight was allocated for an entry the table never had"
        );
    }

    #[test]
    fn chunk_light_binding_to_an_empty_entry_stays_dark() {
        let bindings = [0, u32::from_le_bytes(*b"li12")];
        assert_eq!(
            resolve_chunk_lights(&[1, 2, 0, 0], &bindings),
            [None, Some(bindings[1]), None, None],
            "LightID 0 is an unallocated pool slot (ZoneRenderer.cpp ZoneRenderer::GetOrAllocateLight)"
        );
    }

    // ZoneRenderer.cpp ZoneRenderer::UpdateBlockLightSettings — `(LightID & 0xFF) == 99`, i.e. the `c` prefix the
    // character lights carry (DAT 101 ships `c001` next to its `lt0*` lamps).
    #[test]
    fn character_light_ids_are_never_bound_to_a_chunk() {
        let bindings = [
            u32::from_le_bytes(*b"c001"),
            u32::from_le_bytes(*b"lt01"),
            u32::from_le_bytes(*b"cccc"),
        ];
        assert_eq!(
            resolve_chunk_lights(&[1, 2, 3, 0], &bindings),
            [None, Some(bindings[1]), None, None]
        );
    }

    /// Zone ids measured to ship `CollisionDataOffset == 0` — moving-vehicle
    /// zones, `SubstructureType == 2`. The full set is
    /// {1, 3, 46, 47, 58, 59, 60, 220, 221, 223-228}; these three sample it.
    const NO_COLLISION_ZONE_IDS: [u16; 3] = [1, 46, 220];
    /// Lower Jeuno — an ordinary zone with a populated collision grid.
    const COLLISION_ZONE_ID: u16 = 230;

    fn zone_mzb_body(root: &crate::DatRoot, zone_id: u16) -> Vec<u8> {
        let file_id = crate::zone_dat::zone_id_to_mzb_file_id(zone_id).unwrap();
        let loc = root.resolve(file_id).unwrap();
        let bytes = std::fs::read(loc.path_under(root)).unwrap();
        let chunks: Vec<_> = crate::walk(&bytes).filter_map(Result::ok).collect();
        let chunk = chunks
            .iter()
            .find(|c| c.kind == crate::ChunkKind::Mzb as u8)
            .unwrap_or_else(|| panic!("zone {zone_id} has no MZB chunk"));
        decrypt(chunk.data).unwrap()
    }

    #[test]
    fn retail_zero_collision_zones_parse_empty() {
        let Some(root) = crate::archive::open_test_install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };

        for zone_id in NO_COLLISION_ZONE_IDS {
            let body = zone_mzb_body(&root, zone_id);
            let h = MzbHeader::parse(&body).unwrap();
            assert_eq!(h.collision_data_offset, 0, "zone {zone_id}");
            assert_eq!(h.substructure_type, 2, "zone {zone_id}");
            assert!(parse_placements(&body, &h).unwrap().is_empty());
            assert!(parse_meshes(&body, &h).unwrap().is_empty());
        }

        let body = zone_mzb_body(&root, COLLISION_ZONE_ID);
        let h = MzbHeader::parse(&body).unwrap();
        assert!(h.has_collision_data());
        assert_eq!(h.substructure_type, 0);
        assert!(!parse_placements(&body, &h).unwrap().is_empty());
    }

    /// Zones that declare sub-areas, sampling both file-id branches: Southern
    /// San d'Oria, Lower Jeuno, and the high-offset zone 289.
    const SUB_AREA_ZONE_IDS: [u16; 3] = [230, 245, 289];

    /// Gated on a retail install (self-skips without one). The two DAT-side link
    /// tables — `CollisionObjectData::something2` and
    /// `PositionedMeshBlockData::SubAreaLink` — name the same sub-areas, and are
    /// index-parallel, in all 283 shipped zone MZBs. Deliberately not compared
    /// against the `m`-rect set: 12 zones diverge from it.
    #[test]
    fn collision_and_render_sub_area_links_name_the_same_sub_areas() {
        use std::collections::BTreeSet;

        let Some(root) = crate::archive::open_test_install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };

        for zone_id in SUB_AREA_ZONE_IDS {
            let body = zone_mzb_body(&root, zone_id);
            let h = MzbHeader::parse(&body).unwrap();
            let collision = parse_placements(&body, &h).unwrap();
            let rendered = parse_mmb_placements(&body, &h).unwrap();

            let objects = collision_object_sub_area_links(&body, &h).unwrap();
            assert_eq!(
                objects.len(),
                rendered.len(),
                "zone {zone_id} collision-object array and MMB table are the same length"
            );

            let nonzero =
                |v: &[u32]| -> BTreeSet<u32> { v.iter().copied().filter(|l| *l != 0).collect() };
            let rendered_links: Vec<u32> = rendered.iter().map(|p| p.sub_area_link).collect();
            assert!(
                !nonzero(&objects).is_empty(),
                "zone {zone_id} was chosen because it declares sub-areas"
            );
            assert_eq!(
                nonzero(&objects),
                nonzero(&rendered_links),
                "zone {zone_id}"
            );
            assert_eq!(objects, rendered_links, "zone {zone_id} is index-parallel");

            let mut reached: BTreeSet<u32> = BTreeSet::new();
            for p in &collision {
                let i = p.object_index.unwrap_or_else(|| {
                    panic!("zone {zone_id} object at grid {:?}", (p.grid_x, p.grid_y))
                });
                reached.insert(i);
                assert_eq!(
                    objects[i as usize], p.sub_area_link,
                    "zone {zone_id} object {i} disagrees with the object array"
                );
            }
            assert!(
                reached.len() < collision.len(),
                "zone {zone_id} emits one entry per (object, mesh) pair, so callers must dedupe"
            );
        }
    }

    #[test]
    fn mmb_placement_table_past_the_body_errors() {
        let mut body = synth_mmb_placement_record(27);
        body.truncate(body.len() - 1);
        let h = MzbHeader::parse(&body).unwrap();
        assert!(parse_mmb_placements(&body, &h).is_err());
    }

    fn placement(block_id: u32, sub_area_link: u32) -> MmbPlacement {
        MmbPlacement {
            id: [0u8; PLACEMENT_NAME_LEN],
            trans: [0.0; 3],
            rot: [0.0; 3],
            scale: [1.0; 3],
            block_id,
            lod_near: 0.0,
            lod_mid: 0.0,
            lod_far: 0.0,
            special_effects: 0,
            area_resource_id: 0,
            sub_area_link,
            light_references: [0u32; LIGHT_REFERENCE_COUNT],
        }
    }

    fn fourcc(s: &[u8; 4]) -> u32 {
        u32::from_le_bytes(*s)
    }

    fn area_placement(block_id: u32, area: &[u8; 4]) -> MmbPlacement {
        let mut p = placement(block_id, 0);
        p.area_resource_id = fourcc(area);
        p
    }

    // ZoneLayoutData.cpp ZoneLayoutData::BuildAreaResourceIDList — `BlockID == 0 || (char)BlockID == '_'`.
    #[test]
    fn area_binding_survives_only_unkeyed_and_underscore_blocks() {
        assert_eq!(
            area_placement(0, b"ev01").effective_area_resource_id(),
            fourcc(b"ev01")
        );
        assert_eq!(
            area_placement(fourcc(b"_6e1"), b"ev01").effective_area_resource_id(),
            fourcc(b"ev01")
        );
        // ZoneLayoutData.cpp ZoneLayoutData::BuildAreaResourceIDList — retail zeroes the field on any other keyed
        // block, so it draws with the zone-wide environment.
        assert_eq!(
            area_placement(fourcc(b"@abc"), b"ev01").effective_area_resource_id(),
            0
        );
        assert_eq!(
            area_placement(fourcc(b"sea1"), b"ev01").effective_area_resource_id(),
            0
        );
        assert_eq!(placement(0, 0).effective_area_resource_id(), 0);
    }

    // ZoneLayoutData.cpp ZoneLayoutData::BuildAreaResourceIDList isDuplicate — one entry per distinct surviving FourCC, in
    // placement order; retail allocates one XiArea per entry.
    #[test]
    fn area_resource_id_list_is_deduped_in_placement_order() {
        let placements = [
            area_placement(0, b"ev02"),
            area_placement(fourcc(b"sea1"), b"ev09"),
            area_placement(0, b"ev01"),
            area_placement(fourcc(b"_6e1"), b"ev02"),
            placement(0, 0),
        ];
        assert_eq!(
            area_resource_ids(&placements),
            vec![fourcc(b"ev02"), fourcc(b"ev01")]
        );
    }

    #[test]
    fn area_resource_id_reads_a_dat_directory_name_as_the_placement_field() {
        assert_eq!(
            area_resource_id_from_dir_name(b"ev01"),
            area_placement(0, b"ev01").effective_area_resource_id()
        );
    }

    fn lod_placement(near: f32, mid: f32, far: f32) -> MmbPlacement {
        let mut p = placement(0, 0);
        p.lod_near = near;
        p.lod_mid = mid;
        p.lod_far = far;
        p
    }

    // ZoneRenderer.cpp ZoneRenderer::OpenMzb nearDist — the comparison space is squared distance, so the
    // authored (10, 100, 1000) triple that dominates retail's zones switches at
    // 100 / 10 000 / 1 000 000, not at 10 / 100 / 1000.
    #[test]
    fn lod_thresholds_are_squared_distances() {
        let t = lod_placement(10.0, 100.0, 1000.0).lod_thresholds();
        assert_eq!(
            t,
            MmbLodThresholds {
                near_sq: 100.0,
                mid_sq: 10_000.0,
                far_sq: 1_000_000.0,
            }
        );
    }

    // ZoneRenderer.cpp ZoneRenderer::OpenMzb. Retail zones ship this inversion in bulk (e.g. the
    // (25, 0, 60) triple), and the clamp is what keeps it from selecting Medium for
    // everything closer than near.
    #[test]
    fn mid_below_near_collapses_onto_near() {
        let t = lod_placement(25.0, 0.0, 60.0).lod_thresholds();
        assert_eq!(t.mid_sq, t.near_sq);
        assert_eq!(t.near_sq, 625.0);

        assert_eq!(t.select(624.0), MmbLodLevel::High);
        assert_eq!(t.select(625.0), MmbLodLevel::High);
        assert_eq!(t.select(626.0), MmbLodLevel::Low);
    }

    // ZoneRenderer.cpp ZoneRenderer::RenderChunk2 — both comparisons are `<=`, so a chunk sitting
    // exactly on a threshold takes the *more* detailed variant.
    #[test]
    fn lod_band_edges_are_inclusive() {
        let t = lod_placement(10.0, 100.0, 1000.0).lod_thresholds();
        assert_eq!(t.select(0.0), MmbLodLevel::High);
        assert_eq!(t.select(100.0), MmbLodLevel::High);
        assert_eq!(t.select(100.1), MmbLodLevel::Medium);
        assert_eq!(t.select(10_000.0), MmbLodLevel::Medium);
        assert_eq!(t.select(10_000.1), MmbLodLevel::Low);
    }

    // ZoneRenderer.cpp ZoneRenderer::RenderChunk2 — `FarThresholdSquared` is the draw-distance cull
    // only for chunks flagged UsesLodRendering; the ~10k retail placements authored
    // with far == 0 clear the flag and fall through to the global draw distance,
    // so reading far as an unconditional cull would erase them.
    #[test]
    fn far_threshold_culls_only_lod_flagged_chunks() {
        let mut p = lod_placement(10.0, 100.0, 0.0);
        assert!(!p.uses_lod_rendering());

        p.special_effects = SPECIAL_EFFECTS_LOD_RENDERING;
        assert!(p.uses_lod_rendering());
        let t = p.lod_thresholds();
        assert!(beyond_lod_far_cull(0.1, t));
        assert!(!beyond_lod_far_cull(0.0, t));

        let near_prop = lod_placement(10.0, 100.0, 40.0).lod_thresholds();
        assert!(!beyond_lod_far_cull(1_600.0, near_prop));
        assert!(beyond_lod_far_cull(1_600.1, near_prop));
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    // ZoneRenderer.cpp InitializeMeshLOD — the sibling names are the placement name with its
    // last character swapped for h / m / l.
    #[test]
    fn lod_set_binds_the_h_m_l_siblings() {
        let n = names(&["roofh", "roofm", "roofl"]);
        assert_eq!(
            resolve_mmb_lod_set("roofl", "", &n),
            MmbLodSet {
                high: Some(0),
                medium: Some(1),
                low: Some(2),
            }
        );
    }

    // ZoneRenderer.cpp InitializeMeshLOD — a found `m` sibling overwrites all three slots,
    // unlike `h`/`l` which only backfill the empty ones.
    #[test]
    fn medium_sibling_overwrites_every_slot() {
        let n = names(&["roofm"]);
        assert_eq!(
            resolve_mmb_lod_set("roofh", "", &n),
            MmbLodSet {
                high: Some(0),
                medium: Some(0),
                low: Some(0),
            }
        );
    }

    // ZoneRenderer.cpp InitializeMeshLOD — with no `m` sibling the medium slot keeps whatever
    // the base name resolved to, so a two-variant family draws the low mesh in the
    // medium band.
    #[test]
    fn missing_medium_sibling_leaves_the_base_mesh_in_the_medium_band() {
        let n = names(&["roofh", "roofl"]);
        assert_eq!(
            resolve_mmb_lod_set("roofl", "", &n),
            MmbLodSet {
                high: Some(0),
                medium: Some(1),
                low: Some(1),
            }
        );
    }

    // ZoneRenderer.cpp InitializeMeshLOD — a name that does not end in h/m/l, and a name too
    // short to have a swappable last character, never take the sibling path.
    #[test]
    fn lod_suffix_rule_is_a_blind_last_character_swap() {
        let n = names(&["wall", "walm", "walh"]);
        assert_eq!(
            resolve_mmb_lod_set("wall", "", &n),
            MmbLodSet {
                high: Some(2),
                medium: Some(1),
                low: Some(0),
            },
            "retail swaps the last character blind: any name ending in l is a family",
        );

        let n = names(&["door", "doom", "dooh"]);
        assert_eq!(
            resolve_mmb_lod_set("door", "", &n),
            MmbLodSet {
                high: Some(0),
                medium: Some(0),
                low: Some(0),
            }
        );

        let n = names(&["h", "m", "l"]);
        assert_eq!(
            resolve_mmb_lod_set("h", "", &n),
            MmbLodSet {
                high: Some(0),
                medium: Some(0),
                low: Some(0),
            },
            "nameLength <= 0 returns before any sibling lookup",
        );
    }

    // Retail's BlockManager.GetByName is an exact name compare; the trailing-substring
    // ladder resolve_mmb_indices adds must not invent a sibling out of an unrelated
    // mesh whose name merely ends the same way.
    #[test]
    fn siblings_do_not_bind_through_the_fuzzy_name_ladder() {
        let n = names(&["roofh", "towerroofm"]);
        assert_eq!(
            resolve_mmb_index("roofm", "", &n),
            Some(1),
            "the fuzzy ladder does bind roofm to towerroofm by trailing substring",
        );
        assert_eq!(
            resolve_mmb_lod_set("roofh", "", &n),
            MmbLodSet {
                high: Some(0),
                medium: Some(0),
                low: Some(0),
            }
        );
    }

    #[test]
    fn lod_set_reports_distinct_meshes_and_their_bands() {
        let set = MmbLodSet {
            high: Some(7),
            medium: Some(9),
            low: Some(9),
        };
        assert_eq!(set.distinct_indices(), vec![7, 9]);
        assert_eq!(set.level_mask(7), MmbLodLevel::High.mask());
        assert_eq!(
            set.level_mask(9),
            MmbLodLevel::Medium.mask() | MmbLodLevel::Low.mask()
        );
        assert_eq!(set.level_mask(4), 0);
    }

    /// Northern San d'Oria — measured to resolve 183 multi-variant placements, so
    /// the sibling rule is exercised against shipped data rather than only synthetic
    /// names. (Its southern neighbour ships none at all.)
    const LOD_FAMILY_ZONE_ID: u16 = 231;

    // Retail zones actually ship the sibling families this rule depends on; without
    // it every "…l"-named placement renders its low mesh at point-blank range.
    #[test]
    fn retail_zone_has_resolvable_lod_families() {
        let Some(root) = crate::archive::open_test_install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        let file_id = crate::zone_dat::zone_id_to_mzb_file_id(LOD_FAMILY_ZONE_ID).unwrap();
        let bytes = std::fs::read(root.resolve(file_id).unwrap().path_under(&root)).unwrap();
        let chunks: Vec<_> = crate::walk(&bytes).filter_map(Result::ok).collect();

        let mut mmb_names: Vec<String> = Vec::new();
        for c in &chunks {
            if c.kind != crate::ChunkKind::Mmb as u8 {
                continue;
            }
            let Ok(dec) = crate::mmb::decrypt(c.data) else {
                continue;
            };
            let Ok(h) = crate::mmb::MmbHeader::parse(&dec) else {
                continue;
            };
            mmb_names.push(h.zone_mesh_name());
        }
        let prefix = infer_zone_prefix(&mmb_names);

        let mzb = chunks
            .iter()
            .find(|c| c.kind == crate::ChunkKind::Mzb as u8)
            .unwrap();
        let plain = decrypt(mzb.data).unwrap();
        let header = MzbHeader::parse(&plain).unwrap();
        let placements = parse_mmb_placements(&plain, &header).unwrap();

        let multi = placements
            .iter()
            .filter(|p| {
                resolve_mmb_lod_set(p.id_str().trim_end(), &prefix, &mmb_names)
                    .distinct_indices()
                    .len()
                    > 1
            })
            .count();
        assert!(
            multi > 0,
            "no placement in zone {LOD_FAMILY_ZONE_ID} resolved more than one LOD mesh"
        );
    }

    #[test]
    fn set_render_types_matches_retail() {
        const SUB_AREA: u32 = 0x1CE;
        let plain = placement(0, 0);
        assert_eq!(MmbRenderType::classify(&plain, None), MmbRenderType::Static);
        assert!(MmbRenderType::classify(&plain, None).is_drawn());

        let keyed = placement(fourcc(b"en00"), 0);
        assert_eq!(MmbRenderType::classify(&keyed, None), MmbRenderType::Keyed);
        assert!(!MmbRenderType::classify(&keyed, None).is_drawn());

        let placeholder = placement(0, SUB_AREA);
        assert_eq!(
            MmbRenderType::classify(&placeholder, None),
            MmbRenderType::Static,
        );
        assert_eq!(
            MmbRenderType::classify(&placeholder, Some(SUB_AREA + 1)),
            MmbRenderType::Static,
        );
        assert_eq!(
            MmbRenderType::classify(&placeholder, Some(SUB_AREA)),
            MmbRenderType::SuppressedPlaceholder,
        );
        assert!(!MmbRenderType::classify(&placeholder, Some(SUB_AREA)).is_drawn());

        // ZoneRenderer.cpp ZoneRenderer::SetRenderTypes tests BlockID first and the sub-area second, so
        // the sub-area verdict wins when both hold.
        let both = placement(fourcc(b"en00"), SUB_AREA);
        assert_eq!(
            MmbRenderType::classify(&both, Some(SUB_AREA)),
            MmbRenderType::SuppressedPlaceholder,
        );
    }

    #[test]
    fn a_zero_sub_area_link_never_matches_an_active_sub_area() {
        let p = placement(0, 0);
        assert_eq!(MmbRenderType::classify(&p, Some(0)), MmbRenderType::Static);
    }

    #[test]
    fn underscore_and_at_groups_survive_the_gate() {
        let placements = [
            placement(0, 0),
            placement(fourcc(b"_6e0"), 0),
            placement(fourcc(b"@ab1"), 0),
            placement(fourcc(b"ent0"), 0),
        ];
        assert_eq!(
            drawn_placements(&placements, None),
            vec![true, true, true, false],
        );
    }

    #[test]
    fn underscore_group_draws_only_its_first_four_subchunks() {
        let door = fourcc(b"_6e0");
        let placements: Vec<MmbPlacement> = (0..UNDERSCORE_AT_GROUP_MAX_SUBCHUNKS + 2)
            .map(|_| placement(door, 0))
            .collect();
        let drawn = drawn_placements(&placements, None);
        assert_eq!(
            drawn.iter().filter(|d| **d).count(),
            UNDERSCORE_AT_GROUP_MAX_SUBCHUNKS,
        );
        assert!(drawn[..UNDERSCORE_AT_GROUP_MAX_SUBCHUNKS]
            .iter()
            .all(|d| *d));
        assert!(!drawn[UNDERSCORE_AT_GROUP_MAX_SUBCHUNKS..]
            .iter()
            .any(|d| *d));
    }

    #[test]
    fn underscore_at_groups_keep_placement_table_order() {
        let placements = [
            placement(fourcc(b"_6e1"), 0),
            placement(fourcc(b"@ab1"), 0),
            placement(0, 0),
            placement(fourcc(b"ent0"), 0),
            placement(fourcc(b"_6e1"), 0),
            placement(fourcc(b"@ab1"), 0),
        ];
        let groups = underscore_at_groups(&placements);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].four_cc_bytes(), *b"_6e1");
        assert_eq!(groups[0].subchunks, vec![0, 4]);
        assert_eq!(groups[1].four_cc_bytes(), *b"@ab1");
        assert_eq!(groups[1].subchunks, vec![1, 5]);
    }

    #[test]
    fn an_underscore_at_group_stores_only_its_first_four_subchunks() {
        let door = fourcc(b"_6e0");
        let placements: Vec<MmbPlacement> = (0..UNDERSCORE_AT_GROUP_MAX_SUBCHUNKS + 2)
            .map(|_| placement(door, 0))
            .collect();
        let groups = underscore_at_groups(&placements);
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0].subchunks,
            (0..UNDERSCORE_AT_GROUP_MAX_SUBCHUNKS).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_keyed_block_outside_the_underscore_at_families_forms_no_group() {
        let placements = [placement(fourcc(b"ent0"), 0), placement(0, 0)];
        assert!(underscore_at_groups(&placements).is_empty());
    }

    #[test]
    fn a_suppressed_placeholder_that_is_also_a_door_still_draws() {
        const SUB_AREA: u32 = 0x1CE;
        let placements = [placement(fourcc(b"_6e0"), SUB_AREA)];
        assert_eq!(
            MmbRenderType::classify(&placements[0], Some(SUB_AREA)),
            MmbRenderType::SuppressedPlaceholder,
        );
        assert_eq!(drawn_placements(&placements, Some(SUB_AREA)), vec![true]);
    }

    /// Southern San d'Oria's 82 RenderType-0 chunks are 68 door leaves (`_6e*`)
    /// plus 14 zone-line entrance stand-ins (`ent0`..`entd`, meshes
    /// `eml0`..`emlc`). Zone id from vendor/server/sql/zone_settings.sql.
    const RENDER_TYPE_GATE_ZONE_ID: u16 = 230;
    const RENDER_TYPE_GATE_ZONE_HIDDEN: usize = 14;

    #[test]
    fn retail_zone_gates_zone_line_standins_but_keeps_doors() {
        let Some(root) = crate::archive::open_test_install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        let body = zone_mzb_body(&root, RENDER_TYPE_GATE_ZONE_ID);
        let h = MzbHeader::parse(&body).unwrap();
        let placements = parse_mmb_placements(&body, &h).unwrap();
        let drawn = drawn_placements(&placements, None);

        let hidden: Vec<&MmbPlacement> = placements
            .iter()
            .zip(&drawn)
            .filter(|(_, d)| !**d)
            .map(|(p, _)| p)
            .collect();
        assert_eq!(hidden.len(), RENDER_TYPE_GATE_ZONE_HIDDEN);
        for p in &hidden {
            assert_ne!(p.block_id, 0, "{}", p.id_str());
            assert!(!p.in_underscore_at_group(), "{}", p.id_str());
        }
        assert!(placements
            .iter()
            .zip(&drawn)
            .any(|(p, d)| p.in_underscore_at_group() && *d));
    }

    /// The Southern San d'Oria stables door. `_6ey` is at once the zone-DAT
    /// directory holding the `open`/`clos` Scheduler routines, the `BlockID` of the
    /// two `door03` leaves below, and the LSB `npc_list` name whose `pos_x` -7.999
    /// is those leaves' midpoint.
    const DOOR_GROUP_FOUR_CC: &[u8; 4] = b"_6ey";
    const DOOR_GROUP_MESH: &str = "door03";
    /// One group per `/t_sa/door/<fourcc>/` directory in zone 230's DAT.
    const DOOR_GROUP_ZONE_GROUPS: usize = 34;
    const DOOR_GROUP_LEAF_TRANS: [[f32; 3]; 2] = [[-9.4, 1.4, -92.06], [-6.6, 1.4, -92.06]];
    /// The second leaf is the first mirrored through its own Z axis.
    const DOOR_GROUP_LEAF_SCALE_Z: [f32; 2] = [1.0, -1.0];
    /// The authored tenth-of-a-yalm coordinates reach us as f32 (-9.400009 for
    /// -9.4), so compare to well under the 0.1 authoring grid.
    const AUTHORED_COORD_EPS: f32 = 1e-3;

    /// Northern San d'Oria, a second city zone — asserted only for shape, so a
    /// different client era's placement table cannot invalidate the test.
    const SECOND_DOOR_ZONE_ID: u16 = 231;

    fn assert_group_shape(placements: &[MmbPlacement], groups: &[UnderscoreAtGroup]) {
        let mut seen: Vec<u32> = Vec::new();
        for g in groups {
            assert!(!seen.contains(&g.four_cc));
            seen.push(g.four_cc);
            assert!(!g.subchunks.is_empty());
            assert!(g.subchunks.len() <= UNDERSCORE_AT_GROUP_MAX_SUBCHUNKS);
            assert!(g.subchunks.windows(2).all(|w| w[0] < w[1]));
            for &i in &g.subchunks {
                assert_eq!(placements[i].block_id, g.four_cc);
            }
        }
        let distinct = placements
            .iter()
            .filter(|p| p.in_underscore_at_group())
            .map(|p| p.block_id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(groups.len(), distinct.len());
    }

    #[test]
    fn retail_zone_groups_door_leaves_under_their_four_cc() {
        let Some(root) = crate::archive::open_test_install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        let body = zone_mzb_body(&root, RENDER_TYPE_GATE_ZONE_ID);
        let h = MzbHeader::parse(&body).unwrap();
        let placements = parse_mmb_placements(&body, &h).unwrap();
        let groups = underscore_at_groups(&placements);

        assert_eq!(groups.len(), DOOR_GROUP_ZONE_GROUPS);
        assert_group_shape(&placements, &groups);

        let door = groups
            .iter()
            .find(|g| g.four_cc_bytes() == *DOOR_GROUP_FOUR_CC)
            .unwrap();
        assert_eq!(door.subchunks.len(), DOOR_GROUP_LEAF_TRANS.len());
        for (slot, &i) in door.subchunks.iter().enumerate() {
            let p = &placements[i];
            assert_eq!(p.id_str().trim_end(), DOOR_GROUP_MESH);
            for (axis, expected) in DOOR_GROUP_LEAF_TRANS[slot].iter().enumerate() {
                assert!(
                    (p.trans[axis] - expected).abs() < AUTHORED_COORD_EPS,
                    "{p:?}"
                );
            }
            assert_eq!(p.scale[2], DOOR_GROUP_LEAF_SCALE_Z[slot]);
        }
    }

    #[test]
    fn a_second_retail_city_zone_groups_its_doors() {
        let Some(root) = crate::archive::open_test_install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        let body = zone_mzb_body(&root, SECOND_DOOR_ZONE_ID);
        let h = MzbHeader::parse(&body).unwrap();
        let placements = parse_mmb_placements(&body, &h).unwrap();
        let groups = underscore_at_groups(&placements);

        assert!(!groups.is_empty());
        assert_group_shape(&placements, &groups);
    }
}
