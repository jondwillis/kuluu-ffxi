use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::archive::DatRoot;
use crate::chunk;

// research/xi-tools/docs/dats/ROM_118_115.md "Section layout": the menu DAT
// holds one section per mission/quest category plus `sc_item_`, the key-item
// table. A section opens with its 16-byte resource id, then an entry count and
// a flat entry array, then the text blob the entries point into.
//
// That era table (file id 82) covers ids up to 1878 only. The two-generation
// law below was measured on the PhoenixXI retail-2026 install; where its facts
// disagree with older notes in this file, these measurements win:
//
// * A sweep of every file in that install (52946 DATs) found exactly TWO
//   `menu    sc_item_` sections - ROM/118/113.DAT (JA) and ROM/118/115.DAT
//   (EN). Server-era ids like 1738 (`job gesture: warrior`,
//   `vendor/server/scripts/enum/key_item.lua`) are genuinely absent there.
// * The gen-2 text lives in a plain (un-chunked) `d_msg` fixed-block container
//   at ROM/175/35.DAT - the only file on that install containing the bytes of
//   "job gesture" under any menu text encoding. It is NOT a `sc_item_` section,
//   which is why res-id sweeps missed it; nor are the `menu    keytop...`
//   chunks in ROM/176/74-75.DAT, which turned out to embed `1TXD` window-skin
//   textures (an earlier lead that was wrong - do not revive it).
// * The gen-2 law: header words exactly like [`crate::dmsg`]'s fixed-block
//   layout (tableSize 0, XOR-0xFF past tableOffset), stride 0x2bc, and each
//   block is `u32 count(=7)` + seven {off,flag} descriptor pairs from +4. The
//   pair targets are the d_msg string blobs ({marker, 0x18 metadata, NUL
//   latin1}); slot 0's target instead holds a RAW u32 - the entry's own key-item
//   id, so the table indexes itself like the mission families. Measured on two
//   copies of this file: the game tree's `ROM/175/35.DAT` (3244 blocks) and
//   Pivot's overlay winner over it, `polplugins/DATs/EraDATs/ROM/175/35.DAT`
//   (3205 blocks) - which is what the retail client actually renders. Count
//   uniform, name slot 4, plural slot 5, English description slot 6 with
//   retail's own '\n' wrapping embedded.
// * Id words are wire-stable across copies but block numbers are not (warrior
//   gesture = LSB id 1738 sits at block 1811 in the game tree copy and 1789 in
//   EraDATs). The table carries three genuine duplicate rows - the
//   `Mog Kupon A-Sap/Jad/Rub` coupons appear both as short-description and
//   expanded-description rows (EraDATs blocks 909-911 and 1607-1609, names
//   identical) - so lookups keep the FIRST row claiming an id and log a summary.

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

/// Words 3 and 4 of an entry point at the item's description blocks. The
/// install census (PhoenixXI ROM/118/115.DAT, ids probed live) shows each
/// block is a `{count, ptr*count}` run inside the shared text arena; its own
/// lines carry retail's wrapping and bookend empties/marker runs (a lone
/// `\u{10}` string), so the decoder keeps printable lines in order. Word 3
/// holds the examine text (`Zeruhn report` id 1 opens with Overseer Makarim's
/// letter; `Shadow Lord's participle` id 74 reads "A small part of the / Shadow
/// Lord's body."); word 4 stands in only when word 3 carries no printable line.
const ENTRY_DESCRIPTION_WORD: usize = 3;
const ENTRY_BACKUP_DESCRIPTION_WORD: usize = 4;

/// Probed blocks run up to ten pointers; a larger count means we mis-read the
/// entry and will not follow the pointers into unrelated arena data.
const DESCRIPTION_BLOCK_MAX_POINTERS: u32 = 40;

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

    #[error("not a gen-2 key-item table ({0})")]
    Extended(#[from] crate::dmsg::DmsgError),

    #[error("gen-2 block {block} carries {found} descriptor pairs, not 7")]
    ExtendedBlockShape { block: usize, found: u32 },
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

/// A description block at `off`: `{count, ptr*count}` pointing into the
/// section's text arena. Returns the printable lines in order and empty for a
/// missing/oversized block or one whose strings are all empties and markers.
fn read_description_block(section: &[u8], off: usize) -> Vec<String> {
    let count = match read_u32_le(section, off) {
        Some(n) if (1..=DESCRIPTION_BLOCK_MAX_POINTERS).contains(&n) => n as usize,
        _ => return Vec::new(),
    };
    let mut lines = Vec::new();
    for k in 0..count {
        let Some(ptr) = read_u32_le(section, off + size_of::<u32>() * (k + 1)) else {
            break;
        };
        if usize::try_from(ptr).unwrap_or(usize::MAX) >= section.len() {
            continue;
        }
        let Some(line) = read_text_at(section, ptr as usize) else {
            continue;
        };
        // Empties bookend the block; a run whose first byte is below printable
        // (`\u{10}` in every probed entry) is the table's own slot marker.
        if line.is_empty() || line.as_bytes()[0] < b' ' {
            continue;
        }
        lines.push(line);
    }
    lines
}

pub fn parse_key_item_names(dat_bytes: &[u8]) -> Result<BTreeMap<u16, String>, KeyItemTableError> {
    parse_key_item_entries(dat_bytes).map(|(names, _)| names)
}

/// What both tables hand back: retail's own name per key-item id, and the printable description
/// lines read out of that entry's `{count, ptr}` blocks (empty where it has none). Named once because
/// the gen-1 era container and the gen-2 fixed-block container are the same answer.
pub type KeyItemRows = (BTreeMap<u16, String>, BTreeMap<u16, Vec<String>>);

/// Names plus the `{count, ptr}` description blocks behind entry words 3/4.
pub fn parse_key_item_entries(dat_bytes: &[u8]) -> Result<KeyItemRows, KeyItemTableError> {
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
    let mut descriptions = BTreeMap::new();
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
        for word in [ENTRY_DESCRIPTION_WORD, ENTRY_BACKUP_DESCRIPTION_WORD] {
            let off = read_u32_le(section, at + word * size_of::<u32>()).unwrap_or(0) as usize;
            let block = read_description_block(section, off);
            if !block.is_empty() {
                descriptions.insert(id as u16, block);
                break;
            }
        }
    }
    Ok((names, descriptions))
}

/// File number of the gen-2 (server-era) key-item text table under the install;
/// pinned from the install-wide "job gesture" byte scan described in this
/// module's header. `DatRoot::locate` honours overlay trees, so installs that
/// ship newer members for dir 175 pick them up the way retail does.
pub const KEY_ITEM_EXTENDED_FILE_DIR: u16 = 175;
pub const KEY_ITEM_EXTENDED_FILE_NO: u8 = 35;
const KEY_ITEM_EXTENDED_ERA_ROM_PATH: &str = "ROM/175/35.DAT";

/// Descriptor pairs per gen-2 block, measured uniform across both copies of
/// the file on this install (6449 blocks).
const EXTENDED_PAIRS: usize = 7;
/// Pair whose target holds the entry's raw u32 key-item id (0 = column header
/// or unassigned row), the name, and the English description. Slot 5 (plural)
/// is measured but unused here; kuluu has no consumer for plural forms yet.
const EXTENDED_PAIR_ID: usize = 0;
const EXTENDED_PAIR_NAME: usize = 4;
const EXTENDED_PAIR_DESCRIPTION: usize = 6;

/// Read the gen-2 table (see module header for the measured law): blocks key
/// themselves by their own id word, so no hand-built mapping sits between the
/// wire and the DAT. A wrong pair count condemns the whole file like
/// [`crate::mission_dat`] refuses disagreeing catalogs; duplicate ids do NOT -
/// retail's own shipping data has three (the Mog Kupon coupon rows) - they are
/// kept first-row-wins and counted for a one-line summary.
pub fn parse_extended_key_item_entries(dat_bytes: &[u8]) -> Result<KeyItemRows, KeyItemTableError> {
    let (b, table_offset, table_size, block_size, num_strings) =
        crate::dmsg::decode_common(dat_bytes)?;
    if table_size != 0 {
        return Err(KeyItemTableError::Extended(
            crate::dmsg::DmsgError::NotFixedBlock(table_size),
        ));
    }
    let mut names = BTreeMap::new();
    let mut descriptions = BTreeMap::new();
    let mut claimed: BTreeSet<u16> = BTreeSet::new();
    let mut duplicate_rows = 0usize;
    for block in 0..num_strings {
        let start = table_offset
            .checked_add(block.saturating_mul(block_size))
            .and_then(|s| (s + size_of::<u32>() <= b.len()).then_some(s))
            .ok_or(KeyItemTableError::Extended(
                crate::dmsg::DmsgError::TruncatedHeader,
            ))?;
        let found = read_u32_le(&b, start).unwrap_or(0);
        if found as usize != EXTENDED_PAIRS {
            return Err(KeyItemTableError::ExtendedBlockShape { block, found });
        }
        // Descriptor pairs are {off, flag} - stride eight bytes from +4.
        let pair_off = |pair: usize| read_u32_le(&b, start + 4 + pair * (size_of::<u32>() * 2));
        let id_word = pair_off(EXTENDED_PAIR_ID)
            .and_then(|off| read_u32_le(&b, start + off as usize))
            .ok_or(KeyItemTableError::Extended(
                crate::dmsg::DmsgError::TruncatedHeader,
            ))?;
        if id_word == 0 || id_word > u32::from(u16::MAX) {
            continue;
        }
        let Ok(id) = u16::try_from(id_word) else {
            continue;
        };
        if !claimed.insert(id) {
            // Measured in retail's own file (see module header): keep the
            // earlier row, ignore the later one.
            duplicate_rows += 1;
            continue;
        }
        if let Some(name) = pair_off(EXTENDED_PAIR_NAME)
            .and_then(|off| crate::dmsg::blob_at(&b, start, off as usize))
            .filter(|s| !s.is_empty())
        {
            names.insert(id, name);
        }
        if let Some(text) = pair_off(EXTENDED_PAIR_DESCRIPTION)
            .and_then(|off| crate::dmsg::blob_at(&b, start, off as usize))
        {
            // The blob carries retail's wrapping as '\n' with empty runs
            // bookending it; keep the printable lines in order.
            let lines: Vec<String> = text
                .split('\n')
                .filter(|line| !line.trim().is_empty())
                .map(str::to_string)
                .collect();
            if !lines.is_empty() {
                descriptions.insert(id, lines);
            }
        }
    }
    if duplicate_rows != 0 {
        eprintln!(
            "key item text (gen-2): {duplicate_rows} rows repeat an earlier id; kept the first of each"
        );
    }
    Ok((names, descriptions))
}

/// One key-item menu page as the gen-2 table files it: retail's own label plus,
/// through [`KeyItemFiling::page_of`], the ids stored under it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyItemFiling {
    labels: Vec<String>,
    page_of_id: BTreeMap<u16, u8>,
}

impl KeyItemFiling {
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }

    /// Page names in the order the table files them, retail's own spelling with
    /// [`PAGE_LABEL_MARKER`] removed.
    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    /// The page an id is stored on; `None` for ids the table has no row for.
    pub fn page_of(&self, key_item_id: u16) -> Option<u8> {
        self.page_of_id.get(&key_item_id).copied()
    }

    /// Every id the table files, ascending.
    pub fn ids(&self) -> impl Iterator<Item = u16> + '_ {
        self.page_of_id.keys().copied()
    }

    /// Ids filed per page, index-aligned with [`Self::labels`].
    pub fn ids_on_page(&self, page: u8) -> Vec<u16> {
        self.page_of_id
            .iter()
            .filter(|(_, p)| **p == page)
            .map(|(id, _)| *id)
            .collect()
    }
}

/// Retail marks each page of its key-item menu inside the same table that holds
/// the item text: the row ending a page carries id word 0 and names the page in
/// the name slot, prefixed with this marker.
const PAGE_LABEL_MARKER: char = '-';

/// The name slot of a boundary row, which always opens with the marker: retail's
/// own column-header row (`in-text name`, `English Desc`) is unmarked and files
/// nothing.
fn page_label(text: &str) -> Option<String> {
    let label = text.strip_prefix(PAGE_LABEL_MARKER)?.trim();
    (!label.is_empty()).then(|| label.to_string())
}

/// Read retail's filing of the key-item menu pages out of the gen-2 table (see
/// [`parse_extended_key_item_entries`] for the container law). A row whose id word
/// is zero closes the page above it when its name slot opens with
/// [`PAGE_LABEL_MARKER`]; measured on this install the marked rows sit at blocks
/// 1610/1986/2399... in Pivot's overlay copy of the file and 1632/2022/... in the
/// game tree's own, which is why page boundaries must be found by their label and
/// never by block number. Unmarked id-zero rows (the table's column headers) file
/// nothing. Membership inside a page is retail's own: the gate crystals (ids
/// 352-357, 963-965) and `job gesture: warrior` (1738) sit in the run closed by
/// `-Permanent Key Items`, `voidstone` (1539) under `-Voidwatch`, `Atma of the
/// Savior` (1699) under `-Abyssea`, every claim slip under `-Claim Slips`. Pages
/// come from the table's row order, never from a hand-built id list.
pub fn parse_key_item_filing(dat_bytes: &[u8]) -> Result<KeyItemFiling, KeyItemTableError> {
    let (b, table_offset, table_size, block_size, num_strings) =
        crate::dmsg::decode_common(dat_bytes)?;
    if table_size != 0 {
        return Err(KeyItemTableError::Extended(
            crate::dmsg::DmsgError::NotFixedBlock(table_size),
        ));
    }
    let mut filing = KeyItemFiling::default();
    let mut pending: Vec<u16> = Vec::new();
    for block in 0..num_strings {
        let start = table_offset
            .checked_add(block.saturating_mul(block_size))
            .ok_or(KeyItemTableError::Extended(
                crate::dmsg::DmsgError::TruncatedHeader,
            ))?;
        let found = read_u32_le(&b, start).unwrap_or(0);
        if found as usize != EXTENDED_PAIRS {
            return Err(KeyItemTableError::ExtendedBlockShape { block, found });
        }
        let pair_off = |pair: usize| read_u32_le(&b, start + 4 + pair * (size_of::<u32>() * 2));
        let id_word = pair_off(EXTENDED_PAIR_ID)
            .and_then(|off| read_u32_le(&b, start + off as usize))
            .unwrap_or(0);
        if id_word != COLUMN_HEADER_ROW_ID {
            if let Ok(id) = u16::try_from(id_word) {
                pending.push(id);
                continue;
            }
        }
        let Some(label) = pair_off(EXTENDED_PAIR_NAME)
            .and_then(|off| crate::dmsg::blob_at(&b, start, off as usize))
            .and_then(|text| page_label(&text))
        else {
            continue;
        };
        filing.labels.push(label);
        let page = (filing.labels.len() - 1) as u8;
        for id in pending.drain(..) {
            // First row to claim an id files it, as the text reader does.
            match filing.page_of_id.entry(id) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert(page);
                }
                std::collections::btree_map::Entry::Occupied(_) => {}
            }
        }
    }
    if !pending.is_empty() {
        eprintln!(
            "key item filing: {} rows sit past the last page label, so they are not filed",
            pending.len()
        );
    }
    Ok(filing)
}

/// The installed client's key-item names, keyed by the id
/// `vendor/server/scripts/enum/key_item.lua` uses. Empty when the DAT is
/// missing or malformed, so a partial install degrades to whatever the caller
/// already had.
#[derive(Debug, Clone, Default)]
pub struct KeyItemTable {
    names: BTreeMap<u16, String>,
    descriptions: BTreeMap<u16, Vec<String>>,
    filing: KeyItemFiling,
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
        let mut table = Self::open_path(&path);
        if let Some(loc) = root.locate(KEY_ITEM_EXTENDED_FILE_DIR, KEY_ITEM_EXTENDED_FILE_NO) {
            table.apply_extended(&loc.path_under(root));
        }
        table
    }

    pub fn open(root_dir: &Path) -> KeyItemTable {
        let mut table = match DatRoot::open(root_dir) {
            Ok(root) => Self::open_from_root(&root),
            Err(_) => Self::open_path(&root_dir.join(KEY_ITEM_TEXT_ERA_ROM_PATH)),
        };
        // A plain DAT root without tables (tests, partial installs) still gets
        // the gen-2 file where it sits; re-applying an already-overlaid table
        // is harmless.
        table.apply_extended(&root_dir.join(KEY_ITEM_EXTENDED_ERA_ROM_PATH));
        table
    }

    fn open_path(path: &Path) -> KeyItemTable {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("key item text: {} unreadable ({e})", path.display());
                return KeyItemTable::default();
            }
        };
        match parse_key_item_entries(&bytes) {
            Ok((names, descriptions)) => KeyItemTable {
                filing: KeyItemFiling::default(),
                names,
                descriptions,
            },
            Err(e) => {
                eprintln!("key item text: {} rejected ({e})", path.display());
                KeyItemTable::default()
            }
        }
    }

    /// Overlay the gen-2 table on top of the era rows: its wording is what the
    /// client renders today, so it wins for ids it covers. Ids missing from the
    /// gen-2 file (measured: 148 holes) keep their era text; an absent or
    /// rejected gen-2 file leaves the era table standing with no extra noise.
    fn apply_extended(&mut self, path: &Path) {
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        match parse_extended_key_item_entries(&bytes) {
            Ok((names, descriptions)) => {
                for (id, name) in names {
                    self.names.insert(id, name);
                }
                for (id, lines) in descriptions {
                    // Era text beats nothing: only ids the gen-2 file gives
                    // printable lines for overwrite the era block.
                    self.descriptions.insert(id, lines);
                }
            }
            Err(e) => eprintln!("key item text: {} rejected ({e})", path.display()),
        }
        if self.filing.is_empty() {
            match parse_key_item_filing(&bytes) {
                Ok(filing) => self.filing = filing,
                Err(e) => eprintln!("key item filing: {} rejected ({e})", path.display()),
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

    /// The item's description as the table wraps it: one entry per line, in
    /// block order. `None` when neither of the entry's two block slots held a
    /// printable line.
    pub fn description_lines(&self, key_item_id: u16) -> Option<&[String]> {
        self.descriptions.get(&key_item_id).map(Vec::as_slice)
    }

    /// How retail files its key-item menu pages, read from the same table as the
    /// text; empty when no gen-2 file was readable.
    pub fn filing(&self) -> &KeyItemFiling {
        &self.filing
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

    struct SynthEntry {
        id: u32,
        name: String,
        /// The `desc` block word 3 points at, in the shape the install's own
        /// blocks were measured to hold: bookend empties and a marker run
        /// around retail's wrapped lines.
        desc: Vec<String>,
    }

    struct SynthSection {
        entries: Vec<SynthEntry>,
    }

    impl SynthSection {
        fn bytes(&self) -> Vec<u8> {
            let mut head = Vec::new();
            head.extend_from_slice(SECTION_RESOURCE_ID);
            head.extend_from_slice(&0u32.to_le_bytes());
            head.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());

            let mut text = Vec::new();
            let text_base = ENTRIES_OFFSET + self.entries.len() * ENTRY_LEN;
            for entry in &self.entries {
                let article = text_base + text.len();
                text.extend_from_slice(&encode_text("the"));
                text.push(0);
                let name_offset = text_base + text.len();
                text.extend_from_slice(&encode_text(&entry.name));
                text.push(0);
                // The probed block layout: {count, ptr*count}, an empty string
                // first and last, retail's wrapped lines between them.
                let desc_offset = if entry.desc.is_empty() {
                    0u32
                } else {
                    let block_off = text_base + text.len();
                    let slot_count = entry.desc.len() + 2;
                    text.extend_from_slice(&(slot_count as u32).to_le_bytes());
                    for _ in 0..slot_count {
                        text.extend_from_slice(&0u32.to_le_bytes());
                    }
                    let slot_base = block_off - text_base + size_of::<u32>();
                    for k in 0..slot_count {
                        let ptr = (text_base + text.len()) as u32;
                        let at = slot_base + k * size_of::<u32>();
                        text[at..at + size_of::<u32>()].copy_from_slice(&ptr.to_le_bytes());
                        if k >= 1 && k + 1 < slot_count {
                            text.extend_from_slice(&encode_text(&entry.desc[k - 1]));
                        }
                        text.push(0);
                    }
                    block_off as u32
                };
                head.extend_from_slice(&entry.id.to_le_bytes());
                head.extend_from_slice(&(article as u32).to_le_bytes());
                head.extend_from_slice(&(name_offset as u32).to_le_bytes());
                head.extend_from_slice(&desc_offset.to_le_bytes());
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
                SynthEntry {
                    id: COLUMN_HEADER_ROW_ID,
                    name: "in-text name".to_string(),
                    desc: vec![],
                },
                SynthEntry {
                    id: 1,
                    name: "Zeruhn report".to_string(),
                    // The install's own wrapping for this item, shortened.
                    desc: vec![
                        "The Galka, our main workforce,".to_string(),
                        "seem to suffer from low morale,".to_string(),
                    ],
                },
                SynthEntry {
                    id: 256,
                    name: "treasure map".to_string(),
                    desc: vec![],
                },
                SynthEntry {
                    id: 395,
                    name: "map of the Zeruhn Mines".to_string(),
                    desc: vec!["The key to the mines?".to_string()],
                },
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
    fn description_blocks_decode_in_order_with_their_bookends_dropped() {
        let (_, descriptions) = parse_key_item_entries(&well_formed().dat()).unwrap();
        assert_eq!(
            descriptions.get(&1).map(Vec::as_slice),
            Some(
                &[
                    "The Galka, our main workforce,".to_string(),
                    "seem to suffer from low morale,".to_string()
                ][..]
            )
        );
        assert_eq!(
            descriptions.get(&395).map(Vec::as_slice),
            Some(&["The key to the mines?".to_string()][..])
        );
        // Entries whose word 3 is null carry no block, not an empty one.
        assert!(!descriptions.contains_key(&256));
    }

    /// The install-gated law: the `{count, ptr}` blocks behind entry words 3/4
    /// are retail's own examine text - `Shadow Lord's participle` (id 74) opens
    /// with "A small part of the / Shadow Lord's body.", and the Zeruhn report
    /// (id 1) ends on its signed line. Probed live; if a DAT rev moves these,
    /// the block law changed, not this test.
    #[test]
    fn installed_table_carries_the_real_examine_text() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let table = KeyItemTable::open_from_root(&root);
        let participle = table
            .description_lines(74)
            .expect("Shadow Lord's participle carries examine text");
        assert!(
            participle.iter().any(|line| line.contains("Shadow Lord")),
            "{participle:?}"
        );
        let report = table
            .description_lines(1)
            .expect("Zeruhn report carries examine text");
        assert!(
            report
                .last()
                .is_some_and(|line| line.contains("Overseer Makarim")),
            "{report:?}"
        );
        // A marker-only block (airship passes) yields no printable lines rather
        // than control-char garbage.
        for id in [8, 9] {
            if let Some(lines) = table.description_lines(id) {
                assert!(
                    lines.iter().all(|line| !line.trim().is_empty()),
                    "id {id}: {lines:?}"
                );
            }
        }
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

    // --- gen-2 (server-era) table: synthetic law tests + install fence ---

    const SYNTH_EXTENDED_TABLE_OFFSET: usize = 0x100;
    /// The stride measured on ROM/175/35.DAT, reused so the synthetic file
    /// exercises the same geometry the install has.
    const SYNTH_EXTENDED_BLOCK_SIZE: usize = 0x2bc;

    struct SynthExtendedBlock {
        id: u32,
        name: Option<String>,
        plural: Option<String>,
        desc: Option<String>,
    }

    /// One plaintext (pre-XOR) gen-2 block laid out exactly like the install's:
    /// count=7, descriptor pairs from +4, pair 0 -> raw u32 id at a fixed slot,
    /// pairs 1..6 -> string blobs {marker, 0x18 zeros, NUL text}.
    fn synth_extended_block(b: &SynthExtendedBlock) -> Vec<u8> {
        let mut body = vec![0u8; SYNTH_EXTENDED_BLOCK_SIZE];
        let put =
            |body: &mut [u8], at: usize, v: u32| body[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut body, 0, EXTENDED_PAIRS as u32);
        // Pair 0 -> id word (raw u32, no marker); pair 1 -> the measured
        // companion word (value 1), whose meaning nothing on either side uses.
        put(&mut body, 4, 0x3C);
        put(&mut body, 8, 1);
        put(&mut body, 0xC, 0x40);
        put(&mut body, 0x10, 1);
        put(&mut body, 0x3C, b.id);
        put(&mut body, 0x40, 1);
        let mut cursor = 0x44;
        for (pair_word, text) in [
            (0x14usize, Some("")),
            (0x1C, Some("")),
            (0x24, b.name.as_deref()),
            (0x2C, b.plural.as_deref()),
            (0x34, b.desc.as_deref().or(Some(""))),
        ] {
            let text = text.unwrap_or("");
            put(&mut body, pair_word, cursor as u32);
            // A blob with no text still occupies marker+metadata+NUL; guard
            // the stride so an oversized fixture fails loudly, not silently.
            assert!(cursor + 4 + 0x18 + text.len() < SYNTH_EXTENDED_BLOCK_SIZE);
            put(&mut body, cursor, 1);
            if !text.is_empty() {
                let at = cursor + 4 + 0x18;
                body[at..at + text.len()].copy_from_slice(text.as_bytes());
            }
            cursor += 4 + 0x18 + text.len() + 1;
        }
        body
    }

    /// The whole gen-2 file: d_msg header (fixed-block shape), blocks XOR-0xFF'd
    /// from `tableOffset` - identical container law to [`crate::dmsg`].
    fn synth_extended_file(
        blocks: &[SynthExtendedBlock],
        block_size: usize,
        table_size_field: u32,
    ) -> Vec<u8> {
        let mut file = vec![0u8; SYNTH_EXTENDED_TABLE_OFFSET];
        file[..5].copy_from_slice(b"d_msg");
        let words: [u32; 6] = [
            (SYNTH_EXTENDED_TABLE_OFFSET + block_size * blocks.len()) as u32,
            SYNTH_EXTENDED_TABLE_OFFSET as u32,
            table_size_field,
            block_size as u32,
            0,
            blocks.len() as u32,
        ];
        // dmsg.rs keeps its header-word offset private; 0x14 is the value that
        // every real container (and `decode_common`) puts them at.
        const HEADER_WORDS: usize = 0x14;
        for (i, w) in words.iter().enumerate() {
            file[HEADER_WORDS + i * 4..][..4].copy_from_slice(&w.to_le_bytes());
        }
        let mut region = Vec::new();
        for b in blocks {
            let mut plain = synth_extended_block(b);
            plain.resize(block_size, 0);
            region.extend_from_slice(&plain);
        }
        file.extend(region.iter().map(|byte| byte ^ 0xFF));
        file
    }

    #[test]
    fn gen2_table_keys_entries_by_their_own_id_word() {
        // Ids deliberately out of block order: lookup must follow the id word,
        // never the position (the CoP mission lesson, same family law).
        let dat = synth_extended_file(
            &[
                SynthExtendedBlock {
                    id: 0,
                    name: Some("in-text name".to_string()),
                    plural: None,
                    desc: None,
                },
                SynthExtendedBlock {
                    id: 500,
                    name: Some("job gesture: warrior".to_string()),
                    plural: Some("job gestures: warrior".to_string()),
                    desc: Some("\nStrike a pose befitting a mighty\nwarrior.\n".to_string()),
                },
                SynthExtendedBlock {
                    id: 3,
                    name: Some("third thing".to_string()),
                    plural: None,
                    desc: None,
                },
            ],
            SYNTH_EXTENDED_BLOCK_SIZE,
            0,
        );
        let (names, descriptions) = parse_extended_key_item_entries(&dat).unwrap();
        assert_eq!(
            names.get(&500).map(String::as_str),
            Some("job gesture: warrior")
        );
        assert_eq!(names.get(&3).map(String::as_str), Some("third thing"));
        // The id-0 column header row is not a key item.
        assert!(!names.contains_key(&0));
        // Retail's '\n' wrapping became lines; the empty bookend runs dropped.
        assert_eq!(
            descriptions.get(&500).map(Vec::as_slice),
            Some(
                &[
                    "Strike a pose befitting a mighty".to_string(),
                    "warrior.".to_string()
                ][..]
            )
        );
    }

    #[test]
    fn gen2_block_with_wrong_pair_count_refuses_the_table() {
        let mut dat = synth_extended_file(
            &[
                SynthExtendedBlock {
                    id: 1,
                    name: Some("one".to_string()),
                    plural: None,
                    desc: None,
                },
                SynthExtendedBlock {
                    id: 2,
                    name: Some("two".to_string()),
                    plural: None,
                    desc: None,
                },
            ],
            SYNTH_EXTENDED_BLOCK_SIZE,
            0,
        );
        // Un-XOR block 1's count word, rewrite it to 6, re-hide it.
        let at = SYNTH_EXTENDED_TABLE_OFFSET + SYNTH_EXTENDED_BLOCK_SIZE;
        for k in 0..4 {
            dat[at + k] ^= 0xFF;
        }
        dat[at..at + 4].copy_from_slice(&6u32.to_le_bytes());
        for k in 0..4 {
            dat[at + k] ^= 0xFF;
        }
        assert_eq!(
            parse_extended_key_item_entries(&dat),
            Err(KeyItemTableError::ExtendedBlockShape { block: 1, found: 6 })
        );
    }

    /// Retail's own shipping file carries three duplicate-id rows (the Mog
    /// Kupon coupons); the law is first-row-wins, not refusal.
    #[test]
    fn gen2_duplicate_id_rows_keep_the_first() {
        let dat = synth_extended_file(
            &[
                SynthExtendedBlock {
                    id: 7,
                    name: Some("seven".to_string()),
                    plural: None,
                    desc: Some("first description".to_string()),
                },
                SynthExtendedBlock {
                    id: 7,
                    name: Some("seven again".to_string()),
                    plural: None,
                    desc: Some("second description".to_string()),
                },
            ],
            SYNTH_EXTENDED_BLOCK_SIZE,
            0,
        );
        let (names, descriptions) = parse_extended_key_item_entries(&dat).unwrap();
        assert_eq!(names.get(&7).map(String::as_str), Some("seven"));
        assert_eq!(
            descriptions.get(&7).map(Vec::as_slice),
            Some(&["first description".to_string()][..])
        );
    }

    #[test]
    fn gen2_reader_demands_the_fixed_block_layout() {
        let dat = synth_extended_file(
            &[SynthExtendedBlock {
                id: 1,
                name: Some("one".to_string()),
                plural: None,
                desc: None,
            }],
            SYNTH_EXTENDED_BLOCK_SIZE,
            8,
        );
        assert!(matches!(
            parse_extended_key_item_entries(&dat),
            Err(KeyItemTableError::Extended(_))
        ));
    }

    /// The merge law without any install: gen-2 wording wins for ids it covers
    /// (names always, description blocks only when it has printable lines);
    /// era-only ids keep both.
    #[test]
    fn gen2_text_overlays_the_era_rows() {
        let dir = tempfile::tempdir().unwrap();
        let era = dir.path().join(KEY_ITEM_TEXT_ERA_ROM_PATH);
        std::fs::create_dir_all(era.parent().unwrap()).unwrap();
        std::fs::write(&era, well_formed().dat()).unwrap();
        let ext = dir.path().join(KEY_ITEM_EXTENDED_ERA_ROM_PATH);
        std::fs::create_dir_all(ext.parent().unwrap()).unwrap();
        std::fs::write(
            &ext,
            synth_extended_file(
                &[
                    SynthExtendedBlock {
                        id: 1,
                        name: Some("the Zeruhn report".to_string()),
                        plural: None,
                        desc: Some("fresh gen-2 line one\nline two".to_string()),
                    },
                    SynthExtendedBlock {
                        id: 256,
                        name: Some("gen-2 treasure map".to_string()),
                        plural: None,
                        desc: Some(String::new()),
                    },
                ],
                SYNTH_EXTENDED_BLOCK_SIZE,
                0,
            ),
        )
        .unwrap();

        let table = KeyItemTable::open(dir.path());
        assert_eq!(table.lookup(1), Some("the Zeruhn report"));
        assert_eq!(
            table.description_lines(1),
            Some(&["fresh gen-2 line one".to_string(), "line two".to_string()][..])
        );
        // Name from gen-2, description absent there so the era block stands.
        assert_eq!(table.lookup(256), Some("gen-2 treasure map"));
        assert_eq!(table.description_lines(256), None);
        // Era-only id survives untouched (well_formed has no desc for 256; 1's
        // era text proved the overwrite above).
    }

    /// Install-gated law: the server-era ids `job gesture: warrior` (LSB
    /// `xi.ki.JOB_GESTURE_WARRIOR`) and its siblings exist only in the gen-2
    /// file, keyed by their own id words. Text quoted verbatim from both
    /// copies on this install; block numbers differ between them.
    #[test]
    fn installed_extended_table_names_the_server_era_gestures() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        let table = KeyItemTable::open_from_root(&root);
        assert_eq!(
            table.lookup(1738),
            Some("job gesture: warrior"),
            "LSB id 1738 lives at gen-2 block 1811"
        );
        assert!(table.lookup(2963).is_some(), "geomancer gesture");
        let rune = table
            .description_lines(2964)
            .expect("rune fencer gesture carries retail text");
        assert_eq!(
            rune,
            ["Strike a pose befitting an attuned rune fencer.".to_string()]
        );
        // Shared ids take the gen-2 wording; this one matches the era file's
        // (same retail text, newer container).
        assert_eq!(table.lookup(1), Some("Zeruhn report"));
    }

    /// The page law without any install: a labelled row files every keyed row
    /// above it since the previous label, so page order follows the table.
    #[test]
    fn filing_pages_end_at_their_label_row() {
        let block = |id: u32, name: &str| SynthExtendedBlock {
            id,
            name: Some(name.to_string()),
            plural: None,
            desc: None,
        };
        let dat = synth_extended_file(
            &[
                block(0, "in-text name"),
                block(5, "first item"),
                block(7, "second item"),
                block(0, "-Alpha Page"),
                block(9, "third item"),
                block(0, "-Beta Page"),
            ],
            SYNTH_EXTENDED_BLOCK_SIZE,
            0,
        );
        let filing = parse_key_item_filing(&dat).unwrap();
        assert_eq!(
            filing.labels(),
            &["Alpha Page".to_string(), "Beta Page".to_string()]
        );
        assert_eq!((filing.page_of(5), filing.page_of(7)), (Some(0), Some(0)));
        assert_eq!(filing.page_of(9), Some(1));
        assert_eq!(filing.page_of(4094), None);
        // The label rows are not items, so the text reader keeps skipping them.
        let (names, _) = parse_extended_key_item_entries(&dat).unwrap();
        assert!(!names.contains_key(&0));
    }

    /// Install-gated law: retail's own page labels and filing, quoted from this
    /// install, plus the sample ids that show each boundary (`airship pass` and
    /// `job gesture: warrior` really do sit on the Permanent page, where retail's
    /// window files them). Both copies of the table - the game tree's own and any
    /// Pivot overlay winner over it - must agree.
    #[test]
    fn installed_filing_files_pages_like_retail() {
        let Some(root) = crate::archive::open_test_install() else {
            return;
        };
        const MEASURED_PAGE_LABELS: [&str; 10] = [
            "Temporary Key Items",
            "Permanent Key Items",
            "Abyssea",
            "Voidwatch",
            "Geas Fete",
            "Mounts",
            "Mog Garden",
            "Magical Maps",
            "Claim Slips",
            "Active Effects",
        ];
        let table = KeyItemTable::open_from_root(&root);
        let filing = table.filing();
        assert_eq!(
            filing.labels(),
            MEASURED_PAGE_LABELS
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .as_slice()
        );
        for (id, page) in [
            (1u16, 0u8), // Zeruhn report
            (8, 1),      // airship pass
            (353, 1),    // Dem gate crystal
            (1738, 1),   // job gesture: warrior
            (1699, 2),   // Atma of the Savior
            (1539, 3),   // voidstone
            (2839, 4),   // reaper
            (3055, 5),   // trainer's whistle
            (2398, 6),   // GPS crystal
            (2307, 7),   // map of Escha - Zi'Tah
            (1920, 8),   // iron chainmail claim slip
        ] {
            assert_eq!(
                filing.page_of(id),
                Some(page),
                "{id} ({}) is filed on page {page} ({:?})",
                table.lookup(id).unwrap_or("<unnamed>"),
                filing.labels().get(page as usize).map(String::as_str)
            );
        }

        // The era `sc_item_` section adds nothing that needs filing separately:
        // every era-named id also has a row (and so a page) in the gen-2 table.
        let era_path = root
            .resolve(KEY_ITEM_TEXT_FILE_ID)
            .map(|loc| loc.path_under(&root))
            .unwrap_or_else(|_| root.root().join(KEY_ITEM_TEXT_ERA_ROM_PATH));
        if let Ok(bytes) = std::fs::read(&era_path) {
            let (era_names, _) = parse_key_item_entries(&bytes).unwrap();
            let unfiled: Vec<u16> = era_names
                .keys()
                .copied()
                .filter(|&id| u32::from(id) != COLUMN_HEADER_ROW_ID && filing.page_of(id).is_none())
                .collect();
            assert!(
                unfiled.is_empty(),
                "era ids the gen-2 table does not file: {unfiled:?}"
            );
        }

        // The game tree's own copy of the table labels the pages the same and
        // files every id on the same page but the two pinned below.
        let tree = root.root().join(KEY_ITEM_EXTENDED_ERA_ROM_PATH);
        if tree.is_file() {
            let other = parse_key_item_filing(&std::fs::read(&tree).unwrap()).unwrap();
            assert_eq!(filing.labels(), other.labels());
            eprintln!(
                "copies file {} and {} ids",
                filing.ids().count(),
                other.ids().count()
            );
            let mut disagreements: Vec<u16> = filing
                .ids()
                .filter(|&id| other.page_of(id) != filing.page_of(id))
                .collect();
            disagreements.sort_unstable();
            let page_name = |f: &KeyItemFiling, id: u16| {
                f.page_of(id)
                    .and_then(|p| f.labels().get(p as usize).cloned())
                    .unwrap_or_else(|| "<none>".to_string())
            };
            // Measured on this install: retail's own two copies of the table
            // disagree about exactly two ids - `White Card` (349) and `Black Card`
            // (351) sit on the Temporary page in Pivot's overlay winner, which is
            // what the client renders, and on the Permanent page in the game
            // tree's older copy. Kuluu files them as the resolved winner does, like
            // every other DAT read, so the divergence is recorded here rather than
            // smoothed away.
            assert_eq!(
                disagreements,
                [349u16, 351],
                "copies disagree via {:?}",
                disagreements
                    .iter()
                    .map(|id| {
                        (
                            *id,
                            table.lookup(*id).unwrap_or("<unnamed>"),
                            page_name(filing, *id),
                            page_name(&other, *id),
                        )
                    })
                    .collect::<Vec<_>>()
            );
        }
    }
}
