//! Retail top menu bar (kuluu-5ndh): whenever a menu is open, a bar spans the
//! top of the game window — menu title, an inventory used/capacity counter for
//! item lists, a one-line help string for the highlighted entry, and a "Help"
//! label at the far right (retail capture 2026-07-19, HorizonXI). The counter is
//! bag fill, NOT cursor position.

use bevy::prelude::*;

use crate::hud::menu::{self, DynamicMenu, DynamicMenuAction};
use crate::hud::style::{self, theme};
use crate::input_mode::{InputMode, MenuKind};

#[derive(Component)]
pub struct MenuHelpBar;

#[derive(Component)]
pub struct MenuHelpTitle;

#[derive(Component)]
pub struct MenuHelpCounter;

#[derive(Component)]
pub struct MenuHelpHint;

const BAR_HEIGHT: f32 = 26.0;
// Keeps the "Help" label clear of the network S/R panel pinned at top-right.
const HELP_RIGHT_MARGIN: f32 = 150.0;

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
                padding: UiRect::axes(Val::Px(10.0), Val::Px(2.0)),
                border: UiRect::bottom(Val::Px(1.0)),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(10.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::FRAME_EDGE),
            GlobalZIndex(style::WINDOW_Z),
        ))
        .with_children(|p| {
            p.spawn((
                MenuHelpTitle,
                Text::new(""),
                style::text_font(14.0),
                TextColor(theme::TITLE),
            ));
            p.spawn((
                MenuHelpCounter,
                Text::new(""),
                style::text_font(12.0),
                TextColor(theme::MUTED),
            ));
            p.spawn((
                MenuHelpHint,
                Text::new(""),
                style::text_font(14.0),
                TextColor(theme::TEXT),
            ));
            p.spawn(Node {
                flex_grow: 1.0,
                ..default()
            });
            p.spawn((
                Text::new("Help"),
                style::text_font(12.0),
                TextColor(theme::MUTED),
                Node {
                    margin: UiRect::right(Val::Px(HELP_RIGHT_MARGIN)),
                    ..default()
                },
            ));
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

    let active = match &*mode {
        InputMode::Menu(stack) => stack.current().map(|l| (l.kind, l.cursor)),
        _ => None,
    };

    let Some((kind, cursor)) = active else {
        if node.display != Display::None {
            node.display = Display::None;
        }
        return;
    };
    if node.display != Display::Flex {
        node.display = Display::Flex;
    }

    if let Ok(mut text) = title_q.single_mut() {
        let want = menu::menu_title(kind);
        if **text != *want {
            **text = want.to_string();
        }
    }

    if let Ok(mut text) = counter_q.single_mut() {
        let want = match bag_counter(kind, &scene.snapshot, active_bag.0) {
            Some((used, capacity)) => format!("{used}/{capacity}"),
            None => String::new(),
        };
        if **text != want {
            **text = want;
        }
    }

    if let Ok(mut text) = hint_q.single_mut() {
        let want = entry_help(kind, cursor, &dynamic);
        if **text != want {
            **text = want;
        }
    }
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
}
