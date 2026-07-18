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
const CC_KEY_ITEM: u8 = 0x1a;
const CC_CHOCOBO_NAME: u8 = 0x1c;
const CC_SET_COLOR: u8 = 0x1e;
const CC_AUTO: u8 = 0x7f;
/// The one 0x7f code POLUtils reads as two bytes (`7f 85`, player-gender
/// choice); every other `7f <type>` carries one more parameter byte
/// (POLUtils Things/DialogTableEntry.cs, the 0x7f branch).
const AUTO_GENDER_CHOICE: u8 = 0x85;
const PRINTABLE: std::ops::RangeInclusive<u8> = 0x20..=0x7e;

// Inline substitution tag: `01 <len> <kind> <data…>` where `len` is the whole
// tag's byte count and `data` holds `82 <0x80|param>` message-parameter
// references — the newer FFXiMain tag family POLUtils predates and decodes as
// garbage. Grammar verified byte-for-byte against the NA install's zone-230
// DialogTable (ROM/25/39 entries 6428/6434/6437/6440/6446/6447) cross-checked
// with the LSB messageSpecial call signatures that fill their parameters
// (vendor/server/scripts/globals/npc_util.lua giveKeyItem,
// vendor/server/scripts/globals/sparkshop.lua YOU_OBTAIN_ITEM(item, count)).
const CC_INLINE_TAG: u8 = 0x01;
/// `01 <len> <kind>` — anything shorter cannot be a tag.
const INLINE_TAG_MIN_LEN: usize = 3;
const INLINE_TAG_PARAM_REF: u8 = 0x82;
/// Param indexes arrive offset into the high-bit range: `0x80 | index`.
const INLINE_TAG_PARAM_BASE: u8 = 0x80;
const INLINE_KIND_NUM: u8 = 0x03;
const INLINE_KIND_ITEM: u8 = 0x23;
const INLINE_KIND_ITEM_PLURAL: u8 = 0x25;
const INLINE_KIND_ITEM_COUNTED: u8 = 0x29;
const INLINE_KIND_KEY_ITEM: u8 = 0x33;

/// Names [`StringDat::text`] wraps as `{Name}` (plain, via [`plain_marker`]) or
/// `{Name:param}` (parameterized) for each control code. Render-layer
/// post-processing matches on these, so they are defined here with the decoder
/// that emits them — the single source of truth.
/// `render_markers_match_emitted_output` guards the wrapping.
pub const MARKER_PLAYER_NAME: &str = "PlayerName";
pub const MARKER_SPEAKER_NAME: &str = "SpeakerName";
pub const MARKER_SELECTION: &str = "Selection";
pub const MARKER_NUM: &str = "Num";
pub const MARKER_CHOICE: &str = "Choice";
pub const MARKER_ITEM: &str = "Item";
pub const MARKER_KEY_ITEM: &str = "KeyItem";
pub const MARKER_CHOCOBO_NAME: &str = "ChocoboName";
pub const MARKER_SET_COLOR: &str = "SetColor";
pub const MARKER_AUTO: &str = "Auto";

/// `{Auto` — prefix the render layer strips (an `{Auto:N}` formatting terminator,
/// or bare `{Auto}`); derived from [`MARKER_AUTO`].
pub const AUTO_MARKER_PREFIX: &str = "{Auto";
/// `{SetColor` — prefix the render layer strips (a text-color code, not visible
/// text); derived from [`MARKER_SET_COLOR`].
pub const SET_COLOR_MARKER_PREFIX: &str = "{SetColor";
/// `{Choice:` — prefix the render layer resolves (`{Choice:N}[a/b]`); derived from
/// [`MARKER_CHOICE`].
pub const CHOICE_MARKER_PREFIX: &str = "{Choice:";

/// The `{Name}` text the decoder emits for a plain control code — the single
/// place the plain-marker wrapping is spelled, shared by the decoder and the
/// render layer that substitutes `{PlayerName}` / `{SpeakerName}`.
pub fn plain_marker(name: &str) -> String {
    format!("{{{name}}}")
}

// Emote chat-text control sequences, observed in the NA install's emote
// DialogTable (ROM/27/70.DAT; see [`EmoteTextDat`]). Each line wraps its slots
// in CC_AUTO (0x7f) sequences the generic decoder only knows as `{Auto:N}`:
//   7f fc <caster-name slot> 7f fb   — leading caster block
//   7f 88 01 "[the /]" <target-name slot> — article alternative + target
//   7f 90 "[his/her]"                — caster-gender alternative
//   7f 31 00                          — line terminator
// A name slot is the 3-byte sequence 01 01 <id>.
// `emote_control_sequences_compose_observed_layout` guards this grammar.
pub const AUTO_EMOTE_CASTER_OPEN: u8 = 0xFC;
pub const AUTO_EMOTE_CASTER_CLOSE: u8 = 0xFB;
pub const AUTO_EMOTE_TARGET_ARTICLE: u8 = 0x88;
pub const AUTO_EMOTE_GENDER: u8 = 0x90;
pub const AUTO_EMOTE_END: u8 = 0x31;
const EMOTE_NAME_SLOT_PREFIX: [u8; 2] = [0x01, 0x01];
const EMOTE_NAME_SLOT_CASTER: u8 = 0x10;
const EMOTE_NAME_SLOT_TARGET: u8 = 0x11;
const ALT_OPEN: u8 = b'[';
const ALT_SPLIT: u8 = b'/';
const ALT_CLOSE: u8 = b']';

/// Names substituted into an emote line's control-code slots.
#[derive(Debug, Clone, Copy)]
pub struct EmoteLineContext<'a> {
    pub caster: &'a str,
    pub target: Option<&'a str>,
    /// Keep the leading article of the target's `[the /]` alternative (NPC/mob
    /// targets read "…points at the Wild Rabbit."; PCs drop it).
    pub target_article: bool,
}

/// Which `[a/b]` alternative the pending 0x7f code selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmoteAlt {
    None,
    Article,
    Gender,
}

/// Render one raw emote DialogTable entry to a finished chat line,
/// substituting caster/target names and resolving `[a/b]` alternatives.
/// Gender alternatives currently always pick the first branch ("his") — the
/// caster's model gender is not plumbed here yet.
pub fn compose_emote_line(entry: &[u8], ctx: &EmoteLineContext) -> String {
    let mut out = String::with_capacity(entry.len());
    let mut alt = EmoteAlt::None;
    let mut i = 0;
    while i < entry.len() {
        let b = entry[i];
        if b == CC_AUTO {
            match entry.get(i + 1) {
                Some(&AUTO_EMOTE_END) => break,
                Some(&AUTO_EMOTE_TARGET_ARTICLE) => {
                    alt = EmoteAlt::Article;
                    // consumes one param byte after the code
                    i += 3;
                    continue;
                }
                Some(&AUTO_EMOTE_GENDER) => {
                    alt = EmoteAlt::Gender;
                    i += 2;
                    continue;
                }
                Some(&AUTO_EMOTE_CASTER_OPEN) | Some(&AUTO_EMOTE_CASTER_CLOSE) => {
                    i += 2;
                    continue;
                }
                _ => {
                    i += 2;
                    continue;
                }
            }
        }
        if entry[i..].starts_with(&EMOTE_NAME_SLOT_PREFIX) {
            match entry.get(2 + i) {
                Some(&EMOTE_NAME_SLOT_CASTER) => {
                    out.push_str(ctx.caster);
                    i += 3;
                    continue;
                }
                Some(&EMOTE_NAME_SLOT_TARGET) => {
                    if let Some(t) = ctx.target {
                        out.push_str(t);
                    }
                    i += 3;
                    continue;
                }
                _ => {}
            }
        }
        if b == ALT_OPEN && alt != EmoteAlt::None {
            if let Some((first, second, after)) = split_alternative(&entry[i..]) {
                let keep_first = match alt {
                    EmoteAlt::Article => ctx.target_article,
                    _ => true,
                };
                out.push_str(if keep_first { first } else { second });
                alt = EmoteAlt::None;
                i += after;
                continue;
            }
        }
        if PRINTABLE.contains(&b) {
            out.push(b as char);
        }
        i += 1;
    }
    // Untargeted /point carries an unresolved direction placeholder (0x1d)
    // between "points" and "." — dropping it leaves a stray space.
    if let Some(s) = out.strip_suffix(" .") {
        out.truncate(s.len());
        out.push('.');
    }
    out
}

/// Split a leading `[a/b]` run into its branches; returns the byte length of
/// the whole bracketed block as the third element.
fn split_alternative(bytes: &[u8]) -> Option<(&str, &str, usize)> {
    let close = bytes.iter().position(|&b| b == ALT_CLOSE)?;
    let split = bytes[..close].iter().position(|&b| b == ALT_SPLIT)?;
    let first = std::str::from_utf8(&bytes[1..split]).ok()?;
    let second = std::str::from_utf8(&bytes[split + 1..close]).ok()?;
    Some((first, second, close + 1))
}

/// Entry index for a MesNum: lines come in (targeted, untargeted) pairs —
/// entry = 2*MesNum + 0 for targeted, +1 for untargeted (verified against the
/// NA install: 0/1 point, 2/3 bow, 4/5 salute, …).
pub fn emote_line_index(mes_num: u16, targeted: bool) -> usize {
    mes_num as usize * 2 + usize::from(!targeted)
}

/// MesNum 0..=96 (LSB Emote::Point..=Emote::Aim) each carry a
/// (targeted, untargeted) line pair, so a plausible emote table holds at least
/// 2*97 entries (the NA install's ROM/27/70.DAT has 198).
pub const EMOTE_TABLE_MIN_ENTRIES: usize = 2 * 97;

/// The canned-emote chat-text DialogTable. Located at ROM/27/70.DAT in the NA
/// install (empirical — found by scan, not by a documented file id; other
/// regions may relocate it, hence the parse-shape validation on open).
pub struct EmoteTextDat {
    dat: StringDat,
}

/// `<install root>/ROM/27/70.DAT` (FTABLE sub_path dir 27, file 70).
pub const EMOTE_TEXT_SUB_PATH: (u16, u8) = (27, 70);

impl EmoteTextDat {
    pub fn open(root: &crate::DatRoot) -> Option<Self> {
        let (dir, file) = EMOTE_TEXT_SUB_PATH;
        let path = root
            .root()
            .join("ROM")
            .join(dir.to_string())
            .join(format!("{file}.DAT"));
        let bytes = std::fs::read(path).ok()?;
        let dat = StringDat::parse(&bytes).ok()?;
        (dat.len() >= EMOTE_TABLE_MIN_ENTRIES).then_some(Self { dat })
    }

    /// The composed chat line for a MesNum, or `None` when the table has no
    /// such entry.
    pub fn line(&self, mes_num: u16, targeted: bool, ctx: &EmoteLineContext) -> Option<String> {
        let raw = self.dat.raw(emote_line_index(mes_num, targeted))?;
        Some(compose_emote_line(raw, ctx))
    }
}

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
            CC_PLAYER_NAME => push_plain(&mut out, MARKER_PLAYER_NAME),
            CC_SPEAKER_NAME => push_plain(&mut out, MARKER_SPEAKER_NAME),
            CC_SELECTION => push_plain(&mut out, MARKER_SELECTION),
            CC_NUM => push_param(&mut out, bytes, &mut i, MARKER_NUM),
            CC_CHOICE => push_param(&mut out, bytes, &mut i, MARKER_CHOICE),
            CC_ITEM => push_param(&mut out, bytes, &mut i, MARKER_ITEM),
            CC_KEY_ITEM => push_param(&mut out, bytes, &mut i, MARKER_KEY_ITEM),
            CC_CHOCOBO_NAME => push_param(&mut out, bytes, &mut i, MARKER_CHOCOBO_NAME),
            CC_SET_COLOR => push_param(&mut out, bytes, &mut i, MARKER_SET_COLOR),
            CC_AUTO => match bytes.get(i + 1) {
                Some(&kind) => {
                    out.push_str(&format!("{{{MARKER_AUTO}:{kind}}}"));
                    i += 1;
                    if kind != AUTO_GENDER_CHOICE && i + 1 < bytes.len() {
                        i += 1;
                    }
                }
                None => push_plain(&mut out, MARKER_AUTO),
            },
            // A malformed tag drops only the 0x01, like other control bytes.
            CC_INLINE_TAG => {
                if let Some(tag) = parse_inline_tag(bytes, i) {
                    if let Some(marker) = tag.marker {
                        out.push_str(&format!("{{{marker}:{}}}", tag.param));
                    }
                    i += tag.len - 1;
                }
            }
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

struct InlineTag {
    /// Marker name to emit, `None` for a recognized-but-unrenderable kind
    /// (the tag is still consumed whole so its data bytes never leak as text).
    marker: Option<&'static str>,
    /// Message-parameter index from the tag's last `82 <0x80|n>` reference —
    /// for item kinds that also carry a count/plural reference, the id ref
    /// comes last (observed: `01 09 29 82 81 80 80 82 80` = count param 1,
    /// item id param 0).
    param: u8,
    len: usize,
}

fn parse_inline_tag(bytes: &[u8], at: usize) -> Option<InlineTag> {
    let len = *bytes.get(at + 1)? as usize;
    if len < INLINE_TAG_MIN_LEN || at + len > bytes.len() {
        return None;
    }
    let data = &bytes[at + 3..at + len];
    if !data.iter().all(|&b| b & INLINE_TAG_PARAM_BASE != 0) {
        return None; // tag data lives in the high-bit range; printable text does not
    }
    let marker = match bytes[at + 2] {
        INLINE_KIND_NUM => Some(MARKER_NUM),
        INLINE_KIND_ITEM | INLINE_KIND_ITEM_PLURAL | INLINE_KIND_ITEM_COUNTED => Some(MARKER_ITEM),
        INLINE_KIND_KEY_ITEM => Some(MARKER_KEY_ITEM),
        _ => None,
    };
    let param = data
        .windows(2)
        .rev()
        .find(|w| w[0] == INLINE_TAG_PARAM_REF)
        .map(|w| w[1] & !INLINE_TAG_PARAM_BASE)
        .unwrap_or(0);
    Some(InlineTag { marker, param, len })
}

/// Emit `{name}` for a control code with no parameter.
fn push_plain(out: &mut String, name: &str) {
    out.push_str(&plain_marker(name));
}

/// Emit `{name:N}` for a parameterized control code, consuming the param byte.
fn push_param(out: &mut String, bytes: &[u8], i: &mut usize, name: &str) {
    if let Some(&p) = bytes.get(*i + 1) {
        out.push_str(&format!("{{{name}:{p}}}"));
        *i += 1;
    } else {
        push_plain(out, name);
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
    fn render_markers_match_emitted_output() {
        // The render layer strips {Auto…}/{SetColor…}, resolves {Choice:…}, and
        // substitutes plain {PlayerName}; every prefix/marker it keys off must
        // match what the decoder actually emits, and derive from the names.
        let auto = StringDat::parse(&synth(&[&[CC_AUTO, 0x31]])).expect("parse");
        assert!(auto.text(0).unwrap().starts_with(AUTO_MARKER_PREFIX));
        let color = StringDat::parse(&synth(&[&[CC_SET_COLOR, 1]])).expect("parse");
        assert!(color.text(0).unwrap().starts_with(SET_COLOR_MARKER_PREFIX));
        let choice = StringDat::parse(&synth(&[&[CC_CHOICE, 0]])).expect("parse");
        assert!(choice.text(0).unwrap().starts_with(CHOICE_MARKER_PREFIX));
        let name = StringDat::parse(&synth(&[&[CC_PLAYER_NAME]])).expect("parse");
        assert_eq!(name.text(0).unwrap(), plain_marker(MARKER_PLAYER_NAME));
        // The chat/dialog render layer resolves {KeyItem:N}/{Item:N} through
        // the scraped name tables keyed on these exact prefixes.
        let key_item = StringDat::parse(&synth(&[&[CC_KEY_ITEM, 0]])).expect("parse");
        assert_eq!(
            key_item.text(0).unwrap(),
            format!("{{{MARKER_KEY_ITEM}:0}}")
        );
        let item = StringDat::parse(&synth(&[&[CC_ITEM, 1]])).expect("parse");
        assert_eq!(item.text(0).unwrap(), format!("{{{MARKER_ITEM}:1}}"));

        assert!(AUTO_MARKER_PREFIX.ends_with(MARKER_AUTO));
        assert!(SET_COLOR_MARKER_PREFIX.ends_with(MARKER_SET_COLOR));
        assert_eq!(CHOICE_MARKER_PREFIX, format!("{{{MARKER_CHOICE}:"));
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

    const CASTER_SLOT: [u8; 3] = [0x01, 0x01, EMOTE_NAME_SLOT_CASTER];
    const TARGET_SLOT: [u8; 3] = [0x01, 0x01, EMOTE_NAME_SLOT_TARGET];

    /// Byte-for-byte the observed ROM/27/70 entry layouts (targeted /point,
    /// untargeted /bow, untargeted /bell) — the grammar the AUTO_EMOTE_* consts
    /// pin.
    fn emote_entry(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    #[test]
    fn emote_control_sequences_compose_observed_layout() {
        let targeted_point = emote_entry(&[
            &[CC_AUTO, AUTO_EMOTE_CASTER_OPEN],
            &CASTER_SLOT,
            &[CC_AUTO, AUTO_EMOTE_CASTER_CLOSE],
            b" points at ",
            &[CC_AUTO, AUTO_EMOTE_TARGET_ARTICLE, 0x01],
            b"[the /]",
            &TARGET_SLOT,
            b".",
            &[CC_AUTO, AUTO_EMOTE_END, 0x00, CC_NEWLINE],
        ]);
        let npc = EmoteLineContext {
            caster: "Kupo",
            target: Some("Wild Rabbit"),
            target_article: true,
        };
        assert_eq!(
            compose_emote_line(&targeted_point, &npc),
            "Kupo points at the Wild Rabbit."
        );
        let pc = EmoteLineContext {
            target_article: false,
            ..npc
        };
        assert_eq!(
            compose_emote_line(&targeted_point, &pc),
            "Kupo points at Wild Rabbit."
        );

        let untargeted_bow = emote_entry(&[
            &[CC_AUTO, AUTO_EMOTE_CASTER_OPEN],
            &CASTER_SLOT,
            &[CC_AUTO, AUTO_EMOTE_CASTER_CLOSE],
            b" bows.",
            &[CC_AUTO, AUTO_EMOTE_END, 0x00, CC_NEWLINE],
        ]);
        let solo = EmoteLineContext {
            caster: "Kupo",
            target: None,
            target_article: false,
        };
        assert_eq!(compose_emote_line(&untargeted_bow, &solo), "Kupo bows.");

        let untargeted_bell = emote_entry(&[
            &[CC_AUTO, AUTO_EMOTE_CASTER_OPEN],
            &CASTER_SLOT,
            &[CC_AUTO, AUTO_EMOTE_CASTER_CLOSE],
            b" rings ",
            &[CC_AUTO, AUTO_EMOTE_GENDER],
            b"[his/her] bell.",
            &[CC_AUTO, AUTO_EMOTE_END, 0x00, CC_NEWLINE],
        ]);
        assert_eq!(
            compose_emote_line(&untargeted_bell, &solo),
            "Kupo rings his bell."
        );
    }

    #[test]
    fn stray_direction_placeholder_space_is_tidied() {
        // Untargeted /point: "points <0x1d>." — the placeholder drops, and so
        // must the space it leaves behind.
        let entry = emote_entry(&[
            &CASTER_SLOT,
            b" points ",
            &[0x1d],
            b".",
            &[CC_AUTO, AUTO_EMOTE_END],
        ]);
        let ctx = EmoteLineContext {
            caster: "Kupo",
            target: None,
            target_article: false,
        };
        assert_eq!(compose_emote_line(&entry, &ctx), "Kupo points.");
    }

    /// The (targeted, untargeted) pairing invariant: entry = 2*MesNum + 0|1.
    #[test]
    fn emote_line_index_pairs_targeted_first() {
        assert_eq!(emote_line_index(0, true), 0);
        assert_eq!(emote_line_index(0, false), 1);
        assert_eq!(emote_line_index(1, true), 2);
        assert_eq!(emote_line_index(96, false), 193);
        assert!(emote_line_index(96, false) < EMOTE_TABLE_MIN_ENTRIES);
    }

    /// Composes real lines from the retail install when present; self-skips
    /// without game files.
    #[test]
    fn real_emote_table_composes_bow_lines() {
        let Some(root) = crate::archive::open_test_install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        let Some(table) = EmoteTextDat::open(&root) else {
            eprintln!("skipping: no ROM/27/70.DAT emote table");
            return;
        };
        let ctx = EmoteLineContext {
            caster: "Kupo",
            target: Some("Naji"),
            target_article: false,
        };
        assert_eq!(
            table.line(1, true, &ctx).as_deref(),
            Some("Kupo bows courteously to Naji.")
        );
        assert_eq!(table.line(1, false, &ctx).as_deref(), Some("Kupo bows."));
    }

    #[test]
    fn decodes_inline_substitution_tags() {
        // Byte-for-byte the NA install's zone-230 DialogTable layouts:
        // entry 6437 "Obtained key item: <keyitem>." wraps its tag in a
        // three-byte `7f 80 01` code, entry 6440 "You obtain <n> <item>!"
        // carries a number tag (param 1) and a counted item tag (id param 0).
        let key_item = [
            b'k',
            b'i',
            b':',
            b' ',
            CC_AUTO,
            0x80,
            0x01,
            CC_INLINE_TAG,
            0x05,
            INLINE_KIND_KEY_ITEM,
            0x82,
            0x80,
            0x80,
            0x80,
            b'.',
        ];
        let dat = StringDat::parse(&synth(&[&key_item])).expect("parse");
        assert_eq!(dat.text(0).as_deref(), Some("ki: {Auto:128}{KeyItem:0}."));

        let obtain = [
            b'g',
            b'e',
            b't',
            b' ',
            CC_INLINE_TAG,
            0x05,
            INLINE_KIND_NUM,
            0x82,
            0x81,
            b' ',
            CC_INLINE_TAG,
            0x09,
            INLINE_KIND_ITEM_COUNTED,
            0x82,
            0x81,
            0x80,
            0x80,
            0x82,
            0x80,
            b'!',
        ];
        let dat = StringDat::parse(&synth(&[&obtain])).expect("parse");
        assert_eq!(dat.text(0).as_deref(), Some("get {Num:1} {Item:0}!"));

        let singular = [CC_INLINE_TAG, 0x05, INLINE_KIND_ITEM, 0x82, 0x80];
        let dat = StringDat::parse(&synth(&[&singular])).expect("parse");
        assert_eq!(dat.text(0).as_deref(), Some("{Item:0}"));
    }

    #[test]
    fn inline_tag_unknown_kind_is_consumed_silently() {
        let entry = [b'a', CC_INLINE_TAG, 0x05, 0x77, 0x82, 0x81, b'b'];
        let dat = StringDat::parse(&synth(&[&entry])).expect("parse");
        assert_eq!(dat.text(0).as_deref(), Some("ab"));
    }

    #[test]
    fn inline_tag_malformed_drops_only_the_control_byte() {
        // Truncated length and printable-range "data" must not swallow text.
        let overrun = [b'x', CC_INLINE_TAG, 0x41, b'A'];
        let dat = StringDat::parse(&synth(&[&overrun])).expect("parse");
        assert_eq!(dat.text(0).as_deref(), Some("xAA"));

        let low_data = [CC_INLINE_TAG, 0x05, INLINE_KIND_ITEM, b'h', b'i'];
        let dat = StringDat::parse(&synth(&[&low_data])).expect("parse");
        assert_eq!(dat.text(0).as_deref(), Some("#hi"), "kind byte re-renders");
    }

    /// Southern San d'Oria (zone 230) KEYITEM_OBTAINED in the client era this
    /// repo's default install carries: LSB pinned it at 6437 when its text-id
    /// sync matched this DAT (LandSandBoat b3af49c62ae2, 2023-05-25,
    /// scripts/zones/Southern_San_dOria/IDs.lua — every anchor id in that sync
    /// equals this DAT's physical entry index, confirming ids are identity
    /// DAT indexes). Newer LSB pins say 6438 because SE later inserted
    /// entries; that is client-version skew, not an index-base convention.
    const ZONE230_KEYITEM_OBTAINED_MAY2023: usize = 6437;
    const ZONE230_ID: u16 = 230;

    /// Decodes the real zone-230 KEYITEM_OBTAINED entry to the `{KeyItem:0}`
    /// marker (pre-fix the inline tag leaked as `{Auto:128}3\u{FFFD}`);
    /// self-skips without game files.
    #[test]
    fn real_zone230_keyitem_obtained_decodes_marker() {
        let Some(root) = crate::archive::open_test_install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        let file_id = crate::zone_dat::zone_id_to_string_file_id(ZONE230_ID)
            .expect("zone 230 has a string DAT mapping");
        let loc = root.resolve(file_id).expect("string DAT resolves");
        let bytes = std::fs::read(loc.path_under(root.root())).expect("string DAT readable");
        let dat = StringDat::parse(&bytes).expect("zone 230 dialog table parses");
        let text = dat
            .text(ZONE230_KEYITEM_OBTAINED_MAY2023)
            .expect("entry present");
        assert!(
            text.contains("Obtained key item:"),
            "expected the KEYITEM_OBTAINED entry, got {text:?}"
        );
        assert!(
            text.contains(&format!("{{{MARKER_KEY_ITEM}:0}}")),
            "expected a {{KeyItem:0}} marker, got {text:?}"
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
