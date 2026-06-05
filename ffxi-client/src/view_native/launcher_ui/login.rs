//! Login screen — feathers-based buttons + TextFields.
//!
//! Saved accounts for the active server render as a row of chips above
//! the credential form. Clicking a chip prefills the form (pulling the
//! password from the OS keyring when `remember_password` is set on the
//! matching `SavedAccount`); the per-chip × removes both the launcher_
//! store row and the keyring entry. Replaces the standalone
//! `AccountPicker` screen so the user never has to navigate to a sub-
//! state just to swap between two saved logins on the same server.

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

/// Set by the saved-account chips / `× forget` / `+ New` observers to ask
/// for an in-place rebuild of the login panel. We can't lean on a
/// `next.set(LauncherState::Login)` self-transition for this — Bevy only
/// runs `OnExit`/`OnEnter` when the state *changes*, so re-setting the
/// current state is a silent no-op and the panel never refreshes. The
/// `rebuild_login_ui_system` watches this flag, despawns the old
/// `LoginUiRoot`, and rebuilds from the (now-mutated) `LoginForm` +
/// `LauncherStore`.
#[derive(Resource, Default)]
pub(super) struct LoginUiDirty(pub bool);

/// Return saved (username, remember_password) tuples for the server that
/// `ServerSelectForm.selected` points at. Falls back to `ServerInfo.server`
/// when no profile has been explicitly picked yet (CLI-args path).
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

/// In-place rebuild: when a chip / forget / `+ New` observer flips
/// [`LoginUiDirty`], tear down the existing panel and rebuild it from the
/// current `LoginForm` + store. Runs in `Update` while in `Login`.
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
            spawn_breadcrumb(root, &server, &[Crumb::Sign(None)]);
            root.spawn(panel_node(560.0)).with_children(|panel| {
                panel.spawn(title(format!("Sign in to {}", server.display_label())));
                panel.spawn(hint("Tab cycles fields. Enter submits when both filled."));

                // Saved-account chips. The active row (matches `form.user`)
                // renders Primary; the others Normal. The trailing `+ New`
                // button clears the form back to a blank entry.
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
                });
            });
        });
}

/// Render the saved-accounts chip row above the form. Each row is a
/// (Pick) + (×) pair; the trailing `+ New` button clears the form for a
/// fresh login. The whole row is skipped when no saved accounts exist
/// for the active server — the form alone is enough.
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
            // The feathers font has no ★/✓ glyph (renders as a missing-
            // glyph box), so the "remembered" marker is plain ASCII.
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
                    // Rebuild the panel so the new active chip + prefilled
                    // fields show. (A self-transition would be a no-op.)
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
                    // If the form was showing the forgotten user, clear
                    // it; otherwise leave the user's in-progress entry
                    // alone.
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

/// Spawn a labeled TextField row with per-entity observers for value
/// edits and Enter-submit. The row layout matches `labeled_text_field`
/// but is inlined here so we own the TextField entity and can attach
/// `.observe()`.
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
                // Display child — mirrors the layout in widgets::mod.rs.
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

/// TextField self-renders — kept as a no-op so `mod.rs::register`'s
/// existing system tuple compiles without a touch.
pub(super) fn redraw_login_form_system() {}

// --- LoginError state -----------------------------------------------------

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
