//! Zone-id -> MZB DAT file-id lookup.
//!
//! ## Source-of-truth
//!
//! The mapping is **derived at compile time** from two vendored
//! sources, scraped in `ffxi-dat/build.rs`:
//!
//!   * `vendor/ffxi-navmesh-builder/d_ms.cs` (GPL-3, LandSandBoat/FFXI-NavMesh-Builder) —
//!     contains the formula `fileId = if zone_id < 256 { zone_id + 100 } else { zone_id + 83635 }`.
//!   * `vendor/server/sql/zone_settings.sql` (GPL-3-or-later, LandSandBoat/server) —
//!     supplies the canonical set of valid `zone_id` values.
//!
//! See `ffxi-dat/build.rs` for the scraper and
//! `vendor/ffxi-navmesh-builder/NOTICE` for license attribution.
//!
//! ## Discrepancy with the previous probe-agent stub
//!
//! An earlier hand-curated stub asserted `zone_id 108 -> file_id 115`
//! because file 115 holds an MZB chunk literally named `f_ko` (the
//! Konschtat short-code). That was the *outdoor field* MZB. The
//! FFXI-NavMesh-Builder formula maps `zone_id 108 -> file_id 208`
//! whose MZB chunk is named `cons` (~6078 meshes, full zone collision
//! including structures) versus file 115's `f_ko` (~4569 meshes,
//! field only). **Both files parse cleanly as MZB**, but the
//! navmesh-builder formula is the FFXI-client-native convention LSB
//! uses for collision extraction, so that's what we return.

include!(concat!(env!("OUT_DIR"), "/zone_dat_table.rs"));

/// Look up the FFXI DAT file-id that holds zone `zone_id`'s primary
/// MZB chunk per the FFXI-NavMesh-Builder convention. Returns `None`
/// when the zone-id is not in LSB's `zone_settings.sql` (e.g. id 0,
/// or any id above the highest known zone).
///
/// Backed by a binary search over `ZONE_DAT_TABLE`.
pub fn zone_id_to_mzb_file_id(zone_id: u16) -> Option<u32> {
    ZONE_DAT_TABLE
        .binary_search_by_key(&zone_id, |(z, _)| *z)
        .ok()
        .map(|i| ZONE_DAT_TABLE[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Konschtat_Highlands per `zone_settings.sql` is zone-id 108;
    /// the FFXI-NavMesh-Builder formula (zone_id + 100 for zone_id<256)
    /// maps that to file-id 208. The chunk inside is named `cons`
    /// (full-zone collision), not `f_ko` (field-only MZB at file
    /// 115). See module docs for the discrepancy note.
    #[test]
    fn konschtat_highlands_maps_to_file_208() {
        assert_eq!(zone_id_to_mzb_file_id(108), Some(208));
    }

    /// West_Ronfaure (zone-id 100) -> file-id 200, chunk `f_ro`.
    #[test]
    fn west_ronfaure_maps_to_file_200() {
        assert_eq!(zone_id_to_mzb_file_id(100), Some(200));
    }

    /// East_Sarutabaruta (zone-id 116) -> file-id 216, chunk `f_sa`.
    #[test]
    fn east_sarutabaruta_maps_to_file_216() {
        assert_eq!(zone_id_to_mzb_file_id(116), Some(216));
    }

    /// Bastok_Markets (zone-id 235) -> file-id 335, chunk `t_ba`.
    #[test]
    fn bastok_markets_maps_to_file_335() {
        assert_eq!(zone_id_to_mzb_file_id(235), Some(335));
    }

    #[test]
    fn unknown_zone_returns_none() {
        // 9999 is outside any LSB zone-id range.
        assert_eq!(zone_id_to_mzb_file_id(9999), None);
    }

    /// Confirm the high-zone branch of the formula (zone_id + 83635
    /// when zone_id >= 256) is actually being hit somewhere in the
    /// generated table.
    #[test]
    fn high_zone_branch_is_reachable() {
        let any_high = ZONE_DAT_TABLE
            .iter()
            .any(|(z, f)| *z >= 256 && *f as u32 == *z as u32 + 83635);
        assert!(any_high, "no zone_id >= 256 found applying the high-branch formula");
    }

    /// Sanity: table is sorted (binary_search relies on this).
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

    /// Sanity: covers a meaningful chunk of zones (~300 in LSB).
    #[test]
    fn table_has_reasonable_coverage() {
        assert!(
            ZONE_DAT_TABLE.len() > 250,
            "expected >250 zones, got {}",
            ZONE_DAT_TABLE.len()
        );
    }
}
