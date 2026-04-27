include!(concat!(env!("OUT_DIR"), "/msg_area_table.rs"));

pub fn lookup(id: u16) -> Option<&'static str> {
    MSG_AREA.iter().find_map(|&(k, v)| (k == id).then_some(v))
}

pub fn count() -> usize {
    MSG_AREA.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_area_resolves() {
        assert!(lookup(0).unwrap().to_lowercase().contains("server"));
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(lookup(0xFFFE).is_none());
    }

    #[test]
    fn table_non_empty() {
        assert!(count() > 0, "msg_area table is empty");
    }
}
