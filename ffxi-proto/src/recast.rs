include!(concat!(env!("OUT_DIR"), "/ability_recast_id_table.rs"));

// vendor/server/sql/abilities.sql field 6 `recastId` — the recast group id the
// server keys 0x119 ABIL_RECAST timers by (recasttimer_t::TimerId). 2hr/SP
// abilities share id 0 (the special timer slot).
pub fn ability_recast_id(ability_id: u16) -> Option<u16> {
    ABILITY_RECAST_ID
        .binary_search_by_key(&ability_id, |&(k, _)| k)
        .ok()
        .map(|i| ABILITY_RECAST_ID[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_recast_ids() {
        assert_eq!(ability_recast_id(35), Some(5)); // Provoke
        assert_eq!(ability_recast_id(16), Some(0)); // Mighty Strikes (2hr, slot 0)
    }

    #[test]
    fn unknown_ability_is_none() {
        assert!(ability_recast_id(0xFFFF).is_none());
    }
}
