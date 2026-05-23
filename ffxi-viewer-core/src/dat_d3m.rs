//! D3M effect-geometry → Bevy mesh + material.
//!
//! D3M chunks (kind `0x1F`, parsed in `ffxi_dat::d3m`) carry the
//! triangle billboards FFXI uses for particle effects: flame trails,
//! magic glyphs, status auras. Each chunk is a flat triangle list with
//! HDR-capable per-vertex color (lotus stores ARGB bytes / 128.0 so
//! values above 1.0 drive bloom).
//!
//! This module supplies the converter from `D3m` to `Mesh` plus a
//! three-variant material factory. The actual *spawn site* (binding
//! the mesh to an actor entity, marking lifecycle, picking a blend
//! mode from the source Generator) lives in Stage D2 — this module
//! is the renderer-side primitives.
//!
//! Render notes:
//!
//! * `unlit: true`. Particles are emissive sprites — additive/blended
//!   contribution must survive low-light scenes where a PBR shading
//!   term would drop their fragment color to near zero. The other DAT
//!   formats (MMB / VOS2 / MZB) render fine through PBR; D3M is the
//!   one that genuinely needs the unlit bypass.
//! * `cull_mode: None`. Particle quads expand into both-faces tris and
//!   FFXI does not author them with consistent winding.
//! * `Mesh::ATTRIBUTE_COLOR` carries the per-vertex `D3mVertex::color`
//!   floats. The bevy shader multiplies into `base_color`, so values
//!   above 1.0 in the alpha channel are clamped — the high-magnitude
//!   color components survive and create the bloom-eligible HDR look.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use ffxi_dat::d3m::D3m;

/// FFXI D3M blend modes. Lotus wraps each D3M in three Vulkan pipelines
/// (`pipeline_add`, `pipeline_blend`, `pipeline_sub`); we map to Bevy's
/// `AlphaMode` variants. The choice between them is made per-Generator
/// (the parent that *spawns* the D3M), not encoded in the D3M chunk
/// itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum D3mBlendMode {
    /// Light-emitting particles: flame, magic glyph, spell glow.
    /// Bevy: [`AlphaMode::Add`]. Maps to lotus's `pipeline_add`.
    #[default]
    Additive,
    /// Translucent particles: smoke, water spray, dust. Bevy:
    /// [`AlphaMode::Blend`]. Maps to lotus's `pipeline_blend`.
    Blended,
    /// Darkening particles: shadow auras, sleep clouds. Bevy:
    /// [`AlphaMode::Multiply`] (closest analogue to lotus's
    /// `pipeline_sub`, which subtracts the fragment's color from the
    /// framebuffer).
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

/// Convert a parsed [`D3m`] into a Bevy [`Mesh`]. D3M chunks store
/// triangles flat (3 verts per tri, no index buffer); we emit a
/// sequential `u32` index buffer to satisfy Bevy's triangle-list
/// topology.
///
/// Returns an empty mesh when `d3m.vertices` is empty (legal — a
/// D3M with `num_triangles == 0` is structurally valid and acts as
/// a no-op placeholder).
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

/// Build a [`StandardMaterial`] for a D3M with the given blend mode.
/// `texture` may be `None` for solid-color particles (lotus uses a
/// 1×1 white texture for the same fallback).
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
        let d = synth_d3m(2); // 6 verts, 2 triangles
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
