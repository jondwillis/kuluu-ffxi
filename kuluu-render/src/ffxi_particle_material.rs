#![cfg(not(target_arch = "wasm32"))]

use bevy::asset::embedded_asset;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, CompareFunction, DepthBiasState, DepthStencilState, RenderPipelineDescriptor,
    ShaderType, SpecializedMeshPipelineError, StencilFaceState, StencilState, TextureFormat,
};
use bevy::shader::ShaderRef;

use ffxi_dat::particle_gen::{ParticleBlend, ParticleGeneratorDef};

use crate::dat_d3m::D3mBlendMode;
use crate::element_sort::transparent_sort_bias;
use crate::particle_sim::{ignores_texture_alpha, D3mDrawPath};

// `ffxi_particle.wgsl`'s `PREMULTIPLY_*`: which premultiply the blend state this alpha mode
// resolves to expects. Bevy applies these inside `pbr_functions::premultiply_alpha`, which
// only the StandardMaterial shader calls, so a custom fragment shader owes them itself.
const PREMULTIPLY_NONE: f32 = 0.0;
const PREMULTIPLY_ADD: f32 = 1.0;
const PREMULTIPLY_MULTIPLY: f32 = 2.0;

/// `ffxi_particle.wgsl`'s `FOG_*`: params.y selects the element's distance fog.
const FOG_OFF: f32 = 0.0;
const FOG_ZONE: f32 = 1.0;
const FOG_BLACK: f32 = 2.0;

// CMoElem.cpp CMoElem::PrepDX — blend byte 0x48 is the one additive case that swaps the area's
// fog colour for black, so its far-off elements fade out instead of adding the horizon tint.
const FOG_BLACK_BLEND_BYTE: u8 = 0x48;

// `ffxi_particle.wgsl`'s PATH_*: which fixed-function table the element rides. The tables
// differ per mesh source (CMoD3m.cpp TSS blocks vs ZoneRenderer.cpp DoD3mDraw), not per
// generator, so the selector is a property of the resolved draw path.
const PATH_D3M_TEXTURED: f32 = 0.0;
const PATH_D3M_UNTEXTURED: f32 = 1.0;
const PATH_MMB_TEXTURED: f32 = 2.0;
const PATH_LAMP_ALPHAMAP: f32 = 3.0;

// Forward depth bias for every particle element. Wash volumes like South Gustaberg's
// ghu1 are copies of the wall geometry drawn additively on top of it; without a bias
// they tie with the stone and depth-fight per triangle. The view depth buffer is
// reversed-Z (compare GreaterEqual), so a positive bias moves toward the camera.
const PARTICLE_FORWARD_BIAS: i32 = 1;
const PARTICLE_FORWARD_SLOPE_SCALE: f32 = 1.0;

fn particle_depth_bias() -> DepthBiasState {
    DepthBiasState {
        constant: PARTICLE_FORWARD_BIAS,
        slope_scale: PARTICLE_FORWARD_SLOPE_SCALE,
        clamp: 0.0,
    }
}

impl D3mDrawPath {
    fn selector(self) -> f32 {
        match self {
            Self::D3m => PATH_D3M_TEXTURED,
            Self::D3mUntextured | Self::MmbUntextured => PATH_D3M_UNTEXTURED,
            Self::Mmb => PATH_MMB_TEXTURED,
        }
    }
}

// research/XIClient/src/XIClient/source/World/Generator/Effects/CMoElem.cpp CMoElem::PrepDX —
// the per-element D3DRS_FOGENABLE / D3DRS_FOGCOLOR choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticleFog {
    Off,
    Zone,
    Black,
}

impl ParticleFog {
    pub fn for_def(def: &ffxi_dat::particle_gen::ParticleGeneratorDef) -> Self {
        if !def.fog_enabled {
            Self::Off
        } else if def.blend_byte == FOG_BLACK_BLEND_BYTE {
            Self::Black
        } else {
            Self::Zone
        }
    }

    fn selector(self) -> f32 {
        match self {
            Self::Off => FOG_OFF,
            Self::Zone => FOG_ZONE,
            Self::Black => FOG_BLACK,
        }
    }
}

#[derive(Clone, Debug, ShaderType)]
pub struct ParticleUniform {
    pub params: Vec4,
}

#[derive(Asset, AsBindGroup, Clone, Debug, TypePath)]
#[bind_group_data(FfxiParticleMaterialKey)]
pub struct FfxiParticleMaterial {
    #[uniform(0)]
    pub data: ParticleUniform,

    #[texture(1)]
    #[sampler(2)]
    pub texture: Option<Handle<Image>>,

    pub alpha_mode: AlphaMode,
    // element_sort.rs — added to the view distance in the transparent sort.
    pub sort_bias: f32,
    pub depth_write: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FfxiParticleMaterialKey {
    pub depth_write: bool,
}

impl From<&FfxiParticleMaterial> for FfxiParticleMaterialKey {
    fn from(m: &FfxiParticleMaterial) -> Self {
        Self {
            depth_write: m.depth_write,
        }
    }
}

impl FfxiParticleMaterial {
    pub fn for_def(
        def: &ParticleGeneratorDef,
        texture: Option<Handle<Image>>,
        dat_offset: usize,
        path: D3mDrawPath,
    ) -> Self {
        let blend = match def.blend {
            ParticleBlend::Additive => D3mBlendMode::Additive,
            ParticleBlend::Blend => D3mBlendMode::Blended,
            ParticleBlend::Subtract => D3mBlendMode::Subtractive,
        };
        let mut material = Self::new(
            blend,
            texture,
            ParticleFog::for_def(def),
            transparent_sort_bias(def, dat_offset),
            def.depth_write,
            path,
            ignores_texture_alpha(def, path),
        );
        if crate::particle_sim::is_lamp_halo_def(def) {
            material.data.params.w = PATH_LAMP_ALPHAMAP;
            // The glow sheets pin in chunk order: depth-sorted, they re-sort against each other
            // frame to frame and dance under/over one another as the camera moves.
            material.sort_bias = crate::element_sort::pinned_order_bias(dat_offset);
        }
        material
    }

    pub fn new(
        blend: D3mBlendMode,
        texture: Option<Handle<Image>>,
        fog: ParticleFog,
        sort_bias: f32,
        depth_write: bool,
        path: D3mDrawPath,
        ignore_texture_alpha: bool,
    ) -> Self {
        let alpha_mode = blend.alpha_mode();
        let premultiply = match alpha_mode {
            AlphaMode::Add => PREMULTIPLY_ADD,
            AlphaMode::Multiply => PREMULTIPLY_MULTIPLY,
            _ => PREMULTIPLY_NONE,
        };
        Self {
            data: ParticleUniform {
                params: Vec4::new(
                    premultiply,
                    fog.selector(),
                    f32::from(ignore_texture_alpha),
                    path.selector(),
                ),
            },
            texture,
            alpha_mode,
            sort_bias,
            depth_write,
        }
    }
}

impl Material for FfxiParticleMaterial {
    // A custom @vertex stage: the per-particle factor rides the TANGENT slot (location 4),
    // which the default mesh vertex function does not forward.
    fn vertex_shader() -> ShaderRef {
        "embedded://kuluu_render/ffxi_particle.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://kuluu_render/ffxi_particle.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }

    fn depth_bias(&self) -> f32 {
        self.sort_bias
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // CMoElem::PrepDX sets D3DRS_CULLMODE to D3DCULL_NONE for every particle element
        // (research/XIClient/src/XIClient/source/World/Generator/Effects/CMoElem.cpp CMoElem::PrepDX).
        descriptor.primitive.cull_mode = None;
        // MaterialPlugin pipelines ship no depth-stencil state, and wgpu builds a pipeline with
        // none as NO depth test at all — every additive element painted over the walls. State is
        // explicit here: compare GreaterEqual matches bevy_pbr's 3D pipeline (reversed-Z view
        // buffer), write follows the element's own bit (CMoElem.cpp CMoElem::PrepDX —
        // D3DRS_ZWRITEENABLE; a depth-writing element like Lower Jeuno's `down` sea floor writes
        // from the transparent pass Bevy otherwise keeps read-only).
        descriptor.depth_stencil = Some(DepthStencilState {
            format: TextureFormat::Depth32Float,
            depth_compare: Some(CompareFunction::GreaterEqual),
            depth_write_enabled: Some(key.bind_group_data.depth_write),
            stencil: StencilState {
                front: StencilFaceState::IGNORE,
                back: StencilFaceState::IGNORE,
                read_mask: 0,
                write_mask: 0,
            },
            bias: particle_depth_bias(),
        });
        Ok(())
    }
}

pub struct FfxiParticleMaterialPlugin;

impl Plugin for FfxiParticleMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "ffxi_particle.wgsl");
        app.add_plugins(MaterialPlugin::<FfxiParticleMaterial>::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Bevy resolves Add and Premultiplied to one BlendState and distinguishes them only by
    // what the fragment shader emits, so the premultiply selector has to track alpha_mode.
    #[test]
    fn premultiply_selector_tracks_the_blend_mode() {
        let sel = |b| {
            FfxiParticleMaterial::new(
                b,
                None,
                ParticleFog::Zone,
                0.0,
                false,
                D3mDrawPath::D3m,
                false,
            )
            .data
            .params
            .x
        };
        assert_eq!(sel(D3mBlendMode::Additive), PREMULTIPLY_ADD);
        assert_eq!(sel(D3mBlendMode::Subtractive), PREMULTIPLY_MULTIPLY);
        assert_eq!(sel(D3mBlendMode::Blended), PREMULTIPLY_NONE);
    }

    #[test]
    fn fog_selector_follows_the_render_state_and_blend_byte() {
        let mut def = ParticleGeneratorDef {
            fog_enabled: true,
            blend_byte: 0x03,
            ..Default::default()
        };
        assert_eq!(ParticleFog::for_def(&def), ParticleFog::Zone);
        def.blend_byte = FOG_BLACK_BLEND_BYTE;
        assert_eq!(ParticleFog::for_def(&def), ParticleFog::Black);
        def.fog_enabled = false;
        assert_eq!(ParticleFog::for_def(&def), ParticleFog::Off);
        let sel = |f: ParticleFog| {
            FfxiParticleMaterial::new(
                D3mBlendMode::Blended,
                None,
                f,
                0.0,
                false,
                D3mDrawPath::D3m,
                false,
            )
            .data
            .params
            .y
        };
        assert_eq!(sel(ParticleFog::Off), FOG_OFF);
        assert_eq!(sel(ParticleFog::Zone), FOG_ZONE);
        assert_eq!(sel(ParticleFog::Black), FOG_BLACK);
    }

    #[test]
    fn for_def_carries_blend_fog_sort_and_depth_write() {
        let def = ParticleGeneratorDef {
            blend: ParticleBlend::Blend,
            fog_enabled: true,
            draw_priority: ffxi_dat::particle_gen::DrawPriority::Low,
            depth_write: true,
            ..Default::default()
        };
        let m = FfxiParticleMaterial::for_def(&def, None, 0, D3mDrawPath::D3m);
        assert_eq!(m.alpha_mode, D3mBlendMode::Blended.alpha_mode());
        assert_eq!(m.data.params.y, FOG_ZONE);
        assert_eq!(m.sort_bias, transparent_sort_bias(&def, 0));
        assert!(m.depth_write);
        assert!(FfxiParticleMaterialKey::from(&m).depth_write);
    }

    #[test]
    fn alpha_mode_matches_the_d3m_blend_mode() {
        for blend in [
            D3mBlendMode::Additive,
            D3mBlendMode::Blended,
            D3mBlendMode::Subtractive,
        ] {
            assert_eq!(
                FfxiParticleMaterial::new(
                    blend,
                    None,
                    ParticleFog::Zone,
                    0.0,
                    false,
                    D3mDrawPath::D3m,
                    false
                )
                .alpha_mode,
                blend.alpha_mode()
            );
        }
    }

    #[test]
    fn particle_elements_bias_toward_the_camera() {
        let bias = particle_depth_bias();
        assert!(bias.constant > 0);
        assert!(bias.slope_scale > 0.0);
        assert_eq!(bias.clamp, 0.0);
    }
}
