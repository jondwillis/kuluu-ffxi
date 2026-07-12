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

// Mog House residence-entrance tag prefixes; LSB matches the same prefixes on the
// c2s 0x05E RectID (vendor/server/src/map/packets/c2s/0x05e_maprect.cpp:74-75:
// "zmr* classic cities; zms* WoTG [S] + Adoulin"). The zonelines.sql primary key IS
// the trigger's fourcc as a LE u32, so the prefix test works on `line_id` directly.
const MOG_HOUSE_TAG_PREFIXES: [&[u8; 3]; 2] = [b"zmr", b"zms"];

pub fn is_mog_house_entry(line: &ZoneLine) -> bool {
    let tag = line.line_id.to_le_bytes();
    MOG_HOUSE_TAG_PREFIXES.iter().any(|p| tag.starts_with(*p))
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
    fn mog_house_line_id_is_the_fourcc() {
        assert_eq!(812805498u32.to_le_bytes(), *b"zmr0");
        let sandoria_mh = ZONE_LINES
            .iter()
            .find(|z| z.line_id == 812805498)
            .expect("Southern San d'Oria zmr0 line must exist");
        assert!(is_mog_house_entry(sandoria_mh));
        assert_eq!(sandoria_mh.from_zone, 230);
    }

    #[test]
    fn mog_house_entries_stay_in_zone_and_decode_ascii() {
        let mh: Vec<_> = ZONE_LINES
            .iter()
            .filter(|z| is_mog_house_entry(z))
            .collect();
        assert!(
            mh.len() >= 20,
            "expected ≥20 MH residence lines, got {}",
            mh.len()
        );
        for line in mh {
            let tag = line.line_id.to_le_bytes();
            assert!(
                tag.iter().all(|b| b.is_ascii_graphic()),
                "MH tag not ASCII: {:?}",
                tag
            );
            // vendor/server/src/map/packets/c2s/0x05e_maprect.cpp:234-243 — MH
            // entrances keep the player in the same zone.
            assert_eq!(
                line.from_zone, line.to_zone,
                "MH line {} must stay in-zone",
                line.line_id
            );
        }
    }

    #[test]
    fn same_zone_alone_does_not_mean_mog_house() {
        // Zones 9/34/35/158 carry same-zone lines with z0*/zi*/z4* tags.
        let non_mh_same_zone = ZONE_LINES
            .iter()
            .find(|z| z.from_zone == z.to_zone && !is_mog_house_entry(z))
            .expect("non-MH same-zone lines exist in the scrape");
        let tag = non_mh_same_zone.line_id.to_le_bytes();
        assert_eq!(tag[0], b'z');
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
