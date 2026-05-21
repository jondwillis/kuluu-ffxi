//! Item-id → English display name lookup, sourced from
//! `vendor/server/sql/item_basic.sql` at compile time. Used by the
//! battle-message substitution layer to resolve `<item>` placeholders.

include!(concat!(env!("OUT_DIR"), "/item_names_table.rs"));

pub fn lookup(id: u16) -> Option<&'static str> {
    ITEM_NAMES
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()
        .map(|i| ITEM_NAMES[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_items_resolve() {
        // `(1, 0, 'pile_of_chocobo_bedding', …)` is the canonical first
        // row in item_basic.sql.
        assert_eq!(lookup(1), Some("Pile of Chocobo Bedding"));
    }

    #[test]
    fn unknown_id_returns_none() {
        // 0 is a sentinel "no item" used by some packets.
        assert!(lookup(0).is_none());
    }

    #[test]
    fn table_size_is_reasonable() {
        // item_basic.sql ships ~20k rows; if this drops sharply the SQL
        // parser regressed or upstream pruned data.
        assert!(
            ITEM_NAMES.len() >= 10_000,
            "ITEM_NAMES.len() = {} (expected at least 10000)",
            ITEM_NAMES.len()
        );
    }
}
