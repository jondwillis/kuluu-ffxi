//! VertexOs2 (chunk kind 0x2A) — FFXI's skinned-mesh container used
//! for player characters and humanoid NPCs.
//!
//! This is the *companion* to [`crate::mmb`], not a replacement. MMBs
//! hold rigid static props (helmets, furniture, doors). VertexOs2
//! chunks hold the per-body-part deformable meshes that ride a Sk2
//! (kind 0x29) skeleton. The two are distinct on-disk formats with
//! different layouts, so they get separate parsers.
//!
//! # Provenance
//!
//! Reverse-engineered from format facts only — header struct field
//! offsets, vertex stride constants, polygon-block opcode bytes — as
//! derived independently from byte inspection of real DATs and
//! cross-checked against community reverse-engineering notes that
//! describe the *structure* of the format (struct names like
//! `DAT2AHeader`, opcode bytes `0x5453`/`0x0054`/`0x4353`/`0x0043`).
//! No implementation code was ported; this is a clean-room write.
//!
//! Original Japanese reverse-engineering trail: Tomato's FFXI Tool
//! (2006, no longer maintained) — successor community forks document
//! the format publicly. POLUtils (Apache-2.0) does *not* parse this
//! kind, so we cannot use it as the primary reference here.
//!
//! # Header layout (`DAT2AHeader`, 64 bytes, 2-byte packed)
//!
//! All `offset*` fields are in **word units** — multiply by 2 to get
//! the byte offset within the chunk body.
//!
//! ```text
//! offset 0x00  u8   ver
//! offset 0x01  u8   nazo (unknown)
//! offset 0x02  u16  type   ( &0x7F: 0=model, 1=class )
//! offset 0x04  u16  flip   ( 0=no mirror, !=0 generate mirrored copy )
//! offset 0x06  u32  offsetPoly        (word units)
//! offset 0x0A  u16  PolySuu           (poly-block size hint)
//! offset 0x0C  u32  offsetBoneTbl     (word units)
//! offset 0x10  u16  BoneTblSuu
//! offset 0x12  u32  offsetWeight      (word units)
//! offset 0x16  u16  WeightSuu
//! offset 0x18  u32  offsetBone        (word units)
//! offset 0x1C  u16  BoneSuu
//! offset 0x1E  u32  offsetVertex      (word units)
//! offset 0x22  u16  VertexSuu
//! offset 0x24  u32  offsetPolyLoad    (word units)
//! offset 0x28  u16  PolyLoadSuu
//! offset 0x2A  u16  PolyLodVtx0Suu
//! offset 0x2C  u16  PolyLodVtx1Suu
//! offset 0x2E  u32  offsetPolyLod2
//! offset 0x32  u16  PolyLod2Suu       ( 0 = pure 1-bone vertex layout )
//! offset 0x34..0x40  16 bytes of further unknowns
//! ```
//!
//! # Vertex layout
//!
//! At `offsetWeight * 2`: two `i16` values (`weight1`, `weight2`).
//! `weight1` is the count of 1-bone (rigid) vertices; `weight2` is
//! the count of 2-bone (skinned) vertices.
//!
//! At `offsetVertex * 2`:
//!   - `weight1` records of 24 bytes each (`MODELVERTEX1`):
//!     `[vec3 pos, vec3 normal]`
//!   - then `weight2` records of 56 bytes each (`MODELVERTEX2`):
//!     `[f32 x[2], f32 y[2], f32 z[2], f32 w[2],
//!       f32 hx[2], f32 hy[2], f32 hz[2]]`
//!     (Structure-of-arrays per-vertex: each axis has two values, one
//!     for each bone weight; `w[i]` is the weight scalar; `h*` is the
//!     normal split across the two bones identically.)
//!
//! We currently only consume the 1-bone path (`weight1` × 24-byte
//! records). 2-bone vertices are decoded as bind-pose by collapsing
//! to the first weight (`x[0], y[0], z[0]`) — produces a mesh that
//! co-locates two bone weights at one position. Correct skinning is
//! a follow-up.
//!
//! # Polygon block
//!
//! At `offsetPoly * 2`. State-machine driven; each record begins with
//! a `u16 wf` opcode and `u16 ws` count:
//!
//! | `wf`            | meaning                                  | body
//! |-----------------|------------------------------------------|--------------
//! | `0x5453` `"ST"` | triangle strip, `ws` corners total       | 4 byte header, then 1× 30-byte SFace3, then `ws-1`× 10-byte SFace
//! | `0x0054` `"T"`  | triangle list, `ws` triangles            | 4 byte header, then `ws`× 30-byte SFace3
//! | `0x4353` `"SC"` | cloth strip                              | skip `ws*20 + 12` bytes
//! | `0x0043` `"C"`  | cloth list                               | skip `ws*10 + 4` bytes
//! | `wf & 0x80F0 == 0x8000` | texture-name section (16-byte name) | skip 18 bytes
//! | `wf & 0x80F0 == 0x8010` | unknown header section              | skip 46 bytes
//! | anything else   | terminator — stop parsing                | —
//!
//! `SFace3` = `[u16 i0, u16 i1, u16 i2, vec2 uv0, vec2 uv1, vec2 uv2]`
//! = 30 bytes (no padding; `pragma pack(1)` in the original C struct).
//!
//! `SFace` = `[u16 i, vec2 uv]` = 10 bytes — used for the strip-extend
//! records that follow the first triangle in a strip.

use crate::{DatError, Result};

/// Errors specific to VertexOs2 parsing.
#[derive(Debug, thiserror::Error)]
pub enum Vos2Error {
    #[error("VertexOs2 chunk too small for header: need {needed}, got {got}")]
    HeaderTooSmall { needed: usize, got: usize },
    #[error(
        "VertexOs2 section offset {section} = {byte_offset:#x} out of bounds (body len {body_len})"
    )]
    SectionOob {
        section: &'static str,
        byte_offset: usize,
        body_len: usize,
    },
    #[error("VertexOs2 poly block walked past end of body")]
    PolyOob,
}

impl From<Vos2Error> for DatError {
    fn from(e: Vos2Error) -> Self {
        DatError::Mmb(format!("vos2: {e}"))
    }
}

/// Decoded header for a VertexOs2 chunk. Field offsets/units match
/// the on-disk struct (see module docs).
#[derive(Debug, Clone, Copy)]
pub struct Vos2Header {
    pub version: u8,
    pub kind_type: u16,
    pub flip: u16,
    pub off_poly_bytes: usize,
    pub off_bone_table_bytes: usize,
    /// `BoneTblSuu` — number of `u16` entries in the bone palette at
    /// `off_bone_table_bytes`. Each entry is a skeleton-bone index that
    /// the per-vertex `bone_index` field can address.
    pub bone_table_count: u16,
    pub off_weight_bytes: usize,
    pub off_bone_bytes: usize,
    /// `BoneSuu` — number of `BoneIndices` records (each 16-bit packed)
    /// in the stream at `off_bone_bytes`. This is the *parallel*
    /// per-vertex bone-id stream; vertex positions stay 24 bytes and
    /// the bone id lives here.
    pub bone_indices_count: u16,
    pub off_vertex_bytes: usize,
    pub off_poly_load_bytes: usize,
    pub poly_lod2_count: u16,
}

impl Vos2Header {
    pub fn parse(body: &[u8]) -> Result<Self> {
        if body.len() < 0x40 {
            return Err(Vos2Error::HeaderTooSmall {
                needed: 0x40,
                got: body.len(),
            }
            .into());
        }
        let u16_at = |o: usize| u16::from_le_bytes([body[o], body[o + 1]]);
        let u32_at =
            |o: usize| u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
        Ok(Self {
            version: body[0],
            kind_type: u16_at(0x02),
            flip: u16_at(0x04),
            off_poly_bytes: u32_at(0x06) as usize * 2,
            off_bone_table_bytes: u32_at(0x0C) as usize * 2,
            bone_table_count: u16_at(0x10),
            off_weight_bytes: u32_at(0x12) as usize * 2,
            off_bone_bytes: u32_at(0x18) as usize * 2,
            bone_indices_count: u16_at(0x1C),
            off_vertex_bytes: u32_at(0x1E) as usize * 2,
            off_poly_load_bytes: u32_at(0x24) as usize * 2,
            poly_lod2_count: u16_at(0x32),
        })
    }

    /// Whether the per-vertex `bone_index` (7-bit) is an index into
    /// the [`Vos2Mesh::bone_table`] palette (`true`) or a direct
    /// skeleton-bone index (`false`).
    ///
    /// Source: lotus-ffxi `os2.cppm`, `mVertAndBoneRefFlag & 0x80`
    /// (the field this crate reads as `kind_type`).
    pub fn use_bone_table(&self) -> bool {
        (self.kind_type & 0x80) != 0
    }
}

/// One 16-bit packed bone-assignment record. Each vertex pulls bone
/// indices from a *parallel* stream at `off_bone_bytes` — they are
/// **not** stored inline in the 24-byte vertex record.
///
/// Bit layout (LSB first, matching lotus-ffxi's bitfield order on
/// little-endian targets):
///
/// ```text
///   bits  0..7   bone_index1   (primary bone — used for 1-weight verts)
///   bits  7..14  bone_index2   (secondary bone — used for 2-weight verts)
///   bits 14..16  mirror_axis
/// ```
///
/// For 1-weight (rigid) vertices the renderer uses `bone_index1`
/// directly. For 2-weight (skinned) vertices it blends bone1/bone2
/// per the weight pair in the vertex record.
#[derive(Debug, Clone, Copy)]
pub struct Vos2BoneIndices {
    pub bone_index1: u8,
    pub bone_index2: u8,
    pub mirror_axis: u8,
}

impl Vos2BoneIndices {
    /// Unpack one packed 16-bit record from raw little-endian bytes.
    pub fn from_u16(w: u16) -> Self {
        Self {
            bone_index1: (w & 0x7F) as u8,
            bone_index2: ((w >> 7) & 0x7F) as u8,
            mirror_axis: ((w >> 14) & 0x03) as u8,
        }
    }
}

/// One decoded vertex, bind-pose, no skinning applied. `weight2`
/// vertices collapse their two-bone position to the first weight's
/// position — see module docs for the skinning trade-off.
#[derive(Debug, Clone, Copy)]
pub struct Vos2Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
}

/// Per-vertex multi-bone weight pair, populated only for the
/// 2-bone (`weight2`) records in the OS2 vertex section. Indexed
/// by the vertex index *minus* `weight1` (since the first `weight1`
/// vertices are rigid).
///
/// FFXI's 2-bone format isn't standard linear-blend skinning: each
/// 2-bone vertex carries **separate positions** for each bone's
/// local space (`pos1` and `pos2` below), and the final world
/// position is `w1 * bone1_world * pos1 + w2 * bone2_world * pos2`.
/// Bevy's `SkinnedMesh` expects one position blended by N weighted
/// bone transforms — so feeding `pos1` plus weights `(w1, w2)`
/// produces an approximation, not exact reproduction. The
/// approximation is acceptable because pos1 and pos2 are nearly
/// identical at the joint surface (the format's whole reason to
/// exist is to control how the position diverges *just* at the
/// joint crease).
#[derive(Debug, Clone, Copy)]
pub struct Vos2BoneWeight {
    pub weight1: f32,
    pub weight2: f32,
    pub pos1: [f32; 3],
    pub pos2: [f32; 3],
    pub normal1: [f32; 3],
    pub normal2: [f32; 3],
}

/// One indexed triangle with per-corner UVs. The triangle-list and
/// triangle-strip walkers both produce this shape.
#[derive(Debug, Clone, Copy)]
pub struct Vos2Triangle {
    pub indices: [u16; 3],
    pub uvs: [[f32; 2]; 3],
}

/// A polygon group that shares one texture. The polygon block's
/// texture-name opcode flips the current name; every group that
/// follows binds against that name until the next flip.
#[derive(Debug, Clone)]
pub struct Vos2Group {
    pub texture_name: String,
    pub triangles: Vec<Vos2Triangle>,
    /// Specular exponent from the most-recent `0x8010` DrawState
    /// opcode preceding this group. 0.0 = no specular contribution.
    /// Mirrors lotus's `DrawStateHeader::specular_exponent`
    /// (`vendor/lotus-ffxi/ffxi/dat/os2.cppm:106`).
    pub specular_exponent: f32,
    /// Specular intensity multiplier from the same DrawState.
    /// 0.0 = matte. Combined with the exponent, this drives the
    /// Bevy `StandardMaterial`'s metallic + roughness translation
    /// in the spawn path.
    pub specular_intensity: f32,
}

/// Top-level parsed VertexOs2 chunk: bind-pose vertex pool + groups
/// of triangles. Equivalent to one MMB sub-record from the caller's
/// perspective — but a VertexOs2 chunk may have many groups, each
/// with its own texture.
///
/// `bone_table` and `bone_indices` are surfaced for the skinned-mesh
/// pipeline (Sk2 + bind-pose bake). They are populated when the
/// corresponding header offsets/counts are non-zero; meshes that
/// don't carry skin data leave both empty.
#[derive(Debug, Clone)]
pub struct Vos2Mesh {
    pub header: Vos2Header,
    pub vertices: Vec<Vos2Vertex>,
    pub groups: Vec<Vos2Group>,
    /// Bone palette: `Vec<skeleton_bone_index>` of length
    /// `header.bone_table_count`. The per-vertex `bone_index` field
    /// indexes this palette when `header.use_bone_table()` is true;
    /// otherwise it indexes the skeleton directly.
    pub bone_table: Vec<u16>,
    /// Parallel per-vertex bone-assignment stream. Each entry is the
    /// 16-bit packed `(bone1:7, bone2:7, mirror:2)` record. The
    /// vertex→entry mapping is layout-dependent (1-weight verts and
    /// 2-weight verts consume different numbers of entries — see
    /// [`Vos2Mesh::skeleton_bone_for`]).
    pub bone_indices: Vec<Vos2BoneIndices>,
    /// 2-bone weight pairs for the `weight2`-region vertices (those
    /// indexed `[header.weight1 .. vertices.len()]`). Empty for
    /// meshes that ship only rigid (`weight1`) verts. See
    /// [`Vos2BoneWeight`] for the format / skinning trade-off.
    pub bone_weights: Vec<Vos2BoneWeight>,
}

impl Vos2Mesh {
    /// Resolve the **skeleton-bone index** for vertex `vertex_idx`,
    /// honoring [`Vos2Header::use_bone_table`]. Returns `None` if the
    /// vertex has no recoverable bone assignment (e.g., a mesh that
    /// shipped without a `bone_indices` stream, or an out-of-range
    /// vertex index).
    ///
    /// Per lotus-ffxi's reader, 1-weight (rigid) vertices each
    /// consume *two* `BoneIndices` records (primary + mirror pair)
    /// from the parallel stream; 2-weight (skinned) vertices each
    /// consume *one* record per sub-vertex. We follow that cadence:
    /// the first `weight1 * 2` entries cover the rigid pool (each
    /// rigid vertex at index `i` reads from `bone_indices[i * 2]`),
    /// and the remaining `weight2 * 2` entries cover the skinned
    /// pool's interleaved pairs.
    ///
    /// For 2-weight verts we return the *primary* bone
    /// (`bone_index1`); a full skinning bake would also need
    /// `bone_index2` and the weight pair from the vertex record.
    pub fn skeleton_bone_for(&self, vertex_idx: usize) -> Option<u16> {
        let raw = self.raw_bone_index_for(vertex_idx)?;
        let raw = raw as u16;
        if self.header.use_bone_table() {
            self.bone_table.get(raw as usize).copied()
        } else {
            Some(raw)
        }
    }

    /// Return `bone_index1` for `vertex_idx` *before* the
    /// [`Vos2Header::use_bone_table`] indirection. Exposed for
    /// callers that want to inspect the raw stream (debug/tests).
    pub fn raw_bone_index_for(&self, vertex_idx: usize) -> Option<u8> {
        let bi_idx = vertex_idx.checked_mul(2)?;
        self.bone_indices.get(bi_idx).map(|b| b.bone_index1)
    }

    /// Resolve the **secondary** skeleton-bone for a 2-weight vertex
    /// (vertices at index `[weight1.. ]`). Returns `None` for rigid
    /// vertices (which have no second deformer).
    ///
    /// Per lotus's reader (`os2.cppm:322-329`), for a 2-weight FFXI
    /// vertex `v` the secondary bone is `bone_indices[v*2 + 1]
    /// .bone_index1` (not `[v*2].bone_index2` — the cadence is two
    /// BoneIndices records per FFXI vertex, one per sub-vertex).
    pub fn skeleton_bone2_for(&self, vertex_idx: usize) -> Option<u16> {
        // Only weight2 verts have a meaningful second bone. We can't
        // tell the boundary from inside this impl without storing
        // the weight1 count — instead, callers should already know
        // which range they're in (`bone_weights.is_empty()` or
        // index comparison). Here we just return whatever the
        // off-by-one stream says, and the caller decides how to use it.
        let bi_idx = vertex_idx.checked_mul(2)?.checked_add(1)?;
        let raw = self.bone_indices.get(bi_idx)?.bone_index1 as u16;
        if self.header.use_bone_table() {
            self.bone_table.get(raw as usize).copied()
        } else {
            Some(raw)
        }
    }
}

/// Parse a VertexOs2 chunk body into a bind-pose mesh.
///
/// `body` must be the raw bytes from `Chunk::data` for a `0x2A`
/// chunk. VertexOs2 chunks are *not* encrypted (unlike MMB), so the
/// body is consumed directly.
pub fn parse_vos2(body: &[u8]) -> Result<Vos2Mesh> {
    let header = Vos2Header::parse(body)?;

    // Vertices: weight1 and weight2 live as two i16 at offsetWeight.
    if header.off_weight_bytes + 4 > body.len() {
        return Err(Vos2Error::SectionOob {
            section: "weight",
            byte_offset: header.off_weight_bytes,
            body_len: body.len(),
        }
        .into());
    }
    let weight1 = i16::from_le_bytes([
        body[header.off_weight_bytes],
        body[header.off_weight_bytes + 1],
    ]) as usize;
    let weight2 = i16::from_le_bytes([
        body[header.off_weight_bytes + 2],
        body[header.off_weight_bytes + 3],
    ]) as usize;

    let vstart = header.off_vertex_bytes;
    const STRIDE1: usize = 24; // pos(12) + normal(12)
    const STRIDE2: usize = 56; // 7× vec2 SoA
    let v1_bytes = weight1 * STRIDE1;
    let v2_bytes = weight2 * STRIDE2;
    if vstart + v1_bytes + v2_bytes > body.len() {
        return Err(Vos2Error::SectionOob {
            section: "vertex",
            byte_offset: vstart,
            body_len: body.len(),
        }
        .into());
    }

    let mut vertices = Vec::with_capacity(weight1 + weight2);
    for i in 0..weight1 {
        let off = vstart + i * STRIDE1;
        let pos = [
            f32::from_le_bytes(body[off..off + 4].try_into().unwrap()),
            f32::from_le_bytes(body[off + 4..off + 8].try_into().unwrap()),
            f32::from_le_bytes(body[off + 8..off + 12].try_into().unwrap()),
        ];
        let normal = [
            f32::from_le_bytes(body[off + 12..off + 16].try_into().unwrap()),
            f32::from_le_bytes(body[off + 16..off + 20].try_into().unwrap()),
            f32::from_le_bytes(body[off + 20..off + 24].try_into().unwrap()),
        ];
        vertices.push(Vos2Vertex { pos, normal });
    }
    let mut bone_weights: Vec<Vos2BoneWeight> = Vec::with_capacity(weight2);
    for i in 0..weight2 {
        // SoA layout: x[0], x[1], y[0], y[1], z[0], z[1], w[0], w[1],
        // hx[0], hx[1], hy[0], hy[1], hz[0], hz[1]. We take the
        // weight-0 component as the bind-pose position; the full
        // weight pair + both positions/normals are surfaced via
        // `bone_weights` for the skinning path that wants them.
        let off = vstart + v1_bytes + i * STRIDE2;
        let read = |k: usize| f32::from_le_bytes(body[off + k..off + k + 4].try_into().unwrap());
        let pos1 = [read(0), read(8), read(16)];
        let pos2 = [read(4), read(12), read(20)];
        let weight1_val = read(24);
        let weight2_val = read(28);
        let normal1 = [read(32), read(40), read(48)];
        let normal2 = [read(36), read(44), read(52)];
        vertices.push(Vos2Vertex {
            pos: pos1,
            normal: normal1,
        });
        bone_weights.push(Vos2BoneWeight {
            weight1: weight1_val,
            weight2: weight2_val,
            pos1,
            pos2,
            normal1,
            normal2,
        });
    }

    // Poly walker. Cursor `p` advances through the body until an
    // unknown opcode terminates the walk.
    let groups = parse_poly_block(body, header.off_poly_bytes)?;

    // Bone palette: `bone_table_count` × u16 starting at
    // `off_bone_table_bytes`. Tolerate count=0 by yielding an empty
    // table — many meshes ship without one (the per-vertex bone_id
    // is then a direct skeleton index, gated by use_bone_table).
    let mut bone_table = Vec::with_capacity(header.bone_table_count as usize);
    if header.bone_table_count > 0 {
        let bt_end = header
            .off_bone_table_bytes
            .saturating_add(header.bone_table_count as usize * 2);
        if bt_end > body.len() {
            return Err(Vos2Error::SectionOob {
                section: "bone_table",
                byte_offset: header.off_bone_table_bytes,
                body_len: body.len(),
            }
            .into());
        }
        for i in 0..header.bone_table_count as usize {
            let o = header.off_bone_table_bytes + i * 2;
            bone_table.push(u16::from_le_bytes([body[o], body[o + 1]]));
        }
    }

    // Parallel bone-id stream: `bone_indices_count` × packed u16
    // records at `off_bone_bytes`. Same tolerance: count=0 → empty.
    let mut bone_indices = Vec::with_capacity(header.bone_indices_count as usize);
    if header.bone_indices_count > 0 {
        let bi_end = header
            .off_bone_bytes
            .saturating_add(header.bone_indices_count as usize * 2);
        if bi_end > body.len() {
            return Err(Vos2Error::SectionOob {
                section: "bone_indices",
                byte_offset: header.off_bone_bytes,
                body_len: body.len(),
            }
            .into());
        }
        for i in 0..header.bone_indices_count as usize {
            let o = header.off_bone_bytes + i * 2;
            let w = u16::from_le_bytes([body[o], body[o + 1]]);
            bone_indices.push(Vos2BoneIndices::from_u16(w));
        }
    }

    Ok(Vos2Mesh {
        header,
        vertices,
        groups,
        bone_table,
        bone_indices,
        bone_weights,
    })
}

fn parse_poly_block(body: &[u8], start: usize) -> Result<Vec<Vos2Group>> {
    let mut p = start;
    let mut groups: Vec<Vos2Group> = Vec::new();
    let mut tex_name = String::new();
    // Material state carried forward from the most-recent 0x8010
    // DrawState opcode. The opcode appears *before* the triangle
    // groups it applies to (lotus os2.cppm:151 — `meshes.push_back`
    // happens at DrawState, but in our parser groups push at the
    // triangle opcodes, so we cache the state and stamp it on each
    // group as it's emitted).
    let mut specular_exponent: f32 = 0.0;
    let mut specular_intensity: f32 = 0.0;

    while p + 4 <= body.len() {
        let wf = u16::from_le_bytes([body[p], body[p + 1]]);
        let ws = u16::from_le_bytes([body[p + 2], body[p + 3]]) as usize;

        // Texture-name opcode: wf & 0x80F0 == 0x8000 — 16-byte name
        // follows the wf word at offset +2 (overlaps where `ws`
        // would be — for this opcode `ws` is the first 2 bytes of
        // the texture name, not a count).
        if wf & 0x80F0 == 0x8010 {
            if p + 0x2E > body.len() {
                return Err(Vos2Error::PolyOob.into());
            }
            // DrawStateHeader layout (44 bytes after opcode):
            //   +0:  a[4]              (unknown)
            //   +4:  b[2]              (two floats, unknown)
            //   +12: c                 (uint32, unknown)
            //   +16: d[4]              (four floats, unknown)
            //   +32: e                 (uint32, unknown)
            //   +36: specular_exponent (f32)
            //   +40: specular_intensity(f32)
            let exp_off = p + 2 + 36;
            let int_off = p + 2 + 40;
            specular_exponent =
                f32::from_le_bytes(body[exp_off..exp_off + 4].try_into().unwrap_or([0; 4]));
            specular_intensity =
                f32::from_le_bytes(body[int_off..int_off + 4].try_into().unwrap_or([0; 4]));
            p += 0x2E;
            continue;
        }
        if wf & 0x80F0 == 0x8000 {
            if p + 0x12 > body.len() {
                return Err(Vos2Error::PolyOob.into());
            }
            let name_bytes = &body[p + 2..p + 18];
            tex_name = name_bytes
                .iter()
                .take_while(|&&b| b != 0)
                .map(|&b| b as char)
                .collect::<String>()
                .trim_end()
                .to_string();
            p += 0x12;
            continue;
        }

        match wf {
            0x5453 => {
                // 'ST' triangle strip. Header word at +4, then 30-byte
                // SFace3, then (ws-1)*10-byte SFace records.
                let header_size = 4;
                let strip_bytes = 30 + ws.saturating_sub(1) * 10;
                if p + header_size + strip_bytes > body.len() || ws == 0 {
                    return Err(Vos2Error::PolyOob.into());
                }
                let triangles = parse_strip(&body[p + header_size..], ws)?;
                groups.push(Vos2Group {
                    texture_name: tex_name.clone(),
                    triangles,
                    specular_exponent,
                    specular_intensity,
                });
                p += header_size + strip_bytes;
            }
            0x0054 => {
                // 'T' triangle list. `ws` × 30-byte SFace3.
                let header_size = 4;
                let body_bytes = ws * 30;
                if p + header_size + body_bytes > body.len() {
                    return Err(Vos2Error::PolyOob.into());
                }
                let triangles = parse_tri_list(&body[p + header_size..], ws);
                groups.push(Vos2Group {
                    texture_name: tex_name.clone(),
                    triangles,
                    specular_exponent,
                    specular_intensity,
                });
                p += header_size + body_bytes;
            }
            0x4353 => {
                // 'SC' cloth strip. Skip.
                p += ws * 20 + 0x0C;
            }
            0x0043 => {
                // 'C' cloth list. Skip.
                p += ws * 10 + 0x04;
            }
            _ => break, // unknown opcode = terminator
        }
    }

    Ok(groups)
}

fn parse_tri_list(buf: &[u8], count: usize) -> Vec<Vos2Triangle> {
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let off = i * 30;
        if off + 30 > buf.len() {
            break;
        }
        let i0 = u16::from_le_bytes([buf[off], buf[off + 1]]);
        let i1 = u16::from_le_bytes([buf[off + 2], buf[off + 3]]);
        let i2 = u16::from_le_bytes([buf[off + 4], buf[off + 5]]);
        let read_uv = |k: usize| -> [f32; 2] {
            [
                f32::from_le_bytes(buf[off + k..off + k + 4].try_into().unwrap()),
                f32::from_le_bytes(buf[off + k + 4..off + k + 8].try_into().unwrap()),
            ]
        };
        out.push(Vos2Triangle {
            indices: [i0, i1, i2],
            uvs: [read_uv(6), read_uv(14), read_uv(22)],
        });
    }
    out
}

/// Decode a triangle strip. First record is 30 bytes (full SFace3
/// with 3 corners). Subsequent `ws - 1` records are 10 bytes each
/// (one corner: u16 idx + vec2 uv) and extend the strip by emitting
/// one new triangle per corner with alternating winding.
fn parse_strip(buf: &[u8], corner_count: usize) -> Result<Vec<Vos2Triangle>> {
    if corner_count == 0 || buf.len() < 30 {
        return Ok(Vec::new());
    }
    // Read the first triangle.
    let i0 = u16::from_le_bytes([buf[0], buf[1]]);
    let i1 = u16::from_le_bytes([buf[2], buf[3]]);
    let i2 = u16::from_le_bytes([buf[4], buf[5]]);
    let read_uv = |o: usize| -> [f32; 2] {
        [
            f32::from_le_bytes(buf[o..o + 4].try_into().unwrap()),
            f32::from_le_bytes(buf[o + 4..o + 8].try_into().unwrap()),
        ]
    };
    let uv0 = read_uv(6);
    let uv1 = read_uv(14);
    let uv2 = read_uv(22);

    let mut tris = Vec::with_capacity(corner_count.saturating_sub(2));
    let mut prev2 = (i0, uv0);
    let mut prev1 = (i1, uv1);
    let mut cur = (i2, uv2);
    tris.push(Vos2Triangle {
        indices: [prev2.0, prev1.0, cur.0],
        uvs: [prev2.1, prev1.1, cur.1],
    });

    // Extend strip: each subsequent 10-byte record adds one corner.
    let mut flip = false;
    let mut p = 30;
    for _ in 0..corner_count.saturating_sub(3) {
        if p + 10 > buf.len() {
            break;
        }
        let idx = u16::from_le_bytes([buf[p], buf[p + 1]]);
        let uv = read_uv(p + 2);
        prev2 = prev1;
        prev1 = cur;
        cur = (idx, uv);
        let tri = if flip {
            Vos2Triangle {
                indices: [prev1.0, prev2.0, cur.0],
                uvs: [prev1.1, prev2.1, cur.1],
            }
        } else {
            Vos2Triangle {
                indices: [prev2.0, prev1.0, cur.0],
                uvs: [prev2.1, prev1.1, cur.1],
            }
        };
        tris.push(tri);
        flip = !flip;
        p += 10;
    }
    Ok(tris)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Header parse using the known offsets from chunk[4] of file
    /// 13746 (Kuu Mohzolhil body equip), captured live:
    ///   ver=0x01, type=0x1100, flip=0x0001
    ///   offsetPoly=0x0020 → byte 0x0040
    ///   offsetVertex=0x1110 → byte 0x2220
    ///   offsetWeight=0x0F5C → byte 0x1EB8
    ///   PolyLod2Suu=0 (1-bone path active)
    #[test]
    fn header_decodes_known_chunk_offsets() {
        let mut buf = vec![0u8; 0x40];
        buf[0] = 0x01; // ver
                       // type @ 0x02 = 0x1100
        buf[2..4].copy_from_slice(&0x1100u16.to_le_bytes());
        // flip @ 0x04 = 0x0001
        buf[4..6].copy_from_slice(&0x0001u16.to_le_bytes());
        // offsetPoly @ 0x06 = 0x20
        buf[6..10].copy_from_slice(&0x0020u32.to_le_bytes());
        // offsetWeight @ 0x12 = 0x0F5C
        buf[0x12..0x16].copy_from_slice(&0x00000F5Cu32.to_le_bytes());
        // offsetVertex @ 0x1E = 0x1110
        buf[0x1E..0x22].copy_from_slice(&0x00001110u32.to_le_bytes());
        // PolyLod2Suu @ 0x32 = 0
        buf[0x32..0x34].copy_from_slice(&0u16.to_le_bytes());

        let h = Vos2Header::parse(&buf).unwrap();
        assert_eq!(h.version, 0x01);
        assert_eq!(h.kind_type, 0x1100);
        assert_eq!(h.flip, 0x0001);
        assert_eq!(h.off_poly_bytes, 0x40);
        assert_eq!(h.off_weight_bytes, 0x1EB8);
        assert_eq!(h.off_vertex_bytes, 0x2220);
        assert_eq!(h.poly_lod2_count, 0);
    }

    #[test]
    fn header_too_small_errors() {
        let buf = vec![0u8; 32];
        assert!(Vos2Header::parse(&buf).is_err());
    }

    /// Synth a minimal but valid VertexOs2 chunk: 3 1-bone vertices
    /// forming one triangle, polygon block with a single 'T'
    /// triangle-list opcode for 1 triangle.
    fn synth_minimal() -> Vec<u8> {
        const VSTART: usize = 0x80; // arbitrary, > 0x40 header
        const POLYSTART: usize = 0x40;
        const WSTART: usize = 0x70;
        const VERTEX_COUNT: usize = 3;
        // Total size = vstart + 3*24 + slop. Use 0x200 to leave room.
        let mut buf = vec![0u8; 0x200];

        // Header
        buf[0] = 0x01;
        // offsetPoly @ 0x06: word units → POLYSTART/2 = 0x20
        buf[6..10].copy_from_slice(&((POLYSTART as u32) / 2).to_le_bytes());
        // offsetWeight @ 0x12: WSTART/2
        buf[0x12..0x16].copy_from_slice(&((WSTART as u32) / 2).to_le_bytes());
        // offsetVertex @ 0x1E: VSTART/2
        buf[0x1E..0x22].copy_from_slice(&((VSTART as u32) / 2).to_le_bytes());

        // weight1=3, weight2=0
        buf[WSTART..WSTART + 2].copy_from_slice(&(VERTEX_COUNT as i16).to_le_bytes());
        buf[WSTART + 2..WSTART + 4].copy_from_slice(&0i16.to_le_bytes());

        // 3 vertices @ 24 bytes each
        for i in 0..VERTEX_COUNT {
            let off = VSTART + i * 24;
            // pos
            buf[off..off + 4].copy_from_slice(&(i as f32).to_le_bytes());
            buf[off + 4..off + 8].copy_from_slice(&(i as f32 + 0.5).to_le_bytes());
            buf[off + 8..off + 12].copy_from_slice(&(i as f32 + 1.0).to_le_bytes());
            // normal (0, 1, 0)
            buf[off + 12..off + 16].copy_from_slice(&0f32.to_le_bytes());
            buf[off + 16..off + 20].copy_from_slice(&1f32.to_le_bytes());
            buf[off + 20..off + 24].copy_from_slice(&0f32.to_le_bytes());
        }

        // Poly block: 'T' opcode (0x0054), ws=1, then 1× SFace3
        buf[POLYSTART..POLYSTART + 2].copy_from_slice(&0x0054u16.to_le_bytes());
        buf[POLYSTART + 2..POLYSTART + 4].copy_from_slice(&1u16.to_le_bytes());
        // SFace3 at POLYSTART+4: indices (0,1,2), uvs (0,0),(1,0),(0,1)
        let face_off = POLYSTART + 4;
        buf[face_off..face_off + 2].copy_from_slice(&0u16.to_le_bytes());
        buf[face_off + 2..face_off + 4].copy_from_slice(&1u16.to_le_bytes());
        buf[face_off + 4..face_off + 6].copy_from_slice(&2u16.to_le_bytes());
        buf[face_off + 6..face_off + 10].copy_from_slice(&0f32.to_le_bytes());
        buf[face_off + 10..face_off + 14].copy_from_slice(&0f32.to_le_bytes());
        buf[face_off + 14..face_off + 18].copy_from_slice(&1f32.to_le_bytes());
        buf[face_off + 18..face_off + 22].copy_from_slice(&0f32.to_le_bytes());
        buf[face_off + 22..face_off + 26].copy_from_slice(&0f32.to_le_bytes());
        buf[face_off + 26..face_off + 30].copy_from_slice(&1f32.to_le_bytes());

        // Terminator: leave next u16 as 0x0000 — none of the known
        // opcodes match, so the walker stops cleanly.

        buf
    }

    #[test]
    fn parses_minimal_synthetic_chunk() {
        let bytes = synth_minimal();
        let mesh = parse_vos2(&bytes).unwrap();
        assert_eq!(mesh.vertices.len(), 3);
        assert_eq!(mesh.vertices[0].pos, [0.0, 0.5, 1.0]);
        assert_eq!(mesh.vertices[2].pos, [2.0, 2.5, 3.0]);
        assert_eq!(mesh.vertices[0].normal, [0.0, 1.0, 0.0]);

        assert_eq!(mesh.groups.len(), 1);
        let g = &mesh.groups[0];
        assert_eq!(g.triangles.len(), 1);
        assert_eq!(g.triangles[0].indices, [0, 1, 2]);
        assert_eq!(g.triangles[0].uvs[1], [1.0, 0.0]);
    }

    /// Sanity check: ensure the strip extender produces the right
    /// triangle count for `corner_count` corners.
    #[test]
    fn strip_extender_emits_n_minus_2_triangles() {
        // 5 corners → 3 triangles (one from SFace3, two from extends).
        // SFace3 (30 bytes) + 2× SFace (10 bytes each) = 50 bytes.
        let mut buf = vec![0u8; 50];
        // First triangle (0,1,2) with zero UVs
        buf[0..2].copy_from_slice(&0u16.to_le_bytes());
        buf[2..4].copy_from_slice(&1u16.to_le_bytes());
        buf[4..6].copy_from_slice(&2u16.to_le_bytes());
        // Two extender corners: idx 3, then idx 4
        buf[30..32].copy_from_slice(&3u16.to_le_bytes());
        buf[40..42].copy_from_slice(&4u16.to_le_bytes());

        let tris = parse_strip(&buf, 5).unwrap();
        assert_eq!(tris.len(), 3);
        assert_eq!(tris[0].indices, [0, 1, 2]);
        // Strip extender slides the (prev2, prev1, cur) window. After
        // tri 0, state is (0,1,2). Corner 3 advances to (1,2,3); flip
        // starts false so we emit `[prev2, prev1, cur] = [1,2,3]`
        // (same winding as tri 0). Next corner 4 advances to (2,3,4)
        // with flip=true, emitting `[prev1, prev2, cur] = [3,2,4]`
        // (reversed winding). The renderer uses `cull_mode: None`
        // because FFXI strip winding isn't consistent across
        // sub-records anyway — this test just pins the chosen
        // convention so a future PR doesn't silently flip it.
        assert_eq!(tris[1].indices, [1, 2, 3]);
        assert_eq!(tris[2].indices, [3, 2, 4]);
    }

    /// Bit-unpack the packed bone-indices word. The layout is
    /// lifted from lotus-ffxi `os2.cppm` and the test pins all three
    /// fields against a hand-rolled word so a future "simplify" PR
    /// can't quietly reshape it.
    #[test]
    fn bone_indices_unpacks_bitfields() {
        // bone1=0x03, bone2=0x05, mirror=0x02
        //   = (0x02 << 14) | (0x05 << 7) | 0x03
        //   = 0x8000 | 0x0280 | 0x0003 = 0x8283
        let bi = Vos2BoneIndices::from_u16(0x8283);
        assert_eq!(bi.bone_index1, 0x03);
        assert_eq!(bi.bone_index2, 0x05);
        assert_eq!(bi.mirror_axis, 0x02);
    }

    #[test]
    fn use_bone_table_reads_bit7_of_kind_type() {
        let mut h = Vos2Header::parse(&{
            let mut buf = vec![0u8; 0x40];
            buf[2..4].copy_from_slice(&0x0080u16.to_le_bytes()); // bit 7 set
            buf
        })
        .unwrap();
        assert!(h.use_bone_table(), "bit 7 set must report true");
        h.kind_type = 0x007F;
        assert!(!h.use_bone_table(), "bit 7 clear must report false");
    }

    /// End-to-end: a synthetic chunk with a 2-entry bone palette
    /// and one bone-indices record. `skeleton_bone_for(0)` must
    /// indirect through `bone_table` when the use_bone_table flag is
    /// set, and return the raw index when it isn't.
    #[test]
    fn skeleton_bone_for_honors_use_bone_table_flag() {
        // Hand-build a minimal Vos2Mesh; bypass parse_vos2 since we
        // just want to exercise the resolver logic.
        let mut mesh = Vos2Mesh {
            header: Vos2Header::parse(&vec![0u8; 0x40]).unwrap(),
            vertices: vec![Vos2Vertex {
                pos: [0.0; 3],
                normal: [0.0; 3],
            }],
            groups: vec![],
            // palette: [0]=bone 17, [1]=bone 42
            bone_table: vec![17, 42],
            // vertex 0 → bone_indices[0]: bone_index1 = 1 (palette[1])
            bone_indices: vec![
                Vos2BoneIndices {
                    bone_index1: 1,
                    bone_index2: 0,
                    mirror_axis: 0,
                },
                // second slot for vertex 0 (rigid verts consume 2)
                Vos2BoneIndices {
                    bone_index1: 0,
                    bone_index2: 0,
                    mirror_axis: 0,
                },
            ],
            bone_weights: vec![],
        };

        // use_bone_table = true → resolver goes through palette.
        mesh.header.kind_type = 0x0080;
        assert_eq!(mesh.skeleton_bone_for(0), Some(42));

        // use_bone_table = false → raw value is the skeleton index.
        mesh.header.kind_type = 0x0000;
        assert_eq!(mesh.skeleton_bone_for(0), Some(1));

        // Out-of-range vertex → None, never panics.
        assert_eq!(mesh.skeleton_bone_for(100), None);
    }

    /// Parser populates bone_table and bone_indices from a real
    /// (synthetic) layout, with both sections inside the body. This
    /// is the integration-shaped test — anything that breaks the
    /// header offsets or the bounds-check will trip it.
    #[test]
    fn parse_populates_bone_table_and_indices() {
        const VSTART: usize = 0x80;
        const POLYSTART: usize = 0x40;
        const WSTART: usize = 0x70;
        const BTSTART: usize = 0x100;
        const BISTART: usize = 0x110;
        const VERTEX_COUNT: usize = 2;
        let mut buf = vec![0u8; 0x200];

        // Header — word units for all offsets (*2 for bytes).
        buf[0] = 0x01;
        // type: bit 7 set → use_bone_table = true
        buf[2..4].copy_from_slice(&0x0080u16.to_le_bytes());
        // offsetPoly @ 0x06
        buf[6..10].copy_from_slice(&((POLYSTART as u32) / 2).to_le_bytes());
        // offsetBoneTbl @ 0x0C, BoneTblSuu @ 0x10 = 3
        buf[0x0C..0x10].copy_from_slice(&((BTSTART as u32) / 2).to_le_bytes());
        buf[0x10..0x12].copy_from_slice(&3u16.to_le_bytes());
        // offsetWeight @ 0x12
        buf[0x12..0x16].copy_from_slice(&((WSTART as u32) / 2).to_le_bytes());
        // offsetBone @ 0x18, BoneSuu @ 0x1C = 4
        buf[0x18..0x1C].copy_from_slice(&((BISTART as u32) / 2).to_le_bytes());
        buf[0x1C..0x1E].copy_from_slice(&4u16.to_le_bytes());
        // offsetVertex @ 0x1E
        buf[0x1E..0x22].copy_from_slice(&((VSTART as u32) / 2).to_le_bytes());

        // weight1=2, weight2=0
        buf[WSTART..WSTART + 2].copy_from_slice(&(VERTEX_COUNT as i16).to_le_bytes());
        buf[WSTART + 2..WSTART + 4].copy_from_slice(&0i16.to_le_bytes());

        // 2 vertices, identity positions
        for i in 0..VERTEX_COUNT {
            let off = VSTART + i * 24;
            buf[off..off + 4].copy_from_slice(&(i as f32).to_le_bytes());
        }

        // Bone table: [10, 20, 30]
        for (i, &v) in [10u16, 20, 30].iter().enumerate() {
            buf[BTSTART + i * 2..BTSTART + i * 2 + 2].copy_from_slice(&v.to_le_bytes());
        }

        // Bone indices stream: vertex 0 → palette[1]=20; vertex 1 → palette[2]=30
        // (rigid verts consume 2 records each; only the first matters).
        let bi0 = (0u16 << 14) | (0u16 << 7) | 1u16; // bone1=1
        let bi1 = 0u16; // pad slot
        let bi2 = (0u16 << 14) | (0u16 << 7) | 2u16; // bone1=2
        let bi3 = 0u16;
        for (i, &w) in [bi0, bi1, bi2, bi3].iter().enumerate() {
            buf[BISTART + i * 2..BISTART + i * 2 + 2].copy_from_slice(&w.to_le_bytes());
        }

        // Empty poly block — terminator at POLYSTART (already zero).

        let mesh = parse_vos2(&buf).unwrap();
        assert_eq!(mesh.bone_table, vec![10, 20, 30]);
        assert_eq!(mesh.bone_indices.len(), 4);
        assert_eq!(mesh.bone_indices[0].bone_index1, 1);
        assert_eq!(mesh.bone_indices[2].bone_index1, 2);

        // Resolver: vertex 0 → palette[1] = 20; vertex 1 → palette[2] = 30.
        assert_eq!(mesh.skeleton_bone_for(0), Some(20));
        assert_eq!(mesh.skeleton_bone_for(1), Some(30));
    }
}
