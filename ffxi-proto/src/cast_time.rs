include!(concat!(env!("OUT_DIR"), "/spell_cast_time_table.rs"));
include!(concat!(env!("OUT_DIR"), "/spell_recast_time_table.rs"));

// vendor/server/sql/spell_list.sql field 10 `castTime` — milliseconds the
// server enforces before a spell resolves (CSpell::getCastTime). Drives the
// client cast bar duration and the optimistic cast lock.
pub fn spell_cast_time_ms(spell_id: u16) -> Option<u16> {
    SPELL_CAST_TIME_MS
        .binary_search_by_key(&spell_id, |&(k, _)| k)
        .ok()
        .map(|i| SPELL_CAST_TIME_MS[i].1)
}

// vendor/server/sql/spell_list.sql field 11 `recastTime` — milliseconds before
// the spell can be recast (CSpell::getRecastTime). The map server delivers no
// pre-cast spell-recast list (0x119 ABIL_RECAST is ability-only), so this base
// value is the client's source for greying spell rows.
pub fn spell_recast_time_ms(spell_id: u16) -> Option<u32> {
    SPELL_RECAST_TIME_MS
        .binary_search_by_key(&spell_id, |&(k, _)| k)
        .ok()
        .map(|i| SPELL_RECAST_TIME_MS[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_cast_times() {
        assert_eq!(spell_cast_time_ms(1), Some(2000)); // Cure
        assert_eq!(spell_cast_time_ms(144), Some(500)); // Fire
        assert_eq!(spell_cast_time_ms(159), Some(500)); // Stone
    }

    #[test]
    fn known_recast_times() {
        assert_eq!(spell_recast_time_ms(1), Some(5000)); // Cure
        assert_eq!(spell_recast_time_ms(144), Some(2000)); // Fire
    }

    #[test]
    fn unknown_spell_is_none() {
        assert!(spell_cast_time_ms(0xFFFF).is_none());
        assert!(spell_recast_time_ms(0xFFFF).is_none());
    }
}
