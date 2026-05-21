//! `D3M` chunk parser — kind `0x1F`. Particle / effect-billboard
//! triangle geometry.
//!
//! D3M chunks live alongside Generators inside action/effect DAT
//! files. When a Scheduler stage fires a particle-type Generator
//! (`Generator::effect_type ∈ {0x3B, 0x3C, …}`), the generator's
//! `id` field names a sibling D3M chunk whose triangles get spawned
//! as billboards at the actor's bone position.
//!
//! Wire layout (port of `vendor/lotus-ffxi/ffxi/dat/d3m.cppm:137`):
//!
//! ```text
//! 0x00..0x04  u32 magic = 6                  (lotus asserts == 6)
//! 0x04        u8  numimg                     (texture count? unread)
//! 0x05        u8  numnimg                    (unread)
//! 0x06..0x08  u16 num_triangles              (vertex count = 3×this)
//! 0x08..0x0E  u16[3] numtri1..3              (LOD/section counts; unread)
//! 0x0E..0x1E  char[16] texture_name          (NUL/space padded)
//! 0x1E..      DatVertexD3M[num_triangles*3]  (36-byte stride)
//! ```
//!
//! Vertex stride is **36 bytes** (matches `DatVertexD3M`):
//! ```text
//! 0x00..0x0C  vec3 pos
//! 0x0C..0x18  vec3 normal
//! 0x18..0x1C  u32  color (ARGB, each byte / 128.0 → HDR-capable)
//! 0x1C..0x24  vec2 uv
//! ```
//!
//! D3M expands triangles inline (no index buffer); each consecutive
//! 3-vertex group is one triangle. Lotus's renderer wraps each D3M in
//! three pipelines (add / blend / sub) for additive / alpha-blended /
//! subtractive particle modes; the *blend selection* is a per-mesh
//! property at the Generator level, not encoded in the D3M itself.
//!
//! D3A (animated rect quads) is a sibling chunk kind with a related
//! layout; not parsed here yet — see lotus `d3m.cppm:156` for format.

use crate::{DatError, Result};

/// Expected magic word at offset 0. Lotus asserts on this.
pub const D3M_MAGIC: u32 = 6;

/// Bytes per `DatVertexD3M` on disk.
pub const D3M_VERTEX_STRIDE: usize = 36;

/// Bytes before the vertex array (header + texture name).
pub const D3M_VERTEX_OFFSET: usize = 0x1E;

/// One D3M vertex, decoded. Color channels are floats so the
/// /128.0 HDR convention round-trips without loss; consumers that
/// only want LDR can clamp to `[0.0, 1.0]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct D3mVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    /// RGBA in `[0.0, 2.0]` (each on-disk byte / 128.0). Lotus emits
    /// these straight into a glm::vec4 and uses values above 1.0 to
    /// drive bloom; we keep the same convention.
    pub color: [f32; 4],
    pub uv: [f32; 2],
}

/// Parsed D3M chunk. `vertices.len() == 3 * num_triangles` — D3M is
/// stored as a flat triangle list with no index buffer.
#[derive(Debug, Clone)]
pub struct D3m {
    /// 4-char chunk id from the enclosing DAT header.
    pub name: [u8; 4],
    /// `num_triangles` field at 0x06.
    pub num_triangles: u16,
    /// 16-byte texture-name field (NUL/space padded; raw bytes).
    pub texture_name: [u8; 16],
    pub vertices: Vec<D3mVertex>,
}

impl D3m {
    pub fn parse(name: [u8; 4], body: &[u8]) -> Result<Self> {
        if body.len() < D3M_VERTEX_OFFSET {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: D3M_VERTEX_OFFSET,
                available: body.len(),
            });
        }
        let magic = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
        if magic != D3M_MAGIC {
            // Lotus asserts; we treat as a structural error so callers
            // can fall through to the opaque-chunk path instead of
            // aborting the whole DAT walk.
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: D3M_MAGIC as usize,
                available: magic as usize,
            });
        }
        let num_triangles = u16::from_le_bytes([body[0x06], body[0x07]]);
        let mut texture_name = [0u8; 16];
        texture_name.copy_from_slice(&body[0x0E..0x1E]);

        let vertex_count = num_triangles as usize * 3;
        let needed = D3M_VERTEX_OFFSET + vertex_count * D3M_VERTEX_STRIDE;
        if body.len() < needed {
            return Err(DatError::TruncatedChunk {
                offset: D3M_VERTEX_OFFSET,
                needed,
                available: body.len(),
            });
        }

        let mut vertices = Vec::with_capacity(vertex_count);
        for i in 0..vertex_count {
            let off = D3M_VERTEX_OFFSET + i * D3M_VERTEX_STRIDE;
            let pos = [
                f32_le(body, off),
                f32_le(body, off + 4),
                f32_le(body, off + 8),
            ];
            let normal = [
                f32_le(body, off + 12),
                f32_le(body, off + 16),
                f32_le(body, off + 20),
            ];
            // Color stored as ARGB-in-u32; each channel byte / 128.0.
            // Lotus reads (color & 0xFF0000) >> 16 for R, etc., so byte 2 is
            // R, byte 1 is G, byte 0 is B, byte 3 is A.
            let raw = u32::from_le_bytes([
                body[off + 24],
                body[off + 25],
                body[off + 26],
                body[off + 27],
            ]);
            let color = [
                ((raw >> 16) & 0xFF) as f32 / 128.0,
                ((raw >> 8) & 0xFF) as f32 / 128.0,
                (raw & 0xFF) as f32 / 128.0,
                ((raw >> 24) & 0xFF) as f32 / 128.0,
            ];
            let uv = [f32_le(body, off + 28), f32_le(body, off + 32)];
            vertices.push(D3mVertex {
                pos,
                normal,
                color,
                uv,
            });
        }

        Ok(Self {
            name,
            num_triangles,
            texture_name,
            vertices,
        })
    }

    /// Texture-name as a `String`, NUL- and space-trimmed.
    pub fn texture_name_str(&self) -> String {
        self.texture_name
            .iter()
            .copied()
            .take_while(|&b| b != 0)
            .map(|b| b as char)
            .collect::<String>()
            .trim_end()
            .to_string()
    }
}

#[inline]
fn f32_le(b: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_body(num_triangles: u16, texture_name: &[u8; 16]) -> Vec<u8> {
        let vert_count = num_triangles as usize * 3;
        let mut body = Vec::with_capacity(D3M_VERTEX_OFFSET + vert_count * D3M_VERTEX_STRIDE);
        body.extend_from_slice(&D3M_MAGIC.to_le_bytes()); // 0x00..0x04
        body.push(1); // numimg @ 0x04
        body.push(0); // numnimg @ 0x05
        body.extend_from_slice(&num_triangles.to_le_bytes()); // 0x06..0x08
        body.extend_from_slice(&[0u8; 6]); // numtri1..3 @ 0x08..0x0E
        body.extend_from_slice(texture_name); // 0x0E..0x1E
        body
    }

    fn append_vertex(body: &mut Vec<u8>, pos: [f32; 3], rgba_u8: [u8; 4], uv: [f32; 2]) {
        for c in pos {
            body.extend_from_slice(&c.to_le_bytes());
        }
        // normal: (0, 1, 0)
        for c in [0.0f32, 1.0, 0.0] {
            body.extend_from_slice(&c.to_le_bytes());
        }
        // Color packed as little-endian u32; bytes are [B, G, R, A] in memory
        // so that (raw >> 16) gives R, etc.
        body.push(rgba_u8[2]); // B
        body.push(rgba_u8[1]); // G
        body.push(rgba_u8[0]); // R
        body.push(rgba_u8[3]); // A
        for c in uv {
            body.extend_from_slice(&c.to_le_bytes());
        }
    }

    #[test]
    fn parses_single_triangle() {
        let mut body = build_body(1, b"flame_a\0\0\0\0\0\0\0\0\0");
        append_vertex(&mut body, [0.0, 0.0, 0.0], [128, 64, 32, 255], [0.0, 0.0]);
        append_vertex(&mut body, [1.0, 0.0, 0.0], [128, 64, 32, 255], [1.0, 0.0]);
        append_vertex(&mut body, [0.5, 1.0, 0.0], [128, 64, 32, 255], [0.5, 1.0]);

        let d = D3m::parse(*b"d3m0", &body).unwrap();
        assert_eq!(d.num_triangles, 1);
        assert_eq!(d.texture_name_str(), "flame_a");
        assert_eq!(d.vertices.len(), 3);

        // First vertex: pos (0,0,0), color (128/128, 64/128, 32/128, 255/128) = (1.0, 0.5, 0.25, ~1.99)
        let v0 = d.vertices[0];
        assert_eq!(v0.pos, [0.0, 0.0, 0.0]);
        assert!((v0.color[0] - 1.0).abs() < 1e-5);
        assert!((v0.color[1] - 0.5).abs() < 1e-5);
        assert!((v0.color[2] - 0.25).abs() < 1e-5);
        assert!((v0.color[3] - 255.0 / 128.0).abs() < 1e-5);
    }

    #[test]
    fn rejects_wrong_magic() {
        let mut body = vec![0u8; D3M_VERTEX_OFFSET];
        body[0..4].copy_from_slice(&7u32.to_le_bytes()); // not 6
        assert!(D3m::parse(*b"badm", &body).is_err());
    }

    #[test]
    fn rejects_truncated_vertex_array() {
        // Declares 5 triangles (15 vertices) but provides 0 bytes of
        // vertex data — must error, not panic.
        let body = build_body(5, &[0u8; 16]);
        assert!(D3m::parse(*b"trun", &body).is_err());
    }

    #[test]
    fn texture_name_trims_padding() {
        let mut body = build_body(0, b"abc\0\0\0\0\0\0\0\0\0\0\0\0\0");
        // No vertices; still valid because num_triangles = 0.
        assert_eq!(body.len(), D3M_VERTEX_OFFSET);
        let d = D3m::parse(*b"name", &body).unwrap();
        assert_eq!(d.texture_name_str(), "abc");
        body.truncate(0); // suppress unused warning
    }
}
