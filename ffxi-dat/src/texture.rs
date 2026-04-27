use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TexFormat {
    Dxt1,

    Dxt3,

    Bgra32,

    Argb32,
}

impl TexFormat {
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

#[derive(Debug, Clone)]
pub struct DecodedTexture {
    pub width: u32,
    pub height: u32,
    pub format_tag: TexFormat,
    pub rgba: Vec<u8>,
}

pub fn extract_texture_name(body: &[u8]) -> Option<String> {
    if body.len() < imginfo::NAME_END || body[0] != imginfo::FLG_DXT {
        return None;
    }

    let raw = &body[9..imginfo::NAME_END];
    let s: String = raw
        .iter()
        .map(|&b| {
            if (0x20..0x7f).contains(&b) {
                b as char
            } else {
                '\0'
            }
        })
        .take_while(|&c| c != '\0')
        .collect();
    Some(s.trim().to_string())
}

mod imginfo {

    pub(super) const FLG_DXT: u8 = 0xA1;

    pub(super) const NAME_END: usize = 0x11;

    pub(super) const WIDTH_OFF: usize = 0x15;

    pub(super) const HEIGHT_OFF: usize = 0x19;

    pub(super) const DIMS_END: usize = 0x1D;

    pub(super) const MAGIC_OFF: usize = 0x39;

    pub(super) const MAGIC_END: usize = 0x3D;

    pub(super) const HEADER_SIZE: usize = 0x45;
}

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

pub fn find_texture_format(body: &[u8]) -> Result<Option<(usize, TexFormat)>> {
    for (i, win) in body.windows(4).enumerate() {
        if let Some(fmt) = TexFormat::from_magic(win) {
            return Ok(Some((i, fmt)));
        }
    }
    Ok(None)
}

pub fn decode_texture(body: &[u8]) -> std::result::Result<DecodedTexture, TextureError> {
    if body.is_empty() {
        return Err(TextureError::NoMagic);
    }
    match body[0] {
        imginfo::FLG_DXT => decode_imginfo_a1(body),
        0x01 | 0x81 | 0x91 => decode_palettized(body, 0x39, 0x439),
        0xB1 => decode_palettized(body, 0x3D, 0x43D),

        _ => Err(TextureError::NoMagic),
    }
}

fn decode_imginfo_a1(body: &[u8]) -> std::result::Result<DecodedTexture, TextureError> {
    if body.len() < imginfo::HEADER_SIZE {
        return Err(TextureError::Truncated {
            offset: 0,
            needed: imginfo::HEADER_SIZE,
            available: body.len(),
        });
    }
    let width = i32::from_le_bytes(
        body[imginfo::WIDTH_OFF..imginfo::HEIGHT_OFF]
            .try_into()
            .unwrap(),
    );
    let height = i32::from_le_bytes(
        body[imginfo::HEIGHT_OFF..imginfo::DIMS_END]
            .try_into()
            .unwrap(),
    );
    if width <= 0 || height <= 0 {
        return Err(TextureError::BadDimensions {
            width: width as u32,
            height: height as u32,
        });
    }
    let width = width as u32;
    let height = height as u32;

    let magic = &body[imginfo::MAGIC_OFF..imginfo::MAGIC_END];
    let fmt = TexFormat::from_magic(magic).ok_or(TextureError::NoMagic)?;
    let pixel_off = imginfo::HEADER_SIZE;
    let pixels = &body[pixel_off..];
    let rgba = match fmt {
        TexFormat::Dxt1 => decode_dxt1_blocks(pixels, width, height)?,
        TexFormat::Dxt3 => decode_dxt3_blocks(pixels, width, height)?,
        TexFormat::Bgra32 => decode_bgra_raw(pixels, width, height)?,
        TexFormat::Argb32 => decode_argb_raw(pixels, width, height)?,
    };
    Ok(DecodedTexture {
        width,
        height,
        format_tag: fmt,
        rgba,
    })
}

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
    let width = i32::from_le_bytes(
        body[imginfo::WIDTH_OFF..imginfo::HEIGHT_OFF]
            .try_into()
            .unwrap(),
    );
    let height = i32::from_le_bytes(
        body[imginfo::HEIGHT_OFF..imginfo::DIMS_END]
            .try_into()
            .unwrap(),
    );
    if width <= 0 || height <= 0 {
        return Err(TextureError::BadDimensions {
            width: width as u32,
            height: height as u32,
        });
    }
    let width = width as u32;
    let height = height as u32;

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

fn rgb565_to_rgb888(c: u16) -> (u8, u8, u8) {
    let r5 = ((c >> 11) & 0x1F) as u8;
    let g6 = ((c >> 5) & 0x3F) as u8;
    let b5 = (c & 0x1F) as u8;

    let r = (r5 << 3) | (r5 >> 2);
    let g = (g6 << 2) | (g6 >> 4);
    let b = (b5 << 3) | (b5 >> 2);
    (r, g, b)
}

fn lerp_u8(a: u8, b: u8, num: u32, den: u32) -> u8 {
    (((den - num) * a as u32 + num * b as u32) / den) as u8
}

fn decode_color_block(block: &[u8; 8], out: &mut [u8; 64], punchthrough_alpha: bool) {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let (r0, g0, b0) = rgb565_to_rgb888(c0);
    let (r1, g1, b1) = rgb565_to_rgb888(c1);

    let mut palette = [[0u8; 4]; 4];
    palette[0] = [r0, g0, b0, 255];
    palette[1] = [r1, g1, b1, 255];

    if c0 > c1 || !punchthrough_alpha {
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
    if width == 0 || height == 0 || !width.is_multiple_of(4) || !height.is_multiple_of(4) {
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

            for py in 0..4 {
                let dst_row = ((by * 4 + py) * w + bx * 4) * 4;
                let src_row = py * 4 * 4;
                rgba[dst_row..dst_row + 16].copy_from_slice(&block[src_row..src_row + 16]);
            }
        }
    }
    Ok(rgba)
}

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

pub fn decode_dxt3_blocks(
    blocks: &[u8],
    width: u32,
    height: u32,
) -> std::result::Result<Vec<u8>, TextureError> {
    decode_dxt_common(blocks, width, height, 16, |src, out| {
        let color: &[u8; 8] = src[8..16].try_into().unwrap();
        decode_color_block(color, out, false);
        for i in 0..8 {
            let a_byte = src[i];
            let a_lo_4 = (a_byte & 0x0F) as u16;
            let a_hi_4 = (a_byte >> 4) as u16;

            let a_lo = ((a_lo_4 << 4) | a_lo_4) as u8;
            let a_hi = ((a_hi_4 << 4) | a_hi_4) as u8;

            out[(2 * i) * 4 + 3] = a_lo;
            out[(2 * i + 1) * 4 + 3] = a_hi;
        }
    })
}

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
        out[s] = pixels[s + 2];
        out[s + 1] = pixels[s + 1];
        out[s + 2] = pixels[s];
        out[s + 3] = pixels[s + 3];
    }
    Ok(out)
}

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
        out[s] = pixels[s + 1];
        out[s + 1] = pixels[s + 2];
        out[s + 2] = pixels[s + 3];
        out[s + 3] = pixels[s];
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

    fn dxt1_red_blue_block() -> [u8; 8] {
        let c0: u16 = 0xF800;
        let c1: u16 = 0x001F;

        let indices: u32 = 0b01 << 2;
        let mut block = [0u8; 8];
        block[0..2].copy_from_slice(&c0.to_le_bytes());
        block[2..4].copy_from_slice(&c1.to_le_bytes());
        block[4..8].copy_from_slice(&indices.to_le_bytes());
        block
    }

    #[test]
    fn dxt1_decodes_corner_pixels() {
        let block = dxt1_red_blue_block();

        let rgba = decode_dxt1_blocks(&block, 4, 4).unwrap();
        assert_eq!(rgba.len(), 4 * 4 * 4);

        assert_eq!(&rgba[0..4], &[255, 0, 0, 255]);

        assert_eq!(&rgba[4..8], &[0, 0, 255, 255]);

        assert_eq!(&rgba[8..12], &[255, 0, 0, 255]);

        let row1_col0 = 4 * 4;
        assert_eq!(&rgba[row1_col0..row1_col0 + 4], &[255, 0, 0, 255]);
    }

    #[test]
    fn dxt1_punchthrough_alpha() {
        let c0: u16 = 0x0000;
        let c1: u16 = 0xFFFF;
        let indices: u32 = 0b11;
        let mut block = [0u8; 8];
        block[0..2].copy_from_slice(&c0.to_le_bytes());
        block[2..4].copy_from_slice(&c1.to_le_bytes());
        block[4..8].copy_from_slice(&indices.to_le_bytes());
        let rgba = decode_dxt1_blocks(&block, 4, 4).unwrap();
        assert_eq!(&rgba[0..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn dxt3_alpha_block() {
        let mut block = [0u8; 16];
        block[0] = 0xF0;

        let c0: u16 = 0xF800;
        let c1: u16 = 0x001F;
        block[8..10].copy_from_slice(&c0.to_le_bytes());
        block[10..12].copy_from_slice(&c1.to_le_bytes());

        let rgba = decode_dxt3_blocks(&block, 4, 4).unwrap();

        assert_eq!(&rgba[0..4], &[255, 0, 0, 0]);

        assert_eq!(&rgba[4..8], &[255, 0, 0, 255]);

        assert_eq!(&rgba[8..12], &[255, 0, 0, 0]);
    }

    #[test]
    fn bgra_raw_round_trip() {
        let pixels: Vec<u8> = vec![
            10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 160,
        ];
        let rgba = decode_bgra_raw(&pixels, 2, 2).unwrap();
        assert_eq!(&rgba[0..4], &[30, 20, 10, 40]);
        assert_eq!(&rgba[4..8], &[70, 60, 50, 80]);
    }

    #[test]
    fn argb_raw_round_trip() {
        let pixels = vec![255u8, 10, 20, 30];
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
        let short = [0u8; 8];
        let err = decode_dxt3_blocks(&short, 4, 4).unwrap_err();
        assert!(matches!(err, TextureError::Truncated { .. }));
    }

    #[test]
    fn decode_texture_dxt1_round_trip() {
        use imginfo::*;
        let mut body = vec![0u8; HEADER_SIZE];
        body[0] = FLG_DXT;
        body[WIDTH_OFF..HEIGHT_OFF].copy_from_slice(&4i32.to_le_bytes());
        body[HEIGHT_OFF..DIMS_END].copy_from_slice(&4i32.to_le_bytes());
        body[MAGIC_OFF..MAGIC_END].copy_from_slice(b"1TXD");
        body.extend_from_slice(&dxt1_red_blue_block());
        let dec = decode_texture(&body).unwrap();
        assert_eq!(dec.width, 4);
        assert_eq!(dec.height, 4);
        assert_eq!(dec.format_tag, TexFormat::Dxt1);

        assert_eq!(&dec.rgba[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn decode_texture_no_magic_errors() {
        let body = vec![0u8; 64];
        let err = decode_texture(&body).unwrap_err();
        assert!(matches!(err, TextureError::NoMagic));
    }
}
