//! The alter ego point (Trust upgrade) tables behind the `fp_cat` / `fp_cat2` windows
//! (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md). Every id
//! resolves through the install's FTABLE like any other file id.

/// Category table: its first 288 bytes are [`CATEGORY_RECORDS`] records of
/// [`CATEGORY_RECORD_LEN`] bytes.
pub const CATEGORY_TABLE_FILE_ID: u32 = 98;

/// Upgrade records: [`UPGRADE_RECORD_LEN`] bytes per upgrade id, byte
/// [`UPGRADE_CAP_BYTE`] holding that upgrade's level cap.
pub const UPGRADE_TABLE_FILE_ID: u32 = 97;

/// Names and descriptions on an English install (a Japanese install reads 55563 instead): entry
/// `2n` names id `n`, entry `2n + 1` describes it, for category ids and upgrade ids alike.
pub const STRINGS_FILE_ID_ENGLISH: u32 = 55683;

pub const CATEGORY_RECORDS: usize = 8;
pub const CATEGORY_RECORD_LEN: usize = 36;
/// Upgrade ids per category record, `0` marking an empty slot.
pub const CATEGORY_UPGRADE_SLOTS: usize = 16;
const CATEGORY_ID_OFFSET: usize = 0;
const CATEGORY_UPGRADES_OFFSET: usize = 4;

pub const UPGRADE_RECORD_LEN: usize = 4;
pub const UPGRADE_CAP_BYTE: usize = 3;

const _: () = assert!(
    CATEGORY_UPGRADES_OFFSET + 2 * CATEGORY_UPGRADE_SLOTS == CATEGORY_RECORD_LEN,
    "u16 category id, u16 unused, u16 upgrade ids[16]"
);

/// One category record: its id and its upgrade ids in slot order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlterEgoCategory {
    pub id: u16,
    pub upgrades: [u16; CATEGORY_UPGRADE_SLOTS],
}

/// The category records in file order; a record cut short by the file is dropped.
pub fn parse_categories(bytes: &[u8]) -> Vec<AlterEgoCategory> {
    let rd16 = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
    (0..CATEGORY_RECORDS)
        .map(|record| record * CATEGORY_RECORD_LEN)
        .take_while(|&start| start + CATEGORY_RECORD_LEN <= bytes.len())
        .map(|start| {
            let mut upgrades = [0u16; CATEGORY_UPGRADE_SLOTS];
            for (slot, upgrade) in upgrades.iter_mut().enumerate() {
                *upgrade = rd16(start + CATEGORY_UPGRADES_OFFSET + 2 * slot);
            }
            AlterEgoCategory {
                id: rd16(start + CATEGORY_ID_OFFSET),
                upgrades,
            }
        })
        .collect()
}

/// The level cap of `upgrade_id` from the upgrade record file, `None` past its end.
pub fn upgrade_cap(bytes: &[u8], upgrade_id: u16) -> Option<u8> {
    bytes
        .get(usize::from(upgrade_id) * UPGRADE_RECORD_LEN + UPGRADE_CAP_BYTE)
        .copied()
}

/// String entry naming id `id` in [`STRINGS_FILE_ID_ENGLISH`].
pub fn name_entry(id: u16) -> usize {
    crate::dmsg::paired_name_entry(id)
}

/// String entry describing id `id` in [`STRINGS_FILE_ID_ENGLISH`].
pub fn description_entry(id: u16) -> usize {
    crate::dmsg::paired_description_entry(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: u16, upgrades: &[u16]) -> Vec<u8> {
        let mut out = vec![0u8; CATEGORY_RECORD_LEN];
        out[..2].copy_from_slice(&id.to_le_bytes());
        for (slot, upgrade) in upgrades.iter().enumerate() {
            let o = CATEGORY_UPGRADES_OFFSET + 2 * slot;
            out[o..o + 2].copy_from_slice(&upgrade.to_le_bytes());
        }
        out
    }

    #[test]
    fn categories_read_in_file_order_with_sixteen_upgrade_slots() {
        let mut bytes = record(0, &[]);
        bytes.extend(record(1, &[8, 9]));
        bytes.extend(record(2, &[10, 11, 12, 13, 14, 15, 16]));
        bytes.extend(vec![0u8; CATEGORY_RECORD_LEN - 1]);
        let categories = parse_categories(&bytes);
        assert_eq!(categories.len(), 3, "the truncated fourth record drops");
        assert_eq!(categories[1].id, 1);
        assert_eq!(&categories[1].upgrades[..3], &[8, 9, 0]);
        assert_eq!(categories[2].upgrades[6], 16);
    }

    #[test]
    fn only_the_first_eight_records_count() {
        let bytes: Vec<u8> = (1..=CATEGORY_RECORDS as u16 + 2)
            .flat_map(|id| record(id, &[id]))
            .collect();
        assert_eq!(parse_categories(&bytes).len(), CATEGORY_RECORDS);
    }

    #[test]
    fn upgrade_cap_is_byte_three_of_the_id_record() {
        const HP_UPGRADE: u16 = 8;
        let mut bytes = vec![0u8; UPGRADE_RECORD_LEN * 20];
        bytes[usize::from(HP_UPGRADE) * UPGRADE_RECORD_LEN + UPGRADE_CAP_BYTE] = 50;
        assert_eq!(upgrade_cap(&bytes, HP_UPGRADE), Some(50));
        assert_eq!(upgrade_cap(&bytes, 9), Some(0));
        assert_eq!(upgrade_cap(&bytes, 20), None);
    }

    #[test]
    fn strings_pair_name_and_description_per_id() {
        assert_eq!((name_entry(17), description_entry(17)), (34, 35));
    }
}
