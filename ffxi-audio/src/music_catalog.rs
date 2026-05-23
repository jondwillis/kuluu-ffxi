//! BGM track id → human name + composer lookup, sourced from
//! `vendor/AltanaListener/AltanaListener/track_names.json` at build
//! time. See `build.rs` in this crate for the scrape.
//!
//! Used by the BGM diagnostic HUD to label "now playing" instead of
//! showing just the numeric track id. The underlying playback path
//! doesn't depend on this — it streams the `.bgw` directly off disk
//! regardless of whether we have a label for it.

include!(concat!(env!("OUT_DIR"), "/music_catalog_table.rs"));

/// One catalog entry: `(track_id, track_name, composer)`.
pub type MusicCatalogEntry = (u16, &'static str, &'static str);

/// Linear-scan lookup. The catalog has ~300 entries; for the BGM
/// diagnostic that updates at most a few times per minute, O(n) per
/// query is fine. Convert to a perfect-hash if a hot path ever needs
/// it.
pub fn lookup_name(track_id: u16) -> Option<&'static str> {
    MUSIC_CATALOG
        .iter()
        .find_map(|(id, name, _)| (*id == track_id).then_some(*name))
}

/// Same lookup but returns the composer credit. Useful for "now
/// playing — *<title>* by <composer>" tooltips.
pub fn lookup_composer(track_id: u16) -> Option<&'static str> {
    MUSIC_CATALOG
        .iter()
        .find_map(|(id, _, author)| (*id == track_id).then_some(*author))
}

/// Resolve both fields in one pass. Convenience for callers that want
/// to render `"<title> — <composer>"` without doing two linear scans.
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
        // Spot check against the values we can see in
        // track_names.json's first dozen entries. Regression guards
        // against the scraper accidentally producing an empty table.
        assert_eq!(lookup_name(101), Some("Battle Theme"));
        assert_eq!(lookup_name(103), Some("Battle Theme #2 (Party)"));
    }

    #[test]
    fn lookup_returns_composer_credit() {
        // Most FFXI tracks credit Naoshi Mizuta or Kumi Tanioka.
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
        // The scrape sorts on the way out so binary-search becomes
        // viable later without re-running the scrape. Verify the
        // invariant so a future build.rs refactor doesn't quietly
        // break it.
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
        // AltanaListener's track_names.json had ~223 unique BGW ids
        // at the time of scaffolding. If a future scrape produces
        // zero or two it's almost certainly a regression in build.rs
        // — guard against silent loss.
        assert!(
            MUSIC_CATALOG.len() >= 100,
            "catalog has only {} entries — scraper likely broke",
            MUSIC_CATALOG.len()
        );
    }
}
