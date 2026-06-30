use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ffxi_dat::item_dat::ItemTable;
use ffxi_dat::DatRoot;

#[derive(Resource, Default, Clone)]
pub struct ItemDatRoot(pub Option<Arc<DatRoot>>);

#[derive(Resource, Default)]
pub struct ItemIconCache {
    table: Option<Arc<ItemTable>>,

    unavailable: bool,

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
            .table(dat_root)
            .and_then(|t| t.icon(item_no))
            .map(|img| upload_icon(img, images));
        self.icons.insert(item_no, handle.clone());
        handle
    }

    /// The resolved retail item database, built lazily once a DAT root is
    /// available. Returns `None` until then (and latches off only when the root
    /// exists but holds no readable item DATs), so the first frame with a root
    /// retries.
    pub fn table(&mut self, dat_root: &ItemDatRoot) -> Option<Arc<ItemTable>> {
        if let Some(table) = &self.table {
            return Some(table.clone());
        }
        if self.unavailable {
            return None;
        }
        let root = dat_root.0.as_ref()?;
        let table = ItemTable::open(root.root());
        if table.is_empty() {
            warn!(
                "item DAT: no item tables under {:?}; label-only fallback",
                root.root()
            );
            self.unavailable = true;
            return None;
        }
        let arc = Arc::new(table);
        self.table = Some(arc.clone());
        Some(arc)
    }

    pub fn unavailable(&self) -> bool {
        self.unavailable
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
        assert!(cache.table(&root).is_none());
        assert!(!cache.unavailable, "must retry once root is provided");
    }
}
