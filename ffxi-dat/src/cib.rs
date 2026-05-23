//! `Cib` chunk parser — kind `0x45`. Character / actor info: footstep
//! sound classes, motion-range index, skeletal/equipment scale.
//!
//! Each Cib chunk sits inside an actor DAT (NPC skeleton DAT, PC race
//! DAT) and carries the per-actor metadata that downstream systems
//! need to pick the right sound and motion at runtime:
//!
//!   - `footstep_material` selects which footstep SEP plays when an
//!     animation fires a foot-down keyframe. Reference table lives
//!     in lotus's audio system (Earth, Stone, Wood, Water, etc.).
//!   - `motion_index` / `motion_range_index` select default idle /
//!     locomotion animation tracks for actors that don't otherwise
//!     announce a motion via Scheduler.
//!   - `scale` is a uniform scale multiplier (per lotus comments,
//!     stored as u8 in the 15-byte struct — its exact units are
//!     unclear but `scale == 0` likely means "use 1.0").
//!
//! Wire layout (port of `vendor/lotus-ffxi/ffxi/dat/cib.cppm` — a
//! flat 15-byte packed struct of `uint8_t`):
//!
//! ```text
//! 0x00  u8  unknown1            (from skeleton)
//! 0x01  u8  footstep_material   (lotus "footstep1 / FootMat")
//! 0x02  u8  footstep_size       (lotus "footstep2 / FootSize")
//! 0x03  u8  motion_index
//! 0x04  u8  motion_option
//! 0x05  u8  weapon_unknown      (lotus "Shield?")
//! 0x06  u8  weapon_constrain    (lotus "Constrain?")
//! 0x07  u8  unknown2
//! 0x08  u8  weapon_unknown3
//! 0x09  u8  body_armour_waist   (lotus "Waist?")
//! 0x0A  u8  scale
//! 0x0B  u8  unknown6            (lotus comments "float, default 1.000"
//!                                — likely a scale-fraction byte; meaning
//!                                unclear and unread until empirically
//!                                pinned)
//! 0x0C  u8  unknown7            (same)
//! 0x0D  u8  unknown8            (same)
//! 0x0E  u8  motion_range_index
//! ```
//!
//! Lotus stores these in a struct with no constructor body — the
//! reader is effectively a `reinterpret_cast` over the chunk body.
//! We do the same: copy fields by offset, no decoding tricks.
//!
//! Note: the lotus comments label `unknown6/7/8` as "float, default
//! 1.000" while declaring them as `uint8_t`. Without ground-truth
//! bytes from a real CIB we keep them as raw `u8` fields and surface
//! them on the struct so callers can experiment.

use crate::{DatError, Result};

/// Bytes in a CIB chunk body. Lotus's struct is 15 `uint8_t` fields
/// with no padding pragma (default 1-byte alignment for `uint8_t`).
pub const CIB_LEN: usize = 15;

/// One parsed CIB chunk. All fields are raw u8 — semantics
/// documented at the module level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cib {
    /// 4-char chunk id from the enclosing DAT header.
    pub name: [u8; 4],
    pub unknown1: u8,
    /// Footstep material class. Looked up against a fixed SEP table
    /// at the audio layer (Stone / Grass / Wood / Water / Metal /…).
    pub footstep_material: u8,
    /// Footstep size class (small / medium / large), influences pitch
    /// and choice of variant per material.
    pub footstep_size: u8,
    pub motion_index: u8,
    pub motion_option: u8,
    pub weapon_unknown: u8,
    pub weapon_constrain: u8,
    pub unknown2: u8,
    pub weapon_unknown3: u8,
    pub body_armour_waist: u8,
    pub scale: u8,
    pub unknown6: u8,
    pub unknown7: u8,
    pub unknown8: u8,
    pub motion_range_index: u8,
}

impl Cib {
    pub fn parse(name: [u8; 4], body: &[u8]) -> Result<Self> {
        if body.len() < CIB_LEN {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: CIB_LEN,
                available: body.len(),
            });
        }
        Ok(Self {
            name,
            unknown1: body[0x00],
            footstep_material: body[0x01],
            footstep_size: body[0x02],
            motion_index: body[0x03],
            motion_option: body[0x04],
            weapon_unknown: body[0x05],
            weapon_constrain: body[0x06],
            unknown2: body[0x07],
            weapon_unknown3: body[0x08],
            body_armour_waist: body[0x09],
            scale: body[0x0A],
            unknown6: body[0x0B],
            unknown7: body[0x0C],
            unknown8: body[0x0D],
            motion_range_index: body[0x0E],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_fields() {
        let body: [u8; CIB_LEN] = [
            0x10, 0x02, 0x01, 0x05, 0x00, 0x11, 0x12, 0x13, 0x14, 0x15, 0x80, 0x81, 0x82, 0x83,
            0x07,
        ];
        let c = Cib::parse(*b"cib0", &body).unwrap();
        assert_eq!(c.footstep_material, 0x02);
        assert_eq!(c.footstep_size, 0x01);
        assert_eq!(c.motion_index, 0x05);
        assert_eq!(c.scale, 0x80);
        assert_eq!(c.motion_range_index, 0x07);
    }

    #[test]
    fn rejects_short_body() {
        let body = vec![0u8; CIB_LEN - 1];
        assert!(matches!(
            Cib::parse(*b"shrt", &body),
            Err(DatError::TruncatedChunk {
                needed: 15,
                available: 14,
                ..
            })
        ));
    }

    #[test]
    fn extra_trailing_bytes_are_ignored() {
        let mut body = vec![0u8; CIB_LEN];
        body[0x01] = 0x42; // footstep_material
        body.extend_from_slice(&[0xFF; 8]); // trailing slop
        let c = Cib::parse(*b"long", &body).unwrap();
        assert_eq!(c.footstep_material, 0x42);
    }
}
