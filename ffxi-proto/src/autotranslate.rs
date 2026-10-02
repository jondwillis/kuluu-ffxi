include!(concat!(env!("OUT_DIR"), "/autotranslate_table.rs"));

// vendor/POLUtils/PlayOnline.FFXI/FFXIEncoding.cs FFXIEncoding.GetString (the
// "FFXI Extension: Resource Text" branch) and
// vendor/server/src/map/autotranslate.cpp doLookup: a six-byte
// `FD ty lang cat idx FD` run is one auto-translate phrase.
const MARKER: u8 = 0xFD;
const TAG_LEN: usize = 6;

// research/XiPackets/world/server/0x0047/README.md FromIndex: the second tag
// byte is the sender's language (FromIndex + 1), while the table is the one
// English dictionary, so lookups drop it.
const LANGUAGE_AGNOSTIC_KEY_MASK: u32 = 0xFFFF_00FF;

/// `decode` wraps a resolved phrase in these; kuluu-render's chat panel colours
/// spans delimited by them and must import rather than re-type them (no
/// magic numbers: a wire/format contract lives with its emitter).
pub const PHRASE_OPEN: char = '{';
pub const PHRASE_CLOSE: char = '}';

// research/XiPackets/world/server/0x0047/README.md "Additional Information"
// builds an item's tag from the item id: the type byte says which halves of the
// id the tag carries, and the half it drops holds `TAG_HALF_ABSENT`.
pub const TAG_ITEM_BOTH_BYTES: u8 = 0x07;
pub const TAG_ITEM_LOW_BYTE: u8 = 0x09;
pub const TAG_ITEM_HIGH_BYTE: u8 = 0x0A;

// vendor/server/src/map/autotranslate.cpp documents the key-item tag family and absent-byte forms.
pub const TAG_KEY_ITEM_BOTH_BYTES: u8 = 0x13;
pub const TAG_KEY_ITEM_LOW_BYTE: u8 = 0x15;
pub const TAG_KEY_ITEM_HIGH_BYTE: u8 = 0x16;

#[cfg(test)]
const TAG_HALF_ABSENT: u8 = 0xFF;

/// What a tag whose text is an id's name, rather than a dictionary phrase,
/// names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagTarget {
    Item(u16),
    KeyItem(u16),
}

/// The id a tag carries, or `None` for the phrase-dictionary types.
pub fn tag_target(ty: u8, cat: u8, idx: u8) -> Option<TagTarget> {
    let both = (u16::from(cat) << 8) | u16::from(idx);
    let low = u16::from(idx);
    let high = u16::from(cat) << 8;
    Some(match ty {
        TAG_ITEM_BOTH_BYTES => TagTarget::Item(both),
        TAG_ITEM_LOW_BYTE => TagTarget::Item(low),
        TAG_ITEM_HIGH_BYTE => TagTarget::Item(high),
        TAG_KEY_ITEM_BOTH_BYTES => TagTarget::KeyItem(both),
        TAG_KEY_ITEM_LOW_BYTE => TagTarget::KeyItem(low),
        TAG_KEY_ITEM_HIGH_BYTE => TagTarget::KeyItem(high),
        _ => return None,
    })
}

/// Names the ids the installed client owns. `decode`'s frozen table is one dump
/// of one retail dictionary, and an install renames, repurposes and adds ids
/// against it; a resolver that returns `None` leaves the tag to that table.
pub trait NameResolver {
    fn item_name(&self, item_id: u16) -> Option<String>;

    fn key_item_name(&self, key_item_id: u16) -> Option<String>;
}

/// [`decode`]'s resolver: contributes no names of its own, so every tag
/// resolves through the frozen table.
pub struct FrozenNames;

impl NameResolver for FrozenNames {
    fn item_name(&self, _item_id: u16) -> Option<String> {
        None
    }

    fn key_item_name(&self, _key_item_id: u16) -> Option<String> {
        None
    }
}

pub fn decode(bytes: &[u8]) -> String {
    decode_with(bytes, &FrozenNames)
}

/// [`decode`], but resolving item and key-item tags through `names` first.
pub fn decode_with(bytes: &[u8], names: &impl NameResolver) -> String {
    if !bytes.contains(&MARKER) {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == MARKER {
            if i + TAG_LEN - 1 < bytes.len() && bytes[i + TAG_LEN - 1] == MARKER {
                let ty = bytes[i + 1];
                let lang = bytes[i + 2];
                let cat = bytes[i + 3];
                let idx = bytes[i + 4];
                out.push(PHRASE_OPEN);
                out.push_str(&resolve(ty, lang, cat, idx, names));
                out.push(PHRASE_CLOSE);
                i += TAG_LEN;
                continue;
            }

            out.push('\u{FFFD}');
            i += 1;
            continue;
        }

        let start = i;
        while i < bytes.len() && bytes[i] != MARKER {
            i += 1;
        }
        out.push_str(&String::from_utf8_lossy(&bytes[start..i]));
    }
    out
}

/// One run of a `decode`-produced string: either plain text or a resolved
/// autotranslate phrase (the `text` includes its `PHRASE_OPEN`/`PHRASE_CLOSE`
/// delimiters, as `decode` emitted them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhraseSpan {
    pub text: String,
    pub is_phrase: bool,
}

/// Splits a `decode`-produced (or otherwise `PHRASE_OPEN`/`PHRASE_CLOSE`
/// delimited) string into plain-text and phrase spans, so a UI consumer does
/// not re-match the delimiter chars itself. An unterminated `PHRASE_OPEN` runs
/// to the end of the line, mirroring `decode`'s own tolerance for a lone
/// marker.
pub fn split_phrases(line: &str) -> Vec<PhraseSpan> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == PHRASE_OPEN {
            if !buf.is_empty() {
                out.push(PhraseSpan {
                    text: std::mem::take(&mut buf),
                    is_phrase: false,
                });
            }
            let mut phrase = String::from(PHRASE_OPEN);
            for ic in chars.by_ref() {
                phrase.push(ic);
                if ic == PHRASE_CLOSE {
                    break;
                }
            }
            out.push(PhraseSpan {
                text: phrase,
                is_phrase: true,
            });
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        out.push(PhraseSpan {
            text: buf,
            is_phrase: false,
        });
    }
    out
}

fn resolve(ty: u8, _lang: u8, cat: u8, idx: u8, names: &impl NameResolver) -> String {
    let resolved = tag_target(ty, cat, idx).and_then(|target| match target {
        TagTarget::Item(id) => names.item_name(id),
        TagTarget::KeyItem(id) => names.key_item_name(id),
    });
    if let Some(name) = resolved {
        return name;
    }
    let key = (ty as u32) | ((cat as u32) << 16) | ((idx as u32) << 24);
    if let Some(s) = lookup(key) {
        return s.to_string();
    }
    format!("AT:{:02x}/{:02x}/{:02x}", ty, cat, idx)
}

fn lookup(key: u32) -> Option<&'static str> {
    let wanted = key & LANGUAGE_AGNOSTIC_KEY_MASK;
    AUTOTRANSLATE_TABLE
        .binary_search_by_key(&wanted, |(k, _)| k & LANGUAGE_AGNOSTIC_KEY_MASK)
        .ok()
        .map(|i| AUTOTRANSLATE_TABLE[i].1)
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
        let bytes = [b'h', b'i', b' ', 0xFD, 0x02, 0x02, 0x01, 0x01, 0xFD, b'!'];
        assert_eq!(decode(&bytes), "hi {Nice to meet you.}!");
    }

    #[test]
    fn decodes_japanese_client_tag() {
        let bytes = [0xFD, 0x02, 0x01, 0x01, 0x01, 0xFD];
        assert_eq!(decode(&bytes), "{Nice to meet you.}");
    }

    #[test]
    fn category_header_rows_resolve() {
        let bytes = [0xFD, 0x02, 0x02, 0x01, 0x00, 0xFD];
        assert_eq!(decode(&bytes), "{Greetings}");
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
        let bytes = [
            0xFD, 0x02, 0x02, 0x01, 0x00, 0xFD, b' ', 0xFD, 0x02, 0x02, 0x01, 0x01, 0xFD,
        ];
        assert_eq!(decode(&bytes), "{Greetings} {Nice to meet you.}");
    }

    #[test]
    fn resolves_regardless_of_lang_byte() {
        let bytes = [0xFD, 0x02, 0x00, 0x0F, 0x02, 0xFD];
        assert_eq!(decode(&bytes), "{Party}");
    }

    #[test]
    fn c_escapes_in_the_vendor_map_are_decoded() {
        assert_eq!(lookup(0xD005_0207), Some("\" A \" Egg"));
    }

    // research/XiPackets/world/server/0x0047/README.md Additional Information.
    fn client_item_tag(item_no: u16, from_index: u8) -> [u8; TAG_LEN] {
        let (kind, category, index) = match item_no.to_be_bytes() {
            [0, low] => (TAG_ITEM_LOW_BYTE, TAG_HALF_ABSENT, low),
            [high, 0] => (TAG_ITEM_HIGH_BYTE, high, TAG_HALF_ABSENT),
            [high, low] => (TAG_ITEM_BOTH_BYTES, high, low),
        };
        [MARKER, kind, from_index + 1, category, index, MARKER]
    }

    #[test]
    fn tag_target_inverts_the_clients_item_tag_construction() {
        const ENGLISH_FROM_INDEX: u8 = 1;
        for item_no in [1u16, 0x00FF, 0x0100, 0x475F, 0x7000, 0xFF00, 0xFFFF] {
            let tag = client_item_tag(item_no, ENGLISH_FROM_INDEX);
            assert_eq!(
                tag_target(tag[1], tag[3], tag[4]),
                Some(TagTarget::Item(item_no)),
                "item {item_no:#06x} tag {tag:02x?}"
            );
        }
    }

    #[test]
    fn key_item_tags_carry_the_same_three_forms() {
        assert_eq!(
            tag_target(TAG_KEY_ITEM_LOW_BYTE, TAG_HALF_ABSENT, 0x01),
            Some(TagTarget::KeyItem(1))
        );
        assert_eq!(
            tag_target(TAG_KEY_ITEM_BOTH_BYTES, 0x01, 0x8B),
            Some(TagTarget::KeyItem(395))
        );
        assert_eq!(
            tag_target(TAG_KEY_ITEM_HIGH_BYTE, 0x01, TAG_HALF_ABSENT),
            Some(TagTarget::KeyItem(256))
        );
    }

    #[test]
    fn a_phrase_tag_names_no_id() {
        assert_eq!(tag_target(0x02, 0x01, 0x01), None);
    }

    struct OneName(TagTarget, &'static str);

    impl NameResolver for OneName {
        fn item_name(&self, item_id: u16) -> Option<String> {
            (self.0 == TagTarget::Item(item_id)).then(|| self.1.to_string())
        }

        fn key_item_name(&self, key_item_id: u16) -> Option<String> {
            (self.0 == TagTarget::KeyItem(key_item_id)).then(|| self.1.to_string())
        }
    }

    // 0x475F is Mandau in the frozen table (LSB's dump of the retail
    // dictionary); HorizonXI ships that id under its own name.
    const MANDAU_TAG: [u8; TAG_LEN] = [0xFD, 0x07, 0x02, 0x47, 0x5F, 0xFD];
    const ZERUHN_REPORT_TAG: [u8; TAG_LEN] = [0xFD, 0x15, 0x02, 0xFF, 0x01, 0xFD];

    #[test]
    fn decode_is_the_frozen_table() {
        assert_eq!(decode(&MANDAU_TAG), "{Mandau}");
        assert_eq!(decode(&ZERUHN_REPORT_TAG), "{Zeruhn report}");
    }

    #[test]
    fn a_resolver_overrides_the_frozen_table_for_the_ids_it_names() {
        let names = OneName(TagTarget::Item(0x475F), "Onion Greataxe");
        assert_eq!(decode_with(&MANDAU_TAG, &names), "{Onion Greataxe}");
        assert_eq!(
            decode_with(&ZERUHN_REPORT_TAG, &names),
            "{Zeruhn report}",
            "an id the resolver does not name falls back to the frozen table"
        );
    }

    #[test]
    fn a_resolver_never_reaches_a_phrase_tag() {
        let names = OneName(TagTarget::Item(0x0102), "not a phrase");
        let bytes = [0xFD, 0x02, 0x02, 0x01, 0x01, 0xFD];
        assert_eq!(decode_with(&bytes, &names), "{Nice to meet you.}");
    }

    #[test]
    fn table_is_populated() {
        assert_eq!(lookup(0x0001_0002), Some("Greetings"));
        assert_eq!(lookup(0x0101_0202), Some("Nice to meet you."));
        assert!(AUTOTRANSLATE_TABLE.len() > 28_000);
    }

    #[test]
    fn table_is_strictly_sorted_by_language_agnostic_key() {
        assert!(
            AUTOTRANSLATE_TABLE
                .windows(2)
                .all(|w| (w[0].0 & LANGUAGE_AGNOSTIC_KEY_MASK)
                    < (w[1].0 & LANGUAGE_AGNOSTIC_KEY_MASK))
        );
    }

    /// Guard: pins decode()'s PHRASE_OPEN/PHRASE_CLOSE contract against
    /// split_phrases so a consumer (kuluu-render's chat panel) does not have
    /// to re-type the delimiter chars itself.
    #[test]
    fn split_phrases_finds_exactly_the_decoded_phrase() {
        let bytes = [b'h', b'i', b' ', 0xFD, 0x02, 0x02, 0x01, 0x01, 0xFD, b'!'];
        let decoded = decode(&bytes);
        assert_eq!(decoded, "hi {Nice to meet you.}!");

        let spans = split_phrases(&decoded);
        assert_eq!(
            spans,
            vec![
                PhraseSpan {
                    text: "hi ".to_string(),
                    is_phrase: false,
                },
                PhraseSpan {
                    text: "{Nice to meet you.}".to_string(),
                    is_phrase: true,
                },
                PhraseSpan {
                    text: "!".to_string(),
                    is_phrase: false,
                },
            ]
        );
        assert_eq!(spans.iter().filter(|s| s.is_phrase).count(), 1);
    }
}
