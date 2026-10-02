#![cfg(not(target_arch = "wasm32"))]

use bevy::asset::embedded_asset;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;

use ffxi_dat::particle_gen::{ParticleBlend, ParticleGeneratorDef};

use crate::dat_d3m::D3mBlendMode;
use crate::element_sort::transparent_sort_bias;

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
    ) -> Self {
        let blend = match def.blend {
            ParticleBlend::Additive => D3mBlendMode::Additive,
            ParticleBlend::Blend => D3mBlendMode::Blended,
            ParticleBlend::Subtract => D3mBlendMode::Subtractive,
        };
        Self::new(
            blend,
            texture,
            ParticleFog::for_def(def),
            transparent_sort_bias(def, dat_offset),
            def.depth_write,
        )
    }

    pub fn new(
        blend: D3mBlendMode,
        texture: Option<Handle<Image>>,
        fog: ParticleFog,
        sort_bias: f32,
        depth_write: bool,
    ) -> Self {
        let alpha_mode = blend.alpha_mode();
        let premultiply = match alpha_mode {
            AlphaMode::Add => PREMULTIPLY_ADD,
            AlphaMode::Multiply => PREMULTIPLY_MULTIPLY,
            _ => PREMULTIPLY_NONE,
        };
        Self {
            data: ParticleUniform {
                params: Vec4::new(premultiply, fog.selector(), 0.0, 0.0),
            },
            texture,
            alpha_mode,
            sort_bias,
            depth_write,
        }
    }
}

impl Material for FfxiParticleMaterial {
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
        // CMoElem.cpp CMoElem::PrepDX — D3DRS_ZWRITEENABLE follows the element's own bit, so a
        // depth-writing element (Lower Jeuno's `down` sea floor) writes from the transparent
        // pass Bevy otherwise keeps read-only.
        if key.bind_group_data.depth_write {
            if let Some(ds) = descriptor.depth_stencil.as_mut() {
                ds.depth_write_enabled = Some(true);
            }
        }
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
            FfxiParticleMaterial::new(b, None, ParticleFog::Zone, 0.0, false)
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
            FfxiParticleMaterial::new(D3mBlendMode::Blended, None, f, 0.0, false)
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
        let m = FfxiParticleMaterial::for_def(&def, None, 0);
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
                FfxiParticleMaterial::new(blend, None, ParticleFog::Zone, 0.0, false).alpha_mode,
                blend.alpha_mode()
            );
        }
    }
}
