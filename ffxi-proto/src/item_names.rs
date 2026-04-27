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
        assert_eq!(lookup(1), Some("Pile of Chocobo Bedding"));
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(lookup(0).is_none());
    }

    #[test]
    fn table_size_is_reasonable() {
        assert!(
            ITEM_NAMES.len() >= 10_000,
            "ITEM_NAMES.len() = {} (expected at least 10000)",
            ITEM_NAMES.len()
        );
    }
}
