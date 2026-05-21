//! Spell-id → English display name lookup, sourced from
//! `vendor/server/sql/spell_list.sql` at compile time. Used by the
//! battle-message substitution layer to resolve `<spell>` placeholders
//! against the cmd_arg / data1 carried in 0x028 and 0x029 packets.

include!(concat!(env!("OUT_DIR"), "/spell_names_table.rs"));

/// Binary-search lookup over the build-time-sorted table.
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
        // Cure is the canonical low-id sanity check; LSB ships spell_list
        // with `(1, 'cure', …)` as the first INSERT row.
        assert_eq!(lookup(1), Some("Cure"));
        // Fire is the canonical magic-damage check used in
        // session::tests::battle_message_2_magic_damage_*.
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
