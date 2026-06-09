//! Shared UI helpers for the rewritten (feathers-based) launcher screens.
//!
//! Every screen follows the same shape: a full-screen flex root that lets
//! the zone backdrop show through, with a centered translucent panel
//! containing the form.

use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button, ButtonProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input_focus::tab_navigation::TabGroup;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use super::{LauncherState, ServerInfo};

/// One link in the navigation breadcrumb rendered above every launcher
/// panel. The active server is implicit (always the first segment) so
/// callers only describe what comes *after* it.
#[derive(Clone)]
#[allow(dead_code)]
pub(super) enum Crumb {
    /// Server-select screen as a mid-crumb (rare; the leading chip is
    /// already the canonical ServerSelect re-entry, but we expose this
    /// for screens whose path explicitly includes "Servers" between
    /// the chip and the leaf — e.g. ServerEdit).
    Server,
    /// Sign-in form. The optional username, when present, becomes part
    /// of the label ("Sign in: user").
    Sign(Option<String>),
    /// Char-select for the current account.
    Characters,
    /// Current-screen leaf — never clickable; rendered as plain text.
    Other(String),
}

impl Crumb {
    fn label(&self) -> String {
        match self {
            Crumb::Server => "Servers".to_string(),
            Crumb::Sign(Some(u)) => format!("Sign in: {u}"),
            Crumb::Sign(None) => "Sign in".to_string(),
            Crumb::Characters => "Characters".to_string(),
            Crumb::Other(s) => s.clone(),
        }
    }

    fn target(&self) -> Option<LauncherState> {
        match self {
            Crumb::Server => Some(LauncherState::ServerSelect),
            Crumb::Sign(_) => Some(LauncherState::Login),
            Crumb::Characters => Some(LauncherState::CharList),
            Crumb::Other(_) => None,
        }
    }
}

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
            // bevy 0.18: BorderRadius is now a `Node` field, not a component.
            border_radius: BorderRadius::all(Val::Px(6.0)),
            ..default()
        },
        BackgroundColor(PANEL_BG),
        BorderColor::all(Color::srgb(0.20, 0.20, 0.24)),
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

/// Marker on the Text node inside the leading server chip so
/// `update_server_chips` can refresh just the label when the active
/// profile changes (without rebuilding the breadcrumb entity tree).
#[derive(Component)]
pub(super) struct ServerChipLabel;

/// Top-anchored breadcrumb: `[Server: X] > [Crumb1] > Leaf`. Every
/// segment except the final crumb is rendered as a feathers button that
/// jumps to its target state; the final crumb is plain text so the user
/// has a clear "you are here" anchor.
pub(super) fn spawn_breadcrumb(
    parent: &mut ChildSpawnerCommands,
    server: &ServerInfo,
    crumbs: &[Crumb],
) {
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(6.0),
            padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
            margin: UiRect::bottom(Val::Px(12.0)),
            border: UiRect::all(Val::Px(1.0)),
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        })
        .insert((
            BackgroundColor(PANEL_BG),
            BorderColor::all(Color::srgb(0.20, 0.20, 0.24)),
        ))
        .with_children(|chip| {
            // Server chip: always the first segment and always a button
            // (it's the canonical re-entry point for ServerSelect).
            chip.spawn(button(
                ButtonProps::default(),
                (),
                Spawn((
                    Text::new(format!("Server: {}", server.display_label())),
                    ThemedText,
                    ServerChipLabel,
                )),
            ))
            .observe(
                |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                    next.set(LauncherState::ServerSelect);
                },
            );

            let last = crumbs.len().saturating_sub(1);
            for (idx, crumb) in crumbs.iter().enumerate() {
                chip.spawn((
                    Text::new(">"),
                    TextFont {
                        font_size: 14.0,
                        ..default()
                    },
                    TextColor(Color::srgb(0.55, 0.55, 0.60)),
                    ThemedText,
                ));
                let label = crumb.label();
                if idx == last {
                    chip.spawn((
                        Text::new(label),
                        TextFont {
                            font_size: 14.0,
                            ..default()
                        },
                        TextColor(Color::srgb(0.85, 0.85, 0.90)),
                        ThemedText,
                    ));
                } else if let Some(target) = crumb.target() {
                    chip.spawn(button(
                        ButtonProps {
                            variant: ButtonVariant::Normal,
                            ..default()
                        },
                        (),
                        Spawn((Text::new(label), ThemedText)),
                    ))
                    .observe(
                        move |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                            next.set(target.clone());
                        },
                    );
                } else {
                    chip.spawn((
                        Text::new(label),
                        TextFont {
                            font_size: 14.0,
                            ..default()
                        },
                        TextColor(Color::srgb(0.85, 0.85, 0.90)),
                        ThemedText,
                    ));
                }
            }
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
