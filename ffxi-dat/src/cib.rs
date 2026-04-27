use crate::{DatError, Result};

pub const CIB_LEN: usize = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cib {
    pub name: [u8; 4],
    pub unknown1: u8,

    pub footstep_material: u8,

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
        body[0x01] = 0x42;
        body.extend_from_slice(&[0xFF; 8]);
        let c = Cib::parse(*b"long", &body).unwrap();
        assert_eq!(c.footstep_material, 0x42);
    }
}
