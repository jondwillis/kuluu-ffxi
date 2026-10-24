//! Region Info's four map pictures (bead kuluu-o9qt7.36).
//!
//! The `region` list's items 1..4 install a mode; the drawer routine loads one
//! DAT per mode and draws it — FFXiMain.dll retail-2026-09 `.text 0x1E7980`:
//! bounds-check mode < 4, store it at `[obj+0x160]`, read the file id as a u16
//! from the mode table (`.rdata 0x37F454`: 0x14C0, 0xD0A8, 0xD0C3, 0xD141 —
//! `mov cx, word [ebx*2 + 0x1037F454]` at `.text 0x1E79E4`) and load it. The
//! file ids resolve through VTABLE/FTABLE to installed DATs (probed: a 4 MiB
//! world picture, three 256 KiB nation/region/zone pictures); the loader takes
//! the largest graphic block each file carries.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::input_mode::{InputMode, MenuKind};

/// Mode table of the region drawer — FFXiMain.dll retail-2026-09 `.rdata 0x37F454`,
/// indexed by mode (world / nation / region / zone).
pub const REGION_MAP_FILE_IDS: [u32; 4] = [0x14C0, 0xD0A8, 0xD0C3, 0xD141];

/// The drawer's bounds check (FFXiMain.dll retail-2026-09 `.text 0x1E7980`:
/// `test ebx,ebx`/`jl` and `cmp ebx,4`/`jge` reject everything outside 0..=3
/// before anything loads).
pub fn region_mode_valid(mode: u8) -> bool {
    (mode as usize) < REGION_MAP_FILE_IDS.len()
}

/// The mode whose picture should show now: the top stack level when it is the
/// `region` list itself (retail draws with its own list up and keeps switching).
pub fn region_map_visible_mode(mode: &InputMode) -> Option<u8> {
    let InputMode::Menu(stack) = mode else {
        return None;
    };
    match stack.current().map(|l| l.kind) {
        Some(MenuKind::RegionInfo { mode }) if region_mode_valid(mode) => Some(mode),
        _ => None,
    }
}

/// Decode the picture one mode names. The file id goes through `DatRoot::resolve`
/// (the same VTABLE/FTABLE walk FFXiMain.dll does — never a fixed ROM path), and
/// the largest graphic block wins, as `minimap::retail::load_zone_map_image` does.
pub fn load_region_map_graphic(
    dat_root: &ffxi_dat::DatRoot,
    mode: u8,
) -> Option<ffxi_dat::map_image::GraphicImage> {
    let file_id = *REGION_MAP_FILE_IDS.get(mode as usize)?;
    let path = dat_root.resolve(file_id).ok()?.path_under(dat_root);
    let bytes = std::fs::read(path).ok()?;
    ffxi_dat::map_image::scan_graphics(&bytes).max_by_key(|g| g.width * g.height)
}

/// Panel bounds are kuluu geometry, capped by the companion-window width law: every
/// left-column window record is `x=16 w=366` and selectors sit at `x=384`, so a picture
/// wider than 366 would cover its own selector (vault main_menu.md §59 addendum 17).
/// The drawer's own per-mode word fields (`[obj+0x164..0x16A]`) feed record walks, not
/// pixels (§59 addendum 28).
pub const REGION_MAP_PANEL_MIN_PX: f32 = 256.0;

pub const REGION_MAP_PANEL_MAX_PX: f32 = 366.0;

/// The largest square that fits both the viewport height and the left column.
pub fn region_map_panel_size(viewport_height_px: f32) -> f32 {
    (viewport_height_px - 32.0).clamp(REGION_MAP_PANEL_MIN_PX, REGION_MAP_PANEL_MAX_PX)
}

/// The picture panel; the update system swaps textures and toggles display.
#[derive(Component)]
pub struct RegionMapRoot;

/// The loaded texture for the mode currently displayed (keyed so a re-open is free).
#[derive(Resource, Default)]
pub struct ViewedRegionMap {
    key: Option<(std::path::PathBuf, u8)>,
    handle: Option<Handle<Image>>,
}

impl ViewedRegionMap {
    fn matches(&self, root: &std::path::Path, mode: u8) -> bool {
        self.key.as_ref() == Some(&(root.to_path_buf(), mode)) && self.handle.is_some()
    }
}

pub(crate) fn spawn_region_map(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = crate::hud::item_ui::transparent_placeholder(&mut images);
    commands.spawn((
        crate::components::InGameEntity,
        RegionMapRoot,
        ImageNode {
            image: placeholder,
            // Stretch, as the map screen does: Auto aspect-fits inside the node and
            // detaches the picture from any overlay placed against it.
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(16.0),
            top: Val::Px(16.0),
            ..default()
        },
        Visibility::Hidden,
    ));
}

pub(crate) fn update_region_map(
    input_mode: Res<InputMode>,
    dat_root: Res<crate::minimap::retail::MinimapDatRoot>,
    windows: Single<&Window>,
    mut viewed: ResMut<ViewedRegionMap>,
    mut images: ResMut<Assets<Image>>,
    mut root_q: Query<(&mut ImageNode, &mut Node, &mut Visibility), With<RegionMapRoot>>,
) {
    let Ok((mut image_node, mut node, mut visibility)) = root_q.single_mut() else {
        return;
    };
    let Some(mode) = region_map_visible_mode(&input_mode) else {
        *visibility = Visibility::Hidden;
        return;
    };
    *visibility = Visibility::Visible;
    let side = region_map_panel_size(windows.height());
    node.width = Val::Px(side);
    node.height = Val::Px(side);

    let Some(root) = dat_root.0.as_ref() else {
        return;
    };
    if viewed.matches(root.root(), mode) {
        let handle = viewed.handle.clone();
        if let Some(handle) = handle.filter(|h| *h != image_node.image) {
            image_node.image = handle;
        }
        return;
    }
    let Some(graphic) = load_region_map_graphic(root, mode) else {
        viewed.key = None;
        viewed.handle = None;
        return;
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
    image.sampler = bevy::image::ImageSampler::linear();
    let handle = images.add(image);
    viewed.key = Some((root.root().to_path_buf(), mode));
    image_node.image = handle.clone();
    viewed.handle = Some(handle);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_mode_bounds_match_the_drawer_check() {
        for mode in 0..4u8 {
            assert!(region_mode_valid(mode), "mode {mode} loads");
        }
        assert!(!region_mode_valid(4));
        assert!(!region_mode_valid(u8::MAX));
    }

    #[test]
    fn visibility_tracks_the_region_window_being_on_top() {
        use crate::input_mode::MenuStack;
        assert_eq!(region_map_visible_mode(&InputMode::World), None);
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RegionInfo { mode: 2 });
        assert_eq!(region_map_visible_mode(&InputMode::Menu(stack)), Some(2));

        let mut buried = MenuStack::root();
        buried.push(MenuKind::RegionInfo { mode: 2 });
        buried.push(MenuKind::Map);
        assert_eq!(
            region_map_visible_mode(&InputMode::Menu(buried)),
            None,
            "another window on top hides the picture"
        );
    }

    #[test]
    fn panel_size_fits_the_viewport_square() {
        // Never reaches the selector seat at x=384.
        assert_eq!(region_map_panel_size(544.0), REGION_MAP_PANEL_MAX_PX);
        assert_eq!(region_map_panel_size(700.0), REGION_MAP_PANEL_MAX_PX);
        // Below the cap the viewport height rules: a 390 px window gives 358.
        assert_eq!(region_map_panel_size(390.0), 358.0);
        assert_eq!(region_map_panel_size(248.0), REGION_MAP_PANEL_MIN_PX);
    }

    /// The four mode DATs on an installed client each carry one big picture: the
    /// world map at 2048², and nation/region/zone pictures at least 512² (sizes as
    /// this install ships them).
    #[test]
    fn installed_mode_files_decode_to_one_map_picture_each() {
        const WORLD_FLOOR: u32 = 2048;
        const SMALL_FLOOR: u32 = 512;
        let Some(root_path) = crate::scheduler_runtime::install_root_from_env() else {
            eprintln!("no install: pin skipped");
            return;
        };
        let root = ffxi_dat::DatRoot::open(&root_path).expect("install opens");
        for mode in 0..4u8 {
            let graphic = load_region_map_graphic(&root, mode)
                .unwrap_or_else(|| panic!("mode {mode} must decode on an installed client"));
            let floor = if mode == 0 { WORLD_FLOOR } else { SMALL_FLOOR };
            assert!(
                graphic.width >= floor
                    && graphic.height >= floor
                    && graphic.rgba.len() == (graphic.width * graphic.height * 4) as usize,
                "mode {mode} picture must be at least {floor}² RGBA, got {}x{}",
                graphic.width,
                graphic.height
            );
        }
    }
}
