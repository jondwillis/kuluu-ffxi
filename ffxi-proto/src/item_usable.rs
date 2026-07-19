//! LSB `item_usable` table: which items can be used at all, and with how
//! many charges. Scraped from vendor/server/sql/item_usable.sql by build.rs.
//!
//! Grounding: vendor/server/src/map/packets/../items — the 0x037 item-use
//! path only fires for items present in this table (CItemUsable); equipment
//! with `maxCharges > 0` is charged equipment (usable while equipped).

include!(concat!(env!("OUT_DIR"), "/item_usable_table.rs"));

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct UsableInfo {
    pub item_id: u16,

    /// LSB validTargets bitmask (see `valid_target`): 1 = self, etc.
    pub valid_targets: u16,

    /// Charges for charged equipment; 0 for plain consumables.
    pub max_charges: u8,
}

pub fn lookup(item_id: u16) -> Option<UsableInfo> {
    ITEM_USABLE
        .binary_search_by_key(&item_id, |&(k, _, _)| k)
        .ok()
        .map(|i| {
            let (id, valid_targets, max_charges) = ITEM_USABLE[i];
            UsableInfo {
                item_id: id,
                valid_targets,
                max_charges,
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn potion_is_usable() {
        // sql/item_usable.sql: (4112,'potion',1,1,30,0,0,0,0,0)
        let info = lookup(4112).expect("potion in item_usable");
        assert_eq!(info.valid_targets, 1);
        assert_eq!(info.max_charges, 0);
    }

    #[test]
    fn kupofrieds_ring_is_charged() {
        // sql/item_usable.sql: (15840,'kupofrieds_ring',1,1,76,0,11,5,900,0)
        let info = lookup(15840).expect("kupofried's ring in item_usable");
        assert!(info.max_charges > 0);
    }

    #[test]
    fn plain_equipment_is_not_usable() {
        // silver_earring (13327) is equipment with no use activation.
        assert_eq!(lookup(13327), None);
    }

    #[test]
    fn table_is_sorted_for_binary_search() {
        assert!(ITEM_USABLE.windows(2).all(|w| w[0].0 < w[1].0));
    }
}
