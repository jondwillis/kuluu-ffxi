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
    let n = skeleton.joints.len();
    let mut out = vec![Mat4::IDENTITY; n];
    let mut jt: Vec<Option<JointTransform>> = vec![None; n];

    let mut override_parent: Vec<Option<usize>> = vec![None; n];
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
            } else if i == 2 {
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

    out
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
        t: m.mount_joint_world + Vec3::new(0.0, -0.1, 0.0),
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

pub fn standard_joint_world_position(
    world: &[Mat4],
    skeleton: &Skeleton,
    standard_index: usize,
) -> Option<Vec3> {
    let reference = skeleton.references.get(standard_index)?;
    let mat = world.get(reference.index)?;
    Some(mat.transform_point3(arr3(reference.position_offset)))
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

        assert!(approx(j2, Vec3::new(3.0, 3.9, 5.0), 1e-4), "j2 = {j2}");

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
}
