use std::collections::BTreeMap;
use std::path::Path;

use crate::archive::DatRoot;
use crate::chunk;

// research/xi-tools/docs/dats/ROM_118_115.md "Section layout": the menu DAT
// holds one section per mission/quest category plus `sc_item_`, the key-item
// table. A section opens with its 16-byte resource id, then an entry count and
// a flat entry array, then the text blob the entries point into.

/// VTABLE/FTABLE file id of the English key-item text.
/// vendor/POLUtils/PlayOnline.FFXI.Utils.DataBrowser/ROMFileMappings.xml lists
/// the per-language members of every menu DAT; the Japanese one carries the
/// `sc_item ` section instead.
pub const KEY_ITEM_TEXT_FILE_ID: u32 = 82;

const KEY_ITEM_TEXT_ERA_ROM_PATH: &str = "ROM/118/115.DAT";

pub const KEY_ITEM_SECTION_KIND: u8 = 0x51;

pub const KEY_ITEM_CHUNK_NAME: [u8; 4] = *b"sc_i";

/// research/XIClient/src/XIClient/source/UI/UIManager.cpp
/// `LanguageDependentMenuTable`: the client addresses this section by its full
/// resource id, and the trailing `_` is what separates the English member from
/// the Japanese `sc_item `.
const SECTION_RESOURCE_ID: &[u8; 16] = b"menu    sc_item_";

const ENTRY_COUNT_OFFSET: usize = SECTION_RESOURCE_ID.len() + size_of::<u32>();

const ENTRIES_OFFSET: usize = ENTRY_COUNT_OFFSET + size_of::<u32>();

const ENTRY_WORDS: usize = 5;

const ENTRY_LEN: usize = ENTRY_WORDS * size_of::<u32>();

const ENTRY_NAME_WORD: usize = 2;

/// The table's first row carries its own column headers ("a/an/the/some",
/// "in-text name") under id 0, which is no key item.
const COLUMN_HEADER_ROW_ID: u32 = 0;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyItemTableError {
    #[error("no kind 0x{KEY_ITEM_SECTION_KIND:02X} chunk named {}", String::from_utf8_lossy(&KEY_ITEM_CHUNK_NAME))]
    SectionMissing,

    #[error(
        "section resource id is {found:?}, not {}",
        String::from_utf8_lossy(SECTION_RESOURCE_ID)
    )]
    NotTheEnglishSection { found: String },

    #[error("{count} entries do not fit in the section's {len} bytes")]
    EntryCountOutOfRange { count: u32, len: usize },
}

// vendor/POLUtils/PlayOnline.FFXI/FFXIEncryption.cs
// FFXIEncryption.GetTextShiftSize: a text run is rotated by an amount the
// decoder recovers from the popcounts of its own first two bytes, which is why
// neighbouring strings in one blob come back at different rotations.
const TEXT_SHIFT_BY_POPCOUNT_DELTA: [u32; 5] = [1, 7, 2, 6, 3];

/// No rotation: POLUtils' `Rotate` leaves a run untouched for a shift outside
/// 1..=8, which is what `GetTextShiftSize` returns for a run too short to key
/// on or one opening on two zero bytes.
const TEXT_SHIFT_NONE: u32 = 0;

fn text_shift(encoded: &[u8]) -> u32 {
    let (Some(&first), Some(&second)) = (encoded.first(), encoded.get(1)) else {
        return TEXT_SHIFT_NONE;
    };
    if first == 0 && second == 0 {
        return TEXT_SHIFT_NONE;
    }
    let delta = second.count_ones() as i32 - first.count_ones() as i32;
    TEXT_SHIFT_BY_POPCOUNT_DELTA[delta.unsigned_abs() as usize % TEXT_SHIFT_BY_POPCOUNT_DELTA.len()]
}

fn decode_text(encoded: &[u8]) -> String {
    let shift = text_shift(encoded);
    encoded
        .iter()
        .map(|&b| b.rotate_right(shift) as char)
        .collect()
}

/// A rotated byte is zero only if it was, so the terminator is found before the
/// shift is known.
fn read_text_at(section: &[u8], offset: usize) -> Option<String> {
    let rest = section.get(offset..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    Some(decode_text(&rest[..end]))
}

fn read_u32_le(b: &[u8], off: usize) -> Option<u32> {
    let w = b.get(off..off + size_of::<u32>())?;
    Some(u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
}

fn is_key_item_chunk(c: &chunk::Chunk<'_>) -> bool {
    c.kind == KEY_ITEM_SECTION_KIND && c.name == KEY_ITEM_CHUNK_NAME
}

pub fn parse_key_item_names(dat_bytes: &[u8]) -> Result<BTreeMap<u16, String>, KeyItemTableError> {
    let section = chunk::walk(dat_bytes)
        .filter_map(|r| r.ok())
        .find(is_key_item_chunk)
        .ok_or(KeyItemTableError::SectionMissing)?
        .data;

    let resource_id = section
        .get(..SECTION_RESOURCE_ID.len())
        .ok_or(KeyItemTableError::SectionMissing)?;
    if resource_id != SECTION_RESOURCE_ID {
        return Err(KeyItemTableError::NotTheEnglishSection {
            found: String::from_utf8_lossy(resource_id).into_owned(),
        });
    }

    let count = read_u32_le(section, ENTRY_COUNT_OFFSET).unwrap_or(0);
    let entries_end = (count as usize)
        .checked_mul(ENTRY_LEN)
        .and_then(|len| ENTRIES_OFFSET.checked_add(len));
    if entries_end.is_none_or(|end| end > section.len()) {
        return Err(KeyItemTableError::EntryCountOutOfRange {
            count,
            len: section.len(),
        });
    }

    let mut names = BTreeMap::new();
    for entry in 0..count as usize {
        let at = ENTRIES_OFFSET + entry * ENTRY_LEN;
        let id = read_u32_le(section, at).unwrap_or(COLUMN_HEADER_ROW_ID);
        if id == COLUMN_HEADER_ROW_ID || id > u32::from(u16::MAX) {
            continue;
        }
        let name_offset =
            read_u32_le(section, at + ENTRY_NAME_WORD * size_of::<u32>()).unwrap_or(0) as usize;
        if let Some(name) = read_text_at(section, name_offset).filter(|n| !n.is_empty()) {
            names.insert(id as u16, name);
        }
    }
    Ok(names)
}

/// The installed client's key-item names, keyed by the id
/// `vendor/server/scripts/enum/key_item.lua` uses. Empty when the DAT is
/// missing or malformed, so a partial install degrades to whatever the caller
/// already had.
#[derive(Default)]
pub struct KeyItemTable {
    names: BTreeMap<u16, String>,
}

impl KeyItemTable {
    pub fn open_from_root(root: &DatRoot) -> KeyItemTable {
        let path = match root.resolve(KEY_ITEM_TEXT_FILE_ID) {
            Ok(loc) => loc.path_under(root),
            Err(e) => {
                let fallback = root.root().join(KEY_ITEM_TEXT_ERA_ROM_PATH);
                eprintln!(
                    "key item text: file id {KEY_ITEM_TEXT_FILE_ID} unresolved under {} ({e}); using {}",
                    root.root().display(),
                    fallback.display()
                );
                fallback
            }
        };
        Self::open_path(&path)
    }

    pub fn open(root_dir: &Path) -> KeyItemTable {
        match DatRoot::open(root_dir) {
            Ok(root) => Self::open_from_root(&root),
            Err(_) => Self::open_path(&root_dir.join(KEY_ITEM_TEXT_ERA_ROM_PATH)),
        }
    }

    fn open_path(path: &Path) -> KeyItemTable {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("key item text: {} unreadable ({e})", path.display());
                return KeyItemTable::default();
            }
        };
        match parse_key_item_names(&bytes) {
            Ok(names) => KeyItemTable { names },
            Err(e) => {
                eprintln!("key item text: {} rejected ({e})", path.display());
                KeyItemTable::default()
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn lookup(&self, key_item_id: u16) -> Option<&str> {
        self.names.get(&key_item_id).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::CHUNK_KIND_MASK;

    /// The retail chunk header (`crate::chunk::ChunkWalker`): 16 bytes, the size field in
    /// 16-byte units; the fixture uses only the name word and the kind/size word.
    const CHUNK_HEADER_BYTES: usize = 16;
    const CHUNK_HEADER_USED_BYTES: usize = KEY_ITEM_CHUNK_NAME.len() + size_of::<u32>();

    fn synth_chunk(name: &[u8; 4], kind: u8, body: &[u8]) -> Vec<u8> {
        let total = CHUNK_HEADER_BYTES + body.len();
        let padded_total = total.div_ceil(CHUNK_HEADER_BYTES) * CHUNK_HEADER_BYTES;
        let size_units = (padded_total / CHUNK_HEADER_BYTES) as u32;
        let value = (size_units << 7) | (kind as u32 & CHUNK_KIND_MASK);
        let mut out = Vec::with_capacity(padded_total);
        out.extend_from_slice(name);
        out.extend_from_slice(&value.to_le_bytes());
        out.extend(std::iter::repeat_n(
            0u8,
            CHUNK_HEADER_BYTES - CHUNK_HEADER_USED_BYTES,
        ));
        out.extend_from_slice(body);
        out.extend(std::iter::repeat_n(0u8, padded_total - total));
        out
    }

    fn encode_text(text: &str) -> Vec<u8> {
        let plain: Vec<u8> = text.bytes().collect();
        let shift = text_shift(&plain);
        let encoded: Vec<u8> = plain.iter().map(|&b| b.rotate_left(shift)).collect();
        assert_eq!(text_shift(&encoded), shift, "{text:?} re-keys on decode");
        encoded
    }

    struct SynthSection {
        entries: Vec<(u32, String)>,
    }

    impl SynthSection {
        fn bytes(&self) -> Vec<u8> {
            let mut head = Vec::new();
            head.extend_from_slice(SECTION_RESOURCE_ID);
            head.extend_from_slice(&0u32.to_le_bytes());
            head.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());

            let mut text = Vec::new();
            let text_base = ENTRIES_OFFSET + self.entries.len() * ENTRY_LEN;
            for (id, name) in &self.entries {
                let article = text_base + text.len();
                text.extend_from_slice(&encode_text("the"));
                text.push(0);
                let name_offset = text_base + text.len();
                text.extend_from_slice(&encode_text(name));
                text.push(0);
                head.extend_from_slice(&id.to_le_bytes());
                head.extend_from_slice(&(article as u32).to_le_bytes());
                head.extend_from_slice(&(name_offset as u32).to_le_bytes());
                head.extend_from_slice(&0u32.to_le_bytes());
                head.extend_from_slice(&0u32.to_le_bytes());
            }
            head.extend_from_slice(&text);
            head
        }

        fn dat(&self) -> Vec<u8> {
            synth_chunk(&KEY_ITEM_CHUNK_NAME, KEY_ITEM_SECTION_KIND, &self.bytes())
        }
    }

    fn well_formed() -> SynthSection {
        SynthSection {
            entries: vec![
                (COLUMN_HEADER_ROW_ID, "in-text name".to_string()),
                (1, "Zeruhn report".to_string()),
                (256, "treasure map".to_string()),
                (395, "map of the Zeruhn Mines".to_string()),
            ],
        }
    }

    #[test]
    fn every_shift_the_heuristic_selects_round_trips() {
        for text in [
            "the",
            "an",
            "Zeruhn report",
            "All-You-Can-Ride Pass",
            "For GM use only!",
            "Dynamis Debugger",
            "a/an/the/some",
        ] {
            let encoded = encode_text(text);
            assert_eq!(decode_text(&encoded), text);
        }
    }

    #[test]
    fn a_run_too_short_or_zero_led_is_not_rotated() {
        assert_eq!(text_shift(&[]), TEXT_SHIFT_NONE);
        assert_eq!(text_shift(&[0x41]), TEXT_SHIFT_NONE);
        assert_eq!(text_shift(&[0x00, 0x00]), TEXT_SHIFT_NONE);
    }

    #[test]
    fn parses_a_well_formed_synthetic_section() {
        let names = parse_key_item_names(&well_formed().dat()).unwrap();
        assert_eq!(names.get(&1).map(String::as_str), Some("Zeruhn report"));
        assert_eq!(names.get(&256).map(String::as_str), Some("treasure map"));
        assert_eq!(
            names.get(&395).map(String::as_str),
            Some("map of the Zeruhn Mines")
        );
    }

    #[test]
    fn the_column_header_row_is_not_a_key_item() {
        let names = parse_key_item_names(&well_formed().dat()).unwrap();
        assert!(!names.contains_key(&(COLUMN_HEADER_ROW_ID as u16)));
        assert_eq!(names.len(), well_formed().entries.len() - 1);
    }

    #[test]
    fn requires_both_kind_and_name() {
        let body = well_formed().bytes();
        assert_eq!(
            parse_key_item_names(&synth_chunk(b"xxxx", KEY_ITEM_SECTION_KIND, &body)),
            Err(KeyItemTableError::SectionMissing)
        );
        assert_eq!(
            parse_key_item_names(&synth_chunk(
                &KEY_ITEM_CHUNK_NAME,
                KEY_ITEM_SECTION_KIND + 1,
                &body
            )),
            Err(KeyItemTableError::SectionMissing)
        );
    }

    #[test]
    fn rejects_a_section_that_is_not_the_english_member() {
        let mut body = well_formed().bytes();
        body[..SECTION_RESOURCE_ID.len()].copy_from_slice(b"menu    sc_item ");
        let err = parse_key_item_names(&synth_chunk(
            &KEY_ITEM_CHUNK_NAME,
            KEY_ITEM_SECTION_KIND,
            &body,
        ));
        assert_eq!(
            err,
            Err(KeyItemTableError::NotTheEnglishSection {
                found: "menu    sc_item ".to_string()
            })
        );
    }

    #[test]
    fn rejects_an_entry_count_the_section_cannot_hold() {
        let mut body = well_formed().bytes();
        let bogus = u32::MAX;
        body[ENTRY_COUNT_OFFSET..ENTRY_COUNT_OFFSET + size_of::<u32>()]
            .copy_from_slice(&bogus.to_le_bytes());
        let len = body.len().div_ceil(CHUNK_HEADER_BYTES) * CHUNK_HEADER_BYTES;
        assert_eq!(
            parse_key_item_names(&synth_chunk(
                &KEY_ITEM_CHUNK_NAME,
                KEY_ITEM_SECTION_KIND,
                &body
            )),
            Err(KeyItemTableError::EntryCountOutOfRange { count: bogus, len })
        );
    }

    #[test]
    fn malformed_file_yields_an_empty_table() {
        let dir = tempfile::tempdir().unwrap();
        assert!(KeyItemTable::open(dir.path()).is_empty());
        let path = dir.path().join(KEY_ITEM_TEXT_ERA_ROM_PATH);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, synth_chunk(b"xxxx", KEY_ITEM_SECTION_KIND, &[])).unwrap();
        assert!(KeyItemTable::open(dir.path()).is_empty());
        std::fs::write(&path, well_formed().dat()).unwrap();
        assert_eq!(
            KeyItemTable::open(dir.path()).lookup(1),
            Some("Zeruhn report")
        );
    }
}
