use crate::datid::DatId;

// research/xim resource/SkeletonSection.kt StandardPosition
pub mod standard_position {
    /// The in-world look-at aims at the target's attach point 3 (FFXiMain.dll retail-2026-09 RVA
    /// 0xD5C86); research/XIClient/src/XIClient/include/World/Actor/EID_INDEX.h EID_NECK names it.
    pub const NECK: usize = 3;
    /// The bend's second record bends the bone named by reference slot 7, which XIM names EID_CHEST;
    /// retail writes the two bend references as the byte pair {3, 7} at FFXiMain.dll retail-2026-09 RVA
    /// 0x2AC7A / 0x2AC7F and resolves each through the reference table by RVA 0x2A9B0.
    pub const CHEST: usize = 7;
    pub const ABOVE_HEAD: usize = 2;
    pub const RIGHT_FOOT: usize = 8;
    pub const LEFT_FOOT: usize = 9;
    pub const LEFT_HAND: usize = 126;
    pub const RIGHT_HAND: usize = 127;
}

const CDCD_SENTINEL: u32 = 0xCDCDCDCD;

/// Bytes between the end of the joint-reference table and the first look-at record, matching the
/// fixed `+0x48` in FFXiMain.dll retail-2026-09 RVA 0x35270; that gap is where the bounding boxes live.
const LOOK_AT_RECORDS_OFFSET: usize = 0x48;

/// Stride of one look-at record, from the same `+12*index` term in FFXiMain.dll retail-2026-09 RVA
/// 0x35270.
const LOOK_AT_RECORD_BYTES: usize = 12;

/// One bounding box is six f32 extents.
const BOUNDING_BOX_BYTES: usize = 6 * 4;

#[derive(Debug, Clone)]
pub struct Joint {
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
    pub parent: Option<usize>,
}

/// One entry of a skeleton's reference table (`FFXiMain.dll retail-2026-09` RVA 0x2A9B5 reads an entry as a `word`
/// joint index plus three floats at +2/+6/+0xA and an offset, stride 26).
#[derive(Debug, Clone)]
pub struct JointReference {
    /// The joint this reference stands for: the entry's first `u16` (`FFXiMain.dll retail-2026-09` reads it at
    /// RVA 0x2A9D4) and used directly as a bone index, with no remap behind it (RVA 0x2AFED → 0x2A9B0 in the look-at
    /// bend).
    pub index: usize,
    /// The attach frame's authored Euler rotation in radians, applied X then Y then Z onto a zeroed matrix
    /// (`FFXiMain.dll retail-2026-09` RVA 0x27B80 / 0x27BD0 / 0x27C20 inside the builder at RVA 0x2A780). Measured
    /// across all six PC skeletons it is authored `(0,0,0)` on the slots the look-at bend uses (3, 4 and 7), so no
    /// shipped rig distinguishes the application order today.
    pub rotation: [f32; 3],
    /// The attach frame's translation in the skeleton's own axis space, written as that matrix's translation
    /// (`FFXiMain.dll retail-2026-09` RVA 0x27CF0 to `+0x30/+0x34/+0x38`) — which is why it places relative to the
    /// joint instead of being spun by
    /// [`JointReference::rotation`].
    pub position_offset: [f32; 3],
}

#[derive(Debug, Clone, Copy)]
pub struct BoundingBox {
    pub y_max: f32,
    pub y_min: f32,
    pub x_max: f32,
    pub x_min: f32,
    pub z_max: f32,
    pub z_min: f32,
}

/// One authored angular limit for the look-at bend, as `FFXiMain.dll` retail-2026-09 RVA
/// 0x2B140 reads it: an ellipse in the bone's tangent plane with semi-axes `x_limit` /
/// `y_limit`, projected with `scale`. A record of zeros means that bone does not bend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LookAtLimit {
    pub x_limit: f32,
    pub y_limit: f32,
    pub scale: f32,
}

#[derive(Debug, Clone)]
pub struct Skeleton {
    pub id: DatId,
    pub joints: Vec<Joint>,
    pub references: Vec<JointReference>,
    /// Only the boxes inside [`LOOK_AT_RECORDS_OFFSET`] are authored; past it the section holds
    /// look-at records, not geometry.
    pub bounding_boxes: Vec<BoundingBox>,
    pub look_at_limits: Vec<LookAtLimit>,
}

impl Skeleton {
    pub fn reference_at(&self, standard_index: usize) -> Option<&JointReference> {
        self.references.get(standard_index)
    }

    /// Bend record `record` (0 then 1; the bend skips a zeroed record), as indexed by
    /// `FFXiMain.dll` retail-2026-09 RVA 0x35270 and applied in order by RVA 0x2AC60, which drops
    /// the second record for certain actor statuses.
    pub fn look_at_limit(&self, record: usize) -> Option<&LookAtLimit> {
        self.look_at_limits.get(record)
    }

    /// The vertical extent of the first authored box, its second value less its first (head top to feet in
    /// the skeleton's y-down space): the span retail's chase camera sets its pivot height from
    /// (`FFXiMain.dll retail-2026-09`, RVA 0x1F3A2 reads it through RVA 0x35250).
    pub fn height_span(&self) -> Option<f32> {
        self.bounding_boxes.first().map(|b| b.y_min - b.y_max)
    }
}

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

pub fn parse(id: DatId, data: &[u8]) -> Skeleton {
    let num_joints = read_u8(data, 0x02) as usize;

    let mut joints = Vec::with_capacity(num_joints);
    let mut pos = 0x04;
    for i in 0..num_joints {
        let maybe_parent = read_u8(data, pos) as usize;

        let parent = if maybe_parent == i {
            None
        } else {
            Some(maybe_parent)
        };
        pos += 2;

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
        pos += 28;
        joints.push(Joint {
            rotation,
            translation,
            parent,
        });
    }

    let num_references = read_u16(data, pos) as usize;
    pos += 2;
    pos += 2;

    let mut references = Vec::with_capacity(num_references);
    for _ in 0..num_references {
        let index = read_u16(data, pos) as usize;
        let rotation = [
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
            rotation,
            position_offset,
        });
    }

    // The section after the reference table is fixed-size, not count-prefixed: the boxes occupy
    // exactly `LOOK_AT_RECORDS_OFFSET` bytes and the look-at records start there whatever the box
    // data looks like. FFXiMain.dll retail-2026-09 RVA 0x35270 indexes a record as
    // `refs_end + 0x48 + 12*i`, so reading past that offset as boxes turns authored limits (and the
    // sentinel padding after them) into fake geometry.
    let look_at_base = pos + LOOK_AT_RECORDS_OFFSET;
    let box_limit = (pos + LOOK_AT_RECORDS_OFFSET).min(data.len());
    let mut bounding_boxes = Vec::new();
    'boxes: while pos + BOUNDING_BOX_BYTES <= box_limit {
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

    let mut look_at_limits = Vec::new();
    'records: {
        if look_at_base > data.len() {
            break 'records;
        }
        let mut pos = look_at_base;
        loop {
            if pos + LOOK_AT_RECORD_BYTES > data.len() || read_f32_bits(data, pos) == CDCD_SENTINEL
            {
                break;
            }
            look_at_limits.push(LookAtLimit {
                x_limit: read_f32(data, pos),
                y_limit: read_f32(data, pos + 4),
                scale: read_f32(data, pos + 8),
            });
            pos += LOOK_AT_RECORD_BYTES;
        }
    }

    Skeleton {
        id,
        joints,
        references,
        bounding_boxes,
        look_at_limits,
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

        body[0x02] = 2;

        body.push(0);
        body.push(0);
        for c in [0.0f32, 0.0, 0.0, 1.0] {
            push_f32(&mut body, c);
        }
        for c in [10.0f32, 20.0, 30.0] {
            push_f32(&mut body, c);
        }

        body.push(0);
        body.push(0);
        for c in [0.0f32, 0.0, 0.0, 1.0] {
            push_f32(&mut body, c);
        }
        for c in [1.0f32, 2.0, 3.0] {
            push_f32(&mut body, c);
        }

        body.extend_from_slice(&1u16.to_le_bytes());
        body.extend_from_slice(&0xFFFFu16.to_le_bytes());

        body.extend_from_slice(&7u16.to_le_bytes());
        for c in [0.1f32, 0.2, 0.3] {
            push_f32(&mut body, c);
        }
        for c in [4.0f32, 5.0, 6.0] {
            push_f32(&mut body, c);
        }

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
        let mut body = vec![0u8; 0x04];
        body[0x02] = 0;
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0xFFFFu16.to_le_bytes());
        for c in [9.0f32, -9.0, 8.0, -8.0, 7.0, -7.0] {
            push_f32(&mut body, c);
        }

        let skel = parse(DatId::from_str("0000"), &body);
        assert_eq!(skel.bounding_boxes.len(), 1);
        let bb = skel.bounding_boxes[0];
        assert_eq!((bb.y_max, bb.y_min, bb.x_max), (9.0, -9.0, 8.0));
        assert_eq!((bb.x_min, bb.z_max, bb.z_min), (-8.0, 7.0, -7.0));
    }

    #[test]
    fn the_height_span_runs_from_the_first_boxs_first_value_to_its_second() {
        let (top, feet) = (-1.9f32, 0.0f32);
        let skel = parse(
            DatId::from_str("0000"),
            &empty_refs_tail(&[top, feet, 0.7, -0.7, 0.7, -0.7], &[]),
        );
        assert_eq!(skel.height_span(), Some(feet - top));
        let unboxed = parse(DatId::from_str("0000"), &empty_refs_tail(&[], &[]));
        assert_eq!(unboxed.height_span(), None);
    }

    #[test]
    fn truncated_chunk_does_not_panic() {
        let mut body = vec![0u8; 0x04];
        body[0x02] = 3;
        let skel = parse(DatId::from_str("0000"), &body);
        assert_eq!(skel.joints.len(), 3);
        assert!(skel.references.is_empty());
        assert!(skel.bounding_boxes.is_empty());
    }

    #[test]
    fn cdcd_terminates_immediately() {
        let mut body = vec![0u8; 0x04];
        body[0x02] = 0;
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0xFFFFu16.to_le_bytes());
        body.extend_from_slice(&CDCD_SENTINEL.to_le_bytes());
        let skel = parse(DatId::from_str("0000"), &body);
        assert!(skel.bounding_boxes.is_empty());
    }

    /// A body with no joints and no references, so the authored tail starts at 8 and stays there.
    fn empty_refs_tail(values: &[f32], extra: &[u8]) -> Vec<u8> {
        let mut body = vec![0u8; 0x04];
        body[0x02] = 0;
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0xFFFFu16.to_le_bytes());
        for v in values {
            push_f32(&mut body, *v);
        }
        body.extend_from_slice(extra);
        body
    }

    /// The retail look-at records sit at a fixed offset past the reference table, so three authored
    /// boxes are followed by three records of {x,y,scale} rather than being read as more geometry.
    #[test]
    fn look_at_records_follow_three_bounding_boxes() {
        let mut values = vec![
            1.0, -1.0, 2.0, -2.0, 3.0, -3.0, // box 1
            4.0, -4.0, 5.0, -5.0, 6.0, -6.0, // box 2
            7.0, -7.0, 8.0, -8.0, 9.0, -9.0, // box 3
        ];
        values.extend([0.24, 0.16, 0.5, 0.16, 0.06, 0.5, 0.0, 0.0, 0.5]);
        let body = empty_refs_tail(&values, &CDCD_SENTINEL.to_le_bytes());

        let skel = parse(DatId::from_str("0000"), &body);
        assert_eq!(skel.bounding_boxes.len(), 3);
        assert_eq!(
            (skel.bounding_boxes[2].y_max, skel.bounding_boxes[2].z_min),
            (7.0, -9.0)
        );
        let limits: Vec<[f32; 3]> = skel
            .look_at_limits
            .iter()
            .map(|l| [l.x_limit, l.y_limit, l.scale])
            .collect();
        assert_eq!(
            limits,
            vec![[0.24, 0.16, 0.5], [0.16, 0.06, 0.5], [0.0, 0.0, 0.5]]
        );
    }

    /// The record offset is fixed, not "wherever the boxes stopped": a section whose second group
    /// is sentinel padding still resolves its records at `+0x48`.
    #[test]
    fn look_at_records_do_not_slide_with_the_box_count() {
        let mut body = empty_refs_tail(&[1.0, -1.0, 2.0, -2.0, 3.0, -3.0], &[]);
        for _ in 0..6 {
            body.extend_from_slice(&CDCD_SENTINEL.to_le_bytes());
        }
        let pad = LOOK_AT_RECORDS_OFFSET - (body.len() - 8);
        body.resize(body.len() + pad, 0);
        for v in [0.1, 0.2, 0.3] {
            push_f32(&mut body, v);
        }

        let skel = parse(DatId::from_str("0000"), &body);
        assert_eq!(skel.bounding_boxes.len(), 1);
        assert_eq!(
            skel.look_at_limit(0),
            Some(&LookAtLimit {
                x_limit: 0.1,
                y_limit: 0.2,
                scale: 0.3
            })
        );
    }

    /// The HumeM model DAT (ROM/27/82.DAT, directory `hm_s`), the skeleton the actor code poses.
    const HUME_M_MODEL_DAT: u32 = 7072;

    /// Gated on an install (self-skips). Retail's authored tail is what fixes the split at:
    /// three bounding boxes, then the `{xlim, ylim, scale}` records — humanoid head `(0.24,
    /// 0.16)`, neck/shoulder `(0.16, 0.06)`, and a third record whose limits are zero.
    #[test]
    fn real_dat_hume_skeleton_authors_look_at_limits_after_three_boxes() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let Ok(loc) = root.resolve(HUME_M_MODEL_DAT) else {
            eprintln!("SKIP: HumeM model DAT unresolvable");
            return;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            eprintln!("SKIP: HumeM model DAT unreadable");
            return;
        };
        let dir = crate::resource_dir::ResourceDir::from_bytes(bytes);
        let Some(skel) = dir.collect_skeletons().into_iter().next() else {
            eprintln!("SKIP: no skeleton chunk");
            return;
        };

        assert_eq!(
            (skel.id, skel.bounding_boxes.len()),
            (DatId::from_str("hum_"), 3)
        );
        for bb in &skel.bounding_boxes {
            let extents = [bb.y_max, bb.y_min, bb.x_max, bb.x_min, bb.z_max, bb.z_min];
            assert!(
                extents.iter().all(|v| v.abs() < 1.0e6),
                "a box built from limit floats or sentinel padding: {extents:?}"
            );
        }

        let limits: Vec<[f32; 3]> = skel
            .look_at_limits
            .iter()
            .map(|l| [l.x_limit, l.y_limit, l.scale])
            .collect();
        assert_eq!(
            limits,
            vec![[0.24, 0.16, 0.5], [0.16, 0.06, 0.5], [0.0, 0.0, 0.5]]
        );
    }

    /// Geometry past the fixed section is never read as another bounding box.
    #[test]
    fn bounding_box_section_is_capped_at_the_record_offset() {
        let mut values = Vec::new();
        for group in 0..5u32 {
            values.extend([group as f32 + 1.0, -1.0, -2.0, 2.0, 3.0, -3.0]);
        }
        let body = empty_refs_tail(&values, &[]);

        let skel = parse(DatId::from_str("0000"), &body);
        assert_eq!(skel.bounding_boxes.len(), 3);
        assert_eq!(skel.bounding_boxes[2].y_max, 3.0);
        assert_eq!(
            skel.look_at_limits,
            vec![
                LookAtLimit {
                    x_limit: 4.0,
                    y_limit: -1.0,
                    scale: -2.0
                },
                LookAtLimit {
                    x_limit: 2.0,
                    y_limit: 3.0,
                    scale: -3.0
                },
                LookAtLimit {
                    x_limit: 5.0,
                    y_limit: -1.0,
                    scale: -2.0
                },
                LookAtLimit {
                    x_limit: 2.0,
                    y_limit: 3.0,
                    scale: -3.0
                }
            ]
        );
    }
}
