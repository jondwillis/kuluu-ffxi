#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use bevy::prelude::*;
use ffxi_dat::bone::{BoneLocal, Skeleton};

use crate::skinned_ffxi_material::FfxiSkinnedMaterial;

#[derive(Component)]
pub struct FfxiActor {
    pub skeleton: Arc<Skeleton>,

    pub dat_id: u32,

    pub pivot: Entity,

    pub materials: Vec<Handle<FfxiSkinnedMaterial>>,

    pub min_local_y: f32,

    pub max_local_y: f32,
}

pub fn pc_pivot_rotation() -> Quat {
    Quat::from_rotation_y(std::f32::consts::FRAC_PI_2) * Quat::from_rotation_x(std::f32::consts::PI)
}

pub fn pc_pivot_translation() -> Vec3 {
    Vec3::Y
}

#[inline]
pub fn row_major_to_mat4(m: &[[f32; 4]; 4]) -> Mat4 {
    Mat4::from_cols_array(&[
        m[0][0], m[1][0], m[2][0], m[3][0], m[0][1], m[1][1], m[2][1], m[3][1], m[0][2], m[1][2],
        m[2][2], m[3][2], m[0][3], m[1][3], m[2][3], m[3][3],
    ])
}

pub fn eval_bind_pose(skeleton: &Skeleton) -> Vec<Mat4> {
    eval_pose(skeleton, &[])
}

pub fn eval_pose(skeleton: &Skeleton, overrides: &[Option<BoneLocal>]) -> Vec<Mat4> {
    let n = skeleton.bones.len();
    let mut ov: Vec<Option<BoneLocal>> = overrides.to_vec();
    if ov.len() < n {
        ov.resize(n, None);
    }
    if n > 0 {
        let q = skeleton.bones[0].rot;
        ov[0] = Some(BoneLocal {
            rotation: [-q[0], -q[1], -q[2], q[3]],
            translation: [0.0, 0.0, 0.0],
            scale: [1.0, 1.0, 1.0],
        });
    }
    skeleton
        .pose_world_anim(&ov)
        .iter()
        .map(row_major_to_mat4)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_major_to_mat4_preserves_transform() {
        let q = [0.0f32, 1.0, 0.0, 0.0];
        let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
        let row_major = [
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y - w * z),
                2.0 * (x * z + w * y),
                10.0,
            ],
            [
                2.0 * (x * y + w * z),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z - w * x),
                0.0,
            ],
            [
                2.0 * (x * z - w * y),
                2.0 * (y * z + w * x),
                1.0 - 2.0 * (x * x + y * y),
                0.0,
            ],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let m = row_major_to_mat4(&row_major);
        let p = m.transform_point3(Vec3::new(1.0, 0.0, 0.0));
        assert!((p.x - 9.0).abs() < 1e-4, "got {p:?}");
        assert!(p.y.abs() < 1e-4);
        assert!(p.z.abs() < 1e-4);
    }
}
