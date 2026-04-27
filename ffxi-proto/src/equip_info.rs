include!(concat!(env!("OUT_DIR"), "/equip_info_table.rs"));

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct EquipInfo {
    pub item_id: u16,

    pub level: u8,

    pub jobs_mask: u32,

    pub slot_mask: u16,
}

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

pub fn fits_slot(info: &EquipInfo, slot_id: u8) -> bool {
    if slot_id >= 16 {
        return false;
    }
    info.slot_mask & (1 << slot_id) != 0
}

pub fn fits_job(info: &EquipInfo, job_id: u8) -> bool {
    if job_id >= 32 {
        return false;
    }
    info.jobs_mask & (1 << job_id) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bronze_dagger_resolves() {
        let info = lookup(16448).expect("bronze dagger");
        assert_eq!(info.level, 1);
        assert_eq!(info.slot_mask & 0b11, 0b11, "fits Main + Sub");
        assert!(fits_slot(&info, 0), "Main");
        assert!(fits_slot(&info, 1), "Sub");
        assert!(!fits_slot(&info, 4), "not Head");
    }

    #[test]
    fn unequippable_item_returns_none() {
        assert!(lookup(4112).is_none());
    }

    #[test]
    fn bronze_cap_is_head_only() {
        let info = lookup(12448).expect("bronze cap");
        assert!(fits_slot(&info, 4), "fits Head");
        assert!(!fits_slot(&info, 0), "doesn't fit Main");
        assert!(!fits_slot(&info, 5), "doesn't fit Body");
    }

    #[test]
    fn table_size_is_reasonable() {
        assert!(
            EQUIP_INFO.len() >= 5_000,
            "EQUIP_INFO.len() = {} (expected at least 5000)",
            EQUIP_INFO.len()
        );
    }
}
