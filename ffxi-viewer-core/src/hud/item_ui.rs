//! Shared building blocks for the Equipment and Items HUD windows:
//! transparent icon placeholder and item-detail composition. Window chrome
//! and colors live in `hud::style`.

use bevy::prelude::*;

use crate::hud::item_dat_root::{ItemDatRoot, ItemIconCache};
use crate::hud::{item_detail, item_meta};

pub use crate::hud::style::{
    cursor_prefix, text_font, theme, window_frame as framed_box, WINDOW_Z,
};

pub fn transparent_placeholder(images: &mut Assets<Image>) -> Handle<Image> {
    use bevy::asset::RenderAssetUsages;
    use bevy::image::ImageSampler;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    let mut image = Image::new(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![0u8, 0, 0, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::nearest();
    images.add(image)
}

/// The name + detail rows for a focused item, shared by both windows so the
/// item card reads identically wherever it appears. `None` yields the retail
/// "Select an item." prompt.
pub fn focus_detail(
    item_no: Option<u16>,
    focused_slot: Option<(u8, u8)>,
    snap: &ffxi_viewer_wire::SceneSnapshot,
    dat_root: &ItemDatRoot,
    icon_cache: &mut ItemIconCache,
) -> (String, Vec<String>) {
    let Some(item_no) = item_no else {
        return ("Select an item.".to_string(), Vec::new());
    };
    let dat = icon_cache
        .table(dat_root)
        .and_then(|table| item_detail::lookup_static(&table, item_no));
    let detail = item_meta::compose_item_detail(item_no, focused_slot, snap, dat.clone());
    let name = dat
        .as_ref()
        .map(|s| s.name.clone())
        .filter(|n| !n.is_empty())
        .or_else(|| ffxi_proto::item_names::lookup(item_no).map(|s| s.to_string()))
        .unwrap_or_else(|| format!("Item #{item_no}"));
    (name, item_detail::detail_rows(&detail))
}
