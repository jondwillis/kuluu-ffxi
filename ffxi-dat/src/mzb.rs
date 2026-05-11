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
//! Stage B (TODO, deferred): grid + quadtree + per-cell placements.
//! That's where each mesh actually gets instanced into world space with
//! a 4×3 transform. Skipped here because the MVP renderer can show the
//! mesh-library geometry first and get a "yes there's a zone" signal
//! without it.

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
    CrossedOffsets { pos: usize, verts: usize, normals: usize, tris: usize },
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
        return Err(MzbError::TooSmall { needed: 8, actual: data.len() }.into());
    }

    let decode_length = (u32::from_le_bytes([data[0], data[1], data[2], data[3]])
        & 0x00FF_FFFF) as usize;
    let node_count = (u32::from_le_bytes([data[4], data[5], data[6], data[7]])
        & 0x00FF_FFFF) as usize;

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
            return Err(MzbError::TooSmall { needed: 0x1C, actual: body.len() }.into());
        }

        let decode_length =
            u32::from_le_bytes([body[0], body[1], body[2], body[3]]) & 0x00FF_FFFF;
        let node_count =
            u32::from_le_bytes([body[4], body[5], body[6], body[7]]) & 0x00FF_FFFF;
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
                body[probe], body[probe + 1], body[probe + 2], body[probe + 3],
            ]);
            if v != 0 {
                break v;
            }
            probe += 4;
        };

        let grid_width = body[0x0C];
        let grid_height = body[0x0D];
        let quadtree_offset =
            u32::from_le_bytes([body[0x10], body[0x11], body[0x12], body[0x13]]);
        let maplist_offset =
            u32::from_le_bytes([body[0x14], body[0x15], body[0x16], body[0x17]]);
        let maplist_count =
            u32::from_le_bytes([body[0x18], body[0x19], body[0x1A], body[0x1B]]);

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
        return Err(MzbError::MeshTableOutOfRange { offset: mt, len: body.len() }.into());
    }

    let mesh_count = u32::from_le_bytes([body[mt], body[mt + 1], body[mt + 2], body[mt + 3]]) as usize;
    let mesh_data_offset = u32::from_le_bytes([
        body[mt + 4], body[mt + 5], body[mt + 6], body[mt + 7],
    ]) as usize;

    if mesh_data_offset >= body.len() {
        return Err(MzbError::MeshDataOutOfRange { offset: mesh_data_offset, len: body.len() }.into());
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
        let tri_off = u32::from_le_bytes([body[pos + 8], body[pos + 9], body[pos + 10], body[pos + 11]]) as usize;
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
        return Err(MzbError::MeshDataOutOfRange { offset: pos, len: body.len() }.into());
    }

    let verts_off = u32::from_le_bytes([body[pos], body[pos + 1], body[pos + 2], body[pos + 3]]) as usize;
    let norms_off = u32::from_le_bytes([body[pos + 4], body[pos + 5], body[pos + 6], body[pos + 7]]) as usize;
    let tris_off = u32::from_le_bytes([body[pos + 8], body[pos + 9], body[pos + 10], body[pos + 11]]) as usize;
    // The reference reads triangleCount as i32 at +0x0c and flags as i16 at +0x0e —
    // those fields overlap. Real triangle counts fit easily in a u16, so we treat
    // the low half as `tri_count` and the high half as `flags`.
    let tri_count = u16::from_le_bytes([body[pos + 12], body[pos + 13]]) as usize;
    let flags = u16::from_le_bytes([body[pos + 14], body[pos + 15]]);

    if verts_off >= body.len() || norms_off >= body.len() || tris_off >= body.len() {
        return Err(MzbError::MeshDataOutOfRange { offset: verts_off.max(norms_off).max(tris_off), len: body.len() }.into());
    }
    if !(verts_off <= norms_off && norms_off <= tris_off) {
        return Err(MzbError::CrossedOffsets { pos, verts: verts_off, normals: norms_off, tris: tris_off }.into());
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

    Ok(MzbMesh { vertices, normals, triangles, triangle_normals, flags })
}

/// Convenience: decrypt + parse header + parse all meshes.
pub fn parse_all(encrypted_body: &[u8]) -> Result<(MzbHeader, Vec<MzbMesh>)> {
    let plain = decrypt(encrypted_body)?;
    let header = MzbHeader::parse(&plain)?;
    let meshes = parse_meshes(&plain, &header)?;
    Ok((header, meshes))
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
        let tris: [[u16; 4]; 2] = [
            [0 | 0x4000, 1, 2, 0],
            [0, 2, 3 | 0x8000, 0],
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
        assert_eq!(m.triangles[0], [0, 1, 2], "v0 high bit must be masked with 0x3FFF");
        assert_eq!(m.triangle_normals[0], 0);

        // Triangle 1: [0, 2, 3 | 0x8000, 0] — top bit must be masked away.
        assert_eq!(m.triangles[1], [0, 2, 3], "v2 high bit must be masked with 0x3FFF");
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
            assert_eq!(*b, 0xAA ^ 0x55, "pass 2 should XOR first 16 bytes of each node with 0x55");
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
        assert!(any_change, "at least one key_index should produce a pass-1 XOR change");
    }

    #[test]
    fn too_small_errors() {
        let small = vec![0u8; 4];
        let err = decrypt(&small).unwrap_err();
        assert!(matches!(err, DatError::Mzb(_)));
    }
}
