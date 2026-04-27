use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button, checkbox, ButtonProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::{Activate, ValueChange};

use ffxi_client::launcher_store::{self, keyring_account_key, KEYRING_SERVICE};
use ffxi_client::secret_store::SecretStore;

use super::common::{hint, panel_node, row, screen_root, spawn_breadcrumb, title, Crumb};
use super::{
    Credentials, LauncherState, LoginErrorMsg, LoginField, LoginForm, ServerInfo, ServerSelectForm,
};
use crate::view_native::widgets::text_field::{text_field, TextFieldSubmitted};
use crate::view_native::widgets::{TextFieldDisplay, TextFieldProps};

#[derive(Component)]
pub(super) struct LoginUiRoot;

#[derive(Resource, Default)]
pub(super) struct LoginUiDirty(pub bool);

fn saved_accounts_for(form: &ServerSelectForm, info: &ServerInfo) -> (String, Vec<(String, bool)>) {
    let server_key = form.selected.clone().unwrap_or_else(|| info.server.clone());
    let accts = launcher_store::load()
        .accounts
        .into_iter()
        .filter(|a| a.server_name == server_key)
        .map(|a| (a.username, a.remember_password))
        .collect();
    (server_key, accts)
}

pub(super) fn spawn_login_ui(
    mut commands: Commands,
    server: Res<ServerInfo>,
    form: Res<LoginForm>,
    server_form: Res<ServerSelectForm>,
) {
    build_login_ui(&mut commands, &server, &form, &server_form);
}

pub(super) fn rebuild_login_ui_system(
    mut dirty: ResMut<LoginUiDirty>,
    mut commands: Commands,
    existing: Query<Entity, With<LoginUiRoot>>,
    server: Res<ServerInfo>,
    form: Res<LoginForm>,
    server_form: Res<ServerSelectForm>,
) {
    if !dirty.0 {
        return;
    }
    dirty.0 = false;
    for e in existing.iter() {
        commands.entity(e).despawn();
    }
    build_login_ui(&mut commands, &server, &form, &server_form);
}

fn build_login_ui(
    commands: &mut Commands,
    server: &ServerInfo,
    form: &LoginForm,
    server_form: &ServerSelectForm,
) {
    let user_initial = form.user.clone();
    let pass_initial = form.pass.clone();
    let remember = form.remember_password;
    let active_user = form.user.clone();
    let (server_key, accts) = saved_accounts_for(server_form, server);

    commands
        .spawn((LoginUiRoot, screen_root()))
        .with_children(|root| {
            spawn_breadcrumb(root, server, &[Crumb::Sign(None)]);
            root.spawn(panel_node(560.0)).with_children(|panel| {
                panel.spawn(title(format!("Sign in to {}", server.display_label())));
                panel.spawn(hint("Tab cycles fields. Enter submits when both filled."));

                spawn_saved_accounts_row(panel, &server_key, &active_user, &accts);

                spawn_field(panel, "Username", false, &user_initial, LoginField::User);
                spawn_field(panel, "Password", true, &pass_initial, LoginField::Password);

                let mut cb = panel.spawn(checkbox(
                    (),
                    Spawn((Text::new("Remember password"), ThemedText)),
                ));
                if remember {
                    cb.insert(Checked);
                }
                cb.observe(
                    |ev: On<ValueChange<bool>>,
                     mut form: ResMut<LoginForm>,
                     mut commands: Commands| {
                        form.remember_password = ev.value;
                        if ev.value {
                            commands.entity(ev.source).insert(Checked);
                        } else {
                            commands.entity(ev.source).remove::<Checked>();
                        }
                    },
                );

                panel.spawn(row()).with_children(|r| {
                    r.spawn(button(
                        ButtonProps {
                            variant: ButtonVariant::Primary,
                            ..default()
                        },
                        (),
                        Spawn((Text::new("Log in"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>,
                         form: Res<LoginForm>,
                         mut next: ResMut<NextState<LauncherState>>| {
                            if !form.user.is_empty() && !form.pass.is_empty() {
                                next.set(LauncherState::AuthInFlight);
                            }
                        },
                    );

                    r.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new("Create account"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                            next.set(LauncherState::CreateAccount);
                        },
                    );

                    r.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new("Change password"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                            next.set(LauncherState::ChangePassword);
                        },
                    );

                    r.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new("Settings"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                            next.set(LauncherState::Settings);
                        },
                    );

                    r.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new("Graphics"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                            next.set(LauncherState::Graphics);
                        },
                    );
                });
            });
        });
}

fn spawn_saved_accounts_row(
    panel: &mut ChildSpawnerCommands,
    server_key: &str,
    active_user: &str,
    accts: &[(String, bool)],
) {
    if accts.is_empty() {
        return;
    }

    panel.spawn(hint("Saved accounts on this server:"));
    panel.spawn(row()).with_children(|r| {
        for (u, remember) in accts.iter() {
            let label = if *remember {
                format!("{u}  [saved]")
            } else {
                u.clone()
            };
            let is_active = u == active_user;
            let variant = if is_active {
                ButtonVariant::Primary
            } else {
                ButtonVariant::Normal
            };
            let pick_user = u.clone();
            let pick_server = server_key.to_string();
            let pick_remember = *remember;

            r.spawn(button(
                ButtonProps {
                    variant,
                    ..default()
                },
                (),
                Spawn((Text::new(label), ThemedText)),
            ))
            .observe(
                move |_ev: On<Activate>,
                      mut login: ResMut<LoginForm>,
                      mut dirty: ResMut<LoginUiDirty>| {
                    login.user = pick_user.clone();
                    login.pass.clear();
                    login.remember_password = pick_remember;
                    if pick_remember {
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

                    dirty.0 = true;
                },
            );

            let forget_user = u.clone();
            let forget_server = server_key.to_string();
            r.spawn(button(
                ButtonProps::default(),
                (),
                Spawn((Text::new("×"), ThemedText)),
            ))
            .observe(
                move |_ev: On<Activate>,
                      mut login: ResMut<LoginForm>,
                      mut dirty: ResMut<LoginUiDirty>| {
                    let mut store = launcher_store::load();
                    store
                        .accounts
                        .retain(|a| !(a.server_name == forget_server && a.username == forget_user));
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

                    if login.user == forget_user {
                        login.user.clear();
                        login.pass.clear();
                        login.remember_password = false;
                        login.focus = LoginField::User;
                    }
                    dirty.0 = true;
                },
            );
        }

        r.spawn(button(
            ButtonProps::default(),
            (),
            Spawn((Text::new("+ New"), ThemedText)),
        ))
        .observe(
            |_ev: On<Activate>, mut login: ResMut<LoginForm>, mut dirty: ResMut<LoginUiDirty>| {
                login.user.clear();
                login.pass.clear();
                login.remember_password = false;
                login.focus = LoginField::User;
                dirty.0 = true;
            },
        );
    });
}

fn spawn_field(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    mask: bool,
    initial: &str,
    binding: LoginField,
) {
    parent
        .spawn(Node {
            width: Val::Percent(100.0),
            height: Val::Px(32.0),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(8.0),
            ..default()
        })
        .with_children(|row| {
            row.spawn((
                Node {
                    width: Val::Px(110.0),
                    ..default()
                },
                Text::new(label.to_string()),
                ThemedText,
            ));
            row.spawn(text_field(TextFieldProps {
                initial: initial.to_string(),
                mask,
                submit_on_enter: true,
                ..default()
            }))
            .with_children(|tf| {
                tf.spawn((
                    Node {
                        flex_grow: 1.0,
                        ..default()
                    },
                    Text::new(String::new()),
                    TextColor(Color::srgb(0.92, 0.92, 0.95)),
                    TextFieldDisplay {
                        owner: Entity::PLACEHOLDER,
                    },
                    ThemedText,
                ));
            })
            .observe(
                move |ev: On<ValueChange<String>>, mut form: ResMut<LoginForm>| match binding {
                    LoginField::User => form.user = ev.value.clone(),
                    LoginField::Password => form.pass = ev.value.clone(),
                },
            )
            .observe(
                move |_ev: On<TextFieldSubmitted>,
                      form: Res<LoginForm>,
                      mut next: ResMut<NextState<LauncherState>>| {
                    if !form.user.is_empty() && !form.pass.is_empty() {
                        next.set(LauncherState::AuthInFlight);
                    }
                },
            );
        });
}

pub(super) fn despawn_login_ui(mut commands: Commands, q: Query<Entity, With<LoginUiRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    mut form: ResMut<LoginForm>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if matches!(ev.logical_key, Key::Escape) {
            form.user.clear();
            form.pass.clear();
        }
    }
}

pub(super) fn redraw_login_form_system() {}

#[derive(Component)]
pub(super) struct ErrorUiRoot;

pub(super) fn spawn_error_ui(mut commands: Commands, msg: Res<LoginErrorMsg>) {
    let body = msg.0.clone();
    commands
        .spawn((ErrorUiRoot, screen_root()))
        .with_children(|root| {
            root.spawn(panel_node(520.0)).with_children(|panel| {
                panel.spawn((
                    Text::new("Login failed"),
                    TextFont {
                        font_size: 22.0,
                        ..default()
                    },
                    TextColor(Color::srgb(0.95, 0.20, 0.20)),
                    ThemedText,
                ));
                panel.spawn((
                    Text::new(body),
                    TextFont {
                        font_size: 14.0,
                        ..default()
                    },
                    TextColor(Color::srgb(0.85, 0.85, 0.85)),
                    ThemedText,
                ));
                panel
                    .spawn(button(
                        ButtonProps {
                            variant: ButtonVariant::Primary,
                            ..default()
                        },
                        (),
                        Spawn((Text::new("Back to login"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>,
                         mut form: ResMut<LoginForm>,
                         mut creds: ResMut<Credentials>,
                         mut next: ResMut<NextState<LauncherState>>| {
                            form.pass.clear();
                            creds.user.clear();
                            creds.pass.clear();
                            next.set(LauncherState::Login);
                        },
                    );
            });
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
        if matches!(ev.logical_key, Key::Escape) {
            form.pass.clear();
            creds.user.clear();
            creds.pass.clear();
            next_state.set(LauncherState::Login);
            return;
        }
    }
}
