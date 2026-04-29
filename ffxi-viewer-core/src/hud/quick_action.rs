//! Quick-action picker (Enter from World mode, no target selected).
//!
//! Same FFXI-classic styling as the main menu but smaller and centered —
//! it's a fast picker, dismissed in a key or two, so it shouldn't hide
//! the world view. Like the menu it's a *scaffold* this stage:
//! selecting an entry just emits a chat-line stub.

use bevy::prelude::*;

use crate::hud::palette;
use crate::input_mode::InputMode;

/// Fixed quick-action labels. Keep this list short — the picker is
/// supposed to be one or two keystrokes total.
const ENTRIES: &[&str] = &["Magic", "Check", "Items", "Macros", "Menu"];

pub fn entry_count() -> usize {
    ENTRIES.len()
}

pub fn entry_label(idx: usize) -> &'static str {
    ENTRIES.get(idx).copied().unwrap_or("<unknown>")
}

#[derive(Component)]
pub struct QuickActionPanel;

#[derive(Component)]
pub struct QuickActionRow {
    pub slot: usize,
}

pub fn spawn_quick_action(mut commands: Commands) {
    commands
        .spawn((
            QuickActionPanel,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Percent(40.0),
                left: Val::Percent(50.0),
                width: Val::Px(140.0),
                margin: UiRect::left(Val::Px(-70.0)),
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
            for (slot, label) in ENTRIES.iter().enumerate() {
                p.spawn((
                    QuickActionRow { slot },
                    Text::new(format!("  {label}")),
                    TextFont {
                        font_size: 13.0,
                        ..default()
                    },
                    TextColor(palette::MUTED),
                ));
            }
        });
}

pub fn update_quick_action(
    mode: Res<InputMode>,
    mut panel_q: Query<&mut Node, With<QuickActionPanel>>,
    mut row_q: Query<(&QuickActionRow, &mut Text, &mut TextColor)>,
) {
    let Ok(mut node) = panel_q.single_mut() else {
        return;
    };

    let cursor: Option<usize> = match &*mode {
        InputMode::QuickAction(state) => Some(state.cursor),
        _ => None,
    };

    match cursor {
        Some(c) => {
            node.display = Display::Flex;
            for (row, mut text, mut color) in row_q.iter_mut() {
                let label = ENTRIES.get(row.slot).copied().unwrap_or("");
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
