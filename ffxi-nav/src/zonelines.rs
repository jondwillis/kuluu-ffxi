//! Zone-line table — entry/exit points between zones.
//!
//! Sourced from `vendor/server/sql/zonelines.sql` at compile time by
//! [`build.rs`](../../build.rs). One row per zone-transition trigger:
//! the position the player needs to step through *in* `from_zone`, the
//! `to_zone` they land in, and the spawn position there.
//!
//! Coordinate system is FFXI native (x/y horizontal, z vertical). The
//! viewer crate handles axis remap when rendering markers.
//!
//! Lookup is a binary search over the (sorted-by-from_zone) index
//! followed by a slice — sub-microsecond, no allocation.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoneLine {
    /// LSB's `zonelineid` — opaque server identifier; useful for
    /// debugging and cross-referencing the SQL table.
    pub line_id: u32,
    pub from_zone: u16,
    /// FFXI-native position in `from_zone`: (x, y, z).
    pub from_pos: [f32; 3],
    pub to_zone: u16,
    /// FFXI-native landing position in `to_zone`: (x, y, z).
    pub to_pos: [f32; 3],
    pub scale_x: f32,
    pub scale_z: f32,
    /// Radians.
    pub rotation: f32,
}

include!(concat!(env!("OUT_DIR"), "/zonelines_table.rs"));

/// All zone-lines that originate in `zone_id` (where the player walks
/// *into* a transition trigger). Returns an empty slice for zones
/// without zonelines (most "instance" zones, GM zones, etc.).
pub fn zone_lines_for(zone_id: u16) -> &'static [ZoneLine] {
    // Binary-search the index; ZONE_LINE_INDEX is sorted by from_zone.
    match ZONE_LINE_INDEX.binary_search_by_key(&zone_id, |&(z, _, _)| z) {
        Ok(idx) => {
            let (_, start, end) = ZONE_LINE_INDEX[idx];
            &ZONE_LINES[start as usize..end as usize]
        }
        Err(_) => &[],
    }
}

/// Total number of zone-lines in the static table — cheap sanity check
/// for callers (and tests) that want to ensure scraping ran.
pub fn total_count() -> usize {
    ZONE_LINES.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_was_scraped() {
        // 875 INSERT statements in the source; allow some drift but
        // catch a regression where the parser dropped to zero.
        assert!(
            total_count() >= 800,
            "expected ≥800 zone-lines, got {}",
            total_count()
        );
    }

    #[test]
    fn carpenters_landing_has_known_lines() {
        // from_zone=2 = Carpenters Landing. SQL has at least three
        // outbound zonelines from Carpenters → Jugner / N. Sandoria.
        let lines = zone_lines_for(2);
        assert!(
            lines.len() >= 3,
            "Carpenters Landing should have ≥3 zone-lines, got {}",
            lines.len()
        );
        assert!(
            lines.iter().any(|z| z.to_zone == 104),
            "expected to_zone=104 (Jugner Forest)"
        );
        assert!(
            lines.iter().any(|z| z.to_zone == 231),
            "expected to_zone=231 (Northern San d'Oria)"
        );
    }

    #[test]
    fn unknown_zone_returns_empty() {
        // Zone 0xFFFF should not exist.
        assert!(zone_lines_for(0xFFFF).is_empty());
    }

    #[test]
    fn index_is_sorted() {
        let mut prev = 0u16;
        for &(z, _, _) in ZONE_LINE_INDEX {
            assert!(z >= prev, "ZONE_LINE_INDEX not sorted: {prev} → {z}");
            prev = z;
        }
    }

    #[test]
    fn slice_bounds_are_consistent() {
        for &(_, start, end) in ZONE_LINE_INDEX {
            assert!(start < end);
            assert!(end as usize <= ZONE_LINES.len());
        }
    }
}
