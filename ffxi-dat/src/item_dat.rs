use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::map_image::{self, GraphicImage};

// Retail packs item data into per-type DATs, each a gap-free ascending array of
// 0xC00 blocks keyed by item id. Paths and split match XIM's InventoryItems
// (research/xim/.../InventoryItemParser.kt:262-270), itself a port of Windower
// POLUtils Item.cs. Block index within a file is `item_id - base_id`, where
// base_id is the id stored in the file's first block.
pub const ITEM_DAT_ROM_PATHS: &[&str] = &[
    "ROM/118/106.DAT", // general items     0x0000..
    "ROM/118/107.DAT", // usable items      0x1000..
    "ROM/118/108.DAT", // weapons           0x4000..
    "ROM/118/109.DAT", // armor             0x2800..
    "ROM/174/48.DAT",  // currency
    "ROM/286/73.DAT",  // armor (expansions)
    "ROM/301/115.DAT", // items (expansions)
];

pub const ITEM_BLOCK_STRIDE: usize = 0xC00;

pub const ITEM_ICON_OFFSET: usize = 0x280;

const ITEM_BLOCK_SHIFT: u32 = 5;

pub const ITEM_FLAG_RARE: u16 = 0x8000;

pub const ITEM_FLAG_EX: u16 = 0x4000;

#[derive(Debug, Clone)]
pub struct ItemStatic {
    pub name: String,

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

pub fn lookup(dat_bytes: &[u8], item_id: u16) -> Option<ItemStatic> {
    let block = decoded_block(dat_bytes, item_id)?;
    decode_item_static(&block)
}

fn decode_item_static(block: &[u8]) -> Option<ItemStatic> {
    let stored_id = read_u32_le(block.get(0x00..0x04)?);
    let flags = read_u16_le(block.get(0x04..0x06)?);
    let item_type = read_u16_le(block.get(0x08..0x0A)?);

    let (slot_mask, races_mask, jobs_mask, level, max_charges, recast_base) =
        if is_equipment(stored_id) {
            let mut off = 0x0E;
            let level = read_u16_le(block.get(off..off + 2)?);
            off += 2;
            let slots = read_u16_le(block.get(off..off + 2)?);
            off += 2;
            let races = read_u16_le(block.get(off..off + 2)?);
            off += 2;
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

    let (name, description) = read_item_strings(block).unwrap_or_default();
    let icon = decode_icon(block);

    Some(ItemStatic {
        name,
        description,
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
    let block = decoded_block(dat_bytes, item_id)?;
    decode_icon(&block)
}

struct ItemDatFile {
    path: PathBuf,
    base: u16,
    blocks: usize,
}

/// The retail item database resolved across the per-type DATs. Each file is a
/// gap-free ascending array of 0xC00 blocks, so a lookup is `O(1)`: pick the
/// file whose `[base, base + blocks)` covers the id, then read block
/// `id - base`. Blocks are read on demand (and decoded with the per-byte
/// rotate-right-5 obfuscation), so the table itself stays tiny.
pub struct ItemTable {
    files: Vec<ItemDatFile>,
}

impl ItemTable {
    /// Open every available item DAT under `root_dir` (the retail install root).
    /// Missing or malformed files are skipped, so a partial install still yields
    /// whatever ranges it has.
    pub fn open(root_dir: &Path) -> ItemTable {
        let mut files = Vec::new();
        for rel in ITEM_DAT_ROM_PATHS {
            let path = root_dir.join(rel);
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let len = meta.len() as usize;
            if len == 0 || !len.is_multiple_of(ITEM_BLOCK_STRIDE) {
                continue;
            }
            let Some(base) = read_block_id(&path, 0) else {
                continue;
            };
            files.push(ItemDatFile {
                path,
                base,
                blocks: len / ITEM_BLOCK_STRIDE,
            });
        }
        ItemTable { files }
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    fn block(&self, item_id: u16) -> Option<Vec<u8>> {
        let file = self
            .files
            .iter()
            .find(|f| item_id >= f.base && ((item_id - f.base) as usize) < f.blocks)?;
        let offset = (item_id - file.base) as usize * ITEM_BLOCK_STRIDE;
        let mut block = read_at(&file.path, offset, ITEM_BLOCK_STRIDE)?;
        for b in block.iter_mut() {
            *b = rotate_byte_right(*b, ITEM_BLOCK_SHIFT);
        }
        (read_u32_le(block.get(0x00..0x04)?) as u16 == item_id).then_some(block)
    }

    pub fn lookup(&self, item_id: u16) -> Option<ItemStatic> {
        decode_item_static(&self.block(item_id)?)
    }

    pub fn icon(&self, item_id: u16) -> Option<GraphicImage> {
        decode_icon(&self.block(item_id)?)
    }
}

fn read_at(path: &Path, offset: usize, len: usize) -> Option<Vec<u8>> {
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(offset as u64)).ok()?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn read_block_id(path: &Path, block_index: usize) -> Option<u16> {
    let mut head = read_at(path, block_index * ITEM_BLOCK_STRIDE, 4)?;
    for b in head.iter_mut() {
        *b = rotate_byte_right(*b, ITEM_BLOCK_SHIFT);
    }
    Some(read_u32_le(&head) as u16)
}

fn is_weapon(item_id: u32) -> bool {
    matches!(item_id, 0x4000..=0x59FF)
}

fn decoded_block(dat_bytes: &[u8], item_id: u16) -> Option<Vec<u8>> {
    let id = item_id as usize;
    let start = id.checked_mul(ITEM_BLOCK_STRIDE)?;
    let end = start.checked_add(ITEM_BLOCK_STRIDE)?;
    let mut block = dat_bytes.get(start..end)?.to_vec();
    for b in block.iter_mut() {
        *b = rotate_byte_right(*b, ITEM_BLOCK_SHIFT);
    }
    Some(block)
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

fn read_item_strings(block: &[u8]) -> Option<(String, String)> {
    let table_region_end = ITEM_ICON_OFFSET;

    let mut probe = 0x10;
    while probe + 4 <= table_region_end {
        let count = read_u32_le(block.get(probe..probe + 4)?);
        if (1..=9).contains(&count) {
            let metas_start = probe + 4;
            let body_start = metas_start + (count as usize) * 8;
            if body_start <= table_region_end {
                if let Some(parsed) = parse_string_table(block, probe, count as usize) {
                    return Some(parsed);
                }
            }
        }
        probe += 4;
    }
    None
}

fn parse_string_table(block: &[u8], table_off: usize, count: usize) -> Option<(String, String)> {
    let mut metas = Vec::with_capacity(count);
    for i in 0..count {
        let m = table_off + 4 + i * 8;
        let rel_off = read_u32_le(block.get(m..m + 4)?) as usize;
        let kind = read_u32_le(block.get(m + 4..m + 8)?);
        metas.push((rel_off, kind));
    }

    let read_at = |rel_off: usize| -> Option<String> {
        let abs = table_off + rel_off;
        read_inline_string(block, abs)
    };

    let name = read_at(metas.first()?.0)?;
    let description = match count {
        5 => read_at(metas[4].0).unwrap_or_default(),
        2 => read_at(metas[1].0).unwrap_or_default(),
        _ => String::new(),
    };

    if name.is_empty() {
        return None;
    }
    Some((name, description))
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
