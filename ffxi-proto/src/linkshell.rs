//! Link-shell (link pearl) wire law: c2s 0x0C3/0x0C4 and s2c 0x0E0.
//!
//! Authority is the server source: vendor/server/src/map/packets/c2s/0x0c3_group_comlink_make.h,
//! vendor/server/src/map/packets/c2s/0x0c4_group_comlink_active.h and
//! vendor/server/src/map/packets/s2c/0x0e0_group_comlink.cpp; the client side of the family is the
//! `lscs`/`linkN` window group documented in vault main_menu.md §27 (page-1 row 10's arm
//! FFXiMain.dll retail-2026-09 RVA 0x1405EE, helper FFXiMain.dll retail-2026-09 RVA 0x1E4470).

use crate::linkshell_name;

/// `GP_CLI_COMMAND_GROUP_COMLINK_MAKE` (vendor/server/src/map/packets/c2s/0x0c3_group_comlink_make.h):
/// asks the server to make a link pearl for an equipped linkshell. Body `{ State:u8, LinkshellId:u8 }`.
pub const C2S_GROUP_COMLINK_MAKE: u16 = 0x0C3;

/// `GP_CLI_COMMAND_GROUP_COMLINK_ACTIVE` (vendor/server/src/map/packets/c2s/0x0c4_group_comlink_active.h):
/// create a linkshell, or equip/unequip a pearl into one of the two shell slots.
pub const C2S_GROUP_COMLINK_ACTIVE: u16 = 0x0C4;

/// `GP_SERV_COMMAND_GROUP_COMLINK` (vendor/server/src/map/packets/s2c/0x0e0_group_comlink.h):
/// "the pearl equipped in this shell slot is now this inventory item" — pushed on login for both
/// slots (charutils.cpp) and after every equip/unequip/create/join.
pub const S2C_GROUP_COMLINK: u16 = 0x0E0;

/// Body length of `GP_CLI_COMMAND_GROUP_COMLINK_ACTIVE::PacketData` in declaration order:
/// nibble-packed colour (2), ItemIndex, Category, ActiveFlg, pad[3], sComLinkName[15], LinkshellId.
pub const ACTIVE_BODY_LEN: usize = 24;

/// Body length of `GP_SERV_COMMAND_GROUP_COMLINK::PacketData`:
/// `{ LinkshellNum:u8, ItemIndex:u8, Category:u8, padding00:u8 }`.
pub const COMLINK_BODY_LEN: usize = 4;

/// `GP_CLI_COMMAND_GROUP_COMLINK_ACTIVE_LINKSHELLID` / the same enum in 0x0C3: which of the two
/// shell slots a request targets. Server-side these map to `SLOT_BACK + LinkshellId`
/// (i.e. `SLOT_LINK1`/`SLOT_LINK2`).
pub const LINKSHELL_1: u8 = 1;
pub const LINKSHELL_2: u8 = 2;

/// Bytes of the packed name field in c2s 0x0C4 (`sComLinkName[15]`).
pub const NAME_FIELD_LEN: usize = linkshell_name::ACTIVE_PACKED_LEN;

/// `GP_CLI_COMMAND_GROUP_COMLINK_ACTIVE_ACTIVEFLG`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveFlg {
    /// Take the pearl out of this shell slot.
    Unequip = 0,
    /// Put the pearl in this shell slot — and create the shell if it has no name yet.
    EquipOrCreate = 1,
}

impl ActiveFlg {
    pub fn to_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Unequip),
            1 => Some(Self::EquipOrCreate),
            _ => None,
        }
    }
}

/// Field offsets inside the 24-byte c2s 0x0C4 body.
pub const ACTIVE_OFF_COLOR: usize = 0;
pub const ACTIVE_OFF_ITEM_INDEX: usize = 2;
pub const ACTIVE_OFF_CATEGORY: usize = 3;
pub const ACTIVE_OFF_FLG: usize = 4;
pub const ACTIVE_OFF_NAME: usize = 8;
pub const ACTIVE_OFF_SLOT: usize = 23;

/// Errors a body can refuse to decode as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkshellError {
    TooShort { want: usize, got: usize },
}

impl std::fmt::Display for LinkshellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort { want, got } => {
                write!(f, "link-shell body too short: need {want} bytes, got {got}")
            }
        }
    }
}

impl std::error::Error for LinkshellError {}

/// Encoded c2s 0x0C4 body (no sub-packet header — the session codec frames it).
///
/// `color` is the pearl colour as four 4-bit nibbles. The server rebuilds it as
/// `(a << 12) | (b << 8) | (g << 4) | r`, so byte 0 carries `r|g<<4` and byte 1 `b|a<<4`.
pub fn encode_group_comlink_active(
    color: [u8; 4],
    item_index: u8,
    category: u8,
    active: ActiveFlg,
    name: &str,
    linkshell_slot: u8,
) -> [u8; ACTIVE_BODY_LEN] {
    let mut body = [0u8; ACTIVE_BODY_LEN];
    let [r, g, b, a] = color;
    body[ACTIVE_OFF_COLOR] = (r & 0x0F) | ((g & 0x0F) << 4);
    body[ACTIVE_OFF_COLOR + 1] = (b & 0x0F) | ((a & 0x0F) << 4);
    body[ACTIVE_OFF_ITEM_INDEX] = item_index;
    body[ACTIVE_OFF_CATEGORY] = category;
    body[ACTIVE_OFF_FLG] = active.to_u8();
    linkshell_name::encode_into(
        name,
        &mut body[ACTIVE_OFF_NAME..ACTIVE_OFF_NAME + NAME_FIELD_LEN],
    );
    body[ACTIVE_OFF_SLOT] = linkshell_slot;
    body
}

/// One s2c 0x0E0. LSB leaves `item_index`/`category` at 0 when the slot holds nothing
/// (`equipLocation(SLOT_LINK1/SLOT_LINK2)` returned no entry), so callers treat 0/0 as empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupComlink {
    /// `LinkshellNum`: 1 or 2 (`LINKSHELL_1`/`LINKSHELL_2`).
    pub linkshell_slot: u8,
    pub item_index: u8,
    pub category: u8,
}

/// Decode an s2c 0x0E0 body.
pub fn parse_group_comlink(body: &[u8]) -> Result<GroupComlink, LinkshellError> {
    if body.len() < COMLINK_BODY_LEN {
        return Err(LinkshellError::TooShort {
            want: COMLINK_BODY_LEN,
            got: body.len(),
        });
    }
    Ok(GroupComlink {
        linkshell_slot: body[0],
        item_index: body[1],
        category: body[2],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_body_matches_the_lsb_field_layout_and_colour_nibbles() {
        // Colour law from the create/equip lambda: (a<<12)|(b<<8)|(g<<4)|r.
        let body = encode_group_comlink_active(
            [0x1, 0x2, 0x3, 0x4],
            7,
            8,
            ActiveFlg::EquipOrCreate,
            "Kuluu",
            LINKSHELL_2,
        );
        assert_eq!(body[ACTIVE_OFF_COLOR..ACTIVE_OFF_COLOR + 2], [0x21, 0x43]);
        assert_eq!(body[ACTIVE_OFF_ITEM_INDEX], 7);
        assert_eq!(body[ACTIVE_OFF_CATEGORY], 8);
        assert_eq!(body[ACTIVE_OFF_FLG], 1);
        assert_eq!(&body[5..8], &[0, 0, 0], "padding stays zero");
        assert_eq!(body[ACTIVE_OFF_SLOT], LINKSHELL_2);
        let field = &body[ACTIVE_OFF_NAME..ACTIVE_OFF_NAME + NAME_FIELD_LEN];
        // Same hex the name codec pins on its own: LSB's LE bit window, not naive packing.
        assert_eq!(&field[..5], &[0x95, 0x53, 0x15, 0x57, 0xF0]);
        assert_eq!(linkshell_name::decode(field), "Kuluu");
    }

    #[test]
    fn unequip_request_carries_no_name() {
        let body =
            encode_group_comlink_active([0, 0, 0, 0], 7, 8, ActiveFlg::Unequip, "", LINKSHELL_1);
        assert_eq!(body[ACTIVE_OFF_FLG], 0);
        let field = &body[ACTIVE_OFF_NAME..ACTIVE_OFF_NAME + NAME_FIELD_LEN];
        // EncodeStringLinkshell always writes the end marker, even for an empty name — that is six
        // 1-bits in byte 0 (LSB's `leftover == 8 ? 6 : leftover` rule), not a zeroed field.
        assert_eq!(field[0], 0xFC);
        assert_eq!(&field[1..], &[0u8; NAME_FIELD_LEN - 1]);
        assert_eq!(linkshell_name::decode(field), "");
        assert_eq!(
            ActiveFlg::from_u8(body[ACTIVE_OFF_FLG]),
            Some(ActiveFlg::Unequip)
        );
    }

    #[test]
    fn group_comlink_decodes_its_four_byte_body() {
        // Login push for slot 2 with the pearl at container 0 (inventory) index 12.
        assert_eq!(
            parse_group_comlink(&[2, 12, 0, 0]),
            Ok(GroupComlink {
                linkshell_slot: LINKSHELL_2,
                item_index: 12,
                category: 0,
            })
        );
        // Empty slot: server leaves index/category zeroed.
        assert_eq!(
            parse_group_comlink(&[1, 0, 0, 0]),
            Ok(GroupComlink {
                linkshell_slot: LINKSHELL_1,
                item_index: 0,
                category: 0,
            })
        );
    }

    #[test]
    fn short_group_comlink_body_is_refused() {
        assert_eq!(
            parse_group_comlink(&[1, 0]),
            Err(LinkshellError::TooShort {
                want: COMLINK_BODY_LEN,
                got: 2
            })
        );
    }

    #[test]
    fn opcode_constants_match_the_lsb_packet_enums() {
        assert_eq!(
            (
                C2S_GROUP_COMLINK_MAKE,
                C2S_GROUP_COMLINK_ACTIVE,
                S2C_GROUP_COMLINK
            ),
            (0x0C3, 0x0C4, 0x0E0)
        );
    }
}
