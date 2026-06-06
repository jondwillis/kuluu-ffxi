//! Parser for FFXI's 2D "Graphic" chunk — the format used by the
//! retail in-game map textures (the `Ctrl+M` minimap bitmaps).
//!
//! Distinct from the 3D texture IMG chunk in [`crate::texture`]: that
//! format is for DXT-compressed model textures, this one is for
//! BMP-style 2D images (UI icons, in-game maps, status-effect glyphs).
//!
//! # Spec (derived as algorithmic reference from POLUtils
//! `PlayOnline.FFXI/Things/Graphic.cs`, Apache 2.0):
//!
//! ```text
//! offset  size  field
//! 0       1     flag    — 0x91 / 0xA1 / 0xB1; see [`GraphicFlag`]
//! 1       8     category — ASCII, space-padded
//! 9       8     id       — ASCII, space-padded
//! 17      4     bmi_size — must be 40 (== sizeof BITMAPINFOHEADER)
//! 21      40    BITMAPINFOHEADER (Windows standard)
//! 61      …     pixel data — format depends on `flag` + `bit_count`
//! ```
//!
//! For map images the typical pixel layout is:
//!   * flag = 0x91 or 0xB1 (bitmap; 0xB1 carries an alpha channel)
//!   * bit_count = 8 (palettized) — palette follows as `used_colors *
//!     4` bytes of BGRA, then the indexed pixel rows
//!   * height field is **negative** in some FFXI bitmaps (top-down
//!     row order), positive in others (bottom-up); this parser
//!     normalizes both into top-down output
//!
//! # DXT variant (`flag = 0xA1`)
//!
//! Not implemented yet — minimap DATs in the cases sampled so far are
//! all 8bpp paletted. When a DXT-flavored map is found in the wild
//! the existing [`crate::texture`] decoders can handle the block
//! data; this module just needs to recognize the flag and route.
//!
//! # AGPL note
//!
//! Derived from POLUtils (Apache 2.0). Not derived from xi-tinkerer
//! or any AGPL-3 source — keeps `ffxi-dat` free of viral-license
//! exposure that would prevent linking from the `ffxi-mcp` crate.

use std::io::Read;

use crate::{DatError, Result};

// Compile-time generated table of `(zone_id, map_index, file_id)`
// derived from POLUtils' ROMFileMappings.xml. See `ffxi-dat/build.rs`.
include!(concat!(env!("OUT_DIR"), "/map_dat_table.rs"));

/// Resolve the retail map-DAT file_id for a given zone, defaulting to
/// the primary map (map_index 0). Returns `None` when the zone has
/// no entry in POLUtils' catalog — that happens for zones added after
/// POLUtils froze (post-WotG/SoA expansion content) and for system
/// zones that have no in-game map.
pub fn map_dat_for_zone(zone_id: u16) -> Option<u32> {
    map_dat_for(zone_id, 0)
}

/// Resolve a specific floor / sub-map by `map_index`. Most overworld
/// zones have only index 0. Multi-floor dungeons (Castle Zvahl, Pso'Xja,
/// Garlaige Citadel, …) define indices 1, 2, …
pub fn map_dat_for(zone_id: u16, map_index: u8) -> Option<u32> {
    // Binary search on the (zone_id, map_index) key.
    MAP_DAT_TABLE
        .binary_search_by(|(z, m, _)| (*z, *m).cmp(&(zone_id, map_index)))
        .ok()
        .map(|i| MAP_DAT_TABLE[i].2)
}

/// Number of distinct maps for a zone, or 0 when the zone isn't in
/// the table. Useful for cycling through floors with `/minimap floor`
/// (future work).
pub fn map_count_for_zone(zone_id: u16) -> usize {
    MAP_DAT_TABLE
        .iter()
        .filter(|(z, _, _)| *z == zone_id)
        .count()
}

/// Retail status-effect icon sheet. `DatRoot::resolve(STATUS_ICON_FILE_ID)`
/// lands on `ROM/119/57.DAT` on the dev install — a flat array of 640
/// fixed-size blocks, one per `status_id` (the same id space the 0x063
/// STATUS_ICONS packet carries, and HXUI's `0.png..639.png` theme files).
///
/// Each block embeds a standard [`parse_graphic`] chunk (flag `0x91`,
/// category `sts_icon`, 32×32, 32bpp BGRA) at byte `STATUS_ICON_GRAPHIC_OFFSET`
/// within the block. The file is neither chunk-framed (`crate::walk`
/// misses it) nor paletted (`scan_graphics` misses it), so index by block.
pub const STATUS_ICON_FILE_ID: u32 = 87;
/// Bytes per icon block in the status-icon sheet.
pub const STATUS_ICON_BLOCK_STRIDE: usize = 0x1800;
/// Offset of the `0x91` Graphic flag byte within each block.
pub const STATUS_ICON_GRAPHIC_OFFSET: usize = 0x284;
/// Number of icon blocks in the sheet (`status_id` 0..=639).
pub const STATUS_ICON_COUNT: usize = 640;

/// Decode the status-effect icon for `status_id` from the raw bytes of
/// the status-icon DAT (`STATUS_ICON_FILE_ID`). Returns `None` when the
/// id is out of range, the block is truncated, or the embedded Graphic
/// doesn't parse (e.g. an empty/placeholder slot).
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

/// Graphic chunk flag byte. Decides which pixel-data layout follows
/// the BITMAPINFOHEADER.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicFlag {
    /// Plain bitmap (paletted, no alpha channel).
    Bitmap = 0x91,
    /// DXT-compressed texture. Not handled by [`parse_graphic`] yet
    /// — caller gets [`DatError::Mmb`] (overloaded) until DXT routing
    /// lands.
    Dxt = 0xA1,
    /// Alpha-bitmap (paletted, palette entries carry alpha in the
    /// 4th byte instead of the standard 0).
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

/// Decoded 2D graphic ready to upload as an RGBA8 texture. `rgba`
/// length is always `width * height * 4`. Top-down row order
/// regardless of the on-disk BITMAPINFOHEADER convention.
#[derive(Debug, Clone)]
pub struct GraphicImage {
    pub category: String,
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Parse a single Graphic chunk starting at `bytes[0]`. Returns the
/// decoded image plus the number of bytes consumed so the caller can
/// scan a DAT containing multiple back-to-back graphics.
///
/// Bytes must start with one of the valid flag bytes; mismatched
/// flags return `Ok(None)` so the scanner can advance by one byte
/// and retry (matches POLUtils' behavior).
pub fn parse_graphic(bytes: &[u8]) -> Result<Option<(GraphicImage, usize)>> {
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

    // Sanity check the BITMAPINFO matches POLUtils' Graphic.Read:
    // Planes must be 1; width/height must be in (0, 16384].
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
            // 8bpp ships a palette; 32bpp ships packed BGRA inline. FFXI's
            // status-effect icons (`sts_icon`, ROM/119/57.DAT) are the
            // 32bpp case — the 8bpp `decode_paletted_bitmap` rejects them
            // (`used_colors == 0`, no palette), so route by `bit_count`.
            let (rgba, consumed) = if bit_count == 32 {
                decode_packed_bgra32(&bytes[57..], width_u, height_u, top_down, compression)?
            } else {
                let with_alpha = matches!(flag, GraphicFlag::AlphaBitmap);
                decode_paletted_bitmap(
                    &bytes[57..],
                    width_u,
                    height_u,
                    bit_count,
                    used_colors,
                    with_alpha,
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
        GraphicFlag::Dxt => Err(DatError::Mmb(format!(
            "graphic chunk `{category}/{id}`: DXT (flag 0xA1) decode not yet implemented — \
             route through ffxi-dat::texture decoders when wiring this up",
        ))),
    }
}

/// Iterator over every Graphic chunk in `bytes`. Mirrors POLUtils'
/// `Images.Load`: tries to parse at each offset; on failure, advances
/// by one byte and tries again. Skips silently over the parts of the
/// file that aren't graphics (DAT framing, padding).
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

/// Decode the palette + indexed-pixel block. Layout (per
/// POLUtils + Windows BMP convention):
///
///   * palette: `used_colors * 4` bytes, BGRA order (alpha is 0 for
///     flag 0x91; carries the actual alpha for 0xB1)
///   * indexed pixels: `width * height * bit_count / 8` bytes, with
///     each scanline padded to a 4-byte boundary (BMP convention)
///
/// `compression != 0` would mean run-length encoding (BI_RLE8 / RLE4)
/// — not seen in FFXI maps in samples so far, so unimplemented.
#[allow(clippy::too_many_arguments)]
fn decode_paletted_bitmap(
    bytes: &[u8],
    width: u32,
    height: u32,
    bit_count: u16,
    used_colors: u32,
    with_alpha: bool,
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
    // `scan_graphics` walks the DAT byte-by-byte, so we'll routinely
    // see random offsets that pass the BITMAPINFO sanity check by
    // chance. `used_colors == 0` is the tell — POLUtils' Graphic.Read
    // takes the count literally (it doesn't honor the BMP-standard
    // "0 means 2^bit_count" convention), so real FFXI graphics never
    // ship `used_colors == 0`. Refuse here so the scanner's
    // advance-by-one fallback kicks in instead of panicking on an
    // empty palette slice below.
    if used_colors == 0 || used_colors > 256 {
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
        // BMP rows are bottom-up unless height was negative (top_down).
        let src_y = if top_down { y } else { height as usize - 1 - y };
        let src_row = &pixels[src_y * row_stride..src_y * row_stride + width as usize];
        for (x, &pal_byte) in src_row.iter().enumerate() {
            let idx = pal_byte as usize;
            if idx >= palette_entries {
                // Same scanner-resilience rationale as the
                // `used_colors == 0` check above: an indexed byte
                // pointing past the palette is a strong signal we're
                // decoding a false-positive header, not a real chunk.
                return Err(DatError::Mmb(format!(
                    "paletted bitmap: pixel index {idx} out of palette range (used_colors={used_colors})"
                )));
            }
            let pal_off = idx * 4;
            // BGRA on disk → RGBA in output. Alpha is 0 for flag
            // 0x91 (interpret as opaque); use the byte directly for
            // 0xB1.
            let dst = (y * width as usize + x) * 4;
            rgba[dst] = palette[pal_off + 2]; // R
            rgba[dst + 1] = palette[pal_off + 1]; // G
            rgba[dst + 2] = palette[pal_off]; // B
            rgba[dst + 3] = if with_alpha {
                palette[pal_off + 3]
            } else {
                0xFF
            };
        }
    }
    Ok((rgba, palette_len + pixel_data_len))
}

/// Decode a 32bpp packed BGRA bitmap (no palette). Used by FFXI's
/// status-effect icons. Each pixel is 4 bytes on disk in B, G, R, A
/// order; rows are bottom-up unless `top_down`. Alpha is FFXI's 7-bit
/// convention (0..=0x80), scaled to 8-bit by `(a * 2).min(255)` — the
/// same rule [`crate::texture`] applies to DXT alpha.
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
    // 32bpp rows are inherently 4-byte aligned, so the BMP scanline
    // padding is a no-op; stride == width * 4.
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
            rgba[dst] = bytes[s + 2]; // R
            rgba[dst + 1] = bytes[s + 1]; // G
            rgba[dst + 2] = bytes[s]; // B
                                      // FFXI 7-bit alpha (0..=0x80) → 8-bit.
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

// Unused import workaround — `std::io::Read` is reserved for the
// next iteration that streams from a `BinaryReader`-style cursor
// over the DAT chunk body.
#[allow(dead_code)]
fn _reserve_read_trait<R: Read>(_: &mut R) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recognize the three valid flag bytes; reject everything else.
    #[test]
    fn graphic_flag_recognizes_three_variants() {
        assert_eq!(GraphicFlag::from_u8(0x91), Some(GraphicFlag::Bitmap));
        assert_eq!(GraphicFlag::from_u8(0xA1), Some(GraphicFlag::Dxt));
        assert_eq!(GraphicFlag::from_u8(0xB1), Some(GraphicFlag::AlphaBitmap));
        assert_eq!(GraphicFlag::from_u8(0x00), None);
        assert_eq!(GraphicFlag::from_u8(0x90), None);
    }

    /// `parse_graphic` returns `Ok(None)` on a non-graphic byte so the
    /// `scan_graphics` advance-by-one fallback can keep going.
    #[test]
    fn parse_graphic_returns_none_for_non_flag_byte() {
        let bytes = vec![0x00; 100];
        assert!(parse_graphic(&bytes).unwrap().is_none());
    }

    /// `parse_graphic` errors out on a truncated header instead of
    /// silently returning `None` — protects against subtle bugs
    /// where a real graphic at the EOF gets misclassified.
    #[test]
    fn parse_graphic_errors_on_truncated_header() {
        let bytes = vec![0x91, 0x00, 0x00]; // flag + 2 bytes — way short
        assert!(parse_graphic(&bytes).is_err());
    }

    /// Konschtat Highlands is zone-id 108 and per POLUtils maps to
    /// DAT file_id 5321 (verified against a fresh extraction of
    /// `ROMFileMappings.xml` at the time this test was written).
    /// If the build.rs scraper drifts or POLUtils' XML reshapes,
    /// this test will catch it before the runtime loader hits the
    /// wrong file.
    #[test]
    fn map_dat_for_zone_konschtat_is_5321() {
        assert_eq!(map_dat_for_zone(108), Some(5321));
    }

    /// Pso'Xja (zone 167) is multi-floor — 3 maps. Confirms the
    /// "category with nested rom-files" scraper path produced
    /// distinct indices.
    #[test]
    fn map_count_for_psoxja_is_three() {
        assert_eq!(map_count_for_zone(167), 3);
        assert_eq!(map_dat_for(167, 0), Some(5401));
        assert_eq!(map_dat_for(167, 1), Some(5402));
        assert_eq!(map_dat_for(167, 2), Some(5403));
        assert_eq!(map_dat_for(167, 3), None);
    }

    /// Zones outside POLUtils' catalog return None — the retail
    /// backend then no-ops and Auto-mode falls back to top-down.
    #[test]
    fn map_dat_for_unknown_zone_returns_none() {
        assert_eq!(map_dat_for_zone(9999), None);
    }

    /// `scan_graphics` must not panic when a random offset in the DAT
    /// happens to satisfy every BITMAPINFO sanity check by chance but
    /// has `used_colors == 0`. Prior to the guard in
    /// [`decode_paletted_bitmap`], this triggered an index-out-of-bounds
    /// against an empty palette slice and crashed the Bevy Main schedule
    /// on zone-in. The scanner-resilience contract requires `Err`, not
    /// panic, so the advance-by-one fallback can keep walking.
    #[test]
    fn parse_graphic_errors_cleanly_on_zero_used_colors() {
        let mut bytes = vec![0x91u8];
        bytes.extend_from_slice(b"cat\0\0\0\0\0");
        bytes.extend_from_slice(b"img1\0\0\0\0");
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(&2i32.to_le_bytes()); // width
        bytes.extend_from_slice(&2i32.to_le_bytes()); // height
        bytes.extend_from_slice(&1u16.to_le_bytes()); // planes
        bytes.extend_from_slice(&8u16.to_le_bytes()); // bit_count
        bytes.extend_from_slice(&0u32.to_le_bytes()); // compression
        bytes.extend_from_slice(&0u32.to_le_bytes()); // image_size
        bytes.extend_from_slice(&0u32.to_le_bytes()); // x_pels
        bytes.extend_from_slice(&0u32.to_le_bytes()); // y_pels
        bytes.extend_from_slice(&0u32.to_le_bytes()); // used_colors = 0 (the trigger)
        bytes.extend_from_slice(&0u32.to_le_bytes()); // important_colors
                                                      // Pad out with enough trailing bytes that the header passes
                                                      // the "bytes.len() >= 61" gate and we reach the panic site
                                                      // pre-fix.
        bytes.extend_from_slice(&[0u8; 32]);
        assert!(parse_graphic(&bytes).is_err());
        // And the scanner must drain to completion without panicking
        // — proves the Err is routed into the advance-by-one path.
        let v: Vec<_> = scan_graphics(&bytes).collect();
        assert!(v.is_empty());
    }

    /// Synthetic 2×2 32bpp packed-BGRA bitmap (bottom-up, the FFXI
    /// status-icon case). Validates the BGRA→RGBA swap, the 7-bit
    /// alpha doubling, and bottom-up row normalization.
    #[test]
    fn parse_graphic_decodes_32bpp_packed_bgra() {
        let mut bytes = vec![0x91u8];
        bytes.extend_from_slice(b"sts_icon"); // 8-byte category
        bytes.extend_from_slice(b"st01_32 "); // 8-byte id
        bytes.extend_from_slice(&40u32.to_le_bytes()); // bmi_size
        bytes.extend_from_slice(&2i32.to_le_bytes()); // width
        bytes.extend_from_slice(&2i32.to_le_bytes()); // height (positive = bottom-up)
        bytes.extend_from_slice(&1u16.to_le_bytes()); // planes
        bytes.extend_from_slice(&32u16.to_le_bytes()); // bit_count = 32
        bytes.extend_from_slice(&0u32.to_le_bytes()); // compression
        bytes.extend_from_slice(&0u32.to_le_bytes()); // image_size
        bytes.extend_from_slice(&0u32.to_le_bytes()); // x_pels
        bytes.extend_from_slice(&0u32.to_le_bytes()); // y_pels
        bytes.extend_from_slice(&0u32.to_le_bytes()); // used_colors = 0 (no palette)
        bytes.extend_from_slice(&0u32.to_le_bytes()); // important_colors
                                                      // Pixels, BGRA, bottom-up. Disk row 0 = bottom output row.
                                                      // alpha 0x80 (=128) must scale to 255 (opaque).
        bytes.extend_from_slice(&[0x00, 0x00, 0xFF, 0x80]); // (0,0) disk: red, opaque
        bytes.extend_from_slice(&[0xFF, 0x00, 0x00, 0x00]); // (1,0) disk: blue, transparent
        bytes.extend_from_slice(&[0x00, 0xFF, 0x00, 0x40]); // (0,1) disk: green, alpha 64→128
        bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x80]); // (1,1) disk: black, opaque

        let (img, _consumed) = parse_graphic(&bytes).unwrap().unwrap();
        assert_eq!(img.category, "sts_icon");
        assert_eq!((img.width, img.height), (2, 2));
        // Output row 0 is disk's bottom row (disk row 1): green, black.
        assert_eq!(&img.rgba[0..4], &[0x00, 0xFF, 0x00, 0x80]); // green, 64→128
        assert_eq!(&img.rgba[4..8], &[0x00, 0x00, 0x00, 0xFF]); // black, opaque
                                                                // Output row 1 is disk's top row (disk row 0): red, blue.
        assert_eq!(&img.rgba[8..12], &[0xFF, 0x00, 0x00, 0xFF]); // red, opaque
        assert_eq!(&img.rgba[12..16], &[0x00, 0x00, 0xFF, 0x00]); // blue, transparent
    }

    /// `status_icon_at` indexes block `N` and parses the embedded
    /// Graphic at the fixed sub-offset. Builds a minimal two-block
    /// sheet and checks id 1 resolves to the second block's icon.
    #[test]
    fn status_icon_at_indexes_blocks() {
        // One real 1×1 32bpp Graphic, placed at GRAPHIC_OFFSET in block 1.
        let mut graphic = vec![0x91u8];
        graphic.extend_from_slice(b"sts_icon");
        graphic.extend_from_slice(b"st01_32 ");
        graphic.extend_from_slice(&40u32.to_le_bytes());
        graphic.extend_from_slice(&1i32.to_le_bytes()); // width 1
        graphic.extend_from_slice(&1i32.to_le_bytes()); // height 1
        graphic.extend_from_slice(&1u16.to_le_bytes());
        graphic.extend_from_slice(&32u16.to_le_bytes());
        graphic.extend_from_slice(&[0u8; 24]); // comp..important
        graphic.extend_from_slice(&[0x11, 0x22, 0x33, 0x80]); // B,G,R,A

        let mut sheet = vec![0u8; 2 * STATUS_ICON_BLOCK_STRIDE];
        let at = STATUS_ICON_BLOCK_STRIDE + STATUS_ICON_GRAPHIC_OFFSET;
        sheet[at..at + graphic.len()].copy_from_slice(&graphic);

        let img = status_icon_at(&sheet, 1).expect("block 1 decodes");
        assert_eq!((img.width, img.height), (1, 1));
        assert_eq!(&img.rgba[0..4], &[0x33, 0x22, 0x11, 0xFF]); // R,G,B,A(opaque)
                                                                // Out-of-range id is None, not a panic.
        assert!(status_icon_at(&sheet, STATUS_ICON_COUNT as u16).is_none());
    }

    /// Synthetic 2×2 paletted bitmap with a top-down row order
    /// (height = -2). Validates the palette indexing + BGRA→RGBA
    /// swap + row order normalization end-to-end.
    #[test]
    fn parse_graphic_decodes_minimal_2x2_paletted() {
        // Build: flag=0x91, category="cat", id="img1", BMI=40,
        // width=2, height=-2 (top-down), planes=1, bit_count=8,
        // compression=0, image_size=0, pels=0, used_colors=2,
        // important=0; then palette: 2 BGRA entries; then pixels:
        // row stride = ceil(2 * 8 / 32) * 4 = 4 bytes per row.
        let mut bytes = vec![0x91u8];
        bytes.extend_from_slice(b"cat\0\0\0\0\0"); // 8 bytes category
        bytes.extend_from_slice(b"img1\0\0\0\0"); // 8 bytes id
        bytes.extend_from_slice(&40u32.to_le_bytes()); // bmi_size
        bytes.extend_from_slice(&2i32.to_le_bytes()); // width
        bytes.extend_from_slice(&(-2i32).to_le_bytes()); // negative = top-down
        bytes.extend_from_slice(&1u16.to_le_bytes()); // planes
        bytes.extend_from_slice(&8u16.to_le_bytes()); // bit_count
        bytes.extend_from_slice(&0u32.to_le_bytes()); // compression
        bytes.extend_from_slice(&0u32.to_le_bytes()); // image_size
        bytes.extend_from_slice(&0u32.to_le_bytes()); // x_pels
        bytes.extend_from_slice(&0u32.to_le_bytes()); // y_pels
        bytes.extend_from_slice(&2u32.to_le_bytes()); // used_colors
        bytes.extend_from_slice(&0u32.to_le_bytes()); // important_colors
                                                      // Palette: index 0 = black, index 1 = bright red.
                                                      // BGRA on disk: B, G, R, A.
        bytes.extend_from_slice(&[0, 0, 0, 0]); // black
        bytes.extend_from_slice(&[0, 0, 0xFF, 0]); // red (B=0, G=0, R=255, A=0)
                                                   // Pixel rows, top-down: row 0 = [0, 1, padding..], row 1 = [1, 0, padding..]
        bytes.extend_from_slice(&[0u8, 1, 0, 0]); // row 0 + 2 bytes pad to 4-byte stride
        bytes.extend_from_slice(&[1u8, 0, 0, 0]); // row 1 + 2 bytes pad

        let (img, _consumed) = parse_graphic(&bytes).unwrap().unwrap();
        assert_eq!(img.category, "cat");
        assert_eq!(img.id, "img1");
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 2);
        // Row 0 pixel 0 = black, row 0 pixel 1 = red.
        assert_eq!(&img.rgba[0..4], &[0, 0, 0, 0xFF]); // alpha forced to 0xFF for 0x91
        assert_eq!(&img.rgba[4..8], &[0xFF, 0, 0, 0xFF]);
        // Row 1 pixel 0 = red, row 1 pixel 1 = black.
        assert_eq!(&img.rgba[8..12], &[0xFF, 0, 0, 0xFF]);
        assert_eq!(&img.rgba[12..16], &[0, 0, 0, 0xFF]);
    }
}
