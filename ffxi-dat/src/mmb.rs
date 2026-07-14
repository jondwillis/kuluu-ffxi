use crate::{DatError, Result};

pub(crate) mod keys;

#[derive(Debug, thiserror::Error)]
pub enum MmbError {
    #[error("MMB buffer too small: need at least 8 bytes for header, got {0}")]
    TooSmall(usize),
}

impl From<MmbError> for DatError {
    fn from(e: MmbError) -> Self {
        DatError::Mmb(format!("{e}"))
    }
}

pub fn decrypt_in_place(data: &mut [u8]) -> Result<()> {
    if data.len() < 8 {
        return Err(MmbError::TooSmall(data.len()).into());
    }

    if data[3] >= 5 {
        let key_seed = keys::KEY_TABLE[(data[5] ^ 0xf0) as usize] as i32;
        let mut key: i32 = key_seed;
        let mut key_count: i32 = 0;

        for byte in data.iter_mut().skip(8) {
            let key_low = key & 0xFF;
            let x = (key_low << 8) | key_low;
            key_count = key_count.wrapping_add(1);
            key = key.wrapping_add(key_count);

            let shift = (key & 7) as u32;

            let mask = ((x >> shift) & 0xFF) as u8;
            *byte ^= mask;

            key_count = key_count.wrapping_add(1);
            key = key.wrapping_add(key_count);
        }
    }

    if data[6] == 0xFF && data[7] == 0xFF {
        let len = data.len();
        let mut key1: i32 = (data[5] ^ 0xf0) as i32;
        let mut key2: i32 = keys::KEY_TABLE_2[key1 as usize] as i32;

        let len2 = (((len - 8) & !0xf) >> 1) as i32;

        let mut offset1: usize = 8;
        let mut offset2: usize = (8 + len2) as usize;

        let mut i: i32 = 0;
        while i < len2 {
            if (key2 & 1) == 1 {
                let mut tmp = [0u8; 8];
                tmp.copy_from_slice(&data[offset1..offset1 + 8]);
                let (left, right) = data.split_at_mut(offset2);
                left[offset1..offset1 + 8].copy_from_slice(&right[..8]);
                right[..8].copy_from_slice(&tmp);
            }

            key1 = key1.wrapping_add(9);
            key2 = key2.wrapping_add(key1);
            offset1 += 8;
            offset2 += 8;
            i += 8;
        }
    }

    Ok(())
}

pub fn decrypt(data: &[u8]) -> Result<Vec<u8>> {
    let mut buf = data.to_vec();
    decrypt_in_place(&mut buf)?;
    Ok(buf)
}

#[derive(Debug, Clone)]
pub struct MmbHeader<'a> {
    pub version: u8,
    pub key_index: u8,
    pub feature_flags: u16,
    pub asset_name: &'a [u8],

    pub header_window: &'a [u8],

    pub payload: &'a [u8],
}

impl<'a> MmbHeader<'a> {
    pub fn parse(decrypted: &'a [u8]) -> Result<Self> {
        if decrypted.len() < 32 {
            return Err(MmbError::TooSmall(decrypted.len()).into());
        }
        Ok(Self {
            version: decrypted[3],
            key_index: decrypted[5],
            feature_flags: u16::from_le_bytes([decrypted[6], decrypted[7]]),
            asset_name: &decrypted[8..32],
            header_window: &decrypted[0..32],
            payload: &decrypted[32..],
        })
    }

    pub fn asset_name_str(&self) -> String {
        let raw: String = self
            .asset_name
            .iter()
            .map(|&b| if b == 0 { '.' } else { b as char })
            .collect();
        raw.trim_end().to_string()
    }

    pub fn zone_mesh_name(&self) -> String {
        self.header_window[16..32]
            .iter()
            .map(|&b| {
                if (0x20..0x7f).contains(&b) {
                    b as char
                } else {
                    '\0'
                }
            })
            .take_while(|&c| c != '\0')
            .collect::<String>()
            .trim()
            .to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MmbVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub rgba: [u8; 4],
    pub uv: [f32; 2],
}

#[derive(Debug, Clone, Copy)]
pub struct MmbSubRecord<'a> {
    pub offset: usize,

    pub tag: &'a [u8],

    pub variant_name: &'a [u8],

    pub count: u32,

    pub blending: u16,

    pub body: &'a [u8],
}

impl<'a> MmbSubRecord<'a> {
    pub fn find_all(payload: &'a [u8]) -> Vec<MmbSubRecord<'a>> {
        let mut starts: Vec<usize> = Vec::new();
        let mut i = 0;
        while i + 24 <= payload.len() {
            let tag_word = &payload[i..i + 8];
            let variant = &payload[i + 8..i + 16];

            let vertexsize = u16::from_le_bytes([payload[i + 16], payload[i + 17]]) as u32;
            if is_ascii_tag(tag_word)
                && is_ascii_variant(variant)
                && vertexsize > 0
                && vertexsize <= 0xFFFF
            {
                starts.push(i);
                i += 20;
                continue;
            }

            i += 4;
        }

        starts
            .iter()
            .enumerate()
            .map(|(idx, &start)| {
                let end = starts.get(idx + 1).copied().unwrap_or(payload.len());
                let vertexsize = u16::from_le_bytes([payload[start + 16], payload[start + 17]]);
                let blending = u16::from_le_bytes([payload[start + 18], payload[start + 19]]);
                MmbSubRecord {
                    offset: start,
                    tag: &payload[start..start + 8],
                    variant_name: &payload[start + 8..start + 16],
                    count: vertexsize as u32,
                    blending,
                    body: &payload[start + 20..end],
                }
            })
            .collect()
    }

    /// Decoded view of the `blending` word; see [`MmbRenderState`].
    pub fn render_state(&self) -> MmbRenderState {
        MmbRenderState::from_blending(self.blending)
    }

    pub fn parse_triangle_strip(&self) -> Vec<u16> {
        let vert_bytes = self.count as usize * 36;
        if vert_bytes >= self.body.len() {
            return Vec::new();
        }
        let leftover = &self.body[vert_bytes..];
        leftover
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect()
    }

    pub fn parse_triangle_list(&self) -> Vec<[u16; 3]> {
        const HEADER_BYTES: usize = 4;
        let vert_bytes = self.count as usize * 36;
        if vert_bytes + HEADER_BYTES > self.body.len() {
            return Vec::new();
        }
        let count_off = vert_bytes;
        let num = u16::from_le_bytes([self.body[count_off], self.body[count_off + 1]]) as usize;
        if num < 3 {
            return Vec::new();
        }
        let strip_off = count_off + HEADER_BYTES;
        let bytes_needed = num * 2;
        if strip_off + bytes_needed > self.body.len() {
            return Vec::new();
        }
        let read = |i: usize| -> u16 {
            let p = strip_off + i * 2;
            u16::from_le_bytes([self.body[p], self.body[p + 1]])
        };
        let mut out = Vec::with_capacity(num.saturating_sub(2));
        for i in 0..num - 2 {
            let i1 = read(i);
            let i2 = read(i + 1);
            let i3 = read(i + 2);
            if i1 == i2 || i2 == i3 {
                continue;
            }
            let tri = if i % 2 == 1 {
                [i2, i1, i3]
            } else {
                [i1, i2, i3]
            };
            out.push(tri);
        }
        out
    }

    pub fn parse_vertices(&self) -> Option<Vec<MmbVertex>> {
        const STRIDE: usize = 36;
        let needed = self.count as usize * STRIDE;
        if needed > self.body.len() {
            return None;
        }
        let mut out = Vec::with_capacity(self.count as usize);
        for i in 0..self.count as usize {
            let off = i * STRIDE;
            let pos = [
                f32::from_le_bytes(self.body[off..off + 4].try_into().ok()?),
                f32::from_le_bytes(self.body[off + 4..off + 8].try_into().ok()?),
                f32::from_le_bytes(self.body[off + 8..off + 12].try_into().ok()?),
            ];
            let normal = [
                f32::from_le_bytes(self.body[off + 12..off + 16].try_into().ok()?),
                f32::from_le_bytes(self.body[off + 16..off + 20].try_into().ok()?),
                f32::from_le_bytes(self.body[off + 20..off + 24].try_into().ok()?),
            ];
            let rgba = [
                self.body[off + 24],
                self.body[off + 25],
                self.body[off + 26],
                self.body[off + 27],
            ];
            let uv = [
                f32::from_le_bytes(self.body[off + 28..off + 32].try_into().ok()?),
                f32::from_le_bytes(self.body[off + 32..off + 36].try_into().ok()?),
            ];
            out.push(MmbVertex {
                pos,
                normal,
                rgba,
                uv,
            });
        }
        Some(out)
    }

    pub fn variant_name_str(&self) -> String {
        self.variant_name
            .iter()
            .map(|&b| if b == 0 { '.' } else { b as char })
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    pub fn texture_name_str(&self) -> String {
        let s: String = self
            .variant_name
            .iter()
            .map(|&b| {
                if (0x20..0x7f).contains(&b) {
                    b as char
                } else {
                    '\0'
                }
            })
            .take_while(|&c| c != '\0')
            .collect();
        s.trim_end().to_string()
    }

    pub fn tag<'b>(&'b self, payload: &'a [u8]) -> &'a [u8]
    where
        'a: 'b,
    {
        &payload[self.offset..self.offset + 8]
    }
}

fn is_ascii_tag(b: &[u8]) -> bool {
    if b.len() != 8 || b[0] == b' ' || b[0] == 0 {
        return false;
    }
    b.iter()
        .all(|&c| c == 0 || c.is_ascii_alphanumeric() || c == b'_' || c == b' ')
}

fn is_ascii_variant(b: &[u8]) -> bool {
    if b.len() != 8 {
        return false;
    }
    b.iter()
        .all(|&c| c == 0 || c.is_ascii_alphanumeric() || c == b'_' || c == b' ')
}

/// Per-mesh render state decoded from the 16-bit flags word at sub-record
/// offset 18 (`MmbSubRecord::blending` / `MmbModel::blending`).
///
/// Bit layout follows xim's zone-mesh parser
/// (research/xim/src/jsMain/kotlin/xim/resource/ZoneMeshSection.kt:79-81):
/// - `0x8000`: alpha blending enabled (`blendEnabled`)
/// - `0x2000`: back-face culling DISABLED (culling defaults to on/CCW;
///   the set bit turns it off)
///
/// Derived state (not stored in the DAT, reproduced from xim):
/// - depth bias: `blendEnabled` -> `ZBiasLevel.High`, else `Normal`
///   (ZoneMeshSection.kt:120-123), applied by the GL layer as
///   `polygonOffset(zBias * -1, 1)` (GLDrawer.kt:219, :363).
/// - depth write: disabled for blended meshes (GLDrawer.kt:198-201, :332-342).
/// - discard threshold: 0.375 when the zone-mesh *name* starts with `_`
///   (ZoneMeshSection.kt:118) — i.e. the name-prefix heuristic in
///   `dat_mmb.rs::submesh_alpha_mode` is retail-faithful, not a guess.
///
/// NOTE: `vertexBlendEnabled` is NOT in this word. It is the section-level
/// config bit `0x2` (ZoneMeshSection.kt:35), which for SMMB corresponds to
/// `d3 == 2` / vertex stride 48 (`MmbModel::vertex_blend_enabled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MmbRenderState {
    /// Alpha blending on (bit `0x8000`). Implies high depth bias and no
    /// depth write.
    pub blend_enabled: bool,
    /// Back-face culling on (bit `0x2000` CLEAR).
    pub back_face_culling: bool,
}

impl MmbRenderState {
    pub fn from_blending(blending: u16) -> Self {
        Self {
            blend_enabled: blending & 0x8000 != 0,
            back_face_culling: blending & 0x2000 == 0,
        }
    }

    /// ZoneMeshSection.kt:120-123 — blended zone meshes render at
    /// `ZBiasLevel.High` (1), opaque at `Normal` (0).
    pub fn z_bias_level(&self) -> u8 {
        if self.blend_enabled {
            1
        } else {
            0
        }
    }

    /// GLDrawer.kt:198-201 — blended meshes do not write depth.
    pub fn depth_write(&self) -> bool {
        !self.blend_enabled
    }
}

#[derive(Debug, Clone)]
pub struct MmbModel {
    pub texture_name: String,
    pub blending: u16,
    /// Decoded view of `blending`; see [`MmbRenderState`].
    pub render_state: MmbRenderState,
    /// Section-level vertex-blend flag (`d3 == 2`, stride-48 layout).
    /// Mirrors xim's `vertexBlendEnabled` (ZoneMeshSection.kt:35).
    pub vertex_blend_enabled: bool,
    pub vertices: Vec<MmbVertex>,

    pub indices: Vec<u16>,
}

pub fn parse_models(decrypted: &[u8]) -> Vec<MmbModel> {
    const SMMB_HEAD_SIZE: usize = 16;
    const SMMB_HEADER_SIZE: usize = 48;

    if decrypted.len() < SMMB_HEAD_SIZE + SMMB_HEADER_SIZE {
        return Vec::new();
    }

    let is_v1 = &decrypted[0..3] == b"MMB";

    let d3 = if is_v1 { 0 } else { decrypted[4] };
    let vertex_stride: usize = if d3 == 2 { 48 } else { 36 };

    let header_off = SMMB_HEAD_SIZE;
    let pieces = u32::from_le_bytes([
        decrypted[header_off + 16],
        decrypted[header_off + 17],
        decrypted[header_off + 18],
        decrypted[header_off + 19],
    ]) as usize;
    let offset_block_header = u32::from_le_bytes([
        decrypted[header_off + 44],
        decrypted[header_off + 45],
        decrypted[header_off + 46],
        decrypted[header_off + 47],
    ]) as usize;

    let mut offset_list: Vec<usize> = Vec::new();
    let mut cursor = header_off + SMMB_HEADER_SIZE;
    if offset_block_header == 0 {
        if pieces != 0 {
            for _ in 0..8 {
                if cursor + 4 > decrypted.len() {
                    break;
                }
                let po = u32::from_le_bytes([
                    decrypted[cursor],
                    decrypted[cursor + 1],
                    decrypted[cursor + 2],
                    decrypted[cursor + 3],
                ]) as usize;
                if po != 0 {
                    offset_list.push(po);
                }
                cursor += 4;
            }
        } else {
            offset_list.push(cursor);
        }
    } else {
        offset_list.push(offset_block_header);
        if offset_block_header > cursor {
            let pad = offset_block_header - cursor;
            for _ in (0..pad).step_by(4) {
                if cursor + 4 > decrypted.len() {
                    break;
                }
                let po = u32::from_le_bytes([
                    decrypted[cursor],
                    decrypted[cursor + 1],
                    decrypted[cursor + 2],
                    decrypted[cursor + 3],
                ]) as usize;
                if po != 0 {
                    offset_list.push(po);
                }
                cursor += 4;
            }
        }
    }

    let mut models: Vec<MmbModel> = Vec::new();
    for piece_idx in 0..pieces {
        let piece_off = match offset_list.get(piece_idx).copied() {
            Some(o) => o,
            None => break,
        };
        if piece_off + 32 > decrypted.len() {
            break;
        }
        let num_model = u32::from_le_bytes([
            decrypted[piece_off],
            decrypted[piece_off + 1],
            decrypted[piece_off + 2],
            decrypted[piece_off + 3],
        ]) as usize;

        if num_model > 100 {
            break;
        }
        let mut off = piece_off + 32;

        for _ in 0..num_model {
            if off + 20 > decrypted.len() {
                break;
            }

            let texture_name = {
                let name_bytes = &decrypted[off + 8..off + 16];
                let s: String = name_bytes
                    .iter()
                    .map(|&b| {
                        if (0x20..0x7f).contains(&b) {
                            b as char
                        } else {
                            '\0'
                        }
                    })
                    .take_while(|&c| c != '\0')
                    .collect();
                s.trim_end().to_string()
            };
            let vertexsize =
                u16::from_le_bytes([decrypted[off + 16], decrypted[off + 17]]) as usize;
            let blending = u16::from_le_bytes([decrypted[off + 18], decrypted[off + 19]]);
            off += 20;

            let vert_bytes = vertexsize * vertex_stride;
            if off + vert_bytes > decrypted.len() {
                break;
            }
            let mut vertices = Vec::with_capacity(vertexsize);
            for vi in 0..vertexsize {
                let vo = off + vi * vertex_stride;
                let pos = [
                    f32::from_le_bytes([
                        decrypted[vo],
                        decrypted[vo + 1],
                        decrypted[vo + 2],
                        decrypted[vo + 3],
                    ]),
                    f32::from_le_bytes([
                        decrypted[vo + 4],
                        decrypted[vo + 5],
                        decrypted[vo + 6],
                        decrypted[vo + 7],
                    ]),
                    f32::from_le_bytes([
                        decrypted[vo + 8],
                        decrypted[vo + 9],
                        decrypted[vo + 10],
                        decrypted[vo + 11],
                    ]),
                ];

                let normal_base = if d3 == 2 { vo + 24 } else { vo + 12 };
                let normal = [
                    f32::from_le_bytes([
                        decrypted[normal_base],
                        decrypted[normal_base + 1],
                        decrypted[normal_base + 2],
                        decrypted[normal_base + 3],
                    ]),
                    f32::from_le_bytes([
                        decrypted[normal_base + 4],
                        decrypted[normal_base + 5],
                        decrypted[normal_base + 6],
                        decrypted[normal_base + 7],
                    ]),
                    f32::from_le_bytes([
                        decrypted[normal_base + 8],
                        decrypted[normal_base + 9],
                        decrypted[normal_base + 10],
                        decrypted[normal_base + 11],
                    ]),
                ];
                let color_base = normal_base + 12;
                let rgba = [
                    decrypted[color_base],
                    decrypted[color_base + 1],
                    decrypted[color_base + 2],
                    decrypted[color_base + 3],
                ];
                let uv_base = color_base + 4;
                let uv = [
                    f32::from_le_bytes([
                        decrypted[uv_base],
                        decrypted[uv_base + 1],
                        decrypted[uv_base + 2],
                        decrypted[uv_base + 3],
                    ]),
                    f32::from_le_bytes([
                        decrypted[uv_base + 4],
                        decrypted[uv_base + 5],
                        decrypted[uv_base + 6],
                        decrypted[uv_base + 7],
                    ]),
                ];
                vertices.push(MmbVertex {
                    pos,
                    normal,
                    rgba,
                    uv,
                });
            }
            off += vert_bytes;

            if off + 4 > decrypted.len() {
                break;
            }
            let num_indices = u16::from_le_bytes([decrypted[off], decrypted[off + 1]]) as usize;
            off += 4;

            let mut indices: Vec<u16> = Vec::new();
            let is_list = is_v1 || d3 == 2;
            if off + num_indices * 2 > decrypted.len() {
                break;
            }
            if is_list {
                for i in 0..num_indices {
                    let p = off + i * 2;
                    indices.push(u16::from_le_bytes([decrypted[p], decrypted[p + 1]]));
                }
            } else if num_indices >= 3 {
                for i in 0..(num_indices - 2) {
                    let p = off + i * 2;
                    let i1 = u16::from_le_bytes([decrypted[p], decrypted[p + 1]]);
                    let i2 = u16::from_le_bytes([decrypted[p + 2], decrypted[p + 3]]);
                    let i3 = u16::from_le_bytes([decrypted[p + 4], decrypted[p + 5]]);
                    if i1 == i2 || i2 == i3 {
                        continue;
                    }
                    if i % 2 == 1 {
                        indices.extend_from_slice(&[i2, i1, i3]);
                    } else {
                        indices.extend_from_slice(&[i1, i2, i3]);
                    }
                }
            }
            off += num_indices * 2;
            if !num_indices.is_multiple_of(2) {
                off += 2;
            }

            if !indices.is_empty() && !vertices.is_empty() {
                models.push(MmbModel {
                    texture_name,
                    blending,
                    render_state: MmbRenderState::from_blending(blending),
                    vertex_blend_enabled: d3 == 2,
                    vertices,
                    indices,
                });
            }
        }
    }

    models
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_below_5_is_noop_for_pass1() {
        let mut buf = vec![b'M', b'M', b'B', 4, 0x12, 0x34, 0x00, 0x00, 0xAA, 0xBB];
        decrypt_in_place(&mut buf).unwrap();
        assert_eq!(&buf[8..], &[0xAA, 0xBB]);
    }

    #[test]
    fn too_small_errors() {
        let mut buf = [0u8; 4];
        let err = decrypt_in_place(&mut buf).unwrap_err();
        assert!(matches!(err, DatError::Mmb(_)));
    }

    #[test]
    fn zone_mesh_name_is_bytes_16_to_32_not_asset_name() {
        let mut hdr = vec![0u8; 40];
        hdr[3] = 4;
        hdr[8..16].copy_from_slice(b"bastok  ");
        hdr[16..32].copy_from_slice(b"8\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
        let h = MmbHeader::parse(&hdr).unwrap();

        assert_eq!(h.zone_mesh_name(), "8");

        assert_ne!(h.asset_name_str(), "8");
        assert!(h.asset_name_str().starts_with("bastok"));
    }

    #[test]
    fn round_trip_is_identity_when_version_below_5() {
        let original = vec![
            b'M', b'M', b'B', 4, 0x12, 0x34, 0x00, 0x00, 1, 2, 3, 4, 5, 6, 7, 8,
        ];
        let once = decrypt(&original).unwrap();
        let twice = decrypt(&once).unwrap();
        assert_eq!(once, twice);
        assert_eq!(once, original);
    }

    #[test]
    fn pass1_xor_is_involutive() {
        let mut bytes = vec![b'M', b'M', b'B', 5, 0xAA, 0x42, 0x00, 0x00];
        bytes.extend((0..64).map(|i| i as u8));
        let original = bytes.clone();

        decrypt_in_place(&mut bytes).unwrap();
        assert_ne!(bytes[8..], original[8..], "pass 1 should change bytes");
        decrypt_in_place(&mut bytes).unwrap();
        assert_eq!(bytes, original, "applying twice should be identity");
    }

    const KABU_DECRYPTED_HEAD: [u8; 64] = [
        0xe4, 0x72, 0x00, 0x05, 0x01, 0xb7, 0x47, 0x9f, 0x74, 0x65, 0x6e, 0x73, 0x61, 0x6b, 0x61,
        0x00, 0x6b, 0x61, 0x62, 0x75, 0x73, 0x65, 0x5f, 0x6d, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20,
        0x20, 0x20, 0x01, 0x00, 0x00, 0x00, 0xe6, 0xee, 0x3a, 0xc2, 0x34, 0x22, 0xca, 0x41, 0xb3,
        0xaa, 0x76, 0xc2, 0x99, 0xaa, 0xf6, 0xc1, 0xcd, 0xcc, 0x50, 0xc2, 0x00, 0x00, 0xd4, 0x41,
        0x40, 0x00, 0x00, 0x00,
    ];

    fn synth_payload() -> Vec<u8> {
        let mut buf = Vec::new();

        buf.extend_from_slice(b"model   ");
        buf.extend_from_slice(b"con_wi1 ");
        buf.extend_from_slice(&348u32.to_le_bytes());
        buf.extend(std::iter::repeat_n(0xFFu8, 28));
        buf.extend(std::iter::repeat_n(0xFEu8, 192));

        buf.extend_from_slice(b"clod    ");
        buf.extend_from_slice(b"clod_a01");
        buf.extend_from_slice(&71u32.to_le_bytes());
        buf.extend(std::iter::repeat_n(0xFFu8, 28));
        buf.extend(std::iter::repeat_n(0xFDu8, 48));
        buf
    }

    #[test]
    fn sub_record_walker_finds_mixed_tag_kinds() {
        let payload = synth_payload();
        let records = MmbSubRecord::find_all(&payload);
        assert_eq!(
            records.len(),
            2,
            "should find both 'model   ' and 'clod    ' headers"
        );
        assert_eq!(records[0].variant_name_str(), "con_wi1");
        assert_eq!(records[0].count, 348);
        assert_eq!(&records[0].tag(&payload), b"model   ");
        assert_eq!(records[1].variant_name_str(), "clod_a01");
        assert_eq!(records[1].count, 71);
        assert_eq!(&records[1].tag(&payload), b"clod    ");
    }

    #[test]
    fn texture_name_str_returns_name_half_only() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"model   ");
        buf.extend_from_slice(b"kabe_3\0\0");
        buf.extend_from_slice(&8u16.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend(std::iter::repeat_n(0xFFu8, 64));
        let recs = MmbSubRecord::find_all(&buf);
        assert_eq!(recs.len(), 1, "scanner should accept the model record");
        assert_eq!(
            recs[0].texture_name_str(),
            "kabe_3",
            "NUL-padded short name should compare equal to IMG-side bare name"
        );

        let mut buf2 = Vec::new();
        buf2.extend_from_slice(b"model   ");
        buf2.extend_from_slice(b"jimeni_0");
        buf2.extend_from_slice(&8u16.to_le_bytes());
        buf2.extend_from_slice(&0u16.to_le_bytes());
        buf2.extend(std::iter::repeat_n(0xFFu8, 64));
        let recs2 = MmbSubRecord::find_all(&buf2);
        assert_eq!(recs2[0].texture_name_str(), "jimeni_0");
    }

    #[test]
    fn scanner_accepts_records_with_nonzero_blending() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"glass01\0\0\0\0\0\0\0\0\0");
        buf.extend_from_slice(&42u16.to_le_bytes());
        buf.extend_from_slice(&0x8000u16.to_le_bytes());
        buf.extend(std::iter::repeat_n(0xFFu8, 64));
        let recs = MmbSubRecord::find_all(&buf);
        assert_eq!(
            recs.len(),
            1,
            "non-zero blending must not reject the record"
        );
        assert_eq!(recs[0].count, 42);
        assert_eq!(recs[0].blending, 0x8000);
    }

    #[test]
    fn render_state_decodes_blend_and_cull_bits() {
        // ZoneMeshSection.kt:79-81 — 0x8000 = blend, 0x2000 = cull DISABLED.
        let opaque = MmbRenderState::from_blending(0x0000);
        assert!(!opaque.blend_enabled);
        assert!(opaque.back_face_culling);
        assert_eq!(opaque.z_bias_level(), 0);
        assert!(opaque.depth_write());

        let blended = MmbRenderState::from_blending(0x8000);
        assert!(blended.blend_enabled);
        assert!(blended.back_face_culling);
        assert_eq!(blended.z_bias_level(), 1);
        assert!(!blended.depth_write());

        let no_cull = MmbRenderState::from_blending(0x2000);
        assert!(!no_cull.blend_enabled);
        assert!(!no_cull.back_face_culling);

        let both = MmbRenderState::from_blending(0xA000);
        assert!(both.blend_enabled);
        assert!(!both.back_face_culling);
    }

    #[test]
    fn render_state_ignores_unrelated_bits() {
        // Bits other than 0x8000/0x2000 are not render state per xim.
        let s = MmbRenderState::from_blending(0x1FFF);
        assert!(!s.blend_enabled);
        assert!(s.back_face_culling);
    }

    #[test]
    fn parses_real_kabu_header() {
        let h = MmbHeader::parse(&KABU_DECRYPTED_HEAD).unwrap();
        assert_eq!(h.version, 0x05);
        assert_eq!(h.key_index, 0xB7);
        assert_eq!(h.feature_flags, 0x9F47);
        assert_eq!(h.asset_name_str(), "tensaka.kabuse_m");

        let count = u32::from_le_bytes([h.payload[0], h.payload[1], h.payload[2], h.payload[3]]);
        assert_eq!(count, 1);

        let f0 = f32::from_le_bytes([h.payload[4], h.payload[5], h.payload[6], h.payload[7]]);
        let f1 = f32::from_le_bytes([h.payload[8], h.payload[9], h.payload[10], h.payload[11]]);
        let f2 = f32::from_le_bytes([h.payload[12], h.payload[13], h.payload[14], h.payload[15]]);
        assert!((f0 + 46.73).abs() < 0.1, "f0 was {f0}");
        assert!((f1 - 25.27).abs() < 0.1, "f1 was {f1}");
        assert!((f2 + 61.67).abs() < 0.1, "f2 was {f2}");
    }
}
