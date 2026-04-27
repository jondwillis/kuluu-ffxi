#![cfg(not(target_arch = "wasm32"))]

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ffxi_dat::texture::DecodedTexture;

/// FFXI authors texture alpha in the top nibble (the DXT "bc2/8" convention from
/// Lotus/AltanaViewer); expand it to the full 0..=255 range. Raw alpha < 16 maps
/// to 0 (fully transparent), so `remap(raw) == 0  <=>  raw < 16`.
#[inline]
pub fn ffxi_alpha_remap(raw: u8) -> u8 {
    let bc2 = (raw >> 4) as f32;
    let scaled = bc2 * 255.0 / 8.0;
    scaled.min(255.0).round() as u8
}

/// Raw decoded alpha below this becomes fully transparent after [`ffxi_alpha_remap`].
const CUTOUT_ALPHA_RAW: u8 = 16;

/// Alpha test cutoff used by `FfxiZoneMaterial` (`AlphaMode::Mask(0.5)`), in 8-bit.
const MASK_THRESHOLD_U8: u8 = 128;

/// True when the texture carries cutout transparency — some texel becomes fully
/// transparent after the FFXI alpha remap. Foliage/leaf textures are DXT1
/// punchthrough cutouts whose "transparent" texels are authored as black RGB;
/// such submeshes must alpha-test (Mask) or that black renders opaque.
pub fn has_cutout_alpha(t: &DecodedTexture) -> bool {
    t.rgba.chunks_exact(4).any(|p| p[3] < CUTOUT_ALPHA_RAW)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureQuality {
    /// Generate a mip chain. XIM uploads only mip 0 (bilinear, no mips); mips
    /// trade a little sharpness for less distance shimmer. Off => a single level,
    /// pixel-faithful to XIM.
    pub mipmaps: bool,
    /// `1` disables anisotropic filtering. Anisotropy only helps with mips.
    pub anisotropy: u16,
}

impl Default for TextureQuality {
    fn default() -> Self {
        Self {
            mipmaps: false,
            anisotropy: 1,
        }
    }
}

/// Build a render-ready [`Image`] from a decoded FFXI texture: remap alpha →
/// optional sRGB-correct mip chain → bilinear (+ optional anisotropic) sampler.
///
/// RGB is never modified. FFXI palettized/DXT3 textures store the real surface
/// colour in their low-alpha texels (which render opaque on non-cutout models),
/// so a colour-bleed pass would smear that detail away. XIM does no bleed; we
/// match it.
///
/// The actor (`ffxi_actor_render`) and `dat_vos2` paths have their own,
/// simpler converters; they are intentional future adopters of this one.
pub fn decoded_texture_to_image(t: &DecodedTexture, q: TextureQuality) -> Image {
    let w = t.width.max(1);
    let h = t.height.max(1);
    let expected = (w as usize) * (h as usize) * 4;

    let mut rgba = t.rgba.clone();
    // A malformed/short buffer would break the box filter's indexing; fall back
    // to a tightly-sized buffer so downstream assumptions hold.
    rgba.resize(expected, 0);
    for px in rgba.chunks_exact_mut(4) {
        px[3] = ffxi_alpha_remap(px[3]);
    }

    image_with_mips(rgba, w, h, q, has_cutout_alpha(t))
}

/// Assemble a render-ready [`Image`] from final RGBA8-sRGB texels: optional
/// sRGB-correct mip chain + bilinear/anisotropic sampler. `cutout` preserves
/// alpha-test coverage across mips. The caller owns any alpha remap, so the zone
/// path (remapped) and the actor path (decoder alpha as-is) share one mip+sampler
/// builder — bare mip-0 textures alias into a shimmer under camera motion.
pub fn image_with_mips(
    mut rgba: Vec<u8>,
    w: u32,
    h: u32,
    q: TextureQuality,
    cutout: bool,
) -> Image {
    let w = w.max(1);
    let h = h.max(1);
    rgba.resize((w as usize) * (h as usize) * 4, 0);

    let (data, mip_level_count) = if q.mipmaps {
        let target_cov = if cutout {
            Some(alpha_coverage(&rgba))
        } else {
            None
        };
        build_mip_chain(rgba, w, h, target_cov)
    } else {
        (rgba, 1)
    };

    let mut img = Image::new_uninit(
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    img.texture_descriptor.mip_level_count = mip_level_count;
    img.data = Some(data);
    img.sampler = ImageSampler::Descriptor(sampler_descriptor(q.anisotropy));
    img
}

/// The sampler an MMB texture uses: bilinear min/mag + trilinear mips, repeat
/// wrap. `anisotropy > 1` raises the anisotropy clamp (filters stay Linear,
/// which anisotropy requires).
pub fn sampler_descriptor(anisotropy: u16) -> ImageSamplerDescriptor {
    ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: anisotropy.max(1),
        ..Default::default()
    }
}

fn mip_level_count(w: u32, h: u32) -> u32 {
    32 - w.max(h).max(1).leading_zeros()
}

fn build_mip_chain(mip0: Vec<u8>, w: u32, h: u32, target_cov: Option<f32>) -> (Vec<u8>, u32) {
    let levels = mip_level_count(w, h);
    let mut data = mip0.clone();
    let mut prev = mip0;
    let (mut pw, mut ph) = (w, h);
    for _ in 1..levels {
        let nw = (pw / 2).max(1);
        let nh = (ph / 2).max(1);
        let mut next = downsample_box_srgb(&prev, pw, ph, nw, nh);
        if let Some(cov) = target_cov {
            rescale_alpha_to_coverage(&mut next, cov);
        }
        data.extend_from_slice(&next);
        prev = next;
        pw = nw;
        ph = nh;
    }
    (data, levels)
}

#[inline]
fn srgb_to_linear(c: u8) -> f32 {
    let s = c as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
fn linear_to_srgb(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let s = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Box-filter downsample. RGB is averaged in **linear** space (the texture is
/// sRGB-encoded; averaging the bytes directly would darken the result); alpha is
/// linear already and averaged as-is.
fn downsample_box_srgb(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let (sw, sh, dw, dh) = (sw as usize, sh as usize, dw as usize, dh as usize);
    let mut out = vec![0u8; dw * dh * 4];
    for dy in 0..dh {
        for dx in 0..dw {
            let sx0 = (dx * 2).min(sw - 1);
            let sx1 = (dx * 2 + 1).min(sw - 1);
            let sy0 = (dy * 2).min(sh - 1);
            let sy1 = (dy * 2 + 1).min(sh - 1);
            let (mut lr, mut lg, mut lb, mut a) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for &(sx, sy) in &[(sx0, sy0), (sx1, sy0), (sx0, sy1), (sx1, sy1)] {
                let s = (sy * sw + sx) * 4;
                lr += srgb_to_linear(src[s]);
                lg += srgb_to_linear(src[s + 1]);
                lb += srgb_to_linear(src[s + 2]);
                a += src[s + 3] as f32;
            }
            let d = (dy * dw + dx) * 4;
            out[d] = linear_to_srgb(lr / 4.0);
            out[d + 1] = linear_to_srgb(lg / 4.0);
            out[d + 2] = linear_to_srgb(lb / 4.0);
            out[d + 3] = (a / 4.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

fn alpha_coverage(rgba: &[u8]) -> f32 {
    let n = rgba.len() / 4;
    if n == 0 {
        return 0.0;
    }
    let passing = rgba
        .chunks_exact(4)
        .filter(|p| p[3] >= MASK_THRESHOLD_U8)
        .count();
    passing as f32 / n as f32
}

/// Castaño's "Improved Alpha-Tested Magnification": scale this mip's alpha so the
/// fraction of texels above the mask threshold matches `target` (mip 0's
/// coverage). Binary search on the scale factor.
fn rescale_alpha_to_coverage(rgba: &mut [u8], target: f32) {
    let n = rgba.len() / 4;
    if n == 0 {
        return;
    }
    let thresh = MASK_THRESHOLD_U8 as f32;
    let (mut lo, mut hi) = (0.0f32, 4.0f32);
    let mut scale = 1.0f32;
    for _ in 0..10 {
        scale = 0.5 * (lo + hi);
        let cov = rgba
            .chunks_exact(4)
            .filter(|p| (p[3] as f32 * scale).min(255.0) >= thresh)
            .count() as f32
            / n as f32;
        if cov < target {
            lo = scale;
        } else {
            hi = scale;
        }
    }
    for p in rgba.chunks_exact_mut(4) {
        p[3] = (p[3] as f32 * scale).min(255.0).round() as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::texture::TexFormat;

    fn tex(width: u32, height: u32, rgba: Vec<u8>) -> DecodedTexture {
        DecodedTexture {
            width,
            height,
            format_tag: TexFormat::Bgra32,
            rgba,
        }
    }

    #[test]
    fn alpha_remap_obeys_lotus_spec() {
        assert_eq!(ffxi_alpha_remap(0x00), 0);
        assert_eq!(ffxi_alpha_remap(0x0F), 0, "top nibble 0 -> transparent");
        assert_eq!(ffxi_alpha_remap(0x80), 255);
        assert_eq!(ffxi_alpha_remap(0xFF), 255);
    }

    #[test]
    fn cutout_detected_from_raw_alpha() {
        let opaque = tex(1, 1, vec![10, 20, 30, 0xFF]);
        assert!(!has_cutout_alpha(&opaque));
        let cut = tex(1, 1, vec![10, 20, 30, 0x08]);
        assert!(has_cutout_alpha(&cut), "raw alpha < 16 -> cutout");
    }

    #[test]
    fn rgb_is_never_modified() {
        // A transparent black texel beside an opaque one must keep its black RGB
        // (palettized textures store real colour in low-alpha texels; XIM never
        // bleeds). 2x1, no mips.
        let t = tex(2, 1, vec![0, 200, 0, 255, 0, 0, 0, 0]);
        let img = decoded_texture_to_image(&t, TextureQuality::default());
        let data = img.data.unwrap();
        assert_eq!(&data[4..7], &[0, 0, 0], "transparent texel RGB untouched");
    }

    #[test]
    fn vanilla_is_single_level() {
        let t = tex(4, 4, vec![255; 4 * 4 * 4]);
        let img = decoded_texture_to_image(&t, TextureQuality::default());
        assert_eq!(
            img.texture_descriptor.mip_level_count, 1,
            "no mips by default"
        );
        assert_eq!(img.data.unwrap().len(), 4 * 4 * 4);
    }

    #[test]
    fn mip_chain_byte_length_matches_descriptor() {
        // 4x4 opaque white.
        let t = tex(4, 4, vec![255; 4 * 4 * 4]);
        let img = decoded_texture_to_image(
            &t,
            TextureQuality {
                mipmaps: true,
                anisotropy: 1,
            },
        );
        let levels = img.texture_descriptor.mip_level_count;
        assert_eq!(levels, 3, "4x4 -> mips 4,2,1");
        let mut expected = 0usize;
        let (mut w, mut h) = (4u32, 4u32);
        for _ in 0..levels {
            expected += (w * h * 4) as usize;
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        assert_eq!(img.data.as_ref().unwrap().len(), expected);
    }

    #[test]
    fn coverage_preserving_holds_cutout_fraction() {
        // 4x4 with exactly half the texels opaque (checkerboard). Coverage at
        // mip 0 is 0.5; the 1x1 mip's single texel must stay above threshold so
        // the cutout doesn't vanish.
        let mut rgba = Vec::new();
        for i in 0..16 {
            let a = if i % 2 == 0 { 0xFFu8 } else { 0x00u8 };
            rgba.extend_from_slice(&[120, 120, 120, a]);
        }
        let t = tex(4, 4, rgba);
        let img = decoded_texture_to_image(
            &t,
            TextureQuality {
                mipmaps: true,
                anisotropy: 1,
            },
        );
        let data = img.data.unwrap();
        // Last mip is the final 4 bytes (1x1).
        let last_alpha = data[data.len() - 1];
        assert!(
            last_alpha >= MASK_THRESHOLD_U8,
            "1x1 mip alpha {last_alpha} fell below threshold — cutout dissolved"
        );
    }
}
