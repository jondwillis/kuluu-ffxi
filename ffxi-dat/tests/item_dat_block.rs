use ffxi_dat::item_dat::{self, ITEM_BLOCK_STRIDE, ITEM_FLAG_EX, ITEM_FLAG_RARE, ITEM_ICON_OFFSET};

fn encode_block(plain: &mut [u8]) {
    for b in plain.iter_mut() {
        *b = b.rotate_right(3);
    }
}

fn tiny_icon() -> Vec<u8> {
    let mut g = vec![0x91u8];
    g.extend_from_slice(b"iconcat0");
    g.extend_from_slice(b"itm00001");
    g.extend_from_slice(&40u32.to_le_bytes());
    g.extend_from_slice(&1i32.to_le_bytes());
    g.extend_from_slice(&1i32.to_le_bytes());
    g.extend_from_slice(&1u16.to_le_bytes());
    g.extend_from_slice(&32u16.to_le_bytes());
    g.extend_from_slice(&[0u8; 24]);
    g.extend_from_slice(&[0x11, 0x22, 0x33, 0x80]);
    g
}

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
    block[p] = 0;
    p += 1;
    let pad = (4 - ((text.len() + 1) & 3)) & 3;
    p += pad;
    p - off
}

fn build_block(item_id: u32, name: &str, desc: &str, flags: u16) -> Vec<u8> {
    let mut block = vec![0u8; ITEM_BLOCK_STRIDE];

    block[0x00..0x04].copy_from_slice(&item_id.to_le_bytes());
    block[0x04..0x06].copy_from_slice(&flags.to_le_bytes());
    block[0x06..0x08].copy_from_slice(&1u16.to_le_bytes());
    block[0x08..0x0A].copy_from_slice(&4u16.to_le_bytes());
    block[0x0A..0x0C].copy_from_slice(&0u16.to_le_bytes());
    block[0x0C..0x0E].copy_from_slice(&0u16.to_le_bytes());

    let mut off = 0x0E;
    block[off..off + 2].copy_from_slice(&50u16.to_le_bytes());
    off += 2;
    block[off..off + 2].copy_from_slice(&0x0010u16.to_le_bytes());
    off += 2;
    block[off..off + 2].copy_from_slice(&0x00FFu16.to_le_bytes());
    off += 2;
    block[off..off + 4].copy_from_slice(&0x0000_0FFFu32.to_le_bytes());
    off += 4;
    block[off..off + 2].copy_from_slice(&0u16.to_le_bytes());
    off += 2;
    block[off..off + 2].copy_from_slice(&0u16.to_le_bytes());
    off += 2;

    block[off] = 7;
    off += 1;
    block[off] = 0;
    off += 1;
    block[off..off + 2].copy_from_slice(&0u16.to_le_bytes());
    off += 2;
    block[off..off + 4].copy_from_slice(&300u32.to_le_bytes());
    off += 4;

    let table_off = (off + 3) & !3;

    block[table_off..table_off + 4].copy_from_slice(&5u32.to_le_bytes());

    let metas_start = table_off + 4;
    let mut body = metas_start + 5 * 8;
    let mut rel_offsets = [0u32; 5];

    rel_offsets[0] = (body - table_off) as u32;
    body += write_inline_string(&mut block, body, name);

    rel_offsets[1] = (body - table_off) as u32;
    block[body..body + 4].copy_from_slice(&2u32.to_le_bytes());
    body += 4;

    rel_offsets[2] = (body - table_off) as u32;
    body += write_inline_string(&mut block, body, name);

    rel_offsets[3] = (body - table_off) as u32;
    body += write_inline_string(&mut block, body, name);

    rel_offsets[4] = (body - table_off) as u32;
    let _ = write_inline_string(&mut block, body, desc);

    for (i, &rel) in rel_offsets.iter().enumerate() {
        let m = metas_start + i * 8;
        block[m..m + 4].copy_from_slice(&rel.to_le_bytes());
        let kind: u32 = if i == 1 { 1 } else { 0 };
        block[m + 4..m + 8].copy_from_slice(&kind.to_le_bytes());
    }

    let icon = tiny_icon();
    block[ITEM_ICON_OFFSET..ITEM_ICON_OFFSET + 4]
        .copy_from_slice(&(icon.len() as u32).to_le_bytes());
    let istart = ITEM_ICON_OFFSET + 4;
    block[istart..istart + icon.len()].copy_from_slice(&icon);
    *block.last_mut().unwrap() = 0xFF;

    encode_block(&mut block);
    block
}

#[test]
fn lookup_decodes_minimal_armor_block() {
    let item_id: u16 = 0x3000;
    let block = build_block(
        item_id as u32,
        "Test Cap",
        "DEF:10\nA hand-built test item.",
        ITEM_FLAG_RARE | ITEM_FLAG_EX,
    );

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

    let icon = item.icon.expect("icon decodes");
    assert_eq!((icon.width, icon.height), (1, 1));
    assert_eq!(&icon.rgba[0..4], &[0x33, 0x22, 0x11, 0xFF]);
}

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

#[test]
fn lookup_out_of_range_is_none() {
    let dat = vec![0u8; ITEM_BLOCK_STRIDE];

    assert!(item_dat::lookup(&dat, 1).is_none());

    let short = vec![0u8; 16];
    assert!(item_dat::lookup(&short, 0).is_none());
    assert!(item_dat::icon_at(&short, 0).is_none());
}
