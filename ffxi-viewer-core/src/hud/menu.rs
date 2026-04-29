//! Main menu (`-` to open) — vanilla-FFXI-styled vertical list anchored
//! to the right side of the screen.
//!
//! The menu is a *scaffold* in this stage: the entry list is fixed and
//! selecting an entry just emits a `[menu] Magic — not implemented`
//! chat line. Wiring the entries to real spell / ability / item lookups
//! lands in subsequent stages alongside the data mirrors that populate
//! them. The point of this stage is the input plumbing.
//!
//! Look:
//!   - right-anchored, dark backdrop with a thin border,
//!   - `> Magic` style cursor on the selected row (cyan accent),
//!   - other rows in muted text.
//!
//! Visibility is driven by [`InputMode::Menu`]; otherwise `Display::None`.

use bevy::prelude::*;

use crate::hud::palette;
use crate::input_mode::InputMode;

/// Fixed root-level entry labels. Order matches vanilla FFXI's main menu
/// roughly. `text_input::handle_menu_key` reads this list (via
/// [`root_entry_label`] / [`root_entry_count`]) so cursor bounds match
/// what the user actually sees.
const ROOT_ENTRIES: &[&str] = &[
    "Magic",
    "Abilities",
    "Items",
    "Status",
    "Party",
    "Search",
    "Macros",
    "Config",
    "Logout",
];

/// Number of entries on the root menu. Used by the input router to clamp
/// cursor movement.
pub fn root_entry_count() -> usize {
    ROOT_ENTRIES.len()
}

/// Label for a given root-menu cursor index. Out-of-range returns
/// `"<unknown>"` rather than panicking, since the input router clamps
/// cursor bounds independently and a stale index would otherwise crash.
pub fn root_entry_label(idx: usize) -> &'static str {
    ROOT_ENTRIES.get(idx).copied().unwrap_or("<unknown>")
}

/// Marker on the menu root.
#[derive(Component)]
pub struct MainMenu;

/// Marker on each menu row. `slot` is the row index 0..entries.len().
#[derive(Component)]
pub struct MainMenuRow {
    pub slot: usize,
}

pub fn spawn_main_menu(mut commands: Commands) {
    commands
        .spawn((
            MainMenu,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(48.0),
                right: Val::Px(8.0),
                width: Val::Px(160.0),
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
            for (slot, label) in ROOT_ENTRIES.iter().enumerate() {
                p.spawn((
                    MainMenuRow { slot },
                    Text::new(format!("  {label}")),
                    TextFont {
                        font_size: 14.0,
                        ..default()
                    },
                    TextColor(palette::MUTED),
                ));
            }
        });
}

/// Per-frame: toggle visibility, update cursor highlighting.
pub fn update_main_menu(
    mode: Res<InputMode>,
    mut menu_q: Query<&mut Node, With<MainMenu>>,
    mut row_q: Query<(&MainMenuRow, &mut Text, &mut TextColor)>,
) {
    let Ok(mut node) = menu_q.single_mut() else {
        return;
    };

    let cursor: Option<usize> = match &*mode {
        InputMode::Menu(stack) => stack.current().map(|l| l.cursor),
        _ => None,
    };

    match cursor {
        Some(c) => {
            node.display = Display::Flex;
            for (row, mut text, mut color) in row_q.iter_mut() {
                let label = ROOT_ENTRIES.get(row.slot).copied().unwrap_or("");
                let is_cursor = row.slot == c;
                let want = if is_cursor {
                    format!("> {label}")
                } else {
                    format!("  {label}")
                };
                if **text != want {
                    **text = want;
                }
                let want_color = if is_cursor { palette::ACCENT } else { palette::MUTED };
                if color.0 != want_color {
                    color.0 = want_color;
                }
            }
        }
        None => {
            if node.display != Display::None {
                node.display = Display::None;
            }
        }
    }
}
