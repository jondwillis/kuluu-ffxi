//! Bottom-left chat panel mirroring `chrome::draw_chat`. Bordered list
//! with channel-tagged lines: `[say] Sender: text`, color per channel.
//!
//! Strategy: spawn a fixed-size pool of empty rows once. Each frame, fill
//! visible rows with the most recent N chat lines (newest at the bottom).
//! Avoids spawn-despawn churn at 60 Hz.

use bevy::prelude::*;
use ffxi_viewer_wire::{ChatChannel, ChatLine};

use crate::hud::palette;
use crate::snapshot::SceneState;

/// Number of chat rows visible at once. Matches what fits in the panel
/// height at the default font size.
pub const VISIBLE_ROWS: usize = 8;

/// Marker on the panel root.
#[derive(Component)]
pub struct ChatPanel;

/// Marker on each row container; `slot` is its position 0..VISIBLE_ROWS-1.
#[derive(Component)]
pub struct ChatRow {
    pub slot: usize,
}

/// Marker on the channel-tag text within a row.
#[derive(Component)]
pub struct ChatRowTag;

/// Marker on the body text (sender + message) within a row.
#[derive(Component)]
pub struct ChatRowBody;

pub fn spawn_chat_panel(mut commands: Commands) {
    commands
        .spawn((
            ChatPanel,
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(28.0), // diagnostics bar height
                left: Val::Px(0.0),
                width: Val::Percent(60.0),
                height: Val::Px(160.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::FlexEnd,
                row_gap: Val::Px(2.0),
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::BORDER),
        ))
        .with_children(|p| {
            for slot in 0..VISIBLE_ROWS {
                p.spawn((
                    ChatRow { slot },
                    Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(6.0),
                        ..default()
                    },
                ))
                .with_children(|row| {
                    row.spawn((
                        ChatRowTag,
                        Text::new(""),
                        TextFont {
                            font_size: 13.0,
                            ..default()
                        },
                        TextColor(palette::MUTED),
                    ));
                    row.spawn((
                        ChatRowBody,
                        Text::new(""),
                        TextFont {
                            font_size: 13.0,
                            ..default()
                        },
                        TextColor(palette::TEXT),
                    ));
                });
            }
        });
}

pub fn update_chat_panel(
    state: Res<SceneState>,
    rows: Query<(&ChatRow, &Children)>,
    mut tag_q: Query<(&mut Text, &mut TextColor), (With<ChatRowTag>, Without<ChatRowBody>)>,
    mut body_q: Query<&mut Text, (With<ChatRowBody>, Without<ChatRowTag>)>,
) {
    if !state.dirty {
        return;
    }
    let chat = &state.snapshot.chat;
    let visible: Vec<Option<&ChatLine>> = (0..VISIBLE_ROWS)
        .rev()
        .map(|i| {
            // Oldest visible at top; newest at bottom. Slot N-1 is newest.
            // chat is oldest-first, so the newest line is chat.last().
            let n = chat.len();
            if i < n {
                Some(&chat[n - 1 - i])
            } else {
                None
            }
        })
        .collect();

    for (row, children) in &rows {
        // Map row.slot (0=top) to visible[slot] but visible was built reversed:
        // visible[0] = oldest of N visible, visible[N-1] = newest.
        let line = visible.get(row.slot).copied().flatten();
        for child in children.iter() {
            if let Ok((mut text, mut tc)) = tag_q.get_mut(child) {
                match line {
                    Some(l) => {
                        let (tag, color) = channel_tag(l.channel);
                        **text = tag.into();
                        tc.0 = color;
                    }
                    None => {
                        **text = String::new();
                    }
                }
            } else if let Ok(mut text) = body_q.get_mut(child) {
                match line {
                    Some(l) => {
                        **text = format!("{}: {}", l.sender, l.text);
                    }
                    None => {
                        **text = String::new();
                    }
                }
            }
        }
    }
}

fn channel_tag(c: ChatChannel) -> (&'static str, Color) {
    match c {
        ChatChannel::Say => ("[say]", palette::TEXT),
        ChatChannel::Shout => ("[sho]", palette::ACCENT),
        ChatChannel::Tell => ("[tll]", Color::srgb(0.95, 0.40, 0.95)),
        ChatChannel::Party => ("[pty]", Color::srgb(0.50, 0.65, 1.00)),
        ChatChannel::Linkshell => ("[lin]", Color::srgb(0.40, 0.95, 0.50)),
        ChatChannel::Yell => ("[yel]", Color::srgb(1.00, 0.85, 0.20)),
        ChatChannel::System => ("[sys]", palette::MUTED),
        ChatChannel::Other => ("[---]", palette::DARK),
    }
}
