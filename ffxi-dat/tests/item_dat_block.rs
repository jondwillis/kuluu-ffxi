//! Hand-built minimal item-DAT block round-trip.
//!
//! Builds one fixed-stride equipment block in *plaintext*, embeds a
//! 1×1 32bpp Graphic icon, then applies the on-disk obfuscation
//! (rotate every byte right by 3 — the inverse of the parser's
//! rotate-by-5) so the parser must de-rotate before any field reads
//! correctly. Proves: block indexing, the rotate decode, equip-header
//! field extraction, the inline string table, and the embedded-icon
//! decode all hang together — without a real client install.

use ffxi_dat::item_dat::{
    self, ITEM_BLOCK_STRIDE, ITEM_FLAG_EX, ITEM_FLAG_RARE, ITEM_ICON_OFFSET,
};

/// On-disk encode: rotate each byte RIGHT by 3, which the parser's
/// rotate-RIGHT-by-5 inverts (rotate_right(3) ∘ rotate_right(5) ==
/// rotate_right(8) == identity).
fn encode_block(plain: &mut [u8]) {
    for b in plain.iter_mut() {
        *b = b.rotate_right(3);
    }
}

/// A 1×1 32bpp packed-BGRA Graphic chunk (flag 0x91), the same family
/// `map_image::parse_graphic` decodes for status icons.
fn tiny_icon() -> Vec<u8> {
    let mut g = vec![0x91u8];
    g.extend_from_slice(b"iconcat0"); // 8-byte category
    g.extend_from_slice(b"itm00001"); // 8-byte id
    g.extend_from_slice(&40u32.to_le_bytes()); // bmi_size
    g.extend_from_slice(&1i32.to_le_bytes()); // width
    g.extend_from_slice(&1i32.to_le_bytes()); // height
    g.extend_from_slice(&1u16.to_le_bytes()); // planes
    g.extend_from_slice(&32u16.to_le_bytes()); // bit_count
    g.extend_from_slice(&[0u8; 24]); // compression..important_colors
    g.extend_from_slice(&[0x11, 0x22, 0x33, 0x80]); // B,G,R,A(opaque)
    g
}

/// Write an inline item string at `block[off..]`: u32 `1`, six u32 `0`,
/// the text, a NUL, then 4-byte alignment padding. Returns the byte
/// length written.
fn write_inline_string(block: &mut [u8], off: usize, text: &str) -> usize {
    let mut p = off;
    block[p..p + 4].copy_from_slice(&1u32.to_le_bytes());
    p += 4;
    for _ in 0..6 {
        block[p..p + 4].copy_from_slice(&0u32.to_le_bytes());
        p += 4;
    }
    block[p..p + text.len()].copy_from_slice(text.as_bytes());
    p += text.len();
    block[p] = 0; // NUL terminator
    p += 1;
    let pad = (4 - ((text.len() + 1) & 3)) & 3;
    p += pad;
    p - off
}

/// Build a complete plaintext equipment block for `item_id` with the
/// given name/description and the tiny icon, then encode it.
fn build_block(item_id: u32, name: &str, desc: &str, flags: u16) -> Vec<u8> {
    let mut block = vec![0u8; ITEM_BLOCK_STRIDE];

    // Common header.
    block[0x00..0x04].copy_from_slice(&item_id.to_le_bytes());
    block[0x04..0x06].copy_from_slice(&flags.to_le_bytes());
    block[0x06..0x08].copy_from_slice(&1u16.to_le_bytes()); // stack_size
    block[0x08..0x0A].copy_from_slice(&4u16.to_le_bytes()); // item_type
    block[0x0A..0x0C].copy_from_slice(&0u16.to_le_bytes()); // resource_id
    block[0x0C..0x0E].copy_from_slice(&0u16.to_le_bytes()); // valid_targets

    // Equipment header (item_id is in the armor range → is_equipment).
    let mut off = 0x0E;
    block[off..off + 2].copy_from_slice(&50u16.to_le_bytes()); // level
    off += 2;
    block[off..off + 2].copy_from_slice(&0x0010u16.to_le_bytes()); // slots
    off += 2;
    block[off..off + 2].copy_from_slice(&0x00FFu16.to_le_bytes()); // races
    off += 2;
    block[off..off + 4].copy_from_slice(&0x0000_0FFFu32.to_le_bytes()); // jobs
    off += 4;
    block[off..off + 2].copy_from_slice(&0u16.to_le_bytes()); // superior_level
    off += 2;
    block[off..off + 2].copy_from_slice(&0u16.to_le_bytes()); // shield_size
    off += 2;
    // Not a weapon → no weapon interpose. Charge/recast tail:
    block[off] = 7; // max_charges
    off += 1;
    block[off] = 0; // casting_time
    off += 1;
    block[off..off + 2].copy_from_slice(&0u16.to_le_bytes()); // use_delay
    off += 2;
    block[off..off + 4].copy_from_slice(&300u32.to_le_bytes()); // reuse_delay (300s)
    off += 4;

    // String table immediately follows the equip block (4-byte aligned).
    let table_off = (off + 3) & !3;
    // English: 5 contents (name, article#, singular, plural, desc).
    block[table_off..table_off + 4].copy_from_slice(&5u32.to_le_bytes());

    // Reserve metas; bodies follow them.
    let metas_start = table_off + 4;
    let mut body = metas_start + 5 * 8;
    let mut rel_offsets = [0u32; 5];

    // name (index 0).
    rel_offsets[0] = (body - table_off) as u32;
    body += write_inline_string(&mut block, body, name);
    // article number (index 1) — a 4-byte number, type=1.
    rel_offsets[1] = (body - table_off) as u32;
    block[body..body + 4].copy_from_slice(&2u32.to_le_bytes());
    body += 4;
    // singular (index 2).
    rel_offsets[2] = (body - table_off) as u32;
    body += write_inline_string(&mut block, body, name);
    // plural (index 3).
    rel_offsets[3] = (body - table_off) as u32;
    body += write_inline_string(&mut block, body, name);
    // description (index 4).
    rel_offsets[4] = (body - table_off) as u32;
    let _ = write_inline_string(&mut block, body, desc);

    // Backfill metas. Index 1 is the article number (type 1); the
    // rest are string bodies (type 0).
    for (i, &rel) in rel_offsets.iter().enumerate() {
        let m = metas_start + i * 8;
        block[m..m + 4].copy_from_slice(&rel.to_le_bytes());
        let kind: u32 = if i == 1 { 1 } else { 0 };
        block[m + 4..m + 8].copy_from_slice(&kind.to_le_bytes());
    }

    // Embedded icon at 0x280: u32 size + Graphic chunk + trailing 0xFF.
    let icon = tiny_icon();
    block[ITEM_ICON_OFFSET..ITEM_ICON_OFFSET + 4]
        .copy_from_slice(&(icon.len() as u32).to_le_bytes());
    let istart = ITEM_ICON_OFFSET + 4;
    block[istart..istart + icon.len()].copy_from_slice(&icon);
    *block.last_mut().unwrap() = 0xFF;

    encode_block(&mut block);
    block
}

/// A single armor block at index 2 decodes all fields, strings, and
/// the embedded icon.
#[test]
fn lookup_decodes_minimal_armor_block() {
    // item_id 0x3000 is in the armor range → equipment header present.
    let item_id: u16 = 0x3000;
    let block = build_block(
        item_id as u32,
        "Test Cap",
        "DEF:10\nA hand-built test item.",
        ITEM_FLAG_RARE | ITEM_FLAG_EX,
    );

    // Place it as block index == item_id so lookup indexes it directly.
    let mut dat = vec![0u8; (item_id as usize + 1) * ITEM_BLOCK_STRIDE];
    let start = item_id as usize * ITEM_BLOCK_STRIDE;
    dat[start..start + ITEM_BLOCK_STRIDE].copy_from_slice(&block);

    let item = item_dat::lookup(&dat, item_id).expect("block decodes");
    assert_eq!(item.name, "Test Cap");
    assert_eq!(item.description, "DEF:10\nA hand-built test item.");
    assert_eq!(item.level, 50);
    assert_eq!(item.slot_mask, 0x0010);
    assert_eq!(item.races_mask, 0x00FF);
    assert_eq!(item.jobs_mask, 0x0000_0FFF);
    assert_eq!(item.max_charges, 7);
    assert_eq!(item.recast_base, 300);
    assert!(item.is_rare());
    assert!(item.is_ex());

    // Embedded icon decodes to a 1×1 RGBA pixel (BGRA 0x11,0x22,0x33 →
    // RGBA 0x33,0x22,0x11, alpha 0x80*2 saturated to 0xFF).
    let icon = item.icon.expect("icon decodes");
    assert_eq!((icon.width, icon.height), (1, 1));
    assert_eq!(&icon.rgba[0..4], &[0x33, 0x22, 0x11, 0xFF]);
}

/// `icon_at` is a thin alias for the icon path.
#[test]
fn icon_at_matches_lookup_icon() {
    let item_id: u16 = 0x3001;
    let block = build_block(item_id as u32, "Iconic", "x", 0);
    let mut dat = vec![0u8; (item_id as usize + 1) * ITEM_BLOCK_STRIDE];
    let start = item_id as usize * ITEM_BLOCK_STRIDE;
    dat[start..start + ITEM_BLOCK_STRIDE].copy_from_slice(&block);

    let icon = item_dat::icon_at(&dat, item_id).expect("icon_at decodes");
    assert_eq!((icon.width, icon.height), (1, 1));
}

/// Out-of-range and truncated lookups return None, never panic.
#[test]
fn lookup_out_of_range_is_none() {
    let dat = vec![0u8; ITEM_BLOCK_STRIDE]; // one all-zero block (index 0)
    // Index 1 is past the end.
    assert!(item_dat::lookup(&dat, 1).is_none());
    // A short buffer that can't hold even one block.
    let short = vec![0u8; 16];
    assert!(item_dat::lookup(&short, 0).is_none());
    assert!(item_dat::icon_at(&short, 0).is_none());
}
