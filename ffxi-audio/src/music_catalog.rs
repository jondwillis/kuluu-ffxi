include!(concat!(env!("OUT_DIR"), "/music_catalog_table.rs"));

pub type MusicCatalogEntry = (u16, &'static str, &'static str);

pub fn lookup_name(track_id: u16) -> Option<&'static str> {
    MUSIC_CATALOG
        .iter()
        .find_map(|(id, name, _)| (*id == track_id).then_some(*name))
}

pub fn lookup_composer(track_id: u16) -> Option<&'static str> {
    MUSIC_CATALOG
        .iter()
        .find_map(|(id, _, author)| (*id == track_id).then_some(*author))
}

pub fn lookup(track_id: u16) -> Option<MusicCatalogEntry> {
    MUSIC_CATALOG
        .iter()
        .copied()
        .find(|(id, _, _)| *id == track_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_battle_themes_have_names() {
        assert_eq!(lookup_name(101), Some("Battle Theme"));
        assert_eq!(lookup_name(103), Some("Battle Theme #2 (Party)"));
    }

    #[test]
    fn lookup_returns_composer_credit() {
        let composer = lookup_composer(101);
        assert!(composer.is_some());
        assert!(
            !composer.unwrap().is_empty(),
            "composer should be non-empty for a well-known track"
        );
    }

    #[test]
    fn unknown_track_returns_none() {
        assert_eq!(lookup_name(0xFFFF), None);
        assert_eq!(lookup_composer(0xFFFF), None);
        assert_eq!(lookup(0xFFFF), None);
    }

    #[test]
    fn catalog_is_sorted_by_track_id() {
        for window in MUSIC_CATALOG.windows(2) {
            assert!(
                window[0].0 < window[1].0,
                "catalog out of order at id={}",
                window[0].0
            );
        }
    }

    #[test]
    fn catalog_has_reasonable_entry_count() {
        assert!(
            MUSIC_CATALOG.len() >= 100,
            "catalog has only {} entries — scraper likely broke",
            MUSIC_CATALOG.len()
        );
    }
}
