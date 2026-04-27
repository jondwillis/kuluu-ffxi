use std::collections::HashMap;
use std::sync::OnceLock;

const MARKER: u8 = 0xFD;

pub fn decode(bytes: &[u8]) -> String {
    if !bytes.contains(&MARKER) {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == MARKER {
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

fn resolve(ty: u8, _lang: u8, cat: u8, idx: u8) -> String {
    let key = (ty as u32) | ((cat as u32) << 16) | ((idx as u32) << 24);
    if let Some(s) = table().get(&key) {
        return (*s).to_string();
    }
    format!("AT:{:02x}/{:02x}/{:02x}", ty, cat, idx)
}

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
    fn table_is_populated() {
        assert_eq!(table().get(&0x0001_0002), Some(&"Greetings"));
        assert!(table().len() > 28_000);
    }
}
