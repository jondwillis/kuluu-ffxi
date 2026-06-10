//! CPU skeleton evaluation for the FFXI-faithful character path — the Rust
//! counterpart to FFXI/XIM's `SkeletonInstance`. Produces the per-bone
//! world-pose matrices the skinned shader multiplies (no inverse bind).
//! Matrices stay in FFXI skeleton space; the actor pivot carries the
//! FFXI→Bevy basis change + feet-on-ground.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use bevy::prelude::*;
use ffxi_dat::bone::{BoneLocal, Skeleton};

use crate::skinned_ffxi_material::FfxiSkinnedMaterial;

/// Parent-side state for one actor rendered via [`crate::skinned_ffxi_material`].
/// The per-frame tick evaluates the pose once and stamps it into every
/// mesh-group material's bone uniform.
#[derive(Component)]
pub struct FfxiActor {
    /// Raw skeleton, kept so the per-frame tick can recompose `pose_world`
    /// with animation overrides.
    pub skeleton: Arc<Skeleton>,
    /// Actor DAT id — the key into `BAKED_SKELETONS` / `IDLE_ANIMS` /
    /// `combat_stance`'s motion-DAT lookups. Drives clip selection in
    /// `tick_ffxi_actors` exactly as `SkinnedActor::dat_id` does for the
    /// Bevy path.
    pub dat_id: u32,
    /// Pivot entity between the wire entity and the mesh groups. Carries
    /// the FFXI→Bevy basis change and the feet-on-ground translation, so
    /// neither has to live in the bone matrices.
    pub pivot: Entity,
    /// One material asset per spawned mesh group (an actor's equipment
    /// slots each contribute groups). The per-frame tick stamps the live
    /// bone pose + lighting into each.
    pub materials: Vec<Handle<FfxiSkinnedMaterial>>,
    /// Running min local-Y across loaded slots (feet anchor).
    pub min_local_y: f32,
    /// Running max local-Y across loaded slots (head extent → nameplate).
    pub max_local_y: f32,
}

/// FFXI skeleton-space → Bevy rotation for PC rigs: `Q_y(π/2)·Q_x(π)`, the
/// known-good CPU-bake total. The `Q_x(π)` stands the rig up (FFXI→Bevy
/// up-axis); the `Q_y(π/2)` cancels bone 0's `(0,0.7071,0,-0.7071)` −90°-Y
/// root roll, which the faithful tick leaves un-unrolled (`pose[0]=None`).
/// Dropping the `Q_y(π/2)` yaws the whole actor 90° (legs read as missing).
pub fn pc_pivot_rotation() -> Quat {
    Quat::from_rotation_y(std::f32::consts::FRAC_PI_2) * Quat::from_rotation_x(std::f32::consts::PI)
}

pub fn pc_pivot_translation() -> Vec3 {
    Vec3::Y
}

/// Convert one `ffxi-dat` row-major 4×4 (column-vector convention,
/// `world = m · local`, translation at `m[r][3]`) into a glam [`Mat4`]
/// (column-major) representing the same transform.
#[inline]
pub fn row_major_to_mat4(m: &[[f32; 4]; 4]) -> Mat4 {
    // glam is column-major: column c holds (m[0][c], m[1][c], m[2][c], m[3][c]).
    Mat4::from_cols_array(&[
        m[0][0], m[1][0], m[2][0], m[3][0], // col 0
        m[0][1], m[1][1], m[2][1], m[3][1], // col 1
        m[0][2], m[1][2], m[2][2], m[3][2], // col 2
        m[0][3], m[1][3], m[2][3], m[3][3], // col 3 (translation)
    ])
}

/// Bind-pose world matrices for upload (no animation). Bone 0's root roll
/// is dropped, same as [`eval_pose`].
pub fn eval_bind_pose(skeleton: &Skeleton) -> Vec<Mat4> {
    eval_pose(skeleton, &[])
}

/// Animated world matrices for upload via [`Skeleton::pose_world_anim`].
pub fn eval_pose(skeleton: &Skeleton, overrides: &[Option<BoneLocal>]) -> Vec<Mat4> {
    // Cancel bone 0's SK2 root roll (the pivot carries the FFXI→Bevy basis
    // change; leaving the roll in yaws the actor ~90°). Since pose_world_anim
    // composes `anim·bind`, feeding the conjugate of bind makes bone 0's
    // rotation identity.
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
        // 180° about Y then translate (10,0,0): point (1,0,0) → (9,0,0).
        // Build the row-major matrix the way ffxi-dat does and confirm the
        // glam conversion transforms a point identically.
        let q = [0.0f32, 1.0, 0.0, 0.0]; // x,y,z,w (180° about Y)
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
