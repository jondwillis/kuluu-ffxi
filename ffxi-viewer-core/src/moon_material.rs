//! Custom Bevy material for the moon billboard. Procedurally shades a
//! cratered grey disc and applies an LSB-faithful phase terminator
//! mask. See `moon.wgsl` for the fragment shader.
//!
//! The moon rides as a flat `Rectangle` quad parented to the camera
//! sky-rig in [`crate::sun_moon`]. `sun_moon_system` updates the
//! material's `params` each frame from the current `VanaSky`.

use std::sync::Arc;

use bevy::asset::{embedded_asset, RenderAssetUsages};
use bevy::image::ImageSampler;
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;

#[derive(Clone, Debug, ShaderType)]
pub struct MoonUniform {
    /// `xyz` = weekday/horizon tint, `w` = **has-texture flag**: 0 = use
    /// the procedural grey surface, 1 = sample the real lunar texture.
    pub tint: Vec4,
    /// `x` = illumination [0,1] (1 = full, 0 = new),
    /// `y` = waxing sign (+1 for waxing, -1 for waning),
    /// `z` = overall brightness multiplier,
    /// `w` = earthshine strength.
    pub params: Vec4,
}

impl Default for MoonUniform {
    fn default() -> Self {
        Self {
            // tint.w = 0 → procedural surface until the texture loads.
            tint: Vec4::new(0.92, 0.95, 1.00, 0.0),
            params: Vec4::new(1.0, 1.0, 1.0, 0.0),
        }
    }
}

#[derive(Asset, AsBindGroup, Clone, Debug, TypePath, Default)]
pub struct MoonMaterial {
    #[uniform(0)]
    pub data: MoonUniform,
    /// Real lunar surface texture loaded from the FFXI DAT (file 55660,
    /// `Menu:MoonPhases`). `None` until loaded — AsBindGroup binds a
    /// default white texture in the meantime, and `tint.w` gates whether
    /// the shader actually samples it.
    #[texture(1)]
    #[sampler(2)]
    pub surface: Option<Handle<Image>>,
}

/// DAT root for loading the lunar texture, injected by the front-end —
/// mirrors [`crate::minimap::retail::MinimapDatRoot`]. `None` (headless /
/// no install) leaves the moon on its procedural surface.
#[derive(Resource, Default, Clone)]
pub struct MoonDatRoot(pub Option<Arc<ffxi_dat::DatRoot>>);

/// Candidate file ids for the moon texture (`Menu:MoonPhases`), from
/// POLUtils ROMFileMappings.xml — regional variants. Tried in order;
/// the first that resolves and decodes wins.
const MOON_TEXTURE_FILE_IDS: &[u32] = &[55660, 55780, 56200, 55540];

impl Material for MoonMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://ffxi_viewer_core/moon.wgsl".into()
    }

    // Blend so the soft anti-aliased edge composes against the sky.
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

/// One-shot loader: resolve the moon texture from the DAT, decode it,
/// and bind it onto the live [`MoonMaterial`] (the cached handle in
/// [`crate::sun_moon::CelestialMaterials`]). Retries each frame until it
/// succeeds or there's no DAT root — `done` latches so we don't re-read
/// the DAT every tick once loaded (or once we've given up on a tick
/// where the material handle isn't ready yet).
fn load_moon_texture(
    dat_root: Option<Res<MoonDatRoot>>,
    celestial: Option<Res<crate::sun_moon::CelestialMaterials>>,
    mut moon_materials: ResMut<Assets<MoonMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    let Some(dat_root) = dat_root.and_then(|r| r.0.clone()) else {
        // No DAT install registered — stay procedural, stop retrying.
        *done = true;
        return;
    };
    // Need the material handle to exist before we can bind onto it.
    let Some(celestial) = celestial else {
        return;
    };

    for &file_id in MOON_TEXTURE_FILE_IDS {
        let Ok(loc) = dat_root.resolve(file_id) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(loc.path_under(dat_root.root())) else {
            continue;
        };
        // Largest Graphic by pixel count — same heuristic the minimap
        // uses for map DATs that ship overlay glyphs alongside the art.
        let Some(graphic) = ffxi_dat::map_image::scan_graphics(&bytes)
            .max_by_key(|g| g.width * g.height)
        else {
            continue;
        };
        let mut image = Image::new(
            Extent3d {
                width: graphic.width,
                height: graphic.height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            graphic.rgba,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        image.sampler = ImageSampler::linear();
        let handle = images.add(image);

        if let Some(mat) = moon_materials.get_mut(&celestial.moon) {
            mat.surface = Some(handle);
            mat.data.tint.w = 1.0; // flip the shader to textured surface
            info!(
                "moon: loaded lunar texture from file {} ({}×{})",
                file_id, graphic.width, graphic.height
            );
        }
        *done = true;
        return;
    }

    warn!(
        "moon: no lunar texture found in candidate files {:?}; staying procedural",
        MOON_TEXTURE_FILE_IDS
    );
    *done = true;
}

/// Registers [`MoonMaterial`] and embeds its WGSL.
pub struct MoonMaterialPlugin;

impl Plugin for MoonMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "moon.wgsl");
        app.init_resource::<MoonDatRoot>()
            .add_plugins(MaterialPlugin::<MoonMaterial>::default())
            .add_systems(Update, load_moon_texture);
    }
}
