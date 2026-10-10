use ffxi_dat::skel::{standard_position, LookAtLimit, Skeleton};
use glam::{Mat3, Mat4, Quat, Vec3};

use crate::skeleton_instance::composed_subtree;

/// A bone the model authored a look-at bend for: which joint to rotate, and the ellipse that limits it.
#[derive(Debug, Clone, Copy)]
pub struct LookBone {
    pub joint: usize,
    pub limit: LookAtLimit,
}

/// The bend applies at most two records — its loop count is 1 or 2 (`FFXiMain.dll retail-2026-09` writes the
/// same slot with `2` at RVA 0x2AE8B/0x2AE90 and with `1` at RVA 0x2AE96).
pub const BEND_RECORDS_MAX: usize = 2;

/// The reference slot both of the bend's shared vectors are measured from. Retail fetches it as the literal `4`
/// next to slot 3 (`FFXiMain.dll retail-2026-09`: `push 3` / `call 0x1002a750` at RVA 0x2AC74..0x2AC84 and
/// `push 4` / `call 0x1002a750` at RVA 0x2AC8E..0x2AC92). XIM names slot 4 `EID_LOOK_AT`; it is never a bone to
/// rotate — on shipped rigs it resolves to the root with an authored offset (hum_ `(0,-1.5,-1.8)`, which is what
/// makes it the anchor rather than a joint.
pub const BEND_ANCHOR_SLOT: usize = 4;

/// The value an authored limit is replaced with when it is exactly zero (`FFXiMain.dll retail-2026-09` RVA
/// 0x2B20A stores the raw `0x3a83126f`; the compare feeding it is `fld [ecx] / fcom dword [0x103295d8] (= 0) /
/// fnstsw ax / test ah,0x44 / jp` at RVA 0x2B1DB..0x2B1EF, mirrored for the other axis with `fcomp` at
/// RVA 0x2B1F9..0x2B20A). `jp` keeps the authored value for every comparison except equality, so a *negative*
/// limit reaches the aspect ratio unchanged. Two zero-guarded axes leave nothing to turn: the rebuilt rim point
/// collapses onto the aim axis (see `only_a_record_with_both_axes_zero_is_inert`).
const LIMIT_GUARD: f32 = 0.001;

/// The factor applied to both in-plane components when the projected point sits **at or behind** the basis plane
/// (`FFXiMain.dll retail-2026-09`: `fmul dword ptr [0x1032a3c8]` (= `100.0f`, `.rdata`) at RVA 0x2B26B and
/// RVA 0x2B278). That arm is selected by `fld [esp+0x1c] / fcomp dword [0x103295d8] / fnstsw ax / test ah,0x41 /
/// jp` at RVA 0x2B253..0x2B262, whose jump target (RVA 0x2B280) holds the divide — so ×100 is the arm with no
/// sensible perspective: the point is driven past the rim of its ellipse instead of divided.
const BEHIND_PROJECTION_FACTOR: f32 = 100.0;

/// The up hint retail hands the basis builder verbatim (`FFXiMain.dll retail-2026-09`: a local seeded `(0,1,0)` at
/// RVA 0x2B187..0x2B19F, consumed by the look-at builder RVA 0x2D8141). Kuluu's skeletons stand with `-Y` as up;
/// the sign does not matter here because the ellipse clamp is symmetric in both in-plane components, only the axis
/// does, and the axis is the same.
const BASIS_UP_HINT: Vec3 = Vec3::Y;

/// The actor's forward axis in facing-free pose space, measured from the real skeletons (every humanoid stands with
/// up `-Y` and faces `+X`). This is the axis each bend starts from and the one the ellipse is centred on, handed to
/// [`apply_look_bends`] directly: `pose`'s own nose, because kuluu composes heading outside it (on the entity
/// transform), so a look point measured against this axis has had that same heading removed once.
pub const POSE_FORWARD: Vec3 = Vec3::X;

/// The reference slots whose joints the two records rotate, in record order. Retail writes them as the byte pair
/// {3, 7} at `FFXiMain.dll retail-2026-09` RVA 0x2AC7A / 0x2AC7F, reads one back per record at RVA 0x2AFED and
/// resolves each through the skeleton's reference table at RVA 0x2A9B0 — the same table `Skeleton::references`
/// parses. XIM names slot 3 `EID_NECK` and slot 7 `EID_CHEST`, and in every shipped rig that authors both records
/// the slot-7 joint is the slot-3 joint's parent, which is how the shoulder share sits under the head turn.
const BEND_SLOTS: [usize; BEND_RECORDS_MAX] = [standard_position::NECK, standard_position::CHEST];

/// The frame a reference names. `FFXiMain.dll retail-2026-09` RVA 0x2A750 builds one per slot through RVA 0x2A780:
/// zero the rotation (RVA 0x279B0), rotate about X, then Y, then Z by the reference's three authored floats at
/// +2 / +6 / +0xA (RVA 0x27B80 / 0x27BD0 / 0x27C20 — radians), set the translation from its position offset (RVA
/// 0x27CF0 writing `+0x30/+0x34/+0x38`), multiply by that joint's node matrix at `[this+0x14] + 64·joint` (RVA
/// 0x27D10) and hand the result back. Those authored floats measure zero on every shipped player rig for slots 3,
/// 4 and 7 (`real_dat_hume_bends_its_neck_and_chest_joints` asserts it for Hume), so no shipping model
/// distinguishes the application order today — it is still applied in retail's order rather than dropped.
#[derive(Debug, Clone, Copy)]
pub struct AttachFrame {
    pub origin: Vec3,
    pub rotation: Quat,
}

/// The frame reference `slot` names, expressed in the same space as `pose`.
pub fn attach_frame(pose: &[Mat4], skeleton: &Skeleton, slot: usize) -> Option<AttachFrame> {
    let reference = skeleton.reference_at(slot)?;
    let joint = pose.get(reference.index).copied()?;
    let (_, joint_rotation, joint_origin) = joint.to_scale_rotation_translation();
    // The offset is stored as the local matrix's translation and the authored rotations are appended before it,
    // so the offset is placed by the joint rather than spun by them: `origin = joint · offset`, orientation =
    // `joint_rot · (Z·Y·X)` under glam's column-vector convention.
    let authored = Quat::from_rotation_z(reference.rotation[2])
        * Quat::from_rotation_y(reference.rotation[1])
        * Quat::from_rotation_x(reference.rotation[0]);
    Some(AttachFrame {
        origin: joint_origin + joint_rotation * Vec3::from(reference.position_offset),
        rotation: joint_rotation * authored,
    })
}

/// The basis `FFXiMain.dll retail-2026-09` RVA 0x2D8141 builds for one clamp call from origin `(0,0,0)`, an aim
/// vector and [`BASIS_UP_HINT`]: `d = normalize(at − origin)` (normalise thunk RVA 0x2D6491), `side =
/// normalize(up_hint × d)` (the three `fsub` pairs at RVA 0x2D8179..0x2D81B5) and then `up' = d × side`; those
/// three axes are stored as the matrix's columns, so "rotate a vector by it" (RVA 0x282C0) is exactly dotting the
/// vector against each axis. Getting back out is RVA 0x283C0 (invert) followed by RVA 0x28290 (transform).
#[derive(Debug, Clone, Copy)]
struct LookAtBasis {
    /// Rows are the axes: [`LookAtBasis::project`] reproduces `FFXiMain.dll retail-2026-09` RVA 0x282C0's three
    /// dot products, and an orthonormal basis inverts to its transpose — which is all the inverse at that build's
    /// RVA 0x283C0 amounts to here.
    into_axes: Mat3,
}

impl LookAtBasis {
    fn new(at: Vec3) -> Option<Self> {
        let d = at.try_normalize()?;
        let side = BASIS_UP_HINT.cross(d).try_normalize()?;
        let up = d.cross(side);
        Some(Self {
            into_axes: Mat3::from_cols(side, up, d).transpose(),
        })
    }

    fn project(&self, v: Vec3) -> Vec3 {
        self.into_axes * v
    }

    fn unproject(&self, v: Vec3) -> Vec3 {
        self.into_axes.transpose() * v
    }
}

/// Retail's ellipse clamp — `FFXiMain.dll retail-2026-09` RVA 0x2B140, transcribed against the bytes. It takes the
/// axis the ellipse is centred on (`forward`, which becomes the basis' depth axis), the direction to bound (`look`)
/// and one authored record, and returns the bounded direction back in `look`'s space plus whether the rim was
/// reached. Inside the ellipse `look` comes back as it went in; outside it comes back on the rim, which is what
/// caps the turn at `atan(xlim/scale)` across and `atan(ylim/scale)` up/down. Steps with RVAs:
///
/// 1. Build the basis from `forward`; project `look` into it (RVA 0x2B1BD → RVA 0x282C0) and double it (`push
///    0x40000000` at RVA 0x2B1C6 feeding the scale helper RVA 0x272B0). It cancels inside the ahead arm's divide
///    (`v.x·scale/v.z`) but not inside the other arm, where it scales both in-plane components against an
///    authored bound; kept verbatim because retail keeps it.
/// 2. Guard each semi-axis: an authored `0` becomes [`LIMIT_GUARD`] (RVA 0x2B1DB..0x2B212).
/// 3. Turn the ellipse into a circle by scaling ONE component by the axes' ratio — when `xlim > ylim`, scale
///    `v.y` (`[esp+0x18]` at RVA 0x2B22B..0x2B233) and bound at `xlim`; otherwise scale `v.x` (`[esp+0x14]` at RVA
///    0x2B247..0x2B24F) and bound at `ylim`. The arm is chosen by the compare of the two guarded axes at
///    RVA 0x2B212..0x2B21D, so the radius is always the wider semi-axis in units where the narrower one has been
///    stretched up to it.
/// 4. Project into the plane. Ahead (`v.z > 0`) take `k = 1.0f / v.z` (`.rdata 0x1032961c`, RVA 0x2B280..0x2B28A)
///    and multiply each component by `scale · k`; at or behind, multiply both components by `scale ×
///    BEHIND_PROJECTION_FACTOR` (RVA 0x2B264..0x2B278). Since `scale` multiplies the offsets while the bound stays
///    authored, a record of `(0.24, 0.16)` with `scale 0.5` bounds the turn near `atan(0.24/0.5)` across and
///    `atan(0.16/0.5)` up/down.
/// 5. Compare `hypot(u,w)` against that radius (RVA 0x2B2A0..0x2B2AE). Inside it returns the projected vector
///    unchanged — the copy at RVA 0x2B33F..0x2B371, which also writes `w = 1.0` and makes the function report not
///    clamped. On the rim: bearing from `fpatan` (RVA 0x2B2BD), rebuilt as `(cosθ·radius, sinθ·radius)` with the
///    depth slot written as the record's own `scale` (the raw `[ecx+8]` load/store at RVA 0x2B2DD / 0x2B2EB), then
///    step 3's ratio is undone on exactly the axis it stretched (`fld [esp+0xc] / fdiv [esp+8]` vs `fld [esp+0x10] /
///    fdiv [esp+8]`, selected by the compare against `ylim` at RVA 0x2B2E7..0x2B30C), then the vector goes back out
///    of the basis (RVA 0x2B312, RVA 0x2B31C) and is normalised in place (RVA 0x2B322 → RVA 0x274B0). Reconstructing
///    that undo gives a rim point of `(cosθ·xlim, sinθ·ylim, scale)` — an elliptical boundary, not a circular one.
pub fn clamp_bend_axis(forward: Vec3, look: Vec3, record: &LookAtLimit) -> Option<(Vec3, bool)> {
    let basis = LookAtBasis::new(forward)?;
    let mut v = basis.project(look);
    v *= 2.0;

    let xlim = if record.x_limit == 0.0 {
        LIMIT_GUARD
    } else {
        record.x_limit
    };
    let ylim = if record.y_limit == 0.0 {
        LIMIT_GUARD
    } else {
        record.y_limit
    };

    // The aspect ratio stays in scope because the rim rebuild undoes exactly the axis stretched here.
    let (radius, stretched_y) = if xlim > ylim {
        v.y *= xlim / ylim;
        (xlim, true)
    } else {
        v.x *= ylim / xlim;
        (ylim, false)
    };

    // Ahead of the basis plane each component is divided by its depth; at or behind it nothing divides, so both
    // components are driven past the rim instead.
    let (u, w) = if v.z > 0.0 {
        let per_depth = record.scale / v.z;
        (v.x * per_depth, v.y * per_depth)
    } else {
        let push = record.scale * BEHIND_PROJECTION_FACTOR;
        (v.x * push, v.y * push)
    };

    if (u * u + w * w).sqrt() <= radius {
        return Some((basis.unproject(v), false));
    }

    // `out`'s depth slot carries the record's own `scale`, re-read straight off the record (`FFXiMain.dll
    // retail-2026-09` RVA 0x2B2DD), so neither zero-guard can leak into it; the undo divides only the axis stretched
    // above.
    let bearing = w.atan2(u);
    let mut out = Vec3::new(bearing.cos() * radius, bearing.sin() * radius, record.scale);
    if stretched_y {
        out.y /= xlim / ylim;
    } else {
        out.x /= ylim / xlim;
    }
    Some((basis.unproject(out).try_normalize()?, true))
}

/// The bend's two shared vectors, in the space `pose` lives in. They are built once per frame and every record is
/// clamped against those same operands: pass 1 of retail's bend reaches its clamp call entirely through
/// loop-invariant instructions (`FFXiMain.dll retail-2026-09` RVA 0x2AED1..0x2AF1B) and never re-samples a frame
/// inside the record loop, which is why one bone cannot end up aimed at a different target than its shoulder.
#[derive(Debug, Clone, Copy)]
pub struct BendFrame {
    /// The axis every bend starts from and the ellipse is centred on: the actor's forward. Retail derives it from the
    /// rig (slot 3 minus slot 4, with slot 4 authored behind the neck on shipped rigs: hum_ `(0,-1.5,-1.8)`); kuluu
    /// takes the actor's pose-space forward directly, because its root carries a different bind convention from
    /// retail's node matrix (`update_joint` rotates the root translation by `rotate270`), and running retail's
    /// subtraction through that frame produced a spine-up vector instead of a forward one. Steering a spine axis is
    /// a bow of the torso; steering forward is a head turn.
    pub forward: Vec3,
    /// The chased look point measured from the neck reference (`model+0xB0` minus the anchor in `FFXiMain.dll
    /// retail-2026-09`, taken as a subtract operand at RVA 0x2ACD9 / RVA 0x2ACE7). The point sits 20 units out when
    /// neutral, so measuring from the neck pivot rather than retail's anchor changes the direction by well under a
    /// degree.
    pub look: Vec3,
}

impl BendFrame {
    /// `anchor_origin_y_drop` is retail's sitting/resting adjustment: for wire statuses `5` and `0x55` it subtracts
    /// `1.0f` from the second component of the reference-4 object before either vector is built (`FFXiMain.dll
    /// retail-2026-09`: `fld [esp+0x30] / fsub dword [0x1032961c] (= 1.0) / fstp [esp+0x30]` at RVA 0x2ACA8..0x2ACB2,
    /// gated by `cmp esi,5` / `cmp esi,0x55` at RVA 0x2AC9E..0x2ACA6 against the status argument loaded at
    /// RVA 0x2AC97). It moves the origin the look vector is measured from.
    pub fn new(
        pose: &[Mat4],
        skeleton: &Skeleton,
        look_point: Vec3,
        forward: Vec3,
        anchor_origin_y_drop: f32,
    ) -> Option<Self> {
        let neck = attach_frame(pose, skeleton, standard_position::NECK)?;
        let origin = Vec3::new(
            neck.origin.x,
            neck.origin.y - anchor_origin_y_drop,
            neck.origin.z,
        );
        Some(Self {
            forward: forward.try_normalize()?,
            look: look_point - origin,
        })
    }
}

/// The bones a model authored bends for, in retail's record order: the authored limits through
/// [`Skeleton::look_at_limits`] onto the joint each [`BEND_SLOTS`] entry names. A record is dropped when **both** of
/// its semi-axes are authored as exactly zero — `FFXiMain.dll retail-2026-09` runs `fld [ecx] / fcomp dword
/// [0x103295d8]` then `fld [ecx+4] / fcomp 0`, each with `test ah,0x44`, at RVA 0x2AEE3..0x2AF00 and gated
/// identically again in pass 2 at RVA 0x2AFCA..0x2AFE7. That mask filters out C0, so its jump runs the clamp for any
/// value except zero: one tight axis is still a live record (a per-axis reading here used to delete every rig that
/// authored one axis as zero while giving the other a limit). A record is also
/// dropped when the model has no such reference or its joint runs past the joint count. Only the first `records` of
/// them are candidates: retail drops the second record for an actor whose wire status is in its one-record set
/// (`FFXiMain.dll retail-2026-09` RVA 0x2AE0D..0x2AEBB).
pub fn authored_bones(skeleton: &Skeleton, records: usize) -> Vec<LookBone> {
    let mut out: Vec<LookBone> = Vec::new();
    for (record, slot) in BEND_SLOTS.into_iter().enumerate().take(records) {
        let Some(limit) = skeleton.look_at_limit(record) else {
            continue;
        };
        if limit.x_limit == 0.0 && limit.y_limit == 0.0 {
            continue;
        }
        let Some(joint) = skeleton.reference_at(slot).map(|r| r.index) else {
            continue;
        };
        if joint >= skeleton.joints.len() {
            continue;
        }
        match out.iter_mut().find(|bone| bone.joint == joint) {
            // Retail samples each bone's matrix from a cache the bend itself never writes (`FFXiMain.dll
            // retail-2026-09` RVA 0x2B004..0x2B018), so when both records name one bone (11 shipped rigs do) the
            // later record replaces the earlier instead of compounding onto it.
            Some(seen) => seen.limit = *limit,
            None => out.push(LookBone {
                joint,
                limit: *limit,
            }),
        }
    }
    out
}

/// Bends the first `records` authored bones of `skeleton`, each inside its own ellipse, blended by the look-at
/// weight that opens and closes over about 25 frames (see `HeadLook` in kuluu-render). Applied shallow-first so a
/// shoulder bend carries the head below it the way retail's hierarchy does.
///
/// Frame discipline: every input — the neck frame, each bone pivot, `look_point_pose` and `forward` — lives in one
/// space, namely `pose`'s (`update_joint` bakes facing into the root), so a look point reaching here from another
/// path has to be converted by its caller rather than patched inside this pass, and `forward` is the pose-space nose
/// axis (the rotation that conversion removed never reappears here).
///
/// `parent_overrides` are the `(joint, new parent)` pairs `pose` was composed with (a held weapon's handle riding
/// its hand while engaged): a bend turns whatever is composed under its bone, so a re-parented joint follows the
/// bone it now hangs from instead of staying where its authored parent left it.
#[allow(clippy::too_many_arguments)]
pub fn apply_look_bends(
    pose: &mut [Mat4],
    skeleton: &Skeleton,
    parent_overrides: &[(usize, usize)],
    look_point_pose: Vec3,
    forward: Vec3,
    weight: f32,
    records: usize,
    anchor_origin_y_drop: f32,
) {
    if weight <= 0.0 {
        return;
    }
    let Some(frame) = BendFrame::new(
        pose,
        skeleton,
        look_point_pose,
        forward,
        anchor_origin_y_drop,
    ) else {
        return;
    };
    // Pass 1 (`FFXiMain.dll retail-2026-09` RVA 0x2AED1..0x2AF90) clamps every record against the SAME operands.
    // Pass 2 (RVA 0x2AFBD..0x2B0F4) multiplies each bend onto that bone's own node orientation and stores it in a
    // per-bone override table (`0x1045F030 + bone*0x34`), which dancer's skeleton build composes through the
    // hierarchy — LOCAL rotations whose world effect multiplies: head = shoulder × head. Rotating world-space
    // subtrees in record order instead lets the shallower chest bend drag an already-bent head, which reads as
    // "shoulders move, head frozen". Equivalent composition needs every bend computed from
    // the pre-bend pose — pass 1 reads a cache the bend never writes (RVA 0x2B004..0x2B018) — applied shallow-first so
    // each pivot already carries its ancestors' motion.
    let mut bends: Vec<(usize, Quat, usize)> = Vec::new();
    for bone in authored_bones(skeleton, records) {
        let Some((aim, _saturated)) = clamp_bend_axis(frame.forward, frame.look, &bone.limit)
        else {
            continue;
        };
        let Some(aim) = aim.try_normalize() else {
            continue;
        };
        // Retail's own per-record construction, read end to end in `FFXiMain.dll retail-2026-09`: RVA 0x33360 takes
        // the shared reference vector and that record's clamped output, takes acos of their dot product (clamped to
        // [−1, +1]), multiplies the angle by the blend weight (`model+0xBC`, loaded at RVA 0x2ADF1), builds the axis as
        // the normalised cross product, and writes sin(half)/cos(half) — a minimal-arc quaternion with no roll term of
        // any kind; RVA 0x32E10 then normalises it and RVA 0x32FE0 turns it into a rotation matrix. The weight scaling
        // lands here as `slerp(identity, arc, weight)` in [`rotate_about_pivot`]. The axis being steered is the shared
        // forward, never a bone-local axis: on every shipped rig `rot·X` runs up the spine, which is exactly why
        // reading yaw and pitch off those axes turned a sideways target into a bow of the torso.
        bends.push((
            bone.joint,
            Quat::from_rotation_arc(frame.forward, aim),
            joint_depth(skeleton, bone.joint),
        ));
    }
    bends.sort_by_key(|(_, _, depth)| *depth);
    for (joint, rot, _) in bends {
        rotate_about_pivot(pose, skeleton, parent_overrides, joint, rot, weight);
    }
}

/// Hops to the root of a joint's parent chain; used only to order bends shallow-first so a bone is rotated after its
/// ancestors (`FFXiMain.dll retail-2026-09` composes local orientations through the hierarchy at RVA 0x2AFBD..0x2B0F4
/// rather than rotating world-space subtrees).
fn joint_depth(skeleton: &Skeleton, joint: usize) -> usize {
    let mut depth = 0;
    let mut current = skeleton.joints.get(joint).and_then(|j| j.parent);
    while let Some(parent) = current {
        if parent == joint || depth > skeleton.joints.len() {
            break; // a cycle in authored parents would otherwise spin forever
        }
        depth += 1;
        current = skeleton.joints.get(parent).and_then(|j| j.parent);
    }
    depth
}

/// Rigid subtree rotation about the joint's own pivot: re-posing descendants without recomposing the whole hierarchy
/// for a look.
fn rotate_about_pivot(
    pose: &mut [Mat4],
    skeleton: &Skeleton,
    parent_overrides: &[(usize, usize)],
    joint: usize,
    rot: Quat,
    weight: f32,
) {
    let blended = Quat::IDENTITY.slerp(rot, weight.clamp(0.0, 1.0));
    if blended == Quat::IDENTITY {
        return;
    }
    let Some(mat) = pose.get(joint).copied() else {
        return;
    };
    let pivot = mat.w_axis.truncate();
    let about_pivot =
        Mat4::from_translation(pivot) * Mat4::from_quat(blended) * Mat4::from_translation(-pivot);
    for j in composed_subtree(skeleton, parent_overrides, joint) {
        if let Some(m) = pose.get_mut(j) {
            *m = about_pivot * *m;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::datid::DatId;
    use ffxi_dat::skel::{Joint, JointReference};

    const IDENTITY_QUAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    fn joint(parent: Option<usize>, translation: [f32; 3]) -> Joint {
        Joint {
            rotation: IDENTITY_QUAT,
            translation,
            parent,
        }
    }

    fn reference(index: usize, position_offset: [f32; 3]) -> JointReference {
        JointReference {
            index,
            rotation: [0.0; 3],
            position_offset,
        }
    }

    fn limit(x: f32, y: f32, scale: f32) -> LookAtLimit {
        LookAtLimit {
            x_limit: x,
            y_limit: y,
            scale,
        }
    }

    /// The retail nesting, in the anchor's own numbers: hum_ puts the reference-4 anchor at `(0,-1.5,-1.8)` off the
    /// root and its neck reference (slot 3, joint 51) exactly on the neck pivot, so the shared axis comes out along
    /// `+Z`. Chest is the neck's parent, which is what makes record 1 a shoulder share rather than a second head.
    fn rig(records: Vec<LookAtLimit>) -> Skeleton {
        let mut references = vec![reference(0, [0.0; 3]); standard_position::CHEST + 1];
        references[standard_position::NECK] = reference(1, [0.0; 3]);
        references[BEND_ANCHOR_SLOT] = reference(0, [0.0, -1.5, -1.8]);
        references[standard_position::CHEST] = reference(2, [0.0; 3]);
        Skeleton {
            id: DatId::from_str("test"),
            joints: vec![
                joint(None, [0.0, 0.0, 0.0]),
                joint(Some(2), [0.0, -1.5, 0.0]),
                joint(Some(0), [0.0, -1.25, 0.0]),
            ],
            references,
            bounding_boxes: Vec::new(),
            look_at_limits: records,
        }
    }

    /// The joints at rest: unrotated, pivots on their bind positions.
    fn posed() -> Vec<Mat4> {
        vec![
            Mat4::from_translation(Vec3::ZERO),
            Mat4::from_translation(Vec3::new(0.0, -1.5, 0.0)),
            Mat4::from_translation(Vec3::new(0.0, -1.25, 0.0)),
        ]
    }

    /// The fixture stands with `+Z` forward (its anchor sits at `-Z` behind the neck, the retail nesting).
    const FIXTURE_FORWARD: Vec3 = Vec3::Z;

    /// A joint authored off the root but composed under the head this frame (a held weapon's handle on its hand)
    /// rides the bend with the bone it hangs from; its authored parent never turns, so leaving it out would leave
    /// the weapon hanging in the air while the hand moves on.
    #[test]
    fn a_re_parented_joint_rides_the_bend_of_the_bone_it_hangs_from() {
        let mut skel = rig(vec![limit(0.24, 0.16, 0.5)]);
        skel.joints.push(joint(Some(0), [0.3, -1.5, 0.0]));
        let held = (3, 1);
        let mut pose = posed();
        pose.push(Mat4::from_translation(Vec3::new(0.3, -1.5, 0.0)));
        let on_the_head = pose[1].inverse() * pose[3];

        apply_look_bends(
            &mut pose,
            &skel,
            &[held],
            Vec3::new(9.0, -1.5, 0.0),
            FIXTURE_FORWARD,
            1.0,
            1,
            0.0,
        );

        assert_ne!(pose[1], posed()[1], "a sideways look turns the head");
        assert!(
            (pose[1].inverse() * pose[3]).abs_diff_eq(on_the_head, 1e-5),
            "the held joint must stay where it sits on the head"
        );
    }

    fn frame(look: Vec3, drop: f32) -> BendFrame {
        let skel = rig(vec![limit(0.24, 0.16, 0.5)]);
        BendFrame::new(&posed(), &skel, look, FIXTURE_FORWARD, drop)
            .expect("fixture has a neck frame")
    }

    /// The steered axis is the actor's forward, and the look vector is the point measured from the neck reference.
    #[test]
    fn the_steered_axis_is_the_actors_forward_and_the_look_is_measured_from_the_neck() {
        let f = frame(Vec3::new(5.0, -1.5, 0.0), 0.0);
        assert_eq!(f.forward, Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(f.look, Vec3::new(5.0, 0.0, 0.0));
    }

    /// The sitting/riding adjustment moves the origin the look vector is measured from, never the forward.
    #[test]
    fn the_anchor_drop_moves_the_look_vector_only() {
        let seated = frame(Vec3::new(5.0, -2.5, 0.0), 1.0);
        let standing = frame(Vec3::new(5.0, -2.5, 0.0), 0.0);
        assert_ne!(seated.look, standing.look);
        assert_eq!(seated.forward, standing.forward);
        assert!((seated.look.y - standing.look.y - 1.0).abs() < 1e-6);
    }

    /// A target off to the side turns the head about the up axis, not about a sideways one: the bone's own forward
    /// ends up rotated toward the target in the ground plane, and its up stays up. This is the arched-back
    /// regression: steering a spine-up reference bowed the torso instead.
    #[test]
    fn a_sideways_target_yaws_the_head_instead_of_bowing_it() {
        let skel = rig(vec![limit(0.24, 0.16, 0.5)]);
        let mut pose = posed();
        // Dead left of the neck at neck height: the ellipse must saturate across, with no up/down component.
        apply_look_bends(
            &mut pose,
            &skel,
            &[],
            Vec3::new(9.0, -1.5, 0.0),
            FIXTURE_FORWARD,
            1.0,
            2,
            0.0,
        );
        let (_, rot, _) = pose[1].to_scale_rotation_translation();
        let turned_forward = rot * FIXTURE_FORWARD;
        assert!(turned_forward.y.abs() < 1e-4, "no pitch: {turned_forward}");
        assert!(turned_forward.x > 0.3, "turned toward +X: {turned_forward}");
        let up = rot * Vec3::NEG_Y;
        assert!((up - Vec3::NEG_Y).length() < 1e-4, "up stays up: {up}");
        let turn = turned_forward.angle_between(FIXTURE_FORWARD);
        let cap = (0.24f32 / 0.5).atan();
        assert!(
            (turn - cap).abs() < 1e-3,
            "turn {turn} should sit on the rim {cap}"
        );
    }

    /// A target above pitches the head up about the sideways axis and leaves yaw alone.
    #[test]
    fn a_target_above_pitches_the_head_without_yaw() {
        let skel = rig(vec![limit(0.24, 0.16, 0.5)]);
        let mut pose = posed();
        apply_look_bends(
            &mut pose,
            &skel,
            &[],
            Vec3::new(0.0, -9.0, 3.0),
            FIXTURE_FORWARD,
            1.0,
            2,
            0.0,
        );
        let (_, rot, _) = pose[1].to_scale_rotation_translation();
        let turned_forward = rot * FIXTURE_FORWARD;
        assert!(turned_forward.x.abs() < 1e-4, "no yaw: {turned_forward}");
        assert!(turned_forward.y < -0.1, "pitched up (-Y): {turned_forward}");
        let turn = turned_forward.angle_between(FIXTURE_FORWARD);
        let cap = (0.16f32 / 0.5).atan();
        assert!(
            turn <= cap + 1e-3,
            "turn {turn} past the vertical rim {cap}"
        );
    }

    /// Inside the ellipse nothing happens: retail hands back the vector it was given (`FFXiMain.dll retail-2026-09`
    /// RVA 0x2B33F..0x2B371), so the
    /// arc from the steered axis to it is an identity and the pose stays put. A target sitting exactly along that
    /// axis is the degenerate case of this, and it must not move the spine.
    #[test]
    fn a_target_along_the_steered_axis_bends_nothing() {
        let skel = rig(vec![limit(0.24, 0.16, 0.5)]);
        let mut pose = posed();
        apply_look_bends(
            &mut pose,
            &skel,
            &[],
            Vec3::new(0.0, -1.5, 9.0),
            FIXTURE_FORWARD,
            1.0,
            2,
            0.0,
        );
        assert_eq!(
            pose,
            posed(),
            "aim along the steered axis must not move any joint"
        );
    }

    /// A target off that axis does move it, and how far is capped by the record rather than by the bearing: past the
    /// rim the rebuilt point is `(cosθ·xlim, sinθ·ylim, scale)`, so the turn stops near `atan(xlim/scale)` across.
    #[test]
    fn a_saturated_bend_stops_at_the_records_own_angle() {
        let record = limit(0.24, 0.16, 0.5);
        // A point far off the aim axis: the clamp has to saturate and rebuild on the rim.
        let (aim, saturated) =
            clamp_bend_axis(Vec3::X, Vec3::Z, &record).expect("basis is well defined");
        assert!(saturated, "90° off the aim must saturate");
        let across = record.x_limit / record.scale;
        let up = record.y_limit / record.scale;
        // Measured against the *aim* axis (which is what `at` names), not against the original point: retail's rim
        // rebuild keeps `scale` in the depth slot, so the rebuilt vector sits atan(radius/scale) off the aim.
        let turn = aim.angle_between(Vec3::X);
        let max_turn = across.atan2(1.0).max(up.atan2(1.0));
        let min_turn = up.atan2(1.0);
        assert!(
            turn <= max_turn + 1e-3 && turn >= min_turn - 1e-3,
            "turn {turn} rad outside the record's own cone [{min_turn}, {max_turn}]"
        );
    }

    /// Ahead of the basis plane there is a perspective divide; at or behind it there is not (`FFXiMain.dll
    /// retail-2026-09`: RVA 0x2B253 picks the arm, and RVA 0x2B286's `fdiv` lives on its jump target). A point behind
    /// the plane must therefore still land
    /// somewhere finite and off-axis rather than diverging or collapsing onto the aim.
    #[test]
    fn a_point_behind_the_plane_saturates_without_diverging() {
        // Off-axis as well as behind: a point straight backwards has no lateral component to saturate.
        let behind = Vec3::new(-1.0, 0.0, -1.0).normalize();
        let (aim, saturated) = clamp_bend_axis(Vec3::Z, behind, &limit(0.24, 0.16, 0.5)).unwrap();
        assert!(aim.is_finite(), "{aim} diverged behind the plane");
        assert!(saturated);
        assert!(aim.angle_between(Vec3::Z) > 0.1);
    }

    /// A point inside the ellipse is handed back unchanged, which is what makes a near target cost no bend at all.
    #[test]
    fn a_point_inside_the_ellipse_is_handed_back_unchanged() {
        let record = limit(2.0, 2.0, 1.0);
        let (aim, saturated) =
            clamp_bend_axis(Vec3::Z, Vec3::new(0.05, 0.05, 1.0).normalize(), &record).unwrap();
        assert!(!saturated);
        assert!(aim.angle_between(Vec3::new(0.05, 0.05, 1.0)) < 1e-4);
    }

    /// A record is only inert when both semi-axes are zero (`FFXiMain.dll retail-2026-09` RVA 0x2AEE3..0x2AF00 asks
    /// "is this float equal to zero" twice). Reading that as per-axis drops every rig that authors one axis tight.
    #[test]
    fn only_a_record_with_both_axes_zero_is_inert() {
        let both = rig(vec![limit(0.0, 0.0, 0.5)]);
        assert!(authored_bones(&both, BEND_RECORDS_MAX).is_empty());

        let one_axis = rig(vec![limit(0.24, 0.0, 0.5), limit(0.16, 0.0, 0.5)]);
        let joints: Vec<usize> = authored_bones(&one_axis, BEND_RECORDS_MAX)
            .iter()
            .map(|b| b.joint)
            .collect();
        assert_eq!(
            joints,
            vec![1, 2],
            "a single zeroed axis must not delete the record"
        );
    }

    /// The records go to the joints their slots name — slot 3 then slot 7 — so the anchor (slot 4) never becomes bone.
    #[test]
    fn each_record_uses_the_joint_its_slot_names() {
        let skel = rig(vec![limit(0.5, 0.5, 1.0), limit(0.25, 0.25, 1.0)]);
        let joints: Vec<usize> = authored_bones(&skel, BEND_RECORDS_MAX)
            .iter()
            .map(|b| b.joint)
            .collect();
        assert_eq!(joints, vec![1, 2], "record order follows slots 3 then 7");
        let anchor_joint = skel.reference_at(BEND_ANCHOR_SLOT).unwrap().index;
        assert!(
            !joints.contains(&anchor_joint),
            "the anchor is never a bend bone"
        );
    }

    /// When both records name one bone the later record's ellipse is the one that limits it (retail's pass 2 writes each
    /// joint's override from its own record and never reads back what the earlier one left).
    #[test]
    fn two_records_on_one_bone_keep_the_later_ellipse() {
        let mut skel = rig(vec![limit(0.5, 0.5, 1.0), limit(0.25, 0.25, 1.0)]);
        skel.references[standard_position::CHEST] = reference(1, [0.0; 3]);
        let bones = authored_bones(&skel, BEND_RECORDS_MAX);
        assert_eq!(bones.len(), 1);
        assert!((bones[0].limit.x_limit - 0.25).abs() < 1e-6);
    }

    /// Retail's bend applies at most two records whatever a model authors past them.
    #[test]
    fn no_more_than_two_records_are_applied() {
        let skel = rig(vec![
            limit(0.5, 0.5, 1.0),
            limit(0.4, 0.4, 1.0),
            limit(0.3, 0.3, 1.0),
        ]);
        assert_eq!(
            authored_bones(&skel, BEND_RECORDS_MAX).len(),
            BEND_RECORDS_MAX,
            "a record past the cap is not applied"
        );
    }

    /// Retail's single-record branch: only the first record's bone moves; the parent under it keeps its bind pose.
    #[test]
    fn one_record_bends_only_the_first_bone() {
        let skel = rig(vec![limit(0.5, 0.5, 0.5), limit(0.4, 0.4, 0.5)]);
        let look = Vec3::new(-4.0, -1.5, 0.0);
        let mut one = posed();
        apply_look_bends(&mut one, &skel, &[], look, FIXTURE_FORWARD, 1.0, 1, 0.0);
        assert_ne!(one[1], posed()[1]);
        assert_eq!(
            one[2],
            posed()[2],
            "the chest bone is out of play with one record"
        );

        let mut both = posed();
        apply_look_bends(
            &mut both,
            &skel,
            &[],
            look,
            FIXTURE_FORWARD,
            1.0,
            BEND_RECORDS_MAX,
            0.0,
        );
        assert_ne!(both[2], posed()[2]);
    }

    /// The head sits under the neck, so it holds both bends — and each bend is computed from the shared pre-bend
    /// axis (`FFXiMain.dll retail-2026-09` pass 1 reads a cache the bend never writes at RVA 0x2B004..0x2B018), not
    /// re-aimed after its parent moved.
    #[test]
    fn both_bends_compound_into_the_head_turn() {
        // A subtree rotates about its own pivot, so a bone's own translation never moves — measure how far its
        // orientation turned from bind. Joint 1 hangs off joint 2, so its turn carries its own bend and its
        // parent's whenever both are authored.
        let turned = |pose: &[Mat4], joint: usize| {
            (pose[joint].x_axis - posed()[joint].x_axis).length()
                + (pose[joint].z_axis - posed()[joint].z_axis).length()
        };
        let lim = limit(0.6, 0.6, 0.5);
        let inert = limit(0.0, 0.0, 0.5);
        let look = Vec3::new(-4.0, -1.5, 0.0);
        let run = |records: [LookAtLimit; 2]| {
            let skel = rig(records.to_vec());
            let mut pose = posed();
            apply_look_bends(
                &mut pose,
                &skel,
                &[],
                look,
                FIXTURE_FORWARD,
                1.0,
                BEND_RECORDS_MAX,
                0.0,
            );
            pose
        };
        // Joint 1 is the head-side bone (record 0) and hangs off joint 2 (record 1), so its displacement carries
        // both bends: retail's pass-2 composition makes the world turn a product, never one record alone.
        let head_only = run([lim, inert]);
        let shoulder_only = run([inert, lim]);
        let both = run([lim, lim]);
        assert!(turned(&head_only, 1) > 0.0 && turned(&shoulder_only, 2) > 0.0);
        assert!(
            turned(&both, 1) > turned(&head_only, 1)
                && turned(&both, 1) > turned(&shoulder_only, 1),
            "head turn {} should exceed each single bend ({} / {})",
            turned(&both, 1),
            turned(&head_only, 1),
            turned(&shoulder_only, 1)
        );
    }

    /// The weight gates the bend: at zero nothing is posed, and half weight turns less than full.
    #[test]
    fn the_weight_blends_the_bend() {
        let skel = rig(vec![limit(0.24, 0.16, 0.5)]);
        let look = Vec3::new(-4.0, -1.5, 0.0);
        let mut off = posed();
        apply_look_bends(
            &mut off,
            &skel,
            &[],
            look,
            FIXTURE_FORWARD,
            0.0,
            BEND_RECORDS_MAX,
            0.0,
        );
        assert_eq!(off, posed(), "weight 0 must not bend");

        // Same reason as above: the bend rotates about the bone's pivot, so compare orientation, not translation.
        let turned = |pose: &[Mat4]| (pose[1].x_axis - posed()[1].x_axis).length();
        let mut half = posed();
        apply_look_bends(
            &mut half,
            &skel,
            &[],
            look,
            FIXTURE_FORWARD,
            0.5,
            BEND_RECORDS_MAX,
            0.0,
        );
        let mut full = posed();
        apply_look_bends(
            &mut full,
            &skel,
            &[],
            look,
            FIXTURE_FORWARD,
            1.0,
            BEND_RECORDS_MAX,
            0.0,
        );
        assert!(turned(&half) < turned(&full));
    }

    /// Real data: the HumeM skeleton (`ROM/27/82.DAT`, `hum_`) authors `(0.24,0.16)` and `(0.16,0.06)` at scale 0.5,
    /// and its bend slots name neck joint 51 (child of chest 50) and chest joint 50 — so both records land.
    #[test]
    fn real_dat_hume_bends_its_neck_and_chest_joints() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let Ok(loc) = root.resolve(7072) else { return };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            return;
        };
        let Some(skel) = ffxi_dat::resource_dir::ResourceDir::from_bytes(bytes)
            .collect_skeletons()
            .into_iter()
            .next()
        else {
            return;
        };
        assert_eq!(skel.id, DatId::from_str("hum_"));
        let bones = authored_bones(&skel, BEND_RECORDS_MAX);
        let joints: Vec<usize> = bones.iter().map(|b| b.joint).collect();
        assert_eq!(joints, vec![51, 50], "hume bendable joints: {joints:?}");
        assert!((bones[0].limit.x_limit - 0.24).abs() < 1e-6);
        assert!((bones[1].limit.x_limit - 0.16).abs() < 1e-6);

        // The authored attach-frame rotations on slots 3/4/7 are zero in shipped data, which is what makes the
        // anchor origin (not a rotated one) the thing both shared vectors measure from.
        for slot in [
            standard_position::NECK,
            BEND_ANCHOR_SLOT,
            standard_position::CHEST,
        ] {
            assert_eq!(skel.reference_at(slot).unwrap().rotation, [0.0; 3]);
        }
    }

    /// Real data: the Tarutaru skeleton (file id 19776, chunk `tar `) authors the same pair as Hume at joints 6/5, so
    /// any per-race rule leaving Tarutaru without a head bend — or its shoulder share — was wrong.
    #[test]
    fn real_dat_tarutaru_authors_both_bends_too() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let Ok(loc) = root.resolve(19776) else { return };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            return;
        };
        let Some(skel) = ffxi_dat::resource_dir::ResourceDir::from_bytes(bytes)
            .collect_skeletons()
            .into_iter()
            .next()
        else {
            return;
        };
        assert_eq!(skel.id, DatId::from_name(b"tar "));
        let bones = authored_bones(&skel, BEND_RECORDS_MAX);
        assert_eq!(
            bones
                .iter()
                .map(|b| (b.joint, b.limit.x_limit))
                .collect::<Vec<_>>(),
            vec![(6, 0.24), (5, 0.16)],
            "tarutaru bend joints and their authored horizontal semi-axes"
        );
    }

    /// The Mithra rig — the model the play test runs on — must author a *head* record too: a head that never bends
    /// while the shoulders do was the P1 symptom, and it cannot come from the data if record 0 is authored.
    #[test]
    fn real_dat_mithra_authors_a_head_record() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let Ok(loc) = root.resolve(23176) else { return };
        let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
            return;
        };
        let Some(skel) = ffxi_dat::resource_dir::ResourceDir::from_bytes(bytes)
            .collect_skeletons()
            .into_iter()
            .next()
        else {
            return;
        };
        assert_eq!(skel.id, DatId::from_name(b"mit "));
        let bones = authored_bones(&skel, BEND_RECORDS_MAX);
        assert_eq!(bones.len(), 2, "both records authored for Mithra");
        assert!(
            bones[0].limit.x_limit > 0.0 && bones[0].limit.y_limit > 0.0,
            "record 0 (the head) must carry a real ellipse, got {:?}",
            bones[0].limit
        );

        // And the look vector has to exist at all: without a neck reference there is nothing to measure from,
        // which would freeze both bones rather than one.
        let pose: Vec<Mat4> = skel
            .joints
            .iter()
            .map(|j| Mat4::from_translation(Vec3::from(j.translation)))
            .collect();
        assert!(BendFrame::new(&pose, &skel, Vec3::new(2.0, -1.5, 0.0), Vec3::X, 0.0).is_some());
    }
}
