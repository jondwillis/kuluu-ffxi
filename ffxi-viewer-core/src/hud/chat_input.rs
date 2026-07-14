use bevy::prelude::*;

use crate::hud::palette;
use crate::input_mode::InputMode;

#[derive(Component)]
pub struct ChatInputBar;

#[derive(Component)]
pub struct ChatInputText;

pub fn spawn_chat_input(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            ChatInputBar,
            Node {
                position_type: PositionType::Absolute,

                bottom: Val::Px(28.0),
                left: Val::Px(0.0),
                width: Val::Percent(60.0),
                height: Val::Px(24.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                align_items: AlignItems::Center,
                display: Display::None,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
        ))
        .with_children(|p| {
            p.spawn((
                ChatInputText,
                Text::new("> _"),
                TextFont {
                    font_size: 13.0.into(),
                    ..default()
                },
                TextColor(palette::TEXT),
            ));
        });
}

pub fn update_chat_input(
    mode: Res<InputMode>,
    mut bar_q: Query<&mut Node, With<ChatInputBar>>,
    mut text_q: Query<&mut Text, With<ChatInputText>>,
) {
    let Ok(mut node) = bar_q.single_mut() else {
        return;
    };
    let Ok(mut text) = text_q.single_mut() else {
        return;
    };

    match &*mode {
        InputMode::Chat(buffer) => {
            node.display = Display::Flex;

            let want = format!("> {}_", buffer.text);
            if **text != want {
                **text = want;
            }
        }
        _ => {
            if node.display != Display::None {
                node.display = Display::None;
            }
        }
    }
}
