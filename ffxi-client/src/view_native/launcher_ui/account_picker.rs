//! Account-picker — one button per saved account for the selected
//! server. Click commits selection and advances to Login (prefilled).

use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button, ButtonProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use ffxi_client::launcher_store::{self, keyring_account_key, KEYRING_SERVICE};
use ffxi_client::secret_store::SecretStore;

use super::common::{hint, panel_node, row, screen_root, spawn_breadcrumb, title, Crumb};
use super::{AccountPickerCursor, LauncherState, LoginField, LoginForm, ServerInfo, ServerSelectForm};

#[derive(Component)]
pub(super) struct AccountPickerRoot;

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
    server_info: Res<ServerInfo>,
) {
    let server = form.selected.clone().unwrap_or_default();
    let accts = accounts_for(&server);
    let cursor_idx = cursor.0;

    commands
        .spawn((AccountPickerRoot, screen_root()))
        .with_children(|root| {
            spawn_breadcrumb(root, &server_info, &[Crumb::AccountPicker]);
            root.spawn(panel_node(560.0)).with_children(|panel| {
                panel.spawn(title(format!("Accounts on {server}")));
                if accts.is_empty() {
                    panel.spawn(hint(
                        "No saved accounts on this server — click '+ New account' to log in fresh.",
                    ));
                } else {
                    panel.spawn(hint("Click to pick. Use Forget to remove a saved account."));
                }

                for (idx, (u, remember)) in accts.iter().enumerate() {
                    let label = if *remember {
                        format!("{u}  [remembered]")
                    } else {
                        u.clone()
                    };
                    let user = u.clone();
                    let server_for_obs = server.clone();
                    let remember = *remember;
                    let variant = if idx == cursor_idx {
                        ButtonVariant::Primary
                    } else {
                        ButtonVariant::Normal
                    };

                    panel.spawn(row()).with_children(|r| {
                        let pick_user = user.clone();
                        let pick_server = server_for_obs.clone();
                        r.spawn(button(
                            ButtonProps {
                                variant,
                                ..default()
                            },
                            Node {
                                flex_grow: 1.0,
                                ..default()
                            },
                            Spawn((Text::new(label), ThemedText)),
                        ))
                        .observe(
                            move |_ev: On<Activate>,
                                  mut cursor: ResMut<AccountPickerCursor>,
                                  mut login: ResMut<LoginForm>,
                                  mut next: ResMut<NextState<LauncherState>>| {
                                cursor.0 = idx;
                                login.user = pick_user.clone();
                                login.pass.clear();
                                login.remember_password = remember;
                                if remember {
                                    if let Some(pw) = SecretStore::get(
                                        KEYRING_SERVICE,
                                        &keyring_account_key(&pick_server, &pick_user),
                                    ) {
                                        login.pass = pw;
                                    }
                                }
                                login.focus = if login.pass.is_empty() {
                                    LoginField::Password
                                } else {
                                    LoginField::User
                                };
                                next.set(LauncherState::Login);
                            },
                        );

                        let forget_user = user.clone();
                        let forget_server = server_for_obs.clone();
                        r.spawn(button(
                            ButtonProps::default(),
                            (),
                            Spawn((Text::new("Forget"), ThemedText)),
                        ))
                        .observe(
                            move |_ev: On<Activate>,
                                  mut cursor: ResMut<AccountPickerCursor>,
                                  mut next: ResMut<NextState<LauncherState>>| {
                                let mut store = launcher_store::load();
                                store.accounts.retain(|a| {
                                    !(a.server_name == forget_server
                                        && a.username == forget_user)
                                });
                                if let Some((s, u)) = &store.last_used {
                                    if *s == forget_server && *u == forget_user {
                                        store.last_used = None;
                                    }
                                }
                                if let Err(e) = launcher_store::save(&store) {
                                    tracing::warn!(error = %e, "launcher_store: save failed");
                                }
                                SecretStore::delete(
                                    KEYRING_SERVICE,
                                    &keyring_account_key(&forget_server, &forget_user),
                                );
                                let new_accts = accounts_for(&forget_server);
                                if cursor.0 >= new_accts.len() && !new_accts.is_empty() {
                                    cursor.0 = new_accts.len() - 1;
                                }
                                // Refresh by re-entering.
                                next.set(LauncherState::AccountPicker);
                            },
                        );
                    });
                }

                panel.spawn(row()).with_children(|r| {
                    r.spawn(button(
                        ButtonProps {
                            variant: ButtonVariant::Primary,
                            ..default()
                        },
                        (),
                        Spawn((Text::new("+ New account"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>,
                         mut login: ResMut<LoginForm>,
                         mut next: ResMut<NextState<LauncherState>>| {
                            login.user.clear();
                            login.pass.clear();
                            login.focus = LoginField::User;
                            next.set(LauncherState::Login);
                        },
                    );

                    r.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new("Back"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                            next.set(LauncherState::ServerSelect);
                        },
                    );
                });
            });
        });
}

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<AccountPickerRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    mut next: ResMut<NextState<LauncherState>>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if matches!(ev.logical_key, Key::Escape) {
            next.set(LauncherState::ServerSelect);
            return;
        }
    }
}
