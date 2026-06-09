//! Retail item-DAT decoder — static per-item metadata + embedded icon.
//!
//! Sibling to [`crate::map_image::status_icon_at`]: both index a flat,
//! fixed-stride DAT by an id and decode an embedded [`crate::map_image`]
//! Graphic chunk. Where the status-icon sheet is plaintext, the item
//! DAT family ships each block lightly *encrypted* with a bytewise bit
//! rotation, so a block must be de-rotated before any field is read.
//!
//! # Spec (derived as an algorithmic reference; not a copy)
//!
//! Each `item_id` indexes one fixed `ITEM_BLOCK_STRIDE`-byte block.
//! The whole block is obfuscated by rotating every byte right by 5
//! bits (`(b >> 5) | (b << 3)`); the inverse (rotate by 3) is the
//! on-disk encode. After de-rotation the block is:
//!
//! ```text
//! offset  size  field
//! 0x000   4     u32 item_id
//! 0x004   2     u16 flags        — rare/ex/… bitfield (see ITEM_FLAG_*)
//! 0x006   2     u16 stack_size
//! 0x008   2     u16 item_type
//! 0x00A   2     u16 resource_id
//! 0x00C   2     u16 valid_targets
//! 0x00E   …     category-specific block (Armor/Weapon carry the
//!               equip fields below; other categories don't)
//! …
//! 0x280   4     u32 icon_size, then `icon_size` bytes of a Graphic
//!               chunk (flag 0x91, 32bpp BGRA — same family as
//!               `sts_icon`), zero padding, then a trailing 0xFF.
//! ```
//!
//! For Armor/Weapon the bytes after the common header are, in order:
//! `u16 level, u16 slots, u16 races, u32 jobs, u16 superior_level,
//! u16 shield_size, [weapon-only: u16 dmg, u16 delay, u16 dps,
//! u8 skill, u8 jug, u32 _], u8 max_charges, u8 casting_time,
//! u16 use_delay, u32 reuse_delay, …`.
//!
//! The string table lives between the category block and the icon at
//! 0x280: `u32 content_count`, then `content_count` `(u32 offset,
//! u32 type)` metas, then the string bodies. Each string body is
//! `u32 1`, six `u32 0` of padding, the text bytes, a NUL terminator,
//! then 4-byte alignment padding. English blocks carry 5 contents
//! (name, article#, singular, plural, description); we surface the
//! display `name` and the `description`.
//!
//! # Category id ranges (POLUtils / public reverse-engineering)
//! Only Armor (`0x2000..` minus puppet, `0x5A00..`) and Weapon
//! (`0x4000..0x6000`) carry the equip header; everything else leaves
//! the equip-only fields zeroed.
//!
//! # AGPL note
//! The block layout and the rotate-by-5 obfuscation are facts about
//! the retail client format, cross-referenced against the AGPL-3
//! xi-tinkerer `item_info` reader purely as an algorithmic reference.
//! No code is copied or linked; this module re-implements the decode
//! from first principles so `ffxi-dat` stays linkable from `ffxi-mcp`.

use crate::map_image::{self, GraphicImage};

/// Candidate file_ids for the retail *English* item DAT family. The
/// retail client splits items across several DATs by category (general
/// items, weapons, armor, …). The exact file_id depends on the client
/// version's VTABLE, so callers should try these in order via
/// [`crate::DatRoot::resolve`] and take the first that resolves to a
/// file whose length is a multiple of [`ITEM_BLOCK_STRIDE`].
///
/// These are the well-known English item-info file_ids; treat them as
/// candidates rather than a guarantee — the resolver + stride check is
/// the source of truth.
pub const ITEM_DAT_FILE_ID: &[u32] = &[
    0x0DA9, // item DAT A (general items)
    0x0DAA, // item DAT B
    0x0DAB, // weapons
    0x0DAC, // armor
];

/// Bytes per item block. The whole block is the encryption unit.
pub const ITEM_BLOCK_STRIDE: usize = 0xC00;

/// Byte offset of the icon sub-block (`u32 size` + Graphic chunk).
pub const ITEM_ICON_OFFSET: usize = 0x280;

/// The rotate-right shift used to de-obfuscate an on-disk item block.
const ITEM_BLOCK_SHIFT: u32 = 5;

/// `flags` bit: item is Rare (only one may be held).
pub const ITEM_FLAG_RARE: u16 = 0x8000;
/// `flags` bit: item is Ex (cannot be traded/bazaared/auctioned).
pub const ITEM_FLAG_EX: u16 = 0x4000;

/// Static, install-resident item metadata. Composed at render time
/// with the dynamic per-slot state that arrives over the wire.
#[derive(Debug, Clone)]
pub struct ItemStatic {
    /// Display name (English).
    pub name: String,
    /// Help/description text (English); may contain embedded newlines.
    pub description: String,
    /// Equipment-slot bitmask (0 for non-equipment).
    pub slot_mask: u16,
    /// Job-availability bitmask (0 for non-equipment).
    pub jobs_mask: u32,
    /// Race-restriction bitmask (0 for non-equipment).
    pub races_mask: u16,
    /// Required level (0 for non-equipment), saturated from the DAT u16.
    pub level: u8,
    /// Raw flags bitfield; test against [`ITEM_FLAG_RARE`] / [`ITEM_FLAG_EX`].
    pub flags: u16,
    /// Maximum enchantment charges (0 when not a charged item).
    pub max_charges: u8,
    /// Base recast in seconds, saturated from the DAT's reuse delay.
    pub recast_base: u16,
    /// Item type discriminator (saturated from the DAT u16).
    pub item_type: u8,
    /// Embedded 32bpp icon, decoded via [`map_image::parse_graphic`].
    /// `None` when the block carries no parseable icon (placeholder slots).
    pub icon: Option<GraphicImage>,
}

impl ItemStatic {
    /// True when the item carries the Rare flag.
    pub fn is_rare(&self) -> bool {
        self.flags & ITEM_FLAG_RARE != 0
    }
    /// True when the item carries the Ex flag.
    pub fn is_ex(&self) -> bool {
        self.flags & ITEM_FLAG_EX != 0
    }
}

/// Whether `item_id` falls in an equipment (Armor/Weapon) category, in
/// which case the slot/race/job/level/charge/recast header is present.
/// Ranges from POLUtils' item-category map.
fn is_equipment(item_id: u32) -> bool {
    // Armor + Weapon share the equip header and span a contiguous
    // 0x2800..=0x6FFF range (weapons carve out 0x4000..=0x59FF inside
    // it — see `is_weapon`). Puppet automaton items (0x2000..0x2200)
    // are NOT equipment-shaped.
    matches!(item_id, 0x2800..=0x6FFF)
}

/// Decode the static metadata for `item_id` from the raw bytes of one
/// item DAT (`dat_bytes`). Returns `None` when the id is out of range
/// for this file, the block is truncated, or the de-rotated header
/// doesn't pass the basic id sanity check.
///
/// `dat_bytes` is a single item DAT file's contents; `item_id` is the
/// block index (the same id space the inventory packets carry). For a
/// multi-DAT install, resolve the right file first (see
/// [`ITEM_DAT_FILE_ID`]) — this function does not cross files.
pub fn lookup(dat_bytes: &[u8], item_id: u16) -> Option<ItemStatic> {
    let block = decoded_block(dat_bytes, item_id)?;

    let stored_id = read_u32_le(block.get(0x00..0x04)?);
    let flags = read_u16_le(block.get(0x04..0x06)?);
    let item_type = read_u16_le(block.get(0x08..0x0A)?);

    let (slot_mask, races_mask, jobs_mask, level, max_charges, recast_base) =
        if is_equipment(stored_id) {
            // Common header is 0x0E bytes (id..valid_targets); the
            // equip block follows.
            let mut off = 0x0E;
            let level = read_u16_le(block.get(off..off + 2)?);
            off += 2;
            let slots = read_u16_le(block.get(off..off + 2)?);
            off += 2;
            let races = read_u16_le(block.get(off..off + 2)?);
            off += 2;
            let jobs = read_u32_le(block.get(off..off + 4)?);
            off += 4;
            // superior_level (u16) + shield_size (u16).
            off += 4;
            // Weapons interpose dmg/delay/dps (u16×3) + skill/jug
            // (u8×2) + reserved (u32) before the charge/recast tail.
            if is_weapon(stored_id) {
                off += 6 + 1 + 1 + 4;
            }
            let max_charges = *block.get(off)?;
            off += 1;
            // casting_time (u8) + use_delay (u16).
            off += 1 + 2;
            let reuse_delay = read_u32_le(block.get(off..off + 4)?);
            (
                slots,
                races,
                jobs,
                (level.min(u8::MAX as u16)) as u8,
                max_charges,
                reuse_delay.min(u16::MAX as u32) as u16,
            )
        } else {
            (0, 0, 0, 0, 0, 0)
        };

    let (name, description) = read_item_strings(&block).unwrap_or_default();
    let icon = decode_icon(&block);

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

/// Decode just the embedded icon for `item_id`. Mirrors
/// [`map_image::status_icon_at`] for callers that only need the glyph.
pub fn icon_at(dat_bytes: &[u8], item_id: u16) -> Option<GraphicImage> {
    let block = decoded_block(dat_bytes, item_id)?;
    decode_icon(&block)
}

/// Whether `item_id` is in a weapon category (extra weapon header).
fn is_weapon(item_id: u32) -> bool {
    matches!(item_id, 0x4000..=0x59FF)
}

/// Slice + de-rotate the block for `item_id`. The rotation is applied
/// to a fresh owned copy so the caller's buffer stays untouched.
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

/// Decode the icon sub-block at [`ITEM_ICON_OFFSET`] (a `u32` length
/// prefix followed by a Graphic chunk). `None` for empty/placeholder
/// slots or unparseable chunks.
fn decode_icon(block: &[u8]) -> Option<GraphicImage> {
    let size = read_u32_le(block.get(ITEM_ICON_OFFSET..ITEM_ICON_OFFSET + 4)?) as usize;
    if size == 0 {
        return None;
    }
    let start = ITEM_ICON_OFFSET + 4;
    let end = start.checked_add(size)?;
    let chunk = block.get(start..end.min(block.len()))?;
    map_image::parse_graphic(chunk)
        .ok()
        .flatten()
        .map(|(img, _)| img)
}

/// Read the English `(name, description)` from the string table that
/// sits between the category block and the icon. Returns `None` if the
/// table is malformed; callers fall back to empty strings.
fn read_item_strings(block: &[u8]) -> Option<(String, String)> {
    // The string table starts after the common header + category
    // block, but its exact start varies by category. The metas carry
    // absolute offsets *relative to the start of the table*, so we
    // locate the table by scanning for the `u32 content_count` that
    // precedes a plausible meta list. In practice the table always
    // begins on a 4-byte boundary before the icon; scan forward from
    // the common header for the first content_count in 1..=9 whose
    // metas all point inside the table region.
    let table_region_end = ITEM_ICON_OFFSET;
    // The string table is always 4-byte aligned; start the probe at
    // the first aligned offset at or after the common header so we
    // can't step past the real table by landing on odd offsets.
    let mut probe = 0x10; // first 4-aligned offset >= 0x0E
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

/// Parse a located string table. `table_off` points at the
/// `content_count` u32; `count` is that count. Meta offsets are
/// relative to `table_off`. English layout indices: 0 = name,
/// 1 = article number, 2 = singular, 3 = plural, 4 = description.
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

    // English: 5 contents (name, article#, singular, plural, desc).
    // Japanese: 2 (name, desc). Single: 1 (name only).
    let name = read_at(metas.first()?.0)?;
    let description = match count {
        5 => read_at(metas[4].0).unwrap_or_default(),
        2 => read_at(metas[1].0).unwrap_or_default(),
        _ => String::new(),
    };
    // A real string never decodes empty here; an empty name means we
    // probed a false table.
    if name.is_empty() {
        return None;
    }
    Some((name, description))
}

/// Read one inline string: `u32 1`, six `u32 0` pad, NUL-terminated
/// text, then 4-byte alignment padding. We only need the text.
fn read_inline_string(block: &[u8], at: usize) -> Option<String> {
    // Header: must start with the `1` marker and six zero u32s.
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

/// Decode the FFXI item-string text bytes into a `String`. Retail
/// English item strings are effectively Latin-1 with `0x0A`-style
/// embedded newlines; we map each byte to its codepoint, matching the
/// permissive ASCII handling in [`crate::map_image`]'s field reader.
/// (Auto-translate brackets and shift-JIS sequences are out of scope
/// for the English DAT and would be handled by a richer codec later.)
fn decode_text(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// Rotate a byte right by `shift` bits — the inverse of the on-disk
/// item-block obfuscation (`(b >> shift) | (b << (8 - shift))`).
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
