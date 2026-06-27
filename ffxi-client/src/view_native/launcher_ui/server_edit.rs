use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button, ButtonProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, ValueChange};

use ffxi_client::auth_client;
use ffxi_client::launcher_store::{self, AuthFlavorKind, ServerProfile};

use super::common::{hint, panel_node, row, screen_root, spawn_breadcrumb, title, Crumb};
use crate::view_native::widgets::text_field::text_field;
use crate::view_native::widgets::{TextFieldDisplay, TextFieldProps};

use super::{LauncherState, ServerEditField, ServerEditForm, ServerInfo};

#[derive(Component)]
pub(super) struct ServerEditRoot;

#[derive(Component, Clone, Copy)]
pub(super) struct FlavorButton(AuthFlavorKind);

pub(super) fn spawn_ui(mut commands: Commands, form: Res<ServerEditForm>, server: Res<ServerInfo>) {
    let editing = form.editing_index.is_some();
    let snap = (
        form.name.clone(),
        form.host.clone(),
        form.auth_port.clone(),
        form.data_port.clone(),
        form.view_port.clone(),
        form.flavor,
        form.xiloader_version.clone(),
        form.version_check_url.clone(),
    );

    let default_version = auth_client::resolve_client_version(None);
    let version_placeholder = format!(
        "{}.{}.{}",
        default_version[0], default_version[1], default_version[2]
    );

    let leaf = if editing {
        Crumb::Other(format!("Edit: {}", snap.0))
    } else {
        Crumb::Other("New server".to_string())
    };

    commands
        .spawn((ServerEditRoot, screen_root()))
        .with_children(|root| {
            spawn_breadcrumb(root, &server, &[Crumb::Server, leaf]);
            root.spawn(panel_node(560.0)).with_children(|panel| {
                panel.spawn(title(if editing { "Edit server" } else { "New server" }));
                panel.spawn(hint("Tab cycles fields. Esc cancels."));

                spawn_field(panel, "Name", &snap.0, "", ServerEditField::Name);
                spawn_field(panel, "Host", &snap.1, "", ServerEditField::Host);
                spawn_field(panel, "Auth port", &snap.2, "", ServerEditField::AuthPort);
                spawn_field(panel, "Data port", &snap.3, "", ServerEditField::DataPort);
                spawn_field(panel, "View port", &snap.4, "", ServerEditField::ViewPort);

                panel.spawn(row()).with_children(|r| {
                    r.spawn((
                        Node {
                            width: Val::Px(110.0),
                            ..default()
                        },
                        Text::new("Flavor"),
                        ThemedText,
                    ));
                    spawn_flavor_button(r, "JSON", AuthFlavorKind::Json, snap.5);
                    spawn_flavor_button(r, "Binary", AuthFlavorKind::Binary, snap.5);
                });

                spawn_field(
                    panel,
                    "Xiloader version",
                    &snap.6,
                    &version_placeholder,
                    ServerEditField::XiloaderVersion,
                );

                spawn_field(
                    panel,
                    "Version check URL",
                    &snap.7,
                    "https://server/version.json",
                    ServerEditField::VersionCheckUrl,
                );

                panel.spawn(row()).with_children(|r| {
                    r.spawn(button(
                        ButtonProps {
                            variant: ButtonVariant::Primary,
                            ..default()
                        },
                        (),
                        Spawn((Text::new("Save"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>,
                         form: Res<ServerEditForm>,
                         mut next: ResMut<NextState<LauncherState>>| {
                            save_form(&form, &mut next);
                        },
                    );
                    r.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new("Cancel"), ThemedText)),
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

fn save_form(form: &ServerEditForm, next: &mut NextState<LauncherState>) {
    if form.name.is_empty() || form.host.is_empty() {
        return;
    }
    let auth_port = form.auth_port.parse().unwrap_or(0);
    let data_port = form.data_port.parse().unwrap_or(0);
    let view_port = form.view_port.parse().unwrap_or(0);
    if auth_port == 0 || data_port == 0 || view_port == 0 {
        return;
    }
    let xiloader_version = {
        let trimmed = form.xiloader_version.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    };
    let version_check_url = {
        let trimmed = form.version_check_url.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    };
    let profile = ServerProfile {
        name: form.name.clone(),
        host: form.host.clone(),
        auth_port,
        data_port,
        view_port,
        flavor: form.flavor,
        xiloader_version,
        version_check_url,
    };
    let mut store = launcher_store::load();
    match form.editing_index {
        Some(idx) if idx < store.servers.len() => store.servers[idx] = profile,
        _ => store.servers.push(profile),
    }
    if let Err(e) = launcher_store::save(&store) {
        tracing::warn!(error = %e, "launcher_store: save failed");
    }
    next.set(LauncherState::ServerSelect);
}

fn spawn_field(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    initial: &str,
    placeholder: &str,
    binding: ServerEditField,
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
                    width: Val::Px(140.0),
                    ..default()
                },
                Text::new(label.to_string()),
                ThemedText,
            ));
            row.spawn(text_field(TextFieldProps {
                initial: initial.to_string(),
                placeholder: placeholder.to_string(),
                submit_on_enter: false,
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
                move |ev: On<ValueChange<String>>, mut form: ResMut<ServerEditForm>| match binding {
                    ServerEditField::Name => form.name = ev.value.clone(),
                    ServerEditField::Host => form.host = ev.value.clone(),
                    ServerEditField::AuthPort => {
                        if ev.value.chars().all(|c| c.is_ascii_digit()) {
                            form.auth_port = ev.value.clone();
                        }
                    }
                    ServerEditField::DataPort => {
                        if ev.value.chars().all(|c| c.is_ascii_digit()) {
                            form.data_port = ev.value.clone();
                        }
                    }
                    ServerEditField::ViewPort => {
                        if ev.value.chars().all(|c| c.is_ascii_digit()) {
                            form.view_port = ev.value.clone();
                        }
                    }
                    ServerEditField::XiloaderVersion => {
                        form.xiloader_version = ev.value.clone();
                    }
                    ServerEditField::VersionCheckUrl => {
                        form.version_check_url = ev.value.clone();
                    }
                    ServerEditField::Flavor => {}
                },
            );
        });
}

fn spawn_flavor_button(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    kind: AuthFlavorKind,
    current: AuthFlavorKind,
) {
    let variant = if kind == current {
        ButtonVariant::Primary
    } else {
        ButtonVariant::Normal
    };
    parent
        .spawn((button(
            ButtonProps {
                variant,
                ..default()
            },
            FlavorButton(kind),
            Spawn((Text::new(label.to_string()), ThemedText)),
        ),))
        .observe(move |_ev: On<Activate>, mut form: ResMut<ServerEditForm>| {
            form.flavor = kind;
        });
}

pub(super) fn redraw_flavor_buttons(
    form: Res<ServerEditForm>,
    q: Query<(Entity, &FlavorButton)>,
    mut commands: Commands,
) {
    if !form.is_changed() {
        return;
    }
    for (e, fb) in q.iter() {
        let v = if fb.0 == form.flavor {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Normal
        };
        commands.entity(e).insert(v);
    }
}

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<ServerEditRoot>>) {
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

pub(super) fn redraw_system(
    form: Res<ServerEditForm>,
    q: Query<(Entity, &FlavorButton)>,
    commands: Commands,
) {
    redraw_flavor_buttons(form, q, commands);
}
