//! Animation (Mo2 / "mot_") chunk decoder — scaffolding.
//!
//! `mot_` DATs are animation libraries: each contains many animation
//! clips, each clip is a sequence of "csXX"-kind 0x2B chunks. The csd1
//! / csa1 / csb1 naming convention picks an animation variant.
//!
//! Empirical structure of a kind-0x2B chunk body (csd1 sample,
//! file_id 50000 chunk 3):
//!   offset 0..12   header (bone count? frame count? sub-record offsets?)
//!   offset 12..16  IEEE-754 float (observed 0.6) — probably duration
//!                  or per-frame time
//!   offset 16..    bone-keyed records:
//!                    - bone-id (u8 or u16) + offset table to keyframe groups
//!                    - per-keyframe: quaternion (vec4 unit) + scale vec3
//!                  observed groups end with a quaternion-like value
//!                  `(a, b, c, ~1.0)` confirming the rotation encoding.

use crate::Result;

/// Decoded sub-record inside an animation chunk.
#[derive(Debug, Clone)]
pub struct AnimSubRecord {
    /// Raw bone/joint id from the per-record header.
    pub bone_id: u16,
    /// Quaternion keyframe (xyzw). Empirically these *do* sum to 1 in
    /// the sampled csd1 chunk.
    pub rotation: [f32; 4],
    /// Scale vector3. Often (1, 1, 1).
    pub scale: [f32; 3],
}

/// Quaternion encoding used by FFXI's `mot_` animation chunks.
/// Empirically, each quaternion is 4 little-endian `i16`s scaled to
/// `[-1.0, 1.0]` by `/ 32768.0`. Confirmed by finding 931 unit-magnitude
/// windows in csd1 chunk of file_id 50000, including obvious identity
/// `(0,0,0,-1)` and pure 180° rotations `(0,0,-1,0)` etc.
pub fn decode_i16_quaternion(bytes: &[u8]) -> Option<[f32; 4]> {
    if bytes.len() < 8 {
        return None;
    }
    let a = i16::from_le_bytes([bytes[0], bytes[1]]);
    let b = i16::from_le_bytes([bytes[2], bytes[3]]);
    let c = i16::from_le_bytes([bytes[4], bytes[5]]);
    let d = i16::from_le_bytes([bytes[6], bytes[7]]);
    Some([
        a as f32 / 32768.0,
        b as f32 / 32768.0,
        c as f32 / 32768.0,
        d as f32 / 32768.0,
    ])
}

/// Empirically observed keyframe stride in `csd1`-style animation
/// chunks. 8 bytes quaternion + 6 bytes (likely 3 i16 translation in
/// the same /32768 scaling). Confirmed by gap-histogram analysis of
/// 158 strong-unit-magnitude quaternion windows: gap 14 was the most
/// common real spacing (24 occurrences), distinct from the 2-byte
/// scan noise.
pub const KEYFRAME_STRIDE: usize = 14;

/// One animation keyframe — quat + translation, both i16-quantized.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimKeyframe {
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
}

/// Decode a single keyframe at the start of `bytes`. Returns None if
/// the bytes don't look like a unit quaternion (gating tolerance ±0.05).
pub fn decode_keyframe(bytes: &[u8]) -> Option<AnimKeyframe> {
    if bytes.len() < KEYFRAME_STRIDE {
        return None;
    }
    let q = decode_i16_quaternion(&bytes[0..8])?;
    let m = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
    if (m - 1.0).abs() > 0.05 {
        return None;
    }
    let tx = i16::from_le_bytes([bytes[8], bytes[9]]) as f32 / 32768.0;
    let ty = i16::from_le_bytes([bytes[10], bytes[11]]) as f32 / 32768.0;
    let tz = i16::from_le_bytes([bytes[12], bytes[13]]) as f32 / 32768.0;
    Some(AnimKeyframe {
        rotation: q,
        translation: [tx, ty, tz],
    })
}

/// Sliding-window scan for plausible unit-quaternion records (4 i16 LE
/// scaled to [-1,1]). Returns offsets + decoded quaternions whose
/// squared magnitude is within ±0.05 of 1.0. *Not* a structural parser —
/// a reconnaissance tool to map where rotation data lives in a chunk
/// before we crack the full per-record stride.
pub fn find_quaternion_records(body: &[u8]) -> Result<Vec<(usize, [f32; 4])>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 8 <= body.len() {
        if let Some(q) = decode_i16_quaternion(&body[i..i + 8]) {
            let m = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
            if (m - 1.0).abs() < 0.05 {
                out.push((i, q));
                i += 8;
                continue;
            }
        }
        i += 2; // 2-byte stride to catch unaligned matches
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_body_no_records() {
        let recs = find_quaternion_records(&[]).unwrap();
        assert!(recs.is_empty());
    }

    #[test]
    fn detects_an_i16_identity_quaternion() {
        // Encode identity quat (0, 0, 0, -1) as 4 i16 LE.
        // -1.0 quantized: -32768 / 32768 = -1.0 exactly.
        let mut buf = Vec::new();
        buf.extend(std::iter::repeat_n(0u8, 8)); // padding before the quat
        buf.extend_from_slice(&0i16.to_le_bytes());
        buf.extend_from_slice(&0i16.to_le_bytes());
        buf.extend_from_slice(&0i16.to_le_bytes());
        buf.extend_from_slice(&i16::MIN.to_le_bytes()); // -32768 → -1.0

        let recs = find_quaternion_records(&buf).unwrap();
        assert!(!recs.is_empty(), "should detect the identity quat");
        let (off, q) = recs[0];
        assert_eq!(off, 8);
        assert_eq!(q, [0.0, 0.0, 0.0, -1.0]);
    }

    #[test]
    fn decode_i16_quaternion_basic() {
        // (0.5, -0.5, 0.0, 0.707...) approximated.
        let half = (0.5_f32 * 32768.0) as i16; // 16384
        let neg_half = -half;
        let mut buf = Vec::new();
        buf.extend_from_slice(&half.to_le_bytes());
        buf.extend_from_slice(&neg_half.to_le_bytes());
        buf.extend_from_slice(&0i16.to_le_bytes());
        buf.extend_from_slice(&((0.707_f32 * 32768.0) as i16).to_le_bytes());
        let q = decode_i16_quaternion(&buf).unwrap();
        assert!((q[0] - 0.5).abs() < 0.01);
        assert!((q[1] + 0.5).abs() < 0.01);
        assert_eq!(q[2], 0.0);
        assert!((q[3] - 0.707).abs() < 0.01);
    }
}
