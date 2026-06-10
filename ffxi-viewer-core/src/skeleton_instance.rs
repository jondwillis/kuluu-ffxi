//! CPU skeleton evaluation for the FFXI-faithful character path — the
//! Rust counterpart to FFXI's `SkeletonInstance` (cross-referenced
//! against `research/xim`). Produces the per-bone
//! **world-pose** matrices the skinned shader multiplies vertices by
//! (no inverse bind; vertices are authored bone-local).
//!
//! The matrices come straight from [`ffxi_dat::bone::Skeleton`]: bind
//! pose via [`Skeleton::bind_pose_world`], animated pose via
//! [`Skeleton::pose_world`] with per-bone overrides. The only conversion
//! here is row-major (`ffxi-dat`) → column-major glam [`Mat4`] for upload.
//!
//! Orientation: these matrices stay in FFXI skeleton space. The actor's
//! pivot entity carries the single FFXI→Bevy basis change + heading +
//! feet-on-ground (see [`pc_pivot_rotation`] and `spawn_xim_actor`). This
//! mirrors the known-good CPU bake, whose total skeleton→Bevy rotation is
//! `Q_y(π/2) · Q_x(π)` — the per-vertex `[p0, p2, -p1]` swap (`R_x(-π/2)`)
//! folded into the pivot's `Q_y(π/2) · Q_x(-π/2)`.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::storage::ShaderStorageBuffer;
use ffxi_dat::bone::{BoneLocal, Skeleton};

use crate::skinned_ffxi_material::FfxiSkinnedMaterial;

/// Parent-side state for one actor rendered via [`crate::skinned_ffxi_material`].
/// All of an actor's mesh-group entities share this actor's single bone
/// buffer; the per-frame tick rewrites the buffer from the evaluated pose.
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
    /// Per-actor bone-matrix storage buffer bound at the material's
    /// binding 3. Length = `skeleton.bones.len()`.
    pub bone_buffer: Handle<ShaderStorageBuffer>,
    /// Pivot entity between the wire entity and the mesh groups. Carries
    /// the FFXI→Bevy basis change and the feet-on-ground translation, so
    /// neither has to live in the bone matrices.
    pub pivot: Entity,
    /// One material asset per spawned mesh group (an actor's equipment
    /// slots each contribute groups). The per-frame tick stamps the live
    /// lighting onto each; all reference the same `bone_buffer`.
    pub materials: Vec<Handle<FfxiSkinnedMaterial>>,
    /// Running min local-Y across loaded slots (feet anchor).
    pub min_local_y: f32,
    /// Running max local-Y across loaded slots (head extent → nameplate).
    pub max_local_y: f32,
}

/// FFXI skeleton-space → Bevy rotation for PC rigs. Equal to the known-
/// good CPU bake's total (`Q_y(π/2) · Q_x(π)`): stand the character up
/// (X) then yaw 90° (Y) so forward lands on Bevy -Z, matching
/// `scene::heading_to_quat`.
pub fn pc_pivot_rotation() -> Quat {
    Quat::from_rotation_y(std::f32::consts::FRAC_PI_2) * Quat::from_rotation_x(std::f32::consts::PI)
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

/// Bind-pose world matrices for upload (no animation).
pub fn eval_bind_pose(skeleton: &Skeleton) -> Vec<Mat4> {
    skeleton
        .bind_pose_world()
        .iter()
        .map(row_major_to_mat4)
        .collect()
}

/// Animated world matrices for upload. `overrides[i] = Some(local)`
/// replaces bone `i`'s bind-time local transform with the animation
/// sample; `None` keeps the bind pose. Length should match the skeleton.
pub fn eval_pose(skeleton: &Skeleton, overrides: &[Option<BoneLocal>]) -> Vec<Mat4> {
    skeleton
        .pose_world(overrides)
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
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y), 10.0],
            [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x), 0.0],
            [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y), 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let m = row_major_to_mat4(&row_major);
        let p = m.transform_point3(Vec3::new(1.0, 0.0, 0.0));
        assert!((p.x - 9.0).abs() < 1e-4, "got {p:?}");
        assert!(p.y.abs() < 1e-4);
        assert!(p.z.abs() < 1e-4);
    }
}
