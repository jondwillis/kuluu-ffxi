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

/// Decoded MO2 (`mot_` animation library) chunk. One MO2 chunk holds
/// one named animation clip (e.g. "idl", "wlk") with per-bone keyframe
/// curves over `frames` frames at `speed` units of time per frame.
///
/// Format (lotus-ffxi `mo2.cppm`, `#pragma pack(2)`):
///
/// ```text
///   Animation header (10 bytes):
///     u16 _pad
///     u16 elements
///     u16 frames
///     f32 speed
///
///   Element[elements]  (84 bytes each, pack-2):
///     u32 bone
///     i32[4]  quat_idx     // 0 = use base; >0 = index into float pool
///     f32[4]  quat_base    // fallback quaternion (xyzw)
///     i32[3]  trans_idx
///     f32[3]  trans_base
///     i32[3]  scale_idx
///     f32[3]  scale_base
///
///   Float pool: the same byte stream re-interpreted as f32[].
///     `data[idx + frame]` reads frame `frame` of an animated component.
/// ```
///
/// Per-frame semantics (lotus): if any quat/trans/scale index is
/// negative, the whole element is skipped and bones default to identity.
/// Otherwise each component reads from `data[idx + frame]` when idx > 0,
/// or from `*_base` when idx == 0. Frame 0 is discarded ("animations
/// don't use frame 0 (FFXI thing?)" per lotus).
#[derive(Debug, Clone)]
pub struct Mo2Animation {
    /// Animation name — first 3 bytes of the chunk's `name` field.
    /// Common names: `"idl"` (idle), `"wlk"` (walk), `"run"`, `"atk"`.
    pub name: String,
    /// Number of usable frames (= header.frames - 1, since frame 0 is
    /// dropped).
    pub frames: u32,
    /// Frames per second (= header.speed inverted? lotus stores raw
    /// `header.speed`; in FFXI conventions this is "duration per frame
    /// in seconds" — but the renderer can treat it as a tuning knob and
    /// document the empirical observation).
    pub speed: f32,
    /// Per-bone keyframe array. Key = skeleton bone id (matches what
    /// `Vos2Mesh::skeleton_bone_for` returns); value = `frames`
    /// keyframes for that bone.
    pub per_bone: std::collections::BTreeMap<u32, Vec<Mo2Frame>>,
}

/// One keyframe for one bone: a local rotation + translation + scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mo2Frame {
    /// Quaternion in xyzw order (matches `glm::quat` layout).
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
    pub scale: [f32; 3],
}

impl Mo2Frame {
    pub const IDENTITY: Self = Self {
        rotation: [0.0, 0.0, 0.0, 1.0],
        translation: [0.0; 3],
        scale: [1.0; 3],
    };
}

/// Animation header constants.
const ANIM_HEADER_BYTES: usize = 10;
const ELEMENT_BYTES: usize = 84;

/// Parse an MO2 chunk body (the data portion after the 16-byte DAT
/// chunk header — i.e. what `walk()` yields). `chunk_name` is the
/// chunk's 4-byte name field; we keep only the first 3 chars for the
/// animation identifier (matches lotus's `string(_name, 3)`).
pub fn parse_mo2(body: &[u8], chunk_name: &[u8; 4]) -> Result<Mo2Animation> {
    if body.len() < ANIM_HEADER_BYTES {
        return Err(crate::DatError::Mmb(format!(
            "MO2 body too small for header: {} < {ANIM_HEADER_BYTES}",
            body.len()
        )));
    }
    let elements_count = u16::from_le_bytes([body[2], body[3]]) as usize;
    let header_frames = u16::from_le_bytes([body[4], body[5]]) as u32;
    let speed = f32::from_le_bytes([body[6], body[7], body[8], body[9]]);

    // Float pool: same bytes as the Elements region, reinterpreted.
    // Indices in elements are word-stride (4-byte) offsets into this
    // pool starting at byte 0 of the post-header region.
    let pool_off = ANIM_HEADER_BYTES;
    let read_f32 = |word_idx: usize| -> Option<f32> {
        let o = pool_off + word_idx.checked_mul(4)?;
        if o + 4 > body.len() {
            return None;
        }
        Some(f32::from_le_bytes([
            body[o],
            body[o + 1],
            body[o + 2],
            body[o + 3],
        ]))
    };

    let mut per_bone: std::collections::BTreeMap<u32, Vec<Mo2Frame>> =
        std::collections::BTreeMap::new();
    let mut name_buf = String::with_capacity(3);
    for &b in &chunk_name[..3.min(chunk_name.len())] {
        if b == 0 {
            break;
        }
        name_buf.push(b as char);
    }

    for e in 0..elements_count {
        let off = pool_off + e * ELEMENT_BYTES;
        if off + ELEMENT_BYTES > body.len() {
            break;
        }
        let read_u32 =
            |o: usize| u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
        let read_i32 =
            |o: usize| i32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
        let read_f =
            |o: usize| f32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);

        let bone = read_u32(off);
        let q_idx = [
            read_i32(off + 4),
            read_i32(off + 8),
            read_i32(off + 12),
            read_i32(off + 16),
        ];
        let q_base = [
            read_f(off + 20),
            read_f(off + 24),
            read_f(off + 28),
            read_f(off + 32),
        ];
        let t_idx = [read_i32(off + 36), read_i32(off + 40), read_i32(off + 44)];
        let t_base = [read_f(off + 48), read_f(off + 52), read_f(off + 56)];
        let s_idx = [read_i32(off + 60), read_i32(off + 64), read_i32(off + 68)];
        let s_base = [read_f(off + 72), read_f(off + 76), read_f(off + 80)];

        // lotus: if ANY component index is negative, this element is
        // entirely identity (bone stays at its bind pose).
        let any_negative = q_idx
            .iter()
            .chain(t_idx.iter())
            .chain(s_idx.iter())
            .any(|&i| i < 0);
        let mut frames = Vec::with_capacity(header_frames as usize);
        for f in 0..header_frames {
            if any_negative {
                frames.push(Mo2Frame::IDENTITY);
                continue;
            }
            let sample = |idx: i32, base: f32| -> f32 {
                if idx > 0 {
                    read_f32(idx as usize + f as usize).unwrap_or(base)
                } else {
                    base
                }
            };
            frames.push(Mo2Frame {
                rotation: [
                    sample(q_idx[0], q_base[0]),
                    sample(q_idx[1], q_base[1]),
                    sample(q_idx[2], q_base[2]),
                    sample(q_idx[3], q_base[3]),
                ],
                translation: [
                    sample(t_idx[0], t_base[0]),
                    sample(t_idx[1], t_base[1]),
                    sample(t_idx[2], t_base[2]),
                ],
                scale: [
                    sample(s_idx[0], s_base[0]),
                    sample(s_idx[1], s_base[1]),
                    sample(s_idx[2], s_base[2]),
                ],
            });
        }
        // Drop frame 0 (lotus convention).
        if !frames.is_empty() {
            frames.remove(0);
        }
        per_bone.insert(bone, frames);
    }

    let frames = header_frames.saturating_sub(1);
    Ok(Mo2Animation {
        name: name_buf,
        frames,
        speed,
        per_bone,
    })
}

/// Sliding-window scan for plausible unit-quaternion records (4 i16 LE
/// scaled to [-1,1]). Returns offsets + decoded quaternions whose
/// squared magnitude is within ±0.05 of 1.0. *Not* a structural parser —
/// kept as a diagnostic / reconnaissance tool now that the real
/// structural parser ([`parse_mo2`]) is available.
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
