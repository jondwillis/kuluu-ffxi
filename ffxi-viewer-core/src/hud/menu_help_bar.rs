//! Retail top menu bar (kuluu-5ndh): whenever a menu is open, a bar spans the
//! top of the game window — menu title, an inventory used/capacity counter for
//! item lists, a one-line help string for the highlighted entry, and a "Help"
//! label at the far right (retail capture 2026-07-19, HorizonXI). The counter is
//! bag fill, NOT cursor position.

use bevy::prelude::*;

use crate::hud::menu::{self, DynamicMenu, DynamicMenuAction};
use crate::hud::style::{self, theme};
use crate::input_mode::{InputMode, MenuKind, PassiveCursorFocus};

#[derive(Component)]
pub struct MenuHelpBar;

#[derive(Component)]
pub struct MenuHelpTitle;

#[derive(Component)]
pub struct MenuHelpCounter;

#[derive(Component)]
pub struct MenuHelpHint;

pub const BAR_HEIGHT: f32 = 26.0;
/// Retail frames the active menu title in its own boxed segment, flush left.
const TITLE_BOX_W: f32 = 118.0;

/// Single-line text that truncates instead of wrapping when the bar is narrow.
fn text_nowrap() -> TextLayout {
    TextLayout {
        linebreak: LineBreak::NoWrap,
        ..default()
    }
}

pub fn spawn_menu_help_bar(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            MenuHelpBar,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                height: Val::Px(BAR_HEIGHT),
                border: UiRect::bottom(Val::Px(1.0)),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Stretch,
                display: Display::None,
                ..default()
            },
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::FRAME_EDGE),
            GlobalZIndex(style::WINDOW_Z),
        ))
        .with_children(|p| {
            // Framed title segment (retail draws this as its own box).
            p.spawn((
                Node {
                    width: Val::Px(TITLE_BOX_W),
                    flex_shrink: 0.0,
                    padding: UiRect::axes(Val::Px(8.0), Val::Px(2.0)),
                    border: UiRect::right(Val::Px(1.0)),
                    align_items: AlignItems::Center,
                    ..default()
                },
                BorderColor::all(theme::FRAME_EDGE),
            ))
            .with_children(|seg| {
                seg.spawn((
                    MenuHelpTitle,
                    Text::new(""),
                    text_nowrap(),
                    style::text_font(14.0),
                    TextColor(theme::TITLE),
                ));
            });
            // Counter + help text clip instead of wrapping over the world view.
            p.spawn(Node {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_width: Val::Px(0.0),
                overflow: Overflow::clip_x(),
                align_items: AlignItems::Center,
                column_gap: Val::Px(10.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(2.0)),
                ..default()
            })
            .with_children(|mid| {
                mid.spawn((
                    MenuHelpCounter,
                    Text::new(""),
                    text_nowrap(),
                    style::text_font(12.0),
                    TextColor(theme::MUTED),
                ));
                mid.spawn((
                    MenuHelpHint,
                    Text::new(""),
                    text_nowrap(),
                    style::text_font(14.0),
                    TextColor(theme::TEXT),
                ));
            });
            // "Help" sits just left of the network S/R panel (fixed width,
            // pinned at top-right), so reserve exactly that much space.
            p.spawn(Node {
                flex_shrink: 0.0,
                align_items: AlignItems::Center,
                margin: UiRect::right(Val::Px(crate::hud::network_status::NET_PANEL_W + 8.0)),
                ..default()
            })
            .with_children(|seg| {
                seg.spawn((
                    Text::new("Help"),
                    text_nowrap(),
                    style::text_font(12.0),
                    TextColor(theme::MUTED),
                ));
            });
        });
}

/// One-line help for the highlighted entry. "Use an item." / "Select an item."
/// are retail-confirmed (kuluu-5ndh capture); the rest are provisional
/// retail-style phrasing pending captures (tracked on kuluu-5ndh).
fn root_entry_help(label: &str) -> &'static str {
    match label {
        "Magic" => "Cast magic.",
        "Abilities" => "Use abilities.",
        "Items" => "Use an item.",
        "Key Items" => "Check key items.",
        "Equipment" => "Change equipment.",
        "Status" => "Check current status.",
        "Party" => "Organize your party.",
        "Search" => "Search for players.",
        menu::ROOT_COMMUNICATION => "Communicate with other players.",
        "Macros" => "Edit macros.",
        "Graphics" => "Adjust graphics settings.",
        "Config" => "Change the configuration.",
        menu::ROOT_CURRENT_TIME => "Check the current time.",
        "Debug" => "Toggle debug panels.",
        menu::ROOT_LOG_OUT => "Log out of the game.",
        menu::ROOT_SHUT_DOWN => "Shut down and exit the game.",
        _ => "",
    }
}

fn entry_help(kind: MenuKind, cursor: usize, dynamic: &DynamicMenu) -> String {
    match kind {
        MenuKind::Root => root_entry_help(menu::entry_label(kind, cursor, dynamic)).to_string(),
        MenuKind::Items | MenuKind::UsableItems => "Select an item.".to_string(),
        MenuKind::Magic => "Select a spell.".to_string(),
        MenuKind::Abilities => "Select an ability.".to_string(),
        MenuKind::KeyItems => "Select a key item.".to_string(),
        MenuKind::Equipment | MenuKind::EquipSlot(_) => "Select equipment.".to_string(),
        MenuKind::ItemAction { .. } => match menu::entry_action(kind, cursor, dynamic) {
            Some(DynamicMenuAction::UseItem { .. }) => "Use this item.".to_string(),
            Some(DynamicMenuAction::MoveItem { .. }) => "Move this item.".to_string(),
            _ => "Select an action.".to_string(),
        },
        MenuKind::EmoteList => "Select an emote.".to_string(),
        MenuKind::Status => "Select a category.".to_string(),
        _ => "Select an option.".to_string(),
    }
}

/// Retail shows bag fill (used/capacity, not cursor position) beside the title
/// for item lists (kuluu-5ndh: "20/58" fixed while the cursor scrolled).
fn bag_counter(
    kind: MenuKind,
    snap: &ffxi_viewer_wire::SceneSnapshot,
    active_bag: u8,
) -> Option<(usize, u16)> {
    let container = match kind {
        MenuKind::Items => active_bag,
        MenuKind::UsableItems => ffxi_proto::map::container::LOC_INVENTORY,
        _ => return None,
    };
    snap.container(container)
        .map(|c| (c.items.len(), c.capacity))
}

#[allow(clippy::type_complexity)]
pub fn update_menu_help_bar(
    mode: Res<InputMode>,
    dynamic: Res<DynamicMenu>,
    scene: Res<crate::snapshot::SceneState>,
    active_bag: Res<crate::hud::item_screen::ItemScreenContainer>,
    mut bar_q: Query<&mut Node, With<MenuHelpBar>>,
    mut title_q: Query<
        &mut Text,
        (
            With<MenuHelpTitle>,
            Without<MenuHelpCounter>,
            Without<MenuHelpHint>,
        ),
    >,
    mut counter_q: Query<
        &mut Text,
        (
            With<MenuHelpCounter>,
            Without<MenuHelpTitle>,
            Without<MenuHelpHint>,
        ),
    >,
    mut hint_q: Query<
        &mut Text,
        (
            With<MenuHelpHint>,
            Without<MenuHelpTitle>,
            Without<MenuHelpCounter>,
        ),
    >,
) {
    let Ok(mut node) = bar_q.single_mut() else {
        return;
    };

    let content = match &*mode {
        InputMode::Menu(stack) => stack.current().map(|l| BarContent {
            title: menu::menu_title(l.kind).to_string(),
            counter: match bag_counter(l.kind, &scene.snapshot, active_bag.0) {
                Some((used, capacity)) => format!("{used}/{capacity}"),
                None => String::new(),
            },
            hint: entry_help(l.kind, l.cursor, &dynamic),
        }),
        InputMode::PassiveCursor(s) if s.focus == PassiveCursorFocus::StatusIcons => {
            buff_bar_content(&scene.snapshot, s.status_cursor)
        }
        _ => None,
    };

    let Some(content) = content else {
        if node.display != Display::None {
            node.display = Display::None;
        }
        return;
    };
    if node.display != Display::Flex {
        node.display = Display::Flex;
    }

    if let Ok(mut text) = title_q.single_mut() {
        if **text != content.title {
            **text = content.title;
        }
    }

    if let Ok(mut text) = counter_q.single_mut() {
        if **text != content.counter {
            **text = content.counter;
        }
    }

    if let Ok(mut text) = hint_q.single_mut() {
        if **text != content.hint {
            **text = content.hint;
        }
    }
}

struct BarContent {
    title: String,
    counter: String,
    hint: String,
}

/// Retail's status window names the highlighted buff in the top bar. We mirror
/// it while the F-cycle cursor sits on the ribbon: title "Status", a
/// position/total counter, and the buff's name (from the scraped effect table)
/// as the help line. Returns `None` when the player has no active effects.
fn buff_bar_content(snap: &ffxi_viewer_wire::SceneSnapshot, cursor: usize) -> Option<BarContent> {
    let icons = &snap.status_icons;
    if icons.is_empty() {
        return None;
    }
    let cursor = cursor.min(icons.len() - 1);
    let icon = icons[cursor];
    let name = ffxi_proto::status_names::lookup(icon).unwrap_or("(Unknown effect)");
    Some(BarContent {
        title: menu::menu_title(MenuKind::Status).to_string(),
        counter: format!("{}/{}", cursor + 1, icons.len()),
        hint: name.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_help_covers_every_root_entry() {
        // A root entry without a help line renders an empty bar segment —
        // catch new entries that forget to add one.
        let dynamic = DynamicMenu::default();
        for idx in 0..menu::entry_count(MenuKind::Root, &dynamic) {
            let label = menu::entry_label(MenuKind::Root, idx, &dynamic);
            assert!(
                !root_entry_help(label).is_empty(),
                "root entry {label:?} has no help line"
            );
        }
    }

    #[test]
    fn item_lists_count_bag_fill_not_cursor() {
        use ffxi_viewer_wire::{ContainerView, InventoryItem, SceneSnapshot};
        let inv = ffxi_proto::map::container::LOC_INVENTORY;
        let item = |index: u8, item_no: u16| InventoryItem {
            container: inv,
            index,
            item_no,
            quantity: 1,
            locked: false,
        };
        let snap = SceneSnapshot {
            containers: vec![ContainerView {
                id: inv,
                capacity: 58,
                items: vec![item(1, 4096), item(2, 4097)],
            }],
            ..Default::default()
        };
        assert_eq!(bag_counter(MenuKind::Items, &snap, inv), Some((2, 58)));
        assert_eq!(
            bag_counter(MenuKind::UsableItems, &snap, inv),
            Some((2, 58))
        );
        assert_eq!(bag_counter(MenuKind::Root, &snap, inv), None);
    }

    #[test]
    fn buff_bar_names_the_cursor_slot() {
        use ffxi_viewer_wire::SceneSnapshot;
        // 40 = Protect, 41 = Shell (status_names scrape).
        let snap = SceneSnapshot {
            status_icons: vec![40, 41],
            ..Default::default()
        };
        let c = buff_bar_content(&snap, 1).expect("has effects");
        assert_eq!(c.title, "Status");
        assert_eq!(c.counter, "2/2");
        assert_eq!(c.hint, "Shell");
    }

    #[test]
    fn ribbon_focus_drives_the_live_bar_system() {
        use crate::hud::item_screen::ItemScreenContainer;
        use crate::input_mode::{PassiveCursorFocus, PassiveCursorState};
        use crate::snapshot::SceneState;
        use ffxi_viewer_wire::SceneSnapshot;

        let mut app = App::new();
        app.init_resource::<InputMode>()
            .init_resource::<DynamicMenu>()
            .init_resource::<ItemScreenContainer>();

        let mut scene = SceneState::default();
        scene.snapshot = SceneSnapshot {
            status_icons: vec![40, 41],
            ..Default::default()
        };
        app.insert_resource(scene);
        app.insert_resource(InputMode::PassiveCursor(PassiveCursorState {
            focus: PassiveCursorFocus::StatusIcons,
            status_cursor: 1,
            chat_expanded: false,
        }));

        app.add_systems(Startup, spawn_menu_help_bar);
        app.add_systems(Update, update_menu_help_bar);
        app.update();

        // The bar unhides and its title/counter/hint reflect the highlighted buff.
        let bar_shown = app
            .world_mut()
            .query_filtered::<&Node, With<MenuHelpBar>>()
            .single(app.world())
            .map(|n| n.display == Display::Flex)
            .unwrap();
        assert!(
            bar_shown,
            "bar must be visible while ribbon holds the cursor"
        );

        let title = app
            .world_mut()
            .query_filtered::<&Text, With<MenuHelpTitle>>()
            .single(app.world())
            .unwrap()
            .0
            .clone();
        let hint = app
            .world_mut()
            .query_filtered::<&Text, With<MenuHelpHint>>()
            .single(app.world())
            .unwrap()
            .0
            .clone();
        assert_eq!(title, "Status");
        assert_eq!(hint, "Shell", "cursor 1 names the second effect");
    }

    #[test]
    fn buff_bar_clamps_stale_cursor_and_hides_when_empty() {
        use ffxi_viewer_wire::SceneSnapshot;
        let snap = SceneSnapshot {
            status_icons: vec![40],
            ..Default::default()
        };
        // Cursor past the end (e.g. a buff expired) clamps to the last slot.
        assert_eq!(buff_bar_content(&snap, 9).unwrap().hint, "Protect");
        let empty = SceneSnapshot::default();
        assert!(buff_bar_content(&empty, 0).is_none());
    }
}
