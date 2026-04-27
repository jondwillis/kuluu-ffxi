include!(concat!(env!("OUT_DIR"), "/msg_basic_table.rs"));

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
        assert!(
            MSG_BASIC.len() >= 100,
            "MSG_BASIC.len() = {} (expected at least 100)",
            MSG_BASIC.len()
        );
        eprintln!("MSG_BASIC.len() = {}", MSG_BASIC.len());
    }
}
