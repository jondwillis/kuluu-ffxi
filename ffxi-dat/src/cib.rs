use crate::{DatError, Result};

pub const CIB_LEN: usize = 15;

/// The Info byte value every field treats as "not set" (research/xim resource/InfoSection.kt
/// nullIf0xFF).
pub const CIB_UNSET: u8 = 0xFF;

/// The movement byte of the Cib Info chunk (vekien/xi-model-viewer ui/js/dat/inspect.js
/// MOVEMENT_TYPE). A byte outside the table is kept as `Unknown` so it stays distinguishable
/// from a shipped `CIB_UNSET`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovementType {
    Walking,
    Sliding,
    Large,
    Flying,
    Unset,
    Unknown(u8),
}

impl std::fmt::Display for MovementType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Walking => "Walking",
            Self::Sliding => "Sliding",
            Self::Large => "Large",
            Self::Flying => "Flying",
            Self::Unset => "Unset",
            Self::Unknown(b) => return write!(f, "Unknown({b:#04x})"),
        };
        f.write_str(name)
    }
}

impl MovementType {
    pub fn from_u8(b: u8) -> Self {
        match b {
            0 => Self::Walking,
            1 => Self::Sliding,
            2 => Self::Large,
            3 => Self::Flying,
            CIB_UNSET => Self::Unset,
            other => Self::Unknown(other),
        }
    }
}

/// The range-type byte of the Cib Info chunk (vekien/xi-model-viewer ui/js/dat/inspect.js
/// RANGE_TYPE). xim documents the gaps explicitly ("no 0x07 / no 0x08 / no 0x09",
/// research/xim resource/InfoSection.kt) and reads them as Unset; retail does ship 0x08 CIBs,
/// so an out-of-table byte is kept as `Unknown` rather than folded into Unset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeType {
    None,
    Wind,
    String,
    Marksmanship,
    ThrowingWeapon,
    ThrowingAmmo,
    Archery,
    HandbellIndi,
    HandbellGeo,
    Unset,
    Unknown(u8),
}

impl RangeType {
    pub fn from_u8(b: u8) -> Self {
        match b {
            0x00 => Self::None,
            0x01 => Self::Wind,
            0x02 => Self::String,
            0x03 => Self::Marksmanship,
            0x04 => Self::ThrowingWeapon,
            0x05 => Self::ThrowingAmmo,
            0x06 => Self::Archery,
            0x0a => Self::HandbellIndi,
            0x0b => Self::HandbellGeo,
            CIB_UNSET => Self::Unset,
            other => Self::Unknown(other),
        }
    }
}

/// The weapon-anim-style byte of the Cib Info chunk (viewer WEAPON_ANIM_STYLE).
/// Interpretation only: the loader still reads the raw `motion_index` byte as a DAT offset.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponAnimStyle {
    ClubStaff = 0,
    Sword = 1,
    HandToHand = 2,
    Dagger = 3,
    GreatSword = 4,
    AxeScythe = 5,
    Katana = 6,
    Kunai = 7,
    Polearm = 8,
}

impl WeaponAnimStyle {
    pub fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            0 => Self::ClubStaff,
            1 => Self::Sword,
            2 => Self::HandToHand,
            3 => Self::Dagger,
            4 => Self::GreatSword,
            5 => Self::AxeScythe,
            6 => Self::Katana,
            7 => Self::Kunai,
            8 => Self::Polearm,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cib {
    pub name: [u8; 4],

    /// The Info movement byte (viewer parseInspectInfo b[0]). Flying/Sliding mobs have no
    /// ground stride to match, so their locomotion clips play at the authored rate.
    pub movement_type: MovementType,

    pub footstep_material: u8,

    pub footstep_size: u8,
    pub motion_index: u8,
    pub motion_option: u8,

    /// `is_shield` — read from the SUB slot's CIB. Selects the upper-body motion
    /// DAT as `base + is_shield + 1` (research/XIClient/src/XIClient/source/
    /// World/Actor/SkeletalMeshActor.cpp SkeletalMeshActor::GetUpperBodyDatIndex), so a shield swaps in a variant
    /// with its own joint count.
    pub is_shield: u8,
    pub weapon_constrain: u8,
    pub unknown2: u8,
    pub weapon_unknown3: u8,

    /// `waist_type` — read from the BODY slot's CIB. Selects the waist/skirt
    /// motion DAT as `base + max(waist_type, 1) + 2` (SkeletalMeshActor.cpp SkeletalMeshActor::GetWaistDatIndex,
    /// via `ReadStdMotionRes` at :3014), which is how a robe gets skirt motion
    /// where plate legs get trousers.
    pub body_armour_waist: u8,

    /// Model scale in percent (viewer parseInspectInfo b[10], "Scale"). Retail divides by 100
    /// with only `CIB_UNSET` meaning default (research/xim poc/Model.kt NpcModel.getScale).
    pub scale: u8,

    /// Scale in percent for static NPCs that are not sitting in a chair; retail swaps it in
    /// for `scale` there (research/xim poc/Actor.kt getScale).
    pub static_npc_scale: u8,
    pub unknown7: u8,
    pub unknown8: u8,

    /// The Info range byte (viewer parseInspectInfo b[14]).
    pub range_type: RangeType,
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
            movement_type: MovementType::from_u8(body[0x00]),
            footstep_material: body[0x01],
            footstep_size: body[0x02],
            motion_index: body[0x03],
            motion_option: body[0x04],
            is_shield: body[0x05],
            weapon_constrain: body[0x06],
            unknown2: body[0x07],
            weapon_unknown3: body[0x08],
            body_armour_waist: body[0x09],
            scale: body[0x0A],
            static_npc_scale: body[0x0B],
            unknown7: body[0x0C],
            unknown8: body[0x0D],
            range_type: RangeType::from_u8(body[0x0E]),
        })
    }

    /// The Info `scale` byte as a model multiplier. Retail divides by 100 with only `CIB_UNSET`
    /// meaning "default" (research/xim poc/Model.kt NpcModel.getScale, poc/Actor.kt getScale;
    /// xim's nullIf0xFF in research/xim resource/InfoSection.kt). 100 therefore lands on 1.0 by
    /// the division itself, and a shipped 0 renders at zero size exactly as retail would.
    pub fn scale_factor(&self) -> f32 {
        if self.scale == CIB_UNSET {
            1.0
        } else {
            self.scale as f32 / 100.0
        }
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
        // The movement byte is outside the viewer's MOVEMENT_TYPE table; xim would throw, we keep it.
        assert_eq!(c.movement_type, MovementType::Unknown(0x10));
        // The range byte is one of xim's documented gaps; xim reads it as Unset.
        assert_eq!(c.range_type, RangeType::Unknown(0x07));
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
        body[0x01] = 0x42;
        body.extend_from_slice(&[0xFF; 8]);
        let c = Cib::parse(*b"long", &body).unwrap();
        assert_eq!(c.footstep_material, 0x42);
    }

    #[test]
    fn bat_info_chunk() {
        // The bat's raw Info body as the viewer reads it (ROM/4/106.DAT, file id 1564; its
        // variants 1556/1561/1563/1565 carry the same bytes). All sixteen on-disk bytes are
        // fed in: our CIB_LEN is 15 and the uninterpreted sixteenth is ignored by design.
        let body = [
            0x03, 0x06, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x55, 0x64, 0x73, 0x8C,
            0xFF, 0xFF,
        ];
        let c = Cib::parse(*b"cib0", &body).unwrap();
        assert_eq!(c.movement_type, MovementType::Flying);
        assert_eq!(c.scale, 85);
        assert_eq!(c.static_npc_scale, 100);
        assert_eq!(c.range_type, RangeType::Unset);
        assert!((c.scale_factor() - 0.85).abs() < f32::EPSILON);
    }

    #[test]
    fn scale_factor_rules() {
        let parse = |scale: u8| {
            let mut body = [0u8; CIB_LEN];
            body[0x0A] = scale;
            Cib::parse(*b"cib0", &body).unwrap()
        };
        assert!((parse(100).scale_factor() - 1.0).abs() < f32::EPSILON);
        assert!((parse(0xFF).scale_factor() - 1.0).abs() < f32::EPSILON);
        assert!((parse(85).scale_factor() - 0.85).abs() < f32::EPSILON);
    }

    #[test]
    fn enums_cover_the_viewer_tables() {
        assert_eq!(MovementType::from_u8(0), MovementType::Walking);
        assert_eq!(MovementType::from_u8(1), MovementType::Sliding);
        assert_eq!(MovementType::from_u8(2), MovementType::Large);
        assert_eq!(MovementType::from_u8(3), MovementType::Flying);
        assert_eq!(MovementType::from_u8(0xFF), MovementType::Unset);
        assert_eq!(MovementType::from_u8(4), MovementType::Unknown(4));

        assert_eq!(RangeType::from_u8(0x06), RangeType::Archery);
        assert_eq!(RangeType::from_u8(0x0a), RangeType::HandbellIndi);
        assert_eq!(RangeType::from_u8(0x0b), RangeType::HandbellGeo);
        // The documented gaps read as out-of-table, not Unset.
        assert_eq!(RangeType::from_u8(0x07), RangeType::Unknown(0x07));
        assert_eq!(RangeType::from_u8(0x08), RangeType::Unknown(0x08));

        assert_eq!(
            WeaponAnimStyle::from_u8(0),
            Some(WeaponAnimStyle::ClubStaff)
        );
        assert_eq!(WeaponAnimStyle::from_u8(8), Some(WeaponAnimStyle::Polearm));
        assert_eq!(WeaponAnimStyle::from_u8(9), None);
    }
}
