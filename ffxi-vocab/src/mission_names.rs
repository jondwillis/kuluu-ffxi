//! Mission-chain ids and the scraped name table (kuluu-m4yk arc: the Missions
//! battery's `missionm`/`miss00` windows need per-mission names, and every one
//! of them is data LSB already ships in `scripts/globals/missions.lua`).

include!(concat!(env!("OUT_DIR"), "/mission_names_table.rs"));

/// Chain indices = `xi.mission.log_id` (`vendor/server/scripts/globals/missions.lua`)
/// ≡ `MissionLog` (`vendor/server/src/map/enums/mission_log.h`). TVR is listed for
/// completeness; its id LSB marks guessed, so no scrape rows carry it.
pub const CHAIN_SANDORIA: u8 = 0;
pub const CHAIN_BASTOK: u8 = 1;
pub const CHAIN_WINDURST: u8 = 2;
pub const CHAIN_ZILART: u8 = 3;
pub const CHAIN_TOAU: u8 = 4;
pub const CHAIN_WOTG: u8 = 5;
pub const CHAIN_COP: u8 = 6;
pub const CHAIN_ASSAULT: u8 = 7;
pub const CHAIN_CAMPAIGN: u8 = 8;
pub const CHAIN_ACP: u8 = 9;
pub const CHAIN_AMK: u8 = 10;
pub const CHAIN_ASA: u8 = 11;
pub const CHAIN_SOA: u8 = 12;
pub const CHAIN_ROV: u8 = 13;

/// The scraped name for one `(chain, id)` pair — `id` is the wire mission id
/// LSB stores in `m_missionLog[log].current` and sets as the completion bits on
/// ports 0x00D0/0x00D8 (`vendor/server/src/map/packets/s2c/0x056_mission_other.cpp`) —
/// or `None` where LSB's table carries no name (sentinels, unassigned ids).
pub fn mission_name(chain: u8, id: u16) -> Option<&'static str> {
    let key = |&(c, i, _): &(u8, u16, &str)| ((c as u32) << 16) | (i as u32);
    MISSION_NAMES
        .binary_search_by_key(&((chain as u32) << 16 | id as u32), key)
        .ok()
        .map(|i| MISSION_NAMES[i].2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_for_binary_search() {
        assert!(MISSION_NAMES
            .windows(2)
            .all(|w| (w[0].0, w[0].1) < (w[1].0, w[1].1)));
    }

    #[test]
    fn nation_missions_resolve_from_the_log_port_ids() {
        // xi.mission.id.bastok / .sandoria (vendor/server/scripts/globals/missions.lua);
        // the Bastok list matches the retail Mission Log capture line for line.
        assert_eq!(mission_name(CHAIN_BASTOK, 0), Some("The Zeruhn Report"));
        assert_eq!(mission_name(CHAIN_BASTOK, 1), Some("Geological Survey"));
        assert_eq!(mission_name(CHAIN_BASTOK, 5), Some("The Emissary"));
        assert_eq!(mission_name(CHAIN_BASTOK, 10), Some("The Four Musketeers"));
        assert_eq!(mission_name(CHAIN_SANDORIA, 2), Some("Save the Children"));
        assert_eq!(mission_name(CHAIN_WINDURST, 4), Some("A Testing Time"));
    }

    #[test]
    fn era_chains_resolve() {
        assert_eq!(mission_name(CHAIN_ZILART, 0), Some("The New Frontier"));
        assert_eq!(mission_name(CHAIN_TOAU, 0), Some("Land of Sacred Serpents"));
        assert_eq!(mission_name(CHAIN_WOTG, 0), Some("Cavernous Maws"));
        assert_eq!(mission_name(CHAIN_COP, 110), Some("The Rites of Life"));
        assert_eq!(mission_name(CHAIN_ACP, 0), Some("A Crystalline Prophecy"));
        // LSB's own identifier `A_MOOGLE_KUPO_DETAT` (the vendor spelling).
        assert_eq!(mission_name(CHAIN_AMK, 0), Some("A Moogle Kupo Detat"));
        assert_eq!(mission_name(CHAIN_ASA, 9), Some("Battaru Royale"));
        assert_eq!(mission_name(CHAIN_SOA, 1), Some("The Geomagnetron"));
        assert_eq!(mission_name(CHAIN_ROV, 2), Some("Resonance"));
    }

    #[test]
    fn sentinels_and_gaps_return_none() {
        // `NONE = 65535` rows stay out of the table; SoA ids LSB leaves
        // commented (categories) have no name.
        assert_eq!(mission_name(CHAIN_BASTOK, 65535), None);
        assert_eq!(mission_name(CHAIN_SANDORIA, 24), None);
        assert_eq!(mission_name(CHAIN_CAMPAIGN, 0), None);
        assert_eq!(mission_name(7, 0), None);
    }

    #[test]
    fn table_covers_every_scrapped_chain() {
        for chain in [
            CHAIN_SANDORIA,
            CHAIN_BASTOK,
            CHAIN_WINDURST,
            CHAIN_ZILART,
            CHAIN_TOAU,
        ] {
            assert!(
                MISSION_NAMES.iter().any(|row| row.0 == chain),
                "chain {chain} lost its rows"
            );
        }
    }

    #[test]
    fn table_size_is_reasonable() {
        // 14 chains, ~530 named entries in the pinned vendor tree (the five
        // `NONE = 65535` sentinels and commented categories drop out).
        assert!(
            MISSION_NAMES.len() >= 260,
            "MISSION_NAMES.len() = {} (expected at least 260)",
            MISSION_NAMES.len()
        );
    }
}
