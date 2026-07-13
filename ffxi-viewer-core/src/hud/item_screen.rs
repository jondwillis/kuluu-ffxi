use bevy::prelude::*;

use crate::hud::item_dat_root::{ItemDatRoot, ItemIconCache};
use crate::hud::item_detail::{self, ItemMenuFocus, SortOptionId, SortOptions, SORT_OPTIONS};
use crate::hud::item_ui::{self, framed_box, text_font, theme};
use crate::hud::menu::{DynamicMenu, DynamicMenuAction, MenuRowActivated};
use crate::input_mode::{InputMode, MenuKind};
use crate::snapshot::SceneState;

pub const ITEM_LIST_ROWS: usize = 13;

const DETAIL_ROWS: usize = 10;

const ROW_ICON_PX: f32 = 18.0;

const DETAIL_ICON_PX: f32 = 32.0;

const MAIN_BAG_CAPACITY: u32 = 80;

const LIST_WIDTH_PX: f32 = 240.0;

const SORT_WIDTH_PX: f32 = 132.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ItemRole {
    ListHeader,
    ListRowText(usize),
    DetailName,
    DetailRow(usize),
    SortTitle,
    SortRow(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IconSlot {
    ListRow(usize),
    Detail,
}

#[derive(Component)]
pub(crate) struct ItemWindowRoot;

#[derive(Component, Clone, Copy)]
pub(crate) struct ItemText(ItemRole);

#[derive(Component, Clone, Copy)]
pub(crate) struct ItemIcon(IconSlot);

/// The per-list-row flex container (icon + label). Carries `Button` so the row
/// is mouse-selectable; toggling its `Node` display hides the whole row.
#[derive(Component, Clone, Copy)]
pub(crate) struct ItemListRow(usize);

pub fn items_open(mode: &InputMode) -> bool {
    matches!(mode, InputMode::Menu(stack)
        if stack.current().is_some_and(|l| matches!(l.kind, MenuKind::Items)))
}

fn items_cursor(mode: &InputMode) -> usize {
    match mode {
        InputMode::Menu(stack) => stack
            .current()
            .filter(|l| matches!(l.kind, MenuKind::Items))
            .map(|l| l.cursor)
            .unwrap_or(0),
        _ => 0,
    }
}

/// First visible list index, keeping `cursor` inside the `ITEM_LIST_ROWS`
/// viewport.
pub fn viewport_start(cursor: usize, total: usize) -> usize {
    super::nav_geometry::scroll_window(cursor, total, ITEM_LIST_ROWS)
}

fn row_item_no(rows: &[crate::hud::menu::DynamicMenuRow], idx: usize) -> Option<u16> {
    match rows.get(idx)?.action {
        DynamicMenuAction::UseItem { item_no, .. } => Some(item_no),
        _ => None,
    }
}

pub(crate) fn spawn_item_screen(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = item_ui::transparent_placeholder(&mut images);

    commands
        .spawn((
            crate::components::InGameEntity,
            ItemWindowRoot,
            // One self-contained, content-sized window anchored top-left (like
            // the Equipment screen): list + detail stacked in the left column,
            // the sort box beside it. Sizing to content keeps it clear of the
            // corner HUD (minimap, chat) instead of spanning the screen.
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(48.0),
                left: Val::Px(8.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::FlexStart,
                column_gap: Val::Px(6.0),
                display: Display::None,
                ..default()
            },
            ZIndex(item_ui::WINDOW_Z),
        ))
        .with_children(|root| {
            root.spawn(Node {
                width: Val::Px(LIST_WIDTH_PX),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..default()
            })
            .with_children(|col| {
                spawn_list_box(col, placeholder.clone());
                spawn_detail_box(col, placeholder.clone());
            });

            spawn_sort_box(root);
        });
}

fn spawn_list_box(col: &mut ChildSpawnerCommands, placeholder: Handle<Image>) {
    let (mut n, bg, bd) = framed_box();
    n.width = Val::Px(LIST_WIDTH_PX);
    col.spawn((n, bg, bd)).with_children(|p| {
        spawn_text(p, ItemRole::ListHeader, 14.0, theme::TITLE);
        p.spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(2.0),
            margin: UiRect::top(Val::Px(4.0)),
            ..default()
        })
        .with_children(|list| {
            for i in 0..ITEM_LIST_ROWS {
                list.spawn((
                    ItemListRow(i),
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
                        ItemIcon(IconSlot::ListRow(i)),
                        Node {
                            width: Val::Px(ROW_ICON_PX),
                            height: Val::Px(ROW_ICON_PX),
                            display: Display::None,
                            ..default()
                        },
                        ImageNode::new(placeholder.clone()),
                    ));
                    row.spawn((
                        ItemText(ItemRole::ListRowText(i)),
                        Text::new(""),
                        text_font(13.0),
                        TextColor(theme::TEXT),
                    ));
                });
            }
        });
    });
}

fn spawn_detail_box(col: &mut ChildSpawnerCommands, placeholder: Handle<Image>) {
    let (mut n, bg, bd) = framed_box();
    n.width = Val::Px(LIST_WIDTH_PX);
    col.spawn((n, bg, bd)).with_children(|p| {
        p.spawn(Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(6.0),
            ..default()
        })
        .with_children(|h| {
            h.spawn((
                ItemIcon(IconSlot::Detail),
                Node {
                    width: Val::Px(DETAIL_ICON_PX),
                    height: Val::Px(DETAIL_ICON_PX),
                    display: Display::None,
                    ..default()
                },
                ImageNode::new(placeholder),
            ));
            h.spawn((
                ItemText(ItemRole::DetailName),
                Text::new(""),
                text_font(14.0),
                TextColor(theme::TITLE),
            ));
        });
        for i in 0..DETAIL_ROWS {
            spawn_row(p, ItemRole::DetailRow(i), 12.0, theme::TEXT);
        }
    });
}

fn spawn_sort_box(root: &mut ChildSpawnerCommands) {
    let (mut n, bg, bd) = framed_box();
    n.width = Val::Px(SORT_WIDTH_PX);
    root.spawn((n, bg, bd)).with_children(|p| {
        spawn_text(p, ItemRole::SortTitle, 13.0, theme::TITLE);
        for i in 0..SORT_OPTIONS.len() {
            p.spawn((
                ItemText(ItemRole::SortRow(i)),
                Button,
                Text::new(""),
                text_font(12.0),
                TextColor(theme::MUTED),
            ));
        }
    });
}

fn spawn_text(p: &mut ChildSpawnerCommands, role: ItemRole, size: f32, color: Color) {
    p.spawn((
        ItemText(role),
        Text::new(""),
        text_font(size),
        TextColor(color),
    ));
}

fn spawn_row(p: &mut ChildSpawnerCommands, role: ItemRole, size: f32, color: Color) {
    p.spawn((
        ItemText(role),
        Text::new(""),
        text_font(size),
        TextColor(color),
        Node {
            display: Display::None,
            ..default()
        },
    ));
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_item_screen(
    mode: Res<InputMode>,
    state: Res<SceneState>,
    dynamic: Res<DynamicMenu>,
    sort: Res<SortOptions>,
    mut focus: ResMut<ItemMenuFocus>,
    dat_root: Res<ItemDatRoot>,
    mut icon_cache: ResMut<ItemIconCache>,
    mut images: ResMut<Assets<Image>>,
    mut root_q: Query<
        &mut Node,
        (
            With<ItemWindowRoot>,
            Without<ItemText>,
            Without<ItemIcon>,
            Without<ItemListRow>,
        ),
    >,
    mut listrow_q: Query<
        (&ItemListRow, &mut Node),
        (
            Without<ItemWindowRoot>,
            Without<ItemText>,
            Without<ItemIcon>,
        ),
    >,
    mut text_q: Query<
        (&ItemText, &mut Text, &mut TextColor, &mut Node),
        (
            Without<ItemWindowRoot>,
            Without<ItemIcon>,
            Without<ItemListRow>,
        ),
    >,
    mut icon_q: Query<
        (&ItemIcon, &mut Node, &mut ImageNode),
        (
            Without<ItemWindowRoot>,
            Without<ItemText>,
            Without<ItemListRow>,
        ),
    >,
) {
    let open = items_open(&mode);
    if let Ok(mut node) = root_q.single_mut() {
        let want = if open { Display::Flex } else { Display::None };
        if node.display != want {
            node.display = want;
        }
    }
    if !open {
        // Leaving the window drops sort focus so it reopens on the list.
        if focus.secondary_focused {
            focus.secondary_focused = false;
        }
        return;
    }

    let snap = &state.snapshot;
    let rows = &dynamic.rows;
    let total = rows.len();
    let cursor = items_cursor(&mode);
    let start = viewport_start(cursor, total);

    let focused_item = item_detail::selected_item_no(&mode, &dynamic);
    let (detail_name, detail_rows) =
        item_ui::focus_detail(focused_item, snap, &dat_root, &mut icon_cache);
    let header = format!("Items  {}/{}", snap.inventory_main.len(), MAIN_BAG_CAPACITY);

    for (row, mut node) in listrow_q.iter_mut() {
        let list_idx = start + row.0;
        let visible = list_idx < total;
        let want = if visible {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
    }

    for (tag, mut text, mut color, mut node) in text_q.iter_mut() {
        let (want, want_color, visible) = role_value(
            tag.0,
            rows,
            total,
            cursor,
            start,
            &header,
            &detail_name,
            &detail_rows,
            &sort,
            &focus,
        );
        let display = if visible {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
        if visible && **text != want {
            **text = want;
        }
        if color.0 != want_color {
            color.0 = want_color;
        }
    }

    for (icon, mut node, mut image) in icon_q.iter_mut() {
        let item = match icon.0 {
            IconSlot::ListRow(i) => {
                let list_idx = start + i;
                (list_idx < total)
                    .then(|| row_item_no(rows, list_idx))
                    .flatten()
            }
            IconSlot::Detail => focused_item,
        };
        let handle = item.and_then(|n| icon_cache.ensure(n, &dat_root, &mut images));
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

#[allow(clippy::too_many_arguments)]
fn role_value(
    role: ItemRole,
    rows: &[crate::hud::menu::DynamicMenuRow],
    total: usize,
    cursor: usize,
    start: usize,
    header: &str,
    detail_name: &str,
    detail_rows: &[String],
    sort: &SortOptions,
    focus: &ItemMenuFocus,
) -> (String, Color, bool) {
    match role {
        ItemRole::ListHeader => (header.to_string(), theme::TITLE, true),
        ItemRole::ListRowText(i) => {
            let list_idx = start + i;
            if total == 0 {
                return if i == 0 {
                    ("(inventory empty)".to_string(), theme::MUTED, true)
                } else {
                    (String::new(), theme::TEXT, false)
                };
            }
            match rows.get(list_idx) {
                Some(entry) => {
                    let is_cursor = list_idx == cursor;
                    let prefix = if is_cursor { "> " } else { "  " };
                    let color = if is_cursor {
                        theme::CURSOR
                    } else {
                        theme::TEXT
                    };
                    (format!("{prefix}{}", entry.label), color, true)
                }
                None => (String::new(), theme::TEXT, false),
            }
        }
        ItemRole::DetailName => (detail_name.to_string(), theme::TITLE, true),
        ItemRole::DetailRow(i) => match detail_rows.get(i) {
            Some(line) => (line.clone(), theme::TEXT, true),
            None => (String::new(), theme::TEXT, false),
        },
        ItemRole::SortTitle => ("Sort".to_string(), theme::TITLE, true),
        ItemRole::SortRow(i) => match SORT_OPTIONS.get(i).copied() {
            Some(id) => {
                let active = match id {
                    SortOptionId::Auto => sort.auto,
                    SortOptionId::Manual => !sort.auto,
                };
                let cursor = focus.secondary_focused && focus.secondary_cursor == i;
                let name = match id {
                    SortOptionId::Auto => "Auto",
                    SortOptionId::Manual => "Manual",
                };
                let marker = if active { "\u{25cf}" } else { "\u{25cb}" };
                let prefix = if cursor { ">" } else { " " };
                let color = if cursor {
                    theme::CURSOR
                } else if active {
                    theme::TEXT
                } else {
                    theme::MUTED
                };
                (format!("{prefix} {marker} {name}"), color, true)
            }
            None => (String::new(), theme::TEXT, false),
        },
    }
}

pub(crate) fn item_row_mouse_hover_system(
    mut mode: ResMut<InputMode>,
    dynamic: Res<DynamicMenu>,
    mut focus: ResMut<ItemMenuFocus>,
    rows: Query<(&Interaction, &ItemListRow), Changed<Interaction>>,
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
        let list_idx = start + row.0;
        if list_idx < total {
            // Hovering the list returns focus here, mirroring the sort box
            // grabbing it on hover — so neither pane traps the keyboard.
            if focus.secondary_focused {
                focus.secondary_focused = false;
            }
            if level.cursor != list_idx {
                level.cursor = list_idx;
            }
        }
    }
}

pub(crate) fn item_row_mouse_click_system(
    mode: Res<InputMode>,
    dynamic: Res<DynamicMenu>,
    rows: Query<(&Interaction, &ItemListRow), Changed<Interaction>>,
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
        let list_idx = start + row.0;
        if list_idx < total {
            out.write(MenuRowActivated { slot: list_idx });
        }
    }
}

pub(crate) fn sort_option_mouse_system(
    mode: Res<InputMode>,
    mut sort: ResMut<SortOptions>,
    mut focus: ResMut<ItemMenuFocus>,
    mut sort_req: MessageWriter<item_detail::InventorySortRequested>,
    rows: Query<(&Interaction, &ItemText), Changed<Interaction>>,
) {
    if !items_open(&mode) {
        return;
    }
    for (interaction, tag) in &rows {
        let ItemRole::SortRow(i) = tag.0 else {
            continue;
        };
        match interaction {
            Interaction::Hovered => {
                focus.secondary_focused = true;
                focus.secondary_cursor = i;
            }
            Interaction::Pressed => {
                focus.secondary_focused = true;
                focus.secondary_cursor = i;
                if let Some(&id) = SORT_OPTIONS.get(i) {
                    item_detail::apply_sort_option(&mut sort, id);
                }
                sort_req.write(item_detail::InventorySortRequested {
                    container: ffxi_proto::map::container::LOC_INVENTORY,
                });
            }
            Interaction::None => {}
        }
    }
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
        assert_eq!(viewport_start(0, total), 0);
        let mid = total / 2;
        assert_eq!(viewport_start(mid, total), mid - ITEM_LIST_ROWS / 2);
        assert_eq!(viewport_start(total - 1, total), total - ITEM_LIST_ROWS);
    }
}
