use crate::{DatError, Result};

use crate::mmb::keys::KEY_TABLE;

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

    for i in 0..node_count {
        let base = 0x20usize.saturating_add(i.saturating_mul(0x64));
        let end = base.saturating_add(16);
        if end > data.len() {
            break;
        }
        for b in &mut data[base..end] {
            *b ^= 0x55;
        }
    }

    Ok(())
}

pub fn decrypt(data: &[u8]) -> Result<Vec<u8>> {
    let mut buf = data.to_vec();
    decrypt_in_place(&mut buf)?;
    Ok(buf)
}

#[derive(Debug, Clone, Copy)]
pub struct MzbHeader {
    pub decode_length: u32,
    pub node_count: u32,
    pub version: u8,
    pub key_index: u8,
    pub grid_width: u8,
    pub grid_height: u8,

    pub mesh_table_offset: u32,
    pub quadtree_offset: u32,
    pub maplist_offset: u32,
    pub maplist_count: u32,
}

impl MzbHeader {
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MzbVertex {
    pub pos: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MzbNormal {
    pub n: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MzbTriangleInfo {
    pub material: u8,

    pub is_invalid: bool,

    pub is_barrier: bool,
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

    let mut out = Vec::with_capacity(mesh_count);
    let mut pos = mesh_data_offset;
    for _ in 0..mesh_count {
        if pos + 0x10 > body.len() {
            break;
        }
        let mesh = parse_one_mesh(body, pos)?;

        let tri_off =
            u32::from_le_bytes([body[pos + 8], body[pos + 9], body[pos + 10], body[pos + 11]])
                as usize;

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
    let mut tri_info = Vec::with_capacity(tri_count);
    for i in 0..tri_count {
        let o = tris_off + i * 8;
        if o + 8 > body.len() {
            break;
        }

        let v0_raw = u16::from_le_bytes([body[o], body[o + 1]]);
        let v1_raw = u16::from_le_bytes([body[o + 2], body[o + 3]]);
        let v2_raw = u16::from_le_bytes([body[o + 4], body[o + 5]]);
        let n0_raw = u16::from_le_bytes([body[o + 6], body[o + 7]]);
        let v0 = (v0_raw & 0x7FFF) as u32;
        let v1 = (v1_raw & 0x3FFF) as u32;
        let v2 = (v2_raw & 0x3FFF) as u32;
        let n0 = (n0_raw & 0x7FFF) as u32;
        let m0 = ((v0_raw >> 15) & 1) as u8;
        let m1 = ((v1_raw >> 15) & 1) as u8;
        let m2 = ((v2_raw >> 15) & 1) as u8;
        let m3 = ((n0_raw >> 15) & 1) as u8;
        let material = m0 | (m1 << 1) | (m2 << 2) | (m3 << 3);
        let is_invalid = (v1_raw & 0x4000) != 0;
        let is_barrier = (v2_raw & 0x4000) != 0;
        triangles.push([v0, v1, v2]);
        triangle_normals.push(n0);
        tri_info.push(MzbTriangleInfo {
            material,
            is_invalid,
            is_barrier,
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
}

pub fn parse_mesh_at(body: &[u8], offset: usize) -> Result<MzbMesh> {
    parse_one_mesh(body, offset)
}

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
                if mat_off + 16 * 4 > body.len() || geo_off + 0x10 > body.len() {
                    continue;
                }

                let mut m = [0.0f32; 16];
                for (k, slot) in m.iter_mut().enumerate() {
                    let o = mat_off + k * 4;
                    *slot = f32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
                }

                let det = m[0] * (m[5] * m[10] - m[9] * m[6]) - m[4] * (m[1] * m[10] - m[9] * m[2])
                    + m[8] * (m[1] * m[6] - m[5] * m[2]);

                let flags = u16::from_le_bytes([body[geo_off + 14], body[geo_off + 15]]);

                let water_off = mat_off + 164;
                let water_height = if water_off + 4 <= body.len() {
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

#[derive(Debug, Clone, Copy)]
pub struct MmbPlacement {
    pub id: [u8; 16],
    pub trans: [f32; 3],

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
            [0x8000, 1 | 0x4000, 2, 0],
            [0, 2, 3 | 0x4000 | 0x8000, 0x8000],
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

        assert_eq!(m.vertices[0].pos, [0.0, 0.0, 0.0]);
        assert_eq!(m.vertices[1].pos, [1.0, 0.0, 0.0]);

        assert_eq!(
            m.triangles[0],
            [0, 1, 2],
            "indices: v0 masked with 0x7FFF, v1/v2 with 0x3FFF"
        );
        assert_eq!(m.triangle_normals[0], 0);
        assert_eq!(m.tri_info[0].material, 0b0001, "material from v0 top bit");
        assert!(m.tri_info[0].is_invalid, "is_invalid from v1 bit 14");
        assert!(!m.tri_info[0].is_barrier);

        assert_eq!(m.triangles[1], [0, 2, 3]);
        assert_eq!(
            m.tri_info[1].material, 0b1100,
            "material composed from v2 + n0 top bits"
        );
        assert!(!m.tri_info[1].is_invalid);
        assert!(m.tri_info[1].is_barrier, "is_barrier from v2 bit 14");
    }

    #[test]
    fn pass2_node_xor_runs() {
        let mut buf = vec![0u8; 0x20 + 0x64];
        buf[0..4].copy_from_slice(&((0x10u32 << 24) | 0x20).to_le_bytes());
        buf[4..8].copy_from_slice(&1u32.to_le_bytes());

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
        buf[0x218..0x21C].copy_from_slice(&0x40u32.to_le_bytes());
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
}
