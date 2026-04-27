include!(concat!(env!("OUT_DIR"), "/msg_action_modifier_table.rs"));

pub fn lookup(id: u16) -> Option<&'static str> {
    MSG_ACTION_MODIFIER
        .iter()
        .find_map(|&(k, v)| (k == id).then_some(v))
}

pub fn count() -> usize {
    MSG_ACTION_MODIFIER.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_modifier_resolves() {
        assert!(lookup(2).unwrap().to_lowercase().contains("resist"));
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(lookup(0xFFFE).is_none());
    }

    #[test]
    fn table_non_empty() {
        assert!(count() > 0, "msg_action_modifier table is empty");
    }
}
