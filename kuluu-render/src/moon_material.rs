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
    // rgb = tint, w = mode: 0 = procedural disc, 2 = retail sprite sheet.
    pub tint: Vec4,

    pub params: Vec4,

    // Current phase frame's sub-rect in the sprite-sheet texture (u0,v0,u1,v1).
    pub frame_uv: Vec4,
}

impl Default for MoonUniform {
    fn default() -> Self {
        Self {
            tint: Vec4::new(0.92, 0.95, 1.00, 0.0),
            params: Vec4::new(1.0, 1.0, 1.0, 0.0),
            frame_uv: Vec4::new(0.0, 0.0, 1.0, 1.0),
        }
    }
}

#[derive(Asset, AsBindGroup, Clone, Debug, TypePath, Default)]
pub struct MoonMaterial {
    #[uniform(0)]
    pub data: MoonUniform,

    #[texture(1)]
    #[sampler(2)]
    pub surface: Option<Handle<Image>>,
}

#[derive(Resource, Default, Clone)]
pub struct MoonDatRoot(pub Option<Arc<ffxi_dat::DatRoot>>);

// The 12 retail phase-frame UV rects (u0,v0,u1,v1) for the current zone, or None
// indoors / where no moon sprite sheet exists (then the moon stays procedural).
#[derive(Resource, Default, Clone)]
pub struct MoonSpriteFrames(pub Option<[Vec4; ffxi_dat::sprite_sheet::MOON_PHASE_FRAMES]>);

// Retail day-of-week (0x4E, 8xRGBA) / moon-phase (0x4F, 12xRGBA) celestial tint tables
// scraped from the current zone's sun/moon generator, applied 2x-modulate
// (research/xim Particle.kt getColor). None where the zone ships no such generator (then
// sun_moon falls back to the WEEKDAY_MOON_TINT constants).
#[derive(Resource, Default, Clone)]
pub struct CelestialColorTables {
    pub day_of_week: Option<[[f32; 4]; 8]>,
    pub moon_phase: Option<[[f32; 4]; 12]>,
}

impl Material for MoonMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://kuluu_render/moon.wesl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

// Load the retail moon sprite sheet (12 phase frames + texture) from the current
// zone's DAT, matching how retail/XIM draw the moon. The phase is baked per frame,
// so sun_moon_system just selects the frame; the procedural disc remains the
// fallback for zones without a sky (indoors).
fn load_moon_sprite_sheet(
    scene_state: Res<crate::snapshot::SceneState>,
    dat_root: Option<Res<MoonDatRoot>>,
    celestial: Option<Res<crate::sun_moon::CelestialMaterials>>,
    mut moon_materials: ResMut<Assets<MoonMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut frames_res: ResMut<MoonSpriteFrames>,
    mut color_tables: ResMut<CelestialColorTables>,
    mut loaded_zone: Local<Option<Option<u32>>>,
) {
    let current = crate::snapshot::effective_zone_file_id(&scene_state.snapshot);
    if *loaded_zone == Some(current) {
        return;
    }
    let Some(dat_root) = dat_root.and_then(|r| r.0.clone()) else {
        return;
    };
    let Some(celestial) = celestial else {
        return;
    };
    *loaded_zone = Some(current);

    let dat_bytes = current
        .and_then(|file_id| dat_root.resolve(file_id).ok())
        .and_then(|loc| std::fs::read(loc.path_under(&dat_root)).ok());

    match dat_bytes
        .as_deref()
        .and_then(ffxi_dat::sprite_sheet::extract_celestial_color_tables)
    {
        Some(t) => {
            *color_tables = CelestialColorTables {
                day_of_week: t.day_of_week,
                moon_phase: t.moon_phase,
            };
        }
        None => *color_tables = CelestialColorTables::default(),
    }

    let sheet = dat_bytes
        .as_deref()
        .and_then(ffxi_dat::sprite_sheet::extract_moon_sprite_sheet);

    let Some(sheet) = sheet else {
        frames_res.0 = None;
        if let Some(mut mat) = moon_materials.get_mut(&celestial.moon) {
            mat.surface = None;
        }
        return;
    };

    let mut rgba = sheet.texture.rgba;
    // The moon is a sky shell like the cloud canopy: drawn magnified, so `moonshap`'s
    // stored 4-bit alpha dither (24.5% of its texels are the nibble 7/8 pair) resolves into
    // visible dots rather than averaging away. Before the remap, which doubles whatever it
    // is handed — see `resolve_dxt3_alpha_dither`.
    ffxi_dat::texture::resolve_dxt3_alpha_dither(
        &mut rgba,
        sheet.texture.width,
        sheet.texture.height,
    );
    ffxi_dat::texture::apply_ffxi_alpha_remap(&mut rgba);
    let mut image = Image::new(
        Extent3d {
            width: sheet.texture.width,
            height: sheet.texture.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::linear();
    let handle = images.add(image);

    let mut frames = [Vec4::new(0.0, 0.0, 1.0, 1.0); ffxi_dat::sprite_sheet::MOON_PHASE_FRAMES];
    for (slot, f) in frames.iter_mut().zip(sheet.frames.iter()) {
        *slot = Vec4::new(f.u0, f.v0, f.u1, f.v1);
    }
    frames_res.0 = Some(frames);

    if let Some(mut mat) = moon_materials.get_mut(&celestial.moon) {
        mat.surface = Some(handle);
    }
    info!(
        "moon: loaded retail sprite sheet ({}×{}, {} frames)",
        sheet.texture.width,
        sheet.texture.height,
        sheet.frames.len()
    );
}

pub struct MoonMaterialPlugin;

impl Plugin for MoonMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "moon.wesl");
        app.init_resource::<MoonDatRoot>()
            .init_resource::<MoonSpriteFrames>()
            .init_resource::<CelestialColorTables>()
            .add_plugins(MaterialPlugin::<MoonMaterial>::default())
            .add_systems(Update, load_moon_sprite_sheet);
    }
}
