include!(concat!(env!("OUT_DIR"), "/zone_dat_table.rs"));
include!(concat!(env!("OUT_DIR"), "/string_dat_table.rs"));

pub fn zone_id_to_mzb_file_id(zone_id: u16) -> Option<u32> {
    ZONE_DAT_TABLE
        .binary_search_by_key(&zone_id, |(z, _)| *z)
        .ok()
        .map(|i| ZONE_DAT_TABLE[i].1)
}

/// Mog House interior MODEL id (the 0x00A `MyroomMapNumber`, produced by
/// GetMogHouseModelID — vendor/server/src/map/packets/s2c/0x00a_login.cpp:35-72)
/// → MZB DAT file id. Model ids are NOT zone ids; never feed them to
/// [`zone_id_to_mzb_file_id`]. File ids verified against
/// research/xim/src/jsMain/kotlin/xim/poc/tools/ZoneChanger.kt:18-36; an explicit
/// table because the model interval between the classic and WoTG blocks is
/// unobserved and XIM hard-codes the high entries. The Feretory alias model
/// (ffxi-proto `MYROOM_FERETORY`) is excluded: no verified file id.
const MOGHOUSE_MODEL_DAT_TABLE: [(u16, u32); 16] = [
    (199, 299),   // Bastok [S]
    (214, 314),   // Al Zahbi / Whitegate
    (219, 319),   // Windurst [S]
    (256, 356),   // Jeuno (also LSB default)
    (257, 357),   // San d'Oria rental
    (258, 358),   // Bastok rental
    (288, 388),   // Windurst rental
    (289, 389),   // San d'Oria home nation
    (290, 390),   // Bastok home nation
    (291, 391),   // Windurst home nation
    (292, 392),   // Adoulin
    (615, 83806), // 2F San d'Oria style
    (616, 83807), // 2F Bastok style
    (617, 83808), // 2F Windurst style
    (618, 83809), // 2F Patio style
    (745, 83936), // San d'Oria [S]
];

pub fn moghouse_model_to_mzb_file_id(model: u16) -> Option<u32> {
    MOGHOUSE_MODEL_DAT_TABLE
        .binary_search_by_key(&model, |(m, _)| *m)
        .ok()
        .map(|i| MOGHOUSE_MODEL_DAT_TABLE[i].1)
}

/// The single render key: which MZB DAT file the client should draw. The Mog House
/// interior model (when the player is in one) overrides the surrounding city's zone
/// DAT — inside the MH the server keeps `zone_id` equal to the city.
pub fn effective_zone_dat_file_id(zone_id: Option<u16>, myroom_model: Option<u16>) -> Option<u32> {
    myroom_model
        .and_then(moghouse_model_to_mzb_file_id)
        .or_else(|| zone_id.and_then(zone_id_to_mzb_file_id))
}

/// VTABLE/FTABLE file id of a zone's English dialog string DAT
/// ([`crate::dmsg::StringDat`]), or `None` if the zone has no entry.
pub fn zone_id_to_string_file_id(zone_id: u16) -> Option<u32> {
    STRING_DAT_TABLE
        .binary_search_by_key(&zone_id, |(z, _)| *z)
        .ok()
        .map(|i| STRING_DAT_TABLE[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn konschtat_highlands_maps_to_file_208() {
        assert_eq!(zone_id_to_mzb_file_id(108), Some(208));
    }

    #[test]
    fn west_ronfaure_maps_to_file_200() {
        assert_eq!(zone_id_to_mzb_file_id(100), Some(200));
    }

    #[test]
    fn east_sarutabaruta_maps_to_file_216() {
        assert_eq!(zone_id_to_mzb_file_id(116), Some(216));
    }

    #[test]
    fn bastok_markets_maps_to_file_335() {
        assert_eq!(zone_id_to_mzb_file_id(235), Some(335));
    }

    #[test]
    fn unknown_zone_returns_none() {
        assert_eq!(zone_id_to_mzb_file_id(9999), None);
    }

    #[test]
    fn high_zone_branch_is_reachable() {
        let any_high = ZONE_DAT_TABLE
            .iter()
            .any(|(z, f)| *z >= 256 && *f == *z as u32 + 83635);
        assert!(
            any_high,
            "no zone_id >= 256 found applying the high-branch formula"
        );
    }

    #[test]
    fn table_is_sorted_by_zone_id() {
        for window in ZONE_DAT_TABLE.windows(2) {
            assert!(
                window[0].0 < window[1].0,
                "ZONE_DAT_TABLE not strictly sorted: {} >= {}",
                window[0].0,
                window[1].0
            );
        }
    }

    #[test]
    fn table_has_reasonable_coverage() {
        assert!(
            ZONE_DAT_TABLE.len() > 250,
            "expected >250 zones, got {}",
            ZONE_DAT_TABLE.len()
        );
    }

    #[test]
    fn moghouse_table_spot_pins_and_count() {
        // One pin per source branch of research/xim ZoneChanger.kt:18-36 ×
        // vendor/server 0x00a_login.cpp:35-72 (classic low, WoTG high, 2F range),
        // plus the count — the exhaustive pair list lives only in the table.
        assert_eq!(MOGHOUSE_MODEL_DAT_TABLE.len(), 16);
        assert_eq!(moghouse_model_to_mzb_file_id(256), Some(356), "Jeuno");
        assert_eq!(
            moghouse_model_to_mzb_file_id(745),
            Some(83936),
            "San d'Oria [S]"
        );
        assert_eq!(moghouse_model_to_mzb_file_id(615), Some(83806), "2F");
    }

    #[test]
    fn moghouse_table_rejects_sentinel_and_unknown_models() {
        assert_eq!(moghouse_model_to_mzb_file_id(0x01FF), None, "MYROOM_NONE");
        assert_eq!(
            moghouse_model_to_mzb_file_id(729),
            None,
            "Feretory unverified"
        );
        assert_eq!(moghouse_model_to_mzb_file_id(0), None);
    }

    #[test]
    fn moghouse_model_is_not_a_zone_id() {
        // Model 256 (Jeuno MH) must resolve via the MH table, not collide with
        // zone 256 (Western Adoulin).
        assert_ne!(
            moghouse_model_to_mzb_file_id(256),
            zone_id_to_mzb_file_id(256)
        );
    }

    #[test]
    fn moghouse_table_is_sorted_for_binary_search() {
        for w in MOGHOUSE_MODEL_DAT_TABLE.windows(2) {
            assert!(
                w[0].0 < w[1].0,
                "not strictly sorted: {} >= {}",
                w[0].0,
                w[1].0
            );
        }
    }

    #[test]
    fn effective_file_id_prefers_myroom_model() {
        assert_eq!(effective_zone_dat_file_id(Some(243), Some(256)), Some(356));
        assert_eq!(effective_zone_dat_file_id(Some(230), None), Some(330));
        assert_eq!(effective_zone_dat_file_id(None, Some(617)), Some(83808));
        assert_eq!(effective_zone_dat_file_id(None, None), None);
    }

    #[test]
    fn unmapped_myroom_model_falls_back_to_zone_dat() {
        // LSB sends LoginState MYROOM plus the Feretory alias model (ffxi-proto
        // `MYROOM_FERETORY`) for ZONE_FERETORY, a zone with no real Mog House
        // (vendor/server/src/map/packets/s2c/0x00a_login.cpp:234-239); the model
        // is deliberately unmapped, so the zone fallback is what keeps a MYROOM
        // login renderable.
        let feretory = effective_zone_dat_file_id(Some(285), Some(729));
        assert_eq!(feretory, zone_id_to_mzb_file_id(285));
        assert!(
            feretory.is_some(),
            "Feretory must fall back to the zone DAT"
        );
    }
}
