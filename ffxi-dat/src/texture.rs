//! FFXI texture chunk parser (kind 0x20 = IMG container).
//!
//! Two layers:
//!   1. Format detection: scan for the FourCC magic FFXI uses
//!      (`"3TXD"` = DXT3, `"1TXD"` = DXT1, `"BGRA"`/`"ARGB"` = uncompressed).
//!   2. Pixel decode: BC1 (DXT1) / BC2 (DXT3) → RGBA8, plus straight
//!      byte rearrangement for the uncompressed variants.
//!
//! # IMG chunk body layout (best-effort, partially spec-based)
//!
//! The IMG chunk body (after the 16-byte chunk header that `walk()`
//! strips) appears to begin with an FFXI-specific texture header, then
//! the standard FourCC magic, then either a small format-specific
//! sub-header carrying width/height/mip-count, then raw pixel data.
//!
//!   offset 0..16       inner header — `0xa1` flag + 4-byte name "tim "
//!                      + padded ASCII variant ("hm_ba1_1...")
//!   offset 0x10..0x30  16-byte ASCII variant name + padding (mip/usage flags)
//!   offset 0x30..0x34  texture meta byte + 4-byte format magic
//!   offset 0x34..0x38  FourCC magic (`3TXD` / `1TXD` / `BGRA` / `ARGB`)
//!   offset 0x38..      format-specific sub-header:
//!     - DXT*: u32 width, u32 height (LE), then compressed blocks
//!     - BGRA/ARGB: u32 width, u32 height (LE), then `w*h*4` bytes
//!
//! ## Provenance / caveats
//!
//! The 0x30 / 0x34 offsets are from earlier empirical inspection of
//! file 7368 chunk 2 (`hm_b` helmet texture). The width/height
//! placement after the FourCC is **spec-based, not verified against
//! real DATs** in this commit — no FFXI install was reachable from the
//! sandbox to walk a known texture. Treat the [`decode_texture`] entry
//! point as best-effort for FFXI-specific framing; the *block-level*
//! BC1/BC2/BGRA/ARGB decoders below are spec-correct and unit-tested
//! against synthetic fixtures.
//!
//! Callers that already know `width` and `height` from a surrounding
//! parser (e.g. the MMB material section) can bypass FFXI framing
//! entirely and call [`decode_dxt1_blocks`], [`decode_dxt3_blocks`],
//! [`decode_bgra_raw`], or [`decode_argb_raw`] directly.
//!
//! References:
//!   - DXT block bit-layout: MS DirectX Graphics Programming Guide
//!     <https://learn.microsoft.com/en-us/windows/win32/direct3d10/d3d10-graphics-programming-guide-resources-block-compression>
//!   - POLUtils `PlayOnline.FFXI/Things/Graphic.cs` (Apache-2.0) covers
//!     the BMP-style 2D images; the IMG chunk in MMB/MZB uses the
//!     framing above, distinct from the 2D image format.

use crate::Result;

/// FFXI texture pixel format (subset of the broader DDS surface formats).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TexFormat {
    /// DXT1 / BC1 — 4 bpp, RGB(A) with 1-bit alpha.
    Dxt1,
    /// DXT3 / BC2 — 8 bpp, RGBA with explicit 4-bit alpha (FFXI default).
    Dxt3,
    /// Uncompressed 32-bit B8G8R8A8 (FFXI byte order on disk).
    Bgra32,
    /// Uncompressed 32-bit A8R8G8B8 (rare in FFXI).
    Argb32,
}

impl TexFormat {
    /// Match the FourCC magic FFXI uses for the format header.
    /// FFXI stores these reversed-endian — `"3TXD"` decrypted as ASCII
    /// = `"DXT3"` if you flip the byte order.
    pub fn from_magic(magic: &[u8]) -> Option<Self> {
        match magic {
            b"3TXD" => Some(Self::Dxt3),
            b"1TXD" => Some(Self::Dxt1),
            b"BGRA" => Some(Self::Bgra32),
            b"ARGB" => Some(Self::Argb32),
            _ => None,
        }
    }
}

/// Decoded RGBA8 texture surface (top mip only, sRGB-assumed).
///
/// `rgba.len()` is always `width * height * 4`. Alpha is NOT premultiplied.
#[derive(Debug, Clone)]
pub struct DecodedTexture {
    pub width: u32,
    pub height: u32,
    pub format_tag: TexFormat,
    pub rgba: Vec<u8>,
}

/// Extract the 8-byte internal texture name from an `flg=0xA1` IMG body.
/// The `id[16]` field holds `"model   <name>"` (8-char type tag + 8-char
/// name); we return just the name portion (last 8 bytes, space-trimmed).
/// Returns `None` if the body is too short or `flg != 0xA1`.
pub fn extract_texture_name(body: &[u8]) -> Option<String> {
    if body.len() < 0x11 || body[0] != 0xA1 {
        return None;
    }
    // id[16] sits at body[1..0x11]. The last 8 bytes are the asset name.
    let raw = &body[9..0x11];
    let s: String = raw
        .iter()
        .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '\0' })
        .take_while(|&c| c != '\0')
        .collect();
    Some(s.trim().to_string())
}

/// Errors raised by the texture pixel-decode path.
#[derive(Debug, thiserror::Error)]
pub enum TextureError {
    #[error("no recognised FFXI texture magic in chunk body")]
    NoMagic,
    #[error("chunk body truncated at offset {offset}: needed {needed} bytes, have {available}")]
    Truncated {
        offset: usize,
        needed: usize,
        available: usize,
    },
    #[error("invalid dimensions {width}x{height} (must be > 0 and multiple of 4 for DXT)")]
    BadDimensions { width: u32, height: u32 },
    #[error("dimensions {width}x{height} overflow usize when multiplied")]
    SizeOverflow { width: u32, height: u32 },
}

/// Locate the texture-format magic inside an IMG chunk body. Returns the
/// byte offset and the decoded format.
pub fn find_texture_format(body: &[u8]) -> Result<Option<(usize, TexFormat)>> {
    for (i, win) in body.windows(4).enumerate() {
        if let Some(fmt) = TexFormat::from_magic(win) {
            return Ok(Some((i, fmt)));
        }
    }
    Ok(None)
}

/// Decode an FFXI IMG chunk body into top-mip RGBA8 pixels.
///
/// Body layout for `flg=0xA1` (DXT-compressed, the variant used by zone
/// IMG chunks in MZB/MMB-bearing DATs):
///   - 0x00:        flg byte (0xA1 here)
///   - 0x01..0x11:  id[16] — ASCII texture name (e.g. `"model   s_bas_h"`)
///   - 0x11..0x15:  dwnazo1 DWORD (purpose unknown)
///   - 0x15..0x19:  imgx (width) i32 LE
///   - 0x19..0x1D:  imgy (height) i32 LE
///   - 0x1D..0x35:  dwnazo2[6] (24 bytes, purpose unknown)
///   - 0x35..0x39:  widthbyte DWORD
///   - 0x39..0x3D:  ddsType FourCC: `"3TXD"`/`"1TXD"`/`"5TXD"` reversed
///   - 0x3D..0x41:  size DWORD — pixel-data byte count
///   - 0x41..0x45:  noBlock DWORD
///   - 0x45..:      pixel data (`size` bytes, then padding to chunk end)
///
/// `sizeof(IMGINFOA1) = 69` (0x45). DXT3 sample verifies: a 128×256
/// chunk has body 32848 = 69 header + 32768 pixels (= 128*256 bytes for
/// DXT3) + 11 trailing padding bytes.
///
/// Reference: TeoTwawki/ffxi-dat-hacking `TDWAnalysis.h::IMGINFOA1` +
/// `DatLoader.cpp::extractImage` (flg dispatch).
pub fn decode_texture(body: &[u8]) -> std::result::Result<DecodedTexture, TextureError> {
    if body.is_empty() {
        return Err(TextureError::NoMagic);
    }
    match body[0] {
        0xA1 => decode_imginfo_a1(body),
        0x01 | 0x81 | 0x91 => decode_palettized(body, 0x39, 0x439),
        0xB1 => decode_palettized(body, 0x3D, 0x43D),
        // 0x05 (IMGINFO05, no embedded palette) and unknown flg values
        // — not yet decoded. Reference (TeoTwawki) records header
        // size but doesn't pin down the pixel interpretation; POLUtils
        // would, but is C# and not Rust-portable.
        _ => Err(TextureError::NoMagic),
    }
}

/// DXT-compressed IMGINFOA1 layout. See [`decode_texture`] for the
/// per-field offset table; this branch handles `flg = 0xA1`.
fn decode_imginfo_a1(body: &[u8]) -> std::result::Result<DecodedTexture, TextureError> {
    if body.len() < 0x45 {
        return Err(TextureError::Truncated {
            offset: 0,
            needed: 0x45,
            available: body.len(),
        });
    }
    let width = i32::from_le_bytes(body[0x15..0x19].try_into().unwrap());
    let height = i32::from_le_bytes(body[0x19..0x1D].try_into().unwrap());
    if width <= 0 || height <= 0 {
        return Err(TextureError::BadDimensions {
            width: width as u32,
            height: height as u32,
        });
    }
    let width = width as u32;
    let height = height as u32;

    let magic = &body[0x39..0x3D];
    let fmt = TexFormat::from_magic(magic).ok_or(TextureError::NoMagic)?;
    let pixel_off = 0x45usize;
    let pixels = &body[pixel_off..];
    let rgba = match fmt {
        TexFormat::Dxt1 => decode_dxt1_blocks(pixels, width, height)?,
        TexFormat::Dxt3 => decode_dxt3_blocks(pixels, width, height)?,
        TexFormat::Bgra32 => decode_bgra_raw(pixels, width, height)?,
        TexFormat::Argb32 => decode_argb_raw(pixels, width, height)?,
    };
    Ok(DecodedTexture { width, height, format_tag: fmt, rgba })
}

/// Palette-based IMG (flg=0x01/0x81/0x91 with header_size=0x439,
/// palette@0x39; flg=0xB1 with header_size=0x43D, palette@0x3D). 256
/// BGRA palette entries followed by `width*height` 8-bit indices.
///
/// Reports as `TexFormat::Bgra32` in the result tag since the decoded
/// surface is 32-bit RGBA — the upstream `format_tag` only drives
/// per-format pre-decode dispatch, which is no-op here.
fn decode_palettized(
    body: &[u8],
    palette_off: usize,
    pixel_off: usize,
) -> std::result::Result<DecodedTexture, TextureError> {
    let needed_header = pixel_off;
    if body.len() < needed_header {
        return Err(TextureError::Truncated {
            offset: 0,
            needed: needed_header,
            available: body.len(),
        });
    }
    let width = i32::from_le_bytes(body[0x15..0x19].try_into().unwrap());
    let height = i32::from_le_bytes(body[0x19..0x1D].try_into().unwrap());
    if width <= 0 || height <= 0 {
        return Err(TextureError::BadDimensions {
            width: width as u32,
            height: height as u32,
        });
    }
    let width = width as u32;
    let height = height as u32;

    // 256 × BGRA8 palette. Each on-disk DWORD is little-endian
    // (B, G, R, A). FFXI stores alpha as 7-bit (0..0x80); scale to 8-bit.
    let palette_bytes = &body[palette_off..palette_off + 256 * 4];
    let mut palette: [[u8; 4]; 256] = [[0; 4]; 256];
    for (i, entry) in palette.iter_mut().enumerate() {
        let o = i * 4;
        let b = palette_bytes[o];
        let g = palette_bytes[o + 1];
        let r = palette_bytes[o + 2];
        let a_raw = palette_bytes[o + 3];
        let a = ((a_raw as u16).saturating_mul(2)).min(255) as u8;
        *entry = [r, g, b, a];
    }

    let n_pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or(TextureError::SizeOverflow { width, height })?;
    if body.len() < pixel_off + n_pixels {
        return Err(TextureError::Truncated {
            offset: pixel_off,
            needed: n_pixels,
            available: body.len().saturating_sub(pixel_off),
        });
    }
    let indices = &body[pixel_off..pixel_off + n_pixels];
    let mut rgba: Vec<u8> = Vec::with_capacity(n_pixels * 4);
    for &idx in indices {
        rgba.extend_from_slice(&palette[idx as usize]);
    }
    Ok(DecodedTexture {
        width,
        height,
        format_tag: TexFormat::Bgra32,
        rgba,
    })
}

// ----------------------------------------------------------------------------
// DXT (BC1 / BC2) block decoders
//
// Both formats decode 4x4 pixel blocks. The COLOR block layout is shared:
//   bytes 0..2  : c0 — RGB565 LE
//   bytes 2..4  : c1 — RGB565 LE
//   bytes 4..8  : 16 × 2-bit indices into the 4-color palette (row-major,
//                 LSB = top-left). Order within each byte: low 2 bits are
//                 the leftmost pixel of the byte's 4-pixel row.
//
// BC1 (DXT1): block = 8 bytes (color block only). If c0 <= c1, the
//             4-color palette becomes 3 colors + 1 transparent (alpha=0).
//
// BC2 (DXT3): block = 16 bytes. First 8 bytes = explicit 4-bit alpha for
//             each of the 16 pixels (row-major, two pixels per byte, low
//             nibble first). Following 8 bytes = a BC1-style color block
//             that is ALWAYS interpreted with the 4-color (non-1bit-alpha)
//             rule, regardless of c0 vs c1 ordering.
//
// References:
//   https://learn.microsoft.com/en-us/windows/win32/direct3d10/d3d10-graphics-programming-guide-resources-block-compression
//   https://learn.microsoft.com/en-us/windows/win32/direct3d10/d3d10-graphics-programming-guide-resources-block-compression-bc1
//   https://learn.microsoft.com/en-us/windows/win32/direct3d10/d3d10-graphics-programming-guide-resources-block-compression-bc2
// ----------------------------------------------------------------------------

fn rgb565_to_rgb888(c: u16) -> (u8, u8, u8) {
    let r5 = ((c >> 11) & 0x1F) as u8;
    let g6 = ((c >> 5) & 0x3F) as u8;
    let b5 = (c & 0x1F) as u8;
    // Standard 5→8 and 6→8 expansion: replicate high bits into low.
    let r = (r5 << 3) | (r5 >> 2);
    let g = (g6 << 2) | (g6 >> 4);
    let b = (b5 << 3) | (b5 >> 2);
    (r, g, b)
}

fn lerp_u8(a: u8, b: u8, num: u32, den: u32) -> u8 {
    // Integer lerp matching the canonical DXT reference: ((den-num)*a + num*b) / den.
    (((den - num) * a as u32 + num * b as u32) / den) as u8
}

/// Decode a single DXT1 block (8 bytes) into 16 RGBA8 pixels (64 bytes).
///
/// `palette_punchthrough_alpha`: when true, the c0<=c1 case yields a
/// 3-color palette with the 4th entry being transparent black (BC1's
/// "1-bit alpha" punch-through). DXT3's color subblock ignores this and
/// always uses the 4-color rule.
fn decode_color_block(block: &[u8; 8], out: &mut [u8; 64], punchthrough_alpha: bool) {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let (r0, g0, b0) = rgb565_to_rgb888(c0);
    let (r1, g1, b1) = rgb565_to_rgb888(c1);

    let mut palette = [[0u8; 4]; 4];
    palette[0] = [r0, g0, b0, 255];
    palette[1] = [r1, g1, b1, 255];

    if c0 > c1 || !punchthrough_alpha {
        // 4-color mode.
        palette[2] = [
            lerp_u8(r0, r1, 1, 3),
            lerp_u8(g0, g1, 1, 3),
            lerp_u8(b0, b1, 1, 3),
            255,
        ];
        palette[3] = [
            lerp_u8(r0, r1, 2, 3),
            lerp_u8(g0, g1, 2, 3),
            lerp_u8(b0, b1, 2, 3),
            255,
        ];
    } else {
        // 3-color + transparent (BC1 punch-through).
        palette[2] = [
            ((r0 as u16 + r1 as u16) / 2) as u8,
            ((g0 as u16 + g1 as u16) / 2) as u8,
            ((b0 as u16 + b1 as u16) / 2) as u8,
            255,
        ];
        palette[3] = [0, 0, 0, 0];
    }

    let idx_word = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    for py in 0..4 {
        for px in 0..4 {
            let i = py * 4 + px;
            let sel = ((idx_word >> (2 * i)) & 0x3) as usize;
            let dst = i * 4;
            out[dst..dst + 4].copy_from_slice(&palette[sel]);
        }
    }
}

fn decode_dxt_common(
    blocks: &[u8],
    width: u32,
    height: u32,
    block_size: usize,
    mut block_decode: impl FnMut(&[u8], &mut [u8; 64]),
) -> std::result::Result<Vec<u8>, TextureError> {
    if width == 0 || height == 0 || width % 4 != 0 || height % 4 != 0 {
        return Err(TextureError::BadDimensions { width, height });
    }
    let (w, h) = (width as usize, height as usize);
    let total_pixels = w
        .checked_mul(h)
        .ok_or(TextureError::SizeOverflow { width, height })?;
    let blocks_x = w / 4;
    let blocks_y = h / 4;
    let needed = blocks_x * blocks_y * block_size;
    if blocks.len() < needed {
        return Err(TextureError::Truncated {
            offset: 0,
            needed,
            available: blocks.len(),
        });
    }

    let mut rgba = vec![0u8; total_pixels * 4];
    let mut block = [0u8; 64];
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let src = (by * blocks_x + bx) * block_size;
            block_decode(&blocks[src..src + block_size], &mut block);
            // Copy into the destination at (bx*4, by*4), row by row.
            for py in 0..4 {
                let dst_row = ((by * 4 + py) * w + bx * 4) * 4;
                let src_row = py * 4 * 4;
                rgba[dst_row..dst_row + 16].copy_from_slice(&block[src_row..src_row + 16]);
            }
        }
    }
    Ok(rgba)
}

/// Decode a tightly-packed DXT1 (BC1) bitstream into RGBA8.
pub fn decode_dxt1_blocks(
    blocks: &[u8],
    width: u32,
    height: u32,
) -> std::result::Result<Vec<u8>, TextureError> {
    decode_dxt_common(blocks, width, height, 8, |src, out| {
        let b: &[u8; 8] = src.try_into().unwrap();
        decode_color_block(b, out, true);
    })
}

/// Decode a tightly-packed DXT3 (BC2) bitstream into RGBA8.
pub fn decode_dxt3_blocks(
    blocks: &[u8],
    width: u32,
    height: u32,
) -> std::result::Result<Vec<u8>, TextureError> {
    decode_dxt_common(blocks, width, height, 16, |src, out| {
        // Color block first (last 8 bytes), then overwrite alpha from
        // the explicit-alpha block (first 8 bytes).
        let color: &[u8; 8] = src[8..16].try_into().unwrap();
        decode_color_block(color, out, false);
        for i in 0..8 {
            let a_byte = src[i];
            let a_lo_4 = (a_byte & 0x0F) as u16;
            let a_hi_4 = (a_byte >> 4) as u16;
            // Expand 4-bit → 8-bit by replicating.
            let a_lo = ((a_lo_4 << 4) | a_lo_4) as u8;
            let a_hi = ((a_hi_4 << 4) | a_hi_4) as u8;
            // Pixel order within byte: low nibble = even pixel index,
            // high nibble = odd pixel index (DDS convention).
            out[(2 * i) * 4 + 3] = a_lo;
            out[(2 * i + 1) * 4 + 3] = a_hi;
        }
    })
}

/// Decode raw on-disk BGRA8 bytes into RGBA8. FFXI's `"BGRA"` magic
/// indicates B,G,R,A byte order per pixel — swap channels to canonical
/// R,G,B,A.
pub fn decode_bgra_raw(
    pixels: &[u8],
    width: u32,
    height: u32,
) -> std::result::Result<Vec<u8>, TextureError> {
    if width == 0 || height == 0 {
        return Err(TextureError::BadDimensions { width, height });
    }
    let total = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(TextureError::SizeOverflow { width, height })?;
    if pixels.len() < total {
        return Err(TextureError::Truncated {
            offset: 0,
            needed: total,
            available: pixels.len(),
        });
    }
    let mut out = vec![0u8; total];
    for i in 0..(total / 4) {
        let s = i * 4;
        out[s] = pixels[s + 2]; // R
        out[s + 1] = pixels[s + 1]; // G
        out[s + 2] = pixels[s]; // B
        out[s + 3] = pixels[s + 3]; // A
    }
    Ok(out)
}

/// Decode raw on-disk ARGB8 bytes into RGBA8. FFXI's `"ARGB"` magic
/// indicates A,R,G,B byte order per pixel on disk.
pub fn decode_argb_raw(
    pixels: &[u8],
    width: u32,
    height: u32,
) -> std::result::Result<Vec<u8>, TextureError> {
    if width == 0 || height == 0 {
        return Err(TextureError::BadDimensions { width, height });
    }
    let total = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(TextureError::SizeOverflow { width, height })?;
    if pixels.len() < total {
        return Err(TextureError::Truncated {
            offset: 0,
            needed: total,
            available: pixels.len(),
        });
    }
    let mut out = vec![0u8; total];
    for i in 0..(total / 4) {
        let s = i * 4;
        out[s] = pixels[s + 1]; // R
        out[s + 1] = pixels[s + 2]; // G
        out[s + 2] = pixels[s + 3]; // B
        out[s + 3] = pixels[s]; // A
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_dxt3_magic() {
        let mut buf = vec![0u8; 16];
        buf.extend_from_slice(b"3TXD");
        buf.extend_from_slice(&[0u8; 32]);
        let (off, fmt) = find_texture_format(&buf).unwrap().unwrap();
        assert_eq!(off, 16);
        assert_eq!(fmt, TexFormat::Dxt3);
    }

    #[test]
    fn detects_dxt1_magic() {
        let buf = b"1TXD".to_vec();
        let (off, fmt) = find_texture_format(&buf).unwrap().unwrap();
        assert_eq!(off, 0);
        assert_eq!(fmt, TexFormat::Dxt1);
    }

    #[test]
    fn no_magic_returns_none() {
        let buf = vec![0xFF; 32];
        assert!(find_texture_format(&buf).unwrap().is_none());
    }

    /// Build a DXT1 block: c0 = pure red (RGB565 0xF800), c1 = pure blue
    /// (RGB565 0x001F). Because c0 > c1 we get the 4-color (no-alpha)
    /// palette. Indices: top-left=0 (red), top-right=1 (blue),
    /// other 14 pixels=0 (red). We only verify those corner pixels.
    fn dxt1_red_blue_block() -> [u8; 8] {
        let c0: u16 = 0xF800; // pure red
        let c1: u16 = 0x001F; // pure blue
        // Indices: 16 × 2 bits. Pixel 0 (top-left) = 0 (=c0). Pixel 1
        // (top-left + 1 col) = 1 (=c1). Rest = 0.
        let indices: u32 = 0b01 << 2; // pixel 0 = 00, pixel 1 = 01
        let mut block = [0u8; 8];
        block[0..2].copy_from_slice(&c0.to_le_bytes());
        block[2..4].copy_from_slice(&c1.to_le_bytes());
        block[4..8].copy_from_slice(&indices.to_le_bytes());
        block
    }

    #[test]
    fn dxt1_decodes_corner_pixels() {
        let block = dxt1_red_blue_block();
        // Tile a 4x4 image (one block).
        let rgba = decode_dxt1_blocks(&block, 4, 4).unwrap();
        assert_eq!(rgba.len(), 4 * 4 * 4);
        // Pixel (0,0): c0 = red 0xF800. 5-bit 0x1F → (0x1F<<3)|(0x1F>>2) = 255.
        assert_eq!(&rgba[0..4], &[255, 0, 0, 255]);
        // Pixel (1,0): c1 = blue 0x001F → (0, 0, 255, 255).
        assert_eq!(&rgba[4..8], &[0, 0, 255, 255]);
        // Pixel (2,0): index 0 → red.
        assert_eq!(&rgba[8..12], &[255, 0, 0, 255]);
        // Pixel (0,1): row 1 col 0 — index 0 → red.
        let row1_col0 = (1 * 4 + 0) * 4;
        assert_eq!(&rgba[row1_col0..row1_col0 + 4], &[255, 0, 0, 255]);
    }

    #[test]
    fn dxt1_punchthrough_alpha() {
        // c0 < c1 (0x0000 vs 0xFFFF): 3-color + transparent palette.
        // Indices: pixel 0 = 3 → transparent black.
        let c0: u16 = 0x0000;
        let c1: u16 = 0xFFFF;
        let indices: u32 = 0b11;
        let mut block = [0u8; 8];
        block[0..2].copy_from_slice(&c0.to_le_bytes());
        block[2..4].copy_from_slice(&c1.to_le_bytes());
        block[4..8].copy_from_slice(&indices.to_le_bytes());
        let rgba = decode_dxt1_blocks(&block, 4, 4).unwrap();
        assert_eq!(&rgba[0..4], &[0, 0, 0, 0]); // transparent
    }

    #[test]
    fn dxt3_alpha_block() {
        // Alpha block: explicit per-pixel 4-bit alpha. Pixel 0 = 0x0
        // (low nibble of byte 0), pixel 1 = 0xF (high nibble of byte 0).
        // Both expand by replication: 0x00 and 0xFF.
        let mut block = [0u8; 16];
        block[0] = 0xF0; // pixel 0 alpha = 0, pixel 1 alpha = 15
                        // Bytes 1..8 = zero → pixels 2..15 alpha = 0.
                        // Color block (bytes 8..16): c0 = pure red, c1 = pure blue,
                        // all indices = 0 → every pixel is red.
        let c0: u16 = 0xF800;
        let c1: u16 = 0x001F;
        block[8..10].copy_from_slice(&c0.to_le_bytes());
        block[10..12].copy_from_slice(&c1.to_le_bytes());
        // indices = 0 → all pixels select c0 (red)
        let rgba = decode_dxt3_blocks(&block, 4, 4).unwrap();
        // Pixel 0: red, alpha=0. RGB565 0x1F → 8-bit 255 via bit-replication
        // expansion ((0x1F<<3)|(0x1F>>2) = 248|7 = 255) per the DXT spec.
        assert_eq!(&rgba[0..4], &[255, 0, 0, 0]);
        // Pixel 1: red, alpha=255 (0xF → 0xFF via 4-bit replication)
        assert_eq!(&rgba[4..8], &[255, 0, 0, 255]);
        // Pixel 2: red, alpha=0
        assert_eq!(&rgba[8..12], &[255, 0, 0, 0]);
    }

    #[test]
    fn bgra_raw_round_trip() {
        // 2x2 image, 4 pixels, on-disk B,G,R,A order.
        let pixels: Vec<u8> = vec![
            10, 20, 30, 40, // pixel 0: B=10 G=20 R=30 A=40
            50, 60, 70, 80, // pixel 1
            90, 100, 110, 120, // pixel 2
            130, 140, 150, 160, // pixel 3
        ];
        let rgba = decode_bgra_raw(&pixels, 2, 2).unwrap();
        assert_eq!(&rgba[0..4], &[30, 20, 10, 40]); // R,G,B,A
        assert_eq!(&rgba[4..8], &[70, 60, 50, 80]);
    }

    #[test]
    fn argb_raw_round_trip() {
        // 1x1 image, on-disk A,R,G,B order.
        let pixels = vec![255u8, 10, 20, 30]; // A=255 R=10 G=20 B=30
        let rgba = decode_argb_raw(&pixels, 1, 1).unwrap();
        assert_eq!(&rgba, &[10, 20, 30, 255]);
    }

    #[test]
    fn dxt1_rejects_non_multiple_of_4() {
        let block = [0u8; 8];
        let err = decode_dxt1_blocks(&block, 3, 4).unwrap_err();
        assert!(matches!(err, TextureError::BadDimensions { .. }));
    }

    #[test]
    fn dxt3_rejects_truncated() {
        // Need 16 bytes for one block; supply 8.
        let short = [0u8; 8];
        let err = decode_dxt3_blocks(&short, 4, 4).unwrap_err();
        assert!(matches!(err, TextureError::Truncated { .. }));
    }

    #[test]
    fn decode_texture_dxt1_round_trip() {
        // Synthesize a body that matches the documented (spec-based)
        // FFXI framing: magic + u32 width + u32 height + DXT1 blocks.
        let mut body = Vec::new();
        body.extend_from_slice(b"1TXD");
        body.extend_from_slice(&4u32.to_le_bytes());
        body.extend_from_slice(&4u32.to_le_bytes());
        body.extend_from_slice(&dxt1_red_blue_block());
        let dec = decode_texture(&body).unwrap();
        assert_eq!(dec.width, 4);
        assert_eq!(dec.height, 4);
        assert_eq!(dec.format_tag, TexFormat::Dxt1);
        // RGB565 0xF800 → R=(0x1F<<3)|(0x1F>>2)=255 (spec bit-replication).
        assert_eq!(&dec.rgba[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn decode_texture_no_magic_errors() {
        let body = vec![0u8; 64];
        let err = decode_texture(&body).unwrap_err();
        assert!(matches!(err, TextureError::NoMagic));
    }
}
