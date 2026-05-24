//! Account-creation screen — feathers buttons + TextFields.

use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button, ButtonProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, ValueChange};

use super::common::{hint, panel_node, row, screen_root, spawn_server_chip, title};
use crate::view_native::widgets::text_field::{text_field, TextFieldSubmitted};
use crate::view_native::widgets::{TextFieldDisplay, TextFieldProps};

use super::{CreateAccountErrorMsg, CreateAccountField, CreateAccountForm, LauncherState, ServerInfo};

#[derive(Component)]
pub(super) struct CreateAccountRoot;

#[derive(Component)]
pub(super) struct StatusText;

pub(super) fn spawn_ui(
    mut commands: Commands,
    form: Res<CreateAccountForm>,
    server: Res<ServerInfo>,
) {
    let u0 = form.user.clone();
    let p0 = form.pass.clone();
    let c0 = form.pass_confirm.clone();
    let initial_msg = form.validation_msg().unwrap_or_default();

    commands
        .spawn((CreateAccountRoot, screen_root()))
        .with_children(|root| {
            spawn_server_chip(root, &server);
            root.spawn(panel_node(480.0)).with_children(|panel| {
                panel.spawn(title("Create account"));
                panel.spawn(hint("Tab cycles fields. Esc cancels back to login."));

                spawn_field(panel, "Username", false, &u0, CreateAccountField::User);
                spawn_field(panel, "Password", true, &p0, CreateAccountField::Password);
                spawn_field(
                    panel,
                    "Confirm",
                    true,
                    &c0,
                    CreateAccountField::PasswordConfirm,
                );

                panel.spawn((
                    StatusText,
                    Text::new(initial_msg),
                    TextFont {
                        font_size: 13.0,
                        ..default()
                    },
                    TextColor(Color::srgb(0.95, 0.55, 0.30)),
                    ThemedText,
                ));

                panel.spawn(row()).with_children(|r| {
                    r.spawn(button(
                        ButtonProps {
                            variant: ButtonVariant::Primary,
                            ..default()
                        },
                        (),
                        Spawn((Text::new("Create"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>,
                         form: Res<CreateAccountForm>,
                         mut err: ResMut<CreateAccountErrorMsg>,
                         mut next: ResMut<NextState<LauncherState>>| {
                            if form.validation_msg().is_none() {
                                err.0.clear();
                                next.set(LauncherState::CreateAccountInFlight);
                            }
                        },
                    );

                    r.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new("Cancel"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>,
                         mut err: ResMut<CreateAccountErrorMsg>,
                         mut next: ResMut<NextState<LauncherState>>| {
                            err.0.clear();
                            next.set(LauncherState::Login);
                        },
                    );
                });
            });
        });
}

fn spawn_field(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    mask: bool,
    initial: &str,
    binding: CreateAccountField,
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
                move |ev: On<ValueChange<String>>, mut form: ResMut<CreateAccountForm>| {
                    match binding {
                        CreateAccountField::User => form.user = ev.value.clone(),
                        CreateAccountField::Password => form.pass = ev.value.clone(),
                        CreateAccountField::PasswordConfirm => {
                            form.pass_confirm = ev.value.clone()
                        }
                    }
                },
            )
            .observe(
                |_ev: On<TextFieldSubmitted>,
                 form: Res<CreateAccountForm>,
                 mut err: ResMut<CreateAccountErrorMsg>,
                 mut next: ResMut<NextState<LauncherState>>| {
                    if form.validation_msg().is_none() {
                        err.0.clear();
                        next.set(LauncherState::CreateAccountInFlight);
                    }
                },
            );
        });
}

pub(super) fn despawn_ui(
    mut commands: Commands,
    q: Query<Entity, With<CreateAccountRoot>>,
    mut form: ResMut<CreateAccountForm>,
) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
    form.pass_confirm.clear();
}

pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    mut err: ResMut<CreateAccountErrorMsg>,
    mut next: ResMut<NextState<LauncherState>>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if matches!(ev.logical_key, Key::Escape) {
            err.0.clear();
            next.set(LauncherState::Login);
            return;
        }
    }
}

/// Status hint follows live validation state.
pub(super) fn redraw_form_system(
    form: Res<CreateAccountForm>,
    mut q_status: Query<&mut Text, With<StatusText>>,
) {
    if !form.is_changed() {
        return;
    }
    for mut t in q_status.iter_mut() {
        **t = form.validation_msg().unwrap_or_default();
    }
}

// --- CreateAccountError state ---------------------------------------------

#[derive(Component)]
pub(super) struct CreateAccountErrorRoot;

pub(super) fn spawn_error_ui(mut commands: Commands, msg: Res<CreateAccountErrorMsg>) {
    let body = msg.0.clone();
    commands
        .spawn((CreateAccountErrorRoot, screen_root()))
        .with_children(|root| {
            root.spawn(panel_node(520.0)).with_children(|panel| {
                panel.spawn((
                    Text::new("Account creation failed"),
                    TextFont {
                        font_size: 22.0,
                        ..default()
                    },
                    TextColor(Color::srgb(0.95, 0.30, 0.30)),
                    ThemedText,
                ));
                panel.spawn((
                    Text::new(body),
                    TextFont {
                        font_size: 13.0,
                        ..default()
                    },
                    TextColor(Color::srgb(0.85, 0.85, 0.85)),
                    ThemedText,
                ));
                panel.spawn(row()).with_children(|r| {
                    r.spawn(button(
                        ButtonProps {
                            variant: ButtonVariant::Primary,
                            ..default()
                        },
                        (),
                        Spawn((Text::new("Try again"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                            next.set(LauncherState::CreateAccountInFlight);
                        },
                    );
                    r.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new("Back to form"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                            next.set(LauncherState::CreateAccount);
                        },
                    );
                });
            });
        });
}

pub(super) fn despawn_error_ui(
    mut commands: Commands,
    q: Query<Entity, With<CreateAccountErrorRoot>>,
) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

pub(super) fn error_keyboard_system(
    mut events: MessageReader<KeyboardInput>,
    mut next_state: ResMut<NextState<LauncherState>>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if matches!(ev.logical_key, Key::Escape) {
            next_state.set(LauncherState::CreateAccount);
            return;
        }
    }
}
