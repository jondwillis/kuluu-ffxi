use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button_bundle, ButtonBundleProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::picking::events::{Over, Pointer};
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use super::common::{
    chip_group, hint, panel_node, row, screen_root, spawn_back_titlebar, spawn_breadcrumb, Crumb,
};
use super::{CharListData, Credentials, DefaultCharName, LauncherState, SelectedChar, ServerInfo};

#[derive(Component)]
pub(super) struct CharListRoot;

#[derive(Resource, Default)]
pub(crate) struct CharCursor(pub usize);

#[derive(Component)]
pub(super) struct CharRowButton(pub usize);

pub(super) fn spawn_char_list_ui(
    mut commands: Commands,
    chars: Res<CharListData>,
    default_name: Res<DefaultCharName>,
    server: Res<ServerInfo>,
    creds: Res<Credentials>,
) {
    let new_char_index = chars.0.len();
    let initial_cursor = default_name
        .0
        .as_deref()
        .and_then(|want| chars.0.iter().position(|c| c.name == want))
        .unwrap_or_else(|| {
            if chars.0.is_empty() {
                new_char_index
            } else {
                0
            }
        });
    commands.insert_resource(CharCursor(initial_cursor));

    commands
        .spawn((
            CharListRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::FlexEnd,
                row_gap: Val::Px(8.0),
                padding: UiRect::right(Val::Px(40.0)),
                ..default()
            },
        ))
        .with_children(|root| {
            let sign_label = if creds.user.is_empty() {
                None
            } else {
                Some(creds.user.clone())
            };
            spawn_breadcrumb(root, &server, &[Crumb::Sign(sign_label), Crumb::Characters]);
            root.spawn(panel_node(420.0)).with_children(|panel| {
                spawn_back_titlebar(panel, "Select character");
                if chars.0.is_empty() {
                    panel.spawn(hint("No characters on this account yet."));
                }

                for (idx, slot) in chars.0.iter().enumerate() {
                    let label = format!("[{}] {}  (id {})", idx + 1, slot.name, slot.char_id);
                    let variant = if idx == initial_cursor {
                        ButtonVariant::Primary
                    } else {
                        ButtonVariant::Normal
                    };
                    // One visually-connected pill: select chip + Delete.
                    panel.spawn(row()).with_children(|r| {
                        r.spawn(chip_group()).with_children(|chip| {
                            chip.spawn(button_bundle(
                                ButtonBundleProps {
                                    variant,
                                    ..default()
                                },
                                CharRowButton(idx),
                                Spawn((Text::new(label), ThemedText)),
                            ))
                            .observe(
                                move |_ev: On<Activate>,
                                      chars: Res<CharListData>,
                                      mut cursor: ResMut<CharCursor>,
                                      mut sel: ResMut<SelectedChar>,
                                      mut next: ResMut<NextState<LauncherState>>| {
                                    cursor.0 = idx;
                                    if let Some(slot) = chars.0.get(idx).cloned() {
                                        sel.0 = Some(slot);
                                        next.set(LauncherState::ConnectInFlight);
                                    }
                                },
                            )
                            .observe(
                                move |_ev: On<Pointer<Over>>, mut cursor: ResMut<CharCursor>| {
                                    if cursor.0 != idx {
                                        cursor.0 = idx;
                                    }
                                },
                            );

                            chip.spawn(button_bundle(
                                ButtonBundleProps::default(),
                                (),
                                Spawn((Text::new("Delete"), ThemedText)),
                            ))
                            .observe(
                                move |_ev: On<Activate>,
                                      chars: Res<CharListData>,
                                      mut cursor: ResMut<CharCursor>,
                                      mut sel: ResMut<SelectedChar>,
                                      mut next: ResMut<NextState<LauncherState>>| {
                                    cursor.0 = idx;
                                    if let Some(slot) = chars.0.get(idx).cloned() {
                                        sel.0 = Some(slot);
                                        next.set(LauncherState::CharDeleteConfirm);
                                    }
                                },
                            );
                        });
                    });
                }

                panel.spawn(row()).with_children(|r| {
                    let new_variant = if new_char_index == initial_cursor {
                        ButtonVariant::Primary
                    } else {
                        ButtonVariant::Normal
                    };
                    r.spawn(button_bundle(
                        ButtonBundleProps {
                            variant: new_variant,
                            ..default()
                        },
                        CharRowButton(new_char_index),
                        Spawn((Text::new("+ New character"), ThemedText)),
                    ))
                    .observe(
                        move |_ev: On<Activate>,
                              mut cursor: ResMut<CharCursor>,
                              mut next: ResMut<NextState<LauncherState>>| {
                            cursor.0 = new_char_index;
                            next.set(LauncherState::CharCreate);
                        },
                    );
                });
            });
        });
}

pub(super) fn despawn_char_list_ui(mut commands: Commands, q: Query<Entity, With<CharListRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
    commands.remove_resource::<CharCursor>();
}

pub(super) fn handle_keyboard_system(
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

pub(super) fn keyboard_nav_system(
    mut events: MessageReader<KeyboardInput>,
    chars: Res<CharListData>,
    mut cursor: ResMut<CharCursor>,
    mut sel: ResMut<SelectedChar>,
    mut next: ResMut<NextState<LauncherState>>,
) {
    let count = chars.0.len() + 1;
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &ev.logical_key {
            Key::ArrowUp => cursor.0 = (cursor.0 + count - 1) % count,
            Key::ArrowDown => cursor.0 = (cursor.0 + 1) % count,
            Key::Character(s) if s.eq_ignore_ascii_case("w") => {
                cursor.0 = (cursor.0 + count - 1) % count
            }
            Key::Character(s) if s.eq_ignore_ascii_case("s") => cursor.0 = (cursor.0 + 1) % count,
            Key::Enter => {
                if cursor.0 == chars.0.len() {
                    next.set(LauncherState::CharCreate);
                } else if let Some(slot) = chars.0.get(cursor.0).cloned() {
                    sel.0 = Some(slot);
                    next.set(LauncherState::ConnectInFlight);
                }
                return;
            }
            _ => {}
        }
    }
}

pub(super) fn redraw_char_list_system(
    cursor: Res<CharCursor>,
    q_buttons: Query<(Entity, &CharRowButton)>,
    mut commands: Commands,
) {
    if !cursor.is_changed() {
        return;
    }
    for (e, btn) in q_buttons.iter() {
        let v = if btn.0 == cursor.0 {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Normal
        };
        commands.entity(e).insert(v);
    }
}

pub(super) fn handle_click_system() {}

#[derive(Component)]
pub(super) struct DeleteConfirmRoot;

pub(super) fn spawn_delete_confirm_ui(mut commands: Commands, sel: Res<SelectedChar>) {
    let name = sel
        .0
        .as_ref()
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "?".into());
    commands
        .spawn((DeleteConfirmRoot, screen_root()))
        .with_children(|root| {
            root.spawn(panel_node(480.0)).with_children(|panel| {
                panel.spawn((
                    Text::new(format!("Delete character '{name}'?")),
                    TextFont {
                        font_size: 22.0.into(),
                        ..default()
                    },
                    TextColor(Color::srgb(0.95, 0.20, 0.20)),
                    ThemedText,
                ));
                panel.spawn(hint(
                    "This is destructive and cannot be undone server-side.",
                ));

                panel.spawn(row()).with_children(|r| {
                    r.spawn(button_bundle(
                        ButtonBundleProps {
                            variant: ButtonVariant::Primary,
                            ..default()
                        },
                        (),
                        Spawn((Text::new("Confirm delete"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                            next.set(LauncherState::CharDeleteInFlight);
                        },
                    );

                    r.spawn(button_bundle(
                        ButtonBundleProps::default(),
                        (),
                        Spawn((Text::new("Cancel"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                            next.set(LauncherState::CharList);
                        },
                    );
                });
            });
        });
}

pub(super) fn despawn_delete_confirm_ui(
    mut commands: Commands,
    q: Query<Entity, With<DeleteConfirmRoot>>,
) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

pub(super) fn delete_confirm_keyboard_system(
    mut events: MessageReader<KeyboardInput>,
    mut next_state: ResMut<NextState<LauncherState>>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if matches!(ev.logical_key, Key::Escape) {
            next_state.set(LauncherState::CharList);
            return;
        }
    }
}
