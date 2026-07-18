include!(concat!(env!("OUT_DIR"), "/emote_table.rs"));

pub fn lookup(id: u8) -> Option<&'static str> {
    EMOTES
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()
        .map(|i| EMOTES[i].1)
}

/// Emote id for a slash-command word (`wave` → 8): the command is the
/// LSB enum name lowercased, e.g. `Dance1` → `/dance1`.
pub fn id_for_command(cmd: &str) -> Option<u8> {
    EMOTES
        .iter()
        .find(|(_, name)| name.eq_ignore_ascii_case(cmd))
        .map(|&(id, _)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrape_covers_known_ids_including_aim() {
        // Aim=96 exists only in the C++ enum, not scripts/enum/emote.lua — its
        // presence pins that the scrape reads emote.h.
        assert_eq!(lookup(8), Some("Wave"));
        assert_eq!(lookup(96), Some("Aim"));
        assert_eq!(lookup(0), Some("Point"));
        assert!(lookup(39).is_none());
    }

    #[test]
    fn ids_are_strictly_sorted_for_binary_search() {
        assert!(EMOTES.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn command_words_resolve_case_insensitively() {
        assert_eq!(id_for_command("wave"), Some(8));
        assert_eq!(id_for_command("dance1"), Some(65));
        assert_eq!(id_for_command("nosuchemote"), None);
    }
}
