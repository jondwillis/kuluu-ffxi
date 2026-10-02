use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button_bundle, ButtonBundleProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::input_focus::{FocusCause, InputFocus, InputFocusVisible};
use bevy::picking::events::{Over, Pointer};
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use super::common::{
    chip_group, hint, panel_node, row, screen_root, spawn_back_titlebar, spawn_breadcrumb, Crumb,
    DefaultFocusTarget,
};
use super::{
    CharListData, Credentials, DefaultCharName, LauncherState, OpenedLobby, SelectedChar,
    ServerInfo,
};

fn title_case(name: &str) -> String {
    name.split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_ascii_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The lobby's 0x05 expansion mask
/// (vendor/server/src/login/view_session.cpp), worded for the character
/// screen.
fn expansions_line(names: &[&str]) -> Option<String> {
    let listed: Vec<String> = names
        .iter()
        .filter(|n| **n != "BASE_GAME")
        .map(|n| title_case(n))
        .collect();
    (!listed.is_empty()).then(|| format!("Server expansions: {}", listed.join(", ")))
}

/// Keeps the character panel off the right edge so the backdrop flythrough
/// stays visible beside it.
const CHAR_LIST_RIGHT_INSET_PX: f32 = 40.0;

#[derive(Component)]
pub(super) struct CharListRoot;

#[derive(Resource, Default)]
pub(crate) struct CharCursor(pub usize);

#[derive(Component)]
pub(super) struct CharRowButton(pub usize);

/// Spawns the character list screen. Hover sets the cursor but leaves
/// `InputFocusVisible` alone: hover shares the selection model without
/// painting a focus ring.
pub(super) fn spawn_char_list_ui(
    mut commands: Commands,
    chars: Res<CharListData>,
    default_name: Res<DefaultCharName>,
    server: Res<ServerInfo>,
    creds: Res<Credentials>,
    opened: Res<OpenedLobby>,
) {
    let expansions = opened
        .0
        .lock()
        .ok()
        .and_then(|slot| slot.handle.as_ref().map(|h| h.key().expansion_names()))
        .and_then(|names| expansions_line(&names));
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
                padding: UiRect::new(
                    Val::ZERO,
                    Val::Px(CHAR_LIST_RIGHT_INSET_PX),
                    Val::ZERO,
                    Val::Px(super::footer::FOOTER_RESERVED_PX),
                ),
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
                if let Some(line) = expansions.as_deref() {
                    panel.spawn(hint(line.to_string()));
                }
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
                            .insert_if(DefaultFocusTarget, || idx == initial_cursor)
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
                                move |ev: On<Pointer<Over>>,
                                      mut cursor: ResMut<CharCursor>,
                                      mut focus: ResMut<InputFocus>| {
                                    if cursor.0 != idx {
                                        cursor.0 = idx;
                                    }
                                    focus.set(ev.entity, FocusCause::Pressed);
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
                    .insert_if(DefaultFocusTarget, || new_char_index == initial_cursor)
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

/// Arrow keys move `InputFocus`, not [`CharCursor`] - the cursor follows via
/// [`sync_cursor_to_focus_system`], so the focus ring, the Primary-variant row
/// and the 3D preview cannot disagree whether the keyboard, the pad or the
/// mouse drove the move.
/// Keyboard navigation for the character list. Enter activates only when
/// focus rests outside the rows: a focused row already activates itself
/// through its own `Activate` observer, and this arm is the fallback.
pub(super) fn keyboard_nav_system(
    mut events: MessageReader<KeyboardInput>,
    chars: Res<CharListData>,
    cursor: Res<CharCursor>,
    mut focus: ResMut<InputFocus>,
    mut visible: ResMut<InputFocusVisible>,
    q_rows: Query<(Entity, &CharRowButton)>,
    mut sel: ResMut<SelectedChar>,
    mut next: ResMut<NextState<LauncherState>>,
) {
    let count = chars.0.len() + 1;
    let step = |delta: usize, focus: &mut InputFocus, visible: &mut InputFocusVisible| {
        let target = (cursor.0 + delta) % count;
        if let Some((e, _)) = q_rows.iter().find(|(_, row)| row.0 == target) {
            focus.set(e, FocusCause::Navigated);
            visible.0 = true;
        }
    };
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &ev.logical_key {
            Key::ArrowUp => step(count - 1, &mut focus, &mut visible),
            Key::ArrowDown => step(1, &mut focus, &mut visible),
            Key::Character(s) if s.eq_ignore_ascii_case("w") => {
                step(count - 1, &mut focus, &mut visible)
            }
            Key::Character(s) if s.eq_ignore_ascii_case("s") => step(1, &mut focus, &mut visible),
            Key::Enter if !focus.get().is_some_and(|e| q_rows.contains(e)) => {
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

/// The row highlight and the 3D preview key off [`CharCursor`], not
/// `InputFocus`, so the pad's focus ring has to drag the cursor along - the
/// same thing the `Pointer<Over>` observer does for the mouse. Hover stays
/// authoritative between focus changes.
pub(super) fn sync_cursor_to_focus_system(
    focus: Res<InputFocus>,
    q_rows: Query<&CharRowButton>,
    cursor: Option<ResMut<CharCursor>>,
    mut last: Local<Option<Entity>>,
) {
    let Some(mut cursor) = cursor else {
        return;
    };
    let current = focus.get();
    if *last == current {
        return;
    }
    *last = current;
    let Some(e) = current else {
        return;
    };
    let Ok(row) = q_rows.get(e) else {
        return;
    };
    if cursor.0 != row.0 {
        cursor.0 = row.0;
    }
}

pub(super) fn handle_click_system() {}

#[derive(Component)]
pub(super) struct DeleteConfirmRoot;

/// Spawns the delete-character confirm screen. The focus ring starts on
/// Cancel, not on the destructive action: a stray pad Confirm must not
/// delete a character.
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
                        DefaultFocusTarget,
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input_focus::FocusCause;

    /// A mouse hover that moved the cursor must not be undone by an
    /// unchanged focus.
    #[test]
    fn focused_char_row_moves_the_cursor() {
        let mut app = App::new();
        app.init_resource::<InputFocus>();
        app.insert_resource(CharCursor(0));
        app.add_systems(Update, sync_cursor_to_focus_system);
        let row0 = app.world_mut().spawn(CharRowButton(0)).id();
        let row1 = app.world_mut().spawn(CharRowButton(1)).id();
        let other = app.world_mut().spawn_empty().id();

        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(row1, FocusCause::Navigated);
        app.update();
        assert_eq!(app.world().resource::<CharCursor>().0, 1);

        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(other, FocusCause::Navigated);
        app.update();
        assert_eq!(app.world().resource::<CharCursor>().0, 1);

        app.world_mut().resource_mut::<CharCursor>().0 = 0;
        app.update();
        assert_eq!(app.world().resource::<CharCursor>().0, 0);
        assert!(app.world().get::<CharRowButton>(row0).is_some());
    }

    #[test]
    fn expansions_line_titles_names_and_hides_the_base_game_bit() {
        assert_eq!(
            expansions_line(&["BASE_GAME", "RISE_OF_ZILART", "CHAINS_OF_PROMATHIA"]).as_deref(),
            Some("Server expansions: Rise Of Zilart, Chains Of Promathia")
        );
        assert_eq!(expansions_line(&["BASE_GAME"]), None);
        assert_eq!(expansions_line(&[]), None);
    }
}
