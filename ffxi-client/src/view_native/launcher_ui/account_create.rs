//! Account-creation screen.
//!
//! Three-field form: username, password, confirm-password. Submit calls
//! `auth.ensure_account(user, pass)` against LSB's connect server. The
//! server's `ACCOUNT_CREATION` setting must be true (it is by default
//! in dev — see `settings/default/login.lua` in `vendor/server`).
//!
//! Form structure mirrors `char_create.rs`: row-per-field, focused row
//! highlighted, Tab/Shift-Tab moves focus, Enter submits (gated by live
//! validation), Esc returns to Login.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;

use super::{
    CreateAccountErrorMsg, CreateAccountField, CreateAccountForm, LauncherState,
};

#[derive(Component)]
pub(super) struct CreateAccountRoot;

#[derive(Component)]
pub(super) struct RowText {
    pub field: CreateAccountField,
}

#[derive(Component)]
pub(super) struct StatusText;

pub(super) fn spawn_ui(mut commands: Commands, form: Res<CreateAccountForm>) {
    commands
        .spawn((
            CreateAccountRoot,
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
                Text::new("Create account"),
                TextFont {
                    font_size: 24.0,
                    ..default()
                },
                TextColor(Color::srgb(0.30, 1.0, 0.65)),
            ));
            parent.spawn((
                Text::new(
                    "Tab / Shift-Tab: switch field   Enter: create   Esc: back to login",
                ),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(Color::srgb(0.55, 0.55, 0.55)),
            ));

            for field in [
                CreateAccountField::User,
                CreateAccountField::Password,
                CreateAccountField::PasswordConfirm,
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

pub(super) fn despawn_ui(
    mut commands: Commands,
    q: Query<Entity, With<CreateAccountRoot>>,
    mut form: ResMut<CreateAccountForm>,
) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
    // Don't carry pass_confirm across state transitions — only `user`
    // and `pass` flow forward via the success path (the async task
    // stashes them into LoginForm). pass_confirm is ephemeral.
    form.pass_confirm.clear();
}

pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    mut form: ResMut<CreateAccountForm>,
    mut next_state: ResMut<NextState<LauncherState>>,
    mut err: ResMut<CreateAccountErrorMsg>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &ev.logical_key {
            Key::Escape => {
                err.0.clear();
                next_state.set(LauncherState::Login);
                return;
            }
            Key::Enter => {
                if form.validation_msg().is_none() {
                    err.0.clear();
                    next_state.set(LauncherState::CreateAccountInFlight);
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
            Key::Backspace => match form.focus {
                CreateAccountField::User => {
                    form.user.pop();
                }
                CreateAccountField::Password => {
                    form.pass.pop();
                }
                CreateAccountField::PasswordConfirm => {
                    form.pass_confirm.pop();
                }
            },
            Key::Character(s) => {
                for c in s.chars() {
                    if c.is_control() {
                        continue;
                    }
                    match form.focus {
                        CreateAccountField::User => form.user.push(c),
                        CreateAccountField::Password => form.pass.push(c),
                        CreateAccountField::PasswordConfirm => form.pass_confirm.push(c),
                    }
                }
            }
            Key::Space => match form.focus {
                CreateAccountField::User => form.user.push(' '),
                CreateAccountField::Password => form.pass.push(' '),
                CreateAccountField::PasswordConfirm => form.pass_confirm.push(' '),
            },
            _ => {}
        }
    }
}

pub(super) fn redraw_form_system(
    form: Res<CreateAccountForm>,
    mut q_rows: Query<(&RowText, &mut Text, &mut TextColor), Without<StatusText>>,
    mut q_status: Query<&mut Text, With<StatusText>>,
) {
    if !form.is_changed() {
        return;
    }
    for (row, mut text, mut color) in q_rows.iter_mut() {
        **text = format_row(&form, row.field);
        *color = if row.field == form.focus {
            TextColor(Color::srgb(0.30, 0.95, 0.65))
        } else {
            TextColor(Color::srgb(0.90, 0.90, 0.90))
        };
    }
    for mut t in q_status.iter_mut() {
        **t = form.validation_msg().unwrap_or_default();
    }
}

fn format_row(form: &CreateAccountForm, field: CreateAccountField) -> String {
    let focused = form.focus == field;
    let marker = if focused { "▶ " } else { "  " };
    let cursor = if focused { "_" } else { " " };
    match field {
        CreateAccountField::User => {
            format!("{marker}Username:        {u}{cursor}", u = form.user)
        }
        CreateAccountField::Password => {
            let masked: String = "*".repeat(form.pass.chars().count());
            format!("{marker}Password:        {masked}{cursor}")
        }
        CreateAccountField::PasswordConfirm => {
            let masked: String = "*".repeat(form.pass_confirm.chars().count());
            // Visual mismatch indicator: tint marker red when the
            // confirm field is non-empty AND doesn't match. The
            // validation footer also says so; this is just a faster
            // glance signal next to the row.
            let suffix = if !form.pass_confirm.is_empty() && form.pass != form.pass_confirm {
                "  ✗"
            } else {
                ""
            };
            format!("{marker}Confirm:         {masked}{cursor}{suffix}")
        }
    }
}

// --- CreateAccountError state ---------------------------------------------

#[derive(Component)]
pub(super) struct CreateAccountErrorRoot;

pub(super) fn spawn_error_ui(mut commands: Commands, msg: Res<CreateAccountErrorMsg>) {
    commands
        .spawn((
            CreateAccountErrorRoot,
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
                Text::new("Account creation failed"),
                TextFont {
                    font_size: 22.0,
                    ..default()
                },
                TextColor(Color::srgb(0.95, 0.30, 0.30)),
            ));
            parent.spawn((
                Text::new(msg.0.clone()),
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
    q: Query<Entity, With<CreateAccountErrorRoot>>,
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
                next_state.set(LauncherState::CreateAccountInFlight);
                return;
            }
            Key::Escape => {
                next_state.set(LauncherState::CreateAccount);
                return;
            }
            _ => {}
        }
    }
}
