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

use super::{CharCreateError, CharCreateField, CharCreateForm, LauncherState, ServerInfo};

pub(super) const RACES: &[(u8, &str)] = &[
    (1, "Hume M"),
    (2, "Hume F"),
    (3, "Elv M"),
    (4, "Elv F"),
    (5, "Tar M"),
    (6, "Tar F"),
    (7, "Mithra"),
    (8, "Galka"),
];

pub(super) const JOBS: &[(u8, &str)] = &[
    (1, "WAR"),
    (2, "MNK"),
    (3, "WHM"),
    (4, "BLM"),
    (5, "RDM"),
    (6, "THF"),
];

pub(super) const NATIONS: &[(u8, &str)] = &[(0, "San d'Oria"), (1, "Bastok"), (2, "Windurst")];

pub(super) const SIZES: &[(u8, &str)] = &[(0, "Small"), (1, "Medium"), (2, "Large")];

// Retail picks Face (1-8) and Hair (A-B) as two separate menus; the wire `face`
// byte packs them as `face_index * 2 + hair`. Values here are the face index and
// hair bit; `CharCreateForm::set_field`/`field_selection` do the packing.
pub(super) const FACES: &[(u8, &str)] = &[
    (0, "1"),
    (1, "2"),
    (2, "3"),
    (3, "4"),
    (4, "5"),
    (5, "6"),
    (6, "7"),
    (7, "8"),
];

pub(super) const HAIRS: &[(u8, &str)] = &[(0, "A"), (1, "B")];

#[derive(Component)]
pub(super) struct CharCreateRoot;

#[derive(Component)]
pub(super) struct StatusText;

#[derive(Component, Clone, Copy)]
pub(super) struct EnumChoice {
    field: CharCreateField,
    value: u8,
}

pub(super) fn spawn_ui(mut commands: Commands, form: Res<CharCreateForm>, server: Res<ServerInfo>) {
    let snap = (
        form.name.clone(),
        form.race,
        form.job,
        form.nation,
        form.face,
        form.size,
    );
    let initial_msg = form.validation_msg().unwrap_or_default();

    commands
        .spawn((CharCreateRoot, screen_root()))
        .with_children(|root| {
            spawn_breadcrumb(
                root,
                &server,
                &[Crumb::Characters, Crumb::Other("New character".to_string())],
            );
            root.spawn(panel_node(560.0)).with_children(|panel| {
                panel.spawn(title("Create character"));
                panel.spawn(hint(
                    "Tab cycles fields. Click a value to set it. Esc returns to char list.",
                ));

                panel
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
                                width: Val::Px(80.0),
                                ..default()
                            },
                            Text::new("Name"),
                            ThemedText,
                        ));
                        row.spawn(text_field(TextFieldProps {
                            initial: snap.0.clone(),
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
                            |ev: On<ValueChange<String>>, mut form: ResMut<CharCreateForm>| {
                                let filtered: String = ev
                                    .value
                                    .chars()
                                    .filter(|c| c.is_ascii_alphabetic())
                                    .take(15)
                                    .collect();
                                form.name = filtered;
                            },
                        );
                    });

                spawn_enum_row(panel, "Race", CharCreateField::Race, RACES, snap.1);
                spawn_enum_row(panel, "Job", CharCreateField::Job, JOBS, snap.2);
                spawn_enum_row(panel, "Nation", CharCreateField::Nation, NATIONS, snap.3);
                spawn_enum_row(panel, "Build", CharCreateField::Size, SIZES, snap.5);
                spawn_enum_row(panel, "Face", CharCreateField::Face, FACES, snap.4 / 2);
                spawn_enum_row(panel, "Hair", CharCreateField::Hair, HAIRS, snap.4 % 2);

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
                         form: Res<CharCreateForm>,
                         mut err: ResMut<CharCreateError>,
                         mut next: ResMut<NextState<LauncherState>>| {
                            if form.validation_msg().is_none() {
                                err.0.clear();
                                next.set(LauncherState::CharCreateInFlight);
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
                         mut err: ResMut<CharCreateError>,
                         mut next: ResMut<NextState<LauncherState>>| {
                            err.0.clear();
                            next.set(LauncherState::CharList);
                        },
                    );
                });
            });
        });
}

fn spawn_enum_row(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    field: CharCreateField,
    table: &'static [(u8, &'static str)],
    current: u8,
) {
    parent
        .spawn(Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(6.0),
            ..default()
        })
        .with_children(|row| {
            row.spawn((
                Node {
                    width: Val::Px(80.0),
                    ..default()
                },
                Text::new(label.to_string()),
                ThemedText,
            ));
            for (val, name) in table.iter() {
                let val = *val;
                let variant = if val == current {
                    ButtonVariant::Primary
                } else {
                    ButtonVariant::Normal
                };
                row.spawn((button(
                    ButtonProps {
                        variant,
                        ..default()
                    },
                    EnumChoice { field, value: val },
                    Spawn((Text::new((*name).to_string()), ThemedText)),
                ),))
                    .observe(move |_ev: On<Activate>, mut form: ResMut<CharCreateForm>| {
                        form.set_field(field, val);
                    });
            }
        });
}

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<CharCreateRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

pub(super) fn keyboard_input_system(
    mut events: MessageReader<KeyboardInput>,
    mut err: ResMut<CharCreateError>,
    mut next: ResMut<NextState<LauncherState>>,
) {
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if matches!(ev.logical_key, Key::Escape) {
            err.0.clear();
            next.set(LauncherState::CharList);
            return;
        }
    }
}

pub(super) fn redraw_form_system(
    form: Res<CharCreateForm>,
    q_choices: Query<(Entity, &EnumChoice)>,
    mut q_status: Query<&mut Text, With<StatusText>>,
    mut commands: Commands,
) {
    if !form.is_changed() {
        return;
    }
    for (e, choice) in q_choices.iter() {
        let v = if choice.value == form.field_selection(choice.field) {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Normal
        };
        commands.entity(e).insert(v);
    }
    for mut t in q_status.iter_mut() {
        **t = form.validation_msg().unwrap_or_default();
    }
}

#[derive(Component)]
pub(super) struct CharCreateErrorRoot;

pub(super) fn spawn_error_ui(mut commands: Commands, err: Res<CharCreateError>) {
    let body = err.0.clone();
    commands
        .spawn((CharCreateErrorRoot, screen_root()))
        .with_children(|root| {
            root.spawn(panel_node(520.0)).with_children(|panel| {
                panel.spawn((
                    Text::new("Character creation failed"),
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
                            next.set(LauncherState::CharCreateInFlight);
                        },
                    );
                    r.spawn(button(
                        ButtonProps::default(),
                        (),
                        Spawn((Text::new("Back to form"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                            next.set(LauncherState::CharCreate);
                        },
                    );
                });
            });
        });
}

pub(super) fn despawn_error_ui(
    mut commands: Commands,
    q: Query<Entity, With<CharCreateErrorRoot>>,
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
            next_state.set(LauncherState::CharCreate);
            return;
        }
    }
}
