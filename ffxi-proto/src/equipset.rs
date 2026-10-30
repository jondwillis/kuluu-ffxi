//! Equipment sets: one set record's contents half, and the three wire bodies that wear it, lock style to
//! it, or report an edit of it.
//!
//! Every shape here is byte for byte LandSandBoat's (`vendor/server/src/map/packets/c2s/0x051_equipset_set.h`,
//! `0x052_equipset_check.h`, `0x053_lockstyle.h`). How the retail client fills those fields (the record position
//! carried in `EquipKind`, style lock admitting only positions through feet, nothing applied locally) is
//! established by `.agents/skills/retail-observe/references/2026-10-25-equip-set-apply.md`.

use serde::{Deserialize, Serialize};

/// Entries in one set's contents array: `Equipment[16]` / `Items[16]` in all three packet headers.
pub const EQUIPSET_ENTRIES: usize = 16;
/// Bytes per contents entry (`sizeof(equipsetrequestitem_t)`): flags-and-category byte, index byte, item id word.
pub const EQUIPSET_ENTRY_BYTES: usize = 4;
/// `HasItemFlg`, bit 0 of an entry's first byte.
pub const FLAG_HAS_ITEM: u8 = 1 << 0;
/// `RemoveItemFlg`, bit 1. It stands alone on a cleared or removed entry, the pair that LSB annotates as
/// "(01-equip 0-2 remve)" (`vendor/server/src/map/packets/c2s/0x052_equipset_check.cpp
/// GP_CLI_COMMAND_EQIPSET_CHECK::process`).
pub const FLAG_REMOVE_ITEM: u8 = 1 << 1;
/// `Category` occupies bits 2..7 of that byte, so a container id above [`CATEGORY_MAX`] cannot be carried.
const CATEGORY_SHIFT: u32 = 2;
const CATEGORY_MAX: u8 = u8::MAX >> CATEGORY_SHIFT;

/// One entry of a set's contents: what one equipment position wears, held identically in the on-disk record
/// and on the wire (`equipsetrequestitem_t`, `vendor/server/src/map/packets/c2s/0x052_equipset_check.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EquipSetEntry {
    /// The entry names an item. Only entries with it travel in a wear or style-lock packet.
    pub has_item: bool,
    /// Set on an entry that takes the position back (see [`EquipSetEntry::wire_change`] and
    /// [`EquipSetEntry::cleared`]).
    pub remove_item: bool,
    /// CONTAINER_ID of the bag holding the item; bounded by `PacketValidator::isValidContainer`.
    pub category: u8,
    /// Index of the item inside that container.
    pub item_index: u8,
    /// The item id.
    pub item_no: u16,
}

impl EquipSetEntry {
    /// A virgin record entry reads as all zero (observation record, contents law).
    pub const EMPTY: Self = Self {
        has_item: false,
        remove_item: false,
        category: 0,
        item_index: 0,
        item_no: 0,
    };

    /// The populated test every retail packet loop runs.
    pub fn is_populated(&self) -> bool {
        self.has_item
    }

    /// A worn or staged entry: populated, remove flag off.
    pub fn worn(category: u8, item_index: u8, item_no: u16) -> Self {
        Self {
            has_item: true,
            remove_item: false,
            category,
            item_index,
            item_no,
        }
    }

    /// What wiping an entry writes back: the flag pair restated with only `RemoveItemFlg` standing and
    /// `item_index` / `item_no` zeroed, so a cleared position tells "take what was worn off" rather than
    /// reading as a virgin record (observation record, contents law).
    pub fn cleared() -> Self {
        Self {
            has_item: false,
            remove_item: true,
            category: 0,
            item_index: 0,
            item_no: 0,
        }
    }

    /// The `ItemChange` field of c2s 0x052 carries the entry just placed or removed with its flag bits
    /// normalized: a nonzero id travels populated without the remove flag; an empty one as a removal that
    /// keeps only its category.
    pub fn wire_change(&self) -> Self {
        if self.item_no == 0 {
            Self {
                has_item: false,
                remove_item: true,
                category: self.category,
                item_index: 0,
                item_no: 0,
            }
        } else {
            Self {
                has_item: true,
                remove_item: false,
                ..*self
            }
        }
    }

    /// The four bytes as the set record stores them.
    pub fn to_record_bytes(self) -> [u8; EQUIPSET_ENTRY_BYTES] {
        let flags = ((self.has_item as u8) * FLAG_HAS_ITEM)
            | ((self.remove_item as u8) * FLAG_REMOVE_ITEM)
            | (self.category.min(CATEGORY_MAX) << CATEGORY_SHIFT);
        let item_no = self.item_no.to_le_bytes();
        [flags, self.item_index, item_no[0], item_no[1]]
    }

    /// Parse four record bytes back into an entry.
    pub fn from_record_bytes(bytes: [u8; EQUIPSET_ENTRY_BYTES]) -> Self {
        Self {
            has_item: bytes[0] & FLAG_HAS_ITEM != 0,
            remove_item: bytes[0] & FLAG_REMOVE_ITEM != 0,
            category: bytes[0] >> CATEGORY_SHIFT,
            item_index: bytes[1],
            item_no: u16::from_le_bytes([bytes[2], bytes[3]]),
        }
    }
}

/// c2s 0x051 body layout (`GP_CLI_COMMAND_EQUIPSET_SET`): `Count` at packet offset 4, three pad bytes, then
/// the sixteen entry slots from packet offset 8. Retail commits a fixed total regardless of `Count`.
pub const SET_COUNT_OFFSET: usize = 0;
pub const SET_ITEMS_OFFSET: usize = 4;
pub const SET_BODY_BYTES: usize = SET_ITEMS_OFFSET + EQUIPSET_ENTRIES * EQUIPSET_ENTRY_BYTES;

/// Build the c2s 0x051 body. Populated entries pack to the front in record order, each carrying its record
/// position as `EquipKind` (the field LSB hands straight to `charutils::EquipItem`,
/// `vendor/server/src/map/packets/c2s/0x051_equipset_set.cpp GP_CLI_COMMAND_EQUIPSET_SET::process`); the count
/// counts those packed entries, so no populated entry can sit past it unread.
pub fn equipset_set_body(entries: &[EquipSetEntry; EQUIPSET_ENTRIES]) -> [u8; SET_BODY_BYTES] {
    let mut body = [0u8; SET_BODY_BYTES];
    let mut count = 0usize;
    for (position, entry) in entries.iter().enumerate() {
        if !entry.is_populated() || count >= EQUIPSET_ENTRIES {
            continue;
        }
        let at = SET_ITEMS_OFFSET + count * EQUIPSET_ENTRY_BYTES;
        body[at] = entry.item_index;
        body[at + 1] = position as u8;
        body[at + 2] = entry.category;
        count += 1;
    }
    body[SET_COUNT_OFFSET] = count as u8;
    body
}

/// c2s 0x053 body layout (`GP_CLI_COMMAND_LOCKSTYLE`). Mode "Set" comes from `GP_CLI_COMMAND_LOCKSTYLE_MODE`;
/// the client gate admits exactly positions through feet, matching the server dropping anything past
/// `SLOT_FEET` (`vendor/server/src/map/entities/battle_entity.h`,
/// `0x053_lockstyle.cpp GP_CLI_COMMAND_LOCKSTYLE::process`).
pub const LOCKSTYLE_MODE_SET: u8 = 3;
pub const LOCKSTYLE_LAST_POSITION: usize = 8;
/// One `lockstyleitem_t`: index, kind, category, pad, then the item id word and two more pad bytes.
pub const LOCKSTYLE_ENTRY_BYTES: usize = 8;
const LOCKSTYLE_COUNT_OFFSET: usize = 0;
const LOCKSTYLE_MODE_OFFSET: usize = 1;
const LOCKSTYLE_ITEMS_OFFSET: usize = 4;
/// The whole sixteen-slot body, filled or not (retail's commit size is the fixed full struct).
pub const LOCKSTYLE_BODY_BYTES: usize =
    LOCKSTYLE_ITEMS_OFFSET + EQUIPSET_ENTRIES * LOCKSTYLE_ENTRY_BYTES;

/// Build the c2s 0x053 Mode=Set body. Entries past position [`LOCKSTYLE_LAST_POSITION`] never travel, so a set
/// may carry sixteen and still lock style on at most nine. The `Flags` byte stays zero: its single toggled bit
/// belongs to a caller argument no kuluu arm passes (observation record, style-lock law).
pub fn lockstyle_set_body(
    entries: &[EquipSetEntry; EQUIPSET_ENTRIES],
) -> [u8; LOCKSTYLE_BODY_BYTES] {
    let mut body = [0u8; LOCKSTYLE_BODY_BYTES];
    let mut count = 0usize;
    for (position, entry) in entries.iter().enumerate() {
        if !entry.is_populated() || position > LOCKSTYLE_LAST_POSITION {
            continue;
        }
        let at = LOCKSTYLE_ITEMS_OFFSET + count * LOCKSTYLE_ENTRY_BYTES;
        body[at] = entry.item_index;
        body[at + 1] = position as u8;
        body[at + 2] = entry.category;
        body[at + 4..at + 6].copy_from_slice(&entry.item_no.to_le_bytes());
        count += 1;
    }
    body[LOCKSTYLE_COUNT_OFFSET] = count as u8;
    body[LOCKSTYLE_MODE_OFFSET] = LOCKSTYLE_MODE_SET;
    body
}

/// How many entries a style-lock body can carry from these contents: what an arm checks before asking to send.
pub fn lockstyle_admissible_count(entries: &[EquipSetEntry; EQUIPSET_ENTRIES]) -> usize {
    entries
        .iter()
        .enumerate()
        .filter(|(position, entry)| entry.is_populated() && *position <= LOCKSTYLE_LAST_POSITION)
        .count()
}

/// c2s 0x052 body layout (`GP_CLI_COMMAND_EQUIPSET_CHECK`): `EquipKind`, the edited position (LSB bounds it to
/// 0..15 in `validate`), then a single item-change entry, then the whole contents list. LSB answers with s2c
/// 0x116 EQUIPSET_VALID (`vendor/server/src/map/packets/c2s/0x052_equipset_check.cpp GP_CLI_COMMAND_EQIPSET_CHECK::process`).
pub const CHECK_POSITION_OFFSET: usize = 0;
const CHECK_CHANGE_OFFSET: usize = 4;
const CHECK_LIST_OFFSET: usize = 8;
/// The highest position `EquipKind` may name (the list is one entry per equipment position).
pub const CHECK_LAST_POSITION: u8 = (EQUIPSET_ENTRIES - 1) as u8;
pub const CHECK_BODY_BYTES: usize = CHECK_LIST_OFFSET + EQUIPSET_ENTRIES * EQUIPSET_ENTRY_BYTES;

/// Build the c2s 0x052 body: "here is what changed, and here is the whole set". The change entry goes through
/// [`EquipSetEntry::wire_change`]; the list travels as stored. Nothing is worn by this packet on either side of it.
pub fn equipset_check_body(
    position: u8,
    change: &EquipSetEntry,
    contents: &[EquipSetEntry; EQUIPSET_ENTRIES],
) -> [u8; CHECK_BODY_BYTES] {
    let mut body = [0u8; CHECK_BODY_BYTES];
    body[CHECK_POSITION_OFFSET] = position.min(CHECK_LAST_POSITION);
    body[CHECK_CHANGE_OFFSET..CHECK_CHANGE_OFFSET + EQUIPSET_ENTRY_BYTES]
        .copy_from_slice(&change.wire_change().to_record_bytes());
    for (index, entry) in contents.iter().enumerate() {
        let at = CHECK_LIST_OFFSET + index * EQUIPSET_ENTRY_BYTES;
        body[at..at + EQUIPSET_ENTRY_BYTES].copy_from_slice(&entry.to_record_bytes());
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte 0 of a record entry carries the two flag bits and then the container id in the remaining six, so
    /// wardrobe (8) rides on a populated entry as `1 | 8 << 2` (`equipsetrequestitem_t`, observation-record
    /// contents table).
    #[test]
    fn a_record_entry_packs_flags_then_category_into_byte_zero() {
        let entry = EquipSetEntry::worn(8, 3, 0xBEEF);
        let bytes = entry.to_record_bytes();
        assert_eq!(bytes[0], FLAG_HAS_ITEM | (8 << CATEGORY_SHIFT));
        assert_eq!(bytes[1], 3);
        assert_eq!([bytes[2], bytes[3]], [0xEF, 0xBE]);
        assert_eq!(EquipSetEntry::from_record_bytes(bytes), entry);
        // A cleared position keeps only the remove flag.
        let cleared = EquipSetEntry::cleared();
        assert_eq!(
            EquipSetEntry::from_record_bytes(cleared.to_record_bytes()),
            cleared
        );
        assert!(!cleared.is_populated());
    }

    /// c2s 0x051: populated entries pack to the front carrying their record position as `EquipKind`, count is
    /// how many packed, and the total stays fixed whatever the sparseness (`equipsetitem_t`).
    #[test]
    fn equipset_set_packs_populated_entries_and_counts_them() {
        let mut entries = [EquipSetEntry::EMPTY; EQUIPSET_ENTRIES];
        entries[0] = EquipSetEntry::worn(0, 3, 45); // main hand, inventory slot 3
        entries[9] = EquipSetEntry::worn(8, 7, 1_000); // position 9, wardrobe
        let body = equipset_set_body(&entries);
        assert_eq!(body.len(), SET_BODY_BYTES);
        assert_eq!(
            SET_BODY_BYTES + 4,
            72,
            "retail commits a fixed 72-byte packet"
        );
        assert_eq!(body[SET_COUNT_OFFSET], 2);
        let first = &body[SET_ITEMS_OFFSET..SET_ITEMS_OFFSET + 4];
        assert_eq!(first, &[3, 0, 0, 0]); // ItemIndex, EquipKind=position 0, Category, pad
        let second = &body[SET_ITEMS_OFFSET + 4..SET_ITEMS_OFFSET + 8];
        assert_eq!(second, &[7, 9, 8, 0]); // position 9 travels as its own slot id
                                           // Nothing past the packed entries is written.
        assert!(body[SET_ITEMS_OFFSET + 8..].iter().all(|&byte| byte == 0));
    }

    /// c2s 0x053 Mode=Set: nine positions through feet only, each `lockstyleitem_t` eight bytes wide with the
    /// item id as a little-endian word, mode byte at packet offset 5.
    #[test]
    fn lockstyle_set_admits_only_positions_through_feet() {
        let mut entries = [EquipSetEntry::EMPTY; EQUIPSET_ENTRIES];
        for (position, entry) in entries
            .iter_mut()
            .enumerate()
            .take(LOCKSTYLE_LAST_POSITION + 2)
        {
            *entry = EquipSetEntry::worn(0, position as u8, 100 + position as u16);
        }
        // Position 15 carries an item too: it must not be counted nor travel.
        entries[15] = EquipSetEntry::worn(12, 4, 4_242);
        assert_eq!(lockstyle_admissible_count(&entries), 9);
        let body = lockstyle_set_body(&entries);
        assert_eq!(body.len(), LOCKSTYLE_BODY_BYTES);
        assert_eq!(
            LOCKSTYLE_BODY_BYTES + 4,
            136,
            "retail commits a fixed 136-byte packet"
        );
        assert_eq!(body[LOCKSTYLE_COUNT_OFFSET], 9);
        assert_eq!(body[LOCKSTYLE_MODE_OFFSET], LOCKSTYLE_MODE_SET);
        // The ninth position keeps its own kind even though the tenth is populated: nothing shifts past feet.
        let eighth = &body[LOCKSTYLE_ITEMS_OFFSET + 8 * LOCKSTYLE_ENTRY_BYTES
            ..LOCKSTYLE_ITEMS_OFFSET + 9 * LOCKSTYLE_ENTRY_BYTES];
        assert_eq!(&eighth[..4], &[8, 8, 0, 0]);
        assert_eq!(u16::from_le_bytes([eighth[4], eighth[5]]), 108);
        // The slot that would have held position 9 (and every later one) stays zero.
        assert!(body[LOCKSTYLE_ITEMS_OFFSET + 9 * LOCKSTYLE_ENTRY_BYTES..]
            .iter()
            .all(|&byte| byte == 0));
    }

    /// c2s 0x052: the edited position leads, then one normalized change entry, then the full list in record
    /// order as stored (no packing): "here is what changed and here is the full set now".
    #[test]
    fn equipset_check_carries_position_then_change_then_the_whole_list() {
        let mut entries = [EquipSetEntry::EMPTY; EQUIPSET_ENTRIES];
        entries[2] = EquipSetEntry::worn(8, 5, 700);
        // Removing position 9 stages an empty entry: normalized it travels with the remove flag only.
        let removal = EquipSetEntry {
            has_item: false,
            remove_item: false,
            category: 8,
            item_index: 5,
            item_no: 0,
        };
        let body = equipset_check_body(9, &removal, &entries);
        assert_eq!(body.len(), CHECK_BODY_BYTES);
        assert_eq!(
            CHECK_BODY_BYTES + 4,
            76,
            "retail commits a fixed 76-byte packet"
        );
        assert_eq!(body[CHECK_POSITION_OFFSET], 9);
        let change = &body[CHECK_CHANGE_OFFSET..CHECK_CHANGE_OFFSET + EQUIPSET_ENTRY_BYTES];
        assert_eq!(
            &change[..3],
            &[FLAG_REMOVE_ITEM | (8 << CATEGORY_SHIFT), 0, 0]
        );
        // The list keeps record order: position 2's placement stands where it was staged.
        let placed = &body[CHECK_LIST_OFFSET + 2 * EQUIPSET_ENTRY_BYTES
            ..CHECK_LIST_OFFSET + 3 * EQUIPSET_ENTRY_BYTES];
        assert_eq!(
            EquipSetEntry::from_record_bytes(placed.try_into().unwrap()),
            entries[2]
        );
    }
}
