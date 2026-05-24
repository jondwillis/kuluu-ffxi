//! Server-select screen — one button per saved server, plus action
//! buttons for add/edit/delete/skip. Single click on a server row picks
//! it (commits the selection and advances to the account picker).

use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button, ButtonProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use ffxi_client::launcher_store;

use super::common::{hint, panel_node, row, screen_root, title};
use super::{LauncherState, ServerSelectCursor, ServerSelectForm};

#[derive(Component)]
pub(super) struct ServerSelectRoot;

pub(super) fn spawn_ui(mut commands: Commands, cursor: Res<ServerSelectCursor>) {
    let store = launcher_store::load();
    let cursor_idx = cursor.0;
    let servers = store.servers.clone();
    let n = servers.len();

    commands
        .spawn((ServerSelectRoot, screen_root()))
        .with_children(|root| {
            root.spawn(panel_node(560.0)).with_children(|panel| {
                panel.spawn(title("Select server"));
                if n == 0 {
                    panel.spawn(hint("No servers saved yet — click '+ Add server' below."));
                } else {
                    panel.spawn(hint(
                        "Click a server to pick it. The highlighted row is the edit/delete target.",
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
                    panel
                        .spawn(button(
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
                                  mut next: ResMut<NextState<LauncherState>>| {
                                cursor.0 = idx;
                                form.selected = Some(server_name.clone());
                                // Re-bind the live AuthClient/LobbyClient
                                // + window-title to the picked profile.
                                // Without this the picker would just
                                // relabel the keyring grouping and the
                                // next login would still hit whatever
                                // host main.rs constructed at startup.
                                let store = launcher_store::load();
                                if let Some(profile) =
                                    store.servers.iter().find(|p| p.name == server_name)
                                {
                                    super::apply_server_profile(&mut commands, profile);
                                }
                                next.set(LauncherState::AccountPicker);
                            },
                        );
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
                        Spawn((Text::new("Edit selected"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>,
                         cursor: Res<ServerSelectCursor>,
                         mut edit: ResMut<super::ServerEditForm>,
                         mut next: ResMut<NextState<LauncherState>>| {
                            let store = launcher_store::load();
                            if store.servers.is_empty() {
                                return;
                            }
                            let idx = cursor.0.min(store.servers.len() - 1);
                            *edit = super::ServerEditForm::from_profile(&store.servers[idx]);
                            edit.editing_index = Some(idx);
                            next.set(LauncherState::ServerEdit);
                        },
                    );

                    r.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new("Delete selected"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>,
                         mut cursor: ResMut<ServerSelectCursor>,
                         mut next: ResMut<NextState<LauncherState>>| {
                            let mut store = launcher_store::load();
                            if store.servers.is_empty() {
                                return;
                            }
                            let idx = cursor.0.min(store.servers.len() - 1);
                            let removed = store.servers.remove(idx);
                            store.accounts.retain(|a| a.server_name != removed.name);
                            if let Some((s, _)) = &store.last_used {
                                if *s == removed.name {
                                    store.last_used = None;
                                }
                            }
                            if cursor.0 >= store.servers.len() && !store.servers.is_empty() {
                                cursor.0 = store.servers.len() - 1;
                            }
                            if let Err(e) = launcher_store::save(&store) {
                                tracing::warn!(error = %e, "launcher_store: save failed");
                            }
                            // Cheapest way to refresh the list: re-enter
                            // the same state (OnExit despawns, OnEnter
                            // rebuilds from the freshly-loaded store).
                            next.set(LauncherState::ServerSelect);
                        },
                    );

                    r.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new("Skip → Login"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                            next.set(LauncherState::Login);
                        },
                    );
                });
            });
        });
}

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<ServerSelectRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

/// Esc skips back to login (matches every other cancel affordance).
pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    mut next: ResMut<NextState<LauncherState>>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if matches!(ev.logical_key, Key::Escape) {
            next.set(LauncherState::Login);
            return;
        }
    }
}
