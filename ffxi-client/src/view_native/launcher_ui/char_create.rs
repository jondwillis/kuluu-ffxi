//! Character-creation screen.
//!
//! Form layout: a column of named rows. The focused row is highlighted
//! and accepts value changes via Left/Right (cycle enum) or typed chars
//! (name field). Tab / Shift-Tab moves focus. Enter submits. Esc returns
//! to CharList.
//!
//! Live spec validation happens client-side (length, char class, ranges)
//! mirroring `vendor/server/src/login/login_helpers.cpp:216` so the user
//! sees errors immediately rather than after a server round-trip.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;

use super::{CharCreateForm, CharCreateField, CharCreateError, LauncherState};

/// Race options. Indices map directly to LSB's race id 1..=8
/// (see `vendor/server/src/login/login_helpers.cpp:228`).
pub(super) const RACES: &[(u8, &str)] = &[
    (1, "Hume M"),
    (2, "Hume F"),
    (3, "Elvaan M"),
    (4, "Elvaan F"),
    (5, "Tarutaru M"),
    (6, "Tarutaru F"),
    (7, "Mithra (F)"),
    (8, "Galka (M)"),
];

/// Starting jobs only — server clamps to 1..=6
/// (see `vendor/server/src/login/login_helpers.cpp:248`).
pub(super) const JOBS: &[(u8, &str)] = &[
    (1, "Warrior"),
    (2, "Monk"),
    (3, "White Mage"),
    (4, "Black Mage"),
    (5, "Red Mage"),
    (6, "Thief"),
];

/// Nation determines starting zone — 0..=2 valid
/// (see `vendor/server/src/login/login_helpers.cpp:261`).
pub(super) const NATIONS: &[(u8, &str)] = &[
    (0, "San d'Oria"),
    (1, "Bastok"),
    (2, "Windurst"),
];

/// Body size — 0..=2 (see `vendor/server/src/login/login_helpers.cpp:234`).
pub(super) const SIZES: &[(u8, &str)] = &[
    (0, "Small"),
    (1, "Medium"),
    (2, "Large"),
];

/// 16 faces (0..=15) — server enforces upper bound at
/// `vendor/server/src/login/login_helpers.cpp:240`.
pub(super) const FACE_MAX: u8 = 15;

#[derive(Component)]
pub(super) struct CharCreateRoot;

/// Marker so `redraw_form_system` can find each row's value text.
#[derive(Component)]
pub(super) struct RowText {
    pub field: CharCreateField,
}

/// Marker for the validation/error footer line.
#[derive(Component)]
pub(super) struct StatusText;

pub(super) fn spawn_ui(mut commands: Commands, form: Res<CharCreateForm>) {
    commands
        .spawn((
            CharCreateRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(10.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.04, 0.04, 0.05)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("Create character"),
                TextFont {
                    font_size: 24.0,
                    ..default()
                },
                TextColor(Color::srgb(0.0, 1.0, 1.0)),
            ));
            parent.spawn((
                Text::new(
                    "Tab / Shift-Tab: switch field   ◀ ▶ : cycle value   Enter: create   Esc: back",
                ),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(Color::srgb(0.55, 0.55, 0.55)),
            ));

            for field in [
                CharCreateField::Name,
                CharCreateField::Race,
                CharCreateField::Job,
                CharCreateField::Nation,
                CharCreateField::Face,
                CharCreateField::Size,
            ] {
                parent.spawn((
                    RowText { field },
                    Text::new(format_row(&form, field)),
                    TextFont {
                        font_size: 17.0,
                        ..default()
                    },
                    TextColor(Color::srgb(0.95, 0.95, 0.95)),
                ));
            }

            parent.spawn((
                StatusText,
                Text::new(form.validation_msg().unwrap_or_default()),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(Color::srgb(0.95, 0.55, 0.30)),
            ));
        });
}

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<CharCreateRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    mut form: ResMut<CharCreateForm>,
    mut next_state: ResMut<NextState<LauncherState>>,
    mut error: ResMut<CharCreateError>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &ev.logical_key {
            Key::Escape => {
                error.0.clear();
                next_state.set(LauncherState::CharList);
                return;
            }
            Key::Enter => {
                if form.validation_msg().is_none() {
                    error.0.clear();
                    next_state.set(LauncherState::CharCreateInFlight);
                    return;
                }
            }
            Key::Tab => {
                form.focus = if shift {
                    form.focus.prev()
                } else {
                    form.focus.next()
                };
            }
            Key::ArrowLeft => form.cycle_focused(-1),
            Key::ArrowRight => form.cycle_focused(1),
            Key::Backspace => {
                if form.focus == CharCreateField::Name {
                    form.name.pop();
                }
            }
            Key::Character(s) => {
                if form.focus == CharCreateField::Name {
                    for c in s.chars() {
                        // Server enforces alpha-only (login_helpers.cpp:220).
                        // Reject early so the user can't even type punctuation.
                        if c.is_ascii_alphabetic() && form.name.len() < 15 {
                            form.name.push(c);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

pub(super) fn redraw_form_system(
    form: Res<CharCreateForm>,
    mut q_rows: Query<(&RowText, &mut Text, &mut TextColor), Without<StatusText>>,
    mut q_status: Query<&mut Text, With<StatusText>>,
) {
    if !form.is_changed() {
        return;
    }
    for (row, mut text, mut color) in q_rows.iter_mut() {
        **text = format_row(&form, row.field);
        *color = if row.field == form.focus {
            TextColor(Color::srgb(0.30, 0.85, 1.00))
        } else {
            TextColor(Color::srgb(0.90, 0.90, 0.90))
        };
    }
    for mut t in q_status.iter_mut() {
        **t = form.validation_msg().unwrap_or_default();
    }
}

fn format_row(form: &CharCreateForm, field: CharCreateField) -> String {
    let focused = form.focus == field;
    let marker = if focused { "▶ " } else { "  " };
    match field {
        CharCreateField::Name => {
            let cursor = if focused { "_" } else { " " };
            format!("{marker}Name:    {name}{cursor}", name = form.name)
        }
        CharCreateField::Race => {
            format!("{marker}Race:    ◀ {} ▶", lookup(RACES, form.race))
        }
        CharCreateField::Job => {
            format!("{marker}Job:     ◀ {} ▶", lookup(JOBS, form.job))
        }
        CharCreateField::Nation => {
            format!("{marker}Nation:  ◀ {} ▶", lookup(NATIONS, form.nation))
        }
        CharCreateField::Face => {
            format!(
                "{marker}Face:    ◀ {:>2} / {} ▶",
                form.face, FACE_MAX
            )
        }
        CharCreateField::Size => {
            format!("{marker}Build:   ◀ {} ▶", lookup(SIZES, form.size))
        }
    }
}

fn lookup<'a>(table: &'a [(u8, &'a str)], val: u8) -> &'a str {
    table
        .iter()
        .find(|(v, _)| *v == val)
        .map(|(_, name)| *name)
        .unwrap_or("?")
}

// --- CharCreateError state ------------------------------------------------

#[derive(Component)]
pub(super) struct CharCreateErrorRoot;

pub(super) fn spawn_error_ui(mut commands: Commands, err: Res<CharCreateError>) {
    commands
        .spawn((
            CharCreateErrorRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(16.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.04, 0.04, 0.05)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("Character creation failed"),
                TextFont {
                    font_size: 22.0,
                    ..default()
                },
                TextColor(Color::srgb(0.95, 0.30, 0.30)),
            ));
            parent.spawn((
                Text::new(err.0.clone()),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(Color::srgb(0.85, 0.85, 0.85)),
            ));
            parent.spawn((
                Text::new("Esc: back to form   Enter: try again"),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(Color::srgb(0.55, 0.55, 0.55)),
            ));
        });
}

pub(super) fn despawn_error_ui(
    mut commands: Commands,
    q: Query<Entity, With<CharCreateErrorRoot>>,
) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

pub(super) fn error_keyboard_system(
    mut events: MessageReader<KeyboardInput>,
    mut next_state: ResMut<NextState<LauncherState>>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &ev.logical_key {
            Key::Enter => {
                next_state.set(LauncherState::CharCreateInFlight);
                return;
            }
            Key::Escape => {
                next_state.set(LauncherState::CharCreate);
                return;
            }
            _ => {}
        }
    }
}
