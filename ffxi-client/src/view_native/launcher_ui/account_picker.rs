//! Pick a saved account for the previously-selected server. Enter
//! prefills the Login form (with keyring-stored password if
//! `remember_password`) and transitions to `Login`. Ctrl-N → blank
//! Login. Delete forgets the account locally + clears its keyring entry.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;

use ffxi_client::launcher_store::{self, keyring_account_key, KEYRING_SERVICE};
use ffxi_client::secret_store::SecretStore;

use super::{
    AccountPickerCursor, LauncherState, LoginField, LoginForm, ServerSelectForm,
};

#[derive(Component)]
pub(super) struct AccountPickerRoot;

#[derive(Component)]
pub(super) struct AccountListText;

fn accounts_for(server: &str) -> Vec<(String, bool)> {
    launcher_store::load()
        .accounts
        .into_iter()
        .filter(|a| a.server_name == server)
        .map(|a| (a.username, a.remember_password))
        .collect()
}

pub(super) fn spawn_ui(
    mut commands: Commands,
    form: Res<ServerSelectForm>,
    cursor: Res<AccountPickerCursor>,
) {
    let server = form.selected.clone().unwrap_or_default();
    let accts = accounts_for(&server);
    let body = format_body(&server, &accts, cursor.0);
    commands
        .spawn((
            AccountPickerRoot,
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
                Text::new(format!("Accounts on {server}")),
                TextFont {
                    font_size: 22.0,
                    ..default()
                },
                TextColor(Color::srgb(0.0, 1.0, 1.0)),
            ));
            parent.spawn((
                AccountListText,
                Text::new(body),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(Color::srgb(0.95, 0.95, 0.95)),
            ));
            parent.spawn((
                Text::new(
                    "↑/↓ select   Enter pick   Ctrl-N new   Del forget   Esc back",
                ),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgb(0.5, 0.5, 0.5)),
            ));
        });
}

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<AccountPickerRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    server_form: Res<ServerSelectForm>,
    mut cursor: ResMut<AccountPickerCursor>,
    mut login: ResMut<LoginForm>,
    mut next_state: ResMut<NextState<LauncherState>>,
    mut q_body: Query<&mut Text, With<AccountListText>>,
) {
    let ctrl = keys.pressed(KeyCode::ControlLeft)
        || keys.pressed(KeyCode::ControlRight)
        || keys.pressed(KeyCode::SuperLeft)
        || keys.pressed(KeyCode::SuperRight);
    let server = server_form.selected.clone().unwrap_or_default();
    let accts = accounts_for(&server);
    let n = accts.len();

    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if ctrl {
            if let Key::Character(s) = &ev.logical_key {
                if s.eq_ignore_ascii_case("n") {
                    login.user.clear();
                    login.pass.clear();
                    login.focus = LoginField::User;
                    next_state.set(LauncherState::Login);
                    return;
                }
            }
        }
        match &ev.logical_key {
            Key::Escape => {
                next_state.set(LauncherState::ServerSelect);
                return;
            }
            Key::ArrowUp if n > 0 => {
                cursor.0 = (cursor.0 + n - 1) % n;
                for mut t in q_body.iter_mut() {
                    **t = format_body(&server, &accts, cursor.0);
                }
            }
            Key::ArrowDown if n > 0 => {
                cursor.0 = (cursor.0 + 1) % n;
                for mut t in q_body.iter_mut() {
                    **t = format_body(&server, &accts, cursor.0);
                }
            }
            Key::Enter if n > 0 => {
                let idx = cursor.0.min(n - 1);
                let (user, remember) = accts[idx].clone();
                login.user = user.clone();
                login.pass.clear();
                login.remember_password = remember;
                if remember {
                    if let Some(pw) =
                        SecretStore::get(KEYRING_SERVICE, &keyring_account_key(&server, &user))
                    {
                        login.pass = pw;
                    }
                }
                login.focus = if login.pass.is_empty() {
                    LoginField::Password
                } else {
                    LoginField::User
                };
                next_state.set(LauncherState::Login);
                return;
            }
            Key::Delete if n > 0 => {
                let idx = cursor.0.min(n - 1);
                let (user, _) = accts[idx].clone();
                let mut store = launcher_store::load();
                store
                    .accounts
                    .retain(|a| !(a.server_name == server && a.username == user));
                if let Some((s, u)) = &store.last_used {
                    if *s == server && *u == user {
                        store.last_used = None;
                    }
                }
                if let Err(e) = launcher_store::save(&store) {
                    tracing::warn!(error = %e, "launcher_store: save failed");
                }
                SecretStore::delete(KEYRING_SERVICE, &keyring_account_key(&server, &user));
                let new_accts = accounts_for(&server);
                if cursor.0 >= new_accts.len() && !new_accts.is_empty() {
                    cursor.0 = new_accts.len() - 1;
                }
                for mut t in q_body.iter_mut() {
                    **t = format_body(&server, &new_accts, cursor.0);
                }
            }
            _ => {}
        }
    }
}

fn format_body(server: &str, accts: &[(String, bool)], cursor: usize) -> String {
    if accts.is_empty() {
        return format!("(no saved accounts on '{server}' — Ctrl-N to log in fresh)");
    }
    let mut out = String::new();
    for (i, (u, remember)) in accts.iter().enumerate() {
        let marker = if i == cursor { ">" } else { " " };
        let r = if *remember { "[remembered]" } else { "" };
        out.push_str(&format!("{marker} {u}  {r}\n"));
    }
    out
}
