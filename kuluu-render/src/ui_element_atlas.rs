//! Cache of retail menu UI-element sprites, keyed by (group-name, index).
//! Backed by ffxi_dat::ui_element; mirrors the item/status icon caches
//! (hud/item_dat_root.rs, hud/status_ribbon.rs).

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ffxi_dat::texture::TexFormat;
use ffxi_dat::ui_element::{crop_sprite, find_texture, find_ui_element_group, ui_sprite, UiSprite};
use ffxi_dat::DatRoot;

// The four "static resource" menu UI DATs. XIM hardcodes their ROM paths
// (research/xim/src/jsMain/kotlin/xim/poc/UiResourceManager.kt UiResourceManager uiDats — ROM/0/13, ROM/119/51,
// ROM/280/15, ROM/324/95); these are those paths reverse-mapped through
// VTABLE/FTABLE to file ids, the version-stable handle, so an install whose
// patch level shuffles the physical ROM layout still resolves. The
// day-of-week orbs and weather element icons live in id 39542 (ROM/119/51).
pub const UI_DAT_FILE_IDS: [u32; 4] = [13, 39542, 39551, 39560];

pub fn read_ui_dats(root: &DatRoot) -> Vec<(u32, Vec<u8>)> {
    UI_DAT_FILE_IDS
        .into_iter()
        .filter_map(|id| {
            let loc = root.resolve(id).ok()?;
            let bytes = std::fs::read(loc.path_under(root)).ok()?;
            Some((id, bytes))
        })
        .collect()
}

const FRAMES_JP: &str = "menu    frames  ";
const FRAMES_US: &str = "menu    framesus";

// research/XIClient/src/XIClient/source/UI/UIManager.cpp UIManager::InitDraw.
const UI_ALPHA_MODULATE_2X: f32 = 2.0;

#[derive(Resource, Default, Clone)]
pub struct UiElementDatRoot(pub Option<Arc<DatRoot>>);

#[derive(Resource, Default)]
pub struct UiElementAtlas {
    dats: Vec<Arc<Vec<u8>>>,
    loaded: bool,
    unavailable: bool,
    sprites: HashMap<(String, usize), Option<Handle<Image>>>,
    elements: HashMap<(String, usize), Option<Vec<UiElementQuad>>>,
}

#[derive(Clone)]
pub struct UiElementQuad {
    pub image: Handle<Image>,
    pub rect: Rect,
    pub color: Color,
}

impl UiElementAtlas {
    pub fn ensure_element(
        &mut self,
        group: &str,
        index: usize,
        dat_root: &UiElementDatRoot,
        images: &mut Assets<Image>,
    ) -> Option<Vec<UiElementQuad>> {
        let key = (group.to_string(), index);
        if let Some(slot) = self.elements.get(&key) {
            return slot.clone();
        }
        let quads = self.ensure_dats(dat_root).iter().find_map(|bytes| {
            let resource = find_ui_element_group(bytes, group)?;
            resource
                .elements
                .get(index)?
                .components
                .iter()
                .map(|component| {
                    let texture = find_texture(bytes, &component.texture_ref)?;
                    let mut sprite = crop_sprite(
                        &texture,
                        component.uv_offset_x,
                        component.uv_offset_y,
                        component.uv_width,
                        component.uv_height,
                        component.flip_mode,
                    )?;
                    let mut color = crate::nameplate_color::quad_color(component.colors[0]);
                    if texture.format_tag == TexFormat::Dxt3 {
                        modulate_dxt3_ui_alpha(&mut sprite, color.alpha());
                        color.set_alpha(1.0);
                    }
                    let points = component
                        .positions
                        .map(|(x, y)| Vec2::new(f32::from(x), f32::from(y)));
                    Some(UiElementQuad {
                        image: upload_sprite(sprite, images),
                        rect: Rect::from_corners(
                            points
                                .into_iter()
                                .fold(Vec2::splat(f32::INFINITY), Vec2::min),
                            points
                                .into_iter()
                                .fold(Vec2::splat(f32::NEG_INFINITY), Vec2::max),
                        ),
                        color,
                    })
                })
                .collect::<Option<Vec<_>>>()
        });
        self.elements.insert(key, quads.clone());
        quads
    }

    pub fn ensure(
        &mut self,
        group: &str,
        index: usize,
        dat_root: &UiElementDatRoot,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        let key = (group.to_string(), index);
        if let Some(slot) = self.sprites.get(&key) {
            return slot.clone();
        }
        let handle = self
            .ensure_dats(dat_root)
            .iter()
            .find_map(|bytes| resolve_sprite(bytes, group, index))
            .map(|sprite| upload_sprite(sprite, images));
        self.sprites.insert(key, handle.clone());
        handle
    }

    fn ensure_dats(&mut self, dat_root: &UiElementDatRoot) -> &[Arc<Vec<u8>>] {
        if self.loaded || self.unavailable {
            return &self.dats;
        }
        let Some(root) = dat_root.0.as_ref() else {
            self.unavailable = true;
            return &self.dats;
        };
        self.dats = read_ui_dats(root)
            .into_iter()
            .map(|(_, bytes)| Arc::new(bytes))
            .collect();
        self.loaded = true;
        self.unavailable = self.dats.is_empty();
        &self.dats
    }
}

fn modulate_dxt3_ui_alpha(sprite: &mut UiSprite, vertex_alpha: f32) {
    // DXT3 decoding preserves raw alpha; palette decoding already doubles it.
    // Clamp after modulation so partially transparent vertices retain bright texels.
    for pixel in sprite.rgba.chunks_exact_mut(4) {
        pixel[3] = (f32::from(pixel[3]) * vertex_alpha * UI_ALPHA_MODULATE_2X)
            .round()
            .min(f32::from(u8::MAX)) as u8;
    }
}

// HorizonXI/US ships "menu    framesus" where the JP client uses
// "menu    frames  "; XIM aliases the two (UiResourceManager.kt register).
fn resolve_sprite(bytes: &[u8], group: &str, index: usize) -> Option<UiSprite> {
    ui_sprite(bytes, group, index).or_else(|| {
        if group == FRAMES_JP {
            ui_sprite(bytes, FRAMES_US, index)
        } else {
            None
        }
    })
}

fn upload_sprite(sprite: UiSprite, images: &mut Assets<Image>) -> Handle<Image> {
    let mut image = Image::new(
        Extent3d {
            width: sprite.width,
            height: sprite.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        sprite.rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::linear();
    images.add(image)
}

pub struct UiElementAtlasPlugin;

impl Plugin for UiElementAtlasPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiElementDatRoot>()
            .init_resource::<UiElementAtlas>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn dxt3_ui_alpha_modulates_before_saturating() {
        let mut sprite = UiSprite {
            width: 4,
            height: 1,
            rgba: vec![
                20, 40, 60, 0, 20, 40, 60, 85, 20, 40, 60, 136, 20, 40, 60, 255,
            ],
        };
        let vertex_alpha = crate::nameplate_color::quad_color([127; 4]).alpha();
        modulate_dxt3_ui_alpha(&mut sprite, vertex_alpha);
        assert_eq!(
            sprite.rgba,
            [20, 40, 60, 0, 20, 40, 60, 128, 20, 40, 60, 205, 20, 40, 60, 255],
            "transparent texels stay clear, RGB is untouched, and alpha saturates after modulation"
        );
    }

    fn test_dat_root() -> Option<UiElementDatRoot> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(ffxi_dat::archive::DEFAULT_INSTALL_DIR);
        if !root.join("VTABLE.DAT").exists() {
            return None;
        }
        let root = DatRoot::open(root).ok()?;
        Some(UiElementDatRoot(Some(Arc::new(root))))
    }

    // Gated on a retail install (self-skips). Exercises the whole viewer-side
    // path against real data: load the UI DATs, resolve the frames->framesus
    // alias, decode + crop, and upload a 14x14 day-orb image into Assets.
    #[test]
    fn real_dat_day_orb_uploads_14x14() {
        let Some(dat_root) = test_dat_root() else {
            return;
        };
        let mut images = Assets::<Image>::default();
        let mut atlas = UiElementAtlas::default();

        let handle = atlas
            .ensure(FRAMES_JP, 106, &dat_root, &mut images)
            .expect("Firesday orb resolves via the frames->framesus alias");
        let image = images.get(&handle).expect("uploaded image present");
        assert_eq!(image.width(), 14);
        assert_eq!(image.height(), 14);

        // Second lookup is served from the cache (same handle).
        let again = atlas.ensure(FRAMES_JP, 106, &dat_root, &mut images);
        assert_eq!(again.as_ref(), Some(&handle));
    }

    // Gated on a retail install (self-skips). The weather-icon widget resolves
    // "font    usgaiji " 0-7 (the eight element icons, ROM/119/51.DAT); pin
    // that every index uploads as a 16x16 sprite.
    #[test]
    fn real_dat_usgaiji_weather_icons_upload_16x16() {
        let Some(dat_root) = test_dat_root() else {
            return;
        };
        let mut images = Assets::<Image>::default();
        let mut atlas = UiElementAtlas::default();

        for index in 0..8 {
            let handle = atlas
                .ensure("font    usgaiji ", index, &dat_root, &mut images)
                .unwrap_or_else(|| panic!("usgaiji element {index} resolves"));
            let image = images.get(&handle).expect("uploaded image present");
            assert_eq!((image.width(), image.height()), (16, 16), "index {index}");
        }
    }
}
