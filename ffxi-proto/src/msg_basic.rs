//! Message-id → English text lookup, sourced from
//! `vendor/server/src/map/enums/msg_basic.h` at compile time.
//!
//! Half A of stage C7: the table only. The substitution layer that
//! resolves `<actor>`, `<target>`, `<amount>` against live entity state
//! lives in the client crate (half B).

include!(concat!(env!("OUT_DIR"), "/msg_basic_table.rs"));

/// Linear-scan lookup. The table has ~few hundred entries; for v1, O(n)
/// is fine — convert to a HashMap or perfect-hash if profiling shows
/// it's hot. (It won't be: battle messages arrive at human-perceivable
/// rates, not per-frame.)
pub fn lookup(id: u16) -> Option<&'static str> {
    MSG_BASIC.iter().find_map(|&(k, v)| (k == id).then_some(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_ids_resolve() {
        assert!(lookup(1).unwrap().contains("hits"));
        assert!(lookup(15).unwrap().contains("misses"));
        assert!(lookup(6).unwrap().contains("defeats") || lookup(6).unwrap().contains("Defeats"));
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(lookup(0xFFFF).is_none());
    }

    #[test]
    fn table_size_is_reasonable() {
        // Sanity: header has ~150-200 documented entries; if this drops to
        // near zero the parser regressed.
        assert!(
            MSG_BASIC.len() >= 100,
            "MSG_BASIC.len() = {} (expected at least 100)",
            MSG_BASIC.len()
        );
        eprintln!("MSG_BASIC.len() = {}", MSG_BASIC.len());
    }
}
