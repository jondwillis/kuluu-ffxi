include!(concat!(env!("OUT_DIR"), "/key_item_names_table.rs"));

pub fn lookup(id: u16) -> Option<&'static str> {
    KEY_ITEM_NAMES
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()
        .map(|i| KEY_ITEM_NAMES[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_key_items_resolve() {
        assert_eq!(lookup(1), Some("Zeruhn Report"));
        assert_eq!(lookup(8), Some("Airship Pass"));
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(lookup(u16::MAX).is_none());
    }

    #[test]
    fn table_size_is_reasonable() {
        assert!(
            KEY_ITEM_NAMES.len() >= 3000,
            "KEY_ITEM_NAMES.len() = {} (expected at least 3000)",
            KEY_ITEM_NAMES.len()
        );
    }

    #[test]
    fn ids_are_strictly_sorted_for_binary_search() {
        assert!(KEY_ITEM_NAMES.windows(2).all(|w| w[0].0 < w[1].0));
    }
}
