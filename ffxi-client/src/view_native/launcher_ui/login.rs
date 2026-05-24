//! Login screen — feathers-based buttons + TextFields.

use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button, checkbox, ButtonProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::{Activate, ValueChange};

use super::common::{hint, panel_node, row, screen_root, title};
use crate::view_native::widgets::text_field::{text_field, TextFieldSubmitted};
use crate::view_native::widgets::{TextFieldDisplay, TextFieldProps};
use super::{Credentials, LauncherState, LoginErrorMsg, LoginField, LoginForm, ServerInfo};

#[derive(Component)]
pub(super) struct LoginUiRoot;

pub(super) fn spawn_login_ui(
    mut commands: Commands,
    server: Res<ServerInfo>,
    form: Res<LoginForm>,
) {
    let user_initial = form.user.clone();
    let pass_initial = form.pass.clone();
    let remember = form.remember_password;
    let server_name = server.server.clone();

    commands
        .spawn((LoginUiRoot, screen_root()))
        .with_children(|root| {
            root.spawn(panel_node(480.0)).with_children(|panel| {
                panel.spawn(title(format!("FFXI launcher — {server_name}")));
                panel.spawn(hint("Tab cycles fields. Enter submits when both filled."));

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
                });

                panel.spawn(row()).with_children(|r| {
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

                    r.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new("Server select"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                            next.set(LauncherState::ServerSelect);
                        },
                    );

                    r.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new("Forget saved"), ThemedText)),
                    ))
                    .observe(|_ev: On<Activate>, mut form: ResMut<LoginForm>| {
                        form.user.clear();
                        form.pass.clear();
                        form.remember_password = false;
                    });
                });
            });
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
                move |ev: On<ValueChange<String>>, mut form: ResMut<LoginForm>| {
                    match binding {
                        LoginField::User => form.user = ev.value.clone(),
                        LoginField::Password => form.pass = ev.value.clone(),
                    }
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
