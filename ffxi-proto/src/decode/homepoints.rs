use super::*;
use crate::s2c_layout::miscdata_homepoints as lsb;

/// `GP_SERV_COMMAND_MISCDATA_TYPE::Homepoints` (vendor/server/src/map/packets/s2c/0x063_miscdata.h).
pub const MISCDATA_TYPE_HOMEPOINTS: u16 = 0x06;

/// One s2c 0x063 MISCDATA record of type Homepoints: the fast-travel access masks
/// (vendor/server/src/map/packets/s2c/0x063_miscdata_homepoints.h PacketData). Retail stores
/// everything from `home_point` on — the trailing `unknown00` word included, unmodeled here — as
/// one 64-byte block via a `.text FFXiMain.dll retail-2026-09 0x99330` rep movsd off packet+8
/// (body+4), and reads it back only through the per-field bit-test wrappers `.text 0x1CBC70` /
/// `0x1CBCF0` / `0x1CBD30` / `0x1CBCB0` / `0x1CBD70` / `0x1CBDB0`, whose sole caller (`.text
/// 0x1F97A0`) gates map icon entries by category. Vault main_menu.md §59 addendum 37.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MiscdataHomepoints {
    /// Homepoint teleport access masks (16 bytes).
    pub home_point: [u32; Self::MASKS],
    /// Survival guide access masks (16 bytes).
    pub survival_guide: [u32; Self::MASKS],
    /// Waypoint / Abyssea Maw access masks (16 bytes).
    pub waypoint: [u32; Self::MASKS],
    /// Telepoint access mask.
    pub telepoint: u32,
    /// Atmacite teleport mask.
    pub atmos: u32,
    /// Eschan portal mask.
    pub eschan_portal: u32,
}

impl MiscdataHomepoints {
    /// Dwords per mask array — LSB sizes `homePoint`, `survivalGuide` and `waypoint` at 4 each.
    pub const MASKS: usize = 4;
    pub(crate) const TYPE_OFFSET: usize = 0x0;
    pub(crate) const HOME_POINT_OFFSET: usize = 0x4;
    pub(crate) const SURVIVAL_GUIDE_OFFSET: usize = 0x14;
    pub(crate) const WAYPOINT_OFFSET: usize = 0x24;
    pub(crate) const TELEPOINT_OFFSET: usize = 0x34;
    pub(crate) const ATMOS_OFFSET: usize = 0x38;
    pub(crate) const ESCHAN_PORTAL_OFFSET: usize = 0x3C;
    /// `unknown06` (body+2) through `unknown00` (body+0x40): the whole LSB record body is 68 bytes.
    pub(crate) const SIZE: usize = 0x44;

    /// `None` when the record is not of type Homepoints.
    pub fn decode(body: &[u8]) -> Option<Result<Self, DecodeError>> {
        let kind = body.get(Self::TYPE_OFFSET..Self::TYPE_OFFSET + 2)?;
        if u16::from_le_bytes([kind[0], kind[1]]) != MISCDATA_TYPE_HOMEPOINTS {
            return None;
        }
        if body.len() < Self::SIZE {
            return Some(Err(DecodeError::Truncated(Self::SIZE, body.len())));
        }
        let rd32 = |o: usize| u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
        let array = |base: usize| [0u8, 4, 8, 12].map(|at| rd32(base + at as usize));
        Some(Ok(Self {
            home_point: array(Self::HOME_POINT_OFFSET),
            survival_guide: array(Self::SURVIVAL_GUIDE_OFFSET),
            waypoint: array(Self::WAYPOINT_OFFSET),
            telepoint: rd32(Self::TELEPOINT_OFFSET),
            atmos: rd32(Self::ATMOS_OFFSET),
            eschan_portal: rd32(Self::ESCHAN_PORTAL_OFFSET),
        }))
    }
}

pin_s2c_offset!(
    MiscdataHomepoints::TYPE_OFFSET,
    lsb::TYPE,
    "GP_SERV_COMMAND_MISCDATA::HOMEPOINTS.type"
);
pin_s2c_offset!(
    MiscdataHomepoints::SIZE - 4,
    lsb::UNKNOWN00,
    "GP_SERV_COMMAND_MISCDATA::HOMEPOINTS.unknown00"
);
pin_s2c_offset!(
    MiscdataHomepoints::HOME_POINT_OFFSET,
    lsb::HOME_POINT,
    "GP_SERV_COMMAND_MISCDATA::HOMEPOINTS.homePoint"
);
pin_s2c_offset!(
    MiscdataHomepoints::SURVIVAL_GUIDE_OFFSET,
    lsb::SURVIVAL_GUIDE,
    "GP_SERV_COMMAND_MISCDATA::HOMEPOINTS.survivalGuide"
);
pin_s2c_offset!(
    MiscdataHomepoints::WAYPOINT_OFFSET,
    lsb::WAYPOINT,
    "GP_SERV_COMMAND_MISCDATA::HOMEPOINTS.waypoint"
);
pin_s2c_offset!(
    MiscdataHomepoints::TELEPOINT_OFFSET,
    lsb::TELEPOINT,
    "GP_SERV_COMMAND_MISCDATA::HOMEPOINTS.telepoint"
);
pin_s2c_offset!(
    MiscdataHomepoints::ATMOS_OFFSET,
    lsb::ATMOS,
    "GP_SERV_COMMAND_MISCDATA::HOMEPOINTS.atmos"
);
pin_s2c_offset!(
    MiscdataHomepoints::ESCHAN_PORTAL_OFFSET,
    lsb::ESCHAN_PORTAL,
    "GP_SERV_COMMAND_MISCDATA::HOMEPOINTS.eschanPortal"
);
pin_s2c_offset!(
    MiscdataHomepoints::MASKS,
    lsb::HOME_POINT_COUNT,
    "homePoint length"
);
pin_s2c_offset!(
    MiscdataHomepoints::MASKS,
    lsb::SURVIVAL_GUIDE_COUNT,
    "survivalGuide length"
);
pin_s2c_offset!(
    MiscdataHomepoints::MASKS,
    lsb::WAYPOINT_COUNT,
    "waypoint length"
);

#[cfg(test)]
mod tests {
    use super::*;

    /// A 68-byte Homepoints body with every field carrying a distinct marker.
    fn vector() -> Vec<u8> {
        let mut body = vec![0u8; MiscdataHomepoints::SIZE];
        body[0..2].copy_from_slice(&MISCDATA_TYPE_HOMEPOINTS.to_le_bytes());
        let arrays: [(usize, [u32; 4]); 3] = [
            (MiscdataHomepoints::HOME_POINT_OFFSET, [1, 2, 3, 4]),
            (
                MiscdataHomepoints::SURVIVAL_GUIDE_OFFSET,
                [0x100, 0x200_00FF, 0x3000_FFFF, 0xFFFF_FFFF],
            ),
            (MiscdataHomepoints::WAYPOINT_OFFSET, [0xBEEF_0000; 4]),
        ];
        for (base, words) in arrays {
            for (at, word) in words.iter().enumerate() {
                body[base + at * 4..base + at * 4 + 4].copy_from_slice(&word.to_le_bytes());
            }
        }
        let tail: [(usize, u32); 3] = [
            (MiscdataHomepoints::TELEPOINT_OFFSET, 0xDEAD_BEEF),
            (MiscdataHomepoints::ATMOS_OFFSET, 0xCAFE_BABE),
            (MiscdataHomepoints::ESCHAN_PORTAL_OFFSET, 0xF00D_D00D),
        ];
        for (at, word) in tail {
            body[at..at + 4].copy_from_slice(&word.to_le_bytes());
        }
        body
    }

    #[test]
    fn decodes_each_mask_group_at_lsb_offsets() {
        let body = vector();
        let record = MiscdataHomepoints::decode(&body)
            .expect("homepoints subtype passes")
            .expect("full body");
        assert_eq!(record.home_point, [1, 2, 3, 4]);
        assert_eq!(
            record.survival_guide,
            [0x100, 0x200_00FF, 0x3000_FFFF, 0xFFFF_FFFF]
        );
        assert_eq!(record.waypoint, [0xBEEF_0000; 4]);
        // Telepoint .. eschanPortal follow the arrays; unknown00 is skipped as padding.
        assert_eq!(
            (record.telepoint, record.atmos, record.eschan_portal),
            (0xDEAD_BEEF, 0xCAFE_BABE, 0xF00D_D00D)
        );
    }

    #[test]
    fn other_subtypes_and_truncations_are_declined() {
        assert!(MiscdataHomepoints::decode(&[]).is_none());
        let mut unity = vec![0u8; MiscdataHomepoints::SIZE];
        unity[0] = 0x07; // Unity chunk of the same opcode.
        assert!(MiscdataHomepoints::decode(&unity).is_none());
        let body = vector();
        // A correct subtype with a short body is an error, not a silent decline: retail's
        // rep movsd reads all 68 bytes (`.text FFXiMain.dll retail-2026-09 0x99330`).
        let err = MiscdataHomepoints::decode(&body[..MiscdataHomepoints::SIZE - 1])
            .expect("subtype passes")
            .expect_err("truncated");
        assert!(matches!(
            err,
            DecodeError::Truncated(MiscdataHomepoints::SIZE, _)
        ));
    }
}
