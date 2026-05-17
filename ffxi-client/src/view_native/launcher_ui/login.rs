//! Login screen: username/password fields + Enter-to-submit.
//!
//! Bevy 0.17 has no built-in text-input widget. We roll our own:
//!  * `keyboard_input_system` reads `KeyboardInput` events directly so
//!    we get raw character data alongside the `KeyCode` (the latter
//!    alone can't disambiguate shifted vs unshifted layouts reliably).
//!  * `redraw_login_form_system` runs every frame and rewrites the
//!    `Text` nodes from the [`LoginForm`] resource. Cheaper than
//!    diffing — these are short strings.
//!
//! Cursor positioning isn't supported (append-only). Backspace removes
//! the last char. Tab cycles focus between the two fields.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;

use super::{Credentials, LauncherState, LoginErrorMsg, LoginField, LoginForm, ServerInfo};

/// Marker for the root login UI node so we can despawn the whole tree on
/// state exit.
#[derive(Component)]
pub(super) struct LoginUiRoot;

/// Marker for the username text node.
#[derive(Component)]
pub(super) struct UserText;

/// Marker for the password text node (rendered as `*`s).
#[derive(Component)]
pub(super) struct PassText;

pub(super) fn spawn_login_ui(
    mut commands: Commands,
    server: Res<ServerInfo>,
    form: Res<LoginForm>,
) {
    commands
        .spawn((
            LoginUiRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(12.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.04, 0.04, 0.05)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new(format!("FFXI agent launcher — {}", server.server)),
                TextFont {
                    font_size: 22.0,
                    ..default()
                },
                TextColor(Color::srgb(0.0, 1.0, 1.0)),
            ));
            parent.spawn((
                Text::new(""),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.6, 0.6)),
            ));
            parent.spawn((
                UserText,
                Text::new(format_user_line(&form.user, form.focus == LoginField::User)),
                TextFont {
                    font_size: 18.0,
                    ..default()
                },
                TextColor(Color::srgb(0.95, 0.95, 0.95)),
            ));
            parent.spawn((
                PassText,
                Text::new(format_pass_line(
                    &form.pass,
                    form.focus == LoginField::Password,
                )),
                TextFont {
                    font_size: 18.0,
                    ..default()
                },
                TextColor(Color::srgb(0.95, 0.95, 0.95)),
            ));
            parent.spawn((
                Text::new(
                    "Tab: switch field   Enter: login   Ctrl-N: new account   Esc: clear field",
                ),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgb(0.5, 0.5, 0.5)),
            ));
        });
}

pub(super) fn despawn_login_ui(mut commands: Commands, q: Query<Entity, With<LoginUiRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

/// Reads `KeyboardInput` events and mutates the form. Triggers transition
/// to `AuthInFlight` on Enter when both fields are non-empty.
pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    mut form: ResMut<LoginForm>,
    mut next_state: ResMut<NextState<LauncherState>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let ctrl = keys.pressed(KeyCode::ControlLeft)
        || keys.pressed(KeyCode::ControlRight)
        || keys.pressed(KeyCode::SuperLeft)
        || keys.pressed(KeyCode::SuperRight);
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        // Ctrl-N (or Cmd-N on macOS) → jump to account-creation screen.
        // We check the modifier before the Character match so the 'n'
        // doesn't end up typed into the focused field.
        if ctrl {
            if let Key::Character(s) = &ev.logical_key {
                if s.eq_ignore_ascii_case("n") {
                    next_state.set(LauncherState::CreateAccount);
                    return;
                }
            }
        }
        match &ev.logical_key {
            Key::Enter => {
                if !form.user.is_empty() && !form.pass.is_empty() {
                    next_state.set(LauncherState::AuthInFlight);
                    return;
                }
            }
            Key::Tab => {
                form.focus = match form.focus {
                    LoginField::User => LoginField::Password,
                    LoginField::Password => LoginField::User,
                };
            }
            Key::Backspace => match form.focus {
                LoginField::User => {
                    form.user.pop();
                }
                LoginField::Password => {
                    form.pass.pop();
                }
            },
            Key::Escape => match form.focus {
                LoginField::User => form.user.clear(),
                LoginField::Password => form.pass.clear(),
            },
            Key::Character(s) => {
                // `s` is a SmolStr; iterate over chars to filter control
                // bytes and append printable ones.
                for c in s.chars() {
                    if !c.is_control() {
                        match form.focus {
                            LoginField::User => form.user.push(c),
                            LoginField::Password => form.pass.push(c),
                        }
                    }
                }
            }
            Key::Space => match form.focus {
                LoginField::User => form.user.push(' '),
                LoginField::Password => form.pass.push(' '),
            },
            _ => {}
        }
    }
}

pub(super) fn redraw_login_form_system(
    form: Res<LoginForm>,
    mut q_user: Query<&mut Text, (With<UserText>, Without<PassText>)>,
    mut q_pass: Query<&mut Text, (With<PassText>, Without<UserText>)>,
) {
    if !form.is_changed() {
        return;
    }
    for mut t in q_user.iter_mut() {
        **t = format_user_line(&form.user, form.focus == LoginField::User);
    }
    for mut t in q_pass.iter_mut() {
        **t = format_pass_line(&form.pass, form.focus == LoginField::Password);
    }
}

fn format_user_line(user: &str, focused: bool) -> String {
    let cursor = if focused { "_" } else { " " };
    format!("Username:  {user}{cursor}")
}

fn format_pass_line(pass: &str, focused: bool) -> String {
    let cursor = if focused { "_" } else { " " };
    let masked: String = "*".repeat(pass.chars().count());
    format!("Password:  {masked}{cursor}")
}

// --- LoginError state -----------------------------------------------------

/// Marker for the error UI root.
#[derive(Component)]
pub(super) struct ErrorUiRoot;

pub(super) fn spawn_error_ui(mut commands: Commands, msg: Res<LoginErrorMsg>) {
    commands
        .spawn((
            ErrorUiRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(20.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.04, 0.04, 0.05)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("Login failed"),
                TextFont {
                    font_size: 22.0,
                    ..default()
                },
                TextColor(Color::srgb(0.95, 0.20, 0.20)),
            ));
            parent.spawn((
                Text::new(msg.0.clone()),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(Color::srgb(0.85, 0.85, 0.85)),
            ));
            parent.spawn((
                Text::new("Press Esc to return to login."),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgb(0.55, 0.55, 0.55)),
            ));
        });
}

pub(super) fn despawn_error_ui(mut commands: Commands, q: Query<Entity, With<ErrorUiRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

pub(super) fn error_keyboard_system(
    mut events: MessageReader<KeyboardInput>,
    mut next_state: ResMut<NextState<LauncherState>>,
    mut form: ResMut<LoginForm>,
    mut creds: ResMut<Credentials>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if matches!(ev.logical_key, Key::Escape | Key::Enter) {
            // Wipe stale credentials but keep the username — same shape as
            // the stdin launcher's "ask for password again" behaviour.
            form.pass.clear();
            creds.user.clear();
            creds.pass.clear();
            next_state.set(LauncherState::Login);
            return;
        }
    }
}
