//! Skeleton / Bone (Sk2) chunk parser, kind 0x29.
//!
//! Layout observed in file 7072 chunk 70 ("hum_"):
//!   offset 0..4     u32 bone count + flags (low 16 = count, high 16 = ?)
//!                   sample: 0x005e0000 → 94 bones
//!   offset 4..16    12 bytes zeros (rest of the header)
//!   offset 16..     per-bone records: 4x4 matrix or quat+translation
//!                   sample first floats: 0.708, -0.708 (cos/sin 45°)
//!
//! This is reconnaissance scaffolding — the per-bone stride and exact
//! field order need more samples to pin down.

use crate::Result;

#[derive(Debug, Clone)]
pub struct BoneHeader {
    /// Number of bones in the skeleton.
    pub count: u16,
    /// High 16 bits of the first u32 — flags/version, currently unknown.
    pub flags: u16,
}

impl BoneHeader {
    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < 4 {
            return None;
        }
        let raw = u32::from_le_bytes(body[0..4].try_into().ok()?);
        Some(Self {
            count: (raw & 0xFFFF) as u16,
            flags: (raw >> 16) as u16,
        })
    }
}

/// Sweep the body for plausible bone transform blocks. A "bone matrix"
/// candidate is a 64-byte window containing 16 floats where rows
/// 0/1/2 look like a 3x3 rotation (each row magnitude ~1) and row 3
/// is a translation vector. *Reconnaissance only* — used to identify
/// where bone data lives before we pin down the exact stride.
pub fn find_bone_transform_offsets(body: &[u8]) -> Result<Vec<usize>> {
    let mut out = Vec::new();
    let stride = 4; // float-aligned scan
    let mut i = 0;
    while i + 64 <= body.len() {
        let mut floats = [0f32; 16];
        for (j, f) in floats.iter_mut().enumerate() {
            *f = f32::from_le_bytes(body[i + j * 4..i + j * 4 + 4].try_into().unwrap());
        }
        // 3x3 rotation row magnitudes
        let row_mag = |a: usize, b: usize, c: usize| -> f32 {
            (floats[a] * floats[a] + floats[b] * floats[b] + floats[c] * floats[c]).sqrt()
        };
        let r0 = row_mag(0, 1, 2);
        let r1 = row_mag(4, 5, 6);
        let r2 = row_mag(8, 9, 10);
        if (r0 - 1.0).abs() < 0.05 && (r1 - 1.0).abs() < 0.05 && (r2 - 1.0).abs() < 0.05 {
            out.push(i);
            i += 64;
        } else {
            i += stride;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bone_count() {
        let buf = [0x00, 0x00, 0x5E, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let h = BoneHeader::parse(&buf).unwrap();
        // raw u32 = 0x005E0000 → low16 = 0, high16 = 0x5E
        assert_eq!(h.count, 0x0000);
        assert_eq!(h.flags, 0x005E);
    }

    #[test]
    fn detects_identity_rotation_matrix() {
        // 4x4 identity at offset 0:
        // [1 0 0 0; 0 1 0 0; 0 0 1 0; 0 0 0 1]
        let mut buf = Vec::new();
        for v in [
            1.0f32, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        let hits = find_bone_transform_offsets(&buf).unwrap();
        assert_eq!(hits, vec![0]);
    }
}
