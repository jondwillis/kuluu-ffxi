use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button_bundle, ButtonBundleProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use crate::launcher_store::{self, keyring_account_key, KEYRING_SERVICE};
use crate::secret_store::SecretStore;

use super::brand::{spawn_brand_mark, BrandMark};
use super::common::{
    chip_group, hint, panel_node, row, screen_root, spawn_settings_close_titlebar,
    DefaultFocusTarget,
};
use super::{LauncherState, ServerSelectCursor, ServerSelectForm};

#[derive(Component)]
pub(super) struct ServerSelectRoot;

#[derive(Resource, Default)]
pub(super) struct PendingServerDelete(pub Option<String>);

pub(super) fn spawn_ui(
    mut commands: Commands,
    mut cursor: ResMut<ServerSelectCursor>,
    form: Res<ServerSelectForm>,
    pending: Option<Res<PendingServerDelete>>,
    mark: Res<BrandMark>,
) {
    let store = launcher_store::load();
    let servers = store.servers.clone();

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
            spawn_brand_mark(root, &mark);
            root.spawn(panel_node(620.0)).with_children(|panel| {
                spawn_settings_close_titlebar(panel, "Servers");
                if n == 0 {
                    panel.spawn(hint("No servers saved yet - click '+ Add server' below."));
                }

                for (idx, s) in servers.iter().enumerate() {
                    let label = if s.is_playonline() {
                        format!("{} - {} {}", s.name, s.host, s.flavor.label())
                    } else {
                        format!("{} - {}:{}", s.name, s.host, s.auth_port)
                    };
                    let server_name = s.name.clone();
                    let variant = if idx == cursor_idx {
                        ButtonVariant::Primary
                    } else {
                        ButtonVariant::Normal
                    };
                    let armed = pending_name.as_deref() == Some(s.name.as_str());

                    // One visually-connected pill: select chip + Edit + Delete.
                    panel.spawn(row()).with_children(|r| {
                        r.spawn(chip_group()).with_children(|chip| {
                            let pick_name = server_name.clone();

                            chip.spawn(button_bundle(
                                ButtonBundleProps {
                                    variant,
                                    ..default()
                                },
                                (),
                                Spawn((Text::new(label), ThemedText)),
                            ))
                            .insert_if(DefaultFocusTarget, || idx == cursor_idx)
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

                            let edit_name = server_name.clone();
                            chip.spawn(button_bundle(
                            ButtonBundleProps::default(),
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
                                ("Delete", ButtonVariant::Normal)
                            };
                            chip.spawn(button_bundle(
                            ButtonBundleProps {
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
                    });
                }

                panel.spawn(row()).with_children(|r| {
                    r.spawn(button_bundle(
                        ButtonBundleProps {
                            variant: ButtonVariant::Primary,
                            ..default()
                        },
                        (),
                        Spawn((Text::new("+ Add server"), ThemedText)),
                    ))
                    .insert_if(DefaultFocusTarget, || n == 0)
                    .observe(
                        |_ev: On<Activate>,
                         mut edit: ResMut<super::ServerEditForm>,
                         mut next: ResMut<NextState<LauncherState>>| {
                            *edit = super::ServerEditForm::default();
                            edit.editing_index = None;
                            next.set(LauncherState::ServerEdit);
                        },
                    );

                    if has_last_used {
                        r.spawn(button_bundle(
                            ButtonBundleProps::default(),
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

/// Where Escape backs out from the server list: Login, but only when a
/// server was last used - the same condition the "Cancel" button is shown
/// under. Without it there is no login to return to, so the key stays inert
/// and the user must add a server first.
pub(super) fn escape_back_target(store: &launcher_store::LauncherStore) -> Option<LauncherState> {
    store.last_used.is_some().then_some(LauncherState::Login)
}

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
            commands.insert_resource(PendingServerDelete(None));
            if let Some(target) = escape_back_target(&launcher_store::load()) {
                next.set(target);
            }
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_backs_out_to_login_only_when_a_server_was_last_used() {
        let mut store = launcher_store::LauncherStore::default();
        assert!(
            escape_back_target(&store).is_none(),
            "without a last-used server there is no login to return to"
        );
        store.last_used = Some(("HXI".into(), "batti".into()));
        assert_eq!(escape_back_target(&store), Some(LauncherState::Login));
    }
}
