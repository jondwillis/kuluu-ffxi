//! Skeleton section parser (chunk 0x29) — port of XIM `SkeletonSection.kt`.
//!
//! Body layout (offsets relative to chunk `data`, which == XIM's
//! `dataStartPosition`):
//!   0x02  u8  numJoints
//!   0x04  Joint[numJoints], 30 bytes each
//!   ...   u16 numReferences, u16 unk
//!   ...   JointReference[numReferences], 26 bytes each
//!   ...   bounding boxes: 6 f32 each until a 0xCDCDCDCD sentinel / body end

use crate::datid::DatId;

/// XIM `StandardPosition` reference indices.
pub mod standard_position {
    pub const ABOVE_HEAD: usize = 2;
    pub const RIGHT_FOOT: usize = 8;
    pub const LEFT_FOOT: usize = 9;
    pub const LEFT_HAND: usize = 126;
    pub const RIGHT_HAND: usize = 127;
}

const CDCD_SENTINEL: u32 = 0xCDCDCDCD;

#[derive(Debug, Clone)]
pub struct Joint {
    pub rotation: [f32; 4], // x, y, z, w
    pub translation: [f32; 3],
    pub parent: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct JointReference {
    pub index: usize,
    pub unk_v0: [f32; 3],
    pub position_offset: [f32; 3],
}

/// Bounding box stored in the file's read order: yMax,yMin,xMax,xMin,zMax,zMin.
#[derive(Debug, Clone, Copy)]
pub struct BoundingBox {
    pub y_max: f32,
    pub y_min: f32,
    pub x_max: f32,
    pub x_min: f32,
    pub z_max: f32,
    pub z_min: f32,
}

#[derive(Debug, Clone)]
pub struct Skeleton {
    pub id: DatId,
    pub joints: Vec<Joint>,
    pub references: Vec<JointReference>,
    pub bounding_boxes: Vec<BoundingBox>,
}

impl Skeleton {
    /// Look up a reference by a `standard_position` index.
    pub fn reference_at(&self, standard_index: usize) -> Option<&JointReference> {
        self.references.get(standard_index)
    }
}

// XIM reads over a Uint8Array, which yields 0 past the end rather than
// throwing; these helpers mirror that by returning 0 on a short read so a
// truncated chunk produces a tolerant parse instead of an index panic.
fn read_f32(data: &[u8], off: usize) -> f32 {
    f32::from_bits(read_f32_bits(data, off))
}

fn read_f32_bits(data: &[u8], off: usize) -> u32 {
    let b = |i: usize| data.get(off + i).copied().unwrap_or(0);
    u32::from_le_bytes([b(0), b(1), b(2), b(3)])
}

fn read_u16(data: &[u8], off: usize) -> u16 {
    let b = |i: usize| data.get(off + i).copied().unwrap_or(0);
    u16::from_le_bytes([b(0), b(1)])
}

fn read_u8(data: &[u8], off: usize) -> u8 {
    data.get(off).copied().unwrap_or(0)
}

/// Parse a 0x29 skeleton chunk body. `data` is the chunk body (`Chunk.data`).
///
/// Bounded like XIM's ByteReader-over-Uint8Array: every read tolerates an
/// over-run (yields 0) rather than panicking, so a truncated chunk produces a
/// partial-but-valid `Skeleton` instead of an index-out-of-bounds.
pub fn parse(id: DatId, data: &[u8]) -> Skeleton {
    let num_joints = read_u8(data, 0x02) as usize;

    let mut joints = Vec::with_capacity(num_joints);
    let mut pos = 0x04;
    for i in 0..num_joints {
        let maybe_parent = read_u8(data, pos) as usize;
        // XIM: parent == own index -> root.
        let parent = if maybe_parent == i {
            None
        } else {
            Some(maybe_parent)
        };
        pos += 2; // parent byte + 1 pad

        let rotation = [
            read_f32(data, pos),
            read_f32(data, pos + 4),
            read_f32(data, pos + 8),
            read_f32(data, pos + 12),
        ];
        let translation = [
            read_f32(data, pos + 16),
            read_f32(data, pos + 20),
            read_f32(data, pos + 24),
        ];
        pos += 28; // 4 f32 rotation + 3 f32 translation
        joints.push(Joint {
            rotation,
            translation,
            parent,
        });
    }

    let num_references = read_u16(data, pos) as usize;
    pos += 2;
    pos += 2; // unk u16

    let mut references = Vec::with_capacity(num_references);
    for _ in 0..num_references {
        let index = read_u16(data, pos) as usize;
        let unk_v0 = [
            read_f32(data, pos + 2),
            read_f32(data, pos + 6),
            read_f32(data, pos + 10),
        ];
        let position_offset = [
            read_f32(data, pos + 14),
            read_f32(data, pos + 18),
            read_f32(data, pos + 22),
        ];
        pos += 26;
        references.push(JointReference {
            index,
            unk_v0,
            position_offset,
        });
    }

    // XIM loops `while position < sectionEndPosition`, reading each float
    // unconditionally and stopping only on the 0xCDCDCDCD sentinel or the body
    // end (over-read -> 0). A final box flush against the body end with no
    // sentinel is therefore kept, not dropped.
    let mut bounding_boxes = Vec::new();
    'boxes: while pos < data.len() {
        let mut vals = [0f32; 6];
        for v in vals.iter_mut() {
            if read_f32_bits(data, pos) == CDCD_SENTINEL {
                break 'boxes;
            }
            *v = read_f32(data, pos);
            pos += 4;
        }
        bounding_boxes.push(BoundingBox {
            y_max: vals[0],
            y_min: vals[1],
            x_max: vals[2],
            x_min: vals[3],
            z_max: vals[4],
            z_min: vals[5],
        });
    }

    Skeleton {
        id,
        joints,
        references,
        bounding_boxes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_f32(buf: &mut Vec<u8>, v: f32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }

    #[test]
    fn parent_self_is_root_and_strides() {
        let mut body = vec![0u8; 0x04];
        // numJoints @ 0x02 = 2
        body[0x02] = 2;
        // joint 0: parent == 0 (self) -> root
        body.push(0); // parent
        body.push(0); // pad
        for c in [0.0f32, 0.0, 0.0, 1.0] {
            push_f32(&mut body, c);
        }
        for c in [10.0f32, 20.0, 30.0] {
            push_f32(&mut body, c);
        }
        // joint 1: parent == 0
        body.push(0); // parent
        body.push(0); // pad
        for c in [0.0f32, 0.0, 0.0, 1.0] {
            push_f32(&mut body, c);
        }
        for c in [1.0f32, 2.0, 3.0] {
            push_f32(&mut body, c);
        }
        // numReferences = 1, unk
        body.extend_from_slice(&1u16.to_le_bytes());
        body.extend_from_slice(&0xFFFFu16.to_le_bytes());
        // one reference (26 bytes): index=7, unkV0, offset
        body.extend_from_slice(&7u16.to_le_bytes());
        for c in [0.1f32, 0.2, 0.3] {
            push_f32(&mut body, c);
        }
        for c in [4.0f32, 5.0, 6.0] {
            push_f32(&mut body, c);
        }
        // one bounding box (6 f32), then sentinel
        for c in [1.0f32, -1.0, 2.0, -2.0, 3.0, -3.0] {
            push_f32(&mut body, c);
        }
        body.extend_from_slice(&CDCD_SENTINEL.to_le_bytes());

        let skel = parse(DatId::from_str("0000"), &body);
        assert_eq!(skel.joints.len(), 2);
        assert_eq!(skel.joints[0].parent, None);
        assert_eq!(skel.joints[0].translation, [10.0, 20.0, 30.0]);
        assert_eq!(skel.joints[0].rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(skel.joints[1].parent, Some(0));
        assert_eq!(skel.joints[1].translation, [1.0, 2.0, 3.0]);

        assert_eq!(skel.references.len(), 1);
        assert_eq!(skel.references[0].index, 7);
        assert_eq!(skel.references[0].position_offset, [4.0, 5.0, 6.0]);

        assert_eq!(skel.bounding_boxes.len(), 1);
        let bb = skel.bounding_boxes[0];
        assert_eq!((bb.y_max, bb.y_min, bb.x_max), (1.0, -1.0, 2.0));
        assert_eq!((bb.x_min, bb.z_max, bb.z_min), (-2.0, 3.0, -3.0));
    }

    #[test]
    fn terminal_box_without_sentinel_is_kept() {
        // XIM keeps reading while any body byte remains, so a final box flushed
        // against the body end with no 0xCDCDCDCD sentinel must NOT be dropped.
        let mut body = vec![0u8; 0x04];
        body[0x02] = 0; // no joints
        body.extend_from_slice(&0u16.to_le_bytes()); // numReferences
        body.extend_from_slice(&0xFFFFu16.to_le_bytes()); // unk
        for c in [9.0f32, -9.0, 8.0, -8.0, 7.0, -7.0] {
            push_f32(&mut body, c);
        }
        // No trailing sentinel: body ends exactly after the 6 floats.
        let skel = parse(DatId::from_str("0000"), &body);
        assert_eq!(skel.bounding_boxes.len(), 1);
        let bb = skel.bounding_boxes[0];
        assert_eq!((bb.y_max, bb.y_min, bb.x_max), (9.0, -9.0, 8.0));
        assert_eq!((bb.x_min, bb.z_max, bb.z_min), (-8.0, 7.0, -7.0));
    }

    #[test]
    fn truncated_chunk_does_not_panic() {
        // A header claiming joints/references but with a body cut short must
        // produce a tolerant (zero-filled) parse rather than panicking.
        let mut body = vec![0u8; 0x04];
        body[0x02] = 3; // claims 3 joints, but body ends here
        let skel = parse(DatId::from_str("0000"), &body);
        assert_eq!(skel.joints.len(), 3);
        assert!(skel.references.is_empty());
        assert!(skel.bounding_boxes.is_empty());
    }

    #[test]
    fn cdcd_terminates_immediately() {
        let mut body = vec![0u8; 0x04];
        body[0x02] = 0; // no joints
        body.extend_from_slice(&0u16.to_le_bytes()); // numReferences
        body.extend_from_slice(&0xFFFFu16.to_le_bytes()); // unk
        body.extend_from_slice(&CDCD_SENTINEL.to_le_bytes());
        let skel = parse(DatId::from_str("0000"), &body);
        assert!(skel.bounding_boxes.is_empty());
    }
}
