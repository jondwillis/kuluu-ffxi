#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoneLine {
    pub line_id: u32,
    pub from_zone: u16,

    pub from_pos: [f32; 3],
    pub to_zone: u16,

    pub to_pos: [f32; 3],
    pub scale_x: f32,
    pub scale_z: f32,

    pub rotation: f32,
}

include!(concat!(env!("OUT_DIR"), "/zonelines_table.rs"));

pub fn zone_lines_for(zone_id: u16) -> &'static [ZoneLine] {
    match ZONE_LINE_INDEX.binary_search_by_key(&zone_id, |&(z, _, _)| z) {
        Ok(idx) => {
            let (_, start, end) = ZONE_LINE_INDEX[idx];
            &ZONE_LINES[start as usize..end as usize]
        }
        Err(_) => &[],
    }
}

pub fn total_count() -> usize {
    ZONE_LINES.len()
}

pub fn to_pos_for_line(line_id: u32) -> Option<[f32; 3]> {
    ZONE_LINES
        .iter()
        .find(|z| z.line_id == line_id)
        .map(|z| z.to_pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_was_scraped() {
        assert!(
            total_count() >= 800,
            "expected ≥800 zone-lines, got {}",
            total_count()
        );
    }

    #[test]
    fn carpenters_landing_has_known_lines() {
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
        assert!(zone_lines_for(0xFFFF).is_empty());
    }

    #[test]
    fn to_pos_for_line_resolves_known_line() {
        let to_pos = to_pos_for_line(813314682).expect("line 813314682 must exist");
        assert!((to_pos[0] - -16.039).abs() < 0.01, "x = {}", to_pos[0]);
        assert!((to_pos[1] - -132.804).abs() < 0.01, "y = {}", to_pos[1]);
        assert!((to_pos[2] - -4.217).abs() < 0.01, "z = {}", to_pos[2]);
    }

    #[test]
    fn to_pos_for_line_unknown_is_none() {
        assert!(to_pos_for_line(0xDEAD_BEEF).is_none());
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
