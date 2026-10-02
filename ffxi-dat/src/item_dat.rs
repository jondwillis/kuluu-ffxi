use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::client_profile::ItemBlockLayout;
use crate::map_image::{self, GraphicImage};

// vendor/POLUtils/PlayOnline.FFXI/Things/Item.cs Item: per-type item arrays keyed by the
// first block's id — general from 0x0000, usable from 0x1000, weapons from 0x4000, armor
// from 0x2800, puppet from 0x2000, vouchers and slips from 0x7000, monipulator from 0xF000,
// instincts from 0x7400.
pub const ITEM_DAT_GENERAL: u32 = 73;
pub const ITEM_DAT_USABLE: u32 = 74;
pub const ITEM_DAT_WEAPON: u32 = 75;
pub const ITEM_DAT_ARMOR: u32 = 76;
pub const ITEM_DAT_PUPPET: u32 = 77;
/// Currency.
pub const ITEM_DAT_CURRENCY: u32 = 91;
/// Vouchers and slips (maze tabulae and runes, storage slips, legion passes, grimoires).
pub const ITEM_DAT_VOUCHERS_AND_SLIPS: u32 = 55667;
/// Expansion armor.
pub const ITEM_DAT_ARMOR_EXPANSION: u32 = 55668;
pub const ITEM_DAT_MONIPULATOR: u32 = 55669;
pub const ITEM_DAT_INSTINCT: u32 = 55670;
/// Expansion items.
pub const ITEM_DAT_ITEMS_EXPANSION: u32 = 55671;

// vendor/POLUtils/PlayOnline.FFXI.Utils.DataBrowser/ROMFileMappings.xml Menu:ItemData / Menu:English.
pub const ITEM_DAT_FILE_IDS: [u32; 11] = [
    ITEM_DAT_GENERAL,
    ITEM_DAT_USABLE,
    ITEM_DAT_WEAPON,
    ITEM_DAT_ARMOR,
    ITEM_DAT_PUPPET,
    ITEM_DAT_CURRENCY,
    ITEM_DAT_VOUCHERS_AND_SLIPS,
    ITEM_DAT_ARMOR_EXPANSION,
    ITEM_DAT_MONIPULATOR,
    ITEM_DAT_INSTINCT,
    ITEM_DAT_ITEMS_EXPANSION,
];

/// [`ITEM_DAT_GENERAL`]'s era ROM path, named apart from the rest because
/// [`crate::client_profile::ClientProfile::probe`] reads the item block layout
/// from this file before any table is loaded.
pub(crate) const ITEM_DAT_GENERAL_ERA_ROM_PATH: &str = "ROM/118/106.DAT";

/// Where each item DAT id resolved on the horizonxi-2023 and retail-2026-09
/// [`crate::client_profile::KNOWN_CLIENTS`] rows. Reads go by file id through
/// the install's own VTABLE/FTABLE; this is the fallback for a root whose
/// tables cannot be loaded, and `ffxi-dat/tests/fixed_dat_ids.rs` fails when an
/// install moves one.
pub(crate) fn era_rom_path(file_id: u32) -> Option<&'static str> {
    Some(match file_id {
        ITEM_DAT_GENERAL => ITEM_DAT_GENERAL_ERA_ROM_PATH,
        ITEM_DAT_USABLE => "ROM/118/107.DAT",
        ITEM_DAT_WEAPON => "ROM/118/108.DAT",
        ITEM_DAT_ARMOR => "ROM/118/109.DAT",
        ITEM_DAT_PUPPET => "ROM/118/110.DAT",
        ITEM_DAT_CURRENCY => "ROM/174/48.DAT",
        ITEM_DAT_VOUCHERS_AND_SLIPS => "ROM/217/21.DAT",
        ITEM_DAT_ARMOR_EXPANSION => "ROM/286/73.DAT",
        ITEM_DAT_MONIPULATOR => "ROM/288/67.DAT",
        ITEM_DAT_INSTINCT => "ROM/288/80.DAT",
        ITEM_DAT_ITEMS_EXPANSION => "ROM/301/115.DAT",
        _ => return None,
    })
}

/// Icon offset; identical on both [`ItemBlockLayout`]s, the `Retail2026` pad follows it.
pub const ITEM_ICON_OFFSET: usize = 0x280;

const ITEM_BLOCK_SHIFT: u32 = crate::client_profile::ITEM_BYTE_SHIFT;

const ITEM_FLAGS_OFFSET: usize = 0x04;

/// `Legacy` offset of the header run stack/type/resource/targets; every
/// layout places it at this plus `ItemBlockLayout::header_shift`.
const ITEM_STACK_OFFSET: usize = 0x06;

const ITEM_TYPE_OFFSET: usize = ITEM_STACK_OFFSET + 2;

/// `Legacy` offset of the equipment tail (level/slots/races/jobs...); every
/// layout places it at this plus `ItemBlockLayout::header_shift`.
const ITEM_EQUIPMENT_TAIL_OFFSET: usize = 0x0E;

/// Where the string-table probe starts: the first offset past the common
/// header on either layout.
const STRING_TABLE_PROBE_START: usize = 0x10;

const STRING_TABLE_MAX_ENTRIES: u32 = 9;

const STRING_TABLE_META_LEN: usize = 8;

const STRING_TABLE_COUNT_LEN: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum ItemDatError {
    #[error("{path}: block layout not detected (no known stride ends the first block on the trailer byte)")]
    LayoutUndetected { path: PathBuf },

    #[error("{path}: {len} bytes is not a multiple of the {layout} stride {stride:#x}")]
    UnalignedLength {
        path: PathBuf,
        len: usize,
        layout: &'static str,
        stride: usize,
    },

    #[error("{path}: first block id unreadable")]
    HeaderUnreadable { path: PathBuf },
}

pub const ITEM_FLAG_RARE: u16 = 0x8000;

pub const ITEM_FLAG_EX: u16 = 0x4000;

#[derive(Debug, Clone)]
pub struct ItemStatic {
    pub name: String,

    /// Retail's chat-log name ("fire crystal", "sprig of chamomile") — its own
    /// string in the DAT, not a case-fold of `name`. Falls back to `name` when
    /// the block's string table carries no log entries.
    pub log_name: String,

    pub log_name_plural: String,

    pub description: String,

    pub slot_mask: u16,

    pub jobs_mask: u32,

    pub races_mask: u16,

    pub level: u8,

    pub flags: u16,

    pub max_charges: u8,

    pub recast_base: u32,

    pub item_type: u8,

    pub icon: Option<GraphicImage>,
}

impl ItemStatic {
    pub fn is_rare(&self) -> bool {
        self.flags & ITEM_FLAG_RARE != 0
    }

    pub fn is_ex(&self) -> bool {
        self.flags & ITEM_FLAG_EX != 0
    }
}

fn is_equipment(item_id: u32) -> bool {
    matches!(item_id, 0x2800..=0x6FFF)
}

/// `dat_bytes` is a whole item DAT: the layout comes from its first two
/// blocks and the base id from its first block.
pub fn lookup(dat_bytes: &[u8], item_id: u16) -> Option<ItemStatic> {
    let (layout, block) = decoded_block(dat_bytes, item_id)?;
    decode_item_static(&block, layout)
}

fn decode_item_static(block: &[u8], layout: ItemBlockLayout) -> Option<ItemStatic> {
    let stored_id = read_u32_le(block.get(0x00..0x04)?);
    let flags = read_u16_le(block.get(ITEM_FLAGS_OFFSET..ITEM_FLAGS_OFFSET + 2)?);
    let type_off = ITEM_TYPE_OFFSET + layout.header_shift();
    let item_type = read_u16_le(block.get(type_off..type_off + 2)?);

    let (slot_mask, races_mask, jobs_mask, level, max_charges, recast_base) =
        if is_equipment(stored_id) {
            let mut off = ITEM_EQUIPMENT_TAIL_OFFSET + layout.header_shift();
            let level = read_u16_le(block.get(off..off + 2)?);
            off += 2;
            let slots = read_u16_le(block.get(off..off + 2)?);
            off += 2;
            let races = read_u16_le(block.get(off..off + 2)?);
            off += 2;
            off += layout.races_gap();
            let jobs = read_u32_le(block.get(off..off + 4)?);
            off += 4;

            off += 4;

            if is_weapon(stored_id) {
                off += 6 + 1 + 1 + 4;
            }
            let max_charges = *block.get(off)?;
            off += 1;

            off += 1 + 2;
            let reuse_delay = read_u32_le(block.get(off..off + 4)?);
            (
                slots,
                races,
                jobs,
                (level.min(u8::MAX as u16)) as u8,
                max_charges,
                reuse_delay,
            )
        } else {
            (0, 0, 0, 0, 0, 0)
        };

    let strings = read_item_strings(block).unwrap_or_default();
    let icon = decode_icon(block);

    Some(ItemStatic {
        name: strings.name,
        log_name: strings.log_name,
        log_name_plural: strings.log_name_plural,
        description: strings.description,
        slot_mask,
        jobs_mask,
        races_mask,
        level,
        flags,
        max_charges,
        recast_base,
        item_type: item_type.min(u8::MAX as u16) as u8,
        icon,
    })
}

pub fn icon_at(dat_bytes: &[u8], item_id: u16) -> Option<GraphicImage> {
    let (_, block) = decoded_block(dat_bytes, item_id)?;
    decode_icon(&block)
}

struct ItemDatFile {
    path: PathBuf,
    base: u16,
    blocks: usize,
    layout: ItemBlockLayout,
}

/// The retail item database resolved across the per-type DATs. Each file is a
/// gap-free ascending array of fixed-size blocks, so a lookup is `O(1)`: pick
/// the file whose `[base, base + blocks)` covers the id, then read block
/// `id - base`. Blocks are read on demand (and decoded with the per-byte
/// rotate-right-5 obfuscation), so the table itself stays tiny.
#[derive(Default)]
pub struct ItemTable {
    files: Vec<ItemDatFile>,
    skipped: Vec<ItemDatError>,
}

impl ItemTable {
    /// Open every available item DAT under `root_dir` (the retail install root).
    /// The tables there place each [`ITEM_DAT_FILE_IDS`] entry, so a caller
    /// holding only a path still reads through the FTABLE and any Pivot
    /// overlay; a directory carrying no tables (a synthetic fixture) falls back
    /// to the ROM paths the ids were measured at. Costs a second table load,
    /// which [`ItemTable::open_from_root`] avoids.
    pub fn open(root_dir: &Path) -> ItemTable {
        match crate::DatRoot::open(root_dir) {
            Ok(root) => Self::open_from_root(&root),
            Err(_) => Self::open_paths(
                ITEM_DAT_FILE_IDS
                    .iter()
                    .filter_map(|&id| era_rom_path(id))
                    .map(|rel| root_dir.join(rel)),
            ),
        }
    }

    /// Overlay-aware: resolves every [`ITEM_DAT_FILE_IDS`] entry through the
    /// install's VTABLE/FTABLE, skipping the ids it cannot place.
    pub fn open_from_root(root: &crate::DatRoot) -> ItemTable {
        Self::open_paths(
            ITEM_DAT_FILE_IDS
                .iter()
                .filter_map(|&id| root.resolve(id).ok())
                .map(|loc| loc.path_under(root)),
        )
    }

    /// A missing file is silently absent; a present but unusable one is skipped
    /// and reported by [`ItemTable::skipped`], so a partial install still
    /// yields whatever ranges it has.
    fn open_paths(paths: impl Iterator<Item = PathBuf>) -> ItemTable {
        let mut files = Vec::new();
        let mut skipped = Vec::new();
        for path in paths {
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let len = meta.len() as usize;
            let Some(layout) = ItemBlockLayout::probe_file(&path) else {
                skipped.push(ItemDatError::LayoutUndetected { path });
                continue;
            };
            let stride = layout.stride();
            if len == 0 || !len.is_multiple_of(stride) {
                skipped.push(ItemDatError::UnalignedLength {
                    path,
                    len,
                    layout: layout.name(),
                    stride,
                });
                continue;
            }
            let Some(base) = block_id_at(&path, 0) else {
                skipped.push(ItemDatError::HeaderUnreadable { path });
                continue;
            };
            let blocks = if block_id_at(&path, stride) == Some(0) {
                1
            } else {
                len / stride
            };
            files.push(ItemDatFile {
                path,
                base: base as u16,
                blocks,
                layout,
            });
        }
        ItemTable { files, skipped }
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Present item DATs that could not be opened, in [`ITEM_DAT_FILE_IDS`]
    /// order.
    pub fn skipped(&self) -> &[ItemDatError] {
        &self.skipped
    }

    /// Layouts present across the opened files, deduplicated in file order. A
    /// healthy install has exactly one.
    pub fn layouts(&self) -> Vec<ItemBlockLayout> {
        let mut out: Vec<ItemBlockLayout> = Vec::new();
        for f in &self.files {
            if !out.contains(&f.layout) {
                out.push(f.layout);
            }
        }
        out
    }

    fn block(&self, item_id: u16) -> Option<(ItemBlockLayout, Vec<u8>)> {
        let file = self
            .files
            .iter()
            .find(|f| item_id >= f.base && ((item_id - f.base) as usize) < f.blocks)?;
        let stride = file.layout.stride();
        let offset = (item_id - file.base) as usize * stride;
        let mut block = read_at(&file.path, offset, stride)?;
        decode_bytes(&mut block);
        (read_u32_le(block.get(0x00..0x04)?) as u16 == item_id).then_some((file.layout, block))
    }

    pub fn lookup(&self, item_id: u16) -> Option<ItemStatic> {
        let (layout, block) = self.block(item_id)?;
        decode_item_static(&block, layout)
    }

    pub fn name(&self, item_id: u16) -> Option<String> {
        read_item_strings(&self.block(item_id)?.1).map(|strings| strings.name)
    }

    pub fn icon(&self, item_id: u16) -> Option<GraphicImage> {
        decode_icon(&self.block(item_id)?.1)
    }
}

fn read_at(path: &Path, offset: usize, len: usize) -> Option<Vec<u8>> {
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(offset as u64)).ok()?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn block_id_at(path: &Path, offset: usize) -> Option<u32> {
    let mut head = read_at(path, offset, 4)?;
    decode_bytes(&mut head);
    Some(read_u32_le(&head))
}

fn decode_bytes(bytes: &mut [u8]) {
    for b in bytes.iter_mut() {
        *b = rotate_byte_right(*b, ITEM_BLOCK_SHIFT);
    }
}

fn is_weapon(item_id: u32) -> bool {
    matches!(item_id, 0x4000..=0x59FF)
}

fn decoded_block(dat_bytes: &[u8], item_id: u16) -> Option<(ItemBlockLayout, Vec<u8>)> {
    let layout = ItemBlockLayout::detect(dat_bytes)?;
    let stride = layout.stride();
    let mut head = dat_bytes.get(0..4)?.to_vec();
    decode_bytes(&mut head);
    let base = read_u32_le(&head);
    let index = (item_id as u32).checked_sub(base)? as usize;
    let start = index.checked_mul(stride)?;
    let end = start.checked_add(stride)?;
    let mut block = dat_bytes.get(start..end)?.to_vec();
    decode_bytes(&mut block);
    (read_u32_le(block.get(0x00..0x04)?) == item_id as u32).then_some((layout, block))
}

fn decode_icon(block: &[u8]) -> Option<GraphicImage> {
    let size = read_u32_le(block.get(ITEM_ICON_OFFSET..ITEM_ICON_OFFSET + 4)?) as usize;
    if size == 0 {
        return None;
    }
    let start = ITEM_ICON_OFFSET + 4;
    let end = start.checked_add(size)?;
    let chunk = block.get(start..end.min(block.len()))?;
    map_image::parse_graphic_icon(chunk)
        .ok()
        .flatten()
        .map(|(img, _)| img)
}

#[derive(Debug, Default)]
struct ItemStrings {
    name: String,
    log_name: String,
    log_name_plural: String,
    description: String,
}

// The count==5 string table is (0=display name, 1=numeric kind flag,
// 2=log name singular, 3=log name plural, 4=description) — POLUtils Item.cs's
// LogNameSingular/LogNamePlural layout, verified against this install
// (id 636 "Chamomile" logs as "sprig of chamomile", a distinct string).
const STRING_TABLE_LOG_NAME: usize = 2;
const STRING_TABLE_LOG_NAME_PLURAL: usize = 3;
const STRING_TABLE_DESCRIPTION: usize = 4;

/// The table's first descriptor points just past the descriptors, so a
/// candidate count word is accepted only when the word after it agrees.
fn string_table_body_offset(count: usize) -> usize {
    STRING_TABLE_COUNT_LEN + count * STRING_TABLE_META_LEN
}

fn read_item_strings(block: &[u8]) -> Option<ItemStrings> {
    let table_region_end = ITEM_ICON_OFFSET;

    let mut probe = STRING_TABLE_PROBE_START;
    while probe + STRING_TABLE_COUNT_LEN <= table_region_end {
        let count = read_u32_le(block.get(probe..probe + 4)?);
        if (1..=STRING_TABLE_MAX_ENTRIES).contains(&count) {
            let count = count as usize;
            let first_meta = probe + STRING_TABLE_COUNT_LEN;
            let first_rel = read_u32_le(block.get(first_meta..first_meta + 4)?) as usize;
            let body_start = probe + string_table_body_offset(count);
            if first_rel == string_table_body_offset(count) && body_start <= table_region_end {
                if let Some(parsed) = parse_string_table(block, probe, count) {
                    return Some(parsed);
                }
            }
        }
        probe += STRING_TABLE_COUNT_LEN;
    }
    None
}

fn parse_string_table(block: &[u8], table_off: usize, count: usize) -> Option<ItemStrings> {
    let mut metas = Vec::with_capacity(count);
    for i in 0..count {
        let m = table_off + STRING_TABLE_COUNT_LEN + i * STRING_TABLE_META_LEN;
        let rel_off = read_u32_le(block.get(m..m + 4)?) as usize;
        let kind = read_u32_le(block.get(m + 4..m + 8)?);
        metas.push((rel_off, kind));
    }

    let read_at = |rel_off: usize| -> Option<String> {
        let abs = table_off + rel_off;
        read_inline_string(block, abs)
    };

    let name = read_at(metas.first()?.0)?;
    let (log_name, log_name_plural, description) = match count {
        5 => (
            read_at(metas[STRING_TABLE_LOG_NAME].0).unwrap_or_default(),
            read_at(metas[STRING_TABLE_LOG_NAME_PLURAL].0).unwrap_or_default(),
            read_at(metas[STRING_TABLE_DESCRIPTION].0).unwrap_or_default(),
        ),
        2 => (
            String::new(),
            String::new(),
            read_at(metas[1].0).unwrap_or_default(),
        ),
        _ => Default::default(),
    };

    if name.is_empty() {
        return None;
    }
    let fallback = |s: String| if s.is_empty() { name.clone() } else { s };
    Some(ItemStrings {
        log_name: fallback(log_name),
        log_name_plural: fallback(log_name_plural),
        name,
        description,
    })
}

fn read_inline_string(block: &[u8], at: usize) -> Option<String> {
    if read_u32_le(block.get(at..at + 4)?) != 1 {
        return None;
    }
    for i in 1..=6 {
        if read_u32_le(block.get(at + i * 4..at + i * 4 + 4)?) != 0 {
            return None;
        }
    }
    let text_start = at + 7 * 4;
    let mut end = text_start;
    while end < block.len() && block[end] != 0 {
        end += 1;
    }
    Some(decode_text(&block[text_start..end]))
}

fn decode_text(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

#[inline]
fn rotate_byte_right(b: u8, shift: u32) -> u8 {
    b.rotate_right(shift)
}

#[inline]
fn read_u32_le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

#[inline]
fn read_u16_le(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}
