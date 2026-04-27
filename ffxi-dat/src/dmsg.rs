//! Parser for FFXI per-zone **dialog string DAT** files — the `DialogTable`
//! format that holds the text the event VM renders (NPC speech, menu prompts).
//! Each zone's event bytecode references these entries by index.
//!
//! Format and the XOR-0x80 obfuscation re-expressed from POLUtils
//! `PlayOnline.FFXI/FileTypes/DialogTable.cs` + `Things/DialogTableEntry.cs`
//! (vendored, build-time reference), verified against a retail install:
//!
//! ```text
//! u32 magic     = 0x10000000 + (file_len - 4)
//! u32 offsets[] ^ 0x80808080   // [0] = 4*count = table size; offsets rel. byte 4
//! u8  text[]    ^ 0x80         // per-entry, sliced by adjacent offsets
//! ```
//!
//! Text decode handles ASCII + the control-code placeholder system; Shift-JIS
//! (cp932) double-byte runs are not yet mapped and surface as `\u{FFFD}` (rare
//! in NA English dialog).

const TEXT_XOR: u8 = 0x80;
const OFFSET_XOR: u32 = 0x8080_8080;
const MAGIC_BASE: u32 = 0x1000_0000;

// DialogTable control codes (POLUtils Things/DialogTableEntry.cs).
const CC_NEWLINE: u8 = 0x07;
const CC_PLAYER_NAME: u8 = 0x08;
const CC_SPEAKER_NAME: u8 = 0x09;
const CC_NUM: u8 = 0x0a;
const CC_SELECTION: u8 = 0x0b;
const CC_CHOICE: u8 = 0x0c;
const CC_ITEM: u8 = 0x19;
const CC_CHOICE2: u8 = 0x1a;
const CC_KEY_ITEM: u8 = 0x1c;
const CC_ELEMENT: u8 = 0x1e;
const CC_AUTO: u8 = 0x7f;
const PRINTABLE: std::ops::RangeInclusive<u8> = 0x20..=0x7e;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StringDatError {
    #[error("not a DialogTable: file too short ({0} bytes)")]
    TooShort(usize),
    #[error("not a DialogTable: header word 0x{got:08x} != expected 0x{want:08x}")]
    BadMagic { got: u32, want: u32 },
    #[error("corrupt DialogTable: first-text offset {0} invalid")]
    BadOffsetTable(u32),
    #[error("corrupt DialogTable: entry {index} offset {offset} out of range")]
    BadEntry { index: usize, offset: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringDat {
    /// XOR-decoded raw bytes per entry, indexed by string id (offset-table order).
    entries: Vec<Vec<u8>>,
}

impl StringDat {
    pub fn parse(buf: &[u8]) -> Result<Self, StringDatError> {
        if buf.len() < 8 {
            return Err(StringDatError::TooShort(buf.len()));
        }
        let want_magic = MAGIC_BASE.wrapping_add(buf.len() as u32 - 4);
        let magic = rd_u32(buf, 0);
        if magic != want_magic {
            return Err(StringDatError::BadMagic {
                got: magic,
                want: want_magic,
            });
        }

        // Offsets are relative to byte 4; offsets[0] doubles as the table size.
        let first = rd_u32(buf, 4) ^ OFFSET_XOR;
        let data_len = buf.len() as u32 - 4;
        if first < 4 || !first.is_multiple_of(4) || first > data_len {
            return Err(StringDatError::BadOffsetTable(first));
        }
        let count = (first / 4) as usize;

        let mut offsets = Vec::with_capacity(count);
        offsets.push(first);
        for i in 1..count {
            offsets.push(rd_u32(buf, 4 + i * 4) ^ OFFSET_XOR);
        }

        // Each entry runs to the next offset in ascending order (entries are laid
        // out contiguously), bounded by data_len.
        let mut sorted = offsets.clone();
        sorted.push(data_len);
        sorted.sort_unstable();

        let mut entries = Vec::with_capacity(count);
        for (index, &start) in offsets.iter().enumerate() {
            if start < first || start > data_len {
                return Err(StringDatError::BadEntry {
                    index,
                    offset: start,
                });
            }
            let end = *sorted
                .iter()
                .find(|&&b| b > start)
                .unwrap_or(&data_len)
                .min(&data_len);
            let lo = 4 + start as usize;
            let hi = 4 + end as usize;
            let mut bytes = buf[lo..hi.max(lo)].to_vec();
            for b in &mut bytes {
                *b ^= TEXT_XOR;
            }
            entries.push(bytes);
        }
        Ok(Self { entries })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// XOR-decoded raw bytes of entry `index` (no text interpretation).
    pub fn raw(&self, index: usize) -> Option<&[u8]> {
        self.entries.get(index).map(Vec::as_slice)
    }

    /// Decode entry `index` to display text: ASCII passthrough, control-code
    /// placeholders as `{…}` markers, newlines from 0x07.
    pub fn text(&self, index: usize) -> Option<String> {
        self.entries.get(index).map(|b| decode_dialog_text(b))
    }

    /// Decode entry `index` as a selection menu: text before the `CC_SELECTION`
    /// marker is the prompt, text after it (split on `CC_NEWLINE`) the options.
    /// `None` if the entry has no Selection marker — i.e. plain speech, not a menu.
    pub fn menu(&self, index: usize) -> Option<(String, Vec<String>)> {
        let bytes = self.entries.get(index)?;
        let sel = bytes.iter().position(|&b| b == CC_SELECTION)?;
        let prompt = decode_dialog_text(&bytes[..sel]).trim().to_string();
        let options = bytes[sel + 1..]
            .split(|&b| b == CC_NEWLINE)
            .map(decode_dialog_text)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Some((prompt, options))
    }
}

fn rd_u32(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

fn is_sjis_lead(b: u8) -> bool {
    matches!(b, 0x81..=0x9F | 0xE0..=0xFC)
}

/// Decode an XOR-decoded dialog entry. Control codes per POLUtils
/// `Things/DialogTableEntry.cs`; the parameterized ones consume a following byte.
fn decode_dialog_text(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            CC_NEWLINE => out.push('\n'),
            CC_PLAYER_NAME => out.push_str("{PlayerName}"),
            CC_SPEAKER_NAME => out.push_str("{SpeakerName}"),
            CC_SELECTION => out.push_str("{Selection}"),
            CC_NUM => push_param(&mut out, bytes, &mut i, "Num"),
            CC_CHOICE => push_param(&mut out, bytes, &mut i, "Choice"),
            CC_ITEM => push_param(&mut out, bytes, &mut i, "Item"),
            CC_CHOICE2 => push_param(&mut out, bytes, &mut i, "Choice2"),
            CC_KEY_ITEM => push_param(&mut out, bytes, &mut i, "KeyItem"),
            CC_ELEMENT => push_param(&mut out, bytes, &mut i, "Element"),
            CC_AUTO => push_param(&mut out, bytes, &mut i, "Auto"),
            _ if PRINTABLE.contains(&b) => out.push(b as char),
            _ if is_sjis_lead(b) => {
                out.push('\u{FFFD}'); // cp932 double-byte run not yet mapped
                i += 1; // skip the trail byte too
            }
            _ => {} // other control bytes: drop
        }
        i += 1;
    }
    out
}

/// Emit `{name:N}` for a parameterized control code, consuming the param byte.
fn push_param(out: &mut String, bytes: &[u8], i: &mut usize, name: &str) {
    if let Some(&p) = bytes.get(*i + 1) {
        out.push_str(&format!("{{{name}:{p}}}"));
        *i += 1;
    } else {
        out.push_str(&format!("{{{name}}}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic DialogTable from per-entry (already-plain) text bytes.
    fn synth(entries: &[&[u8]]) -> Vec<u8> {
        let count = entries.len();
        let table_size = 4 * count;
        let mut offsets = Vec::with_capacity(count);
        let mut running = table_size as u32; // first entry begins after the table
        for e in entries {
            offsets.push(running);
            running += e.len() as u32;
        }
        let data_len = table_size as u32 + entries.iter().map(|e| e.len() as u32).sum::<u32>();

        let mut buf = Vec::new();
        buf.extend_from_slice(&(MAGIC_BASE + data_len).to_le_bytes());
        for off in &offsets {
            buf.extend_from_slice(&(off ^ OFFSET_XOR).to_le_bytes());
        }
        for e in entries {
            buf.extend(e.iter().map(|b| b ^ TEXT_XOR));
        }
        buf
    }

    #[test]
    fn parses_ascii_entries_and_newline() {
        let dat = StringDat::parse(&synth(&[b"Hello", b"Bye\x07!"])).expect("parse");
        assert_eq!(dat.len(), 2);
        assert_eq!(dat.text(0).as_deref(), Some("Hello"));
        assert_eq!(dat.text(1).as_deref(), Some("Bye\n!"));
        assert_eq!(dat.text(2), None);
    }

    #[test]
    fn decodes_placeholder_control_codes() {
        // "Hi " + PlayerName + ", " + Num(5)
        let entry = [b'H', b'i', b' ', 0x08, b',', b' ', 0x0a, 5];
        let dat = StringDat::parse(&synth(&[&entry])).expect("parse");
        assert_eq!(dat.text(0).as_deref(), Some("Hi {PlayerName}, {Num:5}"));
    }

    #[test]
    fn menu_splits_prompt_and_options_on_selection_marker() {
        let entry = [
            b'P',
            b'i',
            b'c',
            b'k',
            b':',
            CC_NEWLINE,
            CC_SELECTION,
            b'A',
            CC_NEWLINE,
            b'B',
            CC_NEWLINE,
            b'C',
        ];
        let dat = StringDat::parse(&synth(&[&entry])).expect("parse");
        let (prompt, options) = dat.menu(0).expect("is a menu");
        assert_eq!(prompt, "Pick:");
        assert_eq!(options, vec!["A", "B", "C"]);
        let plain = StringDat::parse(&synth(&[b"Hello"])).expect("parse");
        assert_eq!(plain.menu(0), None);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut buf = synth(&[b"x"]);
        buf[0] ^= 0xFF;
        assert!(matches!(
            StringDat::parse(&buf),
            Err(StringDatError::BadMagic { .. })
        ));
    }

    #[test]
    fn rejects_too_short() {
        assert_eq!(
            StringDat::parse(&[0u8; 4]),
            Err(StringDatError::TooShort(4))
        );
    }

    /// Loads a real DialogTable from the retail install when present; self-skips
    /// on a machine without game files. ROM3/2/11.DAT is a verified dialog table.
    #[test]
    fn loads_real_dialog_table_when_install_present() {
        let Some(root) = crate::archive::open_test_install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        let path = root.root().join("ROM3").join("2").join("11.DAT");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skipping: {} not present", path.display());
            return;
        };
        let dat = StringDat::parse(&bytes).expect("real dialog table parses");
        assert!(!dat.is_empty());
        let any_text = (0..dat.len()).any(|i| dat.text(i).is_some_and(|t| !t.is_empty()));
        assert!(any_text, "expected at least one non-empty dialog string");
    }
}
