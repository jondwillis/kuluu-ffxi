//! Add/edit a `ServerProfile`. Enter saves and returns to `ServerSelect`;
//! Esc cancels.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;

use ffxi_client::launcher_store::{self, AuthFlavorKind, ServerProfile};

use super::{LauncherState, ServerEditField, ServerEditForm};

#[derive(Component)]
pub(super) struct ServerEditRoot;

#[derive(Component)]
pub(super) struct ServerEditBodyText;

pub(super) fn spawn_ui(mut commands: Commands, form: Res<ServerEditForm>) {
    commands
        .spawn((
            ServerEditRoot,
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
                Text::new(if form.editing_index.is_some() {
                    "Edit server"
                } else {
                    "New server"
                }),
                TextFont {
                    font_size: 22.0,
                    ..default()
                },
                TextColor(Color::srgb(0.0, 1.0, 1.0)),
            ));
            parent.spawn((
                ServerEditBodyText,
                Text::new(format_body(&form)),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(Color::srgb(0.95, 0.95, 0.95)),
            ));
            parent.spawn((
                Text::new("Tab: next field   Space: toggle flavor   Enter: save   Esc: cancel"),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgb(0.5, 0.5, 0.5)),
            ));
        });
}

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<ServerEditRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    mut form: ResMut<ServerEditForm>,
    mut next_state: ResMut<NextState<LauncherState>>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &ev.logical_key {
            Key::Escape => {
                next_state.set(LauncherState::ServerSelect);
                return;
            }
            Key::Tab => form.focus = form.focus.next(),
            Key::Enter => {
                if form.name.is_empty() || form.host.is_empty() {
                    continue;
                }
                let auth_port = form.auth_port.parse().unwrap_or(0);
                let data_port = form.data_port.parse().unwrap_or(0);
                let view_port = form.view_port.parse().unwrap_or(0);
                if auth_port == 0 || data_port == 0 || view_port == 0 {
                    continue;
                }
                let profile = ServerProfile {
                    name: form.name.clone(),
                    host: form.host.clone(),
                    auth_port,
                    data_port,
                    view_port,
                    flavor: form.flavor,
                };
                let mut store = launcher_store::load();
                match form.editing_index {
                    Some(idx) if idx < store.servers.len() => store.servers[idx] = profile,
                    _ => store.servers.push(profile),
                }
                if let Err(e) = launcher_store::save(&store) {
                    tracing::warn!(error = %e, "launcher_store: save failed");
                }
                next_state.set(LauncherState::ServerSelect);
                return;
            }
            Key::Backspace => match form.focus {
                ServerEditField::Name => {
                    form.name.pop();
                }
                ServerEditField::Host => {
                    form.host.pop();
                }
                ServerEditField::AuthPort => {
                    form.auth_port.pop();
                }
                ServerEditField::DataPort => {
                    form.data_port.pop();
                }
                ServerEditField::ViewPort => {
                    form.view_port.pop();
                }
                ServerEditField::Flavor => {}
            },
            Key::Space => {
                if form.focus == ServerEditField::Flavor {
                    form.flavor = match form.flavor {
                        AuthFlavorKind::Json => AuthFlavorKind::Binary,
                        AuthFlavorKind::Binary => AuthFlavorKind::Json,
                    };
                }
            }
            Key::Character(s) => {
                for c in s.chars() {
                    if c.is_control() {
                        continue;
                    }
                    match form.focus {
                        ServerEditField::Name => form.name.push(c),
                        ServerEditField::Host => form.host.push(c),
                        ServerEditField::AuthPort if c.is_ascii_digit() => {
                            form.auth_port.push(c)
                        }
                        ServerEditField::DataPort if c.is_ascii_digit() => {
                            form.data_port.push(c)
                        }
                        ServerEditField::ViewPort if c.is_ascii_digit() => {
                            form.view_port.push(c)
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

pub(super) fn redraw_system(
    form: Res<ServerEditForm>,
    mut q: Query<&mut Text, With<ServerEditBodyText>>,
) {
    if !form.is_changed() {
        return;
    }
    for mut t in q.iter_mut() {
        **t = format_body(&form);
    }
}

fn format_body(form: &ServerEditForm) -> String {
    let mark = |f: ServerEditField| if form.focus == f { ">" } else { " " };
    format!(
        "{n} Name:      {name}\n{h} Host:      {host}\n{a} Auth port: {ap}\n{d} Data port: {dp}\n{v} View port: {vp}\n{fl} Flavor:    {flavor:?}",
        n = mark(ServerEditField::Name),
        name = form.name,
        h = mark(ServerEditField::Host),
        host = form.host,
        a = mark(ServerEditField::AuthPort),
        ap = form.auth_port,
        d = mark(ServerEditField::DataPort),
        dp = form.data_port,
        v = mark(ServerEditField::ViewPort),
        vp = form.view_port,
        fl = mark(ServerEditField::Flavor),
        flavor = form.flavor,
    )
}
