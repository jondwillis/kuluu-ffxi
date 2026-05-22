//! NPC event-dialog HUD panel. Read-only in C5 phase 1: surfaces the
//! metadata the server hands us for an in-progress event so the operator
//! has visibility while the event flows through.
//!
//! # Why a metadata panel and not a full text dialog
//!
//! 0x032/0x033/0x034 carry **no English text**. The narrative dialog
//! ("Welcome to Bastok, traveler!") lives in client-side `event.dat` /
//! `events.dat` files keyed off `event_num`. We don't ship those, so we
//! surface what we do have: the NPC reference (resolves to a name via
//! the entity table), the event id (xrefs Phoenix's lua scripts), the
//! mode (event vs cutscene vs menu), the four runtime strings (often
//! player names referenced by the event), and the eight runtime
//! integers (often counts / item ids / shop totals).
//!
//! Lifecycle: visible whenever `SceneState.snapshot.dialog` is `Some`.
//! Hidden via `Display::None` otherwise — toggle is cheaper than spawn-
//! despawn at 60 Hz, and matches the chat-input bar pattern.

use bevy::prelude::*;
use ffxi_viewer_wire::DialogState;

use crate::hud::palette;
use crate::input_mode::{InputMode, DIALOG_MAX_CHOICE};
use crate::snapshot::SceneState;

#[derive(Component)]
pub struct DialogPanel;

#[derive(Component)]
pub struct DialogHeader;

#[derive(Component)]
pub struct DialogBody;

/// One clickable choice button (`0..=DIALOG_MAX_CHOICE`). FFXI events
/// carry no English option text, so the buttons are bare numbers — the
/// operator chooses by index, same as the keyboard cursor + Enter path.
#[derive(Component)]
pub struct DialogChoiceButton {
    pub choice: u32,
}

/// Emitted when an operator clicks a choice button. Consumer in
/// `ffxi-client/src/view_native/text_input.rs` sends `EndEventChoice`
/// with the selected index, same as keyboard Enter on that cursor.
#[derive(Message, Debug, Clone, Copy)]
pub struct DialogChoiceActivated {
    pub choice: u32,
}

const PANEL_WIDTH_PX: f32 = 420.0;

pub fn spawn_dialog_panel(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            DialogPanel,
            Node {
                position_type: PositionType::Absolute,
                // Center horizontally, place a bit below mid-screen so it
                // doesn't collide with the target panel (top-center) or
                // the chat region (bottom-left).
                top: Val::Percent(40.0),
                left: Val::Percent(50.0),
                margin: UiRect {
                    left: Val::Px(-PANEL_WIDTH_PX / 2.0),
                    ..default()
                },
                width: Val::Px(PANEL_WIDTH_PX),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
        ))
        .with_children(|p| {
            p.spawn((
                DialogHeader,
                Text::new(""),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(palette::ACCENT),
            ));
            p.spawn((
                DialogBody,
                Text::new(""),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(palette::TEXT),
            ));
            // Numbered choice buttons row. FFXI events don't carry
            // option text, so the buttons are bare digits — same as
            // keyboard, the operator picks an index `0..=DIALOG_MAX_CHOICE`.
            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(4.0),
                margin: UiRect {
                    top: Val::Px(6.0),
                    ..default()
                },
                ..default()
            })
            .with_children(|row| {
                for choice in 0..=DIALOG_MAX_CHOICE {
                    row.spawn((
                        DialogChoiceButton { choice },
                        Button,
                        Node {
                            width: Val::Px(28.0),
                            height: Val::Px(22.0),
                            padding: UiRect::axes(Val::Px(2.0), Val::Px(0.0)),
                            border: UiRect::all(Val::Px(1.0)),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        BackgroundColor(palette::BACKGROUND),
                        BorderColor::all(palette::BORDER),
                    ))
                    .with_children(|btn| {
                        btn.spawn((
                            Text::new(choice.to_string()),
                            TextFont {
                                font_size: 12.0,
                                ..default()
                            },
                            TextColor(palette::TEXT),
                        ));
                    });
                }
            });
        });
}

pub fn update_dialog_panel_system(
    state: Res<SceneState>,
    mode: Res<InputMode>,
    mut panel_q: Query<&mut Node, With<DialogPanel>>,
    mut header_q: Query<&mut Text, (With<DialogHeader>, Without<DialogBody>)>,
    mut body_q: Query<&mut Text, (With<DialogBody>, Without<DialogHeader>)>,
) {
    if !state.is_changed() && !mode.is_changed() {
        return;
    }

    let Ok(mut panel_node) = panel_q.single_mut() else {
        return;
    };

    let snap = &state.snapshot;
    let Some(dialog) = snap.dialog.as_ref() else {
        if panel_node.display != Display::None {
            panel_node.display = Display::None;
        }
        return;
    };

    if panel_node.display == Display::None {
        panel_node.display = Display::Flex;
    }

    // Three-tier name resolution: prefer the session-pre-resolved
    // `dialog.npc_name` (filled from the session's id→name cache so
    // off-screen NPCs work), then the live snapshot's entity table
    // (covers in-range NPCs), then a hex placeholder so we never show
    // a blank header.
    let npc_name = dialog
        .npc_name
        .clone()
        .or_else(|| {
            snap.entities
                .iter()
                .find(|e| e.id == dialog.npc_id)
                .and_then(|e| e.name.clone())
        })
        .unwrap_or_else(|| format!("#{:08X}", dialog.npc_id));

    if let Ok(mut text) = header_q.single_mut() {
        let want = format!("{npc_name}    event #{}", dialog.event_num);
        if **text != want {
            **text = want;
        }
    }

    if let Ok(mut text) = body_q.single_mut() {
        let cursor = match &*mode {
            InputMode::Dialog(c) => Some(c.cursor),
            _ => None,
        };
        let want = format_body(dialog, cursor);
        if **text != want {
            **text = want;
        }
    }
}

/// Recolor each choice button so the currently-selected one (matching
/// the `DialogCursor`) shows the cyan accent border. Other buttons sit
/// in the muted state. Runs every frame in Dialog mode — cheap, since
/// the button count is `1 + DIALOG_MAX_CHOICE`.
pub fn update_dialog_choice_highlight_system(
    mode: Res<InputMode>,
    mut q: Query<(&DialogChoiceButton, &mut BorderColor)>,
) {
    let cursor = match &*mode {
        InputMode::Dialog(c) => c.cursor,
        _ => return,
    };
    for (btn, mut border) in &mut q {
        let want = if btn.choice == cursor {
            palette::ACCENT
        } else {
            palette::BORDER
        };
        if border.left != want {
            *border = BorderColor::all(want);
        }
    }
}

/// Move the dialog cursor to follow mouse hover. Active only while
/// `InputMode::Dialog` — the choice buttons are part of a panel that
/// hides via `Display::None` when no dialog is up, so their
/// `Interaction` stays Default in that case.
pub fn dialog_mouse_hover_system(
    mut mode: ResMut<InputMode>,
    q: Query<(&Interaction, &DialogChoiceButton), Changed<Interaction>>,
) {
    let InputMode::Dialog(cursor) = &mut *mode else {
        return;
    };
    for (interaction, btn) in &q {
        if matches!(interaction, Interaction::Hovered | Interaction::Pressed)
            && cursor.cursor != btn.choice
        {
            cursor.cursor = btn.choice;
        }
    }
}

/// Emit [`DialogChoiceActivated`] on choice-button click.
pub fn dialog_mouse_click_system(
    mode: Res<InputMode>,
    q: Query<(&Interaction, &DialogChoiceButton), Changed<Interaction>>,
    mut out: MessageWriter<DialogChoiceActivated>,
) {
    if !matches!(*mode, InputMode::Dialog(_)) {
        return;
    }
    for (interaction, btn) in &q {
        if *interaction == Interaction::Pressed {
            out.write(DialogChoiceActivated { choice: btn.choice });
        }
    }
}

/// Compose the multi-line body. `mode` and `event_para` always show;
/// `event_num2/event_para2` only when nonzero (most events leave them
/// at 0); strings/nums only when the carrying packet (0x033/0x034)
/// populated them. When `cursor` is `Some`, append the operator
/// instructions and a "→ choice = N" indicator so the dialog panel
/// doubles as the input affordance.
fn format_body(d: &DialogState, cursor: Option<u32>) -> String {
    let mut out = format!("mode={}  para={}", d.mode, d.event_para);
    if d.event_num2 != 0 || d.event_para2 != 0 {
        out.push_str(&format!("  para2={}/{}", d.event_num2, d.event_para2));
    }
    if !d.strings.is_empty() {
        out.push_str("\nstrings: ");
        out.push_str(&d.strings.join(" | "));
    }
    if !d.nums.is_empty() {
        out.push_str("\nnums: ");
        let parts: Vec<String> = d.nums.iter().map(|n| n.to_string()).collect();
        out.push_str(&parts.join(", "));
    }
    if let Some(c) = cursor {
        out.push_str(&format!(
            "\n\n→ choice = {c}  (↑↓ adjust · Enter send · Esc skip)",
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d() -> DialogState {
        DialogState {
            event_id: 0xDEAD_BEEF,
            npc_id: 0x1234,
            npc_name: None,
            act_index: 7,
            event_num: 42,
            event_para: 1,
            mode: 0,
            event_num2: 0,
            event_para2: 0,
            strings: vec![],
            nums: vec![],
        }
    }

    #[test]
    fn body_with_no_extras_is_just_mode_para() {
        assert_eq!(format_body(&d(), None), "mode=0  para=1");
    }

    #[test]
    fn body_includes_para2_only_when_nonzero() {
        let mut x = d();
        x.event_num2 = 5;
        let body = format_body(&x, None);
        assert!(body.contains("para2=5/0"));
    }

    #[test]
    fn body_includes_strings_and_nums_when_present() {
        let mut x = d();
        x.strings = vec!["Selh".into(), "Bastok".into()];
        x.nums = vec![100, 0, -1];
        let body = format_body(&x, None);
        assert!(body.contains("strings: Selh | Bastok"));
        assert!(body.contains("nums: 100, 0, -1"));
    }

    #[test]
    fn body_shows_cursor_and_hint_when_in_dialog_mode() {
        let body = format_body(&d(), Some(2));
        assert!(body.contains("→ choice = 2"), "got: {body}");
        assert!(body.contains("Enter send"));
    }
}
