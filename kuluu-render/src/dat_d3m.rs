use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use ffxi_dat::d3m::D3m;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum D3mBlendMode {
    #[default]
    Additive,

    Blended,

    Subtractive,
}

impl D3mBlendMode {
    pub fn alpha_mode(self) -> AlphaMode {
        match self {
            Self::Additive => AlphaMode::Add,
            Self::Blended => AlphaMode::Blend,
            Self::Subtractive => AlphaMode::Multiply,
        }
    }
}

pub fn d3m_to_mesh(d3m: &D3m) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    let positions: Vec<[f32; 3]> = d3m.vertices.iter().map(|v| v.pos).collect();
    let normals: Vec<[f32; 3]> = d3m.vertices.iter().map(|v| v.normal).collect();
    let uvs: Vec<[f32; 2]> = d3m.vertices.iter().map(|v| v.uv).collect();
    let colors: Vec<[f32; 4]> = d3m.vertices.iter().map(|v| v.color).collect();
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    let indices: Vec<u32> = (0..d3m.vertices.len() as u32).collect();
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

pub fn decoded_texture_to_image(t: &ffxi_dat::texture::DecodedTexture) -> Image {
    convert(t, false)
}

/// [`decoded_texture_to_image`] plus [`ffxi_dat::texture::resolve_dxt3_alpha_dither`], for the
/// camera-follow celestial billboards.
///
/// Same case as `zone_texture::decoded_sky_texture_to_image`: the celestial set rides a sphere
/// of radius `CELESTIAL_DISTANCE` around the camera, so its on-screen size is set by its own
/// scale rather than by distance, and its sheets are 4-bit-alpha DXT3 (`dat-sky-alpha-histogram`
/// on zone files 210/331: `weat/<type>/kasa` is 100% the nibble 7/8 dithered-opaque pair,
/// `moonshap` a nibble ramp). kuluu-u5mm already made this call for `moonshap` on the
/// moon-material path (moon_material.rs load_moon_sprite_sheet), so the two paths agree on the one sheet they share.
/// Every other D3M particle sheet keeps the plain converter — their on-screen sampling has not
/// been surveyed, and `resolve_dxt3_alpha_dither` declines non-nibble alpha anyway, so widening
/// the set is an argument to make per set, not a correctness gap (kuluu-d9wv).
pub fn decoded_sky_texture_to_image(t: &ffxi_dat::texture::DecodedTexture) -> Image {
    convert(t, true)
}

/// Convert one decoded D3M texture; when `undither` is set the DXT3 alpha dither is
/// resolved before the remap, which doubles whatever alpha it is handed — see the note
/// on `resolve_dxt3_alpha_dither`.
fn convert(t: &ffxi_dat::texture::DecodedTexture, undither: bool) -> Image {
    use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    let mut rgba = t.rgba.clone();
    if undither {
        ffxi_dat::texture::resolve_dxt3_alpha_dither(&mut rgba, t.width, t.height);
    }
    ffxi_dat::texture::apply_ffxi_alpha_remap(&mut rgba);
    let mut image = Image::new(
        Extent3d {
            width: t.width,
            height: t.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    // Scrolling water sheets (zone-static generators) drive UVs past [0,1]; Repeat
    // tiles the sprite instead of smearing the edge texel.
    //
    // Bevy's default descriptor filters Nearest, which no FFXI surface does: a particle sheet is
    // drawn magnified (the dust-storm sheet is a 128px texture over a 6x4 quad a few yalms from
    // the camera) and reads as blocks of texels. Bilinear matches what zone_texture's
    // `sampler_descriptor` gives every other DAT texture; mips need the graphics-settings
    // TextureQuality this path has no access to.
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::d3m::D3mVertex;

    fn synth_d3m(num_triangles: u16) -> D3m {
        let mut verts = Vec::with_capacity((num_triangles as usize) * 3);
        for i in 0..(num_triangles as usize) * 3 {
            verts.push(D3mVertex {
                pos: [i as f32, 0.0, 0.0],
                normal: [0.0, 1.0, 0.0],
                color: [1.0, 1.0, 1.0, 1.0],
                uv: [0.0, 0.0],
            });
        }
        D3m {
            name: *b"d3m0",
            num_triangles,
            texture_name: *b"flame_a\0\0\0\0\0\0\0\0\0",
            vertices: verts,
        }
    }

    #[test]
    fn blend_mode_maps_to_bevy_alpha() {
        assert_eq!(D3mBlendMode::Additive.alpha_mode(), AlphaMode::Add);
        assert_eq!(D3mBlendMode::Blended.alpha_mode(), AlphaMode::Blend);
        assert_eq!(D3mBlendMode::Subtractive.alpha_mode(), AlphaMode::Multiply);
    }

    #[test]
    fn mesh_has_one_index_per_vertex() {
        let d = synth_d3m(2);
        let mesh = d3m_to_mesh(&d);
        assert!(mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .is_some_and(|a| a.len() == 6));
        match mesh.indices().unwrap() {
            Indices::U32(idx) => {
                assert_eq!(idx.len(), 6);
                assert_eq!(idx[0], 0);
                assert_eq!(idx[5], 5);
            }
            _ => panic!("expected u32 indices"),
        }
    }

    #[test]
    fn empty_d3m_produces_empty_mesh() {
        let d = synth_d3m(0);
        let mesh = d3m_to_mesh(&d);
        assert_eq!(
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
                .map(|a| a.len())
                .unwrap_or(0),
            0
        );
    }

    /// A DXT3 alpha plane holds nibble multiples only, so an authored half-opaque 0x80 ships
    /// as the nibble 7/8 pair stippled across neighbours - what `weat/<type>/kasa` is, end to
    /// end. The sky converter averages that back out; the shared particle converter the other
    /// D3M sheets go through leaves the stipple in place.
    /// texture.rs
    #[test]
    fn only_the_sky_converter_resolves_the_dxt3_alpha_stipple() {
        use ffxi_dat::texture::{ffxi_alpha_remap, DecodedTexture, TexFormat};

        const DITHER_LO: u8 = 0x77;
        const DITHER_HI: u8 = 0x88;
        const SIDE: u32 = 8;
        /// 0x80's recovered mean is 127.5, which no 8-bit alpha holds; the remap doubles that
        /// to a 254/255 split. One step is the floor, not a slack tolerance.
        /// texture.rs
        const RESOLVED_RESIDUAL_MAX: u8 = 1;

        let mut rgba = Vec::with_capacity((SIDE * SIDE * 4) as usize);
        for y in 0..SIDE {
            for x in 0..SIDE {
                let a = if (x + y) % 2 == 0 {
                    DITHER_LO
                } else {
                    DITHER_HI
                };
                rgba.extend_from_slice(&[40, 50, 60, a]);
            }
        }
        let t = DecodedTexture {
            width: SIDE,
            height: SIDE,
            format_tag: TexFormat::Dxt3,
            rgba,
        };

        let spread = |img: Image| {
            let alpha: Vec<u8> = img
                .data
                .expect("converted image carries its texels")
                .chunks_exact(4)
                .map(|p| p[3])
                .collect();
            let lo = *alpha.iter().min().expect("non-empty");
            let hi = *alpha.iter().max().expect("non-empty");
            (lo, hi - lo)
        };

        let stipple = ffxi_alpha_remap(DITHER_HI) - ffxi_alpha_remap(DITHER_LO);
        assert_eq!(
            spread(decoded_texture_to_image(&t)),
            (ffxi_alpha_remap(DITHER_LO), stipple),
            "the shared particle converter must not undither"
        );
        let (sky_lo, sky_spread) = spread(decoded_sky_texture_to_image(&t));
        assert!(
            sky_spread <= RESOLVED_RESIDUAL_MAX && sky_lo > ffxi_alpha_remap(DITHER_LO),
            "celestial converter left alpha spread {sky_spread} from {sky_lo}"
        );
    }
}
