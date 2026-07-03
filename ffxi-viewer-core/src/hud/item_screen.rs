use bevy::prelude::*;

use crate::hud::item_dat_root::{ItemDatRoot, ItemIconCache};
use crate::hud::menu::{DynamicMenu, DynamicMenuAction, MenuRowActivated};
use crate::hud::palette;
use crate::input_mode::{InputMode, MenuKind};
use crate::snapshot::SceneState;

pub const ITEM_LIST_ROWS: usize = 13;

const ROW_ICON_PX: f32 = 18.0;

const MAIN_BAG_CAPACITY: u32 = 80;

#[derive(Component)]
pub struct ItemScreenPanel;

#[derive(Component)]
pub struct ItemScreenHeader;

#[derive(Component)]
pub struct ItemScreenRow {
    pub slot: usize,
}

#[derive(Component)]
pub struct ItemScreenRowIcon {
    pub slot: usize,
}

#[derive(Component)]
pub struct ItemScreenRowText {
    pub slot: usize,
}

pub fn items_open(mode: &InputMode) -> bool {
    matches!(mode, InputMode::Menu(stack)
        if stack.current().is_some_and(|l| matches!(l.kind, MenuKind::Items)))
}

fn items_cursor(mode: &InputMode) -> Option<usize> {
    match mode {
        InputMode::Menu(stack) => stack
            .current()
            .filter(|l| matches!(l.kind, MenuKind::Items))
            .map(|l| l.cursor),
        _ => None,
    }
}

/// First visible list index, keeping `cursor` inside the `ITEM_LIST_ROWS`
/// viewport. Mirrors `menu::resolve_viewport` so keyboard and mouse agree.
pub fn viewport_start(cursor: usize, total: usize) -> usize {
    if total <= ITEM_LIST_ROWS {
        return 0;
    }
    let half = ITEM_LIST_ROWS / 2;
    let max_start = total - ITEM_LIST_ROWS;
    cursor.saturating_sub(half).min(max_start)
}

fn row_item_no(rows: &[crate::hud::menu::DynamicMenuRow], idx: usize) -> Option<u16> {
    match rows.get(idx)?.action {
        DynamicMenuAction::UseItem { item_no, .. } => Some(item_no),
        _ => None,
    }
}

pub fn spawn_item_screen(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = transparent_placeholder(&mut images);

    commands
        .spawn((
            crate::components::InGameEntity,
            ItemScreenPanel,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(48.0),
                left: Val::Px(8.0),
                width: Val::Px(240.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
        ))
        .with_children(|p| {
            p.spawn((
                ItemScreenHeader,
                Text::new("Items"),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(palette::ACCENT),
            ));

            for slot in 0..ITEM_LIST_ROWS {
                p.spawn((
                    ItemScreenRow { slot },
                    Button,
                    Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(5.0),
                        display: Display::None,
                        ..default()
                    },
                ))
                .with_children(|row| {
                    row.spawn((
                        ItemScreenRowIcon { slot },
                        Node {
                            width: Val::Px(ROW_ICON_PX),
                            height: Val::Px(ROW_ICON_PX),
                            display: Display::None,
                            ..default()
                        },
                        ImageNode::new(placeholder.clone()),
                    ));
                    row.spawn((
                        ItemScreenRowText { slot },
                        Text::new(""),
                        TextFont {
                            font_size: 13.0,
                            ..default()
                        },
                        TextColor(palette::TEXT),
                    ));
                });
            }
        });
}

#[allow(clippy::too_many_arguments)]
pub fn update_item_screen(
    mode: Res<InputMode>,
    state: Res<SceneState>,
    dynamic: Res<DynamicMenu>,
    dat_root: Res<ItemDatRoot>,
    mut icon_cache: ResMut<ItemIconCache>,
    mut images: ResMut<Assets<Image>>,
    mut panel_q: Query<&mut Node, With<ItemScreenPanel>>,
    mut header_q: Query<&mut Text, (With<ItemScreenHeader>, Without<ItemScreenRowText>)>,
    mut row_q: Query<
        (&ItemScreenRow, &mut Node),
        (Without<ItemScreenPanel>, Without<ItemScreenRowIcon>),
    >,
    mut text_q: Query<(&ItemScreenRowText, &mut Text, &mut TextColor), Without<ItemScreenHeader>>,
    mut icon_q: Query<
        (&ItemScreenRowIcon, &mut Node, &mut ImageNode),
        (Without<ItemScreenRow>, Without<ItemScreenPanel>),
    >,
) {
    let open = items_open(&mode);
    if let Ok(mut node) = panel_q.single_mut() {
        let want = if open { Display::Flex } else { Display::None };
        if node.display != want {
            node.display = want;
        }
    }
    if !open {
        return;
    }

    let cursor = items_cursor(&mode).unwrap_or(0);
    let rows = &dynamic.rows;
    let total = rows.len();
    let start = viewport_start(cursor, total);

    if let Ok(mut text) = header_q.single_mut() {
        let used = state.snapshot.inventory_main.len() as u32;
        let want = format!("Items  {used}/{MAIN_BAG_CAPACITY}");
        if **text != want {
            **text = want;
        }
    }

    for (row, mut node) in row_q.iter_mut() {
        let list_idx = start + row.slot;
        let visible = row.slot < ITEM_LIST_ROWS && list_idx < total;
        let want = if visible {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
    }

    let empty = total == 0;
    for (row, mut text, mut color) in text_q.iter_mut() {
        let list_idx = start + row.slot;
        if empty && row.slot == 0 {
            let want = "(inventory empty)";
            if **text != *want {
                **text = want.to_string();
            }
            if color.0 != palette::MUTED {
                color.0 = palette::MUTED;
            }
            continue;
        }
        let Some(entry) = rows.get(list_idx) else {
            continue;
        };
        let is_cursor = list_idx == cursor;
        let want = if is_cursor {
            format!("> {}", entry.label)
        } else {
            format!("  {}", entry.label)
        };
        if **text != want {
            **text = want;
        }
        let want_color = if is_cursor {
            palette::ACCENT
        } else {
            palette::TEXT
        };
        if color.0 != want_color {
            color.0 = want_color;
        }
    }

    for (icon, mut node, mut image) in icon_q.iter_mut() {
        let list_idx = start + icon.slot;
        let handle = (!empty)
            .then(|| row_item_no(rows, list_idx))
            .flatten()
            .and_then(|item_no| icon_cache.ensure(item_no, &dat_root, &mut images));
        match handle {
            Some(h) => {
                if image.image != h {
                    image.image = h;
                }
                if image.color != Color::WHITE {
                    image.color = Color::WHITE;
                }
                if node.display != Display::Flex {
                    node.display = Display::Flex;
                }
            }
            None => {
                if node.display != Display::None {
                    node.display = Display::None;
                }
            }
        }
    }
}

pub fn item_row_mouse_hover_system(
    mut mode: ResMut<InputMode>,
    dynamic: Res<DynamicMenu>,
    rows: Query<(&Interaction, &ItemScreenRow), Changed<Interaction>>,
) {
    let InputMode::Menu(stack) = &mut *mode else {
        return;
    };
    let Some(level) = stack.current_mut() else {
        return;
    };
    if !matches!(level.kind, MenuKind::Items) {
        return;
    }
    let total = dynamic.rows.len();
    let start = viewport_start(level.cursor, total);
    for (interaction, row) in &rows {
        if !matches!(interaction, Interaction::Hovered | Interaction::Pressed) {
            continue;
        }
        let list_idx = start + row.slot;
        if list_idx < total && level.cursor != list_idx {
            level.cursor = list_idx;
        }
    }
}

pub fn item_row_mouse_click_system(
    mode: Res<InputMode>,
    dynamic: Res<DynamicMenu>,
    rows: Query<(&Interaction, &ItemScreenRow), Changed<Interaction>>,
    mut out: MessageWriter<MenuRowActivated>,
) {
    let InputMode::Menu(stack) = &*mode else {
        return;
    };
    let Some(level) = stack.current() else {
        return;
    };
    if !matches!(level.kind, MenuKind::Items) {
        return;
    }
    let total = dynamic.rows.len();
    let start = viewport_start(level.cursor, total);
    for (interaction, row) in &rows {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let list_idx = start + row.slot;
        if list_idx < total {
            out.write(MenuRowActivated { slot: list_idx });
        }
    }
}

fn transparent_placeholder(images: &mut Assets<Image>) -> Handle<Image> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_keeps_short_lists_at_top() {
        assert_eq!(viewport_start(0, 5), 0);
        assert_eq!(viewport_start(4, 5), 0);
        assert_eq!(viewport_start(0, ITEM_LIST_ROWS), 0);
    }

    #[test]
    fn viewport_centers_and_clamps() {
        let total = ITEM_LIST_ROWS * 3;
        // Cursor near the top stays pinned to 0.
        assert_eq!(viewport_start(0, total), 0);
        // Mid-list centers the cursor.
        let mid = total / 2;
        assert_eq!(viewport_start(mid, total), mid - ITEM_LIST_ROWS / 2);
        // Cursor at the end clamps so the last row is visible.
        assert_eq!(viewport_start(total - 1, total), total - ITEM_LIST_ROWS);
    }
}
