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
    use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    let mut image = Image::new(
        Extent3d {
            width: t.width,
            height: t.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        t.rgba.clone(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    // Scrolling water sheets (zone-static generators) drive UVs past [0,1]; Repeat
    // tiles the sprite instead of smearing the edge texel.
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..default()
    });
    image
}

pub fn d3m_material(blend: D3mBlendMode, texture: Option<Handle<Image>>) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: texture,
        unlit: true,
        alpha_mode: blend.alpha_mode(),
        cull_mode: None,
        ..default()
    }
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

    #[test]
    fn material_is_unlit_and_two_sided() {
        let mat = d3m_material(D3mBlendMode::Additive, None);
        assert!(mat.unlit);
        assert_eq!(mat.cull_mode, None);
        assert_eq!(mat.alpha_mode, AlphaMode::Add);
    }
}
