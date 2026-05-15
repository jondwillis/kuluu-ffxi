//! `xi.msg.channel` — chat channel kinds, scraped from
//! `vendor/server/scripts/enum/msg.lua` at compile time.

include!(concat!(env!("OUT_DIR"), "/msg_channel_table.rs"));

pub fn lookup(id: u16) -> Option<&'static str> {
    MSG_CHANNEL
        .iter()
        .find_map(|&(k, v)| (k == id).then_some(v))
}

pub fn count() -> usize {
    MSG_CHANNEL.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_channel_resolves() {
        // SYSTEM_2 = 7 -- Login / world announcement messages
        assert!(lookup(7).unwrap().to_lowercase().contains("login"));
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(lookup(0xFFFE).is_none());
    }

    #[test]
    fn table_non_empty() {
        assert!(count() > 0, "msg_channel table is empty");
    }
}
