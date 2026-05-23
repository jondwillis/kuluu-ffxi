//! Ability-id → English display name lookup, sourced from
//! `vendor/server/sql/abilities.sql` at compile time. Used by the
//! battle-message substitution layer to resolve `<ability>` placeholders.

include!(concat!(env!("OUT_DIR"), "/ability_names_table.rs"));

pub fn lookup(id: u16) -> Option<&'static str> {
    ABILITY_NAMES
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()
        .map(|i| ABILITY_NAMES[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_abilities_resolve() {
        // `(16, 'mighty_strikes', …)` is the first row of LSB's abilities.sql.
        assert_eq!(lookup(16), Some("Mighty Strikes"));
        // `(22, 'invincible', …)`
        assert_eq!(lookup(22), Some("Invincible"));
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(lookup(0xFFFF).is_none());
    }

    #[test]
    fn table_size_is_reasonable() {
        assert!(
            ABILITY_NAMES.len() >= 400,
            "ABILITY_NAMES.len() = {} (expected at least 400)",
            ABILITY_NAMES.len()
        );
    }
}
