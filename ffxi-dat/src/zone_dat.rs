include!(concat!(env!("OUT_DIR"), "/zone_dat_table.rs"));
include!(concat!(env!("OUT_DIR"), "/string_dat_table.rs"));

pub fn zone_id_to_mzb_file_id(zone_id: u16) -> Option<u32> {
    ZONE_DAT_TABLE
        .binary_search_by_key(&zone_id, |(z, _)| *z)
        .ok()
        .map(|i| ZONE_DAT_TABLE[i].1)
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
}
