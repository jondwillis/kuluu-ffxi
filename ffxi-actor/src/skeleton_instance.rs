//! Per-joint world pose — port of `xim/resource/SkeletonInstance.kt::animate`.
//!
//! Produces one world-space `Mat4` per joint that the skinned shader multiplies
//! directly. FFXI skinned-mesh vertices are authored in bone-local space, so
//! there is NO inverse-bind matrix: `world[joint]` IS the bone matrix.
//!
//! XIM tracks a *decomposed* transform per joint — rotation `r`, translation
//! `t`, scale `s` — and only collapses to a matrix at the end via
//! `JointTransform.toMat4() = copyFrom(r).translateDirect(t).scaleInPlace(s)`,
//! i.e. upper-left = `R * diag(s)`, translation column = `t`. In glam that is
//! exactly [`glam::Mat4::from_scale_rotation_translation`] `(s, r, t)`.
//!
//! XIM stores `r` as a rotation *matrix*; here it is a [`Quat`]. The two are
//! equivalent because every XIM op on `r` is a pure rotation:
//!   * `parentR.multiply(localRotMat, out)`  == `parent_q * local_q`
//!   * `r.transform(v)` (rotate a vector)     == `q * v`
//!
//! Local rotation per joint is `anim.rotation * bind.rotation` — XIM's
//! `Quaternion.multiplyAndStore(anim, bind, out)` is the Hamilton product with
//! the animation quaternion FIRST, which is `anim_q * bind_q` in glam.

use ffxi_dat::skel::{Joint, Skeleton};
use ffxi_dat::skel_anim::KeyFrameTransform;
use glam::{Mat4, Quat, Vec3};

/// Root-joint placement, mirroring the values XIM reads off the `Actor`.
#[derive(Debug, Clone, Copy)]
pub struct RootTransform {
    /// `actor.displayFacingDir` — yaw applied as `rotateY`.
    pub facing_dir: f32,
    /// `actor.displayFacingSkew` — applied as `rotateZ` only when slope-oriented.
    pub skew: f32,
    /// `actorModel.getFootInfoDefinition().movementType.slopeOriented`.
    pub slope_oriented: bool,
    /// `actor.getScale()` — applied to the whole skeleton via the root scale.
    pub scale: Vec3,
}

impl RootTransform {
    /// Identity placement: facing 0, no skew, unit scale.
    pub fn identity() -> Self {
        RootTransform {
            facing_dir: 0.0,
            skew: 0.0,
            slope_oriented: false,
            scale: Vec3::ONE,
        }
    }
}

/// Mount-attach inputs for the joint-2 special case — XIM
/// `applyMountAttachTransform`. The mount's skeleton-joint world position
/// (`mountSkeleton.getStandardJoint(48 + riderTypeIndex)` transformed by its
/// own world matrix) is precomputed by the caller, mirroring XIM reading the
/// already-animated mount skeleton.
#[derive(Debug, Clone, Copy)]
pub struct MountAttach {
    /// World position of the mount's rider-seat joint
    /// (`jointInstance.currentTransform.transform(jointRef.positionOffset)`).
    pub mount_joint_world: Vec3,
    /// `actor.displayFacingDir`.
    pub facing_dir: f32,
    /// `actorMount.getRiderRotation()`.
    pub rider_rotation: f32,
}

/// Decomposed world transform tracked per joint during `animate` — XIM's
/// private `JointTransform`.
#[derive(Clone, Copy)]
struct JointTransform {
    r: Quat,
    t: Vec3,
    s: Vec3,
}

impl JointTransform {
    /// XIM `JointTransform.toMat4()`.
    fn to_mat4(self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.s, self.r, self.t)
    }
}

/// XIM `Vector3f.rotate270`: `(x, y, z) -> (-z, y, x)`. Applied to the root
/// joint's local translation only (the root translation is not in
/// "skeleton-space"). Confirmed against `Vector3f.kt`:
///   `tx = x; this.x = -z; this.z = tx`  (y untouched).
fn rotate270(v: Vec3) -> Vec3 {
    Vec3::new(-v.z, v.y, v.x)
}

/// XIM `Quaternion(joint.definition.rotation)` — the bind rotation, xyzw.
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

/// Compute world pose matrices for every joint — XIM `SkeletonInstance.animate`.
///
/// `get_anim(joint_index)` supplies the per-joint animation transform
/// (XIM `actorModel.skeletonAnimationCoordinator.getJointTransform`); return
/// `None` to use bind only.
///
/// `parent_overrides` is a slice of `(child_joint, new_parent_joint)` pairs —
/// XIM's weapon-handle re-parenting. Pass `&[]` when there is no re-parenting.
///
/// The multi-pass loop mirrors XIM: because re-parenting can give a joint a
/// smaller index than its (effective) parent, we repeat passes until every
/// computable joint is computed. It terminates as long as the effective-parent
/// graph is a forest (no cycles), which FFXI skeletons satisfy.
///
/// When the actor is mounted, [`pose_world_mounted`] handles XIM's joint-2
/// special case; this entry point is equivalent to calling it with `None`.
pub fn pose_world(
    skeleton: &Skeleton,
    get_anim: impl Fn(usize) -> Option<KeyFrameTransform>,
    root: RootTransform,
    parent_overrides: &[(usize, usize)],
) -> Vec<Mat4> {
    pose_world_mounted(skeleton, get_anim, root, parent_overrides, None)
}

/// Like [`pose_world`], but with XIM's mount-attach special case: when an actor
/// is mounted, `updateCurrentJointTransform` short-circuits joint index 2 to
/// `applyMountAttachTransform` — it sets `jt.t = mountJointWorld + (0,-0.1,0)`,
/// `jt.r = rotateY(facingDir - PI/2 + riderRotation)`, and returns that
/// transform directly (skipping bind + animation). The joint-2 subtree then
/// inherits this from its parent in the usual propagation.
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

    // Resolve overrides to a per-child new-parent lookup.
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

            // Effective parent: override takes precedence over the bind parent.
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
                    // XIM: joint 2 of a mounted actor is attached to the mount's
                    // rider-seat joint; t/r are set directly and bind+anim skipped.
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

/// XIM `updateCurrentJointTransform` for both the root branch (index 0) and
/// non-root joints. `parent` is the already-computed parent transform, or
/// `None` for the root.
fn update_joint(
    skeleton: &Skeleton,
    index: usize,
    parent: Option<JointTransform>,
    get_anim: &impl Fn(usize) -> Option<KeyFrameTransform>,
    root: RootTransform,
) -> JointTransform {
    let joint = &skeleton.joints[index];
    let is_root = index == 0;

    // Seed r/s for the root from the actor transform; non-root inherits these
    // implicitly from the parent in the propagation step below.
    let mut jt_r = if is_root {
        // jt.r = rotateY(facing) [* rotateZ(skew) if slope-oriented]
        // multiplyInPlace post-multiplies, so this is rotY * rotZ.
        let mut r = Quat::from_rotation_y(root.facing_dir);
        if root.slope_oriented {
            r *= Quat::from_rotation_z(root.skew);
        }
        r
    } else {
        Quat::IDENTITY
    };
    let mut jt_s = if is_root { root.scale } else { Vec3::ONE };

    // Local transform = bind (+ animation).
    let mut translation = arr3(joint.translation);
    let mut rotation = bind_rotation(joint);
    let mut scale = Vec3::ONE;

    if let Some(anim) = get_anim(index) {
        translation += arr3(anim.translation);
        // multiplyAndStore(anim, bind, out) => anim * bind (animation first).
        rotation = Quat::from_xyzw(
            anim.rotation[0],
            anim.rotation[1],
            anim.rotation[2],
            anim.rotation[3],
        ) * rotation;
        // Root scale is ignored — the actor's scale is used instead.
        if !is_root {
            scale *= arr3(anim.scale);
        }
    }

    // Root translation isn't in skeleton-space.
    if is_root {
        translation = rotate270(translation);
    }

    match parent {
        None => {
            // Root / parent-less branch.
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

/// XIM `applyMountAttachTransform`: place joint 2 at the mount's rider-seat
/// joint. The XIM `JointTransform` is freshly default-constructed here, so
/// `s = (1,1,1)`, `t` accumulates from zero, and `r` rotates a fresh identity
/// about Y.
fn mount_attach_transform(m: MountAttach) -> JointTransform {
    JointTransform {
        r: Quat::from_rotation_y(m.facing_dir - std::f32::consts::FRAC_PI_2 + m.rider_rotation),
        t: m.mount_joint_world + Vec3::new(0.0, -0.1, 0.0),
        s: Vec3::ONE,
    }
}

/// XIM `updateCurrentJointTransformWithParentOverride`: copy the new parent's
/// (t, s, r), then replace scale with the animation scale only (the new
/// parent's scale is intentionally ignored).
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

/// XIM `getStandardJointPosition`: world position of a standard reference =
/// `world[ref.index] * ref.positionOffset`. Used for foot/head placement.
///
/// Returns `None` if the standard index is out of range or its referenced joint
/// index is out of range.
pub fn standard_joint_world_position(
    world: &[Mat4],
    skeleton: &Skeleton,
    standard_index: usize,
) -> Option<Vec3> {
    let reference = skeleton.references.get(standard_index)?;
    let mat = world.get(reference.index)?;
    Some(mat.transform_point3(arr3(reference.position_offset)))
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

    #[test]
    fn bind_only_two_bone_chain_translation_adds() {
        // Root at origin (the root's local translation is rotate270'd in XIM,
        // so keep it zero to isolate the parent->child accumulation). A 3-bone
        // chain whose child/grandchild locals are NOT rotate270'd must add up:
        // child world == root + child_local; grandchild == child + gc_local.
        let s = skel(vec![
            joint(None, [0.0, 0.0, 0.0]),    // root
            joint(Some(0), [5.0, 0.0, 0.0]), // child
            joint(Some(1), [2.0, 0.0, 0.0]), // grandchild
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
        // from_scale_rotation_translation(s, r, t) must equal the XIM
        // copyFrom(r).translateDirect(t).scaleInPlace(s) layout.
        let r = Quat::from_rotation_y(0.7);
        let t = Vec3::new(1.0, 2.0, 3.0);
        let s = Vec3::new(2.0, 3.0, 4.0);
        let jt = JointTransform { r, t, s };
        let m = jt.to_mat4();

        // Upper-left should be R * diag(s); translation column should be t.
        let expected = Mat4::from_scale_rotation_translation(s, r, t);
        assert!((m - expected).abs_diff_eq(Mat4::ZERO, 1e-6));
        // Translation column is exactly t (translateDirect, not multiplied).
        assert!(approx(m.col(3).truncate(), t, 1e-6));
    }

    #[test]
    fn root_anim_rotation_propagates_to_child() {
        // Apply a 90-deg-about-Y animation rotation to the root; the child at
        // local +X should swing to roughly -Z (glam from_rotation_y).
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
        // rotate +X by +90 about Y -> (0,0,-1)
        assert!(approx(child_t, Vec3::new(0.0, 0.0, -1.0), 1e-4));
    }

    #[test]
    fn root_facing_rotates_child_about_y() {
        // facing = 90 deg should rotate the child's world position about Y,
        // matching XIM's rotateY(displayFacingDir). Note the root local
        // translation is rotate270'd, so place the child translation, not the
        // root, to isolate the facing rotation.
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
        // Child local +X rotated +90 about Y -> (0,0,-1).
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
        // Faithful re-parenting layout: index 0 is the true root (XIM keys the
        // root branch on index==0, and the real root always has index 0). Joint
        // index 1's parent is index 2 — a HIGHER index — so the single-pass
        // index loop reaches the child (1) before its parent (2). The do/while
        // must run a second pass and still terminate.
        let s = skel(vec![
            joint(None, [0.0, 0.0, 0.0]),    // index 0, root at origin
            joint(Some(2), [5.0, 0.0, 0.0]), // index 1, child of the LATER joint 2
            joint(Some(0), [3.0, 0.0, 0.0]), // index 2, child of root
        ]);
        let world = pose_world(&s, |_| None, RootTransform::identity(), &[]);
        let p2 = world[2].transform_point3(Vec3::ZERO); // 3 from root
        let c1 = world[1].transform_point3(Vec3::ZERO); // p2 + 5
        assert!(approx(p2, Vec3::new(3.0, 0.0, 0.0), 1e-4));
        // child(1) world = parent(2) world + local(5,0,0) — proves the deferred
        // pass resolved correctly.
        assert!(approx(c1 - p2, Vec3::new(5.0, 0.0, 0.0), 1e-4));
    }

    #[test]
    fn parent_override_copies_new_parent_position() {
        // Three joints: 0 root, 1 a "hand" far away, 2 a "weapon handle" whose
        // bind parent is 0 but is re-parented onto 1. Its world position must
        // snap to joint 1's.
        let s = skel(vec![
            joint(None, [0.0, 0.0, 0.0]),    // 0 root
            joint(Some(0), [9.0, 0.0, 0.0]), // 1 hand
            joint(Some(0), [1.0, 0.0, 0.0]), // 2 handle, bind parent 0
        ]);
        let world = pose_world(&s, |_| None, RootTransform::identity(), &[(2, 1)]);
        let hand = world[1].transform_point3(Vec3::ZERO);
        let handle = world[2].transform_point3(Vec3::ZERO);
        assert!(approx(hand, Vec3::new(9.0, 0.0, 0.0), 1e-4));
        // Re-parented joint copies the new parent's t (and r), so same point.
        assert!(approx(handle, hand, 1e-4));
    }

    #[test]
    fn mount_attach_overrides_joint_2_and_propagates() {
        // 0 root, 1 spine, 2 the mount-attach joint (child of 1), 3 child of 2.
        // When mounted, joint 2 must snap to the mount-seat world position plus
        // (0,-0.1,0), ignoring its bind translation; joint 3 inherits from it.
        let s = skel(vec![
            joint(None, [0.0, 0.0, 0.0]),    // 0 root
            joint(Some(0), [0.0, 1.0, 0.0]), // 1 spine
            joint(Some(1), [9.0, 9.0, 9.0]), // 2 mount-attach (bind ignored)
            joint(Some(2), [0.0, 0.0, 2.0]), // 3 child of 2
        ]);
        let mount = MountAttach {
            mount_joint_world: Vec3::new(3.0, 4.0, 5.0),
            facing_dir: 0.0,
            rider_rotation: 0.0,
        };
        let world = pose_world_mounted(&s, |_| None, RootTransform::identity(), &[], Some(mount));
        let j2 = world[2].transform_point3(Vec3::ZERO);
        // seat + (0,-0.1,0); bind (9,9,9) discarded.
        assert!(approx(j2, Vec3::new(3.0, 3.9, 5.0), 1e-4), "j2 = {j2}");
        // r = rotateY(-PI/2): child 3 at local +Z (len 2) rotates to -X.
        let j3 = world[3].transform_point3(Vec3::ZERO);
        assert!(
            approx(j3 - j2, Vec3::new(-2.0, 0.0, 0.0), 1e-4),
            "j3-j2 = {}",
            j3 - j2
        );
    }

    #[test]
    fn no_mount_leaves_joint_2_using_bind() {
        // Without a mount, joint 2 uses bind translation normally.
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
        // (x,y,z) -> (-z, y, x)
        assert_eq!(
            rotate270(Vec3::new(1.0, 2.0, 3.0)),
            Vec3::new(-3.0, 2.0, 1.0)
        );
    }

    #[test]
    fn standard_joint_world_position_applies_offset() {
        let mut s = skel(vec![
            joint(None, [0.0, 0.0, 0.0]),
            joint(Some(0), [10.0, 0.0, 0.0]),
        ]);
        // reference index 0 points at joint 1 with offset (0,1,0)
        s.references.push(JointReference {
            index: 1,
            unk_v0: [0.0, 0.0, 0.0],
            position_offset: [0.0, 1.0, 0.0],
        });
        let world = pose_world(&s, |_| None, RootTransform::identity(), &[]);
        let p = standard_joint_world_position(&world, &s, 0).unwrap();
        assert!(approx(p, Vec3::new(10.0, 1.0, 0.0), 1e-4));
        // out-of-range standard index
        assert!(standard_joint_world_position(&world, &s, 5).is_none());
    }
}
