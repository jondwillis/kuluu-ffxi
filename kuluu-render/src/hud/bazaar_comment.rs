//! The `Edit Comment` field: the small window that stands directly above the chat log once the
//! `Set Bazaar` panel's second row is confirmed (owner 2026-10-24). What it commits becomes the
//! message buyers read when they check our stall, carried by c2s 0x0DE
//! GP_CLI_COMMAND_INSPECT_MESSAGE (vendor/server/src/map/packets/c2s/0x0de_inspect_message.h);
//! LSB keeps at most [`ffxi_proto::decode::BAZAAR_COMMENT_MAX_BYTES`] of it verbatim and answers
//! nothing.

use bevy::prelude::*;

use crate::hud::item_ui::{framed_box, text_font, theme};
use crate::hud::menu::DynamicMenu;
use crate::input_mode::{InputMode, MenuKind};

/// The stall windows all wear one frame width, and so does this field.
const FIELD_WIDTH_PX: f32 = 372.0;
/// One line at the scale every stall window draws its text at.
const FIELD_TEXT_PX: f32 = 12.0;
/// The most of the draft the box can hold on one line; longer input keeps its tail so the caret
/// stays in sight while typing.
const FIELD_VISIBLE_CHARS: usize = 48;

#[derive(Component)]
pub(crate) struct CommentField;

#[derive(Component)]
pub(crate) struct CommentFieldText;

/// Stand the field in the bottom-left stack, above the chat log it belongs to. It draws only
/// while [`comment_field_open`] holds.
pub(crate) fn spawn_bazaar_comment_editor(p: &mut ChildSpawnerCommands) {
    let (mut node, bg, border) = framed_box();
    node.width = Val::Px(FIELD_WIDTH_PX);
    node.display = Display::None;
    p.spawn((CommentField, node, bg, border))
        .with_children(|field| {
            field.spawn((
                CommentFieldText,
                Text::new(""),
                text_font(FIELD_TEXT_PX),
                TextColor(theme::TEXT),
                TextLayout {
                    linebreak: LineBreak::NoWrap,
                    ..default()
                },
            ));
        });
}

/// The comment field owns the menu stack.
pub(crate) fn comment_field_open(mode: &InputMode) -> bool {
    matches!(
        mode,
        InputMode::Menu(stack)
            if stack.current().is_some_and(|level| level.kind == MenuKind::BazaarComment)
    )
}

/// Draft plus the caret kuluu draws at its end (the same `text_` idiom as the chat bar).
pub(crate) fn field_text(draft: &str) -> String {
    let start = draft.chars().count().saturating_sub(FIELD_VISIBLE_CHARS);
    draft
        .chars()
        .skip(start)
        .chain(std::iter::once('_'))
        .collect()
}

pub(crate) fn update_bazaar_comment_editor(
    mode: Res<InputMode>,
    dynamic: Res<DynamicMenu>,
    mut field_q: Query<&mut Node, With<CommentField>>,
    mut text_q: Query<&mut Text, With<CommentFieldText>>,
) {
    let open = comment_field_open(&mode);
    for mut node in field_q.iter_mut() {
        let want = if open { Display::Flex } else { Display::None };
        if node.display != want {
            node.display = want;
        }
    }
    if !open {
        return;
    }
    let shown = field_text(&dynamic.bazaar_comment_draft);
    for mut text in text_q.iter_mut() {
        if text.as_str() != shown.as_str() {
            **text = shown.clone();
        }
    }
}

/// Whether appending `bytes` keeps the draft inside what LSB stores. The key handler stops at the
/// cap rather than letting a longer draft silently lose its tail on send.
pub fn fits(draft: &str, bytes: usize) -> bool {
    draft.len() + bytes <= ffxi_proto::decode::BAZAAR_COMMENT_MAX_BYTES
}
