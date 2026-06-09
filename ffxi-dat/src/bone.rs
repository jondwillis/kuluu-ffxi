//! Skeleton / Bone (Sk2) chunk parser, kind 0x29.
//!
//! Layout reverse-engineered from lotus-ffxi's `sk2.cppm` (see
//! <https://github.com/teschnei/lotus-ffxi/blob/main/ffxi/dat/sk2.cppm>)
//! and empirically confirmed against file 7072 chunk[70] (humanoid
//! skeleton, 94 bones; mean |q| over all bones = 1.000).
//!
//! After the 16-byte DAT chunk header is stripped by [`crate::walk`],
//! the chunk body is:
//!
//! ```text
//! offset 0  u16 _pad
//! offset 2  u16 bone_count
//! offset 4  Bone[bone_count]     // pack(2), 30 bytes each
//! ...       trailing tables (GeneratorPoint table + extras) — ignored
//! ```
//!
//! Each [`Bone`] is 30 bytes, pack-2:
//!
//! ```text
//!   u8  parent_index     // 0xFF = root (per lotus-ffxi convention).
//!                        // The high byte of what lotus-ffxi declares
//!                        // as `u16 parent_index` is in practice an
//!                        // alternating flag (likely IK / animation
//!                        // channel) — we surface it separately so a
//!                        // simple `bones[i].parent` only sees the
//!                        // index byte.
//!   u8  flags            // Empirically alternates 0/1 between bones.
//!                        // Currently surfaced but not interpreted.
//!   f32 rot[4]           // local-to-parent quaternion in x,y,z,w
//!                        // order (glm::quat in-memory layout).
//!   f32 trans[3]         // local-to-parent translation.
//! ```
//!
//! Quaternion + translation are **bind-pose local** to the parent —
//! the runtime walks the parent chain to compose world transforms.
//! See [`Skeleton::bind_pose_world`] for the chain composer.

use crate::Result;

/// Sentinel for "no parent" used by lotus-ffxi. In FFXI's per-bone u8
/// the practical root convention is `parent_index == 0` on bone 0
/// (which is self-parenting and ignored by the composer). We treat
/// `parent == 0xFF` *or* `parent == own_index` as "stop walking."
pub const PARENT_ROOT: u8 = 0xFF;

/// Total size of one packed `Bone` record on disk.
pub const BONE_STRIDE: usize = 30;

/// Sk2 chunk header. Stored as two u16s at the start of the chunk
/// body (after the 16-byte DAT chunk header is stripped).
#[derive(Debug, Clone, Copy)]
pub struct BoneHeader {
    /// Unknown leading u16. Lotus-ffxi calls it `_pad`; it has been
    /// observed to be 0 on the samples surveyed so far. Surfaced for
    /// completeness so a future investigation can correlate non-zero
    /// values against skeleton variants.
    pub pad: u16,
    /// Number of `Bone` records following the header.
    pub count: u16,
}

impl BoneHeader {
    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < 4 {
            return None;
        }
        let pad = u16::from_le_bytes([body[0], body[1]]);
        let count = u16::from_le_bytes([body[2], body[3]]);
        Some(Self { pad, count })
    }
}

/// One bone in bind pose, local to its parent.
#[derive(Debug, Clone, Copy)]
pub struct Bone {
    /// Index of the parent bone in the same `Skeleton::bones` array.
    /// `0` on `bones[0]` is self-parenting and treated as root.
    pub parent: u8,
    /// Per-bone flag byte. Empirically alternates 0/1 along the chain
    /// — meaning not yet pinned, possibly IK/animation channel
    /// selector. Surfaced raw; consumers can ignore for bind-pose.
    pub flags: u8,
    /// Local-to-parent rotation as a quaternion in **x, y, z, w**
    /// order (glm::quat in-memory layout). Always unit-length on real
    /// skeletons.
    pub rot: [f32; 4],
    /// Local-to-parent translation in FFXI yalms.
    pub trans: [f32; 3],
}

/// A parsed Sk2 skeleton: header + bone array.
///
/// The trailing GeneratorPoint table + any post-table extras are not
/// parsed by this type; if your use case needs particle anchor points,
/// extend the parser to read `body[4 + count*BONE_STRIDE ..]`.
#[derive(Debug, Clone)]
pub struct Skeleton {
    pub header: BoneHeader,
    pub bones: Vec<Bone>,
}

impl Skeleton {
    /// Parse a Sk2 chunk body (post-DAT-header).
    ///
    /// Returns `Err(DatError::TruncatedChunk)` if `body` is too small
    /// to hold `count` × 30-byte bone records after the 4-byte
    /// `BoneHeader`. Trailing bytes past the bone array are tolerated
    /// (they hold the GeneratorPoint table and other sections we
    /// don't model).
    pub fn parse(body: &[u8]) -> Result<Self> {
        let header = BoneHeader::parse(body).ok_or(crate::DatError::TruncatedChunk {
            offset: 0,
            needed: 4,
            available: body.len(),
        })?;
        let bones_end = 4 + header.count as usize * BONE_STRIDE;
        if bones_end > body.len() {
            return Err(crate::DatError::TruncatedChunk {
                offset: 0,
                needed: bones_end,
                available: body.len(),
            });
        }
        let mut bones = Vec::with_capacity(header.count as usize);
        for i in 0..header.count as usize {
            let off = 4 + i * BONE_STRIDE;
            let parent = body[off];
            let flags = body[off + 1];
            let f = |a: usize| -> f32 {
                f32::from_le_bytes([body[a], body[a + 1], body[a + 2], body[a + 3]])
            };
            let rot = [f(off + 2), f(off + 6), f(off + 10), f(off + 14)];
            let trans = [f(off + 18), f(off + 22), f(off + 26)];
            bones.push(Bone {
                parent,
                flags,
                rot,
                trans,
            });
        }
        Ok(Self { header, bones })
    }

    /// Compose each bone's bind-pose **world** transform as a row-
    /// major 4×4 matrix (suitable for direct use with glam's
    /// `Mat4::from_cols_array` after transposing, or as a plain
    /// `[[f32; 4]; 4]` consumer-side).
    ///
    /// Walks the parent chain with cached intermediate results so the
    /// cost is O(bones) rather than O(bones × depth). A bone whose
    /// `parent == 0xFF` or whose `parent == own_index` is treated as
    /// the root and gets its local transform as its world transform.
    ///
    /// The returned matrix `m` is constructed so that
    /// `world_point = m * local_point` with column-vector convention.
    /// We deliberately avoid pulling glam into ffxi-dat — the matrix
    /// is built and composed in plain f32 here; convert to glam in
    /// the consumer crate if convenient.
    /// Per-bone local-transform override for [`Self::pose_world`].
    /// `Some` replaces the bone's bind-time local; `None` keeps bind.
    /// Indexed by bone id (matches the skeleton's `bones` array).
    pub fn pose_world(&self, overrides: &[Option<BoneLocal>]) -> Vec<[[f32; 4]; 4]> {
        let n = self.bones.len();
        let mut out = vec![identity4(); n];
        for i in 0..n {
            let local = if let Some(ov) = overrides.get(i).and_then(|o| o.as_ref()) {
                mat4_from_quat_trans_scale(ov.rotation, ov.translation, ov.scale)
            } else {
                mat4_from_quat_trans(self.bones[i].rot, self.bones[i].trans)
            };
            let p = self.bones[i].parent as usize;
            let is_root = self.bones[i].parent == PARENT_ROOT || p == i || p >= n;
            out[i] = if is_root {
                local
            } else {
                mat4_mul(out[p], local)
            };
        }
        out
    }

    pub fn bind_pose_world(&self) -> Vec<[[f32; 4]; 4]> {
        let n = self.bones.len();
        let mut out = vec![identity4(); n];
        for i in 0..n {
            let local = mat4_from_quat_trans(self.bones[i].rot, self.bones[i].trans);
            let p = self.bones[i].parent as usize;
            let is_root = self.bones[i].parent == PARENT_ROOT || p == i || p >= n;
            out[i] = if is_root {
                local
            } else {
                mat4_mul(out[p], local)
            };
        }
        out
    }
}

/// 4x4 identity in row-major order. Public for tests + downstream
/// consumers that want to start from a known basis.
pub fn identity4() -> [[f32; 4]; 4] {
    let mut m = [[0.0f32; 4]; 4];
    m[0][0] = 1.0;
    m[1][1] = 1.0;
    m[2][2] = 1.0;
    m[3][3] = 1.0;
    m
}

/// One animated bone's local transform (rot + trans + scale).
/// Used by [`Skeleton::pose_world`] to override the bind-time local
/// for bones that an MO2 animation drives.
#[derive(Debug, Clone, Copy)]
pub struct BoneLocal {
    /// Quaternion in xyzw order.
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
    pub scale: [f32; 3],
}

/// Like [`mat4_from_quat_trans`] but multiplies each row by the
/// corresponding scale component. Scale is applied first (in bone-
/// local space) before the rotation/translation lift.
fn mat4_from_quat_trans_scale(q: [f32; 4], t: [f32; 3], s: [f32; 3]) -> [[f32; 4]; 4] {
    let mut m = mat4_from_quat_trans(q, t);
    for row in m.iter_mut().take(3) {
        row[0] *= s[0];
        row[1] *= s[1];
        row[2] *= s[2];
    }
    m
}

/// Build a row-major 4x4 from a quaternion (x,y,z,w) and a
/// translation. Standard formula; pure to ease testing.
fn mat4_from_quat_trans(q: [f32; 4], t: [f32; 3]) -> [[f32; 4]; 4] {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let xx = x * x;
    let yy = y * y;
    let zz = z * z;
    let xy = x * y;
    let xz = x * z;
    let yz = y * z;
    let wx = w * x;
    let wy = w * y;
    let wz = w * z;
    [
        [
            1.0 - 2.0 * (yy + zz),
            2.0 * (xy - wz),
            2.0 * (xz + wy),
            t[0],
        ],
        [
            2.0 * (xy + wz),
            1.0 - 2.0 * (xx + zz),
            2.0 * (yz - wx),
            t[1],
        ],
        [
            2.0 * (xz - wy),
            2.0 * (yz + wx),
            1.0 - 2.0 * (xx + yy),
            t[2],
        ],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn mat4_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut o = [[0.0f32; 4]; 4];
    for r in 0..4 {
        for c in 0..4 {
            let mut s = 0.0f32;
            for k in 0..4 {
                s += a[r][k] * b[k][c];
            }
            o[r][c] = s;
        }
    }
    o
}

/// Transform a 3D point by a row-major 4x4 (column-vector convention).
/// Provided as a convenience so consumers don't have to roll the
/// math themselves when baking vertex positions.
pub fn mat4_transform_point(m: [[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
        m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
        m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
    ]
}

/// Transform a 3D direction (e.g., a vertex normal) by the rotation
/// part of a row-major 4x4 — translation column is intentionally
/// dropped. Result is **not** re-normalized; if the matrix has scale
/// the caller should normalize.
pub fn mat4_transform_dir(m: [[f32; 4]; 4], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_parses_pad_and_count_in_correct_halves() {
        // bytes 0..4 LE: pad=0x0000, count=0x005E (94).
        let buf = [0x00, 0x00, 0x5E, 0x00, 0, 0, 0, 0];
        let h = BoneHeader::parse(&buf).unwrap();
        assert_eq!(h.pad, 0x0000);
        assert_eq!(h.count, 0x005E);
    }

    #[test]
    fn skeleton_parse_single_identity_bone() {
        // pad=0, count=1, one bone: parent=0xFF, flags=0,
        // rot=(0,0,0,1) identity quat, trans=(1,2,3).
        let mut buf = vec![0, 0, 1, 0];
        buf.push(0xFF); // parent
        buf.push(0x00); // flags
        for f in [0.0f32, 0.0, 0.0, 1.0, 1.0, 2.0, 3.0] {
            buf.extend_from_slice(&f.to_le_bytes());
        }
        let s = Skeleton::parse(&buf).unwrap();
        assert_eq!(s.header.count, 1);
        assert_eq!(s.bones.len(), 1);
        assert_eq!(s.bones[0].parent, 0xFF);
        assert_eq!(s.bones[0].rot, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(s.bones[0].trans, [1.0, 2.0, 3.0]);

        let world = s.bind_pose_world();
        assert_eq!(world.len(), 1);
        // Identity rotation → world translation matches local.
        assert!((world[0][0][3] - 1.0).abs() < 1e-6);
        assert!((world[0][1][3] - 2.0).abs() < 1e-6);
        assert!((world[0][2][3] - 3.0).abs() < 1e-6);
    }

    #[test]
    fn bind_pose_world_composes_parent_chain() {
        // Two-bone chain: bone[0] translated (10,0,0), bone[1]
        // parented to 0, locally translated (0,5,0). World pos of
        // bone[1] should be (10, 5, 0).
        let mut buf = vec![0, 0, 2, 0];
        // bone 0: parent=0xFF (root), identity quat, trans=(10,0,0)
        buf.push(0xFF);
        buf.push(0);
        for f in [0.0f32, 0.0, 0.0, 1.0, 10.0, 0.0, 0.0] {
            buf.extend_from_slice(&f.to_le_bytes());
        }
        // bone 1: parent=0, identity quat, trans=(0,5,0)
        buf.push(0);
        buf.push(0);
        for f in [0.0f32, 0.0, 0.0, 1.0, 0.0, 5.0, 0.0] {
            buf.extend_from_slice(&f.to_le_bytes());
        }
        let s = Skeleton::parse(&buf).unwrap();
        let world = s.bind_pose_world();
        assert!((world[1][0][3] - 10.0).abs() < 1e-6);
        assert!((world[1][1][3] - 5.0).abs() < 1e-6);
        assert!((world[1][2][3] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn truncated_body_errors() {
        // Header says 5 bones, body only has room for 1.
        let mut buf = vec![0, 0, 5, 0];
        buf.extend_from_slice(&[0u8; BONE_STRIDE]);
        assert!(Skeleton::parse(&buf).is_err());
    }

    #[test]
    fn mat4_transform_point_applies_rotation_and_translation() {
        // 180° around Y: (1,0,0) → (-1,0,0); plus translate (10,0,0)
        // → world (9, 0, 0).
        let rot_y_180 = [0.0f32, 1.0, 0.0, 0.0]; // x,y,z,w
        let m = mat4_from_quat_trans(rot_y_180, [10.0, 0.0, 0.0]);
        let p = mat4_transform_point(m, [1.0, 0.0, 0.0]);
        assert!((p[0] - 9.0).abs() < 1e-5, "got {:?}", p);
        assert!(p[1].abs() < 1e-5);
        assert!(p[2].abs() < 1e-5);
    }
}
