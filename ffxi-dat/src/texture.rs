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

/// Raw decoded alpha below this remaps to fully transparent, so
/// `ffxi_alpha_remap(raw) == 0 <=> raw < CUTOUT_TRANSPARENT_MAX`. Consumers key their
/// cutout detection off the same threshold (`zone_texture::has_cutout_alpha`).
pub const CUTOUT_TRANSPARENT_MAX: u8 = 16;

/// FFXI authors texture alpha at half scale (the DXT "bc2/8" convention from
/// Lotus/AltanaViewer), so a fully opaque texel decodes as 0x80, not 0xFF. Double it back
/// to the full range.
///
/// The doubling is continuous, NOT a re-quantization of the top nibble. Reading only the
/// top nibble stretched DXT3's 0x11 alpha step to 32/255, doubling the amplitude of every
/// ordered dither in FFXI's 4-bit alpha (see [`DXT3_ALPHA_DITHER_STEP`]) instead of
/// resolving it, and posterized the 8-bit formats — the BGRA32 `fine`/`suny` sky canopies
/// carry a smooth 0-255 alpha ramp — down to nine levels (kuluu-u5mm).
///
/// Every consumer of a decoded texture's alpha has to apply this — a moon sprite whose
/// alpha peaks at 0x88 draws at half its authored opacity otherwise.
#[inline]
pub fn ffxi_alpha_remap(raw: u8) -> u8 {
    if raw < CUTOUT_TRANSPARENT_MAX {
        return 0;
    }
    (raw as u16 * 2).min(255) as u8
}

/// Apply [`ffxi_alpha_remap`] across an RGBA buffer in place.
pub fn apply_ffxi_alpha_remap(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        px[3] = ffxi_alpha_remap(px[3]);
    }
}

fn token(raw: &[u8]) -> String {
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
    s.trim().to_string()
}

pub const NAMESPACE_LEN: usize = imginfo::TOKEN_LEN;

// research/xim DatResource.kt TextureName — a fully qualified 16-char texture name splits into
// a namespace (bytes 0..8) and a local name (bytes 8..16).
pub fn split_qualified_name(raw16: &[u8]) -> (String, String) {
    (
        token(&raw16[..imginfo::TOKEN_LEN.min(raw16.len())]),
        token(&raw16[imginfo::TOKEN_LEN.min(raw16.len())..]),
    )
}

pub fn extract_texture_tokens(body: &[u8]) -> Option<(String, String)> {
    if body.len() < imginfo::NAME_END || !imginfo::NAMED_HEADER_FLAGS.contains(&body[0]) {
        return None;
    }
    let (namespace, local) = split_qualified_name(&body[1..imginfo::NAME_END]);
    // A blank local token is not a name: keyed into the name maps it would collide with every
    // other blank-named chunk and with every mesh that names no texture at all.
    (!local.is_empty()).then_some((namespace, local))
}

pub fn extract_texture_name(body: &[u8]) -> Option<String> {
    extract_texture_tokens(body).map(|(_, id)| id)
}

mod imginfo {

    pub const FLG_DXT: u8 = 0xA1;

    pub(super) const FLG_PALETTE: u8 = 0x91;

    // research/xim TextureSection.kt read textureId, :73-79 — 0xB1 repeats 0x91's palettised layout with
    // one extra header word, which is why its palette/pixel offsets sit 4 bytes later.
    pub(super) const FLG_PALETTE_EXT: u8 = 0xB1;

    // research/XIClient ImageData.h ImageData — the header is `unsigned char Format;
    // ResourceID TextureName;` with no union or variant, and GameTexture::ConfigureFromImageData
    // (GameTexture.cpp GameTexture::ConfigureFromImageData) copies TextureName as its first statement, before the
    // GetTextureFormat() switch at :212-245. The name field is therefore unconditional across
    // type bytes: these two name themselves exactly where the 0x9x/0xAx/0xBx kinds do.
    // (research/xim TextureSection.kt read `warn`s on them, but that is an unhandled case, not
    // evidence of an absent name; XIClient is the stronger tier per research/AGENTS.md.)
    // ImageData.h ImageData separate the pair: both are GetTextureFormat() 0, and it is
    // IsCompressed() = Format >> 7 that tells 0x81 from 0x01.
    pub(super) const FLG_FMT0: u8 = 0x01;

    pub(super) const FLG_FMT0_COMPRESSED: u8 = 0x81;

    pub(super) const NAMED_HEADER_FLAGS: [u8; 5] = [
        FLG_DXT,
        FLG_PALETTE,
        FLG_PALETTE_EXT,
        FLG_FMT0,
        FLG_FMT0_COMPRESSED,
    ];

    pub(super) const NAME_END: usize = 0x11;

    pub(super) const TOKEN_LEN: usize = 8;

    pub(super) const WIDTH_OFF: usize = 0x15;

    pub(super) const HEIGHT_OFF: usize = 0x19;

    pub(super) const DIMS_END: usize = 0x1D;

    // Every type byte shares this header: 0xA1 puts its DXT fourcc here, the palettised kinds
    // their palette.
    pub(super) const HEADER_END: usize = 0x39;

    pub(super) const MAGIC_OFF: usize = HEADER_END;

    pub(super) const MAGIC_END: usize = 0x3D;

    pub(super) const HEADER_SIZE: usize = 0x45;

    pub(super) const PALETTE_OFF: usize = HEADER_END;

    // 256 BGRA entries.
    pub(super) const PALETTE_LEN: usize = 0x400;

    pub(super) const PIXELS_OFF: usize = PALETTE_OFF + PALETTE_LEN;

    // research/xim TextureSection.kt read — 0xB1 reads one extra header word before the payload.
    pub(super) const EXT_HEADER_WORD: usize = 4;

    pub(super) const PALETTE_OFF_EXT: usize = PALETTE_OFF + EXT_HEADER_WORD;

    pub(super) const PIXELS_OFF_EXT: usize = PALETTE_OFF_EXT + PALETTE_LEN;
}

pub use imginfo::FLG_DXT;

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
        imginfo::FLG_PALETTE => decode_palettized(body, imginfo::PALETTE_OFF, imginfo::PIXELS_OFF),
        imginfo::FLG_PALETTE_EXT => {
            decode_palettized(body, imginfo::PALETTE_OFF_EXT, imginfo::PIXELS_OFF_EXT)
        }
        // research/XIClient GameTexture.cpp GameTexture::ConfigureFromImageData — GetTextureFormat() is 0 for both, and
        // `case 0: case 1:` falls straight through to the ColorFormat walk at :249. Two gaps
        // there are unimplemented (kuluu-tm7m caveat): decode_palettized assumes the
        // Indexed8Bit/BitDepth-32 arm at :262-272 for every Img, and 0x81's IsCompressed() bit
        // additionally sends retail to the trailing-fourcc check at :291-300.
        imginfo::FLG_FMT0 | imginfo::FLG_FMT0_COMPRESSED => {
            decode_palettized(body, imginfo::PALETTE_OFF, imginfo::PIXELS_OFF)
        }
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

const RGB565_5BIT_MASK: u16 = 0x1F;
const RGB565_6BIT_MASK: u16 = 0x3F;
const DXT_TEXEL_INDEX_MASK: u32 = 0x3;
const DXT3_ALPHA_NIBBLE_MASK: u8 = 0x0F;

fn rgb565_to_rgb888(c: u16) -> (u8, u8, u8) {
    let r5 = ((c >> 11) & RGB565_5BIT_MASK) as u8;
    let g6 = ((c >> 5) & RGB565_6BIT_MASK) as u8;
    let b5 = (c & RGB565_5BIT_MASK) as u8;

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
            let sel = ((idx_word >> (2 * i)) & DXT_TEXEL_INDEX_MASK) as usize;
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

/// The one step a DXT3 alpha plane can take: alpha is a 4-bit nibble expanded to 8 bits by
/// replicating it (`n -> n * 0x11`), so nothing between two nibbles is representable.
///
/// FFXI's art leans on that gap. An authored value falling between two nibbles is
/// approximated by stippling the pair across neighbouring texels in a 4x4 ordered-dither
/// pattern, and whole sky textures are nothing else: 98.4% of `clod_a01` — the overcast
/// canopy every zone's `weat/clod` hangs off cld1/cld2 — alternates nibble 7 (119) and
/// nibble 8 (136) to reach the half-scale-opaque `0x80` that 4 bits cannot hold, and
/// `star01` alternates nibble 3/4. Retail never shows it: it draws the sky heavily
/// minified, where filtering averages the stipple back out. We draw the same canopy
/// magnified, so it reads as a checkerboard over the whole sky (kuluu-u5mm). Measured with
/// the `dat-sky-alpha-histogram` example, which also confirms the RGB carries no matching
/// stipple — only alpha is dithered.
pub const DXT3_ALPHA_DITHER_STEP: u8 = 0x11;

/// Average an ordered-dithered DXT3 alpha plane back into the continuous value it encodes.
///
/// Restricted to 3x3 neighbourhoods spanning exactly one nibble step, which is precisely
/// the dither signature: a genuine alpha edge (a cutout mask, a sprite border) steps by
/// more than one nibble and is left untouched, and averaging *within* one quantization step
/// cannot destroy detail the format was able to store in the first place.
///
/// **Opt-in, not part of the decode.** The stipple is only visible where a texture is drawn
/// magnified, which in practice means the camera-follow sky shells; every other texture
/// samples it at or below 1:1, where filtering resolves it for free. Running the filter over
/// a whole zone's textures costs ~5x their decode time on the dev profile for no visible
/// gain, so callers opt in per texture (`zone_texture::decoded_sky_texture_to_image`).
///
/// Must run BEFORE [`ffxi_alpha_remap`]: the remap doubles the recovered mean, whereas
/// averaging after it would smooth values that have already been stretched apart.
pub fn resolve_dxt3_alpha_dither(rgba: &mut [u8], width: u32, height: u32) {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return;
    }
    // Two guards on one cheap pass. Every decoded DXT3 alpha is a multiple of the nibble
    // step, so an alpha that is not says this buffer did not come from a 4-bit plane and
    // has no nibble dither to resolve — callers that cannot cheaply name their source
    // format (the moon sheet arrives as a format-erased `GraphicImage`) rely on that rather
    // than on guessing. And a dither needs two levels exactly one nibble apart, so a single
    // level or a punch-through pair at opposite ends has nothing to resolve either.
    let mut present = [false; 16];
    for px in rgba.chunks_exact(4) {
        if px[3] % DXT3_ALPHA_DITHER_STEP != 0 {
            return;
        }
        present[(px[3] / DXT3_ALPHA_DITHER_STEP) as usize] = true;
    }
    if !present.windows(2).any(|pair| pair[0] && pair[1]) {
        return;
    }

    let alpha: Vec<u8> = rgba.chunks_exact(4).map(|p| p[3]).collect();
    for y in 0..h {
        for x in 0..w {
            let (mut lo, mut hi, mut sum, mut n) = (u8::MAX, u8::MIN, 0u32, 0u32);
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let a = alpha[ny * w + nx];
                    lo = lo.min(a);
                    hi = hi.max(a);
                    sum += a as u32;
                    n += 1;
                }
            }
            if hi - lo == DXT3_ALPHA_DITHER_STEP {
                rgba[(y * w + x) * 4 + 3] = ((sum + n / 2) / n) as u8;
            }
        }
    }
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
            let a_lo_4 = (a_byte & DXT3_ALPHA_NIBBLE_MASK) as u16;
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
pub(crate) mod tests {
    use super::*;

    // Emitter/consumer coupling guard: zone_texture, moon_material and the particle texture
    // path all read alpha out of the same decoded buffer, so the top-nibble expansion has to
    // be one function. 0x80 is FFXI's "opaque" and must reach 0xFF, or an additive sprite
    // (Bevy blends Add by src alpha) draws at half its authored strength.
    #[test]
    fn ffxi_alpha_remap_expands_the_top_nibble() {
        assert_eq!(ffxi_alpha_remap(0x00), 0);
        assert_eq!(ffxi_alpha_remap(0x0F), 0, "raw < 16 is fully transparent");
        assert_eq!(ffxi_alpha_remap(0x80), 255, "0x80 is FFXI-opaque");
        assert_eq!(
            ffxi_alpha_remap(0x88),
            255,
            "the real moon sheet peaks at 0x88"
        );
        assert_eq!(ffxi_alpha_remap(0xFF), 255);
        for raw in 0..=254u8 {
            assert!(ffxi_alpha_remap(raw) <= ffxi_alpha_remap(raw + 1));
        }
    }

    #[test]
    fn apply_ffxi_alpha_remap_touches_only_alpha() {
        let mut rgba = vec![10, 20, 30, 0x80, 40, 50, 60, 0x00];
        apply_ffxi_alpha_remap(&mut rgba);
        assert_eq!(rgba, vec![10, 20, 30, 255, 40, 50, 60, 0]);
    }

    // File 3020 (Poison) carries the 16 bytes `venom1  fir     ` in both its 0xA1 Img and its
    // 0x21 sprite sheet. research/xim DatResource.kt TextureName splits that into namespace/local.
    pub(crate) const QUALIFIED_FIR: &[u8; 16] = b"venom1  fir     ";

    pub(crate) fn img_body_named(raw16: &[u8; 16]) -> Vec<u8> {
        img_body_named_with_flag(imginfo::FLG_DXT, raw16)
    }

    fn img_body_named_with_flag(flag: u8, raw16: &[u8; 16]) -> Vec<u8> {
        let mut body = vec![0u8; imginfo::NAME_END];
        body[0] = flag;
        body[1..imginfo::NAME_END].copy_from_slice(raw16);
        body
    }

    #[test]
    fn extract_texture_name_is_the_local_token() {
        let body = img_body_named(QUALIFIED_FIR);
        assert_eq!(
            extract_texture_tokens(&body),
            Some(("venom1".to_string(), "fir".to_string()))
        );
        assert_eq!(extract_texture_name(&body).as_deref(), Some("fir"));
    }

    // research/XIClient GameTexture.cpp GameTexture::ConfigureFromImageData — TextureName is copied before any format branch,
    // so a palettised Img names itself exactly like a DXT one and has to enter the name-keyed
    // maps too or it can never be linked by name.
    #[test]
    fn extract_texture_tokens_accepts_every_named_header_flag() {
        for flag in imginfo::NAMED_HEADER_FLAGS {
            let body = img_body_named_with_flag(flag, QUALIFIED_FIR);
            assert_eq!(
                extract_texture_tokens(&body),
                Some(("venom1".to_string(), "fir".to_string())),
                "flag {flag:#04x}"
            );
        }
    }

    // The ImageData header at ROM/0/28.DAT offset 3389984 (file 100, chunk `smok`), verbatim:
    // the 0x81 type byte, the qualified name, SubsequentDataSize (0x28, "observed to be
    // consistently 40" per research/XIClient ImageData.h ImageData), then Width and Height. That
    // file's `smok` 0x21 sprite sheet names this exact texture, so keeping 0x81 out of the name
    // tiers left the sheet -- and every other texture in the zone, all 25 of which are 0x81 --
    // unlinkable.
    const RETAIL_0X81_IMG_HEADER: [u8; 29] = [
        0x81, b'e', b'f', b'f', b'e', b'c', b't', b' ', b' ', b's', b'm', b'o', b'k', b'e', b'0',
        b'1', b' ', 0x28, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00,
    ];

    #[test]
    fn extract_texture_tokens_accepts_the_retail_0x81_image_header() {
        assert_eq!(
            extract_texture_tokens(&RETAIL_0X81_IMG_HEADER),
            Some(("effect".to_string(), "smoke01".to_string()))
        );
        let dims = |off: usize| {
            i32::from_le_bytes(
                RETAIL_0X81_IMG_HEADER[off..off + 4]
                    .try_into()
                    .expect("4-byte dimension word"),
            )
        };
        assert_eq!(
            (dims(imginfo::WIDTH_OFF), dims(imginfo::HEIGHT_OFF)),
            (128, 128),
            "the constant offsets must land on the retail Width/Height words"
        );
    }

    #[test]
    fn extract_texture_tokens_rejects_a_blank_local_name() {
        for flag in imginfo::NAMED_HEADER_FLAGS {
            let body = img_body_named_with_flag(flag, b"                ");
            assert_eq!(extract_texture_tokens(&body), None, "flag {flag:#04x}");
            let body = img_body_named_with_flag(flag, b"venom1          ");
            assert_eq!(extract_texture_tokens(&body), None, "flag {flag:#04x}");
        }
    }

    #[test]
    fn split_qualified_name_trims_both_tokens() {
        assert_eq!(
            split_qualified_name(QUALIFIED_FIR),
            ("venom1".to_string(), "fir".to_string())
        );
        assert_eq!(
            split_qualified_name(b"moon    moonshap"),
            ("moon".to_string(), "moonshap".to_string())
        );
    }

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

    // Builds a DXT3 alpha plane from a per-texel closure, so a test can state the stipple
    // it cares about instead of hand-packing nibbles.
    fn dxt3_with_alpha(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> Vec<u8> {
        let blocks_x = w.div_ceil(4) as usize;
        let mut out = vec![0u8; blocks_x * (h.div_ceil(4) as usize) * 16];
        for y in 0..h {
            for x in 0..w {
                let block = (y / 4) as usize * blocks_x + (x / 4) as usize;
                let texel = (y % 4) * 4 + (x % 4);
                let byte = block * 16 + (texel / 2) as usize;
                let nibble = f(x, y) & DXT3_ALPHA_NIBBLE_MASK;
                out[byte] |= nibble << (4 * (texel % 2));
            }
        }
        out
    }

    // Post-remap ripple a resolved dither may still carry. A 3x3 mean over a checkerboard
    // covers 5 texels of one phase and 4 of the other, so neighbouring centres land 0x11/9
    // apart and round to adjacent 8-bit codes. One code is the quantization floor — a
    // stipple that small is unrepresentable, where the raw pair is 2*0x11 = 34 codes apart
    // after the remap.
    const RESOLVED_RIPPLE_MAX: u8 = 1;

    fn interior_alpha(rgba: &[u8], side: usize) -> Vec<u8> {
        (1..side - 1)
            .flat_map(|y| (1..side - 1).map(move |x| (y * side + x) * 4 + 3))
            .map(|i| rgba[i])
            .collect()
    }

    fn ripple(values: &[u8]) -> u8 {
        values.iter().max().unwrap() - values.iter().min().unwrap()
    }

    // The whole bug in one assertion: a nibble-7/8 checkerboard is FFXI's way of writing
    // the half-scale-opaque 0x80 that 4-bit alpha cannot hold, and it has to come out of
    // the decode->remap chain flat. Pinning both ends together (rather than the decoder or
    // the remap alone) is what stops a future "restore the exact DXT3 expansion" or a
    // re-quantizing `ffxi_alpha_remap` from silently putting the sky checkerboard back.
    #[test]
    fn dxt3_ordered_dither_decodes_flat() {
        let blocks = dxt3_with_alpha(8, 8, |x, y| if (x + y) % 2 == 0 { 7 } else { 8 });
        let mut rgba = decode_dxt3_blocks(&blocks, 8, 8).unwrap();
        resolve_dxt3_alpha_dither(&mut rgba, 8, 8);
        let remapped: Vec<u8> = interior_alpha(&rgba, 8)
            .into_iter()
            .map(ffxi_alpha_remap)
            .collect();
        assert!(
            ripple(&remapped) <= RESOLVED_RIPPLE_MAX,
            "a nibble 7/8 stipple still ripples after remap: {remapped:?}"
        );
        assert!(
            *remapped.iter().min().unwrap() >= 238,
            "the opaque pair must stay opaque, got {remapped:?}"
        );
    }

    // The same encoder trick around a different target: `star01` alternates nibble 3/4.
    // A fix that only knew about the opaque pair would leave the star dome stippled.
    #[test]
    fn dxt3_dither_resolves_at_any_nibble_pair() {
        for lo in 0u8..15 {
            let blocks = dxt3_with_alpha(8, 8, |x, y| if (x + y) % 2 == 0 { lo } else { lo + 1 });
            let mut rgba = decode_dxt3_blocks(&blocks, 8, 8).unwrap();
            resolve_dxt3_alpha_dither(&mut rgba, 8, 8);
            assert!(
                ripple(&interior_alpha(&rgba, 8)) <= RESOLVED_RIPPLE_MAX,
                "nibble {lo}/{} stipple survived",
                lo + 1
            );
        }
    }

    // Callers that cannot name their source format (the moon sheet arrives as a
    // format-erased `GraphicImage`) hand this any buffer, so it has to recognize 8-bit alpha
    // and decline. Averaging a real 8-bit gradient would soften art nothing was wrong with.
    #[test]
    fn declines_alpha_that_did_not_come_from_a_nibble_plane() {
        let mut rgba: Vec<u8> = (0..8u32 * 8)
            .flat_map(|i| [0, 0, 0, (100 + i % 5) as u8])
            .collect();
        let before = rgba.clone();
        resolve_dxt3_alpha_dither(&mut rgba, 8, 8);
        assert_eq!(rgba, before, "8-bit alpha must pass through untouched");
    }

    // The guard that keeps the averaging honest: a cutout mask steps by more than one
    // nibble, so it must come through the decoder untouched. Blur it and every alpha-tested
    // leaf/fence card in the game grows a soft fringe.
    #[test]
    fn dxt3_leaves_genuine_alpha_edges_alone() {
        let blocks = dxt3_with_alpha(8, 8, |x, _| if x < 4 { 0 } else { 15 });
        let mut rgba = decode_dxt3_blocks(&blocks, 8, 8).unwrap();
        resolve_dxt3_alpha_dither(&mut rgba, 8, 8);
        for y in 0..8 {
            for x in 0..8 {
                let a = rgba[(y * 8 + x) * 4 + 3];
                let want = if x < 4 { 0 } else { 255 };
                assert_eq!(a, want, "edge softened at ({x},{y})");
            }
        }
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
