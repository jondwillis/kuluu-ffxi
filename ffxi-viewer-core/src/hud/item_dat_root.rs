use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ffxi_dat::DatRoot;

#[derive(Resource, Default, Clone)]
pub struct ItemDatRoot(pub Option<Arc<DatRoot>>);

#[derive(Resource, Default)]
pub struct ItemIconCache {
    dat: Option<Arc<Vec<u8>>>,

    dat_unavailable: bool,

    icons: HashMap<u16, Option<Handle<Image>>>,
}

impl ItemIconCache {
    pub fn ensure(
        &mut self,
        item_no: u16,
        dat_root: &ItemDatRoot,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        if let Some(slot) = self.icons.get(&item_no) {
            return slot.clone();
        }
        let handle = self
            .dat_bytes(dat_root)
            .and_then(|bytes| ffxi_dat::item_dat::icon_at(&bytes, item_no))
            .map(|img| upload_icon(img, images));
        self.icons.insert(item_no, handle.clone());
        handle
    }

    fn dat_bytes(&mut self, dat_root: &ItemDatRoot) -> Option<Arc<Vec<u8>>> {
        if let Some(bytes) = &self.dat {
            return Some(bytes.clone());
        }
        if self.dat_unavailable {
            return None;
        }
        let root = match &dat_root.0 {
            Some(r) => r,

            None => return None,
        };

        let loaded = ffxi_dat::item_dat::ITEM_DAT_FILE_ID
            .iter()
            .find_map(|&file_id| {
                let path = root.resolve(file_id).ok()?.path_under(root.root());
                let bytes = std::fs::read(path).ok()?;
                (bytes.len() % ffxi_dat::item_dat::ITEM_BLOCK_STRIDE == 0 && !bytes.is_empty())
                    .then_some(bytes)
            });
        match loaded {
            Some(bytes) => {
                let arc = Arc::new(bytes);
                self.dat = Some(arc.clone());
                Some(arc)
            }
            None => {
                warn!(
                    "item icons: no item DAT in {:?} resolved/readable; label-only fallback",
                    ffxi_dat::item_dat::ITEM_DAT_FILE_ID
                );
                self.dat_unavailable = true;
                None
            }
        }
    }

    pub fn dat_unavailable(&self) -> bool {
        self.dat_unavailable
    }

    pub fn dat_bytes_for_static(&mut self, dat_root: &ItemDatRoot) -> Option<Arc<Vec<u8>>> {
        self.dat_bytes(dat_root)
    }
}

fn upload_icon(
    img: ffxi_dat::map_image::GraphicImage,
    images: &mut Assets<Image>,
) -> Handle<Image> {
    let mut image = Image::new(
        Extent3d {
            width: img.width,
            height: img.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        img.rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::linear();
    images.add(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_without_root_does_not_latch() {
        let mut cache = ItemIconCache::default();
        let root = ItemDatRoot(None);
        assert!(cache.dat_bytes(&root).is_none());
        assert!(!cache.dat_unavailable, "must retry once root is provided");
    }
}
