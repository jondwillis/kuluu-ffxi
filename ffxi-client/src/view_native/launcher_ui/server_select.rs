//! Server-select screen — picks a persisted `ServerProfile` from
//! `LauncherStore`. Arrow keys navigate, Enter picks, Ctrl-N adds new,
//! Ctrl-E edits selected, Delete removes selected. Esc skips to Login
//! with whatever active client `view_native::run` built from CLI args.
//!
//! Note: picking a server here only updates `last_used` for the next
//! launch and prefills the account picker — it does NOT swap the live
//! `AuthClient`/`LobbyClient`, since rebuilding those mid-launch isn't
//! plumbed through `LauncherClients` yet.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;

use ffxi_client::launcher_store::{self, LauncherStore};

use super::{LauncherState, ServerSelectCursor, ServerSelectForm};

#[derive(Component)]
pub(super) struct ServerSelectRoot;

#[derive(Component)]
pub(super) struct ServerListText;

pub(super) fn spawn_ui(mut commands: Commands, cursor: Res<ServerSelectCursor>) {
    let store = launcher_store::load();
    let body = format_body(&store, cursor.0);
    commands
        .spawn((
            ServerSelectRoot,
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
                Text::new("Select server"),
                TextFont {
                    font_size: 22.0,
                    ..default()
                },
                TextColor(Color::srgb(0.0, 1.0, 1.0)),
            ));
            parent.spawn((
                ServerListText,
                Text::new(body),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(Color::srgb(0.95, 0.95, 0.95)),
            ));
            parent.spawn((
                Text::new(
                    "↑/↓ select   Enter pick   Ctrl-N new   Ctrl-E edit   Del remove   Esc skip to login",
                ),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgb(0.5, 0.5, 0.5)),
            ));
        });
}

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<ServerSelectRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mut cursor: ResMut<ServerSelectCursor>,
    mut next_state: ResMut<NextState<LauncherState>>,
    mut form: ResMut<ServerSelectForm>,
    mut edit: ResMut<super::ServerEditForm>,
    mut q_body: Query<&mut Text, With<ServerListText>>,
) {
    let ctrl = keys.pressed(KeyCode::ControlLeft)
        || keys.pressed(KeyCode::ControlRight)
        || keys.pressed(KeyCode::SuperLeft)
        || keys.pressed(KeyCode::SuperRight);

    let mut store = launcher_store::load();
    let n = store.servers.len();
    let mut dirty = false;
    let mut needs_redraw = false;

    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if ctrl {
            if let Key::Character(s) = &ev.logical_key {
                if s.eq_ignore_ascii_case("n") {
                    *edit = super::ServerEditForm::default();
                    edit.editing_index = None;
                    next_state.set(LauncherState::ServerEdit);
                    return;
                }
                if s.eq_ignore_ascii_case("e") && n > 0 {
                    let idx = cursor.0.min(n - 1);
                    *edit = super::ServerEditForm::from_profile(&store.servers[idx]);
                    edit.editing_index = Some(idx);
                    next_state.set(LauncherState::ServerEdit);
                    return;
                }
            }
        }
        match &ev.logical_key {
            Key::ArrowUp if n > 0 => {
                cursor.0 = (cursor.0 + n - 1) % n;
                needs_redraw = true;
            }
            Key::ArrowDown if n > 0 => {
                cursor.0 = (cursor.0 + 1) % n;
                needs_redraw = true;
            }
            Key::Enter if n > 0 => {
                let idx = cursor.0.min(n - 1);
                form.selected = Some(store.servers[idx].name.clone());
                next_state.set(LauncherState::AccountPicker);
                return;
            }
            Key::Delete if n > 0 => {
                let idx = cursor.0.min(n - 1);
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
                dirty = true;
                needs_redraw = true;
            }
            Key::Escape => {
                next_state.set(LauncherState::Login);
                return;
            }
            _ => {}
        }
    }
    if dirty {
        if let Err(e) = launcher_store::save(&store) {
            tracing::warn!(error = %e, "launcher_store: save failed");
        }
    }
    if needs_redraw {
        for mut t in q_body.iter_mut() {
            **t = format_body(&store, cursor.0);
        }
    }
}

fn format_body(store: &LauncherStore, cursor: usize) -> String {
    if store.servers.is_empty() {
        return "(no servers — press Ctrl-N to add one, or Esc to use CLI defaults)".into();
    }
    let mut out = String::new();
    for (i, s) in store.servers.iter().enumerate() {
        let marker = if i == cursor { ">" } else { " " };
        out.push_str(&format!(
            "{marker} {name}  ({host}:{auth}/{data}/{view} {flavor:?})\n",
            name = s.name,
            host = s.host,
            auth = s.auth_port,
            data = s.data_port,
            view = s.view_port,
            flavor = s.flavor,
        ));
    }
    out
}
