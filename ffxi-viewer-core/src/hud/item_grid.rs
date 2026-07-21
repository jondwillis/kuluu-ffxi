//! Shared item-grid cell widget: the bordered square with a centered icon and
//! a small fallback label, used by the equipment screen's 4x4 slot grid and
//! the delivery-box 2x4 grid in the dialog panel.

use bevy::prelude::*;

use crate::hud::style::text_font;
use crate::hud::style::theme;

pub(crate) const CELL_PX: f32 = 36.0;
pub(crate) const ICON_PX: f32 = 30.0;
pub(crate) const CELL_GAP_PX: f32 = 4.0;

/// Spawn one grid cell: a `CELL_PX` framed square containing a (hidden by
/// default) `ICON_PX` icon and a small muted label. Marker components for the
/// frame / icon / label are supplied by the caller so each screen can drive
/// its own update systems over the shared structure.
pub(crate) fn spawn_item_cell(
    p: &mut ChildSpawnerCommands,
    frame_marker: impl Bundle,
    icon_marker: impl Bundle,
    label_marker: impl Bundle,
    label_text: &str,
    placeholder: Handle<Image>,
) {
    p.spawn((
        frame_marker,
        Node {
            width: Val::Px(CELL_PX),
            height: Val::Px(CELL_PX),
            border: UiRect::all(Val::Px(1.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(theme::CELL_BG),
        BorderColor::all(theme::CELL_EDGE),
    ))
    .with_children(|c| {
        c.spawn((
            icon_marker,
            Node {
                width: Val::Px(ICON_PX),
                height: Val::Px(ICON_PX),
                display: Display::None,
                ..default()
            },
            ImageNode::new(placeholder),
        ));
        c.spawn((
            label_marker,
            Text::new(label_text),
            text_font(11.0),
            TextColor(theme::MUTED),
        ));
    });
}
