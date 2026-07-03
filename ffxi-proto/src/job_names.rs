include!(concat!(env!("OUT_DIR"), "/job_names_table.rs"));
include!(concat!(env!("OUT_DIR"), "/job_abbrevs_table.rs"));

pub fn lookup(id: u16) -> Option<&'static str> {
    JOB_NAMES
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()
        .map(|i| JOB_NAMES[i].1)
}

/// The canonical FFXI three-letter job code (e.g. 2 → "MNK"), scraped from
/// LSB's job_name.lua. Not derivable by truncating the full name — "Monk" → "MON"
/// and "White Mage" → "WHI" are both wrong.
pub fn abbrev(id: u16) -> Option<&'static str> {
    JOB_ABBREVS
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()
        .map(|i| JOB_ABBREVS[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_jobs_resolve() {
        assert_eq!(lookup(1), Some("Warrior"));
        assert_eq!(lookup(7), Some("Paladin"));
        assert_eq!(lookup(22), Some("Rune Fencer"));
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(lookup(0xFFFF).is_none());
    }

    #[test]
    fn canonical_abbrevs_not_truncated_names() {
        // The whole point: these differ from a naive 3-char truncation.
        assert_eq!(abbrev(2), Some("MNK")); // not "MON"
        assert_eq!(abbrev(3), Some("WHM")); // not "WHI"
        assert_eq!(abbrev(1), Some("WAR"));
        assert_eq!(abbrev(22), Some("RUN"));
        assert!(abbrev(0xFFFF).is_none());
    }
}
