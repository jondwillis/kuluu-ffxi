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

/// Tag identifying what a row dispatches to when activated. `Char(i)`
/// selects the character at index `i` in [`CharListData`]; `NewChar`
/// opens the creation form.
#[derive(Component, Clone, Copy)]
pub(super) enum RowAction {
    Char(usize),
    NewChar,
}

/// Marker for the keyboard-cursor indicator text. `row_count_total` is
/// the total number of selectable rows (chars + 1 for "new char") so
/// the cursor knows how to wrap.
#[derive(Component)]
pub(super) struct CharRowText {
    pub index: usize,
}

/// Tracks which row the keyboard cursor is on. Range: 0..=chars.len()
/// (the last index is the "+ New character" row).
#[derive(Resource, Default)]
pub(super) struct CharCursor(pub usize);

pub(super) fn spawn_char_list_ui(
    mut commands: Commands,
    chars: Res<CharListData>,
    default_name: Res<DefaultCharName>,
) {
    let new_char_index = chars.0.len();
    // Seed the cursor on whichever row matches the CLI default. Default
    // to row 0 when chars exist, or the "new char" row when the account
    // has none — gets the user to the relevant action immediately.
    let initial_cursor = default_name
        .0
        .as_deref()
        .and_then(|want| chars.0.iter().position(|c| c.name == want))
        .unwrap_or_else(|| if chars.0.is_empty() { new_char_index } else { 0 });
    commands.insert_resource(CharCursor(initial_cursor));

    // No opaque BackgroundColor — the 3D character preview scene
    // (spawned in `char_preview::spawn_preview`) renders behind the
    // launcher UI and would be hidden by a solid fill. The buttons
    // and labels keep their own backgrounds so they remain
    // readable.
    commands
        .spawn((
            CharListRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                // Right-align so the centered 3D character preview
                // (anchored at world (0, 0, 0)) doesn't get hidden
                // by the button column on its left.
                align_items: AlignItems::FlexEnd,
                row_gap: Val::Px(8.0),
                padding: UiRect::right(Val::Px(40.0)),
                ..default()
            },
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
                    Text::new("No characters on this account yet."),
                    TextFont {
                        font_size: 13.0,
                        ..default()
                    },
                    TextColor(Color::srgb(0.65, 0.65, 0.65)),
                ));
            }

            for (idx, slot) in chars.0.iter().enumerate() {
                parent
                    .spawn((
                        Button,
                        RowAction::Char(idx),
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
                            Text::new(format_char_row(idx, slot, idx == initial_cursor)),
                            TextFont {
                                font_size: 16.0,
                                ..default()
                            },
                            TextColor(Color::srgb(0.95, 0.95, 0.95)),
                        ));
                    });
            }

            // Always-present "+ New character" row at the bottom.
            parent
                .spawn((
                    Button,
                    RowAction::NewChar,
                    Node {
                        width: Val::Px(360.0),
                        padding: UiRect::all(Val::Px(8.0)),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        border: UiRect::all(Val::Px(1.0)),
                        ..default()
                    },
                    BorderColor::all(Color::srgb(0.30, 0.55, 0.30)),
                    BackgroundColor(Color::srgb(0.08, 0.12, 0.08)),
                ))
                .with_children(|row| {
                    row.spawn((
                        CharRowText {
                            index: new_char_index,
                        },
                        Text::new(format_new_char_row(new_char_index == initial_cursor)),
                        TextFont {
                            font_size: 16.0,
                            ..default()
                        },
                        TextColor(Color::srgb(0.65, 0.95, 0.65)),
                    ));
                });

            parent.spawn((
                Text::new("Click or use ↑/↓ + Enter   N: new character   Esc: back to login"),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgb(0.5, 0.5, 0.5)),
            ));
        });
}

pub(super) fn despawn_char_list_ui(mut commands: Commands, q: Query<Entity, With<CharListRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
    commands.remove_resource::<CharCursor>();
}

pub(super) fn handle_click_system(
    mut interactions: Query<(&Interaction, &RowAction), Changed<Interaction>>,
    chars: Res<CharListData>,
    mut sel: ResMut<SelectedChar>,
    mut cursor: ResMut<CharCursor>,
    mut next_state: ResMut<NextState<LauncherState>>,
    mut q_rows: Query<(&CharRowText, &mut Text)>,
) {
    let new_char_index = chars.0.len();
    let mut moved = false;
    for (interaction, action) in interactions.iter_mut() {
        // Hover-to-preview: any time the pointer enters a row, move
        // the cursor onto it. The `refresh_preview_on_cursor_change`
        // system then despawns the old 3D preview and respawns the
        // hovered character. Doesn't commit selection — that still
        // requires a click.
        if *interaction == Interaction::Hovered {
            let target_idx = match *action {
                RowAction::Char(idx) => idx,
                RowAction::NewChar => new_char_index,
            };
            if cursor.0 != target_idx {
                cursor.0 = target_idx;
                moved = true;
            }
            continue;
        }
        if *interaction != Interaction::Pressed {
            continue;
        }
        match *action {
            RowAction::Char(idx) => {
                if let Some(slot) = chars.0.get(idx).cloned() {
                    sel.0 = Some(slot);
                    next_state.set(LauncherState::ConnectInFlight);
                }
            }
            RowAction::NewChar => {
                next_state.set(LauncherState::CharCreate);
            }
        }
    }
    if moved {
        // Repaint the row labels' `>` indicator to follow the hover.
        // Mirrors the same loop in `handle_keyboard_system` after an
        // arrow-key move; factored inline rather than into a helper
        // because passing `&mut Query` around in Bevy is fiddly.
        for (row, mut text) in q_rows.iter_mut() {
            if row.index == new_char_index {
                **text = format_new_char_row(row.index == cursor.0);
            } else if let Some(slot) = chars.0.get(row.index) {
                **text = format_char_row(row.index, slot, row.index == cursor.0);
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
    // n = chars.len() + 1; the last index is the "+ New character" row.
    let new_char_index = chars.0.len();
    let n = new_char_index + 1;
    let mut moved = false;
    let mut chosen = false;
    let mut go_back = false;
    let mut new_char_shortcut = false;
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &ev.logical_key {
            Key::ArrowUp => {
                cursor.0 = (cursor.0 + n - 1) % n;
                moved = true;
            }
            Key::ArrowDown => {
                cursor.0 = (cursor.0 + 1) % n;
                moved = true;
            }
            Key::Enter => {
                chosen = true;
            }
            Key::Character(s) if s.eq_ignore_ascii_case("n") => {
                new_char_shortcut = true;
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
    if new_char_shortcut {
        next_state.set(LauncherState::CharCreate);
        return;
    }
    if chosen {
        if cursor.0 == new_char_index {
            next_state.set(LauncherState::CharCreate);
            return;
        }
        if let Some(slot) = chars.0.get(cursor.0).cloned() {
            sel.0 = Some(slot);
            next_state.set(LauncherState::ConnectInFlight);
            return;
        }
    }
    if moved {
        for (row, mut text) in q_rows.iter_mut() {
            if row.index == new_char_index {
                **text = format_new_char_row(row.index == cursor.0);
            } else if let Some(slot) = chars.0.get(row.index) {
                **text = format_char_row(row.index, slot, row.index == cursor.0);
            }
        }
    }
}

fn format_char_row(
    idx: usize,
    slot: &ffxi_client::lobby_client::CharSlot,
    focused: bool,
) -> String {
    let marker = if focused { ">" } else { " " };
    format!(
        "{marker} [{n}] {name}  (charid {id})",
        n = idx + 1,
        name = slot.name,
        id = slot.char_id
    )
}

fn format_new_char_row(focused: bool) -> String {
    let marker = if focused { ">" } else { " " };
    format!("{marker}  + New character")
}
