//! Shared building blocks for the item-bearing HUD windows (Equipment and
//! Items) so the two screens stay visually and structurally DRY: one theme,
//! one framed-box style, one transparent icon placeholder, and one item-detail
//! composition.

use bevy::prelude::*;

use crate::hud::item_dat_root::{ItemDatRoot, ItemIconCache};
use crate::hud::{item_detail, item_meta};

/// Menus draw above the chat stack (which owns the lower-left quadrant), matching
/// the other pop-up windows (target-action / quick-action at 20, trade at 25).
pub const WINDOW_Z: i32 = 20;

/// Retail FFXI's default "blue" window theme: a translucent navy frame with a
/// light steel-blue edge, pale-blue title text, near-white body text, and a
/// golden cursor highlight on the focused row/slot. These are deliberate tunings
/// (retail ships no palette file to scrape), named here per no-magic-numbers.
pub mod theme {
    use bevy::prelude::Color;

    pub const FRAME_BG: Color = Color::srgba(0.05, 0.07, 0.16, 0.88);
    pub const FRAME_EDGE: Color = Color::srgb(0.60, 0.69, 0.85);
    pub const TITLE: Color = Color::srgb(0.80, 0.89, 1.0);
    pub const TEXT: Color = Color::srgb(0.91, 0.92, 0.95);
    pub const MUTED: Color = Color::srgb(0.58, 0.63, 0.74);
    pub const CURSOR: Color = Color::srgb(1.0, 0.84, 0.36);
    pub const CURSOR_BG: Color = Color::srgba(0.20, 0.28, 0.45, 0.65);
    pub const CELL_BG: Color = Color::srgba(0.10, 0.13, 0.24, 0.85);
    pub const CELL_EDGE: Color = Color::srgb(0.32, 0.38, 0.52);
}

pub fn text_font(size: f32) -> TextFont {
    TextFont {
        font_size: size,
        ..default()
    }
}

/// A framed panel: translucent navy fill, steel-blue 1px border, column layout.
pub fn framed_box() -> (Node, BackgroundColor, BorderColor) {
    (
        Node {
            padding: UiRect::axes(Val::Px(10.0), Val::Px(8.0)),
            border: UiRect::all(Val::Px(1.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(2.0),
            ..default()
        },
        BackgroundColor(theme::FRAME_BG),
        BorderColor::all(theme::FRAME_EDGE),
    )
}

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
    let detail = item_meta::compose_item_detail(item_no, snap, dat.clone());
    let name = dat
        .as_ref()
        .map(|s| s.name.clone())
        .filter(|n| !n.is_empty())
        .or_else(|| ffxi_proto::item_names::lookup(item_no).map(|s| s.to_string()))
        .unwrap_or_else(|| format!("Item #{item_no}"));
    (name, item_detail::detail_rows(&detail))
}
