//! Shared UI helpers for the rewritten (feathers-based) launcher screens.
//!
//! Every screen follows the same shape: a full-screen flex root that lets
//! the zone backdrop show through, with a centered translucent panel
//! containing the form.

use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button, ButtonProps};
use bevy::feathers::theme::ThemedText;
use bevy::input_focus::tab_navigation::TabGroup;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use super::{LauncherState, ServerInfo};

/// Translucent form panel background — readable text without hiding the
/// La Theine backdrop scene rendered behind the launcher UI.
pub(super) const PANEL_BG: Color = Color::srgba(0.04, 0.04, 0.05, 0.85);

/// Standard centered-panel bundle. Caller spawns this with a `TabGroup`
/// so feathers' Tab cycling works inside the form.
pub(super) fn panel_node(width_px: f32) -> impl Bundle {
    (
        Node {
            width: Val::Px(width_px),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Stretch,
            justify_content: JustifyContent::FlexStart,
            row_gap: Val::Px(12.0),
            padding: UiRect::all(Val::Px(24.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(PANEL_BG),
        BorderColor::all(Color::srgb(0.20, 0.20, 0.24)),
        BorderRadius::all(Val::Px(6.0)),
        TabGroup::default(),
    )
}

/// Full-screen root that centers its single panel child. No
/// BackgroundColor — the zone backdrop renders behind it.
pub(super) fn screen_root() -> impl Bundle {
    Node {
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        flex_direction: FlexDirection::Column,
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        ..default()
    }
}

/// Title text bundle for the top of a panel.
pub(super) fn title(text: impl Into<String>) -> impl Bundle {
    (
        Text::new(text.into()),
        TextFont {
            font_size: 22.0,
            ..default()
        },
        TextColor(Color::srgb(0.0, 1.0, 1.0)),
        ThemedText,
    )
}

/// Subtitle / hint text.
pub(super) fn hint(text: impl Into<String>) -> impl Bundle {
    (
        Text::new(text.into()),
        TextFont {
            font_size: 12.0,
            ..default()
        },
        TextColor(Color::srgb(0.6, 0.6, 0.65)),
        ThemedText,
    )
}

/// Marker for the text node inside a [`server_chip`] so the
/// `update_server_chips` system can refresh just the label when the
/// active profile changes.
#[derive(Component)]
pub(super) struct ServerChipLabel;

/// Top-anchored "Server: <name> [Change]" chip rendered above the form
/// panel on every user-facing screen. The Change button jumps to
/// `ServerSelect` so the server is always one click away — promoting
/// it from buried-in-the-title-bar to a top-level navigation surface.
pub(super) fn spawn_server_chip(parent: &mut ChildSpawnerCommands, server: &ServerInfo) {
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(10.0),
            padding: UiRect::axes(Val::Px(12.0), Val::Px(6.0)),
            margin: UiRect::bottom(Val::Px(12.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        })
        .insert((
            BackgroundColor(PANEL_BG),
            BorderColor::all(Color::srgb(0.20, 0.20, 0.24)),
            BorderRadius::all(Val::Px(4.0)),
        ))
        .with_children(|chip| {
            chip.spawn((
                Text::new(format!("Server: {}", server.display_label())),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(Color::srgb(0.85, 0.85, 0.90)),
                ThemedText,
                ServerChipLabel,
            ));
            chip.spawn(button(
                ButtonProps::default(),
                (),
                Spawn((Text::new("Change"), ThemedText)),
            ))
            .observe(
                |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                    next.set(LauncherState::ServerSelect);
                },
            );
        });
}

/// Keep [`ServerChipLabel`] text in sync with [`ServerInfo`] — survives
/// the same launcher state if e.g. ServerSelect rebinds without
/// despawning the chip. Cheap; only writes when the label string
/// actually differs.
pub(super) fn update_server_chips(
    server: Res<ServerInfo>,
    mut q: Query<&mut Text, With<ServerChipLabel>>,
) {
    if !server.is_changed() {
        return;
    }
    let want = format!("Server: {}", server.display_label());
    for mut t in q.iter_mut() {
        if t.0 != want {
            t.0 = want.clone();
        }
    }
}

/// Row container — horizontal flex with column gap.
pub(super) fn row() -> impl Bundle {
    Node {
        width: Val::Percent(100.0),
        flex_direction: FlexDirection::Row,
        column_gap: Val::Px(8.0),
        align_items: AlignItems::Center,
        ..default()
    }
}
