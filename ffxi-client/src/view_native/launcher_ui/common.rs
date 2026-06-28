use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::{button, ButtonProps, ButtonVariant};
use bevy::feathers::theme::ThemedText;
use bevy::input_focus::tab_navigation::TabGroup;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use super::{LauncherState, ServerInfo};

#[derive(Clone)]
#[allow(dead_code)]
pub(super) enum Crumb {
    Server,

    Sign(Option<String>),

    Characters,

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

pub(super) const PANEL_BG: Color = Color::srgba(0.04, 0.04, 0.05, 0.85);

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

            border_radius: BorderRadius::all(Val::Px(6.0)),
            ..default()
        },
        BackgroundColor(PANEL_BG),
        BorderColor::all(Color::srgb(0.20, 0.20, 0.24)),
        TabGroup::default(),
    )
}

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

#[derive(Component)]
pub(super) struct ServerChipLabel;

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

pub(super) fn row() -> impl Bundle {
    Node {
        width: Val::Percent(100.0),
        flex_direction: FlexDirection::Row,
        column_gap: Val::Px(8.0),
        align_items: AlignItems::Center,
        ..default()
    }
}

enum NavAction {
    Close,
    Back(LauncherState),
}

fn spawn_titlebar(
    parent: &mut ChildSpawnerCommands,
    title_text: impl Into<String>,
    action: NavAction,
) {
    let label = match &action {
        NavAction::Close => "×",
        NavAction::Back(_) => "Back to login",
    };
    parent
        .spawn(Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(8.0),
            ..default()
        })
        .with_children(|bar| {
            bar.spawn((
                Node {
                    flex_grow: 1.0,
                    ..default()
                },
                Text::new(title_text.into()),
                TextFont {
                    font_size: 22.0,
                    ..default()
                },
                TextColor(Color::srgb(0.0, 1.0, 1.0)),
                ThemedText,
            ));
            bar.spawn(Node::default()).with_children(|slot| {
                let mut btn = slot.spawn(button(
                    ButtonProps::default(),
                    (),
                    Spawn((Text::new(label), ThemedText)),
                ));
                match action {
                    NavAction::Close => {
                        btn.observe(|_ev: On<Activate>, mut exit: MessageWriter<AppExit>| {
                            exit.write_default();
                        });
                    }
                    NavAction::Back(target) => {
                        btn.observe(
                            move |_ev: On<Activate>, mut next: ResMut<NextState<LauncherState>>| {
                                next.set(target.clone());
                            },
                        );
                    }
                }
            });
        });
}

pub(super) fn spawn_close_titlebar(
    parent: &mut ChildSpawnerCommands,
    title_text: impl Into<String>,
) {
    spawn_titlebar(parent, title_text, NavAction::Close);
}

pub(super) fn spawn_back_titlebar(
    parent: &mut ChildSpawnerCommands,
    title_text: impl Into<String>,
) {
    spawn_titlebar(parent, title_text, NavAction::Back(LauncherState::Login));
}
