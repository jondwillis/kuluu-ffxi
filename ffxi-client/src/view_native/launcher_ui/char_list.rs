//! Character-select screen — one feathers button per slot, plus global
//! action buttons (+ New character, Delete selected, Back to login).
//!
//! The 3D PC preview (see `char_preview.rs`) reads `CharCursor` to know
//! which slot to render. We update `CharCursor` on `Pointer<Over>` so
//! hovering a row still drives the preview, while clicking commits.

use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button, ButtonProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::picking::events::{Over, Pointer};
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use super::common::{hint, panel_node, row, screen_root, spawn_breadcrumb, title, Crumb};
use super::{CharListData, Credentials, DefaultCharName, LauncherState, SelectedChar, ServerInfo};

#[derive(Component)]
pub(super) struct CharListRoot;

/// Tracks which row the highlight + 3D preview points at. Range:
/// `0..=chars.len()` (final index is the "+ New character" row).
#[derive(Resource, Default)]
pub(crate) struct CharCursor(pub usize);

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

    // Right-anchored column so the 3D preview centered at world origin
    // isn't covered by the panel. No background on the root — the
    // backdrop scene shows through.
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
                panel.spawn(title("Select character"));
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
                    panel.spawn(row()).with_children(|r| {
                        // Wrapper grows so the per-row [x] sits flush
                        // right. See feedback memory: feathers' button
                        // panics on a Node override, so layout goes on
                        // an outer Node wrapper.
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
                        });

                        // Per-row delete — goes straight to the
                        // existing CharDeleteConfirm state, which IS
                        // the confirm step. No arm-then-confirm
                        // needed here (unlike ServerSelect's inline x)
                        // since the dedicated confirm screen already
                        // gates the destructive action.
                        r.spawn(button(
                            ButtonProps::default(),
                            (),
                            Spawn((Text::new("×"), ThemedText)),
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
                }

                panel.spawn(row()).with_children(|r| {
                    r.spawn(button(
                        ButtonProps {
                            variant: ButtonVariant::Primary,
                            ..default()
                        },
                        (),
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

                    r.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new("Back to login"), ThemedText)),
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

pub(super) fn despawn_char_list_ui(mut commands: Commands, q: Query<Entity, With<CharListRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
    commands.remove_resource::<CharCursor>();
}

/// Esc returns to login. All other navigation is via buttons now.
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

/// Legacy entrypoint retained as a no-op so `mod.rs::register`'s tuple
/// shape doesn't have to change. All click handling now lives on the
/// per-button `Activate` observers spawned in `spawn_char_list_ui`.
pub(super) fn handle_click_system() {}

// --- Char delete confirm --------------------------------------------------

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
                        font_size: 22.0,
                        ..default()
                    },
                    TextColor(Color::srgb(0.95, 0.20, 0.20)),
                    ThemedText,
                ));
                panel.spawn(hint(
                    "This is destructive and cannot be undone server-side.",
                ));

                panel.spawn(row()).with_children(|r| {
                    r.spawn(button(
                        ButtonProps {
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

                    r.spawn(button(
                        ButtonProps::default(),
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
