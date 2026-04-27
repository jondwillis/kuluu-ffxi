use crate::Result;

#[derive(Debug, Clone)]
pub struct AnimSubRecord {
    pub bone_id: u16,

    pub rotation: [f32; 4],

    pub scale: [f32; 3],
}

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

pub const KEYFRAME_STRIDE: usize = 14;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimKeyframe {
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
}

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

#[derive(Debug, Clone)]
pub struct Mo2Animation {
    pub name: String,

    pub frames: u32,

    pub speed: f32,

    pub per_bone: std::collections::BTreeMap<u32, Vec<Mo2Frame>>,

    pub per_bone_dense: Vec<Option<Vec<Mo2Frame>>>,
}

impl Mo2Animation {
    #[inline]
    pub fn frames_for_bone(&self, bone: usize) -> Option<&[Mo2Frame]> {
        self.per_bone_dense.get(bone).and_then(|o| o.as_deref())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mo2Frame {
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

const ANIM_HEADER_BYTES: usize = 10;
const ELEMENT_BYTES: usize = 84;

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

        let any_negative = q_idx
            .iter()
            .chain(t_idx.iter())
            .chain(s_idx.iter())
            .any(|&i| i < 0);
        if any_negative {
            continue;
        }
        let mut frames = Vec::with_capacity(header_frames as usize);
        for f in 0..header_frames {
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

        if !frames.is_empty() {
            frames.remove(0);
        }
        per_bone.insert(bone, frames);
    }

    let frames = header_frames.saturating_sub(1);
    let max_bone = per_bone.keys().copied().max().unwrap_or(0) as usize;
    let mut per_bone_dense: Vec<Option<Vec<Mo2Frame>>> = (0..=max_bone).map(|_| None).collect();
    for (&bone, kfs) in &per_bone {
        if (bone as usize) < per_bone_dense.len() {
            per_bone_dense[bone as usize] = Some(kfs.clone());
        }
    }
    Ok(Mo2Animation {
        name: name_buf,
        frames,
        speed,
        per_bone,
        per_bone_dense,
    })
}

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
        i += 2;
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
        let mut buf = Vec::new();
        buf.extend(std::iter::repeat_n(0u8, 8));
        buf.extend_from_slice(&0i16.to_le_bytes());
        buf.extend_from_slice(&0i16.to_le_bytes());
        buf.extend_from_slice(&0i16.to_le_bytes());
        buf.extend_from_slice(&i16::MIN.to_le_bytes());

        let recs = find_quaternion_records(&buf).unwrap();
        assert!(!recs.is_empty(), "should detect the identity quat");
        let (off, q) = recs[0];
        assert_eq!(off, 8);
        assert_eq!(q, [0.0, 0.0, 0.0, -1.0]);
    }

    #[test]
    fn decode_i16_quaternion_basic() {
        let half = (0.5_f32 * 32768.0) as i16;
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
