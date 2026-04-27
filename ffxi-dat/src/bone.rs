use crate::Result;

pub const PARENT_ROOT: u8 = 0xFF;

pub const BONE_STRIDE: usize = 30;

#[derive(Debug, Clone, Copy)]
pub struct BoneHeader {
    pub pad: u16,

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

#[derive(Debug, Clone, Copy)]
pub struct Bone {
    pub parent: u8,

    pub flags: u8,

    pub rot: [f32; 4],

    pub trans: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct Skeleton {
    pub header: BoneHeader,
    pub bones: Vec<Bone>,
}

impl Skeleton {
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

    pub fn pose_world_anim(&self, overrides: &[Option<BoneLocal>]) -> Vec<[[f32; 4]; 4]> {
        let n = self.bones.len();
        let mut out = vec![identity4(); n];
        for i in 0..n {
            let b = &self.bones[i];
            let local = if let Some(ov) = overrides.get(i).and_then(|o| o.as_ref()) {
                let t = [
                    b.trans[0] + ov.translation[0],
                    b.trans[1] + ov.translation[1],
                    b.trans[2] + ov.translation[2],
                ];
                let r = quat_mul(ov.rotation, b.rot);
                mat4_from_quat_trans_scale(r, t, ov.scale)
            } else {
                mat4_from_quat_trans(b.rot, b.trans)
            };
            let p = b.parent as usize;
            let is_root = b.parent == PARENT_ROOT || p == i || p >= n;
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

pub fn identity4() -> [[f32; 4]; 4] {
    let mut m = [[0.0f32; 4]; 4];
    m[0][0] = 1.0;
    m[1][1] = 1.0;
    m[2][2] = 1.0;
    m[3][3] = 1.0;
    m
}

#[derive(Debug, Clone, Copy)]
pub struct BoneLocal {
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
    pub scale: [f32; 3],
}

fn mat4_from_quat_trans_scale(q: [f32; 4], t: [f32; 3], s: [f32; 3]) -> [[f32; 4]; 4] {
    let mut m = mat4_from_quat_trans(q, t);
    for row in m.iter_mut().take(3) {
        row[0] *= s[0];
        row[1] *= s[1];
        row[2] *= s[2];
    }
    m
}

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

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

pub fn mat4_transform_point(m: [[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
        m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
        m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
    ]
}

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
        let buf = [0x00, 0x00, 0x5E, 0x00, 0, 0, 0, 0];
        let h = BoneHeader::parse(&buf).unwrap();
        assert_eq!(h.pad, 0x0000);
        assert_eq!(h.count, 0x005E);
    }

    #[test]
    fn skeleton_parse_single_identity_bone() {
        let mut buf = vec![0, 0, 1, 0];
        buf.push(0xFF);
        buf.push(0x00);
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

        assert!((world[0][0][3] - 1.0).abs() < 1e-6);
        assert!((world[0][1][3] - 2.0).abs() < 1e-6);
        assert!((world[0][2][3] - 3.0).abs() < 1e-6);
    }

    #[test]
    fn bind_pose_world_composes_parent_chain() {
        let mut buf = vec![0, 0, 2, 0];

        buf.push(0xFF);
        buf.push(0);
        for f in [0.0f32, 0.0, 0.0, 1.0, 10.0, 0.0, 0.0] {
            buf.extend_from_slice(&f.to_le_bytes());
        }

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
        let mut buf = vec![0, 0, 5, 0];
        buf.extend_from_slice(&[0u8; BONE_STRIDE]);
        assert!(Skeleton::parse(&buf).is_err());
    }

    #[test]
    fn mat4_transform_point_applies_rotation_and_translation() {
        let rot_y_180 = [0.0f32, 1.0, 0.0, 0.0];
        let m = mat4_from_quat_trans(rot_y_180, [10.0, 0.0, 0.0]);
        let p = mat4_transform_point(m, [1.0, 0.0, 0.0]);
        assert!((p[0] - 9.0).abs() < 1e-5, "got {:?}", p);
        assert!(p[1].abs() < 1e-5);
        assert!(p[2].abs() < 1e-5);
    }
}
