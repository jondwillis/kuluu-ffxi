//! Character-select screen.
//!
//! Renders a vertical list of `Button` nodes, one per character slot.
//! Click → set [`SelectedChar`] → transition to `ConnectInFlight`.
//! Up/Down + Enter keyboard navigation also works (helpful when you
//! have a default char_name passed in but want to confirm).

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;

use super::{CharListData, DefaultCharName, LauncherState, SelectedChar};

#[derive(Component)]
pub(super) struct CharListRoot;

/// Index of the slot this button represents in `CharListData.0`.
#[derive(Component)]
pub(super) struct CharSlotIndex(pub usize);

/// Marker for the keyboard-cursor indicator text.
#[derive(Component)]
pub(super) struct CharRowText {
    pub index: usize,
}

/// Tracks which row the keyboard cursor is on.
#[derive(Resource, Default)]
pub(super) struct CharCursor(pub usize);

pub(super) fn spawn_char_list_ui(
    mut commands: Commands,
    chars: Res<CharListData>,
    default_name: Res<DefaultCharName>,
) {
    // Seed the cursor on whichever row matches the CLI default, falling
    // back to row 0.
    let initial_cursor = default_name
        .0
        .as_deref()
        .and_then(|want| chars.0.iter().position(|c| c.name == want))
        .unwrap_or(0);
    commands.insert_resource(CharCursor(initial_cursor));

    commands
        .spawn((
            CharListRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(8.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.04, 0.04, 0.05)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("Select character"),
                TextFont {
                    font_size: 22.0,
                    ..default()
                },
                TextColor(Color::srgb(0.0, 1.0, 1.0)),
            ));

            if chars.0.is_empty() {
                parent.spawn((
                    Text::new("No characters on this account."),
                    TextFont {
                        font_size: 14.0,
                        ..default()
                    },
                    TextColor(Color::srgb(0.85, 0.20, 0.20)),
                ));
                parent.spawn((
                    Text::new(
                        "Create / delete are not yet implemented in the GUI launcher.\n\
                         Use the stdin launcher (`ffxi-client play`) for now, or close the window.",
                    ),
                    TextFont {
                        font_size: 12.0,
                        ..default()
                    },
                    TextColor(Color::srgb(0.55, 0.55, 0.55)),
                ));
                return;
            }

            for (idx, slot) in chars.0.iter().enumerate() {
                parent
                    .spawn((
                        Button,
                        CharSlotIndex(idx),
                        Node {
                            width: Val::Px(360.0),
                            padding: UiRect::all(Val::Px(8.0)),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            border: UiRect::all(Val::Px(1.0)),
                            ..default()
                        },
                        BorderColor::all(Color::srgb(0.30, 0.30, 0.30)),
                        BackgroundColor(Color::srgb(0.10, 0.10, 0.12)),
                    ))
                    .with_children(|row| {
                        row.spawn((
                            CharRowText { index: idx },
                            Text::new(format_row(idx, slot, idx == initial_cursor)),
                            TextFont {
                                font_size: 16.0,
                                ..default()
                            },
                            TextColor(Color::srgb(0.95, 0.95, 0.95)),
                        ));
                    });
            }

            parent.spawn((
                Text::new("Click or use ↑/↓ + Enter   Esc: back to login"),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgb(0.5, 0.5, 0.5)),
            ));
        });
}

pub(super) fn despawn_char_list_ui(
    mut commands: Commands,
    q: Query<Entity, With<CharListRoot>>,
) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
    commands.remove_resource::<CharCursor>();
}

pub(super) fn handle_click_system(
    mut interactions: Query<(&Interaction, &CharSlotIndex), Changed<Interaction>>,
    chars: Res<CharListData>,
    mut sel: ResMut<SelectedChar>,
    mut next_state: ResMut<NextState<LauncherState>>,
) {
    for (interaction, idx) in interactions.iter_mut() {
        if *interaction == Interaction::Pressed {
            if let Some(slot) = chars.0.get(idx.0).cloned() {
                sel.0 = Some(slot);
                next_state.set(LauncherState::ConnectInFlight);
            }
        }
    }
}

pub(super) fn handle_keyboard_system(
    mut events: MessageReader<KeyboardInput>,
    chars: Res<CharListData>,
    mut cursor: ResMut<CharCursor>,
    mut sel: ResMut<SelectedChar>,
    mut next_state: ResMut<NextState<LauncherState>>,
    mut q_rows: Query<(&CharRowText, &mut Text)>,
) {
    let n = chars.0.len();
    let mut moved = false;
    let mut chosen = false;
    let mut go_back = false;
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &ev.logical_key {
            Key::ArrowUp => {
                if n > 0 {
                    cursor.0 = (cursor.0 + n - 1) % n;
                    moved = true;
                }
            }
            Key::ArrowDown => {
                if n > 0 {
                    cursor.0 = (cursor.0 + 1) % n;
                    moved = true;
                }
            }
            Key::Enter => {
                if n > 0 {
                    chosen = true;
                }
            }
            Key::Escape => {
                go_back = true;
            }
            _ => {}
        }
    }
    if go_back {
        next_state.set(LauncherState::Login);
        return;
    }
    if chosen {
        if let Some(slot) = chars.0.get(cursor.0).cloned() {
            sel.0 = Some(slot);
            next_state.set(LauncherState::ConnectInFlight);
            return;
        }
    }
    if moved {
        for (row, mut text) in q_rows.iter_mut() {
            if let Some(slot) = chars.0.get(row.index) {
                **text = format_row(row.index, slot, row.index == cursor.0);
            }
        }
    }
}

fn format_row(idx: usize, slot: &ffxi_client::lobby_client::CharSlot, focused: bool) -> String {
    let marker = if focused { ">" } else { " " };
    format!(
        "{marker} [{n}] {name}  (charid {id})",
        n = idx + 1,
        name = slot.name,
        id = slot.char_id
    )
}
