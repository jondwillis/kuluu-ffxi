include!(concat!(env!("OUT_DIR"), "/msg_system_table.rs"));

pub fn lookup(id: u16) -> Option<&'static str> {
    MSG_SYSTEM.iter().find_map(|&(k, v)| (k == id).then_some(v))
}

pub fn count() -> usize {
    MSG_SYSTEM.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executing_logout_resolves() {
        let s = lookup(7).expect("id 7 should resolve");
        assert!(s.contains("Executing logout"), "{s}");
        assert!(s.contains("<seconds>"), "{s}");
    }

    #[test]
    fn executing_shutdown_resolves() {
        let s = lookup(35).expect("id 35 should resolve");
        assert!(s.contains("Executing shutdown"), "{s}");
        assert!(s.contains("<seconds>"), "{s}");
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(lookup(0xFFFE).is_none());
    }

    #[test]
    fn table_non_empty() {
        assert!(count() > 0, "msg_system table is empty");
    }
}
