//! Launcher Graphics screen — surface the in-game quality knobs
//! ([`GraphicsSettings`]) from the launcher so the user can tune
//! shadows / AA / fog / view distance before ever connecting.
//!
//! # Same logic as the in-game menu
//!
//! This screen is a *Feathers shell* over the exact same setting logic the
//! in-game main-menu Graphics tab drives: [`GRAPHICS_FIELDS`] is the row
//! list, [`GraphicsSettings::value_label`] renders the bracketed value, and
//! the ◀ / ▶ buttons call [`GraphicsSettings::cycle`]. Because both menus
//! mutate the one shared [`GraphicsSettings`] resource (persisted to
//! `graphics.json` by `graphics_store::persist_graphics_on_change`), a
//! change made here is reflected in-game and vice versa — there is no
//! second copy of the values to keep in sync.
//!
//! # No explicit Save
//!
//! Like the in-game menu, edits apply live and autosave: mutating the
//! resource trips `persist_graphics_on_change` (registered in
//! `view_native::mod`) on the next frame. The viewer-core reactor systems
//! also run, but they target the in-game `OperatorCamera`, which doesn't
//! exist during the launcher phase, so they no-op here; the values take
//! effect when the in-game camera spawns on connect.

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

/// Marks the value cell of a settings row so [`redraw_graphics_system`]
/// can refresh just that text when the shared [`GraphicsSettings`] changes
/// — no panel rebuild on every ◀ / ▶ click.
#[derive(Component)]
struct GraphicsValueText(GraphicsField);

pub(super) fn spawn_ui(
    mut commands: Commands,
    settings: Res<GraphicsSettings>,
    server: Res<ServerInfo>,
) {
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

                for &field in GRAPHICS_FIELDS {
                    spawn_field_row(panel, field, &settings);
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

/// One settings row: `Label  [◀]  Value  [▶]`. The ◀ / ▶ observers cycle
/// the shared resource by ∓1 / ±1; the value cell is refreshed by
/// [`redraw_graphics_system`] rather than rebuilt here.
fn spawn_field_row(panel: &mut ChildSpawnerCommands, field: GraphicsField, settings: &GraphicsSettings) {
    panel
        .spawn(Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(8.0),
            ..default()
        })
        .with_children(|rowc| {
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
            .observe(move |_ev: On<Activate>, mut settings: ResMut<GraphicsSettings>| {
                settings.cycle(field, -1);
            });

            rowc.spawn((
                Node {
                    width: Val::Px(150.0),
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                Text::new(settings.value_label(field)),
                TextColor(Color::srgb(0.92, 0.92, 0.95)),
                GraphicsValueText(field),
                ThemedText,
            ));

            rowc.spawn(button(
                ButtonProps::default(),
                (),
                Spawn((Text::new("▶"), ThemedText)),
            ))
            .observe(move |_ev: On<Activate>, mut settings: ResMut<GraphicsSettings>| {
                settings.cycle(field, 1);
            });
        });
}

/// Refresh every value cell from the shared [`GraphicsSettings`] when it
/// changes. Gated on `is_changed()` so it's a no-op on idle frames. Note a
/// `Preset` cycle rewrites *all* fields, so every cell must be re-read, not
/// just the one whose button was clicked.
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

pub(super) fn despawn_ui(mut commands: Commands, q: Query<Entity, With<GraphicsRoot>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
}

/// Esc returns to the server picker (the screen Graphics is reached from).
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
