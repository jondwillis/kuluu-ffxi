include!(concat!(env!("OUT_DIR"), "/title_names_table.rs"));

/// Display name of one title id (`profile.title`, which s2c 0x061 sends as `designation`).
/// Generated from LSB's `xi.title` enum, so the text is that file's identifier prettified rather
/// than retail's own caption string; punctuation the identifiers cannot hold (apostrophes) is
/// missing. It is nonetheless the same id space the server equips titles from, which makes it the
/// best-grounded reading available until a client title-text catalog is identified.
pub fn lookup(id: u16) -> Option<&'static str> {
    TITLE_NAMES
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()
        .map(|i| TITLE_NAMES[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_titles_resolve() {
        // Ids straight from vendor/server/scripts/enum/title.lua.
        assert_eq!(lookup(1), Some("Fodderchief Flayer"));
        assert_eq!(lookup(120), Some("Star Onion Brigade Member"));
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(lookup(u16::MAX).is_none());
    }

    #[test]
    fn table_size_is_reasonable() {
        assert!(
            TITLE_NAMES.len() >= 1000,
            "TITLE_NAMES.len() = {} (expected at least 1000)",
            TITLE_NAMES.len()
        );
    }

    #[test]
    fn ids_are_strictly_sorted_for_binary_search() {
        assert!(TITLE_NAMES.windows(2).all(|w| w[0].0 < w[1].0));
    }
}
