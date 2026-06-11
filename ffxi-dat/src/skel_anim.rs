//! Skeleton animation section parser (chunk 0x2B) — port of XIM
//! `SkeletonAnimationParser` / `SkeletonAnimation`.
//!
//! Header (body offset 0):
//!   u16 unk0, u16 numJoints, u16 numFrames, f32 keyFrameDuration
//! Pool of keyframe data begins at body offset 10 (`POOL_START`).
//!
//! Per joint: u32 jointIndex, then rotation(4), translation(3), scale(3)
//! channels. Each channel group is read by [`read_sequences`]: n i32
//! offsets then n f32 const-values (taken `rem 10000`); if any offset is
//! negative the whole joint is dropped; offset 0 = constant channel, else
//! the channel reads `numFrames` f32 from `POOL_START + offset*4`.

use std::collections::HashMap;

use crate::datid::DatId;

const POOL_START: usize = 10;

#[derive(Debug, Clone, Copy)]
pub struct KeyFrameTransform {
    pub rotation: [f32; 4], // x, y, z, w
    pub translation: [f32; 3],
    pub scale: [f32; 3],
}

impl Default for KeyFrameTransform {
    fn default() -> Self {
        KeyFrameTransform {
            rotation: [0.0, 0.0, 0.0, 1.0],
            translation: [0.0, 0.0, 0.0],
            scale: [1.0, 1.0, 1.0],
        }
    }
}

#[derive(Debug, Clone)]
pub struct SkeletonAnimation {
    pub id: DatId,
    pub num_joints: usize,
    pub num_frames: usize,
    pub key_frame_duration: f32,
    pub key_frame_sets: HashMap<u32, Vec<KeyFrameTransform>>,
}

impl SkeletonAnimation {
    /// XIM `getJointTransform`. Returns None if no set exists for the joint.
    pub fn get_joint_transform(&self, joint: u32, frame: f32) -> Option<KeyFrameTransform> {
        let set = self.key_frame_sets.get(&joint)?;

        let scaled = frame * self.key_frame_duration;
        if scaled >= (self.num_frames as f32) - 1.0 {
            return Some(set[self.num_frames - 1]);
        }

        let lower = scaled.floor() as usize;
        let delta = scaled - lower as f32;
        Some(interpolate(&set[lower], &set[lower + 1], delta))
    }

    /// XIM `getLengthInFrames`.
    pub fn length_in_frames(&self) -> f32 {
        let n = (self.num_frames as i64 - 1).max(1) as f32;
        n / self.key_frame_duration
    }
}

/// XIM `Quaternion.nlerp`: component lerp toward the shortest-path neighbor,
/// then normalize.
pub fn nlerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let r = if dot < 0.0 {
        [-b[0], -b[1], -b[2], -b[3]]
    } else {
        b
    };
    let inv = 1.0 - t;
    let mut q = [
        a[0] * inv + r[0] * t,
        a[1] * inv + r[1] * t,
        a[2] * inv + r[2] * t,
        a[3] * inv + r[3] * t,
    ];
    let mag = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    let inv_mag = 1.0 / mag;
    for c in q.iter_mut() {
        *c *= inv_mag;
    }
    q
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let inv = 1.0 - t;
    [
        a[0] * inv + b[0] * t,
        a[1] * inv + b[1] * t,
        a[2] * inv + b[2] * t,
    ]
}

fn interpolate(a: &KeyFrameTransform, b: &KeyFrameTransform, delta: f32) -> KeyFrameTransform {
    KeyFrameTransform {
        rotation: nlerp(a.rotation, b.rotation, delta),
        translation: lerp3(a.translation, b.translation, delta),
        scale: lerp3(a.scale, b.scale, delta),
    }
}

// XIM reads over a Uint8Array, which yields 0 past the end instead of
// throwing; these helpers mirror that so a truncated/malformed chunk parses
// tolerantly (silent garbage, no panic) rather than indexing out of bounds.
fn read_u16(data: &[u8], off: usize) -> u16 {
    let b = |i: usize| data.get(off + i).copied().unwrap_or(0);
    u16::from_le_bytes([b(0), b(1)])
}

fn read_i32(data: &[u8], off: usize) -> i32 {
    let b = |i: usize| data.get(off + i).copied().unwrap_or(0);
    i32::from_le_bytes([b(0), b(1), b(2), b(3)])
}

fn read_u32(data: &[u8], off: usize) -> u32 {
    let b = |i: usize| data.get(off + i).copied().unwrap_or(0);
    u32::from_le_bytes([b(0), b(1), b(2), b(3)])
}

fn read_f32(data: &[u8], off: usize) -> f32 {
    f32::from_bits(read_u32(data, off))
}

/// A resolved channel: either a constant value or one value per frame.
enum Sequence {
    Const(f32),
    Frames(Vec<f32>),
}

impl Sequence {
    fn value(&self, frame: usize) -> f32 {
        match self {
            Sequence::Const(v) => *v,
            Sequence::Frames(v) => v[frame],
        }
    }
}

/// Read `amount` channels at `pos`, advancing it. Returns None if any offset
/// is negative (XIM drops the whole joint).
fn read_sequences(
    data: &[u8],
    pos: &mut usize,
    amount: usize,
    num_frames: usize,
) -> Option<Vec<Sequence>> {
    let mut offsets = Vec::with_capacity(amount);
    for _ in 0..amount {
        offsets.push(read_i32(data, *pos));
        *pos += 4;
    }
    let mut consts = Vec::with_capacity(amount);
    for _ in 0..amount {
        consts.push(read_f32(data, *pos) % 10_000.0);
        *pos += 4;
    }

    if offsets.iter().any(|&o| o < 0) {
        return None;
    }

    let mut seqs = Vec::with_capacity(amount);
    for i in 0..amount {
        if offsets[i] == 0 {
            seqs.push(Sequence::Const(consts[i]));
        } else {
            let base = POOL_START + (offsets[i] as usize) * 4;
            let mut frames = Vec::with_capacity(num_frames);
            for f in 0..num_frames {
                frames.push(read_f32(data, base + f * 4));
            }
            seqs.push(Sequence::Frames(frames));
        }
    }
    Some(seqs)
}

/// Parse a 0x2B skeleton animation chunk body.
///
/// Bounded like XIM's ByteReader-over-Uint8Array: a body shorter than the
/// 10-byte header yields an empty animation, and every subsequent read
/// tolerates an over-run (yields 0) rather than panicking, so a truncated or
/// malformed chunk stays "silent garbage" instead of an index-out-of-bounds.
pub fn parse(id: DatId, data: &[u8]) -> SkeletonAnimation {
    if data.len() < POOL_START {
        return SkeletonAnimation {
            id,
            num_joints: 0,
            num_frames: 0,
            key_frame_duration: 0.0,
            key_frame_sets: HashMap::new(),
        };
    }

    let _unk0 = read_u16(data, 0);
    let num_joints = read_u16(data, 2) as usize;
    let num_frames = read_u16(data, 4) as usize;
    let key_frame_duration = read_f32(data, 6);

    let mut pos = POOL_START;
    let mut key_frame_sets: HashMap<u32, Vec<KeyFrameTransform>> = HashMap::new();

    for _ in 0..num_joints {
        let joint_index = read_u32(data, pos);
        pos += 4;

        let rotation = read_sequences(data, &mut pos, 4, num_frames);
        let translation = read_sequences(data, &mut pos, 3, num_frames);
        let scale = read_sequences(data, &mut pos, 3, num_frames);

        let (Some(rotation), Some(translation), Some(scale)) = (rotation, translation, scale)
        else {
            continue;
        };

        let mut frames = Vec::with_capacity(num_frames);
        for f in 0..num_frames {
            frames.push(KeyFrameTransform {
                rotation: [
                    rotation[0].value(f),
                    rotation[1].value(f),
                    rotation[2].value(f),
                    rotation[3].value(f),
                ],
                translation: [
                    translation[0].value(f),
                    translation[1].value(f),
                    translation[2].value(f),
                ],
                scale: [scale[0].value(f), scale[1].value(f), scale[2].value(f)],
            });
        }
        key_frame_sets.insert(joint_index, frames);
    }

    SkeletonAnimation {
        id,
        num_joints,
        num_frames,
        key_frame_duration,
        key_frame_sets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pi32(buf: &mut Vec<u8>, v: i32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    fn pf(buf: &mut Vec<u8>, v: f32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    fn pu16(buf: &mut Vec<u8>, v: u16) {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    fn pu32(buf: &mut Vec<u8>, v: u32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }

    /// One joint with constant rotation (identity), translation, scale and
    /// `num_frames` frames. Pool data appended after the joint header so a
    /// frame-sequence channel can reference it.
    fn build_anim(num_frames: u16, duration: f32) -> Vec<u8> {
        let mut b = Vec::new();
        pu16(&mut b, 0); // unk0
        pu16(&mut b, 1); // numJoints
        pu16(&mut b, num_frames);
        pf(&mut b, duration);
        // joint 0
        pu32(&mut b, 0); // jointIndex
                         // rotation: offsets all 0 -> const (0,0,0,1)
        pi32(&mut b, 0);
        pi32(&mut b, 0);
        pi32(&mut b, 0);
        pi32(&mut b, 0);
        pf(&mut b, 0.0);
        pf(&mut b, 0.0);
        pf(&mut b, 0.0);
        pf(&mut b, 1.0);
        // translation: x is a frame sequence (offset points into pool), y/z const.
        // The pool sequence sits at body offset POOL_START + poolIndex*4. The
        // joint header (10 hdr + 4 joint + 32 rot + 24 trans + 24 scale) ends at
        // byte 94, and (94 - 10) is divisible by 4, so the pool starts there.
        let pool_byte = 94usize;
        let pool_index = ((pool_byte - POOL_START) / 4) as i32;
        pi32(&mut b, pool_index); // x offset -> sequence
        pi32(&mut b, 0); // y const
        pi32(&mut b, 0); // z const
        pf(&mut b, 0.0); // x const (ignored since offset!=0)
        pf(&mut b, 5.0); // y const
        pf(&mut b, 7.0); // z const
                         // scale: all const 1
        pi32(&mut b, 0);
        pi32(&mut b, 0);
        pi32(&mut b, 0);
        pf(&mut b, 1.0);
        pf(&mut b, 1.0);
        pf(&mut b, 1.0);
        // pad up to pool_byte
        b.resize(pool_byte, 0);
        // pool sequence: numFrames floats for translation.x, increasing
        for f in 0..num_frames {
            pf(&mut b, f as f32 * 10.0);
        }
        b
    }

    #[test]
    fn const_and_sequence_channels() {
        let b = build_anim(3, 1.0);
        let anim = parse(DatId::from_str("idl0"), &b);
        assert_eq!(anim.num_frames, 3);
        let set = anim.key_frame_sets.get(&0).unwrap();
        assert_eq!(set.len(), 3);
        // rotation const identity each frame
        assert_eq!(set[0].rotation, [0.0, 0.0, 0.0, 1.0]);
        // translation.x = sequence (0,10,20), y=5 const, z=7 const
        assert_eq!(set[0].translation, [0.0, 5.0, 7.0]);
        assert_eq!(set[1].translation, [10.0, 5.0, 7.0]);
        assert_eq!(set[2].translation, [20.0, 5.0, 7.0]);
        // scale const
        assert_eq!(set[0].scale, [1.0, 1.0, 1.0]);
    }

    #[test]
    fn negative_offset_skips_joint() {
        let mut b = Vec::new();
        pu16(&mut b, 0);
        pu16(&mut b, 1); // 1 joint
        pu16(&mut b, 2); // 2 frames
        pf(&mut b, 1.0);
        pu32(&mut b, 0); // jointIndex
                         // rotation: first offset negative -> whole joint dropped
        pi32(&mut b, -1);
        pi32(&mut b, 0);
        pi32(&mut b, 0);
        pi32(&mut b, 0);
        pf(&mut b, 0.0);
        pf(&mut b, 0.0);
        pf(&mut b, 0.0);
        pf(&mut b, 1.0);
        // translation + scale const
        for _ in 0..6 {
            pi32(&mut b, 0);
        }
        for _ in 0..6 {
            pf(&mut b, 0.0);
        }
        let anim = parse(DatId::from_str("idl0"), &b);
        assert!(anim.key_frame_sets.is_empty());
    }

    #[test]
    fn const_value_modulo_10000() {
        let mut b = Vec::new();
        pu16(&mut b, 0);
        pu16(&mut b, 1);
        pu16(&mut b, 1); // 1 frame
        pf(&mut b, 1.0);
        pu32(&mut b, 0);
        // rotation const, but w value = 10005 -> rem 10000 = 5
        pi32(&mut b, 0);
        pi32(&mut b, 0);
        pi32(&mut b, 0);
        pi32(&mut b, 0);
        pf(&mut b, 0.0);
        pf(&mut b, 0.0);
        pf(&mut b, 0.0);
        pf(&mut b, 10005.0);
        for _ in 0..6 {
            pi32(&mut b, 0);
        }
        for _ in 0..6 {
            pf(&mut b, 0.0);
        }
        let anim = parse(DatId::from_str("idl0"), &b);
        let set = anim.key_frame_sets.get(&0).unwrap();
        assert!((set[0].rotation[3] - 5.0).abs() < 1e-4);
    }

    #[test]
    fn interpolate_endpoints_and_midpoint() {
        let a = KeyFrameTransform {
            rotation: [0.0, 0.0, 0.0, 1.0],
            translation: [0.0, 0.0, 0.0],
            scale: [1.0, 1.0, 1.0],
        };
        let b = KeyFrameTransform {
            rotation: [0.0, 0.0, 0.0, 1.0],
            translation: [10.0, 0.0, 0.0],
            scale: [3.0, 1.0, 1.0],
        };
        let at0 = interpolate(&a, &b, 0.0);
        assert_eq!(at0.translation, [0.0, 0.0, 0.0]);
        let at1 = interpolate(&a, &b, 1.0);
        assert!((at1.translation[0] - 10.0).abs() < 1e-5);
        let mid = interpolate(&a, &b, 0.5);
        assert!((mid.translation[0] - 5.0).abs() < 1e-5);
        assert!((mid.scale[0] - 2.0).abs() < 1e-5);
    }

    #[test]
    fn nlerp_shortest_path_and_normalized() {
        // qa identity, qb = -identity (opposite hemisphere); dot<0 so it
        // flips qb -> identity, nlerp stays normalized identity.
        let q = nlerp([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, -1.0], 0.5);
        let mag = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        assert!((mag - 1.0).abs() < 1e-5);
        assert!((q[3].abs() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn clamps_at_and_after_last_frame() {
        let b = build_anim(3, 1.0);
        let anim = parse(DatId::from_str("idl0"), &b);
        // frame beyond end -> last frame (translation.x = 20)
        let t = anim.get_joint_transform(0, 5.0).unwrap();
        assert_eq!(t.translation[0], 20.0);
        // exactly at numFrames-1
        let t2 = anim.get_joint_transform(0, 2.0).unwrap();
        assert_eq!(t2.translation[0], 20.0);
        // interpolated mid
        let t3 = anim.get_joint_transform(0, 0.5).unwrap();
        assert!((t3.translation[0] - 5.0).abs() < 1e-4);
        // missing joint
        assert!(anim.get_joint_transform(99, 0.0).is_none());
    }

    #[test]
    fn length_in_frames() {
        let b = build_anim(5, 2.0);
        let anim = parse(DatId::from_str("idl0"), &b);
        // (5-1).max(1) / 2.0 = 2.0
        assert!((anim.length_in_frames() - 2.0).abs() < 1e-5);
    }
}
