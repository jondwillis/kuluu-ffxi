use ffxi_dat::skel::{Joint, Skeleton};
use ffxi_dat::skel_anim::KeyFrameTransform;
use glam::{Mat4, Quat, Vec3};

#[derive(Debug, Clone, Copy)]
pub struct RootTransform {
    pub facing_dir: f32,

    pub skew: f32,

    pub slope_oriented: bool,

    pub scale: Vec3,
}

impl RootTransform {
    pub fn identity() -> Self {
        RootTransform {
            facing_dir: 0.0,
            skew: 0.0,
            slope_oriented: false,
            scale: Vec3::ONE,
        }
    }
}

/// The joint a rider is pinned to when [`MountAttach`] applies — the hip, which
/// every FFXI skeleton files third and which the rest of the body composes from
/// (research/xim resource/SkeletonInstance.kt, updateCurrentJointTransform).
pub const HIP_JOINT: usize = 2;

#[derive(Debug, Clone, Copy)]
pub struct MountAttach {
    pub mount_joint_world: Vec3,

    pub facing_dir: f32,

    pub rider_rotation: f32,
}

#[derive(Clone, Copy)]
struct JointTransform {
    r: Quat,
    t: Vec3,
    s: Vec3,
}

impl JointTransform {
    fn to_mat4(self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.s, self.r, self.t)
    }
}

fn rotate270(v: Vec3) -> Vec3 {
    Vec3::new(-v.z, v.y, v.x)
}

fn bind_rotation(joint: &Joint) -> Quat {
    Quat::from_xyzw(
        joint.rotation[0],
        joint.rotation[1],
        joint.rotation[2],
        joint.rotation[3],
    )
}

fn arr3(a: [f32; 3]) -> Vec3 {
    Vec3::new(a[0], a[1], a[2])
}

/// Reusable working buffers for [`pose_world_into`]: a per-caller scratch that
/// removes the per-call `Vec` allocations on the hot per-actor-per-frame path.
#[derive(Default)]
pub struct PoseScratch {
    jt: Vec<Option<JointTransform>>,
    override_parent: Vec<Option<usize>>,
}

pub fn pose_world(
    skeleton: &Skeleton,
    get_anim: impl Fn(usize) -> Option<KeyFrameTransform>,
    root: RootTransform,
    parent_overrides: &[(usize, usize)],
) -> Vec<Mat4> {
    pose_world_mounted(skeleton, get_anim, root, parent_overrides, None)
}

pub fn pose_world_mounted(
    skeleton: &Skeleton,
    get_anim: impl Fn(usize) -> Option<KeyFrameTransform>,
    root: RootTransform,
    parent_overrides: &[(usize, usize)],
    mount: Option<MountAttach>,
) -> Vec<Mat4> {
    let mut out = Vec::new();
    pose_world_mounted_into(
        &mut out,
        &mut PoseScratch::default(),
        skeleton,
        get_anim,
        root,
        parent_overrides,
        mount,
    );
    out
}

pub fn pose_world_into(
    out: &mut Vec<Mat4>,
    scratch: &mut PoseScratch,
    skeleton: &Skeleton,
    get_anim: impl Fn(usize) -> Option<KeyFrameTransform>,
    root: RootTransform,
    parent_overrides: &[(usize, usize)],
) {
    pose_world_mounted_into(
        out,
        scratch,
        skeleton,
        get_anim,
        root,
        parent_overrides,
        None,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn pose_world_mounted_into(
    out: &mut Vec<Mat4>,
    scratch: &mut PoseScratch,
    skeleton: &Skeleton,
    get_anim: impl Fn(usize) -> Option<KeyFrameTransform>,
    root: RootTransform,
    parent_overrides: &[(usize, usize)],
    mount: Option<MountAttach>,
) {
    let n = skeleton.joints.len();
    out.clear();
    out.resize(n, Mat4::IDENTITY);
    let jt = &mut scratch.jt;
    jt.clear();
    jt.resize(n, None);

    let override_parent = &mut scratch.override_parent;
    override_parent.clear();
    override_parent.resize(n, None);
    for &(child, new_parent) in parent_overrides {
        if child < n {
            override_parent[child] = Some(new_parent);
        }
    }

    loop {
        let mut has_missing_parent = false;

        for i in 0..n {
            if jt[i].is_some() {
                continue;
            }

            let effective_parent = override_parent[i].or(skeleton.joints[i].parent);

            if let Some(p) = effective_parent {
                if p >= n || jt[p].is_none() {
                    has_missing_parent = true;
                    continue;
                }
            }

            let transform = if override_parent[i].is_some() {
                let parent = jt[override_parent[i].unwrap()].unwrap();
                update_with_parent_override(parent, get_anim(i))
            } else if i == HIP_JOINT {
                if let Some(m) = mount {
                    mount_attach_transform(m)
                } else {
                    update_joint(
                        skeleton,
                        i,
                        effective_parent.and_then(|p| jt[p]),
                        &get_anim,
                        root,
                    )
                }
            } else {
                update_joint(
                    skeleton,
                    i,
                    effective_parent.and_then(|p| jt[p]),
                    &get_anim,
                    root,
                )
            };

            out[i] = transform.to_mat4();
            jt[i] = Some(transform);
        }

        if !has_missing_parent {
            break;
        }
    }
}

fn update_joint(
    skeleton: &Skeleton,
    index: usize,
    parent: Option<JointTransform>,
    get_anim: &impl Fn(usize) -> Option<KeyFrameTransform>,
    root: RootTransform,
) -> JointTransform {
    let joint = &skeleton.joints[index];
    let is_root = index == 0;

    let mut jt_r = if is_root {
        let mut r = Quat::from_rotation_y(root.facing_dir);
        if root.slope_oriented {
            r *= Quat::from_rotation_z(root.skew);
        }
        r
    } else {
        Quat::IDENTITY
    };
    let mut jt_s = if is_root { root.scale } else { Vec3::ONE };

    let mut translation = arr3(joint.translation);
    let mut rotation = bind_rotation(joint);
    let mut scale = Vec3::ONE;

    if let Some(anim) = get_anim(index) {
        let anim_t = arr3(anim.translation);
        translation += if is_root {
            Vec3::new(anim_t.x, 0.0, anim_t.z)
        } else {
            anim_t
        };

        rotation = Quat::from_xyzw(
            anim.rotation[0],
            anim.rotation[1],
            anim.rotation[2],
            anim.rotation[3],
        ) * rotation;

        if !is_root {
            scale *= arr3(anim.scale);
        }
    }

    if is_root {
        translation = rotate270(translation);
    }

    match parent {
        None => {
            let t = jt_r * (jt_s * translation);
            jt_s *= scale;
            jt_r *= rotation;
            JointTransform {
                r: jt_r,
                t,
                s: jt_s,
            }
        }
        Some(p) => {
            let t = p.t + p.r * (p.s * translation);
            let s = p.s * scale;
            let r = p.r * rotation;
            JointTransform { r, t, s }
        }
    }
}

fn mount_attach_transform(m: MountAttach) -> JointTransform {
    JointTransform {
        r: Quat::from_rotation_y(m.facing_dir - std::f32::consts::FRAC_PI_2 + m.rider_rotation),
        t: m.mount_joint_world,
        s: Vec3::ONE,
    }
}

fn update_with_parent_override(
    parent: JointTransform,
    anim: Option<KeyFrameTransform>,
) -> JointTransform {
    let scale = match anim {
        Some(a) => Vec3::ONE * arr3(a.scale),
        None => Vec3::ONE,
    };
    JointTransform {
        r: parent.r,
        t: parent.t,
        s: scale,
    }
}

// FFXiMain.dll horizonxi-2023 RVA 0x2B9A0 / retail-2026-09 RVA 0x2C400: locator 2 bypasses
// animated bones and facing; scale is in DAT axes (Model.Scale.x,z,y). Corroborated by
// research/XIClient/src/XIClient/source/World/Model/ModelInstance.cpp GetLocatorWorldPosition.
pub fn nameplate_locator_offset(skeleton: &Skeleton, dat_axis_scale: Vec3) -> Option<Vec3> {
    let reference = skeleton.reference_at(ffxi_dat::skel::standard_position::ABOVE_HEAD)?;
    let offset = arr3(reference.position_offset) * dat_axis_scale;
    offset.is_finite().then_some(offset)
}

pub fn standard_joint_world_position(
    world: &[Mat4],
    skeleton: &Skeleton,
    standard_index: usize,
) -> Option<Vec3> {
    let reference = skeleton.references.get(standard_index)?;
    let mat = world.get(reference.index)?;
    Some(mat.transform_point3(arr3(reference.position_offset)))
}

// research/xim SkeletonInstance.kt getStandardJointExtended — references 49..51 are
// selectors, not placed points: retail files joint 0 with a zero offset there, while references
// 13..20 ring the actor at torso height. The selector stands for whichever of those eight sits
// nearest the other actor of the attachment, which is what puts a melee hit spark on the struck
// side of the victim. How 50 and 51 differ from 49 is not established upstream either.
//
// The DAT facts are pinned to the install by the real-DAT tests below. The nearest-of-eight RULE
// on top of them is xim's, which research/AGENTS.md ranks as pointing at where to look rather than
// as the answer, and it is unverified against the retail client binary or a retail observation.
pub const NEAREST_JOINT_REFERENCES: std::ops::RangeInclusive<usize> = 49..=51;
pub const RING_JOINT_REFERENCES: std::ops::RangeInclusive<usize> = 13..=20;

/// Position, in the actor's own pose frame, of the joint reference a particle
/// generator attaches to. `toward` is the other actor of the attachment in that
/// same frame; without it a 49..51 selector cannot resolve and falls through to
/// the table, which yields the actor root.
pub fn attach_joint_position(
    world: &[Mat4],
    skeleton: &Skeleton,
    reference: usize,
    toward: Option<Vec3>,
) -> Option<Vec3> {
    let reference = match toward {
        Some(toward) if NEAREST_JOINT_REFERENCES.contains(&reference) => {
            nearest_ring_reference(world, skeleton, toward).unwrap_or(reference)
        }
        _ => reference,
    };
    standard_joint_world_position(world, skeleton, reference)
}

fn nearest_ring_reference(world: &[Mat4], skeleton: &Skeleton, toward: Vec3) -> Option<usize> {
    RING_JOINT_REFERENCES
        .filter_map(|reference| {
            let pos = standard_joint_world_position(world, skeleton, reference)?;
            Some((reference, pos.distance_squared(toward)))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(reference, _)| reference)
}

pub fn find_head_neck(skeleton: &Skeleton) -> Option<(usize, usize)> {
    let n = skeleton.joints.len();
    let lh = skeleton.references.get(126)?.index;
    let rh = skeleton.references.get(127)?.index;
    if lh >= n || rh >= n {
        return None;
    }

    let chain = |start: usize| -> Vec<usize> {
        let mut out = Vec::new();
        let mut j = Some(start);
        while let Some(i) = j {
            if out.contains(&i) {
                break;
            }
            out.push(i);
            j = skeleton.joints[i].parent;
        }
        out
    };
    let lh_chain = chain(lh);
    let rh_chain = chain(rh);
    let lh_set: std::collections::HashSet<usize> = lh_chain.iter().copied().collect();
    let rh_set: std::collections::HashSet<usize> = rh_chain.iter().copied().collect();

    let chest = *rh_chain.iter().find(|j| lh_set.contains(j))?;

    let neck = (0..n).find(|&i| {
        skeleton.joints[i].parent == Some(chest) && !lh_set.contains(&i) && !rh_set.contains(&i)
    })?;

    // Non-PC skeletons can expose hand references (126/127) that resolve near
    // the root, putting `chest` at the hips so the subtree spans the whole body
    // — rotating that for head-look would tilt the entire actor. Require a tip.
    let subtree = neck_subtree(skeleton, neck);
    if subtree.len() * 2 > n {
        return None;
    }

    let mut child_count = vec![0usize; n];
    for jt in &skeleton.joints {
        if let Some(p) = jt.parent {
            if p < n {
                child_count[p] += 1;
            }
        }
    }
    let head = subtree
        .into_iter()
        .max_by_key(|&i| child_count[i])
        .unwrap_or(neck);

    Some((neck, head))
}

pub fn neck_subtree(skeleton: &Skeleton, neck: usize) -> Vec<usize> {
    let n = skeleton.joints.len();
    let mut out = vec![neck];
    let mut i = 0;
    while i < out.len() {
        let cur = out[i];
        for c in 0..n {
            if skeleton.joints[c].parent == Some(cur) {
                out.push(c);
            }
        }
        i += 1;
    }
    out
}

pub fn apply_head_look(pose: &mut [Mat4], neck: usize, subtree: &[usize], rot: Quat) {
    let Some(neck_mat) = pose.get(neck).copied() else {
        return;
    };
    let pivot = neck_mat.w_axis.truncate();
    let about_pivot =
        Mat4::from_translation(pivot) * Mat4::from_quat(rot) * Mat4::from_translation(-pivot);
    for &j in subtree {
        if let Some(m) = pose.get_mut(j) {
            *m = about_pivot * *m;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::datid::DatId;
    use ffxi_dat::skel::{Joint, JointReference, Skeleton};

    const IDENTITY_QUAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    fn joint(parent: Option<usize>, translation: [f32; 3]) -> Joint {
        Joint {
            rotation: IDENTITY_QUAT,
            translation,
            parent,
        }
    }

    fn skel(joints: Vec<Joint>) -> Skeleton {
        Skeleton {
            id: DatId::from_str("0000"),
            joints,
            references: Vec::new(),
            bounding_boxes: Vec::new(),
        }
    }

    fn approx(a: Vec3, b: Vec3, eps: f32) -> bool {
        (a - b).length() < eps
    }

    fn jref(index: usize) -> JointReference {
        JointReference {
            index,
            unk_v0: [0.0; 3],
            position_offset: [0.0; 3],
        }
    }

    #[test]
    fn find_head_neck_isolates_neck_and_face_hub() {
        let mut s = skel(vec![
            joint(None, [0.0; 3]),
            joint(Some(0), [0.0; 3]),
            joint(Some(1), [0.0; 3]),
            joint(Some(2), [0.0; 3]),
            joint(Some(3), [0.0; 3]),
            joint(Some(4), [0.0; 3]),
            joint(Some(4), [0.0; 3]),
            joint(Some(2), [0.0; 3]),
            joint(Some(7), [0.0; 3]),
            joint(Some(2), [0.0; 3]),
            joint(Some(9), [0.0; 3]),
        ]);

        s.references = (0..128)
            .map(|i| match i {
                126 => jref(10),
                127 => jref(8),
                _ => jref(0),
            })
            .collect();

        assert_eq!(find_head_neck(&s), Some((3, 4)));
        let mut sub = neck_subtree(&s, 3);
        sub.sort_unstable();
        assert_eq!(sub, vec![3, 4, 5, 6]);
    }

    #[test]
    fn find_head_neck_none_without_hand_references() {
        let s = skel(vec![joint(None, [0.0; 3]), joint(Some(0), [0.0; 3])]);
        assert_eq!(find_head_neck(&s), None);
    }

    #[test]
    fn find_head_neck_rejects_whole_body_subtree() {
        // root, hips(1), a long chain 2..8 under the hips, and hands 9/10 off
        // the hips: chest=1, neck=2, whose subtree is 7 of 11 joints (> half).
        let mut s = skel(vec![
            joint(None, [0.0; 3]),
            joint(Some(0), [0.0; 3]),
            joint(Some(1), [0.0; 3]),
            joint(Some(2), [0.0; 3]),
            joint(Some(3), [0.0; 3]),
            joint(Some(4), [0.0; 3]),
            joint(Some(5), [0.0; 3]),
            joint(Some(6), [0.0; 3]),
            joint(Some(7), [0.0; 3]),
            joint(Some(1), [0.0; 3]),
            joint(Some(1), [0.0; 3]),
        ]);
        s.references = (0..128)
            .map(|i| match i {
                126 => jref(9),
                127 => jref(10),
                _ => jref(0),
            })
            .collect();
        assert_eq!(find_head_neck(&s), None);
    }

    #[test]
    fn apply_head_look_rotates_subtree_about_neck_pivot() {
        let neck = 0usize;
        let mut pose = vec![
            Mat4::from_translation(Vec3::new(1.0, 0.0, 0.0)),
            Mat4::from_translation(Vec3::new(1.0, 0.0, 1.0)),
        ];
        apply_head_look(
            &mut pose,
            neck,
            &[0, 1],
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
        );
        let head_t = pose[1].w_axis.truncate();
        assert!(
            approx(head_t, Vec3::new(2.0, 0.0, 0.0), 1e-4),
            "got {head_t:?}"
        );

        assert!(approx(
            pose[0].w_axis.truncate(),
            Vec3::new(1.0, 0.0, 0.0),
            1e-4
        ));
    }

    #[test]
    fn bind_only_two_bone_chain_translation_adds() {
        let s = skel(vec![
            joint(None, [0.0, 0.0, 0.0]),
            joint(Some(0), [5.0, 0.0, 0.0]),
            joint(Some(1), [2.0, 0.0, 0.0]),
        ]);
        let world = pose_world(&s, |_| None, RootTransform::identity(), &[]);

        let root_t = world[0].transform_point3(Vec3::ZERO);
        let child_t = world[1].transform_point3(Vec3::ZERO);
        let gc_t = world[2].transform_point3(Vec3::ZERO);
        assert!(approx(root_t, Vec3::new(0.0, 0.0, 0.0), 1e-4));
        assert!(approx(child_t, Vec3::new(5.0, 0.0, 0.0), 1e-4));
        assert!(approx(gc_t, Vec3::new(7.0, 0.0, 0.0), 1e-4));
    }

    #[test]
    fn to_mat4_equals_decomposed_srt() {
        let r = Quat::from_rotation_y(0.7);
        let t = Vec3::new(1.0, 2.0, 3.0);
        let s = Vec3::new(2.0, 3.0, 4.0);
        let jt = JointTransform { r, t, s };
        let m = jt.to_mat4();

        let expected = Mat4::from_scale_rotation_translation(s, r, t);
        assert!((m - expected).abs_diff_eq(Mat4::ZERO, 1e-6));

        assert!(approx(m.col(3).truncate(), t, 1e-6));
    }

    #[test]
    fn root_anim_rotation_propagates_to_child() {
        let s = skel(vec![
            joint(None, [0.0, 0.0, 0.0]),
            joint(Some(0), [1.0, 0.0, 0.0]),
        ]);
        let yaw = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let anim = move |i: usize| {
            if i == 0 {
                Some(KeyFrameTransform {
                    rotation: [yaw.x, yaw.y, yaw.z, yaw.w],
                    translation: [0.0, 0.0, 0.0],
                    scale: [1.0, 1.0, 1.0],
                })
            } else {
                None
            }
        };
        let world = pose_world(&s, anim, RootTransform::identity(), &[]);
        let child_t = world[1].transform_point3(Vec3::ZERO);

        assert!(approx(child_t, Vec3::new(0.0, 0.0, -1.0), 1e-4));
    }

    #[test]
    fn root_facing_rotates_child_about_y() {
        let s = skel(vec![
            joint(None, [0.0, 0.0, 0.0]),
            joint(Some(0), [1.0, 0.0, 0.0]),
        ]);
        let root = RootTransform {
            facing_dir: std::f32::consts::FRAC_PI_2,
            skew: 0.0,
            slope_oriented: false,
            scale: Vec3::ONE,
        };
        let world = pose_world(&s, |_| None, root, &[]);
        let child_t = world[1].transform_point3(Vec3::ZERO);

        assert!(approx(child_t, Vec3::new(0.0, 0.0, -1.0), 1e-4));
    }

    #[test]
    fn root_scale_scales_child_position() {
        let s = skel(vec![
            joint(None, [0.0, 0.0, 0.0]),
            joint(Some(0), [2.0, 0.0, 0.0]),
        ]);
        let root = RootTransform {
            facing_dir: 0.0,
            skew: 0.0,
            slope_oriented: false,
            scale: Vec3::splat(3.0),
        };
        let world = pose_world(&s, |_| None, root, &[]);
        let child_t = world[1].transform_point3(Vec3::ZERO);
        assert!(approx(child_t, Vec3::new(6.0, 0.0, 0.0), 1e-4));
    }

    #[test]
    fn multi_pass_terminates_with_child_index_below_parent() {
        let s = skel(vec![
            joint(None, [0.0, 0.0, 0.0]),
            joint(Some(2), [5.0, 0.0, 0.0]),
            joint(Some(0), [3.0, 0.0, 0.0]),
        ]);
        let world = pose_world(&s, |_| None, RootTransform::identity(), &[]);
        let p2 = world[2].transform_point3(Vec3::ZERO);
        let c1 = world[1].transform_point3(Vec3::ZERO);
        assert!(approx(p2, Vec3::new(3.0, 0.0, 0.0), 1e-4));

        assert!(approx(c1 - p2, Vec3::new(5.0, 0.0, 0.0), 1e-4));
    }

    #[test]
    fn parent_override_copies_new_parent_position() {
        let s = skel(vec![
            joint(None, [0.0, 0.0, 0.0]),
            joint(Some(0), [9.0, 0.0, 0.0]),
            joint(Some(0), [1.0, 0.0, 0.0]),
        ]);
        let world = pose_world(&s, |_| None, RootTransform::identity(), &[(2, 1)]);
        let hand = world[1].transform_point3(Vec3::ZERO);
        let handle = world[2].transform_point3(Vec3::ZERO);
        assert!(approx(hand, Vec3::new(9.0, 0.0, 0.0), 1e-4));

        assert!(approx(handle, hand, 1e-4));
    }

    #[test]
    fn mount_attach_overrides_joint_2_and_propagates() {
        let s = skel(vec![
            joint(None, [0.0, 0.0, 0.0]),
            joint(Some(0), [0.0, 1.0, 0.0]),
            joint(Some(1), [9.0, 9.0, 9.0]),
            joint(Some(2), [0.0, 0.0, 2.0]),
        ]);
        let mount = MountAttach {
            mount_joint_world: Vec3::new(3.0, 4.0, 5.0),
            facing_dir: 0.0,
            rider_rotation: 0.0,
        };
        let world = pose_world_mounted(&s, |_| None, RootTransform::identity(), &[], Some(mount));
        let j2 = world[2].transform_point3(Vec3::ZERO);

        assert!(approx(j2, Vec3::new(3.0, 4.0, 5.0), 1e-4), "j2 = {j2}");

        let j3 = world[3].transform_point3(Vec3::ZERO);
        assert!(
            approx(j3 - j2, Vec3::new(-2.0, 0.0, 0.0), 1e-4),
            "j3-j2 = {}",
            j3 - j2
        );
    }

    #[test]
    fn no_mount_leaves_joint_2_using_bind() {
        let s = skel(vec![
            joint(None, [0.0, 0.0, 0.0]),
            joint(Some(0), [0.0, 0.0, 0.0]),
            joint(Some(1), [4.0, 0.0, 0.0]),
        ]);
        let world = pose_world_mounted(&s, |_| None, RootTransform::identity(), &[], None);
        let j2 = world[2].transform_point3(Vec3::ZERO);
        assert!(approx(j2, Vec3::new(4.0, 0.0, 0.0), 1e-4));
    }

    #[test]
    fn pose_world_into_with_reused_scratch_matches_fresh() {
        let big = skel(vec![
            joint(None, [0.0, 0.0, 0.0]),
            joint(Some(0), [5.0, 0.0, 0.0]),
            joint(Some(1), [2.0, 0.0, 0.0]),
        ]);
        let small = skel(vec![
            joint(None, [0.0, 0.0, 0.0]),
            joint(Some(0), [1.0, 2.0, 3.0]),
        ]);

        let mut out = Vec::new();
        let mut scratch = PoseScratch::default();
        for s in [&big, &small, &big] {
            pose_world_into(
                &mut out,
                &mut scratch,
                s,
                |_| None,
                RootTransform::identity(),
                &[(1, 0)],
            );
            let fresh = pose_world(s, |_| None, RootTransform::identity(), &[(1, 0)]);
            assert_eq!(out, fresh, "reused scratch must not leak prior pose state");
        }
    }

    #[test]
    fn rotate270_axis_and_sign() {
        assert_eq!(
            rotate270(Vec3::new(1.0, 2.0, 3.0)),
            Vec3::new(-3.0, 2.0, 1.0)
        );
    }

    #[test]
    fn root_anim_vertical_translation_is_clamped() {
        let s = skel(vec![joint(None, [0.0, 0.0, 0.0])]);
        let anim = |i: usize| {
            (i == 0).then_some(KeyFrameTransform {
                rotation: [0.0, 0.0, 0.0, 1.0],
                translation: [1.0, 5.0, 2.0],
                scale: [1.0, 1.0, 1.0],
            })
        };
        let world = pose_world(&s, anim, RootTransform::identity(), &[]);
        let root_t = world[0].transform_point3(Vec3::ZERO);
        assert!(
            approx(root_t, Vec3::new(-2.0, 0.0, 1.0), 1e-4),
            "root should keep horizontal lunge but not rise: {root_t}",
        );
    }

    #[test]
    fn non_root_anim_vertical_translation_preserved() {
        let s = skel(vec![
            joint(None, [0.0, 0.0, 0.0]),
            joint(Some(0), [0.0, 0.0, 0.0]),
        ]);
        let anim = |i: usize| {
            (i == 1).then_some(KeyFrameTransform {
                rotation: [0.0, 0.0, 0.0, 1.0],
                translation: [0.0, 5.0, 0.0],
                scale: [1.0, 1.0, 1.0],
            })
        };
        let world = pose_world(&s, anim, RootTransform::identity(), &[]);
        let child_t = world[1].transform_point3(Vec3::ZERO);
        assert!(
            approx(child_t, Vec3::new(0.0, 5.0, 0.0), 1e-4),
            "child = {child_t}"
        );
    }

    #[test]
    fn nameplate_locator_uses_static_translation_without_bone_pose_or_facing() {
        use ffxi_dat::skel::standard_position::ABOVE_HEAD;
        let mut skeleton = skel(vec![joint(None, [30.0, -40.0, 50.0])]);
        skeleton.references.resize(
            ABOVE_HEAD + 1,
            JointReference {
                index: 0,
                unk_v0: [0.0; 3],
                position_offset: [1.0, -4.0, 5.0],
            },
        );
        let expected = Vec3::new(2.0, -12.0, 20.0);
        let scale = Vec3::new(2.0, 3.0, 4.0);
        for facing_dir in [0.0, 1.0, 2.0] {
            let world = pose_world(
                &skeleton,
                |_| {
                    Some(KeyFrameTransform {
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        translation: [100.0, 200.0, 300.0],
                        scale: [2.0, 2.0, 2.0],
                    })
                },
                RootTransform {
                    facing_dir,
                    scale,
                    ..RootTransform::identity()
                },
                &[],
            );
            assert_ne!(
                standard_joint_world_position(&world, &skeleton, ABOVE_HEAD),
                Some(expected)
            );
            assert_eq!(nameplate_locator_offset(&skeleton, scale), Some(expected));
        }
        skeleton.references[ABOVE_HEAD].index = usize::MAX;
        assert_eq!(nameplate_locator_offset(&skeleton, scale), Some(expected));
    }

    #[test]
    fn nameplate_locator_missing_or_nonfinite_is_unavailable() {
        use ffxi_dat::skel::standard_position::ABOVE_HEAD;
        let mut skeleton = skel(vec![]);
        assert_eq!(nameplate_locator_offset(&skeleton, Vec3::ONE), None);
        skeleton.references.resize(
            ABOVE_HEAD + 1,
            JointReference {
                index: 0,
                unk_v0: [0.0; 3],
                position_offset: [0.0, f32::NAN, 0.0],
            },
        );
        assert_eq!(nameplate_locator_offset(&skeleton, Vec3::ONE), None);
    }

    #[test]
    fn standard_joint_world_position_applies_offset() {
        let mut s = skel(vec![
            joint(None, [0.0, 0.0, 0.0]),
            joint(Some(0), [10.0, 0.0, 0.0]),
        ]);

        s.references.push(JointReference {
            index: 1,
            unk_v0: [0.0, 0.0, 0.0],
            position_offset: [0.0, 1.0, 0.0],
        });
        let world = pose_world(&s, |_| None, RootTransform::identity(), &[]);
        let p = standard_joint_world_position(&world, &s, 0).unwrap();
        assert!(approx(p, Vec3::new(10.0, 1.0, 0.0), 1e-4));

        assert!(standard_joint_world_position(&world, &s, 5).is_none());
    }

    const RING_HEIGHT: f32 = -1.1;
    const REFERENCE_TABLE_LEN: usize = 128;
    const ABOVE_HEAD_HEIGHT: f32 = -1.81;

    /// Posed positions, not raw offsets, so the fixture root stays unrotated: transcribed from
    /// the retail HumeM skeleton (ROM/27/82.DAT directory `hm_s`, skeleton `hum_`); references
    /// 13..20 sit on joint 0 around a torso-sized, front/back-asymmetric ellipse at
    /// `RING_HEIGHT`, 49..53 carry a zero offset. The real-DAT test
    /// `real_dat_retail_skeleton_resolves_the_nearest_joint_selector_onto_its_ring` holds this
    /// table to the install.
    const RETAIL_RING_POSITIONS: [[f32; 3]; 8] = [
        [0.24, RING_HEIGHT, 0.0],
        [0.2, RING_HEIGHT, -0.2],
        [0.0, RING_HEIGHT, -0.32],
        [-0.14, RING_HEIGHT, -0.14],
        [-0.17, RING_HEIGHT, 0.0],
        [-0.14, RING_HEIGHT, 0.14],
        [0.0, RING_HEIGHT, 0.32],
        [0.2, RING_HEIGHT, 0.2],
    ];

    fn ringed_skel() -> Skeleton {
        let mut s = skel(vec![joint(None, [0.0, 0.0, 0.0])]);
        s.references = (0..REFERENCE_TABLE_LEN).map(jref_at_root).collect();
        s.references[ffxi_dat::skel::standard_position::ABOVE_HEAD].position_offset =
            [0.0, ABOVE_HEAD_HEIGHT, 0.0];
        for (offset, reference) in RETAIL_RING_POSITIONS.iter().zip(RING_JOINT_REFERENCES) {
            s.references[reference].position_offset = *offset;
        }
        s
    }

    fn jref_at_root(_: usize) -> JointReference {
        JointReference {
            index: 0,
            unk_v0: [0.0; 3],
            position_offset: [0.0; 3],
        }
    }

    const OTHER_ACTOR_REACH: f32 = 20.0;

    fn ring_positions(world: &[Mat4], s: &Skeleton) -> Vec<Vec3> {
        RING_JOINT_REFERENCES
            .map(|r| standard_joint_world_position(world, s, r).unwrap())
            .collect()
    }

    /// Nearest-of-eight as a projection rather than a distance: with the other actor at R*dir,
    /// |p - R*dir|^2 = |p|^2 - 2R(p.dir) + R^2, so for R far outside the ring the nearest
    /// point is the one reaching furthest along `dir`.
    fn most_forward_ring_point(world: &[Mat4], s: &Skeleton, dir: Vec3) -> Vec3 {
        ring_positions(world, s)
            .into_iter()
            .max_by(|a, b| a.dot(dir).total_cmp(&b.dot(dir)))
            .unwrap()
    }

    /// The eight authored ring directions plus off-axis ones, so the selector is exercised
    /// where the answer is not simply the ring point it points at.
    fn other_actor_bearings() -> Vec<Vec3> {
        (0..16)
            .map(|i| {
                let a = i as f32 * std::f32::consts::TAU / 16.0;
                Vec3::new(a.cos(), 0.0, a.sin())
            })
            .collect()
    }

    fn assert_selector_tracks_the_other_actor(world: &[Mat4], s: &Skeleton) {
        for dir in other_actor_bearings() {
            let expected = most_forward_ring_point(world, s, dir);
            assert!(
                expected.dot(dir) > 0.0,
                "the struck side must face the other actor: {dir:?} -> {expected:?}"
            );
            for reference in NEAREST_JOINT_REFERENCES {
                assert_eq!(
                    attach_joint_position(world, s, reference, Some(dir * OTHER_ACTOR_REACH)),
                    Some(expected),
                    "reference {reference} toward {dir:?}"
                );
            }
        }
    }

    #[test]
    fn nearest_joint_reference_picks_the_ring_point_facing_the_other_actor() {
        let s = ringed_skel();
        let world = pose_world(&s, |_| None, RootTransform::identity(), &[]);
        assert_selector_tracks_the_other_actor(&world, &s);
    }

    #[test]
    fn nearest_joint_reference_without_a_second_actor_falls_back_to_the_root() {
        let s = ringed_skel();
        let world = pose_world(&s, |_| None, RootTransform::identity(), &[]);
        for reference in NEAREST_JOINT_REFERENCES {
            assert_eq!(
                attach_joint_position(&world, &s, reference, None),
                Some(Vec3::ZERO),
                "reference {reference}"
            );
        }
    }

    #[test]
    fn ordinary_joint_references_ignore_the_nearest_selector() {
        use ffxi_dat::skel::standard_position::ABOVE_HEAD;
        let s = ringed_skel();
        let world = pose_world(&s, |_| None, RootTransform::identity(), &[]);
        let toward = Vec3::new(0.0, 0.0, OTHER_ACTOR_REACH);
        assert_eq!(
            attach_joint_position(&world, &s, ABOVE_HEAD, Some(toward)),
            Some(Vec3::new(0.0, ABOVE_HEAD_HEIGHT, 0.0))
        );
    }

    #[test]
    fn attach_joint_ring_turns_with_the_actor() {
        let s = ringed_skel();
        for facing_dir in [0.0, 0.5, 1.0, 2.0, 3.0] {
            let world = pose_world(
                &s,
                |_| None,
                RootTransform {
                    facing_dir,
                    ..RootTransform::identity()
                },
                &[],
            );
            assert_selector_tracks_the_other_actor(&world, &s);
        }
    }

    /// The retail HumeM skeleton (ROM/27/82.DAT directory `hm_s`, skeleton `hum_`; the same
    /// skeleton kuluu-render's `melee_hit_chain_flattens_to_target_attached_sparks` walks): posed,
    /// it puts the j1=49 attach (the ROM/0/0.DAT hit sparks g010/g011/g013) on the ring point
    /// nearest the attacker, not the actor root that reference 49's own table entry resolves to.
    const HUME_M_SKELETON_FILE: u32 = 7072;

    #[test]
    fn real_dat_retail_skeleton_resolves_the_nearest_joint_selector_onto_its_ring() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let Ok(loc) = root.resolve(HUME_M_SKELETON_FILE) else {
            eprintln!("SKIP: skeleton file unresolvable");
            return;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            eprintln!("SKIP: skeleton file unreadable");
            return;
        };
        let dir = ffxi_dat::resource_dir::ResourceDir::from_bytes(bytes);
        let Some(s) = dir.collect_skeletons().into_iter().next() else {
            eprintln!("SKIP: no skeleton chunk");
            return;
        };
        let world = pose_world(&s, |_| None, RootTransform::identity(), &[]);

        for reference in NEAREST_JOINT_REFERENCES {
            assert_eq!(
                standard_joint_world_position(&world, &s, reference),
                Some(Vec3::ZERO),
                "retail reference {reference} is a selector, not a placed point"
            );
        }
        for (ring, pos) in RING_JOINT_REFERENCES.zip(ring_positions(&world, &s)) {
            assert!(
                pos.length() > 0.0,
                "retail ring reference {ring} is unplaced"
            );
        }
        let fixture = ringed_skel();
        let fixture_world = pose_world(&fixture, |_| None, RootTransform::identity(), &[]);
        for (retail, transcribed) in ring_positions(&world, &s)
            .into_iter()
            .zip(ring_positions(&fixture_world, &fixture))
        {
            assert!(
                approx(retail, transcribed, 1e-4),
                "the fixture above must stay a transcription of the retail ring: {retail:?} vs {transcribed:?}"
            );
        }

        assert_selector_tracks_the_other_actor(&world, &s);
    }
}
