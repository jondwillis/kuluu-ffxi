use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use bevy::asset::AssetId;
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::texture::GpuImage;
use bevy::render::{Render, RenderApp, RenderSystems};

/// Asset IDs carry no ownership; this process-wide mirror follows the render
/// world's residency.
#[derive(Resource, Default, Clone, ExtractResource)]
pub struct GpuAssetResidency(Arc<Mutex<HashSet<AssetId<Image>>>>);

impl GpuAssetResidency {
    pub fn images_ready(&self, images: &[Handle<Image>]) -> bool {
        let ready = self.0.lock().unwrap();
        images.iter().all(|image| ready.contains(&image.id()))
    }
}

fn publish_residency(images: Res<RenderAssets<GpuImage>>, ready: Res<GpuAssetResidency>) {
    let mut ready = ready.0.lock().unwrap();
    ready.clear();
    ready.extend(images.iter().map(|(id, _)| id));
}

pub struct GpuAssetResidencyPlugin;

impl Plugin for GpuAssetResidencyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GpuAssetResidency>()
            .add_plugins(ExtractResourcePlugin::<GpuAssetResidency>::default());
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(
                Render,
                publish_residency.after(RenderSystems::PrepareAssets),
            );
        }
    }
}

pub fn image_bytes(image: &Image) -> usize {
    let desc = &image.texture_descriptor;
    let (block_width, block_height) = desc.format.block_dimensions();
    let block_bytes = desc.format.block_copy_size(None).unwrap_or(0) as usize;
    (0..desc.mip_level_count)
        .map(|level| {
            let size = desc.size.mip_level_size(level, desc.dimension);
            size.width.div_ceil(block_width) as usize
                * size.height.div_ceil(block_height) as usize
                * size.depth_or_array_layers as usize
                * block_bytes
                * desc.sample_count as usize
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn descriptor_bytes_survive_cpu_data_extraction() {
        let mut image = crate::zone_texture::image_with_mips(
            vec![255; 8 * 8 * 4],
            8,
            8,
            crate::zone_texture::TextureQuality {
                mipmaps: true,
                anisotropy: 1,
            },
            false,
        );
        let bytes = image.data.take().unwrap().len();
        assert_eq!(image_bytes(&image), bytes);
    }

    #[test]
    fn a_partial_look_is_not_ready() {
        let mut images = Assets::<Image>::default();
        let handles = vec![images.add(Image::default()), images.add(Image::default())];
        let ready = GpuAssetResidency::default();
        ready.0.lock().unwrap().insert(handles[0].id());
        assert!(!ready.images_ready(&handles));
        ready.0.lock().unwrap().insert(handles[1].id());
        assert!(ready.images_ready(&handles));
        ready.0.lock().unwrap().remove(&handles[0].id());
        assert!(!ready.images_ready(&handles));
    }
}
