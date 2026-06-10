//! Server-select screen — one row per saved server with inline
//! Edit + delete actions, plus an `+ Add server` affordance and an
//! optional `Cancel` button when there's somewhere to fall back to.

use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button, ButtonProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use ffxi_client::launcher_store::{self, keyring_account_key, KEYRING_SERVICE};
use ffxi_client::secret_store::SecretStore;

use super::common::{hint, panel_node, row, screen_root, title};
use super::{LauncherState, ServerSelectCursor, ServerSelectForm};

#[derive(Component)]
pub(super) struct ServerSelectRoot;

/// Per-server "armed for delete" marker. The first `×` click flips a row
/// into the armed state (button relabeled to `Confirm?`); the second
/// click within the same screen lifetime actually deletes. Inline
/// two-click was chosen over a separate confirm sub-state to avoid the
/// state-machine churn for a destructive but recoverable (re-add)
/// action.
#[derive(Resource, Default)]
pub(super) struct PendingServerDelete(pub Option<String>);

pub(super) fn spawn_ui(
    mut commands: Commands,
    mut cursor: ResMut<ServerSelectCursor>,
    form: Res<ServerSelectForm>,
    pending: Option<Res<PendingServerDelete>>,
) {
    let store = launcher_store::load();
    let servers = store.servers.clone();

    // Prefer an explicit in-session pick (`ServerSelectForm.selected`, set
    // when returning from Login), then the persisted `last_used` pair, so
    // the highlight lands on the relevant server. Clamp to a valid row so a
    // deleted/renamed server can't strand the cursor out of range.
    let preferred = form
        .selected
        .clone()
        .or_else(|| store.last_used.as_ref().map(|(s, _)| s.clone()));
    if let Some(name) = preferred {
        if let Some(idx) = servers.iter().position(|s| s.name == name) {
            cursor.0 = idx;
        }
    }
    if cursor.0 >= servers.len() {
        cursor.0 = servers.len().saturating_sub(1);
    }
    let cursor_idx = cursor.0;
    let n = servers.len();
    let pending_name = pending.and_then(|p| p.0.clone());
    let has_last_used = store.last_used.is_some();

    commands
        .spawn((ServerSelectRoot, screen_root()))
        .with_children(|root| {
            root.spawn(panel_node(620.0)).with_children(|panel| {
                panel.spawn(title("Servers"));
                if n == 0 {
                    panel.spawn(hint("No servers saved yet — click '+ Add server' below."));
                } else {
                    panel.spawn(hint(
                        "Click a server to pick it. Use Edit / × for per-row actions.",
                    ));
                }

                for (idx, s) in servers.iter().enumerate() {
                    let label = format!("{} — {}:{}", s.name, s.host, s.auth_port);
                    let server_name = s.name.clone();
                    let variant = if idx == cursor_idx {
                        ButtonVariant::Primary
                    } else {
                        ButtonVariant::Normal
                    };
                    let armed = pending_name.as_deref() == Some(s.name.as_str());

                    panel.spawn(row()).with_children(|r| {
                        let pick_name = server_name.clone();
                        // Wrapper grows to consume row slack so the
                        // contextual [Edit] [x] buttons sit flush
                        // right. We can't pass Node directly into
                        // `button(props, overrides, children)` — the
                        // button bundle already carries its own Node
                        // and Bevy panics on duplicate components in
                        // a merged bundle.
                        r.spawn(Node {
                            flex_grow: 1.0,
                            flex_direction: FlexDirection::Row,
                            ..default()
                        })
                        .with_children(|wrap| {
                            wrap.spawn(button(
                                ButtonProps {
                                    variant,
                                    ..default()
                                },
                                (),
                                Spawn((Text::new(label), ThemedText)),
                            ))
                            .observe(
                                move |_ev: On<Activate>,
                                      mut commands: Commands,
                                      mut cursor: ResMut<ServerSelectCursor>,
                                      mut form: ResMut<ServerSelectForm>,
                                      mut login: ResMut<super::LoginForm>,
                                      mut next: ResMut<NextState<LauncherState>>| {
                                    cursor.0 = idx;
                                    form.selected = Some(pick_name.clone());
                                    let store = launcher_store::load();
                                    if let Some(profile) =
                                        store.servers.iter().find(|p| p.name == pick_name)
                                    {
                                        super::apply_server_profile(&mut commands, profile);
                                    }
                                    // Pre-select this server's account: the
                                    // sole saved account if there's one,
                                    // else the most-recently-used. Pull its
                                    // password from the keyring when it opted
                                    // into remember; otherwise leave the form
                                    // blank for fresh credentials.
                                    if let Some(acct) =
                                        store.preselect_account_for(&pick_name)
                                    {
                                        login.user = acct.username.clone();
                                        login.remember_password = acct.remember_password;
                                        login.pass = if acct.remember_password {
                                            SecretStore::get(
                                                KEYRING_SERVICE,
                                                &keyring_account_key(
                                                    &pick_name,
                                                    &acct.username,
                                                ),
                                            )
                                            .unwrap_or_default()
                                        } else {
                                            String::new()
                                        };
                                        login.focus = if login.pass.is_empty() {
                                            super::LoginField::Password
                                        } else {
                                            super::LoginField::User
                                        };
                                    } else {
                                        login.user.clear();
                                        login.pass.clear();
                                        login.remember_password = false;
                                        login.focus = super::LoginField::User;
                                    }
                                    next.set(LauncherState::Login);
                                },
                            );
                        });

                        let edit_name = server_name.clone();
                        r.spawn(button(
                            ButtonProps::default(),
                            (),
                            Spawn((Text::new("Edit"), ThemedText)),
                        ))
                        .observe(
                            move |_ev: On<Activate>,
                                  mut edit: ResMut<super::ServerEditForm>,
                                  mut next: ResMut<NextState<LauncherState>>| {
                                let store = launcher_store::load();
                                if let Some((i, profile)) = store
                                    .servers
                                    .iter()
                                    .enumerate()
                                    .find(|(_, p)| p.name == edit_name)
                                {
                                    *edit = super::ServerEditForm::from_profile(profile);
                                    edit.editing_index = Some(i);
                                    next.set(LauncherState::ServerEdit);
                                }
                            },
                        );

                        let del_name = server_name.clone();
                        let (del_label, del_variant) = if armed {
                            ("Confirm?", ButtonVariant::Primary)
                        } else {
                            ("×", ButtonVariant::Normal)
                        };
                        r.spawn(button(
                            ButtonProps {
                                variant: del_variant,
                                ..default()
                            },
                            (),
                            Spawn((Text::new(del_label), ThemedText)),
                        ))
                        .observe(
                            move |_ev: On<Activate>,
                                  mut commands: Commands,
                                  mut cursor: ResMut<ServerSelectCursor>,
                                  pending: Option<ResMut<PendingServerDelete>>,
                                  mut next: ResMut<NextState<LauncherState>>| {
                                let already_armed = pending
                                    .as_ref()
                                    .and_then(|p| p.0.clone())
                                    == Some(del_name.clone());
                                if !already_armed {
                                    commands.insert_resource(PendingServerDelete(Some(
                                        del_name.clone(),
                                    )));
                                    next.set(LauncherState::ServerSelect);
                                    return;
                                }
                                let mut store = launcher_store::load();
                                if let Some(pos) =
                                    store.servers.iter().position(|p| p.name == del_name)
                                {
                                    let removed = store.servers.remove(pos);
                                    store.accounts.retain(|a| a.server_name != removed.name);
                                    if let Some((s, _)) = &store.last_used {
                                        if *s == removed.name {
                                            store.last_used = None;
                                        }
                                    }
                                    if cursor.0 >= store.servers.len()
                                        && !store.servers.is_empty()
                                    {
                                        cursor.0 = store.servers.len() - 1;
                                    }
                                    if let Err(e) = launcher_store::save(&store) {
                                        tracing::warn!(error = %e, "launcher_store: save failed");
                                    }
                                }
                                commands.insert_resource(PendingServerDelete(None));
                                next.set(LauncherState::ServerSelect);
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
                        Spawn((Text::new("+ Add server"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>,
                         mut edit: ResMut<super::ServerEditForm>,
                         mut next: ResMut<NextState<LauncherState>>| {
                            *edit = super::ServerEditForm::default();
                            edit.editing_index = None;
                            next.set(LauncherState::ServerEdit);
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

                    // Only render Cancel when there's a prior session to
                    // fall back to. Without `last_used`, hitting Cancel
                    // would land on a Login screen with no creds and no
                    // sensible "back" — the user has to pick a server
                    // first.
                    if has_last_used {
                        r.spawn(button(
                            ButtonProps::default(),
                            (),
                            Spawn((Text::new("Cancel"), ThemedText)),
                        ))
                        .observe(
                            |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                                next.set(LauncherState::Login);
                            },
                        );
                    }
                });
            });
        });
}

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<ServerSelectRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

/// Esc cancels back to login when there's a fall-back account; otherwise
/// it's swallowed (no valid target).
pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    mut commands: Commands,
    mut next: ResMut<NextState<LauncherState>>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if matches!(ev.logical_key, Key::Escape) {
            // Esc also clears any armed delete (so it doesn't carry
            // over silently to the next entry).
            commands.insert_resource(PendingServerDelete(None));
            if launcher_store::load().last_used.is_some() {
                next.set(LauncherState::Login);
            }
            return;
        }
    }
}
