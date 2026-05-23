//! Chat input bar — single-line text entry that slots into the 24px gap
//! between the chat panel (`bottom: 54`) and the diagnostics strip
//! (`bottom: 0, height: 28`) while [`InputMode::Chat`] is active. Sits
//! beneath the chat panel rather than over it, so the bottommost chat
//! row stays visible while the operator is typing.
//!
//! Visual: matches the chat-panel backdrop (same dark fill, same border
//! color) so it reads as a continuation of the chat region rather than
//! a separate widget. Format: `> {text}_` — leading `>` prompt, the
//! buffer text, then a static `_` cursor (matches `launcher_ui/login.rs`
//! styling).
//!
//! Visibility: `Display::None` when not in chat mode, `Display::Flex`
//! when active. Toggling the display flag is cheaper than
//! spawn/despawning the node tree on every chat-open / chat-close.

use bevy::prelude::*;

use crate::hud::palette;
use crate::input_mode::InputMode;

/// Marker on the input-bar root node.
#[derive(Component)]
pub struct ChatInputBar;

/// Marker on the inner text node so the per-frame updater can target it
/// directly via a single-result query.
#[derive(Component)]
pub struct ChatInputText;

/// Spawn the chat input bar at startup. Begins hidden; the per-frame
/// system flips its visibility based on the active [`InputMode`].
pub fn spawn_chat_input(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            ChatInputBar,
            Node {
                position_type: PositionType::Absolute,
                // 28px diagnostics bar + 0px = sits directly on top of
                // the diagnostics strip, in the gap chat_panel leaves
                // (panel starts at bottom: 54).
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
                    font_size: 13.0,
                    ..default()
                },
                TextColor(palette::TEXT),
            ));
        });
}

/// Per-frame: show/hide the bar, refresh the text buffer.
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
            // `_` cursor at the end. Matches the login form's static cursor;
            // a blinking cursor is a Bevy 0.16 nicety we can add later.
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
