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
    /// Minimum body length for a POS_HEAD without `BtTargetID` — older
    /// PS2-era layouts ended at `Flags3`. We accept these but report
    /// `bt_target_id = 0`.
    pub const SIZE: usize = 40;

    /// Body length when the trailing `BtTargetID` field is present.
    /// Phoenix and modern LSB always send this — see
    /// `Phoenix/src/map/packets/char_update.cpp:187`. Below this length,
    /// `bt_target_id` decodes as 0 (= "no target").
    pub const SIZE_WITH_BT_TARGET: usize = 44;

    /// Decode from the *body* of a CHAR_PC/CHAR_NPC sub-packet
    /// (`SubPacket::data`).
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let bt_target_id = if body.len() >= Self::SIZE_WITH_BT_TARGET {
            u32::from_le_bytes(body[40..44].try_into().unwrap())
        } else {
            0
        };
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
            bt_target_id,
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

/// Common party-member fields shared by `0x0DD GROUP_LIST` (other members)
/// and `0x0DF GROUP_ATTR` (self + Trust). Field offsets / types mirror
/// `Phoenix/src/map/packets/s2c/0x0d{d,f}_group_*.h`.
#[derive(Debug, Clone)]
pub struct PartyAttrs {
    pub unique_no: u32,
    pub act_index: u16,
    pub hp: u32,
    pub mp: u32,
    pub tp: u32,
    pub hpp: u8,
    pub mpp: u8,
    pub kind: u8,
    pub zone_no: u16,
    pub mjob_no: u8,
    pub mjob_lv: u8,
    pub sjob_no: u8,
    pub sjob_lv: u8,
}

/// Additional fields only present in `0x0DD GROUP_LIST` (other members):
/// the trailing 16-byte name and the `GAttr` bitfield with leader flags.
#[derive(Debug, Clone)]
pub struct PartyListExtra {
    pub member_number: u8,
    pub is_party_leader: bool,
    pub is_alliance_leader: bool,
    /// Up-to-15-character ASCII name. NUL-terminated.
    pub name: Option<String>,
}

impl PartyAttrs {
    /// Decode the body of `0x0DF GROUP_ATTR` (self / Trust). Layout:
    /// `[0..4]UniqueNo [4..8]Hp [8..12]Mp [12..16]Tp [16..18]ActIndex
    ///  [18]Hpp [19]Mpp [20]Kind [21]MoghouseFlg [22..24]ZoneNo
    ///  [24..28]Monstrosity… [28]mjob [29]mjob_lv [30]sjob [31]sjob_lv …`.
    pub fn decode_group_attr(body: &[u8]) -> Result<Self, DecodeError> {
        const NEEDED: usize = 32;
        if body.len() < NEEDED {
            return Err(DecodeError::Truncated(NEEDED, body.len()));
        }
        Ok(Self {
            unique_no: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            hp: u32::from_le_bytes(body[4..8].try_into().unwrap()),
            mp: u32::from_le_bytes(body[8..12].try_into().unwrap()),
            tp: u32::from_le_bytes(body[12..16].try_into().unwrap()),
            act_index: u16::from_le_bytes(body[16..18].try_into().unwrap()),
            hpp: body[18],
            mpp: body[19],
            kind: body[20],
            zone_no: u16::from_le_bytes(body[22..24].try_into().unwrap()),
            mjob_no: body[28],
            mjob_lv: body[29],
            sjob_no: body[30],
            sjob_lv: body[31],
        })
    }

    /// Decode the body of `0x0DD GROUP_LIST` (other members). Layout:
    /// `[0..4]UniqueNo [4..8]Hp [8..12]Mp [12..16]Tp [16..20]GAttr
    ///  [20..22]ActIndex [22]MemberNumber [23]MoghouseFlg [24]Kind
    ///  [25]Hpp [26]Mpp [27]pad [28..30]ZoneNo [30]mjob [31]mjob_lv
    ///  [32]sjob [33]sjob_lv [34]masterjob_lv [35]masterjob_flags
    ///  [36..52]Name`.
    pub fn decode_group_list(body: &[u8]) -> Result<(Self, PartyListExtra), DecodeError> {
        const NEEDED: usize = 52;
        if body.len() < NEEDED {
            return Err(DecodeError::Truncated(NEEDED, body.len()));
        }
        let attrs = Self {
            unique_no: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            hp: u32::from_le_bytes(body[4..8].try_into().unwrap()),
            mp: u32::from_le_bytes(body[8..12].try_into().unwrap()),
            tp: u32::from_le_bytes(body[12..16].try_into().unwrap()),
            act_index: u16::from_le_bytes(body[20..22].try_into().unwrap()),
            kind: body[24],
            hpp: body[25],
            mpp: body[26],
            zone_no: u16::from_le_bytes(body[28..30].try_into().unwrap()),
            mjob_no: body[30],
            mjob_lv: body[31],
            sjob_no: body[32],
            sjob_lv: body[33],
        };
        let gattr = u32::from_le_bytes(body[16..20].try_into().unwrap());
        // Bitfield: PartyNo:2, PartyLeaderFlg:1, AllianceLeaderFlg:1, …
        let is_party_leader = (gattr >> 2) & 1 == 1;
        let is_alliance_leader = (gattr >> 3) & 1 == 1;
        let name_bytes = &body[36..52];
        let n = name_bytes
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(name_bytes.len());
        let name = if n > 0 && name_bytes[..n].iter().all(|&b| (0x20..=0x7E).contains(&b)) {
            Some(String::from_utf8_lossy(&name_bytes[..n]).into_owned())
        } else {
            None
        };
        let extra = PartyListExtra {
            member_number: body[22],
            is_party_leader,
            is_alliance_leader,
            name,
        };
        Ok((attrs, extra))
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

    #[test]
    fn pos_head_extracts_bt_target_id_when_present() {
        // 44-byte body — full Phoenix layout including BtTargetID.
        let mut buf = vec![0u8; PosHead::SIZE_WITH_BT_TARGET];
        buf[0..4].copy_from_slice(&0xCAFE_F00Du32.to_le_bytes()); // UniqueNo
        buf[40..44].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes()); // BtTargetID
        let h = PosHead::decode(&buf).unwrap();
        assert_eq!(h.unique_no, 0xCAFE_F00D);
        assert_eq!(h.bt_target_id, 0xDEAD_BEEF);
    }

    #[test]
    fn pos_head_legacy_40_byte_body_yields_zero_bt_target() {
        // PS2-era layout ended at Flags3. We accept these but bt_target_id is 0.
        let buf = vec![0u8; PosHead::SIZE];
        let h = PosHead::decode(&buf).unwrap();
        assert_eq!(h.bt_target_id, 0);
    }

    #[test]
    fn party_attrs_group_attr_decodes() {
        let mut buf = vec![0u8; 36];
        buf[0..4].copy_from_slice(&0x0001_0042u32.to_le_bytes()); // UniqueNo
        buf[4..8].copy_from_slice(&1500u32.to_le_bytes()); // Hp
        buf[8..12].copy_from_slice(&500u32.to_le_bytes()); // Mp
        buf[12..16].copy_from_slice(&1234u32.to_le_bytes()); // Tp
        buf[16..18].copy_from_slice(&0x0042u16.to_le_bytes()); // ActIndex
        buf[18] = 75; // Hpp
        buf[19] = 50; // Mpp
        buf[20] = 0; // Kind = PC
        buf[22..24].copy_from_slice(&234u16.to_le_bytes()); // ZoneNo
        buf[28] = 6; // mjob_no = WHM
        buf[29] = 75; // mjob_lv
        buf[30] = 1; // sjob_no = WAR
        buf[31] = 37; // sjob_lv

        let p = PartyAttrs::decode_group_attr(&buf).unwrap();
        assert_eq!(p.unique_no, 0x0001_0042);
        assert_eq!(p.hp, 1500);
        assert_eq!(p.mp, 500);
        assert_eq!(p.tp, 1234);
        assert_eq!(p.act_index, 0x42);
        assert_eq!(p.hpp, 75);
        assert_eq!(p.mpp, 50);
        assert_eq!(p.zone_no, 234);
        assert_eq!(p.mjob_no, 6);
        assert_eq!(p.mjob_lv, 75);
        assert_eq!(p.sjob_no, 1);
        assert_eq!(p.sjob_lv, 37);
    }

    #[test]
    fn party_attrs_group_list_decodes_with_name_and_leader() {
        let mut buf = vec![0u8; 56];
        buf[0..4].copy_from_slice(&0x0010_0001u32.to_le_bytes()); // UniqueNo
        buf[4..8].copy_from_slice(&2000u32.to_le_bytes()); // Hp
        buf[8..12].copy_from_slice(&100u32.to_le_bytes()); // Mp
        buf[12..16].copy_from_slice(&0u32.to_le_bytes()); // Tp
        // GAttr bitfield: PartyNo:2 (=1), PartyLeaderFlg:1 (=1), AllianceLeaderFlg:1 (=0)
        // → low 4 bits = 0b0101 = 5
        buf[16..20].copy_from_slice(&0x0000_0005u32.to_le_bytes());
        buf[20..22].copy_from_slice(&0x0007u16.to_le_bytes()); // ActIndex
        buf[22] = 1; // MemberNumber
        buf[24] = 0; // Kind
        buf[25] = 100; // Hpp
        buf[26] = 100; // Mpp
        buf[28..30].copy_from_slice(&230u16.to_le_bytes()); // ZoneNo (Bastok Markets)
        buf[30] = 1; // mjob WAR
        buf[31] = 75;
        buf[36..36 + 6].copy_from_slice(b"Vanari");

        let (attrs, extra) = PartyAttrs::decode_group_list(&buf).unwrap();
        assert_eq!(attrs.unique_no, 0x0010_0001);
        assert_eq!(attrs.hp, 2000);
        assert_eq!(attrs.act_index, 7);
        assert_eq!(attrs.zone_no, 230);
        assert_eq!(extra.member_number, 1);
        assert!(extra.is_party_leader);
        assert!(!extra.is_alliance_leader);
        assert_eq!(extra.name.as_deref(), Some("Vanari"));
    }

    #[test]
    fn party_attrs_group_list_truncated_errors() {
        let buf = vec![0u8; 40];
        assert!(matches!(
            PartyAttrs::decode_group_list(&buf),
            Err(DecodeError::Truncated(52, 40))
        ));
    }
}
