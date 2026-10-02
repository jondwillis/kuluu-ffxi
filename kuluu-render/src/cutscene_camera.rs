//! Cutscene camera routes: the kind 0x06 camera resources a scheduler routine drives the
//! operator camera along (research/XIClient source/World/Camera/CameraTask.cpp and
//! SplinePath.cpp). A routine's stage 0x04 names one of these by four-char id and plays it for
//! the stage's scaled duration; while a task is live and [`CutsceneMode::camera_locked`] holds,
//! this module owns the operator camera's transform and projection focal length.

use bevy::prelude::*;

use ffxi_dat::camera::{CameraControlPoint, CameraPathMode, CameraResource, CameraSmoothType};

use crate::cutscene::CutsceneMode;
use crate::graphics_settings::{GraphicsSettings, RETAIL_PROJECTION_HALF_HEIGHT};
use crate::scene::BakedActor;
use crate::snapshot::EventLog;

/// research/XIClient source/World/Camera/Spline.cpp MIN_SPINE_SEGMENT_LENGTH - the floor a zero
/// chord takes so chordal parameterization stays finite on stacked points.
const MIN_SPLINE_SEGMENT_LENGTH: f32 = 0.01;

/// research/XIClient source/World/Camera/CameraManager.cpp CalculateDefaultCameraPosition -
/// the default chase eye stands this far behind the actor along its facing.
const DEFAULT_CHASE_STANDOFF: f32 = 3.0;

/// research/XIClient source/World/Camera/CameraTask.cpp EvaluateProgressionCurve / Smooth -
/// the five authored curves over normalized time. A keyframe SmoothingType names a resource
/// retail resolves at load; without it the curve is linear time, which is also retail's
/// fallback when that resource fails to load.
pub fn progression_curve(smoothing: CameraSmoothType, t: f32) -> f32 {
    match smoothing {
        CameraSmoothType::Linear => t,
        CameraSmoothType::Decelerate => (t * std::f32::consts::FRAC_PI_2).sin(),
        CameraSmoothType::Accelerate => 1.0 - (t * std::f32::consts::FRAC_PI_2).cos(),
        CameraSmoothType::DecelerateToMidpointThenAccelerate => {
            let sin_term = 0.5 * (t * std::f32::consts::PI).sin();
            if t <= 0.5 {
                sin_term
            } else {
                1.0 - sin_term
            }
        }
        CameraSmoothType::AccelerateAndDecelerate => 0.5 * (1.0 - (t * std::f32::consts::PI).cos()),
        CameraSmoothType::Keyframe(_) => t,
    }
}

/// One of retail's three chordal Catmull-Rom splines (research/XIClient
/// source/Common/Math/Spline.cpp PrecomputeSpline / EvaluateComponent): the components share
/// one set of segment parameters, the boundary points are mirrored per component, and each
/// segment evaluates a Hermite basis through its precomputed matrix.
struct SplineTrack {
    /// The authored control values per component; evaluation reads them with one mirrored point on each side.
    components: [Vec<f32>; 3],

    /// Normalized chord of every inter-point segment, [0..n-1].
    params: Vec<f32>,

    /// One Hermite-basis matrix per segment, row-major 4x4.
    matrices: Vec<[f32; 16]>,
}

impl SplineTrack {
    /// Builds the per-segment Hermite-basis matrices (research/XIClient
    /// source/Common/Math/Spline.cpp PrecomputeSpline): row 0 is the position
    /// basis, row 1 the first-derivative (tangent) basis, row 2 the
    /// second-derivative (curvature) basis, row 3 the constraint row.
    fn build(components: [Vec<f32>; 3]) -> Option<Self> {
        let n = components[0].len();
        if n < 2 || components[1].len() != n || components[2].len() != n {
            return None;
        }
        // Chordal segment parameters: the inter-point chord across all three components, floored so a stacked point cannot zero a segment (Spline.cpp MIN_SPINE_SEGMENT_LENGTH).
        let mut chords = vec![0.0f32; n - 1];
        let mut total = 0.0f32;
        for i in 0..n - 1 {
            let dx = components[0][i + 1] - components[0][i];
            let dy = components[1][i + 1] - components[1][i];
            let dz = components[2][i + 1] - components[2][i];
            let chord = if dx == 0.0 && dy == 0.0 && dz == 0.0 {
                MIN_SPLINE_SEGMENT_LENGTH
            } else {
                (dx * dx + dy * dy + dz * dz)
                    .sqrt()
                    .max(MIN_SPLINE_SEGMENT_LENGTH)
            };
            chords[i] = chord;
            total += chord;
        }
        let params: Vec<f32> = chords.iter().map(|c| c / total).collect();

        // The per-segment matrices (Spline.cpp PrecomputeSpline): the boundary segments reuse
        // their neighbour's chord, which is what retail's duplicated SegmentParameters entries
        // do.
        let mut matrices = Vec::with_capacity(n - 1);
        for s in 0..n - 1 {
            let prev_param = if s == 0 { chords[0] } else { chords[s - 1] };
            let current_param = chords[s];
            let next_param = if s == n - 2 {
                chords[n - 2]
            } else {
                chords[s + 1]
            };

            let prev_weight = prev_param / (current_param + prev_param);
            let prev_complement = 1.0 - prev_weight;
            let prev_complement_sq = prev_complement * prev_complement;
            let next_weight = current_param / (current_param + next_param);
            let next_complement = 1.0 - next_weight;
            let cross_influence = prev_weight * next_weight;
            let left_endpoint_influence = -prev_complement_sq / prev_weight;
            let right_endpoint_influence = next_weight * next_weight / next_complement;

            matrices.push([
                left_endpoint_influence,
                (prev_complement + cross_influence) / prev_weight,
                -(prev_complement + cross_influence) / next_complement,
                right_endpoint_influence,
                (prev_complement_sq + prev_complement_sq) / prev_weight,
                -(prev_complement + prev_complement + cross_influence) / prev_weight,
                (prev_complement + prev_complement
                    - (next_weight - (cross_influence + cross_influence)))
                    / next_complement,
                -right_endpoint_influence,
                left_endpoint_influence,
                (1.0 - (prev_weight + prev_weight)) / prev_weight,
                prev_weight,
                0.0,
                0.0,
                1.0,
                0.0,
                0.0,
            ]);
        }

        Some(Self {
            components,
            params,
            matrices,
        })
    }

    /// The track's value at normalized time `t` for one component (Spline.cpp EvaluateComponent):
    /// the segment is found by accumulating normalized chords, then the Hermite basis over that
    /// segment's four control values (the two mirrored boundary points included).
    fn evaluate(&self, component: u8, t: f32) -> f32 {
        let points = &self.components[component as usize];
        let n = points.len();
        let segment_count = n - 1;

        let mut cumulative = 0.0f32;
        let mut segment_index = 0;
        for s in 0..segment_count {
            cumulative += self.params[s];
            if cumulative >= t {
                break;
            }
            segment_index = s + 1;
        }

        let (local_t, segment_index) = if segment_index >= segment_count {
            (1.0f32, segment_count - 1)
        } else {
            let current_segment_param = self.params[segment_index];
            let previous_cumulative = cumulative - current_segment_param;
            (
                (t - previous_cumulative) / current_segment_param,
                segment_index,
            )
        };

        // The four control values: the mirrored predecessor, the segment's two points, and the
        // mirrored successor (Spline.cpp EvaluateComponent's componentData[s..s+3]).
        let c0 = points[segment_index] - (points[segment_index + 1] - points[segment_index]);
        let c1 = points[segment_index];
        let c2 = points[segment_index + 1];
        let c3 = if segment_index + 2 < n {
            points[segment_index + 2]
        } else {
            2.0 * points[n - 1] - points[n - 2]
        };

        let m = &self.matrices[segment_index];
        let t2 = local_t * local_t;
        let t3 = t2 * local_t;
        // weights[j] = column j of the matrix times [t^3, t^2, t, 1], the way retail's
        // TransformPointToHomogeneous applies it (GraphicsMathProvider.cpp): out.x takes
        // _11/_21/_31/_41. Row-wise application would not interpolate the control points.
        let w0 = m[0] * t3 + m[4] * t2 + m[8] * local_t + m[12];
        let w1 = m[1] * t3 + m[5] * t2 + m[9] * local_t + m[13];
        let w2 = m[2] * t3 + m[6] * t2 + m[10] * local_t + m[14];
        let w3 = m[3] * t3 + m[7] * t2 + m[11] * local_t + m[15];

        w0 * c0 + w1 * c1 + w2 * c2 + w3 * c3
    }
}

/// One endpoint of a straight path, or the single point of a locked route: everything the
/// camera holds while it sits there (research/XIClient include/World/Camera/CameraFormat.h
/// SplineControlPoint; CameraTask.cpp's StraightPathEndpoints repurpose Param to carry roll and
/// focal length).
#[derive(Debug, Clone, Copy)]
pub struct Endpoint {
    eye: Vec3,
    target: Vec3,
    roll: f32,
    focal_length: f32,
}

/// The kind 0x06 points (camera.rs) are retail world coordinates (Y up); the
/// operator camera lives in Bevy space, whose up axis is -native-y like every
/// other placed asset.
impl From<&CameraControlPoint> for Endpoint {
    fn from(p: &CameraControlPoint) -> Self {
        Self {
            eye: crate::scene::mzb_to_bevy(kuluu_snapshot::Vec3 {
                x: p.position[0],
                y: p.position[1],
                z: p.position[2],
            }),
            target: crate::scene::mzb_to_bevy(kuluu_snapshot::Vec3 {
                x: p.target[0],
                y: p.target[1],
                z: p.target[2],
            }),
            roll: p.roll,
            focal_length: p.focal_length,
        }
    }
}

/// The operator camera's state at the moment a route starts: what START_AT_CURRENT_POS
/// substitutes in (research/XIClient source/World/Camera/CameraTask.cpp CameraTask constructor).
#[derive(Debug, Clone, Copy)]
pub struct CurrentCameraState {
    pub eye: Vec3,
    pub target: Vec3,
    pub roll: f32,
    pub focal_length: f32,
}

/// One frame of camera output from a running route.
#[derive(Debug, Clone, Copy)]
pub struct CameraFrame {
    pub eye: Vec3,
    pub target: Vec3,
    pub roll: f32,
    pub focal_length: f32,
}

impl From<CurrentCameraState> for CameraFrame {
    fn from(state: CurrentCameraState) -> Self {
        Self {
            eye: state.eye,
            target: state.target,
            roll: state.roll,
            focal_length: state.focal_length,
        }
    }
}

/// The attach context a route's start needs (research/XIClient source/World/Camera/
/// CameraTask.cpp constructor: the smoothed attach matrix seeds from the caster/target's
/// state at task creation, and OnMove chases the live matrix at InterpFactor/255 per frame).
#[derive(Debug, Clone, Copy)]
pub struct AttachStart {
    /// The attach actor's wire entity (the cue's caster; mode 2 and the unported modes play
    /// in world space instead).
    pub actor: Entity,

    /// The EID locator index the attach resolves against the actor's skeleton.
    pub locator: u32,

    /// The resource's InterpFactor scaled to 0..=1: how fast the smoothed matrix chases the
    /// live one (0 freezes the start-time matrix).
    pub interp: f32,

    /// The attach matrix at task start, Bevy space, column-vector convention.
    pub initial_matrix: Mat4,
}

/// The running attach: the actor to re-resolve each frame and the smoothed matrix the
/// evaluated frame transforms out through.
struct AttachTask {
    actor: Entity,

    locator: u32,

    interp: f32,

    smoothed: Mat4,
}

/// One running camera route on the renderer's clock (research/XIClient source/World/Camera/
/// CameraTask.cpp OnMove): a scaled frame duration, the path, and per-frame output of eye,
/// look-at, roll and focal length.
pub struct CutsceneCameraTask {
    resource: CameraResource,

    total_frames: f32,

    elapsed_frames: f32,

    done: bool,

    /// Spline mode only: the three chordal tracks (eye, target, focal/roll/id).
    tracks: Option<(SplineTrack, SplineTrack, SplineTrack)>,

    start_point: Endpoint,

    end_point: Endpoint,

    /// None is world space; the points are otherwise local to the attach actor.
    attach: Option<AttachTask>,
}

impl CutsceneCameraTask {
    /// research/XIClient source/World/Camera/CameraResource.cpp CreateCameraTask - a new task
    /// replaces the running one; a locked route with zero duration is a hard cut that applies
    /// its first point and runs out on the next frame. `total_frames` is already scaled by the
    /// 0x45 duration operand (scheduler_speed_ratio). An attached route carries its attach
    /// context: the points are local to the attach actor and each frame's eye/at transform
    /// out through the smoothed attach matrix (CameraTask.cpp constructor and OnMove).
    /// The straight/locked endpoints and the spline tracks all need the full point set;
    /// only the first and last are kept for the non-spline modes.
    pub fn start(
        resource: &CameraResource,
        total_frames: f32,
        current: CurrentCameraState,
        default_chase: Endpoint,
        attach: Option<AttachStart>,
    ) -> Self {
        // The START/END_AT_CURRENT_POS endpoints are expressed in the route's local space:
        // the inverse of the initial attach matrix maps the camera's world state into it
        // (CameraTask.cpp constructor).
        let to_local = attach.as_ref().map(|a| a.initial_matrix.inverse());
        let into_local = |p: Vec3| to_local.map(|m| m.transform_point3(p)).unwrap_or(p);

        // The flags add virtual endpoints from the camera's current state (start) and the
        // default chase position (end), the way CameraTask.cpp's constructor does.
        let mut points: Vec<Endpoint> = Vec::with_capacity(resource.points.len() + 2);
        if resource.flags.starts_at_current_pos() {
            points.push(Endpoint {
                eye: into_local(current.eye),
                target: into_local(current.target),
                roll: current.roll,
                focal_length: current.focal_length,
            });
        }
        for p in &resource.points {
            points.push(p.into());
        }
        if resource.flags.ends_at_current_pos() {
            points.push(Endpoint {
                eye: into_local(default_chase.eye),
                target: into_local(default_chase.target),
                roll: default_chase.roll,
                focal_length: default_chase.focal_length,
            });
        }

        // A route with no authored points and no flags applies the default chase state directly
        // (CameraResource.cpp ApplyCameraSettings' zero-count branch).
        if points.is_empty() {
            points.push(default_chase);
        }

        let mode = resource.path_mode();

        let start_point = points[0];
        let end_point = *points.last().unwrap();

        Self {
            resource: resource.clone(),
            total_frames,
            elapsed_frames: 0.0,
            done: false,
            tracks: match mode {
                CameraPathMode::Spline => Self::spline_tracks(&points),
                _ => None,
            },
            start_point,
            end_point,
            attach: attach.map(|a| AttachTask {
                actor: a.actor,
                locator: a.locator,
                interp: a.interp,
                smoothed: a.initial_matrix,
            }),
        }
    }

    fn spline_tracks(points: &[Endpoint]) -> Option<(SplineTrack, SplineTrack, SplineTrack)> {
        // Three splines with three components each (SplinePath.cpp): the eye position, the
        // look-at target, and focal/roll/id. Each track's chords span its own three components.
        let eye = [
            points.iter().map(|p| p.eye.x).collect(),
            points.iter().map(|p| p.eye.y).collect(),
            points.iter().map(|p| p.eye.z).collect(),
        ];
        let target = [
            points.iter().map(|p| p.target.x).collect(),
            points.iter().map(|p| p.target.y).collect(),
            points.iter().map(|p| p.target.z).collect(),
        ];
        // The third track carries focal, roll and a sequential point id (SplinePath.cpp); only
        // the first two are read back.
        let n = points.len();
        let rf = [
            points.iter().map(|p| p.focal_length).collect(),
            points.iter().map(|p| p.roll).collect(),
            (0..n).map(|i| i as f32).collect(),
        ];

        Some((
            SplineTrack::build(eye)?,
            SplineTrack::build(target)?,
            SplineTrack::build(rf)?,
        ))
    }

    /// Advance the task by `dt_secs` and output this frame's camera state; None once the scaled
    /// duration has run out. The last frame is still emitted: retail's OnMove applies the path
    /// before it checks RemainingDuration. `live_attach` is the attach matrix recomputed from
    /// the actor's current state; a world-space route ignores it, and a zero interp factor
    /// keeps the smoothed matrix at its start value (CameraTask.cpp OnMove).
    pub fn advance(&mut self, dt_secs: f32, live_attach: Option<Mat4>) -> Option<CameraFrame> {
        if self.done {
            return None;
        }
        self.elapsed_frames += dt_secs * crate::scheduler_runtime::ROUTINE_FPS;
        let t = if self.total_frames > 0.0 {
            (self.elapsed_frames / self.total_frames).min(1.0)
        } else {
            1.0
        };
        let curved = progression_curve(self.resource.smoothing, t);

        let frame = match &self.tracks {
            Some((eye_t, target_t, rf_t)) => CameraFrame {
                eye: Vec3::new(
                    eye_t.evaluate(0, curved),
                    eye_t.evaluate(1, curved),
                    eye_t.evaluate(2, curved),
                ),
                target: Vec3::new(
                    target_t.evaluate(0, curved),
                    target_t.evaluate(1, curved),
                    target_t.evaluate(2, curved),
                ),
                roll: rf_t.evaluate(1, curved),
                focal_length: rf_t.evaluate(0, curved),
            },
            None => {
                let eye = self.start_point.eye.lerp(self.end_point.eye, curved);
                let target = self.start_point.target.lerp(self.end_point.target, curved);
                CameraFrame {
                    eye,
                    target,
                    roll: self.start_point.roll
                        + (self.end_point.roll - self.start_point.roll) * curved,
                    focal_length: self.start_point.focal_length
                        + (self.end_point.focal_length - self.start_point.focal_length) * curved,
                }
            }
        };

        let frame = if let Some(attach) = self.attach.as_mut() {
            if attach.interp > 0.0 {
                if let Some(live) = live_attach {
                    attach.smoothed = lerp_matrix(attach.smoothed, live, attach.interp);
                }
            }
            // The path points are local to the attach actor; the smoothed matrix carries the
            // evaluated eye/at out to world (CameraTask.cpp OnMove's VirtProcessor15 pair).
            CameraFrame {
                eye: attach.smoothed.transform_point3(frame.eye),
                target: attach.smoothed.transform_point3(frame.target),
                roll: frame.roll,
                focal_length: frame.focal_length,
            }
        } else {
            frame
        };

        if self.elapsed_frames >= self.total_frames {
            self.done = true;
        }
        Some(frame)
    }
}

/// CameraTask.cpp OnMove: each element of the smoothed matrix steps toward the live matrix by
/// the interp factor.
fn lerp_matrix(smoothed: Mat4, live: Mat4, t: f32) -> Mat4 {
    Mat4::from_cols(
        smoothed.col(0).lerp(live.col(0), t),
        smoothed.col(1).lerp(live.col(1), t),
        smoothed.col(2).lerp(live.col(2), t),
        smoothed.col(3).lerp(live.col(3), t),
    )
}

/// research/XIClient source/World/Actor/Attachment.cpp MakeAttachMatrix mode1 - the attach
/// matrix retail seeds the smoothed matrix with: origin at the caster's EID point, rotation
/// the caster's yaw. The EID point is authored in the model's DAT frame, so it converts to
/// Bevy with mzb_to_bevy and then rides the wire entity's transform (position + yaw, scale 1).
pub fn attach_matrix(actor: &Transform, model_point: Vec3) -> Mat4 {
    let eid = actor
        .to_matrix()
        .transform_point3(crate::scene::mzb_to_bevy(kuluu_snapshot::Vec3 {
            x: model_point.x,
            y: model_point.y,
            z: model_point.z,
        }));
    Mat4::from_translation(eid) * Mat4::from_quat(actor.rotation)
}

/// research/XIClient include/World/Actor/EID_INDEX.h - the locator's point in the actor's
/// model frame: the skeleton's EID reference table when the actor has a loaded model, else a
/// fraction of the actor's height on the model's up axis. The special locators (EID_GROUND
/// and up) need collision or a nearest-actor search and stay unresolvable.
pub fn eid_model_point(
    locator: u32,
    baked: Option<&BakedActor>,
    render: Option<&crate::ffxi_actor_render::FfxiRenderActor>,
) -> Option<Vec3> {
    if locator >= ffxi_dat::camera::EID_NORMAL_MAX {
        return None;
    }
    if let Some(render) = render {
        let point = ffxi_actor::skeleton_instance::standard_joint_world_position(
            render.world_pose(),
            &render.skeleton,
            locator as usize,
        );
        if point.is_some_and(Vec3::is_finite) {
            return point;
        }
    }
    let height = baked.map_or(crate::camera::FALLBACK_ACTOR_HEIGHT, |b| b.actor_height);
    locator_height_fraction(locator).map(|frac| Vec3::new(0.0, height * frac, 0.0))
}

// research/XIClient include/World/Actor/EID_INDEX.h - the named body point indices; the
// height fractions below stand in for the skeleton reference table when the model did not
// load.
const EID_CURRENT: u32 = 0;
const EID_WAIST: u32 = 1;
const EID_NAME: u32 = 2;
const EID_NECK: u32 = 3;
const EID_LOOK_AT: u32 = 4;
const EID_HEAD_TOP: u32 = 5;
const EID_EYE_CENTER: u32 = 6;
const EID_CHEST: u32 = 7;
const EID_R_FOOT: u32 = 8;
const EID_L_FOOT: u32 = 9;
const EID_R_HAND: u32 = 10;
const EID_L_HAND: u32 = 11;
const EID_HEIGHT: u32 = 12;
const EID_BODY_CENTER: u32 = 21;
const EID_HEAD_CENTER: u32 = 22;
const EID_MAGIC0: u32 = 23;
const EID_REACH_H: u32 = 32;
const EID_R_EYE0: u32 = 33;
const EID_CAMERA3: u32 = 42;

fn locator_height_fraction(locator: u32) -> Option<f32> {
    Some(match locator {
        EID_CURRENT => 0.0,
        EID_WAIST => 0.45,
        EID_NAME => 1.05,
        EID_NECK => 0.85,
        EID_LOOK_AT => 0.75,
        EID_HEAD_TOP => 1.0,
        EID_EYE_CENTER => 0.9,
        EID_CHEST => 0.6,
        EID_R_FOOT | EID_L_FOOT => 0.05,
        EID_R_HAND | EID_L_HAND => 0.65,
        EID_HEIGHT..=EID_BODY_CENTER => 0.5,
        EID_HEAD_CENTER => 0.95,
        EID_MAGIC0..=EID_REACH_H => 0.7,
        EID_R_EYE0..=EID_CAMERA3 => 0.5,
        _ => return None,
    })
}

/// The running camera route, singular: retail's CameraManager::CurrentCameraTask is one task,
/// and CreateCameraTask deletes the prior one before installing the new.
#[derive(Resource, Default)]
pub struct CutsceneCameraTasks {
    current: Option<CutsceneCameraTask>,
    /// The last frame applied while the scene held the camera. While locked with no active
    /// task, retail's user-control-disabled operator camera stays where the finished route
    /// left it (research/XiEvents/OpCodes/0x0046.md: case 1 disables control; only case 0
    /// kills all tasks and re-seats the chase at the player), so this frame is re-applied over
    /// resolve_camera's chase writes until the lock releases.
    held: Option<CameraFrame>,
}

impl CutsceneCameraTasks {
    pub fn start(&mut self, task: CutsceneCameraTask) {
        self.current = Some(task);
    }

    pub fn is_active(&self) -> bool {
        self.current.is_some()
    }

    /// The frame held while the lock outlives its route, if any.
    pub fn held(&self) -> Option<CameraFrame> {
        self.held
    }

    /// Remember a route's applied frame so the hold can keep it after the route drains.
    pub fn set_held(&mut self, frame: CameraFrame) {
        self.held = Some(frame);
    }

    /// The running route's live attach matrix (CameraTask.cpp OnMove recomputes it from the
    /// actor's current state before each lerp); None for a world-space route or when the
    /// attach does not resolve.
    fn live_attach_matrix(
        &self,
        q_attach: &Query<
            (&Transform, Option<&BakedActor>),
            (
                With<crate::components::WorldEntity>,
                Without<crate::camera::OperatorCamera>,
            ),
        >,
        q_children: &Query<&Children>,
        q_render: &Query<&crate::ffxi_actor_render::FfxiRenderActor>,
    ) -> Option<Mat4> {
        let attach = self.current.as_ref()?.attach.as_ref()?;
        let (xform, baked) = q_attach.get(attach.actor).ok()?;
        let render = q_children
            .get(attach.actor)
            .ok()
            .and_then(|children| children.iter().find_map(|c| q_render.get(c).ok()));
        let point = eid_model_point(attach.locator, baked, render)?;
        Some(attach_matrix(xform, point))
    }

    /// Advance the running route by `dt_secs`; None when there is no route or it has run out.
    /// `live_attach` feeds the attached route's smoothed matrix (see
    /// [`CutsceneCameraTask::advance`]).
    pub fn advance(&mut self, dt_secs: f32, live_attach: Option<Mat4>) -> Option<CameraFrame> {
        let frame = self.current.as_mut()?.advance(dt_secs, live_attach);
        if self.current.as_ref().is_some_and(|t| t.done) {
            self.current = None;
        }
        frame
    }

    /// The session ended or the lock released: drop route and hold so the chase camera resumes on the next frame.
    pub fn clear(&mut self) {
        self.current = None;
        self.held = None;
    }
}

/// research/XIClient source/World/Camera/CameraManager.cpp CalculateDefaultCameraPosition -
/// the default chase state END_AT_CURRENT_POS substitutes: the eye stands
/// DEFAULT_CHASE_STANDOFF behind the actor along its facing at anchor height, looking at the
/// anchor. The focal is retail's default for the view mode (CameraTask.cpp's 280/350 split).
pub fn default_chase_endpoint(
    self_t: &Transform,
    baked: Option<&BakedActor>,
    first_person: bool,
) -> Endpoint {
    let anchor = self_t.translation + Vec3::Y * crate::camera::third_person_anchor_y(baked);
    let forward = self_t.rotation * Vec3::X;
    let behind = Vec3::new(forward.x, 0.0, forward.z).normalize_or(Vec3::NEG_Z);
    Endpoint {
        eye: anchor - behind * DEFAULT_CHASE_STANDOFF,
        target: anchor,
        roll: 0.0,
        focal_length: if first_person {
            ffxi_dat::camera::DEFAULT_FOCAL_LENGTH_FIRST_PERSON
        } else {
            ffxi_dat::camera::DEFAULT_FOCAL_LENGTH_THIRD_PERSON
        },
    }
}

/// The operator camera's current state in the units a route substitutes: eye and look-at from
/// its transform, roll 0 (Bevy cameras carry no roll outside routes), focal length inverted
/// out of the projection with retail's fixed half-height.
pub fn capture_current_camera(cam_t: &Transform, proj: &Projection) -> Option<CurrentCameraState> {
    let Projection::Perspective(p) = proj else {
        return None;
    };
    let focal = RETAIL_PROJECTION_HALF_HEIGHT / (p.fov * 0.5).tan();
    Some(CurrentCameraState {
        eye: cam_t.translation,
        target: cam_t.translation + cam_t.rotation * Vec3::NEG_Z,
        roll: 0.0,
        focal_length: focal,
    })
}

/// The camera route a cutscene scheduler routine started (research/XiEvents/OpCodes/0x0045.md):
/// advance it while the scene holds the camera and write its output onto the operator camera's
/// transform and projection. While the lock outlives the route, the last applied frame is held
/// over resolve_camera's chase writes (this system runs after it); released at CutsceneEnded or
/// DEFCAMERA case 0 like the fade, when the chase camera resumes on the frame after. With no
/// route running, the hold parks on the finished route's final frame, retail-style (event 503
/// holds its camera from +037F7 to +04683 across all its MESWAITs); a lock with no route yet
/// captures the operator state once and freezes it.
#[cfg(not(target_arch = "wasm32"))]
pub fn advance_cutscene_camera_task(
    time: Res<Time>,
    mode: Res<CutsceneMode>,
    settings: Res<GraphicsSettings>,
    events: Res<EventLog>,
    mut tasks: ResMut<CutsceneCameraTasks>,
    mut cursor: Local<u64>,
    mut logged_start: Local<bool>,
    mut q_cam: Query<(&mut Transform, &mut Projection), With<crate::camera::OperatorCamera>>,
    q_attach: Query<
        (&Transform, Option<&BakedActor>),
        (
            With<crate::components::WorldEntity>,
            Without<crate::camera::OperatorCamera>,
        ),
    >,
    q_children: Query<&Children>,
    q_render: Query<&crate::ffxi_actor_render::FfxiRenderActor>,
) {
    let total = events.pushed_total;
    let first_global = total.saturating_sub(events.recent.len() as u64);
    for g in (*cursor).max(first_global)..total {
        match &events.recent[(g - first_global) as usize] {
            kuluu_snapshot::ViewerEvent::CutsceneEnded
            | kuluu_snapshot::ViewerEvent::ZoneChanged { .. }
            | kuluu_snapshot::ViewerEvent::Disconnected { .. } => {
                tasks.clear();
                *logged_start = false;
                restore_projection(&settings, &mut q_cam);
            }
            _ => {}
        }
    }
    *cursor = total;

    if !mode.camera_locked {
        // DEFCAMERA case 0: retail kills every camera task and re-seats the chase at the player
        // (research/XiEvents/OpCodes/0x0046.md); drop route and hold, hand the focal back to the
        // settings default.
        if tasks.is_active() || tasks.held().is_some() {
            tasks.clear();
            restore_projection(&settings, &mut q_cam);
        }
        return;
    }

    let live = tasks.live_attach_matrix(&q_attach, &q_children, &q_render);
    let Some(frame) = tasks.advance(time.delta_secs(), live) else {
        let held = match tasks.held() {
            Some(held) => Some(held),
            None => q_cam
                .iter()
                .next()
                .and_then(|(cam_t, proj)| capture_current_camera(cam_t, proj))
                .map(CameraFrame::from),
        };
        if let Some(held) = held {
            tasks.set_held(held);
            apply_frame(&held, &mut q_cam);
        } else if *logged_start {
            tracing::debug!(
                target: "kuluu_render::cutscene_camera",
                "cutscene camera route finished"
            );
            *logged_start = false;
        }
        return;
    };
    if !*logged_start {
        tracing::debug!(
            target: "kuluu_render::cutscene_camera",
            eye = ?frame.eye,
            target = ?frame.target,
            focal_length = frame.focal_length,
            roll = frame.roll,
            "cutscene camera route first frame"
        );
        *logged_start = true;
    }
    tasks.set_held(frame);
    apply_frame(&frame, &mut q_cam);
}

/// One route frame onto the operator camera: eye and look-at with roll as an up-axis twist,
/// focal length inverted into the projection with retail's fixed half-height. The roll
/// rotates the up vector about the view direction: Bevy's look_at takes an up axis rather
/// than a roll angle.
fn apply_frame(
    frame: &CameraFrame,
    q_cam: &mut Query<(&mut Transform, &mut Projection), With<crate::camera::OperatorCamera>>,
) {
    for (mut cam_t, mut proj) in q_cam.iter_mut() {
        let dir = (frame.target - frame.eye).normalize_or(Vec3::NEG_Z);
        let up = Quat::from_axis_angle(dir, frame.roll) * Vec3::Y;
        cam_t.translation = frame.eye;
        cam_t.look_at(frame.target, up);
        if let Projection::Perspective(p) = proj.as_mut() {
            p.fov = 2.0 * (RETAIL_PROJECTION_HALF_HEIGHT / frame.focal_length).atan();
        }
    }
}

fn restore_projection(
    settings: &GraphicsSettings,
    q_cam: &mut Query<(&mut Transform, &mut Projection), With<crate::camera::OperatorCamera>>,
) {
    for (_cam_t, mut proj) in q_cam.iter_mut() {
        if let Projection::Perspective(p) = proj.as_mut() {
            p.fov = settings.fov_deg.to_radians();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(position: [f32; 3], focal: f32, target: [f32; 3], roll: f32) -> CameraControlPoint {
        CameraControlPoint {
            position,
            focal_length: focal,
            target,
            roll,
            param: [0.0; 3],
        }
    }

    fn resource(
        smoothing: CameraSmoothType,
        flags: u16,
        points: Vec<CameraControlPoint>,
    ) -> CameraResource {
        CameraResource {
            name: *b"tst1",
            attachment_info: 0,
            interp_factor: 0,
            flags: ffxi_dat::camera::CameraFlags::from_u16(flags),
            smoothing,
            points,
        }
    }

    fn current_state() -> CurrentCameraState {
        CurrentCameraState {
            eye: Vec3::new(1.0, 2.0, 3.0),
            target: Vec3::new(4.0, 5.0, 6.0),
            roll: 0.0,
            focal_length: 350.0,
        }
    }

    fn default_chase() -> Endpoint {
        Endpoint {
            eye: Vec3::new(-2.0, 1.0, 0.0),
            target: Vec3::new(1.0, 1.0, 0.0),
            roll: 0.0,
            focal_length: 350.0,
        }
    }

    #[test]
    fn progression_curves_hit_their_endpoints_and_midpoint() {
        // Every curve maps 0 -> 0 and 1 -> 1 (CameraTask.cpp Smooth).
        for smoothing in [
            CameraSmoothType::Linear,
            CameraSmoothType::Decelerate,
            CameraSmoothType::Accelerate,
            CameraSmoothType::DecelerateToMidpointThenAccelerate,
            CameraSmoothType::AccelerateAndDecelerate,
        ] {
            assert!(
                (progression_curve(smoothing, 0.0)).abs() < 1e-6,
                "{smoothing:?} at 0"
            );
            assert!(
                (progression_curve(smoothing, 1.0) - 1.0).abs() < 1e-5,
                "{smoothing:?} at 1"
            );
        }

        // The midpoint shapes: decelerate leads, accelerate lags, the two compound curves sit
        // exactly on 0.5 by symmetry (CameraTask.cpp Smooth's sin/cos forms).
        assert!(progression_curve(CameraSmoothType::Decelerate, 0.5) > 0.7);
        assert!(progression_curve(CameraSmoothType::Accelerate, 0.5) < 0.3);
        assert!(
            (progression_curve(CameraSmoothType::DecelerateToMidpointThenAccelerate, 0.5) - 0.5)
                .abs()
                < 1e-6
        );
        assert!(
            (progression_curve(CameraSmoothType::AccelerateAndDecelerate, 0.5) - 0.5).abs() < 1e-6
        );

        assert_eq!(
            progression_curve(CameraSmoothType::Keyframe(0x4B), 0.25),
            0.25,
            "keyframe falls back to linear time"
        );
    }

    #[test]
    fn straight_path_interpolates_the_midpoint() {
        let mut task = CutsceneCameraTask::start(
            &resource(
                CameraSmoothType::Linear,
                0,
                vec![
                    point([0.0; 3], 280.0, [0.0, 1.0, 0.0], 0.0),
                    point([4.0, 0.0, 0.0], 682.0, [4.0, 1.0, 0.0], 0.5),
                ],
            ),
            100.0,
            current_state(),
            default_chase(),
            None,
        );

        let frame = task
            .advance(50.0 / crate::scheduler_runtime::ROUTINE_FPS, None)
            .unwrap();
        assert!((frame.eye - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-4);
        assert!((frame.target - Vec3::new(2.0, -1.0, 0.0)).length() < 1e-4);
        assert!((frame.focal_length - 481.0).abs() < 1e-2);
        assert!((frame.roll - 0.25).abs() < 1e-5);

        let last = task
            .advance(50.0 / crate::scheduler_runtime::ROUTINE_FPS, None)
            .unwrap();
        assert!(
            (last.eye - Vec3::new(4.0, 0.0, 0.0)).length() < 1e-4,
            "last frame on the end point"
        );
        assert!(task
            .advance(1.0 / crate::scheduler_runtime::ROUTINE_FPS, None)
            .is_none());
    }

    #[test]
    fn start_at_current_pos_substitutes_the_camera_state() {
        let mut task = CutsceneCameraTask::start(
            &resource(
                CameraSmoothType::Linear,
                ffxi_dat::camera::CameraFlags::START_AT_CURRENT_POS,
                vec![point([4.0, 0.0, 0.0], 350.0, [4.0, 1.0, 0.0], 0.0)],
            ),
            60.0,
            current_state(),
            default_chase(),
            None,
        );

        let frame = task
            .advance(1.0 / crate::scheduler_runtime::ROUTINE_FPS, None)
            .unwrap();
        assert!(
            (frame.eye - Vec3::new(1.0, 2.0, 3.0)).length() < 0.5,
            "frame zero starts on the camera's own state"
        );
    }

    #[test]
    fn end_at_current_pos_substitutes_the_default_chase_state() {
        let mut task = CutsceneCameraTask::start(
            &resource(
                CameraSmoothType::Linear,
                ffxi_dat::camera::CameraFlags::END_AT_CURRENT_POS,
                vec![point([0.0; 3], 280.0, [0.0, 1.0, 0.0], 0.0)],
            ),
            60.0,
            current_state(),
            default_chase(),
            None,
        );

        let dt = 1.0 / crate::scheduler_runtime::ROUTINE_FPS;
        let mut last: Option<CameraFrame> = None;
        for _ in 0..60 {
            last = Some(
                task.advance(dt, None)
                    .expect("route runs its full duration"),
            );
        }
        assert!(task.advance(dt, None).is_none());
        let last = last.expect("the final frame was emitted");
        assert!(
            (last.eye - default_chase().eye).length() < 1e-3,
            "end eye {last:?}"
        );
    }

    fn attach_start(actor: Entity, locator: u32, interp: f32, initial_matrix: Mat4) -> AttachStart {
        AttachStart {
            actor,
            locator,
            interp,
            initial_matrix,
        }
    }

    #[test]
    fn an_attached_route_transforms_through_the_attach_matrix() {
        let mut task = CutsceneCameraTask::start(
            &resource(
                CameraSmoothType::Linear,
                0,
                vec![point([0.0; 3], 280.0, [0.0; 3], 0.0)],
            ),
            60.0,
            current_state(),
            default_chase(),
            Some(attach_start(
                Entity::PLACEHOLDER,
                21,
                0.0,
                Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0)),
            )),
        );

        let frame = task
            .advance(1.0 / crate::scheduler_runtime::ROUTINE_FPS, None)
            .unwrap();
        assert!(
            (frame.eye - Vec3::new(10.0, 0.0, 0.0)).length() < 1e-5,
            "the locked point is local to the attach actor; the frame transforms out through the attach matrix"
        );
    }

    /// The model's +X is the actor's facing; a 90-degree yaw turns it toward -Z in Bevy's
    /// right-handed Y-up space. The EID point lands at the origin plus the yaw-rotated facing
    /// offset; a local +X step keeps riding the facing.
    #[test]
    fn the_attach_matrix_carries_the_actor_origin_and_yaw() {
        let actor = Transform::from_xyz(5.0, 0.0, 0.0)
            * Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        let m = attach_matrix(&actor, Vec3::new(1.0, 0.0, 0.0));
        assert!((m.transform_point3(Vec3::ZERO) - Vec3::new(5.0, 0.0, -1.0)).length() < 1e-5);
        assert!((m.transform_point3(Vec3::X) - Vec3::new(5.0, 0.0, -2.0)).length() < 1e-5);
    }

    #[test]
    fn a_zero_interp_freezes_the_start_matrix() {
        let mut task = CutsceneCameraTask::start(
            &resource(
                CameraSmoothType::Linear,
                0,
                vec![point([0.0; 3], 280.0, [0.0; 3], 0.0)],
            ),
            60.0,
            current_state(),
            default_chase(),
            Some(attach_start(
                Entity::PLACEHOLDER,
                21,
                0.0,
                Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0)),
            )),
        );

        let live = Mat4::from_translation(Vec3::new(99.0, 0.0, 0.0));
        for _ in 0..3 {
            let frame = task
                .advance(1.0 / crate::scheduler_runtime::ROUTINE_FPS, Some(live))
                .unwrap();
            assert!(
                (frame.eye - Vec3::new(10.0, 0.0, 0.0)).length() < 1e-5,
                "the frozen start matrix wins over the live matrix"
            );
        }
    }

    #[test]
    fn the_interp_factor_steps_the_smoothed_matrix_toward_the_live_one() {
        let start = Mat4::IDENTITY;
        let live = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0));
        let dt = 1.0 / crate::scheduler_runtime::ROUTINE_FPS;

        let mut task = CutsceneCameraTask::start(
            &resource(
                CameraSmoothType::Linear,
                0,
                vec![point([0.0; 3], 280.0, [0.0; 3], 0.0)],
            ),
            60.0,
            current_state(),
            default_chase(),
            Some(attach_start(Entity::PLACEHOLDER, 21, 1.0, start)),
        );
        let frame = task.advance(dt, Some(live)).unwrap();
        assert!(
            (frame.eye - Vec3::new(10.0, 0.0, 0.0)).length() < 1e-5,
            "a full interp jumps to the live matrix in one frame"
        );

        let mut task = CutsceneCameraTask::start(
            &resource(
                CameraSmoothType::Linear,
                0,
                vec![point([0.0; 3], 280.0, [0.0; 3], 0.0)],
            ),
            60.0,
            current_state(),
            default_chase(),
            Some(attach_start(Entity::PLACEHOLDER, 21, 0.5, start)),
        );
        let frame = task.advance(dt, Some(live)).unwrap();
        assert!(
            (frame.eye - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-5,
            "a half interp lands halfway on the first step"
        );
    }

    #[test]
    fn a_start_at_current_pos_maps_the_camera_state_into_the_local_space() {
        let initial = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0));
        let mut task = CutsceneCameraTask::start(
            &resource(
                CameraSmoothType::Linear,
                ffxi_dat::camera::CameraFlags::START_AT_CURRENT_POS,
                vec![point([0.0; 3], 280.0, [0.0; 3], 0.0)],
            ),
            60.0,
            current_state(),
            default_chase(),
            Some(attach_start(Entity::PLACEHOLDER, 21, 0.0, initial)),
        );

        let frame = task
            .advance(1.0 / crate::scheduler_runtime::ROUTINE_FPS, None)
            .unwrap();
        assert!(
            (frame.eye - current_state().eye).length() < 0.5,
            "the eye round-trips to the camera's own state through the local space"
        );
    }

    /// EID_NAME stands above the head, EID_BODY_CENTER at half height, EID_CURRENT on the
    /// ground; the special locators and untabled empties stay unresolvable, and without a
    /// baked actor the fallback height stands in.
    #[test]
    fn the_eid_fallback_reads_the_named_points_off_the_actor_height() {
        let baked = BakedActor {
            min_mesh_y: 0.0,
            actor_height: 2.0,
        };
        assert!(
            (eid_model_point(2, Some(&baked), None).unwrap() - Vec3::new(0.0, 2.1, 0.0)).length()
                < 1e-5
        );
        assert!(
            (eid_model_point(21, Some(&baked), None).unwrap() - Vec3::new(0.0, 1.0, 0.0)).length()
                < 1e-5
        );
        assert_eq!(eid_model_point(0, Some(&baked), None), Some(Vec3::ZERO));
        assert!(eid_model_point(48, Some(&baked), None).is_none());
        assert!(eid_model_point(44, Some(&baked), None).is_none());
        assert!(
            (eid_model_point(2, None, None).unwrap()
                - Vec3::new(0.0, 1.05 * crate::camera::FALLBACK_ACTOR_HEIGHT, 0.0))
            .length()
                < 1e-5
        );
    }

    #[test]
    fn the_eid_point_reads_the_skeleton_reference_table() {
        let skeleton = ffxi_dat::skel::Skeleton {
            id: ffxi_dat::datid::DatId::from_str("0000"),
            joints: vec![ffxi_dat::skel::Joint {
                rotation: [0.0, 0.0, 0.0, 1.0],
                translation: [0.0; 3],
                parent: None,
            }],
            references: (0..ffxi_dat::camera::EID_NORMAL_MAX)
                .map(|i| ffxi_dat::skel::JointReference {
                    index: 0,
                    unk_v0: [0.0; 3],
                    position_offset: [0.0, i as f32, 0.0],
                })
                .collect(),
            bounding_boxes: Vec::new(),
        };
        let pose = ffxi_actor::skeleton_instance::pose_world(
            &skeleton,
            |_| None,
            ffxi_actor::skeleton_instance::RootTransform::identity(),
            &[],
        );
        let render = crate::ffxi_actor_render::render_actor_for_test(skeleton, pose);
        assert!(
            (eid_model_point(7, None, Some(&render)).unwrap() - Vec3::new(0.0, 7.0, 0.0)).length()
                < 1e-4
        );
    }

    #[test]
    fn a_locked_route_holds_its_single_point() {
        let mut task = CutsceneCameraTask::start(
            &resource(
                CameraSmoothType::Linear,
                0,
                vec![point([7.0, 8.0, 9.0], 500.0, [1.0, 2.0, 3.0], 0.1)],
            ),
            60.0,
            current_state(),
            default_chase(),
            None,
        );

        for _ in 0..3 {
            let frame = task
                .advance(1.0 / crate::scheduler_runtime::ROUTINE_FPS, None)
                .unwrap();
            assert!((frame.eye - Vec3::new(7.0, -8.0, -9.0)).length() < 1e-5);
            assert!((frame.focal_length - 500.0).abs() < 1e-4);
        }
    }

    #[test]
    fn a_zero_duration_locked_route_is_a_hard_cut() {
        let mut task = CutsceneCameraTask::start(
            &resource(
                CameraSmoothType::Linear,
                0,
                vec![point([7.0; 3], 500.0, [1.0; 3], 0.0)],
            ),
            0.0,
            current_state(),
            default_chase(),
            None,
        );

        let frame = task
            .advance(1.0 / crate::scheduler_runtime::ROUTINE_FPS, None)
            .unwrap();
        assert!((frame.eye - Vec3::new(7.0, -7.0, -7.0)).length() < 1e-5);
        assert!(task
            .advance(1.0 / crate::scheduler_runtime::ROUTINE_FPS, None)
            .is_none());
    }

    #[test]
    fn spline_tracks_pass_through_their_control_points() {
        // A three-point spline must sit exactly on its control points at t = 0, 0.5 and 1: the
        // Catmull-Rom basis interpolates, it does not merely approximate (Spline.cpp). The
        // chords are equal so chordal parameterization puts the middle point at t = 0.5.
        let mut task = CutsceneCameraTask::start(
            &resource(
                CameraSmoothType::Linear,
                0,
                vec![
                    point([0.0; 3], 280.0, [0.0; 3], 0.0),
                    point([1.0, 2.0, 3.0], 500.0, [4.0, 5.0, 6.0], 0.1),
                    point([2.0, 4.0, 6.0], 682.0, [7.0, 8.0, 9.0], 0.2),
                ],
            ),
            300.0,
            current_state(),
            default_chase(),
            None,
        );

        let dt = 1.0 / crate::scheduler_runtime::ROUTINE_FPS;
        let first = task.advance(dt, None).unwrap();
        assert!(
            (first.eye - Vec3::ZERO).length() < 0.03,
            "t~0 on the first point: {first:?}"
        );

        for _ in 0..148 {
            let _ = task.advance(dt, None);
        }
        let mid = task.advance(dt, None).unwrap();
        assert!(
            (mid.eye - Vec3::new(1.0, -2.0, -3.0)).length() < 1e-3,
            "t=0.5 on the middle point: {mid:?}"
        );

        for _ in 0..149 {
            let _ = task.advance(dt, None);
        }
        let last = task.advance(dt, None).unwrap();
        assert!(
            (last.eye - Vec3::new(2.0, -4.0, -6.0)).length() < 1e-3,
            "t=1 on the last point: {last:?}"
        );
        assert!(task.advance(dt, None).is_none());
    }

    #[test]
    fn path_mode_selection_follows_the_effective_point_count() {
        let res = resource(
            CameraSmoothType::Linear,
            ffxi_dat::camera::CameraFlags::START_AT_CURRENT_POS
                | ffxi_dat::camera::CameraFlags::END_AT_CURRENT_POS,
            vec![point([0.0; 3], 280.0, [0.0; 3], 0.0)],
        );
        assert_eq!(res.path_mode(), CameraPathMode::Spline);

        let mut task =
            CutsceneCameraTask::start(&res, 60.0, current_state(), default_chase(), None);
        for _ in 0..70 {
            if task
                .advance(1.0 / crate::scheduler_runtime::ROUTINE_FPS, None)
                .is_none()
            {
                break;
            }
        }
    }

    /// The kind 0x06 points are retail world coordinates (Y up); Bevy's up axis is
    /// -native-y, so the vertical and horizontal-z components both negate (camera.rs).
    #[test]
    fn dat_points_convert_to_bevy_space() {
        let p = point([1.0, 2.0, 3.0], 500.0, [4.0, 5.0, 6.0], 0.0);
        let e = Endpoint::from(&p);
        assert_eq!(e.eye, Vec3::new(1.0, -2.0, -3.0));
        assert_eq!(e.target, Vec3::new(4.0, -5.0, -6.0));
    }

    #[test]
    fn the_focal_to_fov_conversion_matches_retail() {
        // GameManager.cpp UpdateProjectionMatrix: fovy = 2 * atan(192 / focal), and the
        // settings default is exactly that at retail's 350 default focal.
        let fov = 2.0 * (RETAIL_PROJECTION_HALF_HEIGHT / 350.0).atan();
        assert!(
            (fov.to_degrees() - crate::graphics_settings::retail_default_fov_deg()).abs() < 1e-4
        );
    }

    /// Simulates resolve_camera, which rewrites the operator eye every frame in Chase mode:
    /// whatever the hold does not overwrite this frame is lost.
    fn chase_steal(mut q_cam: Query<&mut Transform, With<crate::camera::OperatorCamera>>) {
        for mut cam_t in q_cam.iter_mut() {
            cam_t.translation = Vec3::new(99.0, 50.0, -99.0);
            cam_t.look_at(Vec3::ZERO, Vec3::Y);
        }
    }

    fn hold_app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<EventLog>()
            .insert_resource(CutsceneMode {
                camera_locked: true,
                ..Default::default()
            })
            .insert_resource(GraphicsSettings::default())
            .init_resource::<CutsceneCameraTasks>();
        app.world_mut().spawn((
            crate::camera::OperatorCamera,
            Transform::from_xyz(0.0, 1.0, -3.0).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
            Projection::Perspective(crate::PerspectiveProjection {
                fov: crate::graphics_settings::retail_default_fov_deg().to_radians(),
                ..Default::default()
            }),
        ));
        app.add_systems(Update, (chase_steal, advance_cutscene_camera_task).chain());
        app
    }

    fn hold_step(app: &mut App, frames: u32) {
        for _ in 0..frames {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(
                    1.0 / crate::scheduler_runtime::ROUTINE_FPS,
                ));
            app.update();
        }
    }

    fn hold_cam(app: &mut App) -> (Vec3, f32) {
        let mut q = app
            .world_mut()
            .query_filtered::<(&Transform, &Projection), With<crate::camera::OperatorCamera>>();
        let (t, p) = q.single(app.world()).expect("one operator camera");
        let Projection::Perspective(p) = p else {
            panic!("perspective projection")
        };
        (t.translation, p.fov)
    }

    /// Event 503's shape: DEFCAMERA case 1 at +037F7, camera routes throughout, MESWAITs
    /// between them, case 0 only at +04683. A route that drains mid-MESWAIT parks on its
    /// final frame instead of handing the eye back to the chase.
    #[test]
    fn a_finished_route_holds_its_last_frame_while_the_lock_outlives_it() {
        let mut app = hold_app();
        let task = CutsceneCameraTask::start(
            &resource(
                CameraSmoothType::Linear,
                0,
                vec![
                    point([0.0; 3], 280.0, [0.0, 1.0, 0.0], 0.0),
                    point([4.0, 0.0, 0.0], 682.0, [4.0, 1.0, 0.0], 0.5),
                ],
            ),
            60.0,
            current_state(),
            default_chase(),
            None,
        );
        app.world_mut()
            .resource_mut::<CutsceneCameraTasks>()
            .start(task);

        hold_step(&mut app, 60);
        let (last_eye, last_fov) = hold_cam(&mut app);
        assert!(
            (last_eye - Vec3::new(4.0, 0.0, 0.0)).length() < 1e-3,
            "route end: {last_eye:?}"
        );

        hold_step(&mut app, 120);
        let (held_eye, held_fov) = hold_cam(&mut app);
        assert!(
            (held_eye - last_eye).length() < 1e-4,
            "hold lost to the chase: {held_eye:?}"
        );
        assert!(
            (held_fov - last_fov).abs() < 1e-5,
            "focal drifted while holding"
        );

        app.world_mut().resource_mut::<CutsceneMode>().camera_locked = false;
        hold_step(&mut app, 2);
        let tasks = app.world().resource::<CutsceneCameraTasks>();
        assert!(
            !tasks.is_active() && tasks.held().is_none(),
            "hold survived the release"
        );
        let (_, fov) = hold_cam(&mut app);
        let default_fov = crate::graphics_settings::retail_default_fov_deg().to_radians();
        assert!(
            (fov - default_fov).abs() < 1e-5,
            "focal not restored: {fov}"
        );
    }

    /// A lock with no route yet (event 503's first MESWAIT at +03846 precedes its camera
    /// routines): retail's user-control-disabled camera stays put, so the operator state is
    /// captured once and held over every chase write.
    #[test]
    fn a_lock_before_any_route_freezes_the_operator_state() {
        let mut app = hold_app();
        hold_step(&mut app, 3);
        let (first_eye, _) = hold_cam(&mut app);
        hold_step(&mut app, 60);
        let (frozen_eye, _) = hold_cam(&mut app);
        assert!(
            (frozen_eye - first_eye).length() < 1e-4,
            "drifted while locked with no route: {frozen_eye:?}"
        );
    }

    /// An attached route re-resolves the attach matrix from the actor's wire transform each
    /// frame; a full interp follows every move. EID_BODY_CENTER is half the actor's height in
    /// the model frame; mzb_to_bevy maps the model's up to Bevy -Y, so the eye parks one unit
    /// below the actor's origin.
    #[test]
    fn an_attached_route_tracks_the_actor_as_it_moves() {
        let mut app = hold_app();
        let actor = app
            .world_mut()
            .spawn((
                crate::components::WorldEntity {
                    id: 7,
                    act_index: 0,
                    kind: kuluu_snapshot::EntityKind::Mob,
                },
                Transform::from_xyz(10.0, 0.0, 0.0),
                BakedActor {
                    min_mesh_y: 0.0,
                    actor_height: 2.0,
                },
            ))
            .with_children(|parent| {
                parent.spawn(crate::components::InGameEntity);
            })
            .id();
        let task = CutsceneCameraTask::start(
            &resource(
                CameraSmoothType::Linear,
                0,
                vec![point([0.0; 3], 280.0, [0.0; 3], 0.0)],
            ),
            300.0,
            current_state(),
            default_chase(),
            Some(attach_start(
                actor,
                21,
                1.0,
                attach_matrix(
                    &Transform::from_xyz(10.0, 0.0, 0.0),
                    Vec3::new(0.0, 1.0, 0.0),
                ),
            )),
        );
        app.world_mut()
            .resource_mut::<CutsceneCameraTasks>()
            .start(task);

        hold_step(&mut app, 2);
        let (eye, _) = hold_cam(&mut app);
        assert!(
            (eye - Vec3::new(10.0, -1.0, 0.0)).length() < 1e-3,
            "parked: {eye:?}"
        );

        {
            let mut entity = app.world_mut().entity_mut(actor);
            let mut actor_t = entity.get_mut::<Transform>().unwrap();
            actor_t.translation = Vec3::new(30.0, 0.0, 0.0);
        }
        hold_step(&mut app, 2);
        let (eye, _) = hold_cam(&mut app);
        assert!(
            (eye - Vec3::new(30.0, -1.0, 0.0)).length() < 1e-3,
            "tracked: {eye:?}"
        );
    }
}
