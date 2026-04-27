//! Typed decoders for the most useful S2C map packets.
//!
//! Both `CHAR_PC` (0x00D) and `CHAR_NPC` (0x00E) share the same 40-byte
//! `GP_SERV_POS_HEAD` prefix (after the 4-byte sub-packet header). After
//! that, the layout is variable based on the `SendFlg` bits. For v1 we
//! decode the position-head only; later we can pull `name` from the trailing
//! fields when `SendFlg.Name` is set.
//!
//! References:
//! - `server/src/map/packets/char_update.cpp::GP_SERV_CHAR_PC`
//! - `server/src/map/packets/entity_update.cpp::GP_SERV_CHAR_NPC`
//! - `server/src/map/packets/s2c/0x00b_logout.h`

use crate::framing::SubPacket;

/// Position-and-status head shared by `GP_SERV_CHAR_PC` (0x00D) and
/// `GP_SERV_CHAR_NPC` (0x00E). The first 36 bytes of the *body* (i.e.
/// the bytes after the 4-byte sub-packet header).
#[derive(Debug, Clone, Copy)]
pub struct PosHead {
    /// Globally-unique entity ID (server-side).
    pub unique_no: u32,
    /// Per-zone short index.
    pub act_index: u16,
    /// `SendFlg` bits — which optional fields trail.
    pub send_flag: u8,
    /// Heading 0..=255 (0°..360°).
    pub dir: u8,
    /// World X coordinate.
    pub x: f32,
    /// World Z coordinate (height).
    pub z: f32,
    /// World Y coordinate.
    pub y: f32,
    /// Movement timer / flags (bit-packed) — opaque for now.
    pub flags0: u32,
    /// Current speed.
    pub speed: u8,
    /// Base speed.
    pub speed_base: u8,
    /// HP percentage 0..=100.
    pub hpp: u8,
    /// Server-side status enum.
    pub server_status: u8,
    pub flags1: u32,
    pub flags2: u32,
    pub flags3: u32,
    /// Battle target ID, 0 if not in combat.
    pub bt_target_id: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("expected at least {0} bytes, have {1}")]
    Truncated(usize, usize),
    #[error("opcode 0x{got:03x} does not match expected 0x{expected:03x}")]
    OpcodeMismatch { expected: u16, got: u16 },
}

impl PosHead {
    pub const SIZE: usize = 40;

    /// Decode from the *body* of a CHAR_PC/CHAR_NPC sub-packet
    /// (`SubPacket::data`).
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            unique_no: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            act_index: u16::from_le_bytes(body[4..6].try_into().unwrap()),
            send_flag: body[6],
            dir: body[7],
            x: f32::from_le_bytes(body[8..12].try_into().unwrap()),
            z: f32::from_le_bytes(body[12..16].try_into().unwrap()),
            y: f32::from_le_bytes(body[16..20].try_into().unwrap()),
            flags0: u32::from_le_bytes(body[20..24].try_into().unwrap()),
            speed: body[24],
            speed_base: body[25],
            hpp: body[26],
            server_status: body[27],
            flags1: u32::from_le_bytes(body[28..32].try_into().unwrap()),
            flags2: u32::from_le_bytes(body[32..36].try_into().unwrap()),
            flags3: u32::from_le_bytes(body[36..40].try_into().unwrap()),
            bt_target_id: 0, // BtTargetID is at offset 36..40 in some clients;
                             // older POS_HEAD ends at flags3. Surface as 0
                             // until we know which the live server emits.
        })
    }

    /// Extract the NUL-terminated ASCII name from a CHAR_PC or CHAR_NPC body.
    ///
    /// `CHAR_PC` (0x00D): name lives at `body[body.len() - 16..]`. LSB's
    /// `GP_SERV_CHAR_PC` ends with `uint8_t name[16]`
    /// (server `char_update.cpp:203`), so the trailing-16 trick is correct
    /// regardless of which optional fields preceded it.
    ///
    /// `CHAR_NPC` (0x00E): name lives at fixed body offset 48 (LSB writes
    /// at packet absolute 0x34 minus the 4-byte sub-packet header — see
    /// `entity_update.cpp:371`). Only present when the packet was emitted
    /// with `UPDATE_NAME` set; we infer presence by body length ≥ 64.
    ///
    /// Returns None for any other opcode, missing-name layouts, or non-ASCII
    /// content.
    pub fn try_extract_name(opcode: u16, body: &[u8]) -> Option<String> {
        use crate::map::s2c;
        let candidate: &[u8] = if opcode == s2c::CHAR_PC {
            if body.len() < Self::SIZE + 16 {
                return None;
            }
            &body[body.len() - 16..]
        } else if opcode == s2c::CHAR_NPC {
            if body.len() < 64 {
                return None;
            }
            &body[48..64]
        } else {
            return None;
        };
        let n = candidate.iter().position(|&b| b == 0).unwrap_or(candidate.len());
        // 3-char floor: filters false positives where the body[48..64] window
        // catches a single non-NUL byte (e.g., a flag value of 0x6B = 'k')
        // followed by NUL when UPDATE_NAME is *not* set. Real FFXI NPC names
        // are 3+ chars; a 2-char tradeoff loses essentially nothing.
        if n < 3 {
            return None;
        }
        let name_bytes = &candidate[..n];
        if !name_bytes.iter().all(|&b| (0x20..=0x7E).contains(&b)) {
            return None;
        }
        Some(String::from_utf8_lossy(name_bytes).into_owned())
    }
}

/// Server's `GP_SERV_COMMAND_LOGOUT` (0x00B) — used for zone changes and
/// disconnects. Carries the destination map server's IP/port and the
/// reason code.
///
/// Layout (after the 4-byte sub-packet header):
/// ```text
///   [0..4]   LogoutState (u32 enum)
///   [4..8]   Iwasaki.ip (u32, LE — the new map server IP, host-LE order)
///   [8..12]  Iwasaki.port (u32, LE — only the low 16 bits are meaningful)
///   [12..20] Iwasaki.padding (8 bytes, zero)
///   [20..24] cliErrCode (u32 enum)
/// ```
#[derive(Debug, Clone, Copy)]
pub struct ServerLogout {
    pub logout_state: u32,
    pub new_server_ip: u32,
    pub new_server_port: u16,
    pub error_code: u32,
}

impl ServerLogout {
    pub const SIZE: usize = 24;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            logout_state: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            new_server_ip: u32::from_le_bytes(body[4..8].try_into().unwrap()),
            new_server_port: u32::from_le_bytes(body[8..12].try_into().unwrap()) as u16,
            error_code: u32::from_le_bytes(body[20..24].try_into().unwrap()),
        })
    }

    /// `LogoutState` enum values from
    /// `server/src/map/packets/s2c/0x00b_logout.h::GP_GAME_LOGOUT_STATE`.
    pub fn is_zone_change(&self) -> bool {
        self.logout_state == 2 // ZONECHANGE
    }
}

/// Convenience: attempt to decode a sub-packet by opcode.
pub fn decode_pos_head(sub: &SubPacket<'_>) -> Result<PosHead, DecodeError> {
    PosHead::decode(sub.data)
}

pub fn decode_logout(sub: &SubPacket<'_>) -> Result<ServerLogout, DecodeError> {
    ServerLogout::decode(sub.data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pos_head_minimal_decode() {
        // Build a minimal 40-byte body with known field values.
        let mut buf = vec![0u8; PosHead::SIZE];
        buf[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes()); // UniqueNo
        buf[4..6].copy_from_slice(&0x0042u16.to_le_bytes()); // ActIndex
        buf[6] = 0b0000_0001; // SendFlg.Position
        buf[7] = 64; // dir
        buf[8..12].copy_from_slice(&123.5f32.to_le_bytes()); // x
        buf[12..16].copy_from_slice(&(-12.0f32).to_le_bytes()); // z
        buf[16..20].copy_from_slice(&7.25f32.to_le_bytes()); // y
        buf[24] = 25; // speed
        buf[25] = 25; // speed_base
        buf[26] = 100; // hpp
        buf[27] = 1; // server_status

        let h = PosHead::decode(&buf).unwrap();
        assert_eq!(h.unique_no, 0xDEAD_BEEF);
        assert_eq!(h.act_index, 0x42);
        assert_eq!(h.send_flag, 1);
        assert_eq!(h.dir, 64);
        assert_eq!(h.x, 123.5);
        assert_eq!(h.z, -12.0);
        assert_eq!(h.y, 7.25);
        assert_eq!(h.hpp, 100);
    }

    #[test]
    fn server_logout_zone_change() {
        let mut buf = vec![0u8; ServerLogout::SIZE];
        buf[0..4].copy_from_slice(&2u32.to_le_bytes()); // ZONECHANGE
        buf[4..8].copy_from_slice(&0x6F00_A8C0u32.to_le_bytes()); // 192.168.0.111-ish
        buf[8..12].copy_from_slice(&54230u32.to_le_bytes());
        let l = ServerLogout::decode(&buf).unwrap();
        assert!(l.is_zone_change());
        assert_eq!(l.new_server_port, 54230);
        assert_eq!(l.new_server_ip, 0x6F00_A8C0);
    }

    #[test]
    fn pos_head_truncated_errors() {
        let buf = vec![0u8; PosHead::SIZE - 1];
        assert!(matches!(
            PosHead::decode(&buf),
            Err(DecodeError::Truncated(_, _))
        ));
    }
}
