use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button, ButtonProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use ffxi_viewer_core::{GraphicsField, GraphicsSettings, GRAPHICS_FIELDS};

use super::common::{hint, panel_node, row, screen_root, spawn_breadcrumb, title, Crumb};
use super::{LauncherState, ServerInfo};

#[derive(Component)]
pub(super) struct GraphicsRoot;

#[derive(Component)]
pub(super) struct GraphicsValueText(GraphicsField);

/// A field row gated behind the "Advanced" disclosure (the indented sky/light
/// sub-knobs). Toggled visible/hidden by [`redraw_advanced_visibility`].
#[derive(Component)]
pub(super) struct AdvancedRow;

/// The disclosure row's label text, swapped between ▸ and ▾.
#[derive(Component)]
pub(super) struct AdvancedToggleLabel;

/// Whether the Advanced sub-knobs are expanded. Persists across visits to the
/// Graphics screen; collapsed by default so the screen stays short.
#[derive(Resource, Default)]
pub(super) struct GraphicsAdvancedOpen(pub bool);

const ADVANCED_COLLAPSED: &str = "▸ Advanced — light tuning";
const ADVANCED_EXPANDED: &str = "▾ Advanced — light tuning";

pub(super) fn spawn_ui(
    mut commands: Commands,
    settings: Res<GraphicsSettings>,
    server: Res<ServerInfo>,
    advanced: Res<GraphicsAdvancedOpen>,
) {
    let open = advanced.0;
    commands
        .spawn((GraphicsRoot, screen_root()))
        .with_children(|root| {
            spawn_breadcrumb(root, &server, &[Crumb::Other("Graphics".to_string())]);
            root.spawn(panel_node(560.0)).with_children(|panel| {
                panel.spawn(title("Graphics"));
                panel.spawn(hint(
                    "Tune the same quality settings as the in-game menu. \
                     Changes apply when you connect and are saved \
                     automatically. Esc goes back.",
                ));

                for &field in GRAPHICS_FIELDS.iter().filter(|f| !f.is_advanced()) {
                    spawn_field_row(panel, field, &settings, false, open);
                }

                panel.spawn(row()).with_children(|r| {
                    r.spawn(button(
                        ButtonProps {
                            variant: ButtonVariant::Normal,
                            ..default()
                        },
                        (),
                        Spawn((
                            Text::new(if open {
                                ADVANCED_EXPANDED
                            } else {
                                ADVANCED_COLLAPSED
                            }),
                            ThemedText,
                            AdvancedToggleLabel,
                        )),
                    ))
                    .observe(
                        |_ev: On<Activate>, mut open: ResMut<GraphicsAdvancedOpen>| {
                            open.0 = !open.0;
                        },
                    );
                });

                for &field in GRAPHICS_FIELDS.iter().filter(|f| f.is_advanced()) {
                    spawn_field_row(panel, field, &settings, true, open);
                }

                panel.spawn(row()).with_children(|r| {
                    r.spawn(button(
                        ButtonProps {
                            variant: ButtonVariant::Normal,
                            ..default()
                        },
                        (),
                        Spawn((Text::new("Reset to High"), ThemedText)),
                    ))
                    .observe(
                        |_ev: On<Activate>, mut settings: ResMut<GraphicsSettings>| {
                            settings.reset_to_default();
                        },
                    );

                    r.spawn(button(
                        ButtonProps {
                            variant: ButtonVariant::Primary,
                            ..default()
                        },
                        (),
                        Spawn((Text::new("Back"), ThemedText)),
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

fn spawn_field_row(
    panel: &mut ChildSpawnerCommands,
    field: GraphicsField,
    settings: &GraphicsSettings,
    advanced: bool,
    advanced_open: bool,
) {
    let value_color = Color::srgb(0.92, 0.92, 0.95);
    let mut row_cmd = panel.spawn(Node {
        width: Val::Percent(100.0),
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        column_gap: Val::Px(8.0),
        display: if advanced && !advanced_open {
            Display::None
        } else {
            Display::Flex
        },
        ..default()
    });
    if advanced {
        row_cmd.insert(AdvancedRow);
    }
    row_cmd.with_children(|rowc| {
        rowc.spawn((
            Node {
                width: Val::Px(160.0),
                ..default()
            },
            Text::new(field.label().to_string()),
            ThemedText,
        ));

        rowc.spawn(button(
            ButtonProps::default(),
            (),
            Spawn((Text::new("◀"), ThemedText)),
        ))
        .observe(
            move |_ev: On<Activate>, mut settings: ResMut<GraphicsSettings>| {
                settings.cycle(field, -1);
            },
        );

        rowc.spawn((
            Node {
                width: Val::Px(150.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            Text::new(settings.value_label(field)),
            TextColor(value_color),
            GraphicsValueText(field),
            ThemedText,
        ));

        rowc.spawn(button(
            ButtonProps::default(),
            (),
            Spawn((Text::new("▶"), ThemedText)),
        ))
        .observe(
            move |_ev: On<Activate>, mut settings: ResMut<GraphicsSettings>| {
                settings.cycle(field, 1);
            },
        );
    });
}

pub(super) fn redraw_graphics_system(
    settings: Res<GraphicsSettings>,
    mut q: Query<(&GraphicsValueText, &mut Text)>,
) {
    if !settings.is_changed() {
        return;
    }
    for (cell, mut text) in q.iter_mut() {
        let want = settings.value_label(cell.0);
        if text.0 != want {
            text.0 = want;
        }
    }
}

/// Show/hide the Advanced sub-knob rows and flip the ▸/▾ disclosure label when
/// the user toggles the Advanced row.
pub(super) fn redraw_advanced_visibility(
    open: Res<GraphicsAdvancedOpen>,
    mut rows: Query<&mut Node, With<AdvancedRow>>,
    mut labels: Query<&mut Text, With<AdvancedToggleLabel>>,
) {
    if !open.is_changed() {
        return;
    }
    let display = if open.0 { Display::Flex } else { Display::None };
    for mut node in rows.iter_mut() {
        if node.display != display {
            node.display = display;
        }
    }
    let want = if open.0 {
        ADVANCED_EXPANDED
    } else {
        ADVANCED_COLLAPSED
    };
    for mut text in labels.iter_mut() {
        if text.0 != want {
            text.0 = want.to_string();
        }
    }
}

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<GraphicsRoot>>) {
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
