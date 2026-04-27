include!(concat!(env!("OUT_DIR"), "/status_names_table.rs"));

pub fn lookup(id: u16) -> Option<&'static str> {
    STATUS_NAMES
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()
        .map(|i| STATUS_NAMES[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_statuses_resolve() {
        assert_eq!(lookup(40), Some("Protect"));
        assert_eq!(lookup(41), Some("Shell"));

        assert_eq!(lookup(34), Some("Blaze Spikes"));
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(lookup(0xFFFF).is_none());
    }

    #[test]
    fn table_size_is_reasonable() {
        assert!(
            STATUS_NAMES.len() >= 200,
            "STATUS_NAMES.len() = {} (expected at least 200)",
            STATUS_NAMES.len()
        );
    }
}
