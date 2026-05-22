//! Per-item equip metadata sourced from
//! `vendor/server/sql/item_equipment.sql` at compile time. Drives the
//! HUD's equip-from-inventory picker (Stage 4) — given an `item_id`
//! we report which equipment slot(s) it fits in, which jobs can use
//! it, and the level required.
//!
//! The underlying SQL table is the unified source of truth for armor,
//! accessories, *and* weapons (every equippable item has a row).
//! Items missing from the table (currency, consumables, key items)
//! return `None`.

include!(concat!(env!("OUT_DIR"), "/equip_info_table.rs"));

/// Decoded equip metadata for a single item id.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct EquipInfo {
    pub item_id: u16,
    /// Minimum character level to equip. 0 = no level requirement.
    pub level: u8,
    /// Bitmap of jobs that can equip this item. Bit `N` set means
    /// job id `N` is allowed (bit 0 is LSB's "NONE" sentinel; bits
    /// 1..=22 are WAR..GEO per `vendor/server/scripts/enum/job_name.lua`).
    pub jobs_mask: u32,
    /// Bitmap of equipment slots this item fits in. Bit `N` set means
    /// `SLOTTYPE` slot `N` accepts it (`Main=0`, `Sub=1`, …, `Back=15`).
    pub slot_mask: u16,
}

/// Resolve `item_id` → equip metadata, or `None` if the id isn't
/// present in `item_equipment` (consumables, currency, key items).
pub fn lookup(item_id: u16) -> Option<EquipInfo> {
    EQUIP_INFO
        .binary_search_by_key(&item_id, |&(k, _, _, _)| k)
        .ok()
        .map(|i| {
            let (id, level, jobs_mask, slot_mask) = EQUIP_INFO[i];
            EquipInfo {
                item_id: id,
                level,
                jobs_mask,
                slot_mask,
            }
        })
}

/// True when the item fits in the given `SLOTTYPE` slot id (0..16).
pub fn fits_slot(info: &EquipInfo, slot_id: u8) -> bool {
    if slot_id >= 16 {
        return false;
    }
    info.slot_mask & (1 << slot_id) != 0
}

/// True when the given job id (1=WAR .. 22=GEO per LSB's job_name.lua)
/// is permitted to equip this item.
pub fn fits_job(info: &EquipInfo, job_id: u8) -> bool {
    if job_id >= 32 {
        return false;
    }
    info.jobs_mask & (1 << job_id) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bronze Dagger (item_id 16448) is `(level=1, jobs=1605625, slot=3)`
    /// per the raw SQL row — slot bits 0+1 = Main + Sub. Pins the
    /// column order so a SQL field shuffle doesn't silently mis-attribute
    /// slot vs jobs.
    #[test]
    fn bronze_dagger_resolves() {
        let info = lookup(16448).expect("bronze dagger");
        assert_eq!(info.level, 1);
        assert_eq!(info.slot_mask & 0b11, 0b11, "fits Main + Sub");
        assert!(fits_slot(&info, 0), "Main");
        assert!(fits_slot(&info, 1), "Sub");
        assert!(!fits_slot(&info, 4), "not Head");
    }

    /// Consumables / key items / currencies don't live in
    /// `item_equipment`. Pins the `None` path.
    #[test]
    fn unequippable_item_returns_none() {
        // 4112 is "fire_crystal" in item_basic — a currency, not equip.
        assert!(lookup(4112).is_none());
    }

    /// Sanity: bronze_cap (item_id 12448, raw row
    /// `(12448,'bronze_cap',1,0,2472947,15,0,0,16,0,0,0)`) carries
    /// `slot=16` = bit 4 set = Head slot only.
    #[test]
    fn bronze_cap_is_head_only() {
        let info = lookup(12448).expect("bronze cap");
        assert!(fits_slot(&info, 4), "fits Head");
        assert!(!fits_slot(&info, 0), "doesn't fit Main");
        assert!(!fits_slot(&info, 5), "doesn't fit Body");
    }

    #[test]
    fn table_size_is_reasonable() {
        // item_equipment ships ~15k rows; defend against a parser
        // regression that drops most of them.
        assert!(
            EQUIP_INFO.len() >= 5_000,
            "EQUIP_INFO.len() = {} (expected at least 5000)",
            EQUIP_INFO.len()
        );
    }
}
