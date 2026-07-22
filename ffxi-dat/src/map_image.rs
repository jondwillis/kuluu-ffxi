use std::io::Read;

use crate::{DatError, Result};

include!(concat!(env!("OUT_DIR"), "/map_dat_table.rs"));

pub fn map_dat_for_zone(zone_id: u16) -> Option<u32> {
    map_dat_for(zone_id, 0)
}

pub fn map_dat_for(zone_id: u16, map_index: u8) -> Option<u32> {
    MAP_DAT_TABLE
        .binary_search_by(|(z, m, _)| (*z, *m).cmp(&(zone_id, map_index)))
        .ok()
        .map(|i| MAP_DAT_TABLE[i].2)
}

pub fn map_count_for_zone(zone_id: u16) -> usize {
    MAP_DAT_TABLE
        .iter()
        .filter(|(z, _, _)| *z == zone_id)
        .count()
}

/// Every zone id that ships at least one map DAT, ascending and deduplicated.
/// `MAP_DAT_TABLE` is sorted by `(zone, map_index)`, so equal zone ids are
/// contiguous and dropping consecutive duplicates yields each zone once.
pub fn zones_with_maps() -> Vec<u16> {
    let mut out = Vec::new();
    for (zone, _, _) in MAP_DAT_TABLE.iter() {
        if out.last() != Some(zone) {
            out.push(*zone);
        }
    }
    out
}

pub const STATUS_ICON_FILE_ID: u32 = 87;

pub const STATUS_ICON_BLOCK_STRIDE: usize = 0x1800;

pub const STATUS_ICON_GRAPHIC_OFFSET: usize = 0x284;

pub const STATUS_ICON_COUNT: usize = 640;

pub fn status_icon_at(dat_bytes: &[u8], status_id: u16) -> Option<GraphicImage> {
    let id = status_id as usize;
    if id >= STATUS_ICON_COUNT {
        return None;
    }
    let block_start = id * STATUS_ICON_BLOCK_STRIDE + STATUS_ICON_GRAPHIC_OFFSET;
    let block_end = (id + 1) * STATUS_ICON_BLOCK_STRIDE;
    let chunk = dat_bytes.get(block_start..block_end)?;
    parse_graphic(chunk).ok().flatten().map(|(img, _)| img)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicFlag {
    Bitmap = 0x91,

    Dxt = 0xA1,

    AlphaBitmap = 0xB1,
}

impl GraphicFlag {
    pub fn from_u8(b: u8) -> Option<Self> {
        match b {
            0x91 => Some(Self::Bitmap),
            0xA1 => Some(Self::Dxt),
            0xB1 => Some(Self::AlphaBitmap),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GraphicImage {
    pub category: String,
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn parse_graphic(bytes: &[u8]) -> Result<Option<(GraphicImage, usize)>> {
    parse_graphic_inner(bytes, false)
}

/// Like [`parse_graphic`], but for item icons: 8bpp palettes carry FFXI
/// half-range alpha (transparent background) even under the plain `0x91` Bitmap
/// flag, so honor it instead of forcing everything opaque.
pub fn parse_graphic_icon(bytes: &[u8]) -> Result<Option<(GraphicImage, usize)>> {
    parse_graphic_inner(bytes, true)
}

fn parse_graphic_inner(bytes: &[u8], icon: bool) -> Result<Option<(GraphicImage, usize)>> {
    if bytes.is_empty() {
        return Ok(None);
    }
    let Some(flag) = GraphicFlag::from_u8(bytes[0]) else {
        return Ok(None);
    };
    if bytes.len() < 61 {
        return Err(DatError::Mmb(format!(
            "graphic chunk: truncated header (need 61 bytes, have {})",
            bytes.len()
        )));
    }

    let category = read_ascii_field(&bytes[1..9]);
    let id = read_ascii_field(&bytes[9..17]);
    let bmi_size = read_u32_le(&bytes[17..21]);
    if bmi_size != 40 {
        return Err(DatError::Mmb(format!(
            "graphic chunk `{category}/{id}`: BITMAPINFO length {bmi_size} ≠ 40 — not a Graphic"
        )));
    }
    let width = read_i32_le(&bytes[21..25]);
    let height_signed = read_i32_le(&bytes[25..29]);
    let planes = read_u16_le(&bytes[29..31]);
    let bit_count = read_u16_le(&bytes[31..33]);
    let compression = read_u32_le(&bytes[33..37]);
    let _image_size = read_u32_le(&bytes[37..41]);
    let _x_pels = read_u32_le(&bytes[41..45]);
    let _y_pels = read_u32_le(&bytes[45..49]);
    let used_colors = read_u32_le(&bytes[49..53]);
    let _important_colors = read_u32_le(&bytes[53..57]);

    const MAX_DIM: u32 = 16 * 1024;
    let height_abs = height_signed.unsigned_abs();
    if planes != 1
        || width <= 0
        || (width as u32) > MAX_DIM
        || height_abs == 0
        || height_abs > MAX_DIM
    {
        return Err(DatError::Mmb(format!(
            "graphic chunk `{category}/{id}`: nonsensical BITMAPINFO (w={width} h={height_signed} planes={planes})",
        )));
    }
    let width_u = width as u32;
    let height_u = height_abs;
    let top_down = height_signed < 0;

    match flag {
        GraphicFlag::Bitmap | GraphicFlag::AlphaBitmap => {
            let (rgba, consumed) = if bit_count == 32 {
                decode_packed_bgra32(&bytes[57..], width_u, height_u, top_down, compression)?
            } else {
                let alpha = if icon {
                    PaletteAlpha::HalfScaled
                } else if matches!(flag, GraphicFlag::AlphaBitmap) {
                    PaletteAlpha::Raw
                } else {
                    PaletteAlpha::Opaque
                };
                decode_paletted_bitmap(
                    &bytes[57..],
                    width_u,
                    height_u,
                    bit_count,
                    used_colors,
                    alpha,
                    top_down,
                    compression,
                )?
            };
            Ok(Some((
                GraphicImage {
                    category,
                    id,
                    width: width_u,
                    height: height_u,
                    rgba,
                },
                57 + consumed,
            )))
        }
        GraphicFlag::Dxt => {
            const FOURCC_OFF: usize = 57;
            const BLOCKS_OFF: usize = 69;
            if bytes.len() < BLOCKS_OFF {
                return Err(DatError::Mmb(format!(
                    "graphic chunk `{category}/{id}`: DXT header truncated (need {BLOCKS_OFF} bytes, have {})",
                    bytes.len()
                )));
            }
            let fourcc = &bytes[FOURCC_OFF..FOURCC_OFF + 4];
            let blocks = &bytes[BLOCKS_OFF..];
            let to_err = |e: crate::texture::TextureError| {
                DatError::Mmb(format!("graphic chunk `{category}/{id}`: {e}"))
            };
            let (rgba, block_size) = match fourcc {
                b"1TXD" => (
                    crate::texture::decode_dxt1_blocks(blocks, width_u, height_u)
                        .map_err(to_err)?,
                    8usize,
                ),
                b"3TXD" => (
                    crate::texture::decode_dxt3_blocks(blocks, width_u, height_u)
                        .map_err(to_err)?,
                    16usize,
                ),
                other => {
                    return Err(DatError::Mmb(format!(
                        "graphic chunk `{category}/{id}`: unsupported DXT FourCC {other:?} \
                         (only 1TXD/DXT1 and 3TXD/DXT3 are decoded)"
                    )));
                }
            };
            let consumed =
                BLOCKS_OFF + (width_u as usize / 4) * (height_u as usize / 4) * block_size;
            Ok(Some((
                GraphicImage {
                    category,
                    id,
                    width: width_u,
                    height: height_u,
                    rgba,
                },
                consumed,
            )))
        }
    }
}

pub fn scan_graphics(bytes: &[u8]) -> impl Iterator<Item = GraphicImage> + '_ {
    let mut cursor = 0usize;
    std::iter::from_fn(move || {
        while cursor < bytes.len() {
            match parse_graphic(&bytes[cursor..]) {
                Ok(Some((img, consumed))) => {
                    cursor += consumed;
                    return Some(img);
                }
                Ok(None) | Err(_) => {
                    cursor += 1;
                }
            }
        }
        None
    })
}

// How to derive per-pixel alpha from an 8bpp palette entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaletteAlpha {
    // Ignore palette alpha, everything opaque (general 0x91 bitmaps: maps, zones).
    Opaque,
    // Palette alpha as stored (0xB1 AlphaBitmap).
    Raw,
    // FFXI half-range alpha (0..=128, 128 = opaque) scaled to 0..=255. Item
    // icons are flag 0x91 but their palette carries this: index 0 = A0
    // (transparent background), art pixels = A128.
    HalfScaled,
}

#[allow(clippy::too_many_arguments)]
fn decode_paletted_bitmap(
    bytes: &[u8],
    width: u32,
    height: u32,
    bit_count: u16,
    used_colors: u32,
    alpha: PaletteAlpha,
    top_down: bool,
    compression: u32,
) -> Result<(Vec<u8>, usize)> {
    if compression != 0 {
        return Err(DatError::Mmb(format!(
            "paletted bitmap with compression={compression}: RLE not implemented (FFXI maps observed so far are BI_RGB)"
        )));
    }
    if bit_count != 8 {
        return Err(DatError::Mmb(format!(
            "paletted bitmap with bit_count={bit_count}: only 8bpp is implemented today \
             (24bpp / 32bpp packed bitmaps land when first encountered)",
        )));
    }

    let used_colors = if used_colors == 0 { 256 } else { used_colors };
    if used_colors > 256 {
        return Err(DatError::Mmb(format!(
            "paletted bitmap: implausible used_colors={used_colors} for 8bpp (expected 1..=256)"
        )));
    }
    let palette_len = (used_colors as usize) * 4;
    if bytes.len() < palette_len {
        return Err(DatError::Mmb(format!(
            "paletted bitmap: palette truncated (need {palette_len} bytes, have {})",
            bytes.len()
        )));
    }
    let palette = &bytes[..palette_len];

    let row_stride = (width as usize * bit_count as usize).div_ceil(32) * 4;
    let pixel_data_len = row_stride * height as usize;
    if bytes.len() < palette_len + pixel_data_len {
        return Err(DatError::Mmb(format!(
            "paletted bitmap: pixel data truncated (need {} bytes, have {})",
            palette_len + pixel_data_len,
            bytes.len() - palette_len
        )));
    }
    let pixels = &bytes[palette_len..palette_len + pixel_data_len];

    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let palette_entries = used_colors as usize;
    for y in 0..height as usize {
        let src_y = if top_down { y } else { height as usize - 1 - y };
        let src_row = &pixels[src_y * row_stride..src_y * row_stride + width as usize];
        for (x, &pal_byte) in src_row.iter().enumerate() {
            let idx = pal_byte as usize;
            if idx >= palette_entries {
                return Err(DatError::Mmb(format!(
                    "paletted bitmap: pixel index {idx} out of palette range (used_colors={used_colors})"
                )));
            }
            let pal_off = idx * 4;

            let dst = (y * width as usize + x) * 4;
            rgba[dst] = palette[pal_off + 2];
            rgba[dst + 1] = palette[pal_off + 1];
            rgba[dst + 2] = palette[pal_off];
            let a = palette[pal_off + 3];
            rgba[dst + 3] = match alpha {
                PaletteAlpha::Opaque => 0xFF,
                PaletteAlpha::Raw => a,
                PaletteAlpha::HalfScaled => ((a as u16) * 2).min(255) as u8,
            };
        }
    }
    Ok((rgba, palette_len + pixel_data_len))
}

fn decode_packed_bgra32(
    bytes: &[u8],
    width: u32,
    height: u32,
    top_down: bool,
    compression: u32,
) -> Result<(Vec<u8>, usize)> {
    if compression != 0 {
        return Err(DatError::Mmb(format!(
            "32bpp bitmap with compression={compression}: only BI_RGB (uncompressed) is supported"
        )));
    }

    let row_stride = width as usize * 4;
    let pixel_data_len = row_stride * height as usize;
    if bytes.len() < pixel_data_len {
        return Err(DatError::Mmb(format!(
            "32bpp bitmap: pixel data truncated (need {pixel_data_len} bytes, have {})",
            bytes.len()
        )));
    }

    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for y in 0..height as usize {
        let src_y = if top_down { y } else { height as usize - 1 - y };
        let src = src_y * row_stride;
        for x in 0..width as usize {
            let s = src + x * 4;
            let dst = (y * width as usize + x) * 4;
            rgba[dst] = bytes[s + 2];
            rgba[dst + 1] = bytes[s + 1];
            rgba[dst + 2] = bytes[s];

            rgba[dst + 3] = ((bytes[s + 3] as u16) * 2).min(255) as u8;
        }
    }
    Ok((rgba, pixel_data_len))
}

#[inline]
fn read_ascii_field(bytes: &[u8]) -> String {
    let s: String = bytes
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect();
    s.trim_end().to_string()
}

#[inline]
fn read_u32_le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

#[inline]
fn read_u16_le(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

#[inline]
fn read_i32_le(b: &[u8]) -> i32 {
    i32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

#[allow(dead_code)]
fn _reserve_read_trait<R: Read>(_: &mut R) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphic_flag_recognizes_three_variants() {
        assert_eq!(GraphicFlag::from_u8(0x91), Some(GraphicFlag::Bitmap));
        assert_eq!(GraphicFlag::from_u8(0xA1), Some(GraphicFlag::Dxt));
        assert_eq!(GraphicFlag::from_u8(0xB1), Some(GraphicFlag::AlphaBitmap));
        assert_eq!(GraphicFlag::from_u8(0x00), None);
        assert_eq!(GraphicFlag::from_u8(0x90), None);
    }

    #[test]
    fn parse_graphic_returns_none_for_non_flag_byte() {
        let bytes = vec![0x00; 100];
        assert!(parse_graphic(&bytes).unwrap().is_none());
    }

    #[test]
    fn parse_graphic_errors_on_truncated_header() {
        let bytes = vec![0x91, 0x00, 0x00];
        assert!(parse_graphic(&bytes).is_err());
    }

    #[test]
    fn map_dat_for_zone_konschtat_is_5321() {
        assert_eq!(map_dat_for_zone(108), Some(5321));
    }

    #[test]
    fn map_count_for_psoxja_is_three() {
        assert_eq!(map_count_for_zone(167), 3);
        assert_eq!(map_dat_for(167, 0), Some(5401));
        assert_eq!(map_dat_for(167, 1), Some(5402));
        assert_eq!(map_dat_for(167, 2), Some(5403));
        assert_eq!(map_dat_for(167, 3), None);
    }

    #[test]
    fn map_dat_for_unknown_zone_returns_none() {
        assert_eq!(map_dat_for_zone(9999), None);
    }

    #[test]
    fn parse_graphic_errors_cleanly_on_zero_used_colors() {
        let mut bytes = vec![0x91u8];
        bytes.extend_from_slice(b"cat\0\0\0\0\0");
        bytes.extend_from_slice(b"img1\0\0\0\0");
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(&2i32.to_le_bytes());
        bytes.extend_from_slice(&2i32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&8u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());

        bytes.extend_from_slice(&[0u8; 32]);
        assert!(parse_graphic(&bytes).is_err());

        let v: Vec<_> = scan_graphics(&bytes).collect();
        assert!(v.is_empty());
    }

    #[test]
    fn parse_graphic_decodes_32bpp_packed_bgra() {
        let mut bytes = vec![0x91u8];
        bytes.extend_from_slice(b"sts_icon");
        bytes.extend_from_slice(b"st01_32 ");
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(&2i32.to_le_bytes());
        bytes.extend_from_slice(&2i32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());

        bytes.extend_from_slice(&[0x00, 0x00, 0xFF, 0x80]);
        bytes.extend_from_slice(&[0xFF, 0x00, 0x00, 0x00]);
        bytes.extend_from_slice(&[0x00, 0xFF, 0x00, 0x40]);
        bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x80]);

        let (img, _consumed) = parse_graphic(&bytes).unwrap().unwrap();
        assert_eq!(img.category, "sts_icon");
        assert_eq!((img.width, img.height), (2, 2));

        assert_eq!(&img.rgba[0..4], &[0x00, 0xFF, 0x00, 0x80]);
        assert_eq!(&img.rgba[4..8], &[0x00, 0x00, 0x00, 0xFF]);

        assert_eq!(&img.rgba[8..12], &[0xFF, 0x00, 0x00, 0xFF]);
        assert_eq!(&img.rgba[12..16], &[0x00, 0x00, 0xFF, 0x00]);
    }

    #[test]
    fn status_icon_at_indexes_blocks() {
        let mut graphic = vec![0x91u8];
        graphic.extend_from_slice(b"sts_icon");
        graphic.extend_from_slice(b"st01_32 ");
        graphic.extend_from_slice(&40u32.to_le_bytes());
        graphic.extend_from_slice(&1i32.to_le_bytes());
        graphic.extend_from_slice(&1i32.to_le_bytes());
        graphic.extend_from_slice(&1u16.to_le_bytes());
        graphic.extend_from_slice(&32u16.to_le_bytes());
        graphic.extend_from_slice(&[0u8; 24]);
        graphic.extend_from_slice(&[0x11, 0x22, 0x33, 0x80]);

        let mut sheet = vec![0u8; 2 * STATUS_ICON_BLOCK_STRIDE];
        let at = STATUS_ICON_BLOCK_STRIDE + STATUS_ICON_GRAPHIC_OFFSET;
        sheet[at..at + graphic.len()].copy_from_slice(&graphic);

        let img = status_icon_at(&sheet, 1).expect("block 1 decodes");
        assert_eq!((img.width, img.height), (1, 1));
        assert_eq!(&img.rgba[0..4], &[0x33, 0x22, 0x11, 0xFF]);

        assert!(status_icon_at(&sheet, STATUS_ICON_COUNT as u16).is_none());
    }

    #[test]
    fn parse_graphic_decodes_dxt1() {
        let mut bytes = vec![0xA1u8];
        bytes.extend_from_slice(b"cat\0\0\0\0\0");
        bytes.extend_from_slice(b"map1\0\0\0\0");
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(&4i32.to_le_bytes());
        bytes.extend_from_slice(&4i32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 24]);
        bytes.extend_from_slice(b"1TXD");
        bytes.extend_from_slice(&[0u8; 8]);
        bytes.extend_from_slice(&0xF800u16.to_le_bytes());
        bytes.extend_from_slice(&0x001Fu16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());

        let (img, consumed) = parse_graphic(&bytes)
            .unwrap()
            .expect("dxt1 graphic decodes");
        assert_eq!((img.width, img.height), (4, 4));
        assert_eq!(img.rgba.len(), 4 * 4 * 4);
        assert_eq!(&img.rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(consumed, 69 + 8);
    }

    #[test]
    fn parse_graphic_treats_zero_used_colors_as_256() {
        let mut bytes = vec![0xB1u8];
        bytes.extend_from_slice(b"cat\0\0\0\0\0");
        bytes.extend_from_slice(b"map1\0\0\0\0");
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(&2i32.to_le_bytes());
        bytes.extend_from_slice(&2i32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&8u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());

        let mut palette = vec![0u8; 256 * 4];
        palette[0] = 0xFF;
        bytes.extend_from_slice(&palette);
        bytes.extend_from_slice(&[0u8; 8]);

        let (img, _) = parse_graphic(&bytes)
            .unwrap()
            .expect("8bpp graphic with used_colors=0 decodes as 256-color");
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(&img.rgba[0..3], &[0, 0, 255]);
    }

    #[test]
    fn parse_graphic_decodes_minimal_2x2_paletted() {
        let mut bytes = vec![0x91u8];
        bytes.extend_from_slice(b"cat\0\0\0\0\0");
        bytes.extend_from_slice(b"img1\0\0\0\0");
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(&2i32.to_le_bytes());
        bytes.extend_from_slice(&(-2i32).to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&8u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());

        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(&[0, 0, 0xFF, 0]);

        bytes.extend_from_slice(&[0u8, 1, 0, 0]);
        bytes.extend_from_slice(&[1u8, 0, 0, 0]);

        let (img, _consumed) = parse_graphic(&bytes).unwrap().unwrap();
        assert_eq!(img.category, "cat");
        assert_eq!(img.id, "img1");
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 2);

        assert_eq!(&img.rgba[0..4], &[0, 0, 0, 0xFF]);
        assert_eq!(&img.rgba[4..8], &[0xFF, 0, 0, 0xFF]);

        assert_eq!(&img.rgba[8..12], &[0xFF, 0, 0, 0xFF]);
        assert_eq!(&img.rgba[12..16], &[0, 0, 0, 0xFF]);
    }
}
