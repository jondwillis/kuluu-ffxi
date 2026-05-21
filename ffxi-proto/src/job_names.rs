//! Job-id → display name lookup, sourced from
//! `vendor/server/scripts/enum/job_name.lua` at compile time. Used by
//! the battle-message substitution layer to resolve `<job>`
//! placeholders against the data fields of job-change messages.

include!(concat!(env!("OUT_DIR"), "/job_names_table.rs"));

pub fn lookup(id: u16) -> Option<&'static str> {
    JOB_NAMES
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()
        .map(|i| JOB_NAMES[i].1)
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
}
