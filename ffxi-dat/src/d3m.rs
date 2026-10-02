use crate::mmb::D3DCOLOR_CHANNEL_MASK;
use crate::{DatError, Result};

// research/XIClient/src/XIClient/source/Resource/Derived/CMoD3m.cpp CMoD3m::Open — the low nibble of
// the header word is the layout marker; 5 and 6 are the vertex-array layouts, 7 is what Open
// stamps on a resource whose first vertices fail its sanity check, and below 5 is a different
// resource shape.
pub const D3M_MAGIC: u32 = 6;
const D3M_MARKER_MASK: u16 = 0xF;
const D3M_VERTEX_ARRAY_MARKERS: std::ops::RangeInclusive<u16> = 5..=6;

pub const D3M_VERTEX_STRIDE: usize = 36;
const D3M_MATERIAL_LEN: usize = 16;
const D3M_MAT_COUNT_OFFSET: usize = 0x04;
const D3M_EXTRA_COUNT_OFFSET: usize = 0x05;
const D3M_TRI_COUNT_OFFSET: usize = 0x06;
const D3M_COUNT_TABLE_OFFSET: usize = 0x08;

/// The one-material layout every shipped effect mesh but two uses: vertices follow the
/// single 16-byte material at `material_table_offset(1, 0)`.
pub const D3M_VERTEX_OFFSET: usize = 0x1E;

// research/XIClient/src/XIClient/source/Resource/Derived/CMoD3m.cpp CMoD3m::Open
// GetShortPointer — the per-entry triangle-count table is padded by rounding the ENTRY count
// down to a multiple of four and adding three, not by aligning the byte offset: n = 1..=3 all
// put the material table at byte 14, n = 4 at 16, n = 5..=7 at 22. A byte-aligned reader
// agrees for the shipped one-material meshes and reads two bytes into every vertex past that.
pub fn material_table_offset(mat_count: usize, extra_count: usize) -> usize {
    let n = mat_count + extra_count;
    let shorts = if n.is_multiple_of(4) {
        n
    } else {
        n - n % 4 + 3
    };
    D3M_COUNT_TABLE_OFFSET + 2 * shorts
}

pub fn vertex_offset(mat_count: usize, extra_count: usize) -> usize {
    material_table_offset(mat_count, extra_count) + D3M_MATERIAL_LEN * mat_count
}

// D3m vertex colour is normalised by 128 rather than 255, folding the D3m texture-stage-0
// MODULATE2X into the stored value. Distinct from `mmb::VERTEX_COLOR_DIVISOR`, which is the
// plain D3DCOLOR byte/255 because the zone shader models that MODULATE2X itself.
// research/XIClient/src/XIClient/source/Resource/Derived/CMoD3m.cpp ZeroOneTSS
pub const VERTEX_COLOR_DIVISOR: f32 = 128.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct D3mVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],

    pub color: [f32; 4],
    pub uv: [f32; 2],
}

#[derive(Debug, Clone)]
pub struct D3m {
    pub name: [u8; 4],

    pub num_triangles: u16,

    pub texture_name: [u8; 16],
    pub vertices: Vec<D3mVertex>,
}

impl D3m {
    pub fn parse(name: [u8; 4], body: &[u8]) -> Result<Self> {
        if body.len() < D3M_COUNT_TABLE_OFFSET {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: D3M_COUNT_TABLE_OFFSET,
                available: body.len(),
            });
        }
        let marker = u16::from_le_bytes([body[0], body[1]]) & D3M_MARKER_MASK;
        if !D3M_VERTEX_ARRAY_MARKERS.contains(&marker) {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: D3M_MAGIC as usize,
                available: marker as usize,
            });
        }
        let mat_count = body[D3M_MAT_COUNT_OFFSET] as usize;
        let extra_count = body[D3M_EXTRA_COUNT_OFFSET] as usize;
        let num_triangles =
            u16::from_le_bytes([body[D3M_TRI_COUNT_OFFSET], body[D3M_TRI_COUNT_OFFSET + 1]]);
        let materials_at = material_table_offset(mat_count, extra_count);
        let vertices_at = vertex_offset(mat_count, extra_count);
        if body.len() < vertices_at {
            return Err(DatError::TruncatedChunk {
                offset: materials_at,
                needed: vertices_at,
                available: body.len(),
            });
        }
        let mut texture_name = [0u8; 16];
        if mat_count > 0 {
            texture_name.copy_from_slice(&body[materials_at..materials_at + D3M_MATERIAL_LEN]);
        }

        let vertex_count = num_triangles as usize * 3;
        let needed = vertices_at + vertex_count * D3M_VERTEX_STRIDE;
        if body.len() < needed {
            return Err(DatError::TruncatedChunk {
                offset: vertices_at,
                needed,
                available: body.len(),
            });
        }

        let mut vertices = Vec::with_capacity(vertex_count);
        for i in 0..vertex_count {
            let off = vertices_at + i * D3M_VERTEX_STRIDE;
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

            let raw = u32::from_le_bytes([
                body[off + 24],
                body[off + 25],
                body[off + 26],
                body[off + 27],
            ]);
            let color = [
                ((raw >> 16) & D3DCOLOR_CHANNEL_MASK) as f32 / VERTEX_COLOR_DIVISOR,
                ((raw >> 8) & D3DCOLOR_CHANNEL_MASK) as f32 / VERTEX_COLOR_DIVISOR,
                (raw & D3DCOLOR_CHANNEL_MASK) as f32 / VERTEX_COLOR_DIVISOR,
                ((raw >> 24) & D3DCOLOR_CHANNEL_MASK) as f32 / VERTEX_COLOR_DIVISOR,
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

    // research/xim ParticleMeshSection.kt read textureName — a mesh links its texture by the raw 16-byte
    // qualified name, resolved as (namespace, local) then local-only.
    pub fn texture_name_tokens(&self) -> (String, String) {
        crate::texture::split_qualified_name(&self.texture_name)
    }

    // The legacy texture key: the raw padded bytes at NAMESPACE_LEN..+4, correct only when they
    // happen to equal the backing Img chunk's 4-byte DatId. `pou` yields `pou ` and never
    // matches chunk `pou1`; `kumori` yields `kumo` and matches only by truncation.
    pub fn texture_dat_id(&self) -> [u8; 4] {
        let local = &self.texture_name[crate::texture::NAMESPACE_LEN..];
        let mut id = [0u8; 4];
        let n = id.len().min(local.len());
        id[..n].copy_from_slice(&local[..n]);
        id
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
        build_body_with_counts(num_triangles, 1, 0, texture_name)
    }

    fn build_body_with_counts(
        num_triangles: u16,
        mat_count: u8,
        extra_count: u8,
        texture_name: &[u8; 16],
    ) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&D3M_MAGIC.to_le_bytes());
        body.push(mat_count);
        body.push(extra_count);
        body.extend_from_slice(&num_triangles.to_le_bytes());
        body.resize(
            material_table_offset(mat_count as usize, extra_count as usize),
            0,
        );
        for _ in 0..mat_count {
            body.extend_from_slice(texture_name);
        }
        assert_eq!(
            body.len(),
            vertex_offset(mat_count as usize, extra_count as usize)
        );
        body
    }

    fn append_vertex(body: &mut Vec<u8>, pos: [f32; 3], rgba_u8: [u8; 4], uv: [f32; 2]) {
        for c in pos {
            body.extend_from_slice(&c.to_le_bytes());
        }

        for c in [0.0f32, 1.0, 0.0] {
            body.extend_from_slice(&c.to_le_bytes());
        }

        body.push(rgba_u8[2]);
        body.push(rgba_u8[1]);
        body.push(rgba_u8[0]);
        body.push(rgba_u8[3]);
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
        body[0..4].copy_from_slice(&7u32.to_le_bytes());
        assert!(D3m::parse(*b"badm", &body).is_err());
    }

    #[test]
    fn rejects_truncated_vertex_array() {
        let body = build_body(5, &[0u8; 16]);
        assert!(D3m::parse(*b"trun", &body).is_err());
    }

    #[test]
    fn material_table_follows_the_entry_count_padding_rule() {
        assert_eq!(material_table_offset(0, 0), 8);
        assert_eq!(material_table_offset(1, 0), 14);
        assert_eq!(material_table_offset(0, 1), 14);
        assert_eq!(material_table_offset(1, 2), 14);
        assert_eq!(material_table_offset(1, 3), 16);
        assert_eq!(material_table_offset(2, 3), 22);
        assert_eq!(vertex_offset(1, 0), D3M_VERTEX_OFFSET);
        assert_eq!(vertex_offset(0, 1), 14);
        assert_eq!(vertex_offset(2, 3), 22 + 32);
    }

    /// ROM/0/0's `coll` and `hi14` ship matCount 0 / extraCount 1: no material, vertices at 14.
    #[test]
    fn material_less_mesh_reads_vertices_at_the_table_end() {
        let mut body = build_body_with_counts(1, 0, 1, &[0u8; 16]);
        append_vertex(&mut body, [1.0, 2.0, 3.0], [128, 128, 128, 255], [0.5, 0.5]);
        append_vertex(&mut body, [4.0, 5.0, 6.0], [128, 128, 128, 255], [0.5, 0.5]);
        append_vertex(&mut body, [7.0, 8.0, 9.0], [128, 128, 128, 255], [0.5, 0.5]);
        let d = D3m::parse(*b"coll", &body).unwrap();
        assert_eq!(d.texture_name_str(), "");
        assert_eq!(d.vertices[0].pos, [1.0, 2.0, 3.0]);
        assert_eq!(d.vertices[2].pos, [7.0, 8.0, 9.0]);
    }

    #[test]
    fn multi_entry_mesh_takes_its_texture_from_the_first_material() {
        for (mats, extra) in [(1u8, 3u8), (2, 3), (3, 2)] {
            let mut body = build_body_with_counts(1, mats, extra, b"kori\0\0\0\0\0\0\0\0\0\0\0\0");
            append_vertex(&mut body, [1.0, 0.0, 0.0], [128, 0, 0, 255], [0.0, 0.0]);
            append_vertex(&mut body, [0.0, 1.0, 0.0], [128, 0, 0, 255], [0.0, 0.0]);
            append_vertex(&mut body, [0.0, 0.0, 1.0], [128, 0, 0, 255], [0.0, 0.0]);
            let d = D3m::parse(*b"mult", &body).unwrap();
            assert_eq!(d.texture_name_str(), "kori", "mats={mats} extra={extra}");
            assert_eq!(
                d.vertices[1].pos,
                [0.0, 1.0, 0.0],
                "mats={mats} extra={extra}"
            );
        }
    }

    #[test]
    fn accepts_marker_5_and_rejects_the_rewritten_marker_7() {
        let mut body = build_body(0, &[0u8; 16]);
        body[0] = 5;
        assert!(D3m::parse(*b"mk5 ", &body).is_ok());
        body[0] = 7;
        assert!(D3m::parse(*b"mk7 ", &body).is_err());
    }

    #[test]
    fn texture_name_trims_padding() {
        let mut body = build_body(0, b"abc\0\0\0\0\0\0\0\0\0\0\0\0\0");

        assert_eq!(body.len(), D3M_VERTEX_OFFSET);
        let d = D3m::parse(*b"name", &body).unwrap();
        assert_eq!(d.texture_name_str(), "abc");
        body.clear();
    }
}
