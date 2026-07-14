pub mod text_field;

use bevy::feathers::controls::{button_bundle, ButtonBundleProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input_focus::tab_navigation::TabGroup;
use bevy::prelude::*;

pub use text_field::{text_field, TextField, TextFieldDisplay, TextFieldPlugin, TextFieldProps};

#[allow(unused_imports)]
pub use text_field::TextFieldSubmitted;

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

fn spawn_text_field_with_display(props: TextFieldProps) -> impl Bundle {
    (
        text_field(props),
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

pub struct WidgetsPlugin;

impl Plugin for WidgetsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(TextFieldPlugin)
            .add_systems(PreUpdate, attach_display_owner);
    }
}

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
            root.spawn(button_bundle(
                ButtonBundleProps {
                    variant: ButtonVariant::Primary,
                    ..default()
                },
                (),
                Spawn((Text::new("Sign in"), ThemedText)),
            ));
        });
}

use bevy::ecs::spawn::Spawn;
