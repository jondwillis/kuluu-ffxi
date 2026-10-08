//! Locked on, a side step keeps the upper body pointed at the target through the look-at, inside its limits.
//!
//! The walker aims the body at the locked target on every moving tick (`kuluu/src/view_native/input.rs`, the
//! `locked_bearing` re-aim), but the side-step clips turn the body off that aim: the lower set keys joint 2, which
//! the legs and the upper body both hang off, and the upper set twists the spine on top of it. Two things go wrong
//! with that on screen, and this module owns both answers:
//!
//! * The look-at measures from the root's forward, so a target the walker keeps dead ahead of the root reads as
//!   "straight on" and the bend does nothing while the chest and head face somewhere else. [`bend_reference`] hands
//!   the bend the chest's real facing instead, so the authored neck and chest records turn the head and shoulders
//!   back toward the target, each stopping at its own ellipse limit.
//! * On the A/D changeover a spine joint whose two side-step records sit past a right angle on either side blends
//!   the short way, which is through its back. [`merge_through_front`] takes the other arc for exactly that case, so
//!   the torso comes round through the target side.

use bevy::math::{Mat4, Quat, Vec3};
use ffxi_actor::animation::{interpolate_kf, SkeletonAnimationCoordinator};
use ffxi_actor::skeleton_instance::{pose_world, RootTransform};
use ffxi_dat::skel::{standard_position, Skeleton};
use ffxi_dat::skel_anim::KeyFrameTransform;

/// The body's forward in pose space before the actor's facing is applied (every humanoid rig faces `+X`).
const POSE_FORWARD: Vec3 = ffxi_actor::look_bend::POSE_FORWARD;

/// What the pass needs from one rig: how to read the chest's facing, and which joints turn the torso round.
#[derive(Debug, Clone, PartialEq)]
pub struct UpperBody {
    /// The chest reference's joint.
    pub chest: usize,
    /// The chest joint's own axis that points along the body's forward in the bind pose.
    pub chest_forward: Vec3,
    /// The neck reference's joint and its ancestors down to the joint the legs branch from, inclusive: every joint
    /// whose turn swings the torso round.
    pub spine: Vec<usize>,
}

impl UpperBody {
    /// `None` for a rig without chest, neck and feet references, or whose spine never meets the legs.
    pub fn of(skeleton: &Skeleton) -> Option<Self> {
        let chest = skeleton.reference_at(standard_position::CHEST)?.index;
        let neck = skeleton.reference_at(standard_position::NECK)?.index;
        let leg_ancestors: Vec<usize> =
            [standard_position::RIGHT_FOOT, standard_position::LEFT_FOOT]
                .into_iter()
                .filter_map(|slot| skeleton.reference_at(slot).map(|r| r.index))
                .flat_map(|foot| ancestors(skeleton, foot))
                .collect();
        let neck_chain = ancestors(skeleton, neck);
        let branch_at = neck_chain
            .iter()
            .position(|joint| leg_ancestors.contains(joint))?;
        let spine = neck_chain[..=branch_at].to_vec();

        let bind = pose_world(skeleton, |_| None, RootTransform::identity(), &[]);
        let (_, chest_bind, _) = bind.get(chest)?.to_scale_rotation_translation();
        Some(Self {
            chest,
            chest_forward: chest_bind.inverse() * POSE_FORWARD,
            spine,
        })
    }

    /// The chest's facing projected on the ground plane (pose space is `-Y` up).
    pub fn chest_facing(&self, pose: &[Mat4]) -> Option<Vec3> {
        let (_, rotation, _) = pose.get(self.chest)?.to_scale_rotation_translation();
        ground(rotation * self.chest_forward)
    }

    /// This frame's record for each spine joint whose owning layer is crossfading, merged along
    /// [`merge_through_front`]; every other joint keeps what the coordinator sampled.
    pub fn steered_spine(
        &self,
        coordinator: &SkeletonAnimationCoordinator,
    ) -> Vec<(usize, KeyFrameTransform)> {
        let mut steered = Vec::new();
        for &joint in &self.spine {
            let mask = coordinator.mask_at(joint);
            if !mask.writable() || !mask.accepts_blend_layers() {
                continue;
            }
            let Some(owner) = coordinator
                .animations
                .iter()
                .flatten()
                .find(|layer| layer.get_joint_transform(joint).is_some())
            else {
                continue;
            };
            let Some(transition) = owner.transition.as_ref() else {
                continue;
            };
            let (Some(outgoing), Some(incoming), t) = transition.sides(joint) else {
                continue;
            };
            let mut merged = interpolate_kf(&outgoing, &incoming, t);
            merged.rotation = merge_through_front(outgoing.rotation, incoming.rotation, t);
            steered.push((joint, merged));
        }
        steered
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

/// The layer merge (`FFXiMain.dll retail-2026-09` RVA 0x33220: copy at `t == 1`, otherwise the shortest arc and
/// `a·(1−t) + b·t` stored raw), except where the shortest arc passes further from the bind orientation than both
/// of its ends. That arc swings the joint round behind both poses it is blending between; the other arc comes
/// through the bind orientation, which for a spine joint is upright and facing forward. A pair that is not past a
/// right angle on either side never meets the condition, so it merges exactly as retail does.
pub fn merge_through_front(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    if t == 1.0 {
        return b;
    }
    let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let short = if dot < 0.0 { neg(b) } else { b };
    let long = neg(short);
    let behind_both_ends =
        nearness_to_bind(add(a, short)) < nearness_to_bind(a).min(nearness_to_bind(b));
    let through_bind = nearness_to_bind(add(a, long)) > nearness_to_bind(add(a, short));
    let other = if behind_both_ends && through_bind {
        long
    } else {
        short
    };
    let inv = 1.0 - t;
    [
        a[0] * inv + other[0] * t,
        a[1] * inv + other[1] * t,
        a[2] * inv + other[2] * t,
        a[3] * inv + other[3] * t,
    ]
}

/// `|w|` of the normalised quaternion: 1 at the bind orientation, 0 half a turn away. A sum with no length has no
/// orientation at all and counts as furthest.
fn nearness_to_bind(q: [f32; 4]) -> f32 {
    let length = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if length <= f32::EPSILON {
        0.0
    } else {
        q[3].abs() / length
    }
}

fn add(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
}

fn neg(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], -q[3]]
}

/// The rotation about `+Y` that carries ground direction `from` onto `to`, in `(-PI, PI]`.
fn signed_yaw(from: Vec3, to: Vec3) -> f32 {
    from.cross(to).y.atan2(from.dot(to))
}

fn ground(v: Vec3) -> Option<Vec3> {
    Vec3::new(v.x, 0.0, v.z).try_normalize()
}

/// `joint` first, then each parent up to the root.
fn ancestors(skeleton: &Skeleton, joint: usize) -> Vec<usize> {
    let mut chain = vec![joint];
    let mut current = skeleton.joints.get(joint).and_then(|j| j.parent);
    while let Some(parent) = current {
        if chain.contains(&parent) {
            break;
        }
        chain.push(parent);
        current = skeleton.joints.get(parent).and_then(|j| j.parent);
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::datid::DatId;
    use ffxi_dat::skel::{Joint, JointReference};

    const IDENTITY_QUAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    fn joint(parent: Option<usize>, translation: [f32; 3]) -> Joint {
        Joint {
            parent,
            rotation: IDENTITY_QUAT,
            translation,
        }
    }

    fn reference(index: usize) -> JointReference {
        JointReference {
            index,
            rotation: [0.0; 3],
            position_offset: [0.0; 3],
        }
    }

    /// root(0) -> hips(1) -> { spine(2) -> chest(3) -> neck(4), leg(5) -> foot(6) }.
    fn rig() -> Skeleton {
        let mut references = vec![reference(0); standard_position::LEFT_FOOT + 1];
        references[standard_position::NECK] = reference(4);
        references[standard_position::CHEST] = reference(3);
        references[standard_position::RIGHT_FOOT] = reference(6);
        references[standard_position::LEFT_FOOT] = reference(6);
        Skeleton {
            id: DatId::from_str("test"),
            joints: vec![
                joint(None, [0.0, 0.0, 0.0]),
                joint(Some(0), [0.0, -1.0, 0.0]),
                joint(Some(1), [0.0, -0.3, 0.0]),
                joint(Some(2), [0.0, -0.3, 0.0]),
                joint(Some(3), [0.0, -0.3, 0.0]),
                joint(Some(1), [0.0, 0.5, 0.0]),
                joint(Some(5), [0.0, 0.5, 0.0]),
            ],
            references,
            bounding_boxes: Vec::new(),
            look_at_limits: Vec::new(),
        }
    }

    fn yaw(deg: f32) -> [f32; 4] {
        let q = Quat::from_rotation_y(deg.to_radians());
        [q.x, q.y, q.z, q.w]
    }

    fn angle_of(q: [f32; 4]) -> f32 {
        Quat::from_xyzw(q[0], q[1], q[2], q[3])
            .normalize()
            .to_axis_angle()
            .1
            .to_degrees()
    }

    #[test]
    fn the_spine_runs_from_the_neck_down_to_where_the_legs_branch() {
        let upper = UpperBody::of(&rig()).expect("the rig has chest, neck and feet");
        assert_eq!(upper.chest, 3);
        assert_eq!(
            upper.spine,
            vec![4, 3, 2, 1],
            "the hips are where the leg chain meets the spine"
        );
    }

    /// Two side-step records 100 degrees either side of forward: the shortest arc runs through half a turn, the
    /// merge here runs through forward instead.
    #[test]
    fn a_pair_past_a_right_angle_each_side_comes_round_through_the_front() {
        let mid = merge_through_front(yaw(100.0), yaw(-100.0), 0.5);
        assert!(
            angle_of(mid).abs() < 1e-3,
            "mid-blend sits at {} deg from forward",
            angle_of(mid)
        );
        let retail = ffxi_actor::animation::merge_layer_rotation(yaw(100.0), yaw(-100.0), 0.5);
        assert!(
            (angle_of(retail) - 180.0).abs() < 1e-3,
            "the shortest arc is the one through the back ({} deg)",
            angle_of(retail)
        );
    }

    /// Anything the shortest arc already brings through the front, and anything that is not a turn past a right
    /// angle, merges exactly as retail's layer merge does.
    #[test]
    fn every_other_pair_merges_as_retail_does() {
        let pairs = [
            (yaw(45.0), yaw(-45.0)),
            (yaw(80.0), yaw(-80.0)),
            (yaw(100.0), yaw(-60.0)),
            (yaw(170.0), yaw(160.0)),
            (yaw(-30.0), [0.0, -0.3826834, 0.0, -0.9238795]),
        ];
        for (a, b) in pairs {
            for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
                assert_eq!(
                    merge_through_front(a, b, t),
                    ffxi_actor::animation::merge_layer_rotation(a, b, t),
                    "{a:?} -> {b:?} at {t}"
                );
            }
        }
    }

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
