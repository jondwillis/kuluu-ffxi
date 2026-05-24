//! Shared UI helpers for the rewritten (feathers-based) launcher screens.
//!
//! Every screen follows the same shape: a full-screen flex root that lets
//! the zone backdrop show through, with a centered translucent panel
//! containing the form.

use bevy::feathers::theme::ThemedText;
use bevy::input_focus::tab_navigation::TabGroup;
use bevy::prelude::*;

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
