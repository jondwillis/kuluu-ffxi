use crate::{DatError, Result};

pub const D3M_MAGIC: u32 = 6;

pub const D3M_VERTEX_STRIDE: usize = 36;

pub const D3M_VERTEX_OFFSET: usize = 0x1E;

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
        if body.len() < D3M_VERTEX_OFFSET {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: D3M_VERTEX_OFFSET,
                available: body.len(),
            });
        }
        let magic = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
        if magic != D3M_MAGIC {
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
        body.extend_from_slice(&D3M_MAGIC.to_le_bytes());
        body.push(1);
        body.push(0);
        body.extend_from_slice(&num_triangles.to_le_bytes());
        body.extend_from_slice(&[0u8; 6]);
        body.extend_from_slice(texture_name);
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
    fn texture_name_trims_padding() {
        let mut body = build_body(0, b"abc\0\0\0\0\0\0\0\0\0\0\0\0\0");

        assert_eq!(body.len(), D3M_VERTEX_OFFSET);
        let d = D3m::parse(*b"name", &body).unwrap();
        assert_eq!(d.texture_name_str(), "abc");
        body.clear();
    }
}
