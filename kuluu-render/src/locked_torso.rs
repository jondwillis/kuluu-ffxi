//! Locked on, a side step keeps the head and chest on the target through the look-at, inside its limits.
//!
//! The walker aims the body at the locked target on every moving tick (`kuluu/src/view_native/input.rs`, the
//! `locked_bearing` re-aim), but the side-step clips turn the chest off that aim. The look-at measures from the
//! root's forward, so a target the walker keeps dead ahead of the root reads as "straight on" and the bend does
//! nothing while the chest and head face somewhere else. [`bend_reference`] hands the bend the chest's real facing
//! instead, so the authored neck and chest records turn the head and shoulders back toward the target, each stopping
//! at its own ellipse limit. The side-step clips themselves play as authored.

use bevy::math::{Mat4, Quat, Vec3};
use ffxi_actor::skeleton_instance::{pose_world, RootTransform};
use ffxi_dat::skel::{standard_position, Skeleton};

/// The body's forward in pose space before the actor's facing is applied (every humanoid rig faces `+X`).
const POSE_FORWARD: Vec3 = ffxi_actor::look_bend::POSE_FORWARD;

/// What the look-at needs from one rig to read where its chest faces.
#[derive(Debug, Clone, PartialEq)]
pub struct UpperBody {
    /// The chest reference's joint.
    pub chest: usize,
    /// The chest joint's own axis that points along the body's forward in the bind pose.
    pub chest_forward: Vec3,
}

impl UpperBody {
    /// `None` for a rig without a chest reference.
    pub fn of(skeleton: &Skeleton) -> Option<Self> {
        let chest = skeleton.reference_at(standard_position::CHEST)?.index;
        let bind = pose_world(skeleton, |_| None, RootTransform::identity(), &[]);
        let (_, chest_bind, _) = bind.get(chest)?.to_scale_rotation_translation();
        Some(Self {
            chest,
            chest_forward: chest_bind.inverse() * POSE_FORWARD,
        })
    }

    /// The chest's facing projected on the ground plane (pose space is `-Y` up).
    pub fn chest_facing(&self, pose: &[Mat4]) -> Option<Vec3> {
        let (_, rotation, _) = pose.get(self.chest)?.to_scale_rotation_translation();
        ground(rotation * self.chest_forward)
    }
}

/// The forward the look-at bend measures from: the body's aim turned toward where the chest really faces, by
/// `weight` of the way. At weight 0 it is the aim itself, which is what the bend has always been given.
pub fn bend_reference(aim: Vec3, chest_facing: Option<Vec3>, weight: f32) -> Vec3 {
    let (Some(aim_ground), Some(facing)) = (ground(aim), chest_facing) else {
        return aim;
    };
    if weight <= 0.0 {
        return aim;
    }
    Quat::from_rotation_y(signed_yaw(aim_ground, facing) * weight.min(1.0)) * aim
}

/// The rotation about `+Y` that carries ground direction `from` onto `to`, in `(-PI, PI]`.
fn signed_yaw(from: Vec3, to: Vec3) -> f32 {
    from.cross(to).y.atan2(from.dot(to))
}

fn ground(v: Vec3) -> Option<Vec3> {
    Vec3::new(v.x, 0.0, v.z).try_normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_turns_from_the_aim_to_the_chest_by_the_weight() {
        let chest = Some(Vec3::new(0.0, 0.0, -1.0));
        assert_eq!(bend_reference(POSE_FORWARD, chest, 0.0), POSE_FORWARD);
        let full = bend_reference(POSE_FORWARD, chest, 1.0);
        assert!(
            full.abs_diff_eq(Vec3::new(0.0, 0.0, -1.0), 1e-5),
            "{full:?}"
        );
        let half = bend_reference(POSE_FORWARD, chest, 0.5);
        assert!(
            half.angle_between(POSE_FORWARD) > 0.7 && half.angle_between(POSE_FORWARD) < 0.9,
            "{half:?}"
        );
        assert_eq!(bend_reference(POSE_FORWARD, None, 1.0), POSE_FORWARD);
    }
}
