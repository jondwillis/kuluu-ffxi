use serde::{Deserialize, Serialize};

use super::DecodeError;

/// The extra action offered by s2c 0x0F9. `None` is the ordinary home-point
/// menu; the server only sends `Raise` or `Tractor` while that offer is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeathMenuOffer {
    Raise,
    Tractor,
}

/// One s2c 0x0F9 `GP_SERV_COMMAND_RES` death-menu update.
///
/// LSB's `PacketData` is exactly eight body bytes: `UniqueNo` u32 @ 0,
/// `ActIndex` u16 @ 4, and `type` u16 @ 6
/// (`vendor/server/src/map/packets/s2c/0x0f9_res.h:38-45`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeathMenu {
    pub unique_no: u32,
    pub act_index: u16,
    pub offer: Option<DeathMenuOffer>,
}

impl DeathMenu {
    pub(crate) const SIZE: usize = 8;
    const TYPE_RAISE: u16 = 1;
    const TYPE_TRACTOR: u16 = 2;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let kind = u16::from_le_bytes([body[6], body[7]]);
        Ok(Self {
            unique_no: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            act_index: u16::from_le_bytes([body[4], body[5]]),
            // RecvServRes treats every value other than Raise/Tractor as the
            // default home-point menu (research/XiPackets/world/server/0x00F9).
            offer: match kind {
                Self::TYPE_RAISE => Some(DeathMenuOffer::Raise),
                Self::TYPE_TRACTOR => Some(DeathMenuOffer::Tractor),
                _ => None,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(kind: u16) -> [u8; DeathMenu::SIZE] {
        let mut body = [0u8; DeathMenu::SIZE];
        body[0..4].copy_from_slice(&0x0102_0304u32.to_le_bytes());
        body[4..6].copy_from_slice(&0x0506u16.to_le_bytes());
        body[6..8].copy_from_slice(&kind.to_le_bytes());
        body
    }

    #[test]
    fn decodes_lsb_packet_data_offsets_and_types() {
        for (kind, offer) in [
            (u16::default(), None),
            (DeathMenu::TYPE_RAISE, Some(DeathMenuOffer::Raise)),
            (DeathMenu::TYPE_TRACTOR, Some(DeathMenuOffer::Tractor)),
            (u16::MAX, None),
        ] {
            let decoded = DeathMenu::decode(&body(kind)).unwrap();
            assert_eq!(decoded.unique_no, 0x0102_0304);
            assert_eq!(decoded.act_index, 0x0506);
            assert_eq!(decoded.offer, offer);
        }
    }

    #[test]
    fn rejects_a_truncated_body() {
        assert!(matches!(
            DeathMenu::decode(&[0; DeathMenu::SIZE - 1]),
            Err(DecodeError::Truncated(DeathMenu::SIZE, n)) if n == DeathMenu::SIZE - 1
        ));
    }
}
