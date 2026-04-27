use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button, ButtonProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, ValueChange};

use super::common::{hint, panel_node, row, screen_root, spawn_breadcrumb, title, Crumb};
use crate::view_native::widgets::text_field::text_field;
use crate::view_native::widgets::{TextFieldDisplay, TextFieldProps};

use super::{ChangePasswordField, ChangePasswordForm, LauncherState, ServerInfo};

#[derive(Component)]
pub(super) struct ChangePasswordRoot;

#[derive(Component)]
pub(super) struct ChangePasswordBody;

pub(super) fn spawn_ui(
    mut commands: Commands,
    form: Res<ChangePasswordForm>,
    server: Res<ServerInfo>,
) {
    let o0 = form.old.clone();
    let n0 = form.new_pw.clone();
    let c0 = form.confirm.clone();
    let err0 = form.error.clone();

    commands
        .spawn((ChangePasswordRoot, screen_root()))
        .with_children(|root| {
            spawn_breadcrumb(
                root,
                &server,
                &[
                    Crumb::Sign(None),
                    Crumb::Other("Change password".to_string()),
                ],
            );
            root.spawn(panel_node(480.0)).with_children(|panel| {
                panel.spawn(title("Change password"));
                panel.spawn(hint("Tab cycles fields. Esc cancels."));

                spawn_field(panel, "Old", &o0, ChangePasswordField::Old);
                spawn_field(panel, "New", &n0, ChangePasswordField::New);
                spawn_field(panel, "Confirm", &c0, ChangePasswordField::Confirm);

                panel.spawn((
                    ChangePasswordBody,
                    Text::new(err0),
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
                        Spawn((Text::new("Change"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>,
                         mut form: ResMut<ChangePasswordForm>,
                         mut next: ResMut<NextState<LauncherState>>| {
                            submit(&mut form, &mut next);
                        },
                    );

                    r.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new("Cancel"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>,
                         mut form: ResMut<ChangePasswordForm>,
                         mut next: ResMut<NextState<LauncherState>>| {
                            form.old.clear();
                            form.new_pw.clear();
                            form.confirm.clear();
                            form.error.clear();
                            next.set(LauncherState::Login);
                        },
                    );
                });
            });
        });
}

fn submit(form: &mut ChangePasswordForm, next: &mut NextState<LauncherState>) {
    if form.new_pw.is_empty() || form.old.is_empty() {
        form.error = "Fill all fields.".into();
        return;
    }
    if form.new_pw != form.confirm {
        form.error = "Confirmation doesn't match.".into();
        return;
    }
    form.error.clear();
    next.set(LauncherState::ChangePasswordInFlight);
}

fn spawn_field(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    initial: &str,
    binding: ChangePasswordField,
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
                mask: true,
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
                move |ev: On<ValueChange<String>>, mut form: ResMut<ChangePasswordForm>| {
                    match binding {
                        ChangePasswordField::Old => form.old = ev.value.clone(),
                        ChangePasswordField::New => form.new_pw = ev.value.clone(),
                        ChangePasswordField::Confirm => form.confirm = ev.value.clone(),
                    }
                },
            );
        });
}

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<ChangePasswordRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    mut form: ResMut<ChangePasswordForm>,
    mut next: ResMut<NextState<LauncherState>>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if matches!(ev.logical_key, Key::Escape) {
            form.old.clear();
            form.new_pw.clear();
            form.confirm.clear();
            form.error.clear();
            next.set(LauncherState::Login);
            return;
        }
    }
}

pub(super) fn redraw_system(
    form: Res<ChangePasswordForm>,
    mut q: Query<&mut Text, With<ChangePasswordBody>>,
) {
    if !form.is_changed() {
        return;
    }
    for mut t in q.iter_mut() {
        **t = form.error.clone();
    }
}
