//! Auto-translate block parsing for FFXI chat strings.
//!
//! The wire form of an auto-translate phrase is six raw bytes embedded
//! inline in a chat message body:
//!
//! ```text
//! 0xFD <type> <lang> <category> <index> 0xFD
//! ```
//!
//! `0xFD` is never a valid UTF-8 leading byte, so the markers are
//! unambiguous on the wire. The four payload bytes form a little-endian
//! key into a lookup table sourced from
//! `vendor/server/src/map/autotranslate.cpp` (LSB), itself extracted from
//! the retail client's `ROM/168/25.DAT`. Special cases:
//!   - `type=0x07` is an item (queryable in LSB's `auto_translate_items`).
//!   - `type=0x13` is a key item.
//!   - `type=0x02` is the default phrase table; `lang` byte is ignorable.
//!
//! We render resolved blocks as `{Phrase}` and unresolved blocks as
//! `{AT:type/cat/idx}` so testers can still see *that* an AT phrase was
//! present even when the table doesn't cover it.
//!
//! The lookup table is lazy: `decode` walks bytes without touching the
//! table when no `0xFD` marker is present, and the table is parsed on
//! first hit via `OnceLock`.

use std::collections::HashMap;
use std::sync::OnceLock;

const MARKER: u8 = 0xFD;

/// Decode a raw chat-message byte slice, replacing auto-translate blocks
/// with human-readable text and lossily decoding the surrounding UTF-8.
///
/// Bytes outside AT blocks are converted with `String::from_utf8_lossy`
/// applied to each maximal non-AT run, so a stray malformed UTF-8 byte
/// becomes U+FFFD but does not consume an AT marker by accident.
pub fn decode(bytes: &[u8]) -> String {
    if !bytes.contains(&MARKER) {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == MARKER {
            // Need 5 more bytes: 4 payload + closing marker.
            if i + 5 < bytes.len() && bytes[i + 5] == MARKER {
                let ty = bytes[i + 1];
                let lang = bytes[i + 2];
                let cat = bytes[i + 3];
                let idx = bytes[i + 4];
                out.push('{');
                out.push_str(&resolve(ty, lang, cat, idx));
                out.push('}');
                i += 6;
                continue;
            }
            // Lone 0xFD with no closing marker — render as U+FFFD so the
            // caller can still see something was there.
            out.push('\u{FFFD}');
            i += 1;
            continue;
        }
        // Consume one maximal non-marker run and decode it lossily.
        let start = i;
        while i < bytes.len() && bytes[i] != MARKER {
            i += 1;
        }
        out.push_str(&String::from_utf8_lossy(&bytes[start..i]));
    }
    out
}

/// Resolve a single AT block to a display string. Returns the looked-up
/// phrase, or `AT:type/cat/idx` for keys missing from the table.
///
/// The `lang` byte is intentionally dropped from the lookup: LSB's
/// shipped table is keyed at `lang=2` for all 28k entries with zero
/// cross-lang collisions, but in the wild clients send AT phrases with
/// `lang=0` too (Japanese-locale clients chatting to English players,
/// for instance). Per LSB's own header comment on `replaceBytes()`,
/// "YY is a language code and can be safely ignored."
fn resolve(ty: u8, _lang: u8, cat: u8, idx: u8) -> String {
    let key = (ty as u32) | ((cat as u32) << 16) | ((idx as u32) << 24);
    if let Some(s) = table().get(&key) {
        return (*s).to_string();
    }
    format!("AT:{:02x}/{:02x}/{:02x}", ty, cat, idx)
}

/// Embedded copy of LSB's autotranslate table. ~28k entries, ~700KB —
/// small enough to ship in the binary but parsed lazily on first lookup.
const TABLE_TSV: &str = include_str!("../data/autotranslate.tsv");

fn table() -> &'static HashMap<u32, &'static str> {
    static TABLE: OnceLock<HashMap<u32, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut map = HashMap::with_capacity(28_500);
        for line in TABLE_TSV.lines() {
            let mut parts = line.splitn(2, '\t');
            let Some(key_str) = parts.next() else {
                continue;
            };
            let Some(text) = parts.next() else { continue };
            let Ok(key) = key_str.parse::<u32>() else {
                continue;
            };
            // Drop the lang byte (bits 8..16) so wire-side lang variants
            // all collide on the same slot. See `resolve` for why.
            let stripped = key & 0xFFFF_00FF;
            map.insert(stripped, text);
        }
        map
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_no_marker() {
        assert_eq!(decode(b"hello world"), "hello world");
    }

    #[test]
    fn decodes_known_phrase_greetings() {
        // key 66050 = 0x00010202 -> type=0x02, lang=0x02, cat=0x01, idx=0x00.
        let bytes = [b'h', b'i', b' ', 0xFD, 0x02, 0x02, 0x01, 0x00, 0xFD, b'!'];
        assert_eq!(decode(&bytes), "hi {Greetings}!");
    }

    #[test]
    fn renders_unknown_block_as_at_placeholder() {
        let bytes = [0xFD, 0x02, 0x00, 0xFE, 0xFE, 0xFD];
        assert_eq!(decode(&bytes), "{AT:02/fe/fe}");
    }

    #[test]
    fn handles_lone_marker_gracefully() {
        let bytes = [b'a', 0xFD, b'b'];
        assert_eq!(decode(&bytes), "a\u{FFFD}b");
    }

    #[test]
    fn decodes_back_to_back_blocks() {
        // Two adjacent valid blocks: Greetings followed by another.
        // "Nice to meet you." is key 16843266 = 0x01010202.
        let bytes = [
            0xFD, 0x02, 0x02, 0x01, 0x00, 0xFD, b' ', 0xFD, 0x02, 0x02, 0x01, 0x01, 0xFD,
        ];
        assert_eq!(decode(&bytes), "{Greetings} {Nice to meet you.}");
    }

    #[test]
    fn resolves_regardless_of_lang_byte() {
        // Real wire sample observed in /shout: lang=0x00 instead of the
        // table's native 0x02. Must still resolve.
        let bytes = [0xFD, 0x02, 0x00, 0x0F, 0x02, 0xFD];
        assert_eq!(decode(&bytes), "{Party}");
    }

    #[test]
    fn table_is_populated() {
        // Smoke test: query via the stripped (lang-less) key form that
        // `resolve` uses. 66050 = 0x00010202 -> stripped 0x00010002.
        assert_eq!(table().get(&0x0001_0002), Some(&"Greetings"));
        assert!(table().len() > 28_000);
    }
}
