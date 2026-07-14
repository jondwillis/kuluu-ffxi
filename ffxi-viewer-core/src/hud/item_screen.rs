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

const LIST_WIDTH_PX: f32 = 240.0;

const SORT_WIDTH_PX: f32 = 132.0;

/// The bag the Items window shows, an LSB CONTAINER_ID. Set by the Mog Menu
/// storage rows and cycled from the window itself.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ItemScreenContainer(pub u8);

/// Retail's bag-flip order in the item window.
pub const BAG_DISPLAY_ORDER: &[u8] = {
    use ffxi_proto::map::container as c;
    &[
        c::LOC_INVENTORY,
        c::LOC_MOGSAFE,
        c::LOC_MOGSAFE2,
        c::LOC_STORAGE,
        c::LOC_MOGLOCKER,
        c::LOC_MOGSATCHEL,
        c::LOC_MOGSACK,
        c::LOC_MOGCASE,
        c::LOC_WARDROBE,
        c::LOC_WARDROBE2,
        c::LOC_WARDROBE3,
        c::LOC_WARDROBE4,
        c::LOC_WARDROBE5,
        c::LOC_WARDROBE6,
        c::LOC_WARDROBE7,
        c::LOC_WARDROBE8,
        c::LOC_TEMPITEMS,
    ]
};

/// Whether `id` is browsable right now. Mirrors LSB's 0x029 validContainers
/// (vendor/server/src/map/packets/c2s/0x029_item_move.cpp): Safe/Safe 2F/
/// Storage/Locker only inside your own Mog House, everything else whenever the
/// server granted it capacity. Temporary items are server-managed (never a
/// move destination) but stay viewable.
pub fn container_accessible(snap: &ffxi_viewer_wire::SceneSnapshot, id: u8) -> bool {
    use ffxi_proto::map::container as c;
    let granted = snap.container(id).is_some_and(|v| v.capacity > 0);
    let mh_only = matches!(
        id,
        c::LOC_MOGSAFE | c::LOC_MOGSAFE2 | c::LOC_STORAGE | c::LOC_MOGLOCKER
    );
    // Safe 2F additionally needs profile.mhflag & 0x20 server-side; the server
    // streams its capacity regardless, so capacity alone over-offers it.
    let flag_ok = id != c::LOC_MOGSAFE2 || snap.mh_2f_unlocked == Some(true);
    granted && flag_ok && (!mh_only || snap.myroom.is_some())
}

/// The bags the window can flip through, in display order.
pub fn accessible_containers(snap: &ffxi_viewer_wire::SceneSnapshot) -> Vec<u8> {
    BAG_DISPLAY_ORDER
        .iter()
        .copied()
        .filter(|&id| {
            id == ffxi_proto::map::container::LOC_INVENTORY || container_accessible(snap, id)
        })
        .collect()
}

/// Advance the shown bag to the next accessible one (wrapping), returning the
/// new id when it changed.
pub fn cycle_container(
    snap: &ffxi_viewer_wire::SceneSnapshot,
    active: &mut ItemScreenContainer,
) -> Option<u8> {
    let bags = accessible_containers(snap);
    let pos = bags.iter().position(|&id| id == active.0).unwrap_or(0);
    let next = bags[(pos + 1) % bags.len()];
    (next != active.0).then(|| {
        active.0 = next;
        next
    })
}

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
        DynamicMenuAction::OpenItemAction { item_no, .. } => Some(item_no),
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
        p.spawn((
            ItemText(ItemRole::ListHeader),
            Button,
            Text::new(""),
            text_font(14.0),
            TextColor(theme::TITLE),
        ));
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
    mut active_bag: ResMut<ItemScreenContainer>,
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
        if focus.sort_focused {
            focus.sort_focused = false;
        }
        return;
    }

    let snap = &state.snapshot;

    // Leaving the Mog House (or losing a bag) snaps the view back to the
    // inventory rather than showing a bag the server would reject.
    if active_bag.0 != ffxi_proto::map::container::LOC_INVENTORY
        && !container_accessible(snap, active_bag.0)
    {
        active_bag.0 = ffxi_proto::map::container::LOC_INVENTORY;
    }

    let rows = &dynamic.rows;
    let total = rows.len();
    let cursor = items_cursor(&mode);
    let start = viewport_start(cursor, total);

    let focused_item = item_detail::selected_item_no(&mode, &dynamic);
    let (detail_name, detail_rows) =
        item_ui::focus_detail(focused_item, snap, &dat_root, &mut icon_cache);
    let bag_name = ffxi_proto::map::container::name(active_bag.0).unwrap_or("Items");
    let (used, capacity) = snap
        .container(active_bag.0)
        .map(|c| (c.items.len(), c.capacity))
        .unwrap_or((0, 0));
    let bag_count = accessible_containers(snap).len();
    // The ◀ ▶ affordance appears once another bag is reachable (header click /
    // NavLeft cycles).
    let flip = if bag_count > 1 { "  ◀▶" } else { "" };
    let header = format!("{bag_name}  {used}/{capacity}{flip}");

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
                let cursor = focus.sort_focused && focus.sort_cursor == i;
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
            if focus.sort_focused {
                focus.sort_focused = false;
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

/// Clicking the list header flips to the next accessible bag (the keyboard
/// path is NavLeft in the list pane).
pub(crate) fn bag_header_mouse_system(
    mut mode: ResMut<InputMode>,
    state: Res<SceneState>,
    mut active_bag: ResMut<ItemScreenContainer>,
    rows: Query<(&Interaction, &ItemText), Changed<Interaction>>,
) {
    if !items_open(&mode) {
        return;
    }
    for (interaction, tag) in &rows {
        if !matches!(tag.0, ItemRole::ListHeader) || *interaction != Interaction::Pressed {
            continue;
        }
        if cycle_container(&state.snapshot, &mut active_bag).is_some() {
            if let InputMode::Menu(stack) = &mut *mode {
                if let Some(level) = stack.current_mut() {
                    level.cursor = 0;
                }
            }
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
                focus.sort_focused = true;
                focus.sort_cursor = i;
            }
            Interaction::Pressed => {
                focus.sort_focused = true;
                focus.sort_cursor = i;
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
