use bevy::prelude::*;

use crate::hud::item_dat_root::{ItemDatRoot, ItemIconCache};
use crate::hud::item_detail::{self, ItemMenuFocus, SortOptionId, SortOptions, SORT_OPTIONS};
use crate::hud::item_ui::{self, cursor_prefix, framed_box, text_font, theme};
use crate::hud::menu::{DynamicMenu, MenuRowActivated};
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

/// Short bag names for the tab strip; the header still shows the full
/// `container::name`.
pub fn tab_label(id: u8) -> &'static str {
    use ffxi_proto::map::container as c;
    match id {
        c::LOC_INVENTORY => "Inv",
        c::LOC_MOGSAFE => "Safe",
        c::LOC_MOGSAFE2 => "Safe2",
        c::LOC_STORAGE => "Storage",
        c::LOC_MOGLOCKER => "Locker",
        c::LOC_MOGSATCHEL => "Satchel",
        c::LOC_MOGSACK => "Sack",
        c::LOC_MOGCASE => "Case",
        c::LOC_WARDROBE => "Wdr1",
        c::LOC_WARDROBE2 => "Wdr2",
        c::LOC_WARDROBE3 => "Wdr3",
        c::LOC_WARDROBE4 => "Wdr4",
        c::LOC_WARDROBE5 => "Wdr5",
        c::LOC_WARDROBE6 => "Wdr6",
        c::LOC_WARDROBE7 => "Wdr7",
        c::LOC_WARDROBE8 => "Wdr8",
        c::LOC_TEMPITEMS => "Temp",
        _ => "?",
    }
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

/// Retail's "Select active window" key (F on compact keyboards, Numpad + on
/// the full keyboard) inside the Items window: each press steps focus along
/// the window's panes — every accessible bag in display order, then the sort
/// box, then back to the first bag. Returns the newly shown bag id when the
/// bag changed (the caller resets the list cursor).
pub fn select_active_window(
    snap: &ffxi_viewer_wire::SceneSnapshot,
    active: &mut ItemScreenContainer,
    focus: &mut ItemMenuFocus,
    sort: &SortOptions,
) -> Option<u8> {
    let bags = accessible_containers(snap);
    if focus.sort_focused() {
        // The sort box is the last pane; wrap back to the first bag.
        focus.exit_sort();
        let first = *bags.first()?;
        (first != active.0).then(|| {
            active.0 = first;
            first
        })
    } else {
        let pos = bags.iter().position(|&id| id == active.0).unwrap_or(0);
        match bags.get(pos + 1) {
            Some(&next) => {
                active.0 = next;
                Some(next)
            }
            None => {
                // Keyboard entry lands the cursor on the active sort mode.
                focus.enter_sort(sort.active());
                None
            }
        }
    }
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

/// The bag tab strip above the item list; hidden while only one bag is
/// accessible.
#[derive(Component)]
pub(crate) struct BagTabRow;

/// One tab in the strip; the index is a position into
/// `accessible_containers`, not a container id.
#[derive(Component, Clone, Copy)]
pub(crate) struct BagTab(usize);

#[derive(Component, Clone, Copy)]
pub(crate) struct BagTabText(usize);

/// The sort-options box beside the list. Shown only in Inventory mode; the
/// action-ring Usable list has no sort pane.
#[derive(Component)]
pub(crate) struct ItemSortBox;

/// Which list the item window is showing. Inventory is the full main-menu bag
/// browser (tabs + sort + per-item context on select); Usable is the action
/// ring's cross-container "Items" list (no tabs, no sort, use-on-select).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemScreenMode {
    Inventory,
    Usable,
}

pub(crate) fn item_screen_mode(mode: &InputMode) -> Option<ItemScreenMode> {
    let InputMode::Menu(stack) = mode else {
        return None;
    };
    match stack.current()?.kind {
        MenuKind::Items => Some(ItemScreenMode::Inventory),
        MenuKind::UsableItems => Some(ItemScreenMode::Usable),
        _ => None,
    }
}

pub fn items_open(mode: &InputMode) -> bool {
    item_screen_mode(mode).is_some()
}

fn items_cursor(mode: &InputMode) -> usize {
    match mode {
        InputMode::Menu(stack) if items_open(mode) => {
            stack.current().map(|l| l.cursor).unwrap_or(0)
        }
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
    rows.get(idx)?.action.item_no()
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
                spawn_bag_tabs(col);
                spawn_list_box(col, placeholder.clone());
                spawn_detail_box(col, placeholder.clone());
            });

            spawn_sort_box(root);
        });
}

fn spawn_bag_tabs(col: &mut ChildSpawnerCommands) {
    col.spawn((
        BagTabRow,
        Node {
            width: Val::Px(LIST_WIDTH_PX),
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            column_gap: Val::Px(3.0),
            row_gap: Val::Px(3.0),
            display: Display::None,
            ..default()
        },
    ))
    .with_children(|strip| {
        for i in 0..BAG_DISPLAY_ORDER.len() {
            strip
                .spawn((
                    BagTab(i),
                    Button,
                    Node {
                        padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
                        display: Display::None,
                        ..default()
                    },
                    BackgroundColor(theme::CELL_BG),
                ))
                .with_children(|tab| {
                    tab.spawn((
                        BagTabText(i),
                        Text::new(""),
                        text_font(11.0),
                        TextColor(theme::MUTED),
                    ));
                });
        }
    });
}

fn spawn_list_box(col: &mut ChildSpawnerCommands, placeholder: Handle<Image>) {
    let (mut n, bg, bd) = framed_box();
    n.width = Val::Px(LIST_WIDTH_PX);
    col.spawn((n, bg, bd)).with_children(|p| {
        p.spawn((
            ItemText(ItemRole::ListHeader),
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
    root.spawn((ItemSortBox, n, bg, bd)).with_children(|p| {
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
    bindings: Res<crate::keybinds::Bindings>,
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
    mut sortbox_q: Query<
        &mut Node,
        (
            With<ItemSortBox>,
            Without<ItemWindowRoot>,
            Without<ItemText>,
            Without<ItemIcon>,
            Without<ItemListRow>,
        ),
    >,
) {
    let screen_mode = item_screen_mode(&mode);
    let open = screen_mode.is_some();
    if let Ok(mut node) = root_q.single_mut() {
        let want = if open { Display::Flex } else { Display::None };
        if node.display != want {
            node.display = want;
        }
    }
    // The sort pane belongs to the full inventory browser only; the action-ring
    // Usable list hides it (and never takes sort focus).
    let inventory = screen_mode == Some(ItemScreenMode::Inventory);
    if let Ok(mut node) = sortbox_q.single_mut() {
        let want = if inventory {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
    }
    if !open || !inventory {
        // Leaving the window, or showing the Usable list, drops sort focus so
        // the inventory browser reopens on the list.
        if focus.sort_focused() {
            focus.exit_sort();
        }
        if !open {
            return;
        }
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

    let now_vana = crate::hud::item_meta::now_vana_ts();
    let unusable: Vec<bool> = rows
        .iter()
        .map(|row| {
            row.action
                .item_slot()
                .and_then(|(container, index)| {
                    crate::hud::item_meta::find_slot(snap, container, index)
                })
                .is_some_and(|it| crate::hud::item_meta::item_unusable(it, now_vana))
        })
        .collect();

    let focused_item = item_detail::selected_item_no(&mode, &dynamic);
    let focused_slot = item_detail::selected_slot(&mode, &dynamic);
    let (detail_name, detail_rows) =
        item_ui::focus_detail(focused_item, focused_slot, snap, &dat_root, &mut icon_cache);
    // The Usable list spans every container, so it has no single bag/capacity —
    // it shows a plain "Items" header like retail's command-menu list.
    let header = if inventory {
        let bag_name = ffxi_proto::map::container::name(active_bag.0).unwrap_or("Items");
        let (used, capacity) = snap
            .container(active_bag.0)
            .map(|c| (c.items.len(), c.capacity))
            .unwrap_or((0, 0));
        format!("{bag_name}  {used}/{capacity}")
    } else {
        "Items".to_string()
    };
    // Sort-pane hint follows whatever key SelectActiveWindow (FFXI's window-change
    // key) is currently bound to, rather than a hard-coded key name.
    let sort_title = match bindings.key_label(crate::keybinds::Action::SelectActiveWindow) {
        Some(label) => format!("Sort  [{label}]"),
        None => "Sort".to_string(),
    };

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
            &sort_title,
            &sort,
            &focus,
            &unusable,
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
    sort_title: &str,
    sort: &SortOptions,
    focus: &ItemMenuFocus,
    unusable: &[bool],
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
                    let color = if is_cursor {
                        theme::CURSOR
                    } else if unusable.get(list_idx) == Some(&true) {
                        theme::MUTED
                    } else {
                        theme::TEXT
                    };
                    (
                        format!("{}{}", cursor_prefix(is_cursor), entry.label),
                        color,
                        true,
                    )
                }
                None => (String::new(), theme::TEXT, false),
            }
        }
        ItemRole::DetailName => (detail_name.to_string(), theme::TITLE, true),
        ItemRole::DetailRow(i) => match detail_rows.get(i) {
            Some(line) => (line.clone(), theme::TEXT, true),
            None => (String::new(), theme::TEXT, false),
        },
        ItemRole::SortTitle => (sort_title.to_string(), theme::TITLE, true),
        ItemRole::SortRow(i) => match SORT_OPTIONS.get(i).copied() {
            Some(id) => {
                let active = sort.active() == id;
                let cursor = focus.sort_selection() == Some(id);
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
    if !matches!(level.kind, MenuKind::Items | MenuKind::UsableItems) {
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
            if focus.sort_focused() {
                focus.exit_sort();
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
    if !matches!(level.kind, MenuKind::Items | MenuKind::UsableItems) {
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

/// Drives the tab strip: one tab per accessible bag in display order, active
/// bag highlighted. Split from `update_item_screen` so the tab queries stay
/// disjoint from the window's text/icon queries.
pub(crate) fn update_bag_tabs(
    mode: Res<InputMode>,
    state: Res<SceneState>,
    active_bag: Res<ItemScreenContainer>,
    mut strip_q: Query<&mut Node, (With<BagTabRow>, Without<BagTab>)>,
    mut tab_q: Query<(&BagTab, &mut Node, &mut BackgroundColor), Without<BagTabRow>>,
    mut text_q: Query<(&BagTabText, &mut Text, &mut TextColor)>,
) {
    if !items_open(&mode) {
        return;
    }
    // Bag tabs belong to the full inventory browser; the action-ring Usable list
    // is a flat cross-container list with no bags to flip.
    let inventory = item_screen_mode(&mode) == Some(ItemScreenMode::Inventory);
    let bags = accessible_containers(&state.snapshot);
    let strip_visible = inventory && bags.len() > 1;
    if let Ok(mut node) = strip_q.single_mut() {
        let want = if strip_visible {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
    }
    for (tab, mut node, mut bg) in tab_q.iter_mut() {
        let visible = strip_visible && tab.0 < bags.len();
        let want = if visible {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
        if visible {
            let active = bags[tab.0] == active_bag.0;
            let want_bg = if active {
                theme::CURSOR_BG
            } else {
                theme::CELL_BG
            };
            if bg.0 != want_bg {
                bg.0 = want_bg;
            }
        }
    }
    for (tag, mut text, mut color) in text_q.iter_mut() {
        let Some(&id) = bags.get(tag.0) else {
            continue;
        };
        let want = tab_label(id);
        if **text != want {
            **text = want.to_string();
        }
        let want_color = if id == active_bag.0 {
            theme::TITLE
        } else {
            theme::MUTED
        };
        if color.0 != want_color {
            color.0 = want_color;
        }
    }
}

/// Clicking a tab jumps straight to that bag (the keyboard path is the
/// "Select active window" key stepping through the panes).
pub(crate) fn bag_tab_mouse_system(
    mut mode: ResMut<InputMode>,
    state: Res<SceneState>,
    mut active_bag: ResMut<ItemScreenContainer>,
    tabs: Query<(&Interaction, &BagTab), Changed<Interaction>>,
) {
    if !items_open(&mode) {
        return;
    }
    for (interaction, tab) in &tabs {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let bags = accessible_containers(&state.snapshot);
        let Some(&id) = bags.get(tab.0) else {
            continue;
        };
        if id != active_bag.0 {
            active_bag.0 = id;
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
        let Some(&id) = SORT_OPTIONS.get(i) else {
            continue;
        };
        match interaction {
            Interaction::Hovered => {
                focus.enter_sort(id);
            }
            Interaction::Pressed => {
                focus.enter_sort(id);
                item_detail::apply_sort_option(&mut sort, id);
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

    fn snapshot_with_bags(caps: &[(u8, u16)]) -> ffxi_viewer_wire::SceneSnapshot {
        ffxi_viewer_wire::SceneSnapshot {
            containers: caps
                .iter()
                .map(|&(id, capacity)| ffxi_viewer_wire::ContainerView {
                    id,
                    capacity,
                    items: Vec::new(),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn select_active_window_steps_bags_then_sort_then_wraps() {
        use ffxi_proto::map::container as c;
        let snap = snapshot_with_bags(&[
            (c::LOC_INVENTORY, 30),
            (c::LOC_MOGCASE, 80),
            (c::LOC_WARDROBE, 80),
        ]);
        let mut active = ItemScreenContainer(c::LOC_INVENTORY);
        let mut focus = ItemMenuFocus::default();
        let sort = SortOptions { auto: true };
        assert_eq!(
            select_active_window(&snap, &mut active, &mut focus, &sort),
            Some(c::LOC_MOGCASE)
        );
        assert_eq!(
            select_active_window(&snap, &mut active, &mut focus, &sort),
            Some(c::LOC_WARDROBE)
        );
        // Past the last bag: focus moves into the sort box, bag unchanged.
        assert_eq!(
            select_active_window(&snap, &mut active, &mut focus, &sort),
            None
        );
        assert_eq!(focus.sort_selection(), Some(SortOptionId::Auto));
        assert_eq!(active.0, c::LOC_WARDROBE);
        // Next press wraps back to the first bag and leaves the sort box.
        assert_eq!(
            select_active_window(&snap, &mut active, &mut focus, &sort),
            Some(c::LOC_INVENTORY)
        );
        assert!(!focus.sort_focused());
    }

    #[test]
    fn select_active_window_with_one_bag_toggles_sort_box() {
        use ffxi_proto::map::container as c;
        let snap = snapshot_with_bags(&[(c::LOC_INVENTORY, 30)]);
        let mut active = ItemScreenContainer(c::LOC_INVENTORY);
        let mut focus = ItemMenuFocus::default();
        let sort = SortOptions { auto: false };
        assert_eq!(
            select_active_window(&snap, &mut active, &mut focus, &sort),
            None
        );
        // Cursor starts on the current sort mode (Manual here).
        assert_eq!(focus.sort_selection(), Some(SortOptionId::Manual));
        assert_eq!(
            select_active_window(&snap, &mut active, &mut focus, &sort),
            None
        );
        assert!(!focus.sort_focused());
        assert_eq!(active.0, c::LOC_INVENTORY);
    }

    #[test]
    fn every_display_order_bag_has_a_tab_label() {
        for &id in BAG_DISPLAY_ORDER {
            assert_ne!(tab_label(id), "?", "container {id} missing a tab label");
        }
    }
}
