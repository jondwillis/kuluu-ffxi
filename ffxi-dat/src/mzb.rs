//! MZB zone-mesh chunk parser.
//!
//! SPDX-License-Identifier: GPL-3.0-or-later
//!
//! # Format notes (clean-room, written from a documented GPL-3 reference)
//!
//! MZB chunks (`kind = 0x1C`) hold a zone's static collision/visual
//! geometry: a flat list of meshes plus a grid + quadtree of placements.
//! Each placement provides a 4×3 affine transform and points at one of
//! the meshes. The renderer-relevant data — vertex/triangle lists — is
//! at the mesh level; the grid + quadtree are spatial-index structures
//! we don't need for an MVP "spawn the geometry" path.
//!
//! Reference: LandSandBoat/FFXI-NavMesh-Builder (GPL-3) — specifically
//! `Common/dat/Types/MZB.cs` (`DecodeMzb`, `ParseMzb`, `ParseMesh`,
//! `ParseGridMesh`). License is workspace-compatible. We read it for
//! *understanding* and write this parser from documented offsets, not
//! by translating its code line-by-line.
//!
//! ## Body layout (after the 16-byte chunk header has already been stripped
//! by `chunk::walk`; what we receive is the chunk *body*)
//!
//! ### Decryption preamble (first 8 bytes; never encrypted)
//!   offset 0..3   `decode_length`: u24 LE — total bytes in the encrypted
//!                                  payload, including the 8-byte preamble.
//!                                  Loaded as `u32 LE & 0x00FFFFFF`.
//!   offset 3      `version`: if `>= 0x1B`, the body is XOR-encrypted.
//!                            Otherwise the body is plaintext and decode
//!                            is a noop.
//!   offset 4..7   `node_count`: u24 LE — count of "node" records at
//!                              offset 0x20 onward, each 0x64 bytes long.
//!                              First 16 bytes of each node have a fixed
//!                              0x55 XOR mask applied during the second
//!                              decrypt pass.
//!   offset 7      `key_index` — XOR with 0xFF, look up in `KEY_TABLE`
//!                              (shared with MMB) to seed the XOR.
//!
//! ### Pass 1 (XOR on encrypted bodies)
//!   pos = 8
//!   key = KEY_TABLE[data[7] ^ 0xFF]
//!   key_count = 0
//!   while pos < decode_length:
//!       xor_len = ((key >> 4) & 7) + 16     // 16..23 bytes
//!       if (key & 1) == 1 and pos + xor_len < decode_length:
//!           XOR data[pos..pos+xor_len] with 0xFF
//!       key += ++key_count
//!       pos += xor_len
//!
//! ### Pass 2 (always; key-independent 0x55 mask on node-record heads)
//!   for i in 0..node_count:
//!       for j in 0..16:
//!           data[0x20 + i*0x64 + j] ^= 0x55
//!
//! ### Plaintext layout
//!   offset 0x08      `mesh_table_offset`: i32 LE — absolute offset (from
//!                    chunk-body start) of the mesh table. May be zero on
//!                    "ship" zones that have a different leading layout;
//!                    scan forward in 4-byte steps for the first nonzero
//!                    value.
//!   offset 0x0c..0x0d   grid_width, grid_height: u8 each (× 10 in usage)
//!   offset 0x10      `quadtree_offset`: i32 LE
//!   offset 0x14      `maplist_offset`: i32 LE
//!   offset 0x18      `maplist_count`: i32 LE
//!
//!   At `mesh_table_offset`:
//!     u32 LE   `mesh_count`
//!     u32 LE   `mesh_data_offset`     (absolute, into chunk body)
//!     ...
//!     u32 LE at meshtbl+0x10   `grid_offset`   (absolute)
//!
//!   At `mesh_data_offset`, `mesh_count` mesh records. Each record:
//!     offset 0x00   `vertices_offset` (i32 LE; absolute)
//!     offset 0x04   `normals_offset`  (i32 LE; absolute)
//!     offset 0x08   `triangles_offset` (i32 LE; absolute)
//!     offset 0x0c   `triangle_count`  (i32 LE)
//!     offset 0x0e..0x10   `flags` (i16 LE) — bit 0 = doesn't block LoS
//!     ...payload (record stride not directly stored; derived as
//!        `triangles_offset + triangle_count * 8` per the reference,
//!        i.e. 4 u16s per triangle).
//!
//!   Vertex block (at vertices_offset): packed f32 xyz triples, 12 B each.
//!     Vertex count is `(normals_offset - vertices_offset) / 12`.
//!   Normal  block (at normals_offset):  packed f32 xyz triples, 12 B each.
//!     Normal count is `(triangles_offset - normals_offset) / 12`.
//!   Triangle block (at triangles_offset): `triangle_count` × 4 u16 LE.
//!     The four u16s are `[v0, v1, v2, n0]`. The top 2 bits of each u16
//!     are flag bits — **mask with `& 0x3FFF`** to get the actual index.
//!     `n0` indexes the normal table (one normal per triangle, not per
//!     vertex). Winding may flip per the determinant of the placement's
//!     3x3 rotation; that's a placement-level concern, not part of
//!     decoding the mesh itself.
//!
//! ## What this parser covers
//!
//! Stage A (this file): decryption + the *mesh-list* path (vertices,
//! triangles, per-tri normals). Sufficient to render the un-placed mesh
//! library — every chunk of geometry the zone uses, sitting at the
//! origin. Useful for sanity-checking geometry; not yet a placed scene.
//!
//! Stage B (this file): grid + per-cell placements. Each non-zero
//! cell in the `(grid_width*10) × (grid_height*10)` grid points at a
//! null-terminated list of u32 entries. Entry 0 is a packed
//! cell-metadata word (bit-packed xx/yy/flags — irrelevant for placement
//! decode). Subsequent entries come in pairs `(matrix_offset,
//! geometry_offset)`:
//!   - `matrix_offset` → 16 consecutive f32s, a 4×4 row-major affine
//!     where column 3 is translation:
//!       p_world.x = m[0]*x + m[4]*y + m[8]*z  + m[12]
//!       p_world.y = m[1]*x + m[5]*y + m[9]*z  + m[13]
//!       p_world.z = m[2]*x + m[6]*y + m[10]*z + m[14]
//!     (i.e. m[0..4]=column 0, m[12..16]=column 3 = translation).
//!   - `geometry_offset` → an MzbMesh record (same on-disk layout as
//!     the mesh-library entries). Many grid cells can reuse the same
//!     geometry_offset — that's how FFXI shares one stair template
//!     across dozens of stairwells. Dedupe by offset when baking GPU
//!     meshes.
//!
//! Quadtree decode (visibility BVH at `quadtree_offset`) is *not*
//! placement-related — it's a render-culling structure that points back
//! at grid cells by index. Skipped.
//!
//! ## Coord-system note
//!
//! Both the mesh vertices and the placement matrix are in raw MZB
//! coordinates (FFXI client space). The viewer converts to Bevy via
//! `ffxi_to_bevy(p) = Vec3(p.x, p.z, -p.y)`. Apply that mapping at the
//! placement-output boundary: transform the vertex by the matrix in
//! MZB-space first, then map the world-space result with
//! `ffxi_to_bevy`. Doing it the other way around (mapping vertices to
//! Bevy and then multiplying by the MZB-space matrix) would compose
//! axes incorrectly.
//!
//! ## Winding
//!
//! Per the reference: if the 3×3 rotation part of the matrix has a
//! negative determinant, triangle winding is reversed at instantiation.
//! We surface the determinant sign as `flip_winding` so the renderer
//! can swap v0/v2 (or rely on Bevy's two-sided material — which is
//! cheaper and what `dat_mzb.rs` already does via `cull_mode: None`).

use crate::{DatError, Result};

// MMB and MZB share the same primary XOR key table. Re-use it.
use crate::mmb::keys::KEY_TABLE;

/// Errors specific to MZB decoding/parsing.
#[derive(Debug, thiserror::Error)]
pub enum MzbError {
    #[error("MZB body too small: need at least {needed} bytes, got {actual}")]
    TooSmall { needed: usize, actual: usize },
    #[error("MZB mesh table offset {offset} out of range (body is {len} bytes)")]
    MeshTableOutOfRange { offset: usize, len: usize },
    #[error("MZB mesh-data offset {offset} out of range (body is {len} bytes)")]
    MeshDataOutOfRange { offset: usize, len: usize },
    #[error("MZB no mesh table found (all zero offsets in the first 64 bytes)")]
    NoMeshTable,
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

/// Decrypt an MZB body in place. The first 8 bytes (preamble) are never
/// encrypted; bytes from offset 8 onward are XOR-masked when
/// `data[3] >= 0x1B`. The 16-byte head of each "node" record is
/// unconditionally masked with 0x55 in a second pass.
///
/// Idempotent only when the body was already plaintext (`data[3] < 0x1B`);
/// running the XOR pass twice on real encrypted data does *not* round-trip
/// because the per-step `xorLength` depends on the running `key`, which
/// drifts when we touch already-decrypted bytes. Always start from the
/// on-disk encrypted blob.
pub fn decrypt_in_place(data: &mut [u8]) -> Result<()> {
    if data.len() < 8 {
        return Err(MzbError::TooSmall {
            needed: 8,
            actual: data.len(),
        }
        .into());
    }

    let decode_length =
        (u32::from_le_bytes([data[0], data[1], data[2], data[3]]) & 0x00FF_FFFF) as usize;
    let node_count =
        (u32::from_le_bytes([data[4], data[5], data[6], data[7]]) & 0x00FF_FFFF) as usize;

    // Pass 1: stride XOR
    if data[3] >= 0x1B {
        let seed_idx = (data[7] ^ 0xFF) as usize;
        let mut key: i32 = KEY_TABLE[seed_idx] as i32;
        let mut key_count: i32 = 0;
        let mut pos = 8usize;

        let end = decode_length.min(data.len());

        while pos < end {
            let xor_len = (((key >> 4) & 7) as usize) + 16;
            if (key & 1) == 1 && pos + xor_len < end {
                for b in &mut data[pos..pos + xor_len] {
                    *b ^= 0xFF;
                }
            }
            key_count = key_count.wrapping_add(1);
            key = key.wrapping_add(key_count);
            pos = pos.saturating_add(xor_len);
        }
    }

    // Pass 2: per-node 16-byte head ^= 0x55
    for i in 0..node_count {
        let base = 0x20usize.saturating_add(i.saturating_mul(0x64));
        let end = base.saturating_add(16);
        if end > data.len() {
            break; // Tolerate truncation — last node may be partial in corrupt files.
        }
        for b in &mut data[base..end] {
            *b ^= 0x55;
        }
    }

    Ok(())
}

/// Return a fresh decrypted copy of an MZB body.
pub fn decrypt(data: &[u8]) -> Result<Vec<u8>> {
    let mut buf = data.to_vec();
    decrypt_in_place(&mut buf)?;
    Ok(buf)
}

/// Parsed MZB top-level header (the few fields we currently consume).
///
/// All offsets are relative to the chunk-body start (the slice passed
/// to `parse`).
#[derive(Debug, Clone, Copy)]
pub struct MzbHeader {
    pub decode_length: u32,
    pub node_count: u32,
    pub version: u8,
    pub key_index: u8,
    pub grid_width: u8,
    pub grid_height: u8,
    /// Offset to the mesh-table record (which itself contains
    /// `mesh_count` + `mesh_data_offset`).
    pub mesh_table_offset: u32,
    pub quadtree_offset: u32,
    pub maplist_offset: u32,
    pub maplist_count: u32,
}

impl MzbHeader {
    /// Parse the plaintext MZB header. Caller is responsible for
    /// having run `decrypt_in_place` first (or having a chunk where
    /// `data[3] < 0x1B`, which is plaintext on disk).
    pub fn parse(body: &[u8]) -> Result<Self> {
        if body.len() < 0x1C {
            return Err(MzbError::TooSmall {
                needed: 0x1C,
                actual: body.len(),
            }
            .into());
        }

        let decode_length = u32::from_le_bytes([body[0], body[1], body[2], body[3]]) & 0x00FF_FFFF;
        let node_count = u32::from_le_bytes([body[4], body[5], body[6], body[7]]) & 0x00FF_FFFF;
        let version = body[3];
        let key_index = body[7];

        // Scan forward from 8 for the first nonzero u32 — this is the
        // mesh-table offset. Real zones put it directly at 8; "ship"
        // zones may have leading zeros that we skip over.
        let mut probe = 8usize;
        let mesh_table_offset = loop {
            if probe + 4 > body.len() {
                return Err(MzbError::NoMeshTable.into());
            }
            let v = u32::from_le_bytes([
                body[probe],
                body[probe + 1],
                body[probe + 2],
                body[probe + 3],
            ]);
            if v != 0 {
                break v;
            }
            probe += 4;
        };

        let grid_width = body[0x0C];
        let grid_height = body[0x0D];
        let quadtree_offset = u32::from_le_bytes([body[0x10], body[0x11], body[0x12], body[0x13]]);
        let maplist_offset = u32::from_le_bytes([body[0x14], body[0x15], body[0x16], body[0x17]]);
        let maplist_count = u32::from_le_bytes([body[0x18], body[0x19], body[0x1A], body[0x1B]]);

        Ok(Self {
            decode_length,
            node_count,
            version,
            key_index,
            grid_width,
            grid_height,
            mesh_table_offset,
            quadtree_offset,
            maplist_offset,
            maplist_count,
        })
    }
}

/// One vertex in the MZB mesh library. Position only — MZB encodes
/// normals separately (per-triangle), not per-vertex.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MzbVertex {
    pub pos: [f32; 3],
}

/// One per-triangle normal record (vec3 f32).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MzbNormal {
    pub n: [f32; 3],
}

/// One mesh in the MZB mesh library: a vertex list, a normal list, and
/// an index list. Indices index into `vertices` and `normals` (the
/// normal index is per-triangle, one entry per triangle).
///
/// "Library" mesh: the geometry sits at the origin. World placement
/// happens via grid/quadtree placement records (Stage B; not yet
/// decoded here).
#[derive(Debug, Clone)]
pub struct MzbMesh {
    pub vertices: Vec<MzbVertex>,
    pub normals: Vec<MzbNormal>,
    /// `[v0, v1, v2]` triples. Indices already masked with `& 0x3FFF`.
    pub triangles: Vec<[u32; 3]>,
    /// One per triangle, parallel to `triangles`. Indexes into `normals`.
    pub triangle_normals: Vec<u32>,
    /// Per-mesh flags from the record header. Bit 0 = does NOT block
    /// line of sight (i.e. visual-only / non-collision).
    pub flags: u16,
}

/// Parse all meshes in the MZB body's mesh library.
///
/// Expects `body` to be plaintext (after `decrypt_in_place`). Returns
/// an empty Vec if `mesh_count == 0`. Does not parse grid placements,
/// quadtree, or map-list.
pub fn parse_meshes(body: &[u8], header: &MzbHeader) -> Result<Vec<MzbMesh>> {
    let mt = header.mesh_table_offset as usize;
    if mt + 0x14 > body.len() {
        return Err(MzbError::MeshTableOutOfRange {
            offset: mt,
            len: body.len(),
        }
        .into());
    }

    let mesh_count =
        u32::from_le_bytes([body[mt], body[mt + 1], body[mt + 2], body[mt + 3]]) as usize;
    let mesh_data_offset =
        u32::from_le_bytes([body[mt + 4], body[mt + 5], body[mt + 6], body[mt + 7]]) as usize;

    if mesh_data_offset >= body.len() {
        return Err(MzbError::MeshDataOutOfRange {
            offset: mesh_data_offset,
            len: body.len(),
        }
        .into());
    }

    // Each mesh record is 16 bytes of fixed fields. Stride between
    // records in the reference is computed as
    // `triangles_offset + triangle_count * 8`, but that only tells you
    // where *the geometry data* of one record ends — it's also where
    // the next mesh-record header could be packed *if* records are laid
    // out contiguously after their triangle data. Empirically (per the
    // reference) records *are* contiguous, so we advance by that
    // formula. If the next-record offsets are nonsensical, we stop
    // gracefully.
    let mut out = Vec::with_capacity(mesh_count);
    let mut pos = mesh_data_offset;
    for _ in 0..mesh_count {
        if pos + 0x10 > body.len() {
            break;
        }
        let mesh = parse_one_mesh(body, pos)?;
        // Advance past this mesh's record header + its triangle block.
        let tri_off =
            u32::from_le_bytes([body[pos + 8], body[pos + 9], body[pos + 10], body[pos + 11]])
                as usize;
        // See parse_one_mesh — tri_count is the low u16 of the i32 at +0x0c, flags is the high u16.
        let tri_count = u16::from_le_bytes([body[pos + 12], body[pos + 13]]) as usize;
        out.push(mesh);
        let next = tri_off.saturating_add(tri_count.saturating_mul(8));
        if next <= pos || next >= body.len() {
            break;
        }
        pos = next;
    }
    Ok(out)
}

fn parse_one_mesh(body: &[u8], pos: usize) -> Result<MzbMesh> {
    if pos + 0x10 > body.len() {
        return Err(MzbError::MeshDataOutOfRange {
            offset: pos,
            len: body.len(),
        }
        .into());
    }

    let verts_off =
        u32::from_le_bytes([body[pos], body[pos + 1], body[pos + 2], body[pos + 3]]) as usize;
    let norms_off =
        u32::from_le_bytes([body[pos + 4], body[pos + 5], body[pos + 6], body[pos + 7]]) as usize;
    let tris_off =
        u32::from_le_bytes([body[pos + 8], body[pos + 9], body[pos + 10], body[pos + 11]]) as usize;
    // The reference reads triangleCount as i32 at +0x0c and flags as i16 at +0x0e —
    // those fields overlap. Real triangle counts fit easily in a u16, so we treat
    // the low half as `tri_count` and the high half as `flags`.
    let tri_count = u16::from_le_bytes([body[pos + 12], body[pos + 13]]) as usize;
    let flags = u16::from_le_bytes([body[pos + 14], body[pos + 15]]);

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

    let vert_count = (norms_off - verts_off) / 12;
    let norm_count = (tris_off - norms_off) / 12;

    let mut vertices = Vec::with_capacity(vert_count);
    for i in 0..vert_count {
        let o = verts_off + i * 12;
        if o + 12 > body.len() {
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
        let o = norms_off + i * 12;
        if o + 12 > body.len() {
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
    for i in 0..tri_count {
        let o = tris_off + i * 8;
        if o + 8 > body.len() {
            break;
        }
        let v0 = u16::from_le_bytes([body[o], body[o + 1]]) & 0x3FFF;
        let v1 = u16::from_le_bytes([body[o + 2], body[o + 3]]) & 0x3FFF;
        let v2 = u16::from_le_bytes([body[o + 4], body[o + 5]]) & 0x3FFF;
        let n0 = u16::from_le_bytes([body[o + 6], body[o + 7]]) & 0x3FFF;
        triangles.push([v0 as u32, v1 as u32, v2 as u32]);
        triangle_normals.push(n0 as u32);
    }

    Ok(MzbMesh {
        vertices,
        normals,
        triangles,
        triangle_normals,
        flags,
    })
}

/// Convenience: decrypt + parse header + parse all meshes.
pub fn parse_all(encrypted_body: &[u8]) -> Result<(MzbHeader, Vec<MzbMesh>)> {
    let plain = decrypt(encrypted_body)?;
    let header = MzbHeader::parse(&plain)?;
    let meshes = parse_meshes(&plain, &header)?;
    Ok((header, meshes))
}

/// One placement: instance the mesh at `geometry_offset` with `transform`.
///
/// Several placements can share a `geometry_offset` — that's how the
/// format reuses one stair/wall template across many world locations.
/// Bake one Bevy `Mesh` per unique offset and instance it per
/// placement.
///
/// `transform` is a 4×4 row-major affine in raw MZB (FFXI client)
/// coordinates. To apply to a vertex `(x, y, z)`:
///   p.x = t[0]*x + t[4]*y + t[8]*z  + t[12]
///   p.y = t[1]*x + t[5]*y + t[9]*z  + t[13]
///   p.z = t[2]*x + t[6]*y + t[10]*z + t[14]
/// i.e. matrix is stored column-major in the f32 stream (m[0..4] is
/// column 0). `transform[12..15]` is the world translation in MZB
/// space; convert to Bevy via `ffxi_to_bevy(Vec3(.x, .y, .z))` *after*
/// applying the matrix, not before.
#[derive(Debug, Clone, Copy)]
pub struct MzbPlacement {
    /// Absolute offset into the chunk body where this placement's mesh
    /// record lives. Use [`parse_mesh_at`] to materialize the
    /// `MzbMesh`. Multiple placements can share the same offset; cache
    /// the parsed mesh by offset to avoid duplicate work.
    pub geometry_offset: u32,
    /// 16 f32s as written on disk. See struct-level docs for the
    /// matrix-multiply convention.
    pub transform: [f32; 16],
    /// True iff this geometry has bit 0 of its mesh flags set
    /// (decoded from the mesh record header). Cached here so the
    /// caller can pick a material without re-parsing the mesh.
    pub doesnt_block_los: bool,
    /// True iff the matrix's 3×3 linear part has negative determinant
    /// (i.e. the instance is mirrored — winding must flip to keep
    /// triangle facing consistent). Renderers using a two-sided
    /// material can ignore this.
    pub flip_winding: bool,
    /// Grid cell coords this placement was discovered in. Useful for
    /// debugging / culling; not semantically meaningful for rendering.
    pub grid_x: u16,
    pub grid_y: u16,
}

/// Parse one mesh record at an absolute body offset. Used by the
/// placement-decode path: grid entries point directly at mesh records
/// (the same record format as the mesh-library) by absolute offset.
///
/// Performs the same offset-sanity checks as [`parse_meshes`]; returns
/// an `MzbError` if the record's verts/normals/tris offsets are
/// inverted or out of range.
pub fn parse_mesh_at(body: &[u8], offset: usize) -> Result<MzbMesh> {
    parse_one_mesh(body, offset)
}

/// Decode all grid-cell placements in the MZB body.
///
/// Returns `Ok(vec![])` (not an error) when:
///   - `grid_width == 0` or `grid_height == 0` (zone has no grid —
///     e.g. some ship/cutscene MZBs), or
///   - the mesh table's `grid_offset` field is zero.
///
/// Each non-zero grid cell may emit zero or more placements. The
/// emitted vector is in cell-major order (y outer, x inner, list
/// order inside the cell) — i.e. deterministic.
///
/// Grid stride: per the reference, the grid is
/// `(grid_width * 10) × (grid_height * 10)` cells, each one a 4-byte
/// pointer. Each non-zero pointer addresses a null-terminated list of
/// u32 entries inside the chunk body. Entry 0 is packed cell metadata
/// (ignored). Subsequent entries come in (matrix_offset,
/// geometry_offset) pairs.
pub fn parse_placements(body: &[u8], header: &MzbHeader) -> Result<Vec<MzbPlacement>> {
    let mt = header.mesh_table_offset as usize;
    if mt + 0x14 > body.len() {
        return Err(MzbError::MeshTableOutOfRange {
            offset: mt,
            len: body.len(),
        }
        .into());
    }
    let grid_offset = u32::from_le_bytes([
        body[mt + 0x10],
        body[mt + 0x11],
        body[mt + 0x12],
        body[mt + 0x13],
    ]) as usize;
    if grid_offset == 0 || grid_offset >= body.len() {
        return Ok(Vec::new());
    }

    // Grid dimensions: per the reference, both grid_width and
    // grid_height are multiplied by 10 to get cell counts along each
    // axis. The reference's outer loop runs `gridheight * 10` from
    // `(block[0x0d] * 10) * 10` — yes that's an extra ×10 — but the
    // *cell-count* the pointer table actually contains is
    // `(grid_width * 10) × (grid_height * 10)` and the reference's
    // `offsets = (y * gridwidth*10 + x) * 4` indexing only fills the
    // first row of that nominally-larger range. Replicating exactly:
    let gw = (header.grid_width as usize).saturating_mul(10);
    let gh = (header.grid_height as usize).saturating_mul(10);
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

            // Read a null-terminated u32 list at entry_off.
            let mut entries: Vec<u32> = Vec::new();
            let mut cur = entry_off;
            // Hard cap to defend against runaway lists in corrupt data.
            // Real grid cells have a handful of meshes — 4096 is far
            // beyond any plausible per-cell count.
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

            // Entries[0] is packed cell metadata (xx/yy/flags); skip.
            // Pairs follow: (matrix_offset, geometry_offset).
            let mut i = 1usize;
            while i + 1 < entries.len() {
                let mat_off = entries[i] as usize;
                let geo_off = entries[i + 1] as usize;
                i += 2;

                if mat_off == 0 || geo_off == 0 {
                    continue;
                }
                if mat_off + 16 * 4 > body.len() || geo_off + 0x10 > body.len() {
                    continue;
                }

                // 16 f32s, on-disk order.
                let mut m = [0.0f32; 16];
                for k in 0..16 {
                    let o = mat_off + k * 4;
                    m[k] = f32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
                }

                // Determinant of the 3×3 linear part. Per the
                // reference, the matrix is stored column-major in the
                // f32 stream (m[0..4] = column 0, etc.), so the linear
                // 3×3 has columns m[0..3], m[4..7], m[8..11]:
                //   | m0 m4 m8  |
                //   | m1 m5 m9  |
                //   | m2 m6 m10 |
                let det = m[0] * (m[5] * m[10] - m[9] * m[6]) - m[4] * (m[1] * m[10] - m[9] * m[2])
                    + m[8] * (m[1] * m[6] - m[5] * m[2]);

                // Read the geometry record's flags field
                // (low half of u32 @ +0x0C, then high half at +0x0E is
                // flags i16 — bit 0 = doesn't block LoS).
                let flags = u16::from_le_bytes([body[geo_off + 14], body[geo_off + 15]]);

                out.push(MzbPlacement {
                    geometry_offset: geo_off as u32,
                    transform: m,
                    doesnt_block_los: (flags & 1) != 0,
                    flip_winding: det < 0.0,
                    grid_x: x as u16,
                    grid_y: y as u16,
                });
            }
        }
    }

    Ok(out)
}

/// Apply an `MzbPlacement` transform to a vertex in MZB-space.
/// Returns a point in MZB-space (the caller is responsible for
/// converting to Bevy with `ffxi_to_bevy`).
///
/// Matrix-stream convention matches the reference: column-major in the
/// f32 stream, so `m[0..4]` is column 0 and `m[12..15]` is the
/// translation column.
#[inline]
pub fn apply_placement(m: &[f32; 16], v: [f32; 3]) -> [f32; 3] {
    let (x, y, z) = (v[0], v[1], v[2]);
    [
        m[0] * x + m[4] * y + m[8] * z + m[12],
        m[1] * x + m[5] * y + m[9] * z + m[13],
        m[2] * x + m[6] * y + m[10] * z + m[14],
    ]
}

/// One MMB-instance placement record. The MZB chunk carries
/// `node_count` of these starting at body offset 0x20, stride 100
/// (`SMZBBlock100`). The 16-byte `id` field names an MMB asset (see
/// matching rules at [`resolve_mmb_index`]).
///
/// Source: TeoTwawki/ffxi-dat-hacking `TDWAnalysis.h::SMZBBlock100` and
/// the LandSandBoat FFXI-NavMesh-Builder `MZB.cs::DecodeMzb` reference
/// implementations. The body-level `decrypt_in_place` pass already
/// renders the `id` bytes in plaintext for the FFXI install we target;
/// no extra per-record XOR is applied (some older references describe a
/// `^= 0x55` step that is *already folded into* our body-level decrypt).
#[derive(Debug, Clone, Copy)]
pub struct MmbPlacement {
    /// 16-byte name. ASCII, null-padded. Matches an MMB `asset_name` in
    /// the same zone DAT either directly or after prefixing with the
    /// zone's 8-char tag and truncating to 16 bytes (see
    /// [`resolve_mmb_index`]).
    pub id: [u8; 16],
    pub trans: [f32; 3],
    /// Euler radians (x, y, z) per the reference.
    pub rot: [f32; 3],
    pub scale: [f32; 3],
}

impl MmbPlacement {
    pub fn id_str(&self) -> &str {
        let end = self
            .id
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.id.len());
        std::str::from_utf8(&self.id[..end]).unwrap_or("")
    }
}

/// Decode MMB-instance placements from the MZB body. Returns
/// `Ok(vec![])` when `header.node_count == 0` or the table region is
/// out of range — i.e. this never errors on "no placements," only on
/// malformed body bytes.
pub fn parse_mmb_placements(body: &[u8], header: &MzbHeader) -> Result<Vec<MmbPlacement>> {
    let count = header.node_count as usize;
    if count == 0 {
        return Ok(Vec::new());
    }
    let table_end = 0x20usize.saturating_add(count.saturating_mul(100));
    if table_end > body.len() {
        return Err(MzbError::MeshTableOutOfRange {
            offset: table_end,
            len: body.len(),
        }
        .into());
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let off = 0x20 + i * 100;
        let rec = &body[off..off + 100];
        let mut id = [0u8; 16];
        id.copy_from_slice(&rec[..16]);
        let f = |o: usize| f32::from_le_bytes([rec[o], rec[o + 1], rec[o + 2], rec[o + 3]]);
        out.push(MmbPlacement {
            id,
            trans: [f(16), f(20), f(24)],
            rot: [f(28), f(32), f(36)],
            scale: [f(40), f(44), f(48)],
        });
    }
    Ok(out)
}

/// Resolve a placement `id` to the index in `mmb_asset_names` that owns
/// it, by trying (in order):
/// 1. Exact match against `placement.id` (most direct).
/// 2. Match against `<zone_prefix><placement.id>` truncated to 16
///    bytes. Works for single-vendor zones where `infer_zone_prefix`
///    yields a meaningful 8-byte tag.
/// 3. Suffix match: any MMB asset name that ends with `placement.id`
///    after both have been space-trimmed. Handles multi-vendor zones
///    where MMBs ship with prefixes like `tshimono`, `ooiwa...`,
///    `mariko..` simultaneously, so no single zone_prefix exists.
///
/// Returns `None` if no MMB asset_name matches. Suffix match is
/// last-resort because it can collide when multiple MMBs share the
/// same suffix string — for placement-correctness most zones use IDs
/// long enough that collisions are rare in practice.
pub fn resolve_mmb_index(
    placement_id: &str,
    zone_prefix: &str,
    mmb_asset_names: &[String],
) -> Option<usize> {
    resolve_mmb_indices(placement_id, zone_prefix, mmb_asset_names)
        .into_iter()
        .next()
}

/// Like [`resolve_mmb_index`] but returns ALL matching chunk indices in
/// DAT order. Necessary for disambiguating duplicate-name placements:
/// FFXI variant MMBs (wall_id01, wall_id02, …, house_p1_m) end up with
/// the SAME 16-byte truncated asset_name in their MMB header
/// (`tshimonowall_id01` → truncated to `tshimonowall_id0`, identical to
/// the base variant). The caller pairs placements with chunks by
/// round-robin: the Nth placement referencing a name consumes the Nth
/// chunk with that name. Without this, the first-match resolver maps
/// every variant placement to chunk #1 and silently drops the rest —
/// visible in-game as missing walls / houses / stair pieces.
pub fn resolve_mmb_indices(
    placement_id: &str,
    zone_prefix: &str,
    mmb_asset_names: &[String],
) -> Vec<usize> {
    // Tier 1: exact match. Collect all positions, not just the first.
    let exact: Vec<usize> = mmb_asset_names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| (n.trim_end() == placement_id).then_some(i))
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    // Tier 2: prefix+exact. Build `<zone_prefix><placement_id>` and match
    // it against the full 24-byte asset_name (trim_end strips space
    // padding). NO truncation: the MMB asset_name field is 24 bytes wide
    // (see MmbHeader::parse). Truncating to 16 here would collapse
    // `wall_id01`/`wall_id09`/... and `house_p1_h`/`house_p1_m` onto a
    // single chunk, silently dropping the rest — that was a real bug
    // that hid buildings.
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
    // Tier 3: vendor-prefix. Match first 8 bytes of placement_id against
    // last 8 bytes of asset_name — works when zone_prefix is empty
    // (multi-vendor zone) but placement_id starts with a known 8-byte
    // asset-id pattern.
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
    // Tier 4: full-string suffix. Tiny IDs (<3 chars) are too wild.
    if placement_id.len() < 3 {
        return Vec::new();
    }
    mmb_asset_names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.trim_end().ends_with(placement_id).then_some(i))
        .collect()
}

/// Infer the 8-char zone prefix shared by all MMB asset_names in a
/// zone DAT. Picks the longest common prefix (up to 8 bytes) of the
/// supplied names; falls back to an empty string if the names disagree.
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

    /// Build a synthetic MZB body with `version < 0x1B` (so decryption
    /// is a noop on pass 1) and a single mesh containing 4 vertices,
    /// 1 normal, and 2 triangles.
    ///
    /// Layout we construct (all offsets relative to body start):
    ///   0x00..0x08  preamble: decode_length=0 (don't care), version=0x10,
    ///               node_count=0 (no pass-2 work), key_index=0.
    ///   0x08..0x0C  mesh_table_offset = 0x20
    ///   0x0C        grid_width = 0
    ///   0x0D        grid_height = 0
    ///   0x10..0x14  quadtree_offset = 0
    ///   0x14..0x18  maplist_offset = 0
    ///   0x18..0x1C  maplist_count = 0
    ///   0x1C..0x20  pad
    ///   0x20..0x28  mesh table: mesh_count=1, mesh_data_offset=0x30
    ///   0x28..0x30  pad
    ///   0x30..0x40  mesh record header: verts=0x40, norms=0x70, tris=0x7C,
    ///               tri_count=2, flags=0
    ///   0x40..0x70  4 vertices (12 bytes each = 48 bytes = 0x30)
    ///   0x70..0x7C  1 normal (12 bytes)
    ///   0x7C..0x8C  2 triangles (8 bytes each = 16 bytes)
    fn synth_mzb() -> Vec<u8> {
        let mut buf = vec![0u8; 0x8C];

        // Preamble
        // decode_length doesn't matter for plaintext; we set it to total length.
        let decode_len = 0x8Cu32 | (0x10 << 24); // top byte = version = 0x10
        buf[0..4].copy_from_slice(&decode_len.to_le_bytes());
        // node_count = 0, top byte = key_index = 0
        buf[4..8].copy_from_slice(&0u32.to_le_bytes());

        // mesh_table_offset @ 0x08
        buf[8..12].copy_from_slice(&0x20u32.to_le_bytes());

        // 0x10..0x1C: zero placeholders for quadtree/maplist

        // Mesh table @ 0x20
        buf[0x20..0x24].copy_from_slice(&1u32.to_le_bytes()); // mesh_count
        buf[0x24..0x28].copy_from_slice(&0x30u32.to_le_bytes()); // mesh_data_offset

        // Mesh record @ 0x30
        buf[0x30..0x34].copy_from_slice(&0x40u32.to_le_bytes()); // verts_off
        buf[0x34..0x38].copy_from_slice(&0x70u32.to_le_bytes()); // norms_off
        buf[0x38..0x3C].copy_from_slice(&0x7Cu32.to_le_bytes()); // tris_off
                                                                 // Note: the reference reads tri_count as i32 at +0x0C and flags as i16 at +0x0E,
                                                                 // which overlap. To produce both `tri_count=2` (low 16 bits) and `flags=0` (high 16),
                                                                 // we just write a u32=2 and the high half is 0.
        buf[0x3C..0x40].copy_from_slice(&2u32.to_le_bytes()); // tri_count=2, flags=0

        // Vertices @ 0x40: (0,0,0), (1,0,0), (0,1,0), (0,0,1)
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

        // Normal @ 0x70: (0, 1, 0)
        buf[0x70..0x74].copy_from_slice(&0.0f32.to_le_bytes());
        buf[0x74..0x78].copy_from_slice(&1.0f32.to_le_bytes());
        buf[0x78..0x7C].copy_from_slice(&0.0f32.to_le_bytes());

        // Triangles @ 0x7C: [0,1,2,0] and [0,2,3,0]
        // Set the top bits to exercise the 0x3FFF mask: encode `0|0x4000` and ensure mask trims it.
        let tris: [[u16; 4]; 2] = [[0 | 0x4000, 1, 2, 0], [0, 2, 3 | 0x8000, 0]];
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
        assert_eq!(buf, orig, "version < 0x1B should bypass pass 1 entirely");
    }

    #[test]
    fn header_parses_basic_fields() {
        let body = synth_mzb();
        let h = MzbHeader::parse(&body).unwrap();
        assert_eq!(h.version, 0x10);
        assert_eq!(h.key_index, 0x00);
        assert_eq!(h.node_count, 0);
        assert_eq!(h.mesh_table_offset, 0x20);
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

        // Verify vertex positions round-tripped.
        assert_eq!(m.vertices[0].pos, [0.0, 0.0, 0.0]);
        assert_eq!(m.vertices[1].pos, [1.0, 0.0, 0.0]);

        // Triangle 0: [0 | 0x4000, 1, 2, 0] — top bit must be masked away.
        assert_eq!(
            m.triangles[0],
            [0, 1, 2],
            "v0 high bit must be masked with 0x3FFF"
        );
        assert_eq!(m.triangle_normals[0], 0);

        // Triangle 1: [0, 2, 3 | 0x8000, 0] — top bit must be masked away.
        assert_eq!(
            m.triangles[1],
            [0, 2, 3],
            "v2 high bit must be masked with 0x3FFF"
        );
    }

    #[test]
    fn pass2_node_xor_runs() {
        // Build a minimal body: version=0x10 (no pass-1), node_count=1.
        // Put bytes 0x10101010... at the node-head region and verify
        // pass 2 XORs them with 0x55.
        let mut buf = vec![0u8; 0x20 + 0x64];
        buf[0..4].copy_from_slice(&((0x10u32 << 24) | 0x20).to_le_bytes());
        buf[4..8].copy_from_slice(&1u32.to_le_bytes()); // node_count = 1
                                                        // mesh_table_offset = anything-but-zero so header parse later wouldn't choke
                                                        // (we won't call parse_meshes here, just verify the XOR mask landed).
        buf[8..12].copy_from_slice(&0x20u32.to_le_bytes());
        for b in &mut buf[0x20..0x30] {
            *b = 0xAA;
        }
        decrypt_in_place(&mut buf).unwrap();
        for b in &buf[0x20..0x30] {
            assert_eq!(
                *b,
                0xAA ^ 0x55,
                "pass 2 should XOR first 16 bytes of each node with 0x55"
            );
        }
        // Beyond the 16-byte head must be untouched.
        for b in &buf[0x30..0x20 + 0x64] {
            assert_eq!(*b, 0);
        }
    }

    #[test]
    fn pass1_xor_runs_when_version_is_encrypted() {
        // The XOR pass is gated on `version >= 0x1B`. We can't easily
        // test a *real* roundtrip here without a known plaintext, so we
        // just verify the pass *modifies* bytes when version is set
        // appropriately, and leaves them alone when it isn't.
        let mut encrypted = vec![0u8; 64];
        encrypted[0..4].copy_from_slice(&((0x1Bu32 << 24) | 64).to_le_bytes());
        encrypted[4..8].copy_from_slice(&0u32.to_le_bytes());
        // Bytes 8..64 are zeros. After pass 1 they may stay zero (if
        // (key & 1) == 0 for every step), so we can't assert *change*.
        // Instead, pick a key_index whose KEY_TABLE entry has bit 0 set.
        // Probe seeds until we find one that produces a change.
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

    /// Build a synthetic body with one mesh-library mesh, a 1x1 grid
    /// (grid_width=1, grid_height=1 → 10×10 cells), and a single
    /// non-zero grid cell at (0,0) referencing one (matrix, geometry)
    /// pair. The matrix is a pure translation of (100, 200, 300) so
    /// `apply_placement` to the origin should give exactly that.
    #[allow(clippy::identity_op)]
    fn synth_mzb_with_placement() -> Vec<u8> {
        // Layout:
        //  0x00..0x20  header
        //   - mesh_table_offset @ 0x08 = 0x20
        //   - grid_width = 1 @ 0x0C
        //   - grid_height = 1 @ 0x0D
        //  0x20..0x34  mesh table:
        //   - mesh_count = 1   @ 0x20
        //   - mesh_data_off    = 0x40  @ 0x24
        //   - (skip 0x28..0x30, irrelevant)
        //   - grid_off         = 0x80  @ 0x30 (mesh_table + 0x10)
        //  0x40..0x50  mesh record header (verts=0x50, norms=0x70, tris=0x7C, tri_count=2)
        //  0x50..0x80  vertices + normals + tris (same as synth_mzb), then pad
        //  0x80..0x80+10*10*4 = 0x80..0x210  grid pointer table (only [0]=0x210 nonzero)
        //  0x210..0x220  grid entry list: [meta, mat_off, geo_off, 0_term]
        //  0x220..0x260  matrix (16 f32 = 64 bytes)
        //
        // Geometry is at 0x40 (same as mesh-library mesh).
        let mut buf = vec![0u8; 0x260];

        // Preamble: version 0x10 (no XOR), node_count=0
        let decode_len = (buf.len() as u32) | (0x10u32 << 24);
        buf[0..4].copy_from_slice(&decode_len.to_le_bytes());
        buf[4..8].copy_from_slice(&0u32.to_le_bytes());

        // mesh_table_offset
        buf[8..12].copy_from_slice(&0x20u32.to_le_bytes());
        buf[0x0C] = 1; // grid_width
        buf[0x0D] = 1; // grid_height

        // Mesh table @ 0x20
        buf[0x20..0x24].copy_from_slice(&1u32.to_le_bytes()); // mesh_count
        buf[0x24..0x28].copy_from_slice(&0x40u32.to_le_bytes()); // mesh_data_off
        buf[0x30..0x34].copy_from_slice(&0x80u32.to_le_bytes()); // grid_offset @ mt+0x10

        // Mesh record @ 0x40
        buf[0x40..0x44].copy_from_slice(&0x50u32.to_le_bytes()); // verts
        buf[0x44..0x48].copy_from_slice(&0x70u32.to_le_bytes()); // norms (need verts<=norms)
        buf[0x48..0x4C].copy_from_slice(&0x7Cu32.to_le_bytes()); // tris
        buf[0x4C..0x50].copy_from_slice(&2u32.to_le_bytes()); // tri_count=2, flags=0

        // Vertices @ 0x50: room for (0x70-0x50)/12 ≈ 2.67 → 2 verts; we
        // only write one but parser computes count from offsets.
        // Triangles @ 0x7C: 2 × 8 = 16 bytes → ends at 0x8C, which
        // overlaps grid_offset (0x80)! Fine for the test — we only
        // exercise placement decode, not mesh decode of this record.

        // Grid pointer table @ 0x80 (100 entries × 4 = 400 bytes = 0x190 → ends 0x210).
        // Only cell (0,0) → 0x210.
        buf[0x80..0x84].copy_from_slice(&0x210u32.to_le_bytes());

        // Grid entry list @ 0x210: [meta=0xDEAD, mat_off=0x220, geo_off=0x40, 0]
        buf[0x210..0x214].copy_from_slice(&0xDEADu32.to_le_bytes());
        buf[0x214..0x218].copy_from_slice(&0x220u32.to_le_bytes());
        buf[0x218..0x21C].copy_from_slice(&0x40u32.to_le_bytes());
        buf[0x21C..0x220].copy_from_slice(&0u32.to_le_bytes()); // terminator

        // Matrix @ 0x220: identity rotation + translation (100,200,300).
        // Column-major: m[0]=1, m[5]=1, m[10]=1, m[12..15]=trans.
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
        assert_eq!(h.grid_width, 1);
        assert_eq!(h.grid_height, 1);

        let placements = parse_placements(&body, &h).unwrap();
        assert_eq!(
            placements.len(),
            1,
            "exactly one (mat,geo) pair in cell (0,0)"
        );
        let p = placements[0];
        assert_eq!(p.geometry_offset, 0x40);
        assert_eq!(p.grid_x, 0);
        assert_eq!(p.grid_y, 0);
        assert!(
            !p.flip_winding,
            "identity rotation has positive determinant"
        );
        // Translation column.
        assert_eq!(p.transform[12], 100.0);
        assert_eq!(p.transform[13], 200.0);
        assert_eq!(p.transform[14], 300.0);

        // apply_placement to origin yields the translation.
        let world = apply_placement(&p.transform, [0.0, 0.0, 0.0]);
        assert_eq!(world, [100.0, 200.0, 300.0]);
        // apply_placement to (1,0,0) yields (101, 200, 300) under identity rotation.
        let world = apply_placement(&p.transform, [1.0, 0.0, 0.0]);
        assert_eq!(world, [101.0, 200.0, 300.0]);
    }

    #[test]
    fn placements_empty_when_no_grid() {
        // Reuse the no-grid synth_mzb (grid_width=0, grid_height=0).
        let body = synth_mzb();
        let h = MzbHeader::parse(&body).unwrap();
        let placements = parse_placements(&body, &h).unwrap();
        assert!(placements.is_empty(), "grid_width=0 → no placements");
    }

    #[test]
    fn placement_flip_winding_on_negative_det() {
        // Build a placement matrix with x-axis mirror: m[0] = -1.
        // Determinant of diag(-1, 1, 1) = -1 < 0 → flip_winding true.
        let mut body = synth_mzb_with_placement();
        // Matrix is at 0x220. m[0] is the first float.
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
}
