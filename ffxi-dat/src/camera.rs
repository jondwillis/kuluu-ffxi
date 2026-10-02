//! Kind 0x06 camera route chunks (research/XIClient include/World/Camera/CameraFormat.h):
//! the paths a scheduler routine drives the camera along. A routine's stage 0x04 names one of
//! these by four-char id and plays it for the stage's scaled duration (research/XIClient
//! Game/Scheduler/Tags/0x04.cpp HandleTag0x04 looks the resource up by that name and calls
//! CameraResource::CreateCameraTask).

use crate::{DatError, Result};

// research/XIClient include/World/Camera/CameraFormat.h CameraAttachmentHeader - after the
// 16-byte chunk header comes AttachmentInfo plus three fields the client overwrites at load.
const ATTACHMENT_INFO_OFFSET: usize = 0;
const CAMERA_HEADER_OFFSET: usize = 16;

// research/XIClient include/World/Camera/CameraFormat.h CameraHeader - packed, right after the
// attachment header.
const CONTROL_POINT_COUNT_OFFSET: usize = CAMERA_HEADER_OFFSET;
const INTERP_FACTOR_OFFSET: usize = CAMERA_HEADER_OFFSET + 1;
const FLAGS_OFFSET: usize = CAMERA_HEADER_OFFSET + 2;
const SMOOTHING_TYPE_OFFSET: usize = CAMERA_HEADER_OFFSET + 4;
// research/XIClient include/World/Camera/CameraFormat.h CameraHeader - after SmoothingType
// the header carries a KeyframeResource pointer and an int the client fills at load; both
// are zero in every shipped byte.
pub const POINTS_OFFSET: usize = CAMERA_HEADER_OFFSET + 16;

// research/XIClient include/World/Camera/CameraFormat.h SplineControlPoint - packed, 48 bytes.
pub const CONTROL_POINT_LEN: usize = 48;
const POSITION_OFFSET: usize = 0;
const FOCAL_LENGTH_OFFSET: usize = 12;
const TARGET_OFFSET: usize = 16;
const ROLL_OFFSET: usize = 28;
const PARAM_OFFSET: usize = 32;

// research/XIClient source/World/Camera/CameraTask.cpp - the focal length a straight path's
// synthesized end point takes when END_AT_CURRENT_POS lands with no current value to carry.
pub const DEFAULT_FOCAL_LENGTH_FIRST_PERSON: f32 = 280.0;
pub const DEFAULT_FOCAL_LENGTH_THIRD_PERSON: f32 = 350.0;

// research/XIClient source/World/Actor/Attachment.cpp GetAttachMode - the mode field of
// AttachmentInfo: the low nibble, bit 16 extending the high group.
pub const ATTACH_MODE_MASK: u32 = 0xF;
pub const ATTACH_MODE_EXT_SHIFT: u32 = 16;
// research/XIClient source/World/Actor/Attachment.cpp MakeAttachMatrix - the attach modes
// this tree resolves: mode 1 rides the caster's EID locator, mode 3 (the source-to-target
// basis, research/xim ParticleGeneratorAttachment.kt SourceToTargetBasis) resolves against
// the caster the same way, and every other mode plays the route in world space (mode 0's
// identity default arm; the rest the decompilation leaves identity).
pub const ATTACH_MODE_WORLD: u32 = 0;
pub const ATTACH_MODE_CASTER: u32 = 1;
pub const ATTACH_MODE_SOURCE_TO_TARGET: u32 = 3;
// research/XIClient source/World/Actor/Attachment.cpp MakeAttachMatrix mode1 - the locator
// field of AttachmentInfo: bits 4..9, bit 18 extending the high group.
pub const ATTACH_LOCATOR_SHIFT: u32 = 4;
pub const ATTACH_LOCATOR_MASK: u32 = 0x3F;
pub const ATTACH_LOCATOR_BITS: u32 = 6;
pub const ATTACH_LOCATOR_EXT_SHIFT: u32 = 18;
// research/XIClient include/World/Actor/EID_INDEX.h EID_NORMAL_MAX - the first special
// locator (EID_GROUND and on); the index below it reads the skeleton's reference table.
pub const EID_NORMAL_MAX: u32 = 48;
// research/XIClient source/World/Camera/CameraTask.cpp OnMove - the InterpFactor byte is a
// fraction of this: each frame the smoothed attach matrix steps toward the live one by
// InterpFactor / INTERP_FACTOR_SCALE.
pub const INTERP_FACTOR_SCALE: f32 = 255.0;

/// The SmoothingType field of a camera header (research/XIClient include/World/Camera/
/// CameraFormat.h CameraSmoothType). Values above the five curves are keyframe resource
/// FourCCs retail resolves at load time (CameraTask.cpp EvaluateProgressionCurve default arm);
/// the shipped install carries six of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraSmoothType {
    Linear,
    Decelerate,
    Accelerate,
    DecelerateToMidpointThenAccelerate,
    AccelerateAndDecelerate,
    /// A keyframe resource FourCC; the curve is not in this file.
    Keyframe(u32),
}

impl CameraSmoothType {
    pub fn from_u32(raw: u32) -> Self {
        match raw {
            0 => Self::Linear,
            1 => Self::Decelerate,
            2 => Self::Accelerate,
            3 => Self::DecelerateToMidpointThenAccelerate,
            4 => Self::AccelerateAndDecelerate,
            other => Self::Keyframe(other),
        }
    }
}

/// The Flags field of a camera header (research/XIClient include/World/Camera/CameraFormat.h
/// CameraFlags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CameraFlags(u16);

impl CameraFlags {
    /// The first endpoint is the camera's current state, transformed into the route's space.
    pub const START_AT_CURRENT_POS: u16 = 1 << 3;
    /// The last endpoint is the default (chase) camera state at the end of the task.
    pub const END_AT_CURRENT_POS: u16 = 1 << 4;
    /// research/XIClient names the bit SOMETHING; EvaluateProgressionCurve picks the keyframe
    /// spline value over the plain frame value when it is set.
    pub const SOMETHING: u16 = 1 << 5;

    pub fn from_u16(raw: u16) -> Self {
        Self(raw)
    }

    pub fn raw(self) -> u16 {
        self.0
    }

    pub fn starts_at_current_pos(self) -> bool {
        self.0 & Self::START_AT_CURRENT_POS != 0
    }

    pub fn ends_at_current_pos(self) -> bool {
        self.0 & Self::END_AT_CURRENT_POS != 0
    }
}

/// How the camera travels between its points (research/XIClient include/World/Camera/
/// CameraFormat.h CameraPathMode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraPathMode {
    /// One effective point: apply it directly.
    Locked,
    /// Two effective points: interpolate between them.
    Straight,
    /// More than two: the spline through all of them.
    Spline,
}

/// One control point of a camera route (research/XIClient include/World/Camera/CameraFormat.h
/// SplineControlPoint).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraControlPoint {
    pub position: [f32; 3],
    /// FovCalculationParameter: the projection focal length this point holds.
    pub focal_length: f32,
    pub target: [f32; 3],
    pub roll: f32,
    /// Param.x is the normalized time of the point along the path in the shipped data.
    pub param: [f32; 3],
}

/// A parsed `ChunkKind::Camera` chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraResource {
    pub name: [u8; 4],
    /// AttachmentInfo (research/XIClient include/World/Camera/CameraFormat.h
    /// CameraAttachmentHeader field_00): zero is world space, nonzero means the points are
    /// relative to the caster/target attach matrix.
    pub attachment_info: u32,
    pub interp_factor: u8,
    pub flags: CameraFlags,
    pub smoothing: CameraSmoothType,
    pub points: Vec<CameraControlPoint>,
}

fn read_f32(body: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(
        body[offset..offset + 4]
            .try_into()
            .expect("checked by the caller"),
    )
}

impl CameraResource {
    pub fn parse(name: [u8; 4], body: &[u8]) -> Result<Self> {
        if body.len() < POINTS_OFFSET {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: POINTS_OFFSET,
                available: body.len(),
            });
        }
        let attachment_info = u32::from_le_bytes(
            body[ATTACHMENT_INFO_OFFSET..ATTACHMENT_INFO_OFFSET + 4]
                .try_into()
                .expect("fixed offset"),
        );
        let count = body[CONTROL_POINT_COUNT_OFFSET] as usize;
        let interp_factor = body[INTERP_FACTOR_OFFSET];
        let flags = CameraFlags::from_u16(u16::from_le_bytes([
            body[FLAGS_OFFSET],
            body[FLAGS_OFFSET + 1],
        ]));
        let smoothing = CameraSmoothType::from_u32(u32::from_le_bytes([
            body[SMOOTHING_TYPE_OFFSET],
            body[SMOOTHING_TYPE_OFFSET + 1],
            body[SMOOTHING_TYPE_OFFSET + 2],
            body[SMOOTHING_TYPE_OFFSET + 3],
        ]));
        let needed = POINTS_OFFSET + count * CONTROL_POINT_LEN;
        if body.len() < needed {
            return Err(DatError::TruncatedChunk {
                offset: POINTS_OFFSET,
                needed,
                available: body.len(),
            });
        }
        let mut points = Vec::with_capacity(count);
        for i in 0..count {
            let base = POINTS_OFFSET + i * CONTROL_POINT_LEN;
            points.push(CameraControlPoint {
                position: [
                    read_f32(body, base + POSITION_OFFSET),
                    read_f32(body, base + POSITION_OFFSET + 4),
                    read_f32(body, base + POSITION_OFFSET + 8),
                ],
                focal_length: read_f32(body, base + FOCAL_LENGTH_OFFSET),
                target: [
                    read_f32(body, base + TARGET_OFFSET),
                    read_f32(body, base + TARGET_OFFSET + 4),
                    read_f32(body, base + TARGET_OFFSET + 8),
                ],
                roll: read_f32(body, base + ROLL_OFFSET),
                param: [
                    read_f32(body, base + PARAM_OFFSET),
                    read_f32(body, base + PARAM_OFFSET + 4),
                    read_f32(body, base + PARAM_OFFSET + 8),
                ],
            });
        }
        Ok(Self {
            name,
            attachment_info,
            interp_factor,
            flags,
            smoothing,
            points,
        })
    }

    /// The effective point count driving the path: the authored count plus one virtual
    /// endpoint per flag that substitutes the camera's current state (research/XIClient
    /// include/World/Camera/CameraFormat.h CameraHeader::CalculateControlPointCount).
    pub fn control_point_count(&self) -> u32 {
        let mut n = self.points.len() as u32;
        if self.flags.starts_at_current_pos() {
            n += 1;
        }
        if self.flags.ends_at_current_pos() {
            n += 1;
        }
        n
    }

    /// research/XIClient include/World/Camera/CameraFormat.cpp GetPathMode: more than two
    /// effective points is a spline, exactly two is straight, else locked.
    pub fn path_mode(&self) -> CameraPathMode {
        match self.control_point_count() {
            0..=1 => CameraPathMode::Locked,
            2 => CameraPathMode::Straight,
            _ => CameraPathMode::Spline,
        }
    }

    /// research/XIClient source/World/Actor/Attachment.cpp GetAttachMode: the low nibble of
    /// AttachmentInfo plus bit 16 select one of the attach modes; zero and the unhandled
    /// modes leave the attach matrix identity (world space).
    pub fn attach_mode(&self) -> u32 {
        (self.attachment_info & ATTACH_MODE_MASK)
            + 16 * ((self.attachment_info >> ATTACH_MODE_EXT_SHIFT) & 1)
    }

    /// research/XIClient source/World/Actor/Attachment.cpp MakeAttachMatrix mode1: the
    /// EID_INDEX locator read off the caster's skeleton - bits 4..9, bit 18 extending the
    /// high group (EID_AF_START0 and on).
    pub fn attach_locator_index(&self) -> u32 {
        ((self.attachment_info >> ATTACH_LOCATOR_SHIFT) & ATTACH_LOCATOR_MASK)
            + (((self.attachment_info >> ATTACH_LOCATOR_EXT_SHIFT) & 1) << ATTACH_LOCATOR_BITS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point_bytes(
        position: [f32; 3],
        focal: f32,
        target: [f32; 3],
        roll: f32,
        param: [f32; 3],
    ) -> Vec<u8> {
        let mut b = Vec::new();
        for v in position {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&focal.to_le_bytes());
        for v in target {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&roll.to_le_bytes());
        for v in param {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&0.0f32.to_le_bytes());
        assert_eq!(b.len(), CONTROL_POINT_LEN);
        b
    }

    fn chunk_body(
        attachment_info: u32,
        interp_factor: u8,
        flags: u16,
        smoothing: u32,
        points: &[Vec<u8>],
    ) -> Vec<u8> {
        let mut b = vec![0u8; POINTS_OFFSET];
        b[ATTACHMENT_INFO_OFFSET..ATTACHMENT_INFO_OFFSET + 4]
            .copy_from_slice(&attachment_info.to_le_bytes());
        b[CONTROL_POINT_COUNT_OFFSET] = points.len() as u8;
        b[INTERP_FACTOR_OFFSET] = interp_factor;
        b[FLAGS_OFFSET..FLAGS_OFFSET + 2].copy_from_slice(&flags.to_le_bytes());
        b[SMOOTHING_TYPE_OFFSET..SMOOTHING_TYPE_OFFSET + 4]
            .copy_from_slice(&smoothing.to_le_bytes());
        for p in points {
            b.extend_from_slice(p);
        }
        b
    }

    #[test]
    fn decodes_every_field_of_a_two_point_route() {
        let body = chunk_body(
            0,
            17,
            CameraFlags::START_AT_CURRENT_POS,
            4,
            &[
                point_bytes(
                    [1.0, 2.0, 3.0],
                    500.0,
                    [4.0, 5.0, 6.0],
                    0.5,
                    [0.0, 9.0, 8.0],
                ),
                point_bytes(
                    [-1.0, -2.0, -3.0],
                    682.0,
                    [-4.0, -5.0, -6.0],
                    -0.25,
                    [1.0, 0.0, 0.0],
                ),
            ],
        );
        let cam = CameraResource::parse(*b"c043", &body).unwrap();
        assert_eq!(cam.name, *b"c043");
        assert_eq!(cam.attachment_info, 0);
        assert_eq!(cam.interp_factor, 17);
        assert!(cam.flags.starts_at_current_pos());
        assert!(!cam.flags.ends_at_current_pos());
        assert_eq!(cam.smoothing, CameraSmoothType::AccelerateAndDecelerate);
        assert_eq!(cam.points.len(), 2);
        let p0 = cam.points[0];
        assert_eq!(p0.position, [1.0, 2.0, 3.0]);
        assert_eq!(p0.focal_length, 500.0);
        assert_eq!(p0.target, [4.0, 5.0, 6.0]);
        assert_eq!(p0.roll, 0.5);
        assert_eq!(p0.param, [0.0, 9.0, 8.0]);
        let p1 = cam.points[1];
        assert_eq!(p1.position, [-1.0, -2.0, -3.0]);
        assert_eq!(p1.focal_length, 682.0);
    }

    /// One authored point: locked with no flags, straight once a flag adds the virtual
    /// endpoint, spline when both do. Two authored points are straight, three a spline.
    #[test]
    fn path_mode_follows_the_effective_point_count() {
        let one = [point_bytes([0.0; 3], 280.0, [0.0; 3], 0.0, [0.0; 3])];
        let two = [
            point_bytes([0.0; 3], 280.0, [0.0; 3], 0.0, [0.0; 3]),
            point_bytes([1.0; 3], 280.0, [1.0; 3], 0.0, [1.0; 3]),
        ];
        let three = vec![two[0].clone(), two[1].clone(), two[0].clone()];

        let locked = CameraResource::parse(*b"lk01", &chunk_body(0, 0, 0, 0, &[one[0].clone()]));
        assert_eq!(locked.unwrap().path_mode(), CameraPathMode::Locked);

        let straight = CameraResource::parse(
            *b"st01",
            &chunk_body(0, 0, 0, 0, &[two[0].clone(), two[1].clone()]),
        )
        .unwrap();
        assert_eq!(straight.path_mode(), CameraPathMode::Straight);

        let spline = CameraResource::parse(*b"sp01", &chunk_body(0, 0, 0, 0, &three)).unwrap();
        assert_eq!(spline.path_mode(), CameraPathMode::Spline);

        let flagged = CameraResource::parse(
            *b"fl01",
            &chunk_body(
                0,
                0,
                CameraFlags::START_AT_CURRENT_POS,
                0,
                &[one[0].clone()],
            ),
        )
        .unwrap();
        assert_eq!(flagged.control_point_count(), 2);
        assert_eq!(flagged.path_mode(), CameraPathMode::Straight);

        let both = CameraResource::parse(
            *b"fl02",
            &chunk_body(
                0,
                0,
                CameraFlags::START_AT_CURRENT_POS | CameraFlags::END_AT_CURRENT_POS,
                0,
                &[one[0].clone()],
            ),
        )
        .unwrap();
        assert_eq!(both.control_point_count(), 3);
        assert_eq!(both.path_mode(), CameraPathMode::Spline);
    }

    #[test]
    fn attach_mode_and_locator_decode_the_shipped_forms() {
        // The forms the install ships (corpus scan of every kind 0x06 chunk): the asserts pin
        // each raw value against the Attachment.cpp decode - the dominant form orbits the
        // caster's BODY_CENTER, the source-to-target pair rides the NAME locator, and the
        // mode-zero values carry locator bits the default arm ignores.
        let decode = |raw: u32| -> (u32, u32) {
            let cam = CameraResource::parse(*b"at01", &chunk_body(raw, 0, 0, 0, &[])).unwrap();
            (cam.attach_mode(), cam.attach_locator_index())
        };
        assert_eq!(decode(0x0), (ATTACH_MODE_WORLD, 0));
        assert_eq!(decode(0x1), (ATTACH_MODE_CASTER, 0));
        assert_eq!(decode(0x71), (ATTACH_MODE_CASTER, 7));
        assert_eq!(decode(0x151), (ATTACH_MODE_CASTER, 21));
        assert_eq!(decode(0x191), (ATTACH_MODE_CASTER, 25));
        assert_eq!(decode(0x821), (ATTACH_MODE_CASTER, 2));
        assert_eq!(decode(0x823), (ATTACH_MODE_SOURCE_TO_TARGET, 2));
        for raw in [0x10, 0x20, 0x50, 0x170] {
            assert_eq!(
                decode(raw).0,
                ATTACH_MODE_WORLD,
                "mode 0 ignores the locator bits: {raw:#x}"
            );
        }
        assert_eq!(decode(0x10151), (17, 21));
        assert_eq!(decode(0x40151), (ATTACH_MODE_CASTER, 64 + 21));
    }

    #[test]
    fn smoothing_values_above_the_curves_are_keyframe_fourccs() {
        let body = chunk_body(
            0,
            0,
            0,
            808_464_491,
            &[point_bytes([0.0; 3], 280.0, [0.0; 3], 0.0, [0.0; 3])],
        );
        let cam = CameraResource::parse(*b"kf01", &body).unwrap();
        assert_eq!(cam.smoothing, CameraSmoothType::Keyframe(808_464_491));
    }

    /// A body shorter than the header and a point count that outruns the bytes both error.
    #[test]
    fn truncated_bodies_error() {
        let short = vec![0u8; POINTS_OFFSET - 1];
        assert!(CameraResource::parse(*b"tr01", &short).is_err());
        let mut lying = chunk_body(
            0,
            0,
            0,
            0,
            &[point_bytes([0.0; 3], 280.0, [0.0; 3], 0.0, [0.0; 3])],
        );
        lying[CONTROL_POINT_COUNT_OFFSET] = 2;
        assert!(CameraResource::parse(*b"tr02", &lying).is_err());
    }

    // research/XIClient source/Game/Scheduler/Tags/0x04.cpp HandleTag0x04 - retail-byte guard
    // (skips without an install): the Southern San d'Oria opening scene's two scheduler DATs
    // and the fade file, walked end to end. Every camera-route stage in them names a
    // ChunkKind::Camera chunk that parses in the same file.
    /// ROM/62/82.DAT
    const SANDY_SCHEDULER_FILE_ID: u32 = 30834;
    /// ROM/94/123.DAT
    const SANDY_VARIATION_FILE_ID: u32 = 30912;
    /// ROM/62/110.DAT
    const FADE_FILE_ID: u32 = 30904;

    fn file_bytes(file_id: u32) -> Option<Vec<u8>> {
        let root = crate::archive::open_test_install()?;
        let loc = root.resolve(file_id).ok()?;
        std::fs::read(loc.path_under(&root)).ok()
    }

    #[test]
    fn real_dat_sandy_camera_routes_parse_and_match_their_stages() {
        for file_id in [
            SANDY_SCHEDULER_FILE_ID,
            SANDY_VARIATION_FILE_ID,
            FADE_FILE_ID,
        ] {
            let Some(bytes) = file_bytes(file_id) else {
                return;
            };
            let cameras: std::collections::HashMap<[u8; 4], CameraResource> = crate::walk(&bytes)
                .flatten()
                .filter(|c| c.kind == crate::kind::ChunkKind::Camera as u8)
                .filter_map(|c| CameraResource::parse(c.name, c.data).ok())
                .map(|cam| (cam.name, cam))
                .collect();
            assert!(!cameras.is_empty(), "{file_id} has no camera chunks");

            let mut stage_routes = 0;
            for chunk in crate::walk(&bytes).flatten() {
                if chunk.kind != crate::kind::ChunkKind::Scheduler as u8 {
                    continue;
                }
                let Ok(routine) = crate::scheduler::Scheduler::parse(chunk.name, chunk.data) else {
                    continue;
                };
                for timed in &routine.stages {
                    if timed.stage.kind != crate::scheduler::StageKind::CameraRoute {
                        continue;
                    }
                    stage_routes += 1;
                    assert!(
                        cameras.contains_key(&timed.stage.id),
                        "{file_id}: routine {} names camera {} which the file does not hold",
                        String::from_utf8_lossy(&routine.name),
                        String::from_utf8_lossy(&timed.stage.id)
                    );
                }
            }
            assert!(stage_routes > 0, "{file_id} has no camera stages");
        }
    }

    #[test]
    fn real_dat_c043_is_two_point_linear_and_c077_a_three_point_spline() {
        let Some(bytes) = file_bytes(SANDY_SCHEDULER_FILE_ID) else {
            return;
        };
        let cam = |name: [u8; 4]| -> CameraResource {
            crate::walk(&bytes)
                .flatten()
                .find(|c| c.kind == crate::kind::ChunkKind::Camera as u8 && c.name == name)
                .and_then(|c| CameraResource::parse(c.name, c.data).ok())
                .unwrap_or_else(|| panic!("camera {name:?} missing from 30834"))
        };

        let c043 = cam(*b"c043");
        assert_eq!(c043.path_mode(), CameraPathMode::Straight);
        assert_eq!(c043.points.len(), 2);
        assert!(!c043.flags.starts_at_current_pos());
        assert!(!c043.flags.ends_at_current_pos());
        assert_eq!(c043.smoothing, CameraSmoothType::Linear);
        assert!((c043.points[0].focal_length - 500.14).abs() < 0.5);
        assert!((c043.points[1].focal_length - 500.16).abs() < 0.5);

        let c077 = cam(*b"c077");
        assert_eq!(c077.path_mode(), CameraPathMode::Spline);
        assert_eq!(c077.points.len(), 3);
        let times: Vec<f32> = c077.points.iter().map(|p| p.param[0]).collect();
        assert!((times[0] - 0.0).abs() < 1e-4);
        assert!(
            (times[1] - 2.0 / 3.0).abs() < 1e-2,
            "middle point {times:?}"
        );
        assert!((times[2] - 1.0).abs() < 1e-4);
        let first = c077.points[0].focal_length;
        for p in &c077.points {
            assert_eq!(p.focal_length, first);
        }
        assert!((first - 682.0).abs() < 0.5);
    }
}
