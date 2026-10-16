include!(concat!(env!("OUT_DIR"), "/quest_lattice_table.rs"));

/// Resolve one quest's scraped label for a `(log, id)` pair from the s2c 0x056
/// quest-port bitmasks (vault main_menu.md §31); `None` when the vendor lattice
/// holds no name for it.
pub fn quest_name(log: u8, id: u16) -> Option<&'static str> {
    let key = |&(l, i, _): &(u8, u16, &str)| ((l as u32) << 16) | (i as u32);
    QUEST_LATTICE
        .binary_search_by_key(&((log as u32) << 16 | id as u32), key)
        .ok()
        .map(|i| QUEST_LATTICE[i].2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lattice_is_sorted_for_binary_search() {
        assert!(QUEST_LATTICE
            .windows(2)
            .all(|w| (w[0].0, w[0].1) < (w[1].0, w[1].1)));
    }

    #[test]
    fn well_known_quests_resolve() {
        // scripts/globals/quests.lua, San d'Oria block.
        assert_eq!(quest_name(0, 0), Some("A Sentrys Peril"));
        assert_eq!(quest_name(3, 0), Some("Crest of Davoi"));
        assert!(quest_name(8, 0).is_some(), "Abyssea block present");
    }

    #[test]
    fn unknown_pairs_return_none() {
        assert_eq!(quest_name(11, 0), None);
        assert_eq!(quest_name(0, u16::MAX), None);
    }

    #[test]
    fn areas_cover_the_quest_log_enum() {
        assert_eq!(QUEST_AREA_NAMES.len(), 11);
        assert_eq!(QUEST_AREA_NAMES[0], "Sandoria");
        assert_eq!(QUEST_AREA_NAMES[10], "Coalition");
    }

    #[test]
    fn table_size_is_reasonable() {
        assert!(
            QUEST_LATTICE.len() >= 1000,
            "QUEST_LATTICE.len() = {} (expected at least 1000)",
            QUEST_LATTICE.len()
        );
    }
}
