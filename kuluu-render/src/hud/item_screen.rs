use bevy::prelude::*;

use crate::hud::item_dat_root::{ItemDatRoot, ItemIconCache};
use crate::hud::item_detail::{self, ItemMenuFocus, SortOptionId, SortOptions, SORT_OPTIONS};
use crate::hud::item_ui::{self, cursor_prefix, framed_box, text_font, theme};
use crate::hud::menu::{self, DynamicMenuRow, MenuRowActivated};
use crate::input_mode::{InputMode, MenuKind, MenuLevel, MenuStack};
use crate::snapshot::SceneState;

/// Rows on one page of the retail item list
/// (.agents/skills/retail-observe/references/2026-09-11-items-window.md).
pub const ITEM_LIST_ROWS: usize = 10;

const DETAIL_ROWS: usize = 10;

const ROW_ICON_PX: f32 = 18.0;

const DETAIL_ICON_PX: f32 = 32.0;

const LIST_WIDTH_PX: f32 = 240.0;

const DETAIL_WIDTH_PX: f32 = 300.0;

const OPTIONS_WIDTH_PX: f32 = 132.0;

const SCROLLBAR_WIDTH_PX: f32 = 4.0;

const BADGE_FONT_PX: f32 = 9.0;

pub const OPTIONS_TITLE: &str = "Options";

pub const OPTIONS_SORT_LABEL: &str = "Sort";

/// The bag the Items window shows, an LSB CONTAINER_ID. Set by the Mog Menu
/// storage rows and cycled from the window itself.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ItemScreenContainer(pub u8);

/// First visible row of the item list. Retail scrolls one row when the cursor
/// leaves the page and shifts the page along with the cursor on Left/Right, so
/// the offset is state, not a function of the cursor.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ItemListViewport {
    pub start: usize,
}

impl ItemListViewport {
    fn max_start(total: usize) -> usize {
        total.saturating_sub(ITEM_LIST_ROWS)
    }

    pub fn follow(&mut self, cursor: usize, total: usize) {
        if cursor < self.start {
            self.start = cursor;
        } else if cursor >= self.start + ITEM_LIST_ROWS {
            self.start = cursor + 1 - ITEM_LIST_ROWS;
        }
        self.start = self.start.min(Self::max_start(total));
    }

    /// Left/Right: the page shifts by a full row count in step with the
    /// cursor (see [`page_cursor`]), clamped at both ends, no wrap.
    pub fn page(&mut self, forward: bool, total: usize) {
        self.start = if forward {
            (self.start + ITEM_LIST_ROWS).min(Self::max_start(total))
        } else {
            self.start.saturating_sub(ITEM_LIST_ROWS)
        };
    }
}

/// Up/Down in the item list: one row, clamped at both ends (retail: Up on the
/// first row and Down on the last stay put).
pub fn step_cursor(cursor: usize, total: usize, down: bool) -> usize {
    if down {
        (cursor + 1).min(total.saturating_sub(1))
    } else {
        cursor.saturating_sub(1)
    }
}

/// Left/Right in the item list: the cursor jumps a full row count, clamped.
pub fn page_cursor(cursor: usize, total: usize, forward: bool) -> usize {
    if forward {
        (cursor + ITEM_LIST_ROWS).min(total.saturating_sub(1))
    } else {
        cursor.saturating_sub(ITEM_LIST_ROWS)
    }
}

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
pub fn container_accessible(snap: &kuluu_snapshot::SceneSnapshot, id: u8) -> bool {
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
        c::LOC_RECYCLEBIN => "Bin",
        _ => "?",
    }
}

/// The bags the window can flip through, in display order.
pub fn accessible_containers(snap: &kuluu_snapshot::SceneSnapshot) -> Vec<u8> {
    BAG_DISPLAY_ORDER
        .iter()
        .copied()
        .filter(|&id| {
            id == ffxi_proto::map::container::LOC_INVENTORY || container_accessible(snap, id)
        })
        .collect()
}

/// Retail's "Select active window" key (Numpad + on the full keyboard) inside
/// the Items window: each press steps focus along the window's panes — every
/// accessible bag in display order, then the Options box, then back to the
/// first bag. Returns the newly shown bag id when the bag changed (the caller
/// resets the list cursor).
pub fn select_active_window(
    snap: &kuluu_snapshot::SceneSnapshot,
    active: &mut ItemScreenContainer,
    focus: &mut ItemMenuFocus,
    sort: &SortOptions,
) -> Option<u8> {
    let bags = accessible_containers(snap);
    if focus.sort_focused() {
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

/// The Options box's Recycle Bin row: browse the bin when the server grants
/// one, returning focus to the list. Returns whether the bag changed (the
/// caller resets the list cursor).
pub fn open_recycle_bin(
    snap: &kuluu_snapshot::SceneSnapshot,
    active: &mut ItemScreenContainer,
    focus: &mut ItemMenuFocus,
) -> bool {
    let bin = ffxi_proto::map::container::LOC_RECYCLEBIN;
    if !container_accessible(snap, bin) {
        return false;
    }
    focus.exit_sort();
    if active.0 == bin {
        return false;
    }
    active.0 = bin;
    true
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ItemRole {
    ListRowText(usize),
    ListBadge(usize),
    DetailName,
    DetailRow(usize),
    OptionsTitle,
    OptionsSortLabel,
    OptionsRow(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IconSlot {
    ListRow(usize),
    Detail,
}

/// The top-left list window.
#[derive(Component)]
pub(crate) struct ItemWindowRoot;

/// The top-right Options box (Sort: Auto / Manual / Recycle Bin), occupying
/// the slot the per-item submenu takes over while it is open.
#[derive(Component)]
pub(crate) struct ItemOptionsRoot;

/// The item card docked above the chat log in the bottom-left stack; retail
/// shows it in place of the compass and clock while the list is open.
#[derive(Component)]
pub(crate) struct ItemDetailCard;

/// Compass + clock wrapper in the bottom-left stack, hidden while the item card
/// takes their place.
#[derive(Component)]
pub struct CompassClockCluster;

#[derive(Component, Clone, Copy)]
pub(crate) struct ItemText(ItemRole);

#[derive(Component, Clone, Copy)]
pub(crate) struct ItemIcon(IconSlot);

/// The per-list-row flex container (icon + label). Carries `Button` so the row
/// is mouse-selectable; toggling its `Node` display hides the whole row.
#[derive(Component, Clone, Copy)]
pub(crate) struct ItemListRow(usize);

#[derive(Component)]
pub(crate) struct ItemScrollTrack;

#[derive(Component)]
pub(crate) struct ItemScrollThumb;

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

/// Which list the item window is showing. Inventory is the full main-menu bag
/// browser (tabs + options + per-item submenu on select); Usable is the action
/// ring's cross-container "Items" list (no tabs, no options, use-on-select).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ItemScreenMode {
    Inventory,
    Usable,
}

/// Levels that stack on top of the list while retail keeps it on screen.
pub fn is_item_submenu(kind: MenuKind) -> bool {
    matches!(
        kind,
        MenuKind::ItemAction { .. } | MenuKind::ItemDropConfirm { .. }
    )
}

/// The Items/UsableItems level the window renders, looking through the
/// per-item submenu levels stacked above it.
pub fn item_list_level(stack: &MenuStack) -> Option<&MenuLevel> {
    stack
        .levels
        .iter()
        .rev()
        .find(|l| !is_item_submenu(l.kind))
        .filter(|l| matches!(l.kind, MenuKind::Items | MenuKind::UsableItems))
}

pub(crate) fn item_screen_mode(mode: &InputMode) -> Option<ItemScreenMode> {
    let InputMode::Menu(stack) = mode else {
        return None;
    };
    match item_list_level(stack)?.kind {
        MenuKind::Items => Some(ItemScreenMode::Inventory),
        MenuKind::UsableItems => Some(ItemScreenMode::Usable),
        _ => None,
    }
}

pub fn items_open(mode: &InputMode) -> bool {
    item_screen_mode(mode).is_some()
}

fn submenu_open(mode: &InputMode) -> bool {
    match mode {
        InputMode::Menu(stack) => stack.current().is_some_and(|l| is_item_submenu(l.kind)),
        _ => false,
    }
}

fn items_cursor(mode: &InputMode) -> usize {
    match mode {
        InputMode::Menu(stack) => item_list_level(stack).map(|l| l.cursor).unwrap_or(0),
        _ => 0,
    }
}

/// The rows the window lists for its mode: the active bag's slots (Inventory)
/// or every usable item across bags (Usable). Built here rather than read from
/// `DynamicMenu` because that resource follows the top level, which is the
/// per-item submenu while one is open.
pub(crate) fn list_rows(
    snap: &kuluu_snapshot::SceneSnapshot,
    mode: ItemScreenMode,
    bag: u8,
    sort: &SortOptions,
) -> Vec<DynamicMenuRow> {
    match mode {
        ItemScreenMode::Inventory => menu::inventory_rows(snap, bag, sort.auto),
        ItemScreenMode::Usable => menu::usable_item_rows(snap),
    }
}

fn row_item_no(rows: &[DynamicMenuRow], idx: usize) -> Option<u16> {
    rows.get(idx)?.action.item_no()
}

pub(crate) fn spawn_item_screen(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = item_ui::transparent_placeholder(&mut images);

    commands
        .spawn((
            crate::components::InGameEntity,
            ItemWindowRoot,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(48.0),
                left: Val::Px(8.0),
                width: Val::Px(LIST_WIDTH_PX),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                display: Display::None,
                ..default()
            },
            ZIndex(item_ui::WINDOW_Z),
        ))
        .with_children(|col| {
            spawn_bag_tabs(col);
            spawn_list_box(col, placeholder);
        });

    commands
        .spawn((
            crate::components::InGameEntity,
            ItemOptionsRoot,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(48.0),
                right: Val::Px(8.0),
                display: Display::None,
                ..default()
            },
            ZIndex(item_ui::WINDOW_Z),
        ))
        .with_children(spawn_options_box);
}

/// Spawned inside the bottom-left stack's compass column so the card sits
/// where retail docks it: above the chat log, in the compass/clock slot.
pub fn spawn_item_detail_card_as_child(col: &mut ChildSpawnerCommands, placeholder: Handle<Image>) {
    let (mut n, bg, bd) = framed_box();
    n.width = Val::Px(DETAIL_WIDTH_PX);
    n.display = Display::None;
    col.spawn((ItemDetailCard, n, bg, bd)).with_children(|p| {
        p.spawn(Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            column_gap: Val::Px(6.0),
            ..default()
        })
        .with_children(|h| {
            h.spawn((
                ItemIcon(IconSlot::Detail),
                Node {
                    width: Val::Px(DETAIL_ICON_PX),
                    height: Val::Px(DETAIL_ICON_PX),
                    flex_shrink: 0.0,
                    display: Display::None,
                    ..default()
                },
                ImageNode::new(placeholder),
            ));
            h.spawn(Node {
                flex_direction: FlexDirection::Column,
                ..default()
            })
            .with_children(|c| {
                c.spawn((
                    ItemText(ItemRole::DetailName),
                    Text::new(""),
                    text_font(14.0),
                    TextColor(theme::TITLE),
                ));
                for i in 0..DETAIL_ROWS {
                    spawn_row(c, ItemRole::DetailRow(i), 12.0, theme::TEXT);
                }
            });
        });
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
    n.flex_direction = FlexDirection::Row;
    n.column_gap = Val::Px(4.0);
    col.spawn((n, bg, bd)).with_children(|p| {
        p.spawn(Node {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_width: Val::Px(0.0),
            row_gap: Val::Px(2.0),
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
                            flex_shrink: 0.0,
                            display: Display::None,
                            ..default()
                        },
                        ImageNode::new(placeholder.clone()),
                    ))
                    .with_children(|icon| {
                        // Retail overlays the stack count on the icon's
                        // top-left corner rather than suffixing the name.
                        icon.spawn((
                            ItemText(ItemRole::ListBadge(i)),
                            Text::new(""),
                            text_font(BADGE_FONT_PX),
                            TextColor(theme::TEXT),
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(0.0),
                                top: Val::Px(-2.0),
                                display: Display::None,
                                ..default()
                            },
                        ));
                    });
                    row.spawn((
                        ItemText(ItemRole::ListRowText(i)),
                        Text::new(""),
                        text_font(13.0),
                        TextColor(theme::TEXT),
                    ));
                });
            }
        });
        p.spawn((
            ItemScrollTrack,
            Node {
                width: Val::Px(SCROLLBAR_WIDTH_PX),
                align_self: AlignSelf::Stretch,
                flex_shrink: 0.0,
                display: Display::None,
                ..default()
            },
            BackgroundColor(theme::CELL_BG),
        ))
        .with_children(|track| {
            track.spawn((
                ItemScrollThumb,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    top: Val::Percent(0.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(theme::FRAME_EDGE),
            ));
        });
    });
}

fn spawn_options_box(root: &mut ChildSpawnerCommands) {
    let (mut n, bg, bd) = framed_box();
    n.width = Val::Px(OPTIONS_WIDTH_PX);
    root.spawn((n, bg, bd)).with_children(|p| {
        spawn_text(p, ItemRole::OptionsTitle, 13.0, theme::TITLE);
        spawn_text(p, ItemRole::OptionsSortLabel, 12.0, theme::MUTED);
        for i in 0..SORT_OPTIONS.len() {
            p.spawn((
                ItemText(ItemRole::OptionsRow(i)),
                Button,
                Text::new(""),
                text_font(12.0),
                TextColor(theme::TEXT),
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

fn set_display(node: &mut Node, visible: bool) {
    let want = if visible {
        Display::Flex
    } else {
        Display::None
    };
    if node.display != want {
        node.display = want;
    }
}

pub fn option_label(id: SortOptionId) -> &'static str {
    match id {
        SortOptionId::Auto => "Auto",
        SortOptionId::Manual => "Manual",
        SortOptionId::RecycleBin => "Recycle Bin",
    }
}

/// Shows/hides the window's three surfaces (list, Options box, item card) and
/// swaps the compass/clock cluster out for the card while the list is up.
pub(crate) fn update_item_screen_layout(
    mode: Res<InputMode>,
    mut focus: ResMut<ItemMenuFocus>,
    mut root_q: Query<&mut Node, (With<ItemWindowRoot>, Without<ItemOptionsRoot>)>,
    mut options_q: Query<
        &mut Node,
        (
            With<ItemOptionsRoot>,
            Without<ItemWindowRoot>,
            Without<ItemDetailCard>,
        ),
    >,
    mut card_q: Query<
        &mut Node,
        (
            With<ItemDetailCard>,
            Without<ItemWindowRoot>,
            Without<ItemOptionsRoot>,
            Without<CompassClockCluster>,
        ),
    >,
    mut cluster_q: Query<
        &mut Node,
        (
            With<CompassClockCluster>,
            Without<ItemWindowRoot>,
            Without<ItemOptionsRoot>,
            Without<ItemDetailCard>,
        ),
    >,
) {
    let screen_mode = item_screen_mode(&mode);
    let open = screen_mode.is_some();
    if let Ok(mut node) = root_q.single_mut() {
        set_display(&mut node, open);
    }
    if let Ok(mut node) = card_q.single_mut() {
        set_display(&mut node, open);
    }
    if let Ok(mut node) = cluster_q.single_mut() {
        set_display(&mut node, !open);
    }
    // The Options box belongs to the full inventory browser only, and the
    // per-item submenu takes its screen slot while open.
    let options_visible = screen_mode == Some(ItemScreenMode::Inventory) && !submenu_open(&mode);
    if let Ok(mut node) = options_q.single_mut() {
        set_display(&mut node, options_visible);
    }
    if !options_visible && focus.sort_focused() {
        focus.exit_sort();
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_item_screen(
    mode: Res<InputMode>,
    state: Res<SceneState>,
    sort: Res<SortOptions>,
    bindings: Res<crate::keybinds::Bindings>,
    mut active_bag: ResMut<ItemScreenContainer>,
    focus: Res<ItemMenuFocus>,
    mut viewport: ResMut<ItemListViewport>,
    dat_root: Res<ItemDatRoot>,
    mut icon_cache: ResMut<ItemIconCache>,
    mut images: ResMut<Assets<Image>>,
    mut listrow_q: Query<(&ItemListRow, &mut Node), (Without<ItemText>, Without<ItemIcon>)>,
    mut text_q: Query<
        (&ItemText, &mut Text, &mut TextColor, &mut Node),
        (Without<ItemIcon>, Without<ItemListRow>),
    >,
    mut icon_q: Query<
        (&ItemIcon, &mut Node, &mut ImageNode),
        (Without<ItemText>, Without<ItemListRow>),
    >,
) {
    let Some(screen_mode) = item_screen_mode(&mode) else {
        return;
    };

    let snap = &state.snapshot;

    // Leaving the Mog House (or losing a bag) snaps the view back to the
    // inventory rather than showing a bag the server would reject.
    if active_bag.0 != ffxi_proto::map::container::LOC_INVENTORY
        && !container_accessible(snap, active_bag.0)
    {
        active_bag.0 = ffxi_proto::map::container::LOC_INVENTORY;
    }

    let rows = list_rows(snap, screen_mode, active_bag.0, &sort);
    let total = rows.len();
    let cursor = items_cursor(&mode);
    viewport.follow(cursor, total);
    let start = viewport.start;

    let now_vana = crate::hud::item_meta::now_vana_ts();
    let slots: Vec<Option<&kuluu_snapshot::InventoryItem>> = rows
        .iter()
        .map(|row| {
            row.action.item_slot().and_then(|(container, index)| {
                crate::hud::item_meta::find_slot(snap, container, index)
            })
        })
        .collect();
    let unusable: Vec<bool> = slots
        .iter()
        .map(|slot| slot.is_some_and(|it| crate::hud::item_meta::item_unusable(it, now_vana)))
        .collect();
    let quantities: Vec<u32> = slots
        .iter()
        .map(|slot| slot.map(|it| it.quantity).unwrap_or(0))
        .collect();

    let focused_item = row_item_no(&rows, cursor);
    let focused_slot = rows.get(cursor).and_then(|r| r.action.item_slot());
    let (detail_name, detail_rows) =
        item_ui::focus_detail(focused_item, focused_slot, snap, &dat_root, &mut icon_cache);
    // Retail prints the window-change key's glyph before "Sort" ("+ :Sort");
    // ours follows whatever SelectActiveWindow is bound to.
    let sort_label = match bindings.key_label(crate::keybinds::Action::SelectActiveWindow) {
        Some(label) => format!("{label} :{OPTIONS_SORT_LABEL}"),
        None => OPTIONS_SORT_LABEL.to_string(),
    };

    for (row, mut node) in listrow_q.iter_mut() {
        set_display(&mut node, start + row.0 < total);
    }

    for (tag, mut text, mut color, mut node) in text_q.iter_mut() {
        let value = role_value(
            tag.0,
            &rows,
            &quantities,
            cursor,
            start,
            &detail_name,
            &detail_rows,
            &sort_label,
            &focus,
            &unusable,
        );
        set_display(&mut node, value.visible);
        if value.visible && **text != value.text {
            **text = value.text;
        }
        if color.0 != value.color {
            color.0 = value.color;
        }
    }

    for (icon, mut node, mut image) in icon_q.iter_mut() {
        let item = match icon.0 {
            IconSlot::ListRow(i) => {
                let list_idx = start + i;
                (list_idx < total)
                    .then(|| row_item_no(&rows, list_idx))
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
                set_display(&mut node, true);
            }
            None => set_display(&mut node, false),
        }
    }
}

/// The list's scrollbar: thumb top/height are the viewport's share of the list.
pub(crate) fn update_item_scrollbar(
    mode: Res<InputMode>,
    state: Res<SceneState>,
    sort: Res<SortOptions>,
    active_bag: Res<ItemScreenContainer>,
    viewport: Res<ItemListViewport>,
    mut track_q: Query<&mut Node, (With<ItemScrollTrack>, Without<ItemScrollThumb>)>,
    mut thumb_q: Query<&mut Node, (With<ItemScrollThumb>, Without<ItemScrollTrack>)>,
) {
    let Some(screen_mode) = item_screen_mode(&mode) else {
        return;
    };
    let total = list_rows(&state.snapshot, screen_mode, active_bag.0, &sort).len();
    let scrollable = total > ITEM_LIST_ROWS;
    if let Ok(mut node) = track_q.single_mut() {
        set_display(&mut node, scrollable);
    }
    if !scrollable {
        return;
    }
    if let Ok(mut node) = thumb_q.single_mut() {
        let top = Val::Percent(viewport.start as f32 / total as f32 * 100.0);
        let height = Val::Percent(ITEM_LIST_ROWS as f32 / total as f32 * 100.0);
        if node.top != top {
            node.top = top;
        }
        if node.height != height {
            node.height = height;
        }
    }
}

struct RoleValue {
    text: String,
    color: Color,
    visible: bool,
}

fn hidden() -> RoleValue {
    RoleValue {
        text: String::new(),
        color: theme::TEXT,
        visible: false,
    }
}

fn plain(text: String, color: Color) -> RoleValue {
    RoleValue {
        text,
        color,
        visible: true,
    }
}

#[allow(clippy::too_many_arguments)]
fn role_value(
    role: ItemRole,
    rows: &[DynamicMenuRow],
    quantities: &[u32],
    cursor: usize,
    start: usize,
    detail_name: &str,
    detail_rows: &[String],
    sort_label: &str,
    focus: &ItemMenuFocus,
    unusable: &[bool],
) -> RoleValue {
    let total = rows.len();
    match role {
        ItemRole::ListRowText(i) => {
            let list_idx = start + i;
            if total == 0 {
                return if i == 0 {
                    plain("(inventory empty)".to_string(), theme::MUTED)
                } else {
                    hidden()
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
                    plain(
                        format!("{}{}", cursor_prefix(is_cursor), entry.label),
                        color,
                    )
                }
                None => hidden(),
            }
        }
        ItemRole::ListBadge(i) => {
            let list_idx = start + i;
            match quantities.get(list_idx) {
                Some(&q) if list_idx < total && q > 1 => plain(q.to_string(), theme::TEXT),
                _ => hidden(),
            }
        }
        ItemRole::DetailName => plain(detail_name.to_string(), theme::TITLE),
        ItemRole::DetailRow(i) => match detail_rows.get(i) {
            Some(line) => plain(line.clone(), theme::TEXT),
            None => hidden(),
        },
        ItemRole::OptionsTitle => plain(OPTIONS_TITLE.to_string(), theme::TITLE),
        ItemRole::OptionsSortLabel => plain(sort_label.to_string(), theme::MUTED),
        // Retail paints Auto / Manual / Recycle Bin identically whichever
        // mode is in effect; only the cursor row differs (.agents/skills/retail-observe/references/2026-09-11-items-window.md).
        ItemRole::OptionsRow(i) => match SORT_OPTIONS.get(i).copied() {
            Some(id) => {
                let is_cursor = focus.sort_selection() == Some(id);
                let color = if is_cursor {
                    theme::CURSOR
                } else {
                    theme::TEXT
                };
                plain(
                    format!("{}{}", cursor_prefix(is_cursor), option_label(id)),
                    color,
                )
            }
            None => hidden(),
        },
    }
}

pub(crate) fn item_row_mouse_hover_system(
    mut mode: ResMut<InputMode>,
    viewport: Res<ItemListViewport>,
    state: Res<SceneState>,
    sort: Res<SortOptions>,
    active_bag: Res<ItemScreenContainer>,
    mut focus: ResMut<ItemMenuFocus>,
    rows: Query<(&Interaction, &ItemListRow), Changed<Interaction>>,
) {
    let Some(screen_mode) = item_screen_mode(&mode) else {
        return;
    };
    if submenu_open(&mode) {
        return;
    }
    let total = list_rows(&state.snapshot, screen_mode, active_bag.0, &sort).len();
    let start = viewport.start;
    let InputMode::Menu(stack) = &mut *mode else {
        return;
    };
    let Some(level) = stack.current_mut() else {
        return;
    };
    for (interaction, row) in &rows {
        if !matches!(interaction, Interaction::Hovered | Interaction::Pressed) {
            continue;
        }
        let list_idx = start + row.0;
        if list_idx < total {
            // Hovering the list returns focus here, mirroring the options box
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
    viewport: Res<ItemListViewport>,
    state: Res<SceneState>,
    sort: Res<SortOptions>,
    active_bag: Res<ItemScreenContainer>,
    rows: Query<(&Interaction, &ItemListRow), Changed<Interaction>>,
    mut out: MessageWriter<MenuRowActivated>,
) {
    let Some(screen_mode) = item_screen_mode(&mode) else {
        return;
    };
    if submenu_open(&mode) {
        return;
    }
    let total = list_rows(&state.snapshot, screen_mode, active_bag.0, &sort).len();
    let start = viewport.start;
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
        set_display(&mut node, strip_visible);
    }
    for (tab, mut node, mut bg) in tab_q.iter_mut() {
        let visible = strip_visible && tab.0 < bags.len();
        set_display(&mut node, visible);
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
    mut mode: ResMut<InputMode>,
    state: Res<SceneState>,
    mut sort: ResMut<SortOptions>,
    mut focus: ResMut<ItemMenuFocus>,
    mut active_bag: ResMut<ItemScreenContainer>,
    mut sort_req: MessageWriter<item_detail::InventorySortRequested>,
    rows: Query<(&Interaction, &ItemText), Changed<Interaction>>,
) {
    if !items_open(&mode) || submenu_open(&mode) {
        return;
    }
    for (interaction, tag) in &rows {
        let ItemRole::OptionsRow(i) = tag.0 else {
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
                if id == SortOptionId::RecycleBin {
                    if open_recycle_bin(&state.snapshot, &mut active_bag, &mut focus) {
                        if let InputMode::Menu(stack) = &mut *mode {
                            if let Some(level) = stack.current_mut() {
                                level.cursor = 0;
                            }
                        }
                    }
                } else if item_detail::apply_sort_option(&mut sort, id) {
                    sort_req.write(item_detail::InventorySortRequested {
                        container: ffxi_proto::map::container::LOC_INVENTORY,
                    });
                }
            }
            Interaction::None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_scrolls_one_row_when_cursor_leaves_the_page() {
        let mut v = ItemListViewport::default();
        let total = 59;
        for cursor in 0..ITEM_LIST_ROWS {
            v.follow(cursor, total);
            assert_eq!(v.start, 0, "cursor {cursor} stays on the first page");
        }
        v.follow(ITEM_LIST_ROWS, total);
        assert_eq!(v.start, 1, "one past the page scrolls by one row");
        v.follow(ITEM_LIST_ROWS - 1, total);
        assert_eq!(v.start, 1, "moving back inside the page leaves it");
        v.follow(0, total);
        assert_eq!(v.start, 0, "moving above the page scrolls back up");
    }

    /// Retail Right/Left (.agents/skills/retail-observe/references/2026-09-11-items-window.md,
    /// 59-item list): from a page-aligned view the next page shows with the
    /// cursor on its top row; after a one-row scroll the page and cursor shift
    /// together so the cursor stays on the bottom row; both clamp at the last
    /// page with no wrap.
    #[test]
    fn paging_shifts_page_and_cursor_together() {
        let total = 59;
        let mut v = ItemListViewport::default();
        let mut cursor = 0;
        v.page(true, total);
        cursor = page_cursor(cursor, total, true);
        assert_eq!((v.start, cursor), (10, 10));

        let mut v = ItemListViewport { start: 1 };
        let mut cursor = 10;
        for expect in [(11, 20), (21, 30), (31, 40), (41, 50), (49, 58), (49, 58)] {
            v.page(true, total);
            cursor = page_cursor(cursor, total, true);
            v.follow(cursor, total);
            assert_eq!((v.start, cursor), expect);
        }
        v.page(false, total);
        cursor = page_cursor(cursor, total, false);
        v.follow(cursor, total);
        assert_eq!((v.start, cursor), (39, 48));
        for _ in 0..6 {
            v.page(false, total);
            cursor = page_cursor(cursor, total, false);
            v.follow(cursor, total);
        }
        assert_eq!((v.start, cursor), (0, 0), "clamped at the first row");
    }

    #[test]
    fn short_lists_never_scroll() {
        let mut v = ItemListViewport { start: 3 };
        v.follow(4, 5);
        assert_eq!(v.start, 0);
        v.page(true, 5);
        assert_eq!(v.start, 0);
        assert_eq!(page_cursor(2, 5, true), 4);
    }

    #[test]
    fn cursor_steps_clamp_at_both_ends() {
        assert_eq!(step_cursor(0, 59, false), 0);
        assert_eq!(step_cursor(5, 59, false), 4);
        assert_eq!(step_cursor(58, 59, true), 58);
        assert_eq!(step_cursor(0, 0, true), 0);
    }

    fn snapshot_with_bags(caps: &[(u8, u16)]) -> kuluu_snapshot::SceneSnapshot {
        kuluu_snapshot::SceneSnapshot {
            containers: caps
                .iter()
                .map(|&(id, capacity)| kuluu_snapshot::ContainerView {
                    id,
                    capacity,
                    items: Vec::new(),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn item_list_level_looks_through_the_item_submenu() {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Items);
        stack.current_mut().unwrap().cursor = 7;
        stack.push(MenuKind::ItemAction {
            container: 0,
            index: 7,
            item_no: 4096,
        });
        let level = item_list_level(&stack).expect("list level under the submenu");
        assert_eq!(level.kind, MenuKind::Items);
        assert_eq!(level.cursor, 7);
        let mode = InputMode::Menu(stack.clone());
        assert_eq!(item_screen_mode(&mode), Some(ItemScreenMode::Inventory));
        assert!(submenu_open(&mode));
        assert_eq!(items_cursor(&mode), 7);

        stack.pop();
        assert!(!submenu_open(&InputMode::Menu(stack.clone())));
        stack.pop();
        assert!(item_list_level(&stack).is_none());
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

    #[test]
    fn recycle_bin_opens_only_when_granted() {
        use ffxi_proto::map::container as c;
        let mut active = ItemScreenContainer(c::LOC_INVENTORY);
        let mut focus = ItemMenuFocus::Sort(SortOptionId::RecycleBin);
        let none = snapshot_with_bags(&[(c::LOC_INVENTORY, 30)]);
        assert!(!open_recycle_bin(&none, &mut active, &mut focus));
        assert_eq!(active.0, c::LOC_INVENTORY);
        assert!(
            focus.sort_focused(),
            "an ungranted bin leaves the box focused"
        );

        let granted = snapshot_with_bags(&[(c::LOC_INVENTORY, 30), (c::LOC_RECYCLEBIN, 10)]);
        assert!(open_recycle_bin(&granted, &mut active, &mut focus));
        assert_eq!(active.0, c::LOC_RECYCLEBIN);
        assert!(!focus.sort_focused());
        assert!(
            !open_recycle_bin(&granted, &mut active, &mut focus),
            "already showing the bin: no cursor reset"
        );
    }

    #[test]
    fn every_option_has_a_label() {
        for &id in SORT_OPTIONS {
            assert!(!option_label(id).is_empty());
        }
    }
}
