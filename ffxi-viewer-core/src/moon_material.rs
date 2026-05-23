//! Custom Bevy material for the moon billboard. Procedurally shades a
//! cratered grey disc and applies an LSB-faithful phase terminator
//! mask. See `moon.wgsl` for the fragment shader.
//!
//! The moon rides as a flat `Rectangle` quad parented to the camera
//! sky-rig in [`crate::sun_moon`]. `sun_moon_system` updates the
//! material's `params` each frame from the current `VanaSky`.

use bevy::asset::embedded_asset;
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

#[derive(Clone, Debug, ShaderType)]
pub struct MoonUniform {
    pub tint: Vec4,
    /// `x` = illumination [0,1] (1 = full, 0 = new),
    /// `y` = waxing sign (+1 for waxing, -1 for waning),
    /// `z` = overall brightness multiplier,
    /// `w` = reserved.
    pub params: Vec4,
}

impl Default for MoonUniform {
    fn default() -> Self {
        Self {
            tint: Vec4::new(0.92, 0.95, 1.00, 1.0),
            params: Vec4::new(1.0, 1.0, 1.0, 0.0),
        }
    }
}

#[derive(Asset, AsBindGroup, Clone, Debug, TypePath, Default)]
pub struct MoonMaterial {
    #[uniform(0)]
    pub data: MoonUniform,
}

impl Material for MoonMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://ffxi_viewer_core/moon.wgsl".into()
    }

    // Blend so the soft anti-aliased edge composes against the sky.
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

/// Registers [`MoonMaterial`] and embeds its WGSL.
pub struct MoonMaterialPlugin;

impl Plugin for MoonMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "moon.wgsl");
        app.add_plugins(MaterialPlugin::<MoonMaterial>::default());
    }
}
