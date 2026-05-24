//! Local widget kit layered on top of `bevy_feathers` 0.17.
//!
//! Feathers ships button/checkbox/slider/radio/toggle_switch/virtual_keyboard
//! plus a dark theme, but **no single-line text input** — every launcher
//! form needs that, so [`text_field`] fills the gap. The combo
//! [`labeled_text_field`] wraps a `Text` label + `text_field` in a row
//! `Node` so the six existing launcher forms don't each reinvent it.
//!
//! See `text_field.rs` for the focus/key/render model.

pub mod text_field;

use bevy::feathers::controls::{button, ButtonProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input_focus::tab_navigation::TabGroup;
use bevy::prelude::*;

pub use text_field::{
    text_field, TextField, TextFieldDisplay, TextFieldPlugin, TextFieldProps,
};
// Re-exported separately because the demo doesn't reference it but
// downstream launcher code will subscribe to it.
#[allow(unused_imports)]
pub use text_field::TextFieldSubmitted;

/// Spawn helper: a labelled, editable text field in a horizontal row.
/// `label` is the static text shown to the left; `props` configures the
/// inner [`text_field`]. The returned bundle should be passed to
/// `commands.spawn(...)` (it includes its own `Children`).
pub fn labeled_text_field(label: impl Into<String>, props: TextFieldProps) -> impl Bundle {
    let label = label.into();
    (
        Node {
            width: Val::Percent(100.0),
            height: Val::Px(32.0),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(8.0),
            ..default()
        },
        children![
            (
                Node {
                    width: Val::Px(110.0),
                    ..default()
                },
                Text::new(label),
                ThemedText,
            ),
            spawn_text_field_with_display(props),
        ],
    )
}

/// Internal: spawn a text_field parent plus its display child. The display
/// child carries the live `Text` node the render system updates each
/// frame and the [`TextFieldDisplay`] marker pointing back at the parent.
fn spawn_text_field_with_display(props: TextFieldProps) -> impl Bundle {
    (
        text_field(props),
        // Inserted via a closure-spawn so we can capture the parent's Entity
        // for the TextFieldDisplay marker. Bevy 0.17's children![] doesn't
        // expose parent Entity at construction time, so we use an observer
        // that fires `Added<TextField>` to backfill the child.
        // For v1 we instead use a small startup-style system; spawn the
        // child immediately with a placeholder owner of Entity::PLACEHOLDER
        // and let `attach_display_owner` rewrite it.
        children![(
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
        )],
    )
}

/// Backfill: any `TextFieldDisplay` with a PLACEHOLDER owner gets pointed
/// at its parent `TextField`. Runs every frame in `PreUpdate`; cost is one
/// query per added display. Registered by [`TextFieldPlugin`] via
/// [`register_display_backfill`].
pub fn attach_display_owner(
    mut q: Query<(&mut TextFieldDisplay, &ChildOf), Added<TextFieldDisplay>>,
    q_field: Query<(), With<TextField>>,
) {
    for (mut display, child_of) in q.iter_mut() {
        if display.owner == Entity::PLACEHOLDER && q_field.get(child_of.parent()).is_ok() {
            display.owner = child_of.parent();
        }
    }
}

/// Convenience plugin that registers [`TextFieldPlugin`] plus the
/// display-owner backfill system. Apps should add this once; it's
/// idempotent (Bevy dedupes plugin instances).
pub struct WidgetsPlugin;

impl Plugin for WidgetsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(TextFieldPlugin)
            .add_systems(PreUpdate, attach_display_owner);
    }
}

/// One-shot smoke demo. Spawns a Camera2d, a TabGroup root, a labeled
/// text field, a password (masked) field, and a feathers Button. Gated on
/// the `FFXI_WIDGET_DEMO=1` env var so a follow-up agent can sanity-check
/// the runtime before rewriting the real launcher screens:
///
/// ```sh
/// FFXI_WIDGET_DEMO=1 cargo run -p ffxi-client --features native-window -- native
/// ```
///
/// The demo overlays the launcher (z-order: spawned after) and prints
/// edits + submission events to stdout. Remove the env var to disable.
pub fn spawn_widget_demo(mut commands: Commands) {
    commands.spawn(Camera2d);
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(20.0),
                left: Val::Px(20.0),
                width: Val::Px(360.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(10.0),
                padding: UiRect::all(Val::Px(12.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.08, 0.08, 0.10, 0.95)),
            TabGroup::default(),
        ))
        .with_children(|root| {
            root.spawn((
                Text::new("Widget Smoke Demo (Tab to cycle, Enter to submit)"),
                ThemedText,
            ));
            root.spawn(labeled_text_field(
                "Username",
                TextFieldProps {
                    placeholder: "enter name".into(),
                    submit_on_enter: true,
                    ..default()
                },
            ));
            root.spawn(labeled_text_field(
                "Password",
                TextFieldProps {
                    placeholder: "•••••".into(),
                    mask: true,
                    submit_on_enter: true,
                    ..default()
                },
            ));
            root.spawn(button(
                ButtonProps {
                    variant: ButtonVariant::Primary,
                    ..default()
                },
                (),
                Spawn((Text::new("Sign in"), ThemedText)),
            ));
        });
}

// Re-export Spawn so the demo `button(...)` children call compiles.
use bevy::ecs::spawn::Spawn;
