use std::path::Path;

use ffxi_dat::client_profile::{ItemBlockLayout, ITEM_BLOCK_TRAILER, ITEM_BYTE_SHIFT};
use ffxi_dat::item_dat::{
    self, ItemDatError, ItemTable, ITEM_FLAG_EX, ITEM_FLAG_RARE, ITEM_ICON_OFFSET,
};

const ARMOR_BASE: u16 = 0x2800;
const GIL: u16 = 0xFFFF;
const STRING_TABLE_ENTRIES: usize = 5;

fn encode_block(plain: &mut [u8]) {
    for b in plain.iter_mut() {
        *b = b.rotate_left(ITEM_BYTE_SHIFT);
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

fn put_u16(block: &mut [u8], off: usize, v: u16) -> usize {
    block[off..off + 2].copy_from_slice(&v.to_le_bytes());
    off + 2
}

fn put_u32(block: &mut [u8], off: usize, v: u32) -> usize {
    block[off..off + 4].copy_from_slice(&v.to_le_bytes());
    off + 4
}

fn write_string_table(block: &mut [u8], table_off: usize, names: [&str; 3], desc: &str) {
    let [name, log_name, log_name_plural] = names;
    put_u32(block, table_off, STRING_TABLE_ENTRIES as u32);
    let metas_start = table_off + 4;
    let mut body = metas_start + STRING_TABLE_ENTRIES * 8;
    let mut rel_offsets = [0u32; STRING_TABLE_ENTRIES];

    rel_offsets[0] = (body - table_off) as u32;
    body += write_inline_string(block, body, name);

    rel_offsets[1] = (body - table_off) as u32;
    body = put_u32(block, body, 2);

    rel_offsets[2] = (body - table_off) as u32;
    body += write_inline_string(block, body, log_name);

    rel_offsets[3] = (body - table_off) as u32;
    body += write_inline_string(block, body, log_name_plural);

    rel_offsets[4] = (body - table_off) as u32;
    let _ = write_inline_string(block, body, desc);

    for (i, &rel) in rel_offsets.iter().enumerate() {
        let m = metas_start + i * 8;
        put_u32(block, m, rel);
        let kind: u32 = if i == 1 { 1 } else { 0 };
        put_u32(block, m + 4, kind);
    }
}

/// Where the string table sits in a measured armor block (Defending Ring) and
/// the measured currency block (Gil), per layout.
fn measured_table_offset(layout: ItemBlockLayout, equipment: bool) -> usize {
    match (layout, equipment) {
        (ItemBlockLayout::Legacy, true) => 0x2C,
        (ItemBlockLayout::Retail2026, true) => 0x30,
        (ItemBlockLayout::Legacy, false) => 0x10,
        (ItemBlockLayout::Retail2026, false) => 0x14,
    }
}

/// An armor block for an id in the equipment range, a currency-shaped block
/// otherwise, laid out per `layout` and obfuscated like the on-disk DAT.
fn build_block(
    layout: ItemBlockLayout,
    item_id: u16,
    names: [&str; 3],
    desc: &str,
    flags: u16,
) -> Vec<u8> {
    let mut block = vec![0u8; layout.stride()];
    let shift = layout.header_shift();

    put_u32(&mut block, 0x00, item_id as u32);
    put_u16(&mut block, 0x04, flags);
    let mut off = 0x06 + shift;
    off = put_u16(&mut block, off, 1);
    off = put_u16(&mut block, off, 4);
    off = put_u16(&mut block, off, 0);
    off = put_u16(&mut block, off, 0);
    assert_eq!(off, 0x0E + shift);

    let equipment = (0x2800..=0x6FFF).contains(&item_id);
    if equipment {
        off = put_u16(&mut block, off, 50);
        off = put_u16(&mut block, off, 0x0010);
        off = put_u16(&mut block, off, 0x00FF);
        off += layout.races_gap();
        off = put_u32(&mut block, off, 0x0000_0FFF);
        off = put_u32(&mut block, off, 0);

        block[off] = 7;
        off += 1;
        block[off] = 0;
        off += 1;
        off = put_u16(&mut block, off, 0);
        off = put_u32(&mut block, off, 300);
        off = put_u32(&mut block, off, 0);
        off = put_u32(&mut block, off, 0);
    } else {
        off = put_u16(&mut block, off, 0);
        off += shift;
    }

    let table_off = off;
    assert_eq!(table_off, measured_table_offset(layout, equipment));
    write_string_table(&mut block, table_off, names, desc);

    let icon = tiny_icon();
    put_u32(&mut block, ITEM_ICON_OFFSET, icon.len() as u32);
    let istart = ITEM_ICON_OFFSET + 4;
    block[istart..istart + icon.len()].copy_from_slice(&icon);
    *block.last_mut().unwrap() = ITEM_BLOCK_TRAILER;

    encode_block(&mut block);
    block
}

/// Plants a count-like word and a plausible descriptor in the armor tail just
/// before the real table, pointing at an inline string parked past the table
/// body; only the descriptor-to-body check tells it from a real table.
fn plant_decoy_table(layout: ItemBlockLayout, encoded: &[u8]) -> Vec<u8> {
    const DECOY_STRING_OFFSET: usize = 0x200;
    let mut block: Vec<u8> = encoded
        .iter()
        .map(|b| b.rotate_right(ITEM_BYTE_SHIFT))
        .collect();
    let table_off = measured_table_offset(layout, true);
    let decoy_off = table_off - 8;
    put_u32(&mut block, decoy_off, 1);
    put_u32(
        &mut block,
        decoy_off + 4,
        (DECOY_STRING_OFFSET - decoy_off) as u32,
    );
    write_inline_string(&mut block, DECOY_STRING_OFFSET, "Decoy");
    encode_block(&mut block);
    block
}

fn build_dat(blocks: &[Vec<u8>]) -> Vec<u8> {
    blocks.concat()
}

fn armor_pair(layout: ItemBlockLayout) -> Vec<u8> {
    build_dat(&[
        build_block(
            layout,
            ARMOR_BASE,
            ["Base Cap", "base cap", "base caps"],
            "x",
            0,
        ),
        build_block(
            layout,
            ARMOR_BASE + 1,
            ["Test Cap", "test cap", "test caps"],
            "DEF:10\nA hand-built test item.",
            ITEM_FLAG_RARE | ITEM_FLAG_EX,
        ),
    ])
}

fn currency_dat(layout: ItemBlockLayout, zero_blocks: usize) -> Vec<u8> {
    let mut blocks = vec![build_block(
        layout,
        GIL,
        ["Gil", "gil", "gil"],
        "Currency.",
        0,
    )];
    blocks.resize(1 + zero_blocks, vec![0u8; layout.stride()]);
    build_dat(&blocks)
}

#[test]
fn lookup_decodes_the_same_armor_block_on_every_layout() {
    let mut decoded = Vec::new();
    for layout in ItemBlockLayout::ALL {
        let dat = armor_pair(layout);
        assert_eq!(ItemBlockLayout::detect(&dat), Some(layout));

        let item = item_dat::lookup(&dat, ARMOR_BASE + 1).expect("block decodes");
        assert_eq!(item.name, "Test Cap", "{}", layout.name());
        assert_eq!(item.log_name, "test cap");
        assert_eq!(item.log_name_plural, "test caps");
        assert_eq!(item.description, "DEF:10\nA hand-built test item.");
        assert_eq!(item.level, 50);
        assert_eq!(item.slot_mask, 0x0010);
        assert_eq!(item.races_mask, 0x00FF);
        assert_eq!(item.jobs_mask, 0x0000_0FFF);
        assert_eq!(item.item_type, 4);
        assert_eq!(item.max_charges, 7);
        assert_eq!(item.recast_base, 300);
        assert!(item.is_rare());
        assert!(item.is_ex());

        let icon = item.icon.as_ref().expect("icon decodes");
        assert_eq!((icon.width, icon.height), (1, 1));
        assert_eq!(&icon.rgba[0..4], &[0x33, 0x22, 0x11, 0xFF]);

        assert_eq!(
            item_dat::lookup(&dat, ARMOR_BASE)
                .expect("base decodes")
                .name,
            "Base Cap"
        );
        decoded.push(format!("{item:?}"));
    }
    assert!(
        decoded.windows(2).all(|w| w[0] == w[1]),
        "layouts disagree: {decoded:#?}"
    );
}

#[test]
fn empty_log_names_fall_back_to_the_display_name() {
    for layout in ItemBlockLayout::ALL {
        let dat = build_dat(&[
            build_block(layout, ARMOR_BASE, ["Base Cap", "", ""], "x", 0),
            build_block(layout, ARMOR_BASE + 1, ["Test Cap", "", ""], "x", 0),
        ]);
        let item = item_dat::lookup(&dat, ARMOR_BASE + 1).expect("block decodes");
        assert_eq!(item.log_name, "Test Cap", "{}", layout.name());
        assert_eq!(item.log_name_plural, "Test Cap");
    }
}

#[test]
fn a_count_like_tail_word_does_not_hijack_the_string_table() {
    for layout in ItemBlockLayout::ALL {
        let base = build_block(
            layout,
            ARMOR_BASE,
            ["Base Cap", "base cap", "base caps"],
            "x",
            0,
        );
        let cap = build_block(
            layout,
            ARMOR_BASE + 1,
            ["Test Cap", "test cap", "test caps"],
            "x",
            0,
        );
        let dat = build_dat(&[base, plant_decoy_table(layout, &cap)]);
        let item = item_dat::lookup(&dat, ARMOR_BASE + 1).expect("block decodes");
        assert_eq!(item.name, "Test Cap", "{}", layout.name());
        assert_eq!(item.log_name, "test cap");
    }
}

#[test]
fn icon_at_matches_lookup_icon() {
    for layout in ItemBlockLayout::ALL {
        let dat = armor_pair(layout);
        let icon = item_dat::icon_at(&dat, ARMOR_BASE + 1).expect("icon_at decodes");
        assert_eq!((icon.width, icon.height), (1, 1), "{}", layout.name());
    }
}

/// Legacy blocks spaced at the Retail2026 stride do not decode through the
/// Retail2026 path: the first reads as a lone Legacy block (trailer then
/// zeros), and the second sits past both strides.
#[test]
fn legacy_blocks_at_the_retail_stride_do_not_decode_as_retail() {
    let legacy = ItemBlockLayout::Legacy;
    let retail = ItemBlockLayout::Retail2026;
    let padded: Vec<u8> = [ARMOR_BASE, ARMOR_BASE + 1]
        .into_iter()
        .flat_map(|id| {
            let mut b = build_block(legacy, id, ["Cap", "cap", "caps"], "x", 0);
            b.resize(retail.stride(), 0);
            b
        })
        .collect();
    assert_eq!(padded.len(), retail.stride() * 2);
    assert_eq!(ItemBlockLayout::detect(&padded), Some(legacy));
    assert!(item_dat::lookup(&padded, ARMOR_BASE + 1).is_none());
    assert!(item_dat::icon_at(&padded, ARMOR_BASE + 1).is_none());
    assert_eq!(
        item_dat::lookup(&padded, ARMOR_BASE)
            .expect("first block decodes")
            .name,
        "Cap"
    );

    let head = &armor_pair(legacy)[..retail.stride() + 4];
    assert_eq!(ItemBlockLayout::detect(head), Some(legacy));
    assert!(item_dat::lookup(head, ARMOR_BASE + 1).is_none());
}

#[test]
fn detect_rejects_consecutive_ids_without_the_trailer() {
    for layout in ItemBlockLayout::ALL {
        let mut dat = armor_pair(layout);
        assert_eq!(ItemBlockLayout::detect(&dat), Some(layout));
        dat[layout.stride() - 1] = 0;
        assert_eq!(ItemBlockLayout::detect(&dat), None, "{}", layout.name());
        assert!(item_dat::lookup(&dat, ARMOR_BASE + 1).is_none());
    }
}

#[test]
fn detect_accepts_one_real_block_followed_by_zeros() {
    for layout in ItemBlockLayout::ALL {
        let dat = currency_dat(layout, 1);
        assert_eq!(
            ItemBlockLayout::detect(&dat),
            Some(layout),
            "{}",
            layout.name()
        );
        assert_eq!(
            item_dat::lookup(&dat, GIL).expect("gil decodes").name,
            "Gil"
        );

        let mut untrailed = dat.clone();
        untrailed[layout.stride() - 1] = 0;
        assert_eq!(ItemBlockLayout::detect(&untrailed), None);
    }
}

#[test]
fn lookup_out_of_range_is_none() {
    for layout in ItemBlockLayout::ALL {
        let dat = armor_pair(layout);
        assert!(item_dat::lookup(&dat, ARMOR_BASE + 2).is_none());
        assert!(item_dat::lookup(&dat, ARMOR_BASE - 1).is_none());
        assert!(item_dat::lookup(&dat, 0).is_none());
    }

    let single = vec![0u8; ItemBlockLayout::Legacy.stride()];
    assert!(item_dat::lookup(&single, 1).is_none());

    let short = vec![0u8; 16];
    assert!(item_dat::lookup(&short, 0).is_none());
    assert!(item_dat::icon_at(&short, 0).is_none());
}

fn write_dat(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn table_resolves_every_layout_and_reports_undetectable_files() {
    for layout in ItemBlockLayout::ALL {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_dat(root, "ROM/118/109.DAT", &armor_pair(layout));
        write_dat(root, "ROM/174/48.DAT", &currency_dat(layout, 15));
        write_dat(root, "ROM/118/106.DAT", &vec![0u8; layout.stride() * 2]);

        let table = ItemTable::open(root);
        assert_eq!(table.layouts(), vec![layout]);

        let cap = table.lookup(ARMOR_BASE + 1).expect("armor resolves");
        assert_eq!(cap.name, "Test Cap");
        assert_eq!(cap.level, 50);
        assert_eq!(table.lookup(GIL).expect("gil resolves").name, "Gil");
        assert!(table.icon(ARMOR_BASE).is_some());
        assert!(table.lookup(ARMOR_BASE + 2).is_none());

        let skipped = table.skipped();
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert!(
            matches!(&skipped[0], ItemDatError::LayoutUndetected { path } if path.ends_with("ROM/118/106.DAT")),
            "{skipped:?}"
        );
    }
}
