//! Bottom-left chat panel — retail-FFXI-style formatting.
//!
//! Each line is rendered as a single colored row. The format is
//! channel-specific (matching SE's default client):
//!   - Say / Shout / Other: `Sender : text`
//!   - Tell:                `>>Sender : text`
//!   - Party:               `(Sender) text`
//!   - Linkshell:           `<Sender> text`
//!   - Yell:                `[Sender] : text`
//!   - System / Battle:     `text` (no sender — server already formatted)
//!
//! Color encodes channel: the whole line picks up [`channel_color`].
//! There is no separate `[say]`-style tag prefix any more — color is the
//! channel cue.
//!
//! Strategy: spawn a fixed-size pool of empty rows once. Each frame, fill
//! visible rows with the most recent N chat lines (newest at the bottom).
//! Avoids spawn-despawn churn at 60 Hz.

use bevy::prelude::*;
use ffxi_viewer_wire::{ChatChannel, ChatLine};

use crate::hud::palette;
use crate::input_mode::{InputMode, PassiveCursorFocus};
use crate::snapshot::{rendered_chat, SceneState};

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

/// Marker on the line text within a row.
#[derive(Component)]
pub struct ChatRowBody;

pub fn spawn_chat_panel(mut commands: Commands) {
    commands
        .spawn((
            ChatPanel,
            Node {
                position_type: PositionType::Absolute,
                // Stack: 28px diagnostics bar + 24px chat-input slot + 2px
                // gap = 54. The chat input bar at `bottom: 28` (height 24)
                // slots into the gap below this panel rather than overlaying
                // its bottommost row. When the input is hidden the gap is
                // just empty breathing room above the diagnostics strip.
                bottom: Val::Px(54.0),
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
                        ..default()
                    },
                ))
                .with_children(|row| {
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
    mode: Res<InputMode>,
    mut panel_q: Query<&mut BorderColor, With<ChatPanel>>,
    rows: Query<(&ChatRow, &Children)>,
    mut body_q: Query<(&mut Text, &mut TextColor), With<ChatRowBody>>,
) {
    // Intentionally NOT gated on `state.dirty` — the dirty flag is reset
    // by `ingest_system` in `PreUpdate`, but `text_input_system` /
    // `dialog_mode_sync_system` push local toasts mid-`Update` and there
    // is no enforced ordering with this system. Without a strict chain a
    // toast set after this system ran would never paint (race: ingest
    // resets dirty next frame before this gets a second chance). The
    // body's per-row `if **text != want` change-detection guard keeps
    // the per-frame cost trivial when nothing actually changed.
    let chat = rendered_chat(&state);

    // PassiveCursor with focus on Chat shifts the visible window back
    // by `chat_scroll` rows. The handler clamps `chat_scroll` against
    // the buffer length, so we can trust it here. World / other modes
    // always render the latest tail of the buffer.
    let scroll_offset = match &*mode {
        InputMode::PassiveCursor(s) if matches!(s.focus, PassiveCursorFocus::Chat) => {
            s.chat_scroll
        }
        _ => 0,
    };
    let chat_focused = scroll_offset != 0
        || matches!(
            &*mode,
            InputMode::PassiveCursor(s) if matches!(s.focus, PassiveCursorFocus::Chat)
        );

    // Toggle the panel border between the muted default and the accent
    // color when chat is focused. Same accent the chat-input bar uses
    // when active, so the visual cue is consistent.
    if let Ok(mut border) = panel_q.single_mut() {
        let want = if chat_focused { palette::ACCENT } else { palette::BORDER };
        if border.left != want {
            *border = BorderColor::all(want);
        }
    }

    let visible: Vec<Option<&ChatLine>> = (0..VISIBLE_ROWS)
        .rev()
        .map(|i| {
            // Oldest visible at top; newest at bottom. Slot N-1 is newest
            // (or `n - 1 - scroll_offset` when scrolled back). `chat` is
            // oldest-first (server lines, then local toasts).
            let n = chat.len();
            // The newest visible index from the user's viewpoint is
            // `n - 1 - scroll_offset`; row i (0..VISIBLE_ROWS) reads
            // `(n - 1 - scroll_offset) - i`. If that's negative we
            // emit None for the slot.
            let newest_visible = n.checked_sub(1 + scroll_offset);
            match newest_visible {
                Some(top) => {
                    if i <= top {
                        Some(chat[top - i])
                    } else {
                        None
                    }
                }
                None => None,
            }
        })
        .collect();

    for (row, children) in &rows {
        let line = visible.get(row.slot).copied().flatten();
        for child in children.iter() {
            if let Ok((mut text, mut tc)) = body_q.get_mut(child) {
                match line {
                    Some(l) => {
                        let want = format_chat_line(l.channel, &l.sender, &l.text);
                        if **text != want {
                            **text = want;
                        }
                        let want_color = channel_color(l.channel);
                        if tc.0 != want_color {
                            tc.0 = want_color;
                        }
                    }
                    None => {
                        if !text.is_empty() {
                            **text = String::new();
                        }
                    }
                }
            }
        }
    }
}

/// Pure formatter for a single chat row, matching SE's default-client
/// per-channel layout. Pulled out so it can be unit-tested without a
/// Bevy app.
pub fn format_chat_line(channel: ChatChannel, sender: &str, text: &str) -> String {
    match channel {
        // Say / Shout / Other: "Sender : text". Same shape as the
        // canonical FFXI default-client display; channel is conveyed by
        // color, not by tag.
        ChatChannel::Say | ChatChannel::Shout | ChatChannel::Other => {
            format!("{sender} : {text}")
        }
        // Tell: ">>Sender : text". The double-greater-than is FFXI's
        // tell sigil. For *outbound* tells the sender field carries the
        // recipient (the operator's local-echo path puts the recipient
        // there); the layout still reads correctly: ">>Daisy : msg" is
        // "I told Daisy" or "Daisy told me" depending on direction.
        ChatChannel::Tell => format!(">>{sender} : {text}"),
        // Party: "(Sender) text". Parens around name, no colon.
        ChatChannel::Party => format!("({sender}) {text}"),
        // Linkshell: "<Sender> text". Angle brackets, no colon.
        ChatChannel::Linkshell => format!("<{sender}> {text}"),
        // Yell: "[Sender] : text". Square brackets around name.
        ChatChannel::Yell => format!("[{sender}] : {text}"),
        // System and Battle: server already formatted these. Print the
        // text bare — no sender prefix, no decoration.
        ChatChannel::System | ChatChannel::Battle => text.to_string(),
    }
}

/// Per-channel line color — the whole row picks this up.
pub fn channel_color(c: ChatChannel) -> Color {
    match c {
        ChatChannel::Say => palette::TEXT,
        ChatChannel::Shout => palette::ACCENT,
        ChatChannel::Tell => Color::srgb(0.95, 0.40, 0.95),
        ChatChannel::Party => Color::srgb(0.50, 0.65, 1.00),
        ChatChannel::Linkshell => Color::srgb(0.40, 0.95, 0.50),
        ChatChannel::Yell => Color::srgb(1.00, 0.85, 0.20),
        ChatChannel::System => palette::MUTED,
        ChatChannel::Other => palette::DARK,
        // Orange — matches classic FFXI's combat log color so the
        // operator's at-a-glance read picks up battle lines apart from
        // social channels.
        ChatChannel::Battle => Color::srgb(1.00, 0.55, 0.10),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn say_format_is_name_colon_text() {
        assert_eq!(
            format_chat_line(ChatChannel::Say, "Daisy", "hi"),
            "Daisy : hi"
        );
    }

    #[test]
    fn shout_uses_same_format_as_say() {
        // Color differentiates them, not the layout.
        assert_eq!(
            format_chat_line(ChatChannel::Shout, "Daisy", "hi"),
            "Daisy : hi"
        );
    }

    #[test]
    fn tell_prepends_double_arrow() {
        assert_eq!(
            format_chat_line(ChatChannel::Tell, "Daisy", "hi"),
            ">>Daisy : hi"
        );
    }

    #[test]
    fn party_uses_parens_no_colon() {
        assert_eq!(
            format_chat_line(ChatChannel::Party, "Daisy", "hi"),
            "(Daisy) hi"
        );
    }

    #[test]
    fn linkshell_uses_angle_brackets_no_colon() {
        assert_eq!(
            format_chat_line(ChatChannel::Linkshell, "Daisy", "hi"),
            "<Daisy> hi"
        );
    }

    #[test]
    fn yell_uses_square_brackets() {
        assert_eq!(
            format_chat_line(ChatChannel::Yell, "Daisy", "hi"),
            "[Daisy] : hi"
        );
    }

    #[test]
    fn system_and_battle_omit_sender() {
        assert_eq!(
            format_chat_line(ChatChannel::System, "ignored", "Welcome to Vana'diel."),
            "Welcome to Vana'diel."
        );
        assert_eq!(
            format_chat_line(ChatChannel::Battle, "ignored", "Daisy hits the Mandragora for 12 points of damage."),
            "Daisy hits the Mandragora for 12 points of damage."
        );
    }

    #[test]
    fn empty_text_still_renders_sender_layout() {
        assert_eq!(format_chat_line(ChatChannel::Say, "Daisy", ""), "Daisy : ");
    }
}
