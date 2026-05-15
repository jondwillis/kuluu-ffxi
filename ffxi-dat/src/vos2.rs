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
    pub off_weight_bytes: usize,
    pub off_bone_bytes: usize,
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
            off_weight_bytes: u32_at(0x12) as usize * 2,
            off_bone_bytes: u32_at(0x18) as usize * 2,
            off_vertex_bytes: u32_at(0x1E) as usize * 2,
            off_poly_load_bytes: u32_at(0x24) as usize * 2,
            poly_lod2_count: u16_at(0x32),
        })
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
}

/// Top-level parsed VertexOs2 chunk: bind-pose vertex pool + groups
/// of triangles. Equivalent to one MMB sub-record from the caller's
/// perspective — but a VertexOs2 chunk may have many groups, each
/// with its own texture.
#[derive(Debug, Clone)]
pub struct Vos2Mesh {
    pub header: Vos2Header,
    pub vertices: Vec<Vos2Vertex>,
    pub groups: Vec<Vos2Group>,
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
    for i in 0..weight2 {
        // SoA layout: x[0], x[1], y[0], y[1], z[0], z[1], w[0], w[1],
        // hx[0], hx[1], hy[0], hy[1], hz[0], hz[1]. We take the
        // weight-0 component as the bind-pose position.
        let off = vstart + v1_bytes + i * STRIDE2;
        let read = |k: usize| f32::from_le_bytes(body[off + k..off + k + 4].try_into().unwrap());
        let pos = [read(0), read(8), read(16)];
        let normal = [read(32), read(40), read(48)];
        vertices.push(Vos2Vertex { pos, normal });
    }

    // Poly walker. Cursor `p` advances through the body until an
    // unknown opcode terminates the walk.
    let groups = parse_poly_block(body, header.off_poly_bytes)?;

    Ok(Vos2Mesh {
        header,
        vertices,
        groups,
    })
}

fn parse_poly_block(body: &[u8], start: usize) -> Result<Vec<Vos2Group>> {
    let mut p = start;
    let mut groups: Vec<Vos2Group> = Vec::new();
    let mut tex_name = String::new();

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
}
