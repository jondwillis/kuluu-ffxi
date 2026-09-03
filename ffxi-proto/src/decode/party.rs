use super::*;

/// `GAttr.PartyNo` sentinel for "not in a party of the alliance".
/// vendor/server/src/map/packets/s2c/0x0dd_group_list.cpp:40.
pub const NO_PARTY: u8 = 3;

// ---- GROUP_TBL (0x0C8) — party definition -----------------------------------

/// One entry in the GROUP_TBL packet (12 bytes each, up to 20 entries).
/// vendor/server/src/map/packets/s2c/0x0c8_group_tbl.h.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GroupTblEntry {
    pub unique_no: u32,
    pub act_index: u16,
    pub party_no: u8,
    pub is_party_leader: bool,
    pub is_alliance_leader: bool,
    pub zone_no: u16,
}

/// Kind byte from GROUP_TBL: what type of group this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    None,
    Party,
    Alliance,
    Unknown(u8),
}

/// Decoded GROUP_TBL (s2c 0x0C8) packet.
#[derive(Debug, Clone)]
pub struct GroupTbl {
    pub kind: GroupKind,
    pub members: Vec<GroupTblEntry>,
}

impl GroupTbl {
    /// Decode the GROUP_TBL body (everything after the sub-packet header).
    ///
    /// Layout (vendor/server/src/map/packets/s2c/0x0c8_group_tbl.h):
    ///   [0]      Kind: u8 (0 = none, 1 = party, 2 = alliance)
    ///   [1..4]   padding
    ///   [4..]    array of up to 20 GROUP_TBL entries, 12 bytes each:
    ///     [0..4]   UniqueNo: u32
    ///     [4..6]   ActIndex: u16
    ///     [6]      flags byte (PartyNo[0:1], PartyLeaderFlg[2], AllianceLeaderFlg[3], ...)
    ///     [7]      padding
    ///     [8..10]  ZoneNo: u16
    ///     [10..12] padding
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < 4 {
            return Err(DecodeError::Truncated(4, body.len()));
        }
        let kind = match body[0] {
            0 => GroupKind::None,
            1 => GroupKind::Party,
            2 => GroupKind::Alliance,
            v => GroupKind::Unknown(v),
        };

        const ENTRY_SIZE: usize = 12;
        const ENTRY_START: usize = 4;
        const MAX_ENTRIES: usize = 20;

        let mut members = Vec::new();
        let entry_data = &body[ENTRY_START..];
        let entry_count = (entry_data.len() / ENTRY_SIZE).min(MAX_ENTRIES);

        for i in 0..entry_count {
            let off = i * ENTRY_SIZE;
            if off + ENTRY_SIZE > entry_data.len() {
                break;
            }
            let e = &entry_data[off..off + ENTRY_SIZE];
            let unique_no = u32::from_le_bytes(e[0..4].try_into().unwrap());
            if unique_no == 0 {
                // Remaining entries are empty (zero-padded).
                continue;
            }
            let act_index = u16::from_le_bytes(e[4..6].try_into().unwrap());
            let flags = e[6];
            let party_no = flags & 0x03;
            let is_party_leader = (flags >> 2) & 1 == 1;
            let is_alliance_leader = (flags >> 3) & 1 == 1;
            let zone_no = u16::from_le_bytes(e[8..10].try_into().unwrap());
            members.push(GroupTblEntry {
                unique_no,
                act_index,
                party_no,
                is_party_leader,
                is_alliance_leader,
                zone_no,
            });
        }

        Ok(Self { kind, members })
    }
}

// ---- GROUP_LIST (0x0DD) / GROUP_ATTR (0x0DF) --------------------------------

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

    pub moghouse_flg: u8,
    pub zone_no: u16,
    pub mjob_no: u8,
    pub mjob_lv: u8,
    pub sjob_no: u8,
    pub sjob_lv: u8,
}

#[derive(Debug, Clone)]
pub struct PartyListExtra {
    pub member_number: u8,
    pub is_party_leader: bool,
    pub is_alliance_leader: bool,

    /// `GAttr.PartyNo`: which party of the alliance this member sits in — 0..2,
    /// or 3 for "no party". vendor/server/src/map/packets/s2c/0x0dd_group_list.cpp:40.
    /// Retail compares it against the first member's to tell an alliance-mate's
    /// claim from a party-mate's (research/XIClient/.../ActorTelemetry.cpp:1706).
    pub party_no: u8,

    pub name: Option<String>,
}

impl PartyAttrs {
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
            moghouse_flg: body[21],
            zone_no: u16::from_le_bytes(body[22..24].try_into().unwrap()),
            mjob_no: body[28],
            mjob_lv: body[29],
            sjob_no: body[30],
            sjob_lv: body[31],
        })
    }

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
            moghouse_flg: body[23],
            zone_no: u16::from_le_bytes(body[28..30].try_into().unwrap()),
            mjob_no: body[30],
            mjob_lv: body[31],
            sjob_no: body[32],
            sjob_lv: body[33],
        };
        let gattr = u32::from_le_bytes(body[16..20].try_into().unwrap());

        const PARTY_NO_MASK: u32 = 0x03;
        let party_no = (gattr & PARTY_NO_MASK) as u8;
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
            party_no,
            name,
        };
        Ok((attrs, extra))
    }
}

#[cfg(test)]
mod party_attrs_tests {
    use super::*;

    #[test]
    fn party_attrs_group_attr_decodes() {
        let mut buf = vec![0u8; 36];
        buf[0..4].copy_from_slice(&0x0001_0042u32.to_le_bytes());
        buf[4..8].copy_from_slice(&1500u32.to_le_bytes());
        buf[8..12].copy_from_slice(&500u32.to_le_bytes());
        buf[12..16].copy_from_slice(&1234u32.to_le_bytes());
        buf[16..18].copy_from_slice(&0x0042u16.to_le_bytes());
        buf[18] = 75;
        buf[19] = 50;
        buf[20] = 0;
        buf[21] = 1;
        buf[22..24].copy_from_slice(&234u16.to_le_bytes());
        buf[28] = 6;
        buf[29] = 75;
        buf[30] = 1;
        buf[31] = 37;

        let p = PartyAttrs::decode_group_attr(&buf).unwrap();
        assert_eq!(p.unique_no, 0x0001_0042);
        assert_eq!(p.hp, 1500);
        assert_eq!(p.mp, 500);
        assert_eq!(p.tp, 1234);
        assert_eq!(p.act_index, 0x42);
        assert_eq!(p.hpp, 75);
        assert_eq!(p.mpp, 50);
        assert_eq!(p.moghouse_flg, 1);
        assert_eq!(p.zone_no, 234);
        assert_eq!(p.mjob_no, 6);
        assert_eq!(p.mjob_lv, 75);
        assert_eq!(p.sjob_no, 1);
        assert_eq!(p.sjob_lv, 37);
    }

    #[test]
    fn party_attrs_group_list_decodes_with_name_and_leader() {
        let mut buf = vec![0u8; 56];
        buf[0..4].copy_from_slice(&0x0010_0001u32.to_le_bytes());
        buf[4..8].copy_from_slice(&2000u32.to_le_bytes());
        buf[8..12].copy_from_slice(&100u32.to_le_bytes());
        buf[12..16].copy_from_slice(&0u32.to_le_bytes());

        buf[16..20].copy_from_slice(&0x0000_0005u32.to_le_bytes());
        buf[20..22].copy_from_slice(&0x0007u16.to_le_bytes());
        buf[22] = 1;
        buf[23] = 1;
        buf[24] = 0;
        buf[25] = 100;
        buf[26] = 100;
        buf[28..30].copy_from_slice(&230u16.to_le_bytes());
        buf[30] = 1;
        buf[31] = 75;
        buf[36..36 + 6].copy_from_slice(b"Vanari");

        let (attrs, extra) = PartyAttrs::decode_group_list(&buf).unwrap();
        assert_eq!(attrs.unique_no, 0x0010_0001);
        assert_eq!(attrs.hp, 2000);
        assert_eq!(attrs.act_index, 7);
        assert_eq!(attrs.zone_no, 230);
        assert_eq!(attrs.moghouse_flg, 1);
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

#[cfg(test)]
mod group_tbl_tests {
    use super::*;

    fn entry(unique_no: u32, act_index: u16, flags: u8, zone_no: u16) -> [u8; 12] {
        let mut e = [0u8; 12];
        e[0..4].copy_from_slice(&unique_no.to_le_bytes());
        e[4..6].copy_from_slice(&act_index.to_le_bytes());
        e[6] = flags;
        e[8..10].copy_from_slice(&zone_no.to_le_bytes());
        e
    }

    #[test]
    fn group_tbl_solo_nullptr_is_kind_none_zero_members() {
        // LSB's solo answer to 0x076: pushPacket<GROUP_TBL>(nullptr) — Kind 0,
        // all 20 slots zero-filled.
        let mut body = vec![0u8; 4 + 20 * 12];
        body[0] = 0;
        let tbl = GroupTbl::decode(&body).unwrap();
        assert_eq!(tbl.kind, GroupKind::None);
        assert!(tbl.members.is_empty());

        // The minimal shape (kind + pad only) decodes the same.
        let tbl = GroupTbl::decode(&[0, 0, 0, 0]).unwrap();
        assert_eq!(tbl.kind, GroupKind::None);
        assert!(tbl.members.is_empty());
    }

    #[test]
    fn group_tbl_party_parses_flags_and_zones() {
        let mut body = vec![0u8; 4 + 20 * 12];
        body[0] = 1;
        // party_no=1, PartyLeaderFlg set (bit2) -> flags 0b101
        body[4..16].copy_from_slice(&entry(0x0010_0042, 7, 0b101, 235));
        // party_no=2 (bits0-1), no leader flags; ZoneNo 0 is legal here
        body[16..28].copy_from_slice(&entry(0x0010_0007, 9, 0b010, 0));
        // slot 3 stays zero-filled -> stop

        let tbl = GroupTbl::decode(&body).unwrap();
        assert_eq!(tbl.kind, GroupKind::Party);
        assert_eq!(tbl.members.len(), 2);

        let self_row = &tbl.members[0];
        assert_eq!(self_row.unique_no, 0x0010_0042);
        assert_eq!(self_row.act_index, 7);
        assert_eq!(self_row.party_no, 1);
        assert!(self_row.is_party_leader);
        assert!(!self_row.is_alliance_leader);
        assert_eq!(self_row.zone_no, 235);

        let mate = &tbl.members[1];
        assert_eq!(mate.unique_no, 0x0010_0007);
        assert_eq!(mate.party_no, 2);
        assert!(!mate.is_party_leader);
        assert_eq!(mate.zone_no, 0);
    }

    #[test]
    fn group_tbl_alliance_kind_and_truncated() {
        let mut body = vec![0u8; 4 + 12];
        body[0] = 2;
        body[4..16].copy_from_slice(&entry(5, 1, 0b1000, 1)); // AllianceLeaderFlg (bit3)
        let tbl = GroupTbl::decode(&body).unwrap();
        assert_eq!(tbl.kind, GroupKind::Alliance);
        assert!(tbl.members[0].is_alliance_leader);

        assert!(matches!(
            GroupTbl::decode(&[0, 0, 0]),
            Err(DecodeError::Truncated(4, 3))
        ));
    }
}
