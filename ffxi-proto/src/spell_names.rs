include!(concat!(env!("OUT_DIR"), "/spell_names_table.rs"));

pub fn lookup(id: u16) -> Option<&'static str> {
    SPELL_NAMES
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()
        .map(|i| SPELL_NAMES[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_spells_resolve() {
        assert_eq!(lookup(1), Some("Cure"));

        assert_eq!(lookup(144), Some("Fire"));
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(lookup(0xFFFF).is_none());
    }

    #[test]
    fn table_size_is_reasonable() {
        assert!(
            SPELL_NAMES.len() >= 500,
            "SPELL_NAMES.len() = {} (expected at least 500)",
            SPELL_NAMES.len()
        );
    }
}
