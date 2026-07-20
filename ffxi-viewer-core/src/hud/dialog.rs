use bevy::prelude::*;
use ffxi_viewer_wire::DialogState;

use crate::hud::style::{self, theme};
use crate::input_mode::InputMode;
use crate::snapshot::SceneState;

#[derive(Component)]
pub struct DialogPanel;

#[derive(Component)]
pub struct DialogHeader;

#[derive(Component)]
pub struct DialogBody;

/// One selectable option row in the menu. `choice` is its 0-based index; rows
/// past the current menu's option count are hidden.
#[derive(Component)]
pub struct DialogChoiceButton {
    pub choice: u32,
}

/// The text label inside a [`DialogChoiceButton`] row, updated with the option's
/// text (and a cursor marker) each frame.
#[derive(Component)]
pub struct DialogOptionText {
    pub choice: u32,
}

#[derive(Message, Debug, Clone, Copy)]
pub struct DialogChoiceActivated {
    pub choice: u32,
}

const PANEL_WIDTH_PX: f32 = 420.0;
const CURSOR_MARKER: &str = "▸";
const CONTINUE_MARKER: &str = "▶";
/// Pooled option rows spawned once and shown/hidden per menu. FFXI talk menus
/// stay well under this; longer lists are clamped (and logged) at the input.
pub const MAX_OPTION_ROWS: u32 = 16;

pub fn spawn_dialog_panel(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            DialogPanel,
            Node {
                position_type: PositionType::Absolute,

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
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::FRAME_EDGE),
        ))
        .with_children(|p| {
            p.spawn((
                DialogHeader,
                Text::new(""),
                style::text_font(14.0),
                TextColor(theme::TITLE),
            ));
            p.spawn((
                DialogBody,
                Text::new(""),
                style::text_font(13.0),
                TextColor(theme::TEXT),
            ));

            p.spawn(Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(1.0),
                margin: UiRect {
                    top: Val::Px(6.0),
                    ..default()
                },
                ..default()
            })
            .with_children(|col| {
                for choice in 0..MAX_OPTION_ROWS {
                    col.spawn((
                        DialogChoiceButton { choice },
                        Button,
                        Node {
                            width: Val::Percent(100.0),
                            padding: UiRect::axes(Val::Px(4.0), Val::Px(1.0)),
                            display: Display::None,
                            ..default()
                        },
                        BackgroundColor(theme::FRAME_BG),
                    ))
                    .with_children(|btn| {
                        btn.spawn((
                            DialogOptionText { choice },
                            Text::new(""),
                            style::text_font(13.0),
                            TextColor(theme::TEXT),
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
        if **text != npc_name {
            **text = npc_name;
        }
    }

    if let Ok(mut text) = body_q.single_mut() {
        let (cursor, entry) = match &*mode {
            InputMode::Dialog(c) => (Some(c.cursor), c.entry.as_deref()),
            _ => (None, None),
        };
        let want = format_body(dialog, cursor, entry);
        if **text != want {
            **text = want;
        }
    }
}

/// Fill the pooled option rows from the current menu's choices (with a cursor
/// marker on the selected one) and hide unused rows. For plain speech (no
/// choices) all rows are hidden.
pub fn update_dialog_options_system(
    state: Res<SceneState>,
    mode: Res<InputMode>,
    mut rows: Query<(&DialogChoiceButton, &mut Node)>,
    mut labels: Query<(&DialogOptionText, &mut Text, &mut TextColor)>,
) {
    if !state.is_changed() && !mode.is_changed() {
        return;
    }
    let choices: &[String] = state
        .snapshot
        .dialog
        .as_ref()
        .map(|d| d.choices.as_slice())
        .unwrap_or(&[]);
    let cursor = match &*mode {
        InputMode::Dialog(c) => c.cursor,
        _ => 0,
    };

    for (btn, mut node) in &mut rows {
        let want = if (btn.choice as usize) < choices.len() {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
    }
    for (lbl, mut text, mut color) in &mut labels {
        let (want_text, want_color) = match choices.get(lbl.choice as usize) {
            Some(opt) if lbl.choice == cursor => (format!("{CURSOR_MARKER} {opt}"), theme::CURSOR),
            Some(opt) => (format!("  {opt}"), theme::TEXT),
            None => (String::new(), theme::TEXT),
        };
        if **text != want_text {
            **text = want_text;
        }
        if color.0 != want_color {
            color.0 = want_color;
        }
    }
}

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

fn format_body(d: &DialogState, cursor: Option<u32>, entry: Option<&str>) -> String {
    if d.text_entry {
        // Free-text frame (delivery-box recipient prompt): show the typed line
        // with a caret, retail-style.
        let prompt = d.prompt.as_deref().unwrap_or("Enter a name.");
        let typed = entry.unwrap_or("");
        return format!("{prompt}\n\n{typed}_\n\nEnter send · Esc cancel");
    }
    if let Some(prompt) = &d.prompt {
        // A menu's options render as separate rows, so the body is the prompt
        // alone; plain speech gets an advance hint.
        if d.choices.is_empty() {
            return format!("{prompt}\n\n{CONTINUE_MARKER} Enter to continue");
        }
        return prompt.clone();
    }

    // Fallback: no event DAT drove this dialog — show the raw packet params.
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
            prompt: None,
            choices: vec![],
            text_entry: false,
        }
    }

    #[test]
    fn body_with_no_extras_is_just_mode_para() {
        assert_eq!(format_body(&d(), None, None), "mode=0  para=1");
    }

    #[test]
    fn body_includes_para2_only_when_nonzero() {
        let mut x = d();
        x.event_num2 = 5;
        let body = format_body(&x, None, None);
        assert!(body.contains("para2=5/0"));
    }

    #[test]
    fn body_includes_strings_and_nums_when_present() {
        let mut x = d();
        x.strings = vec!["Selh".into(), "Bastok".into()];
        x.nums = vec![100, 0, -1];
        let body = format_body(&x, None, None);
        assert!(body.contains("strings: Selh | Bastok"));
        assert!(body.contains("nums: 100, 0, -1"));
    }

    #[test]
    fn body_shows_cursor_and_hint_when_in_dialog_mode() {
        let body = format_body(&d(), Some(2), None);
        assert!(body.contains("→ choice = 2"), "got: {body}");
        assert!(body.contains("Enter send"));
    }

    #[test]
    fn message_body_is_speech_plus_advance_hint() {
        let mut x = d();
        x.prompt = Some("Good luck, citizen.".into());
        let body = format_body(&x, None, None);
        assert!(body.starts_with("Good luck, citizen."), "got: {body}");
        assert!(body.contains("Enter to continue"));
    }

    #[test]
    fn menu_body_is_prompt_only_options_render_as_rows() {
        let mut x = d();
        x.prompt = Some("What do you want?".into());
        x.choices = vec!["Cast Signet".into(), "Set home point".into()];
        let body = format_body(&x, Some(0), None);
        assert_eq!(body, "What do you want?");
        assert!(!body.contains("Cast Signet"));
    }

    #[test]
    fn text_entry_body_shows_prompt_typed_line_and_caret() {
        let mut x = d();
        x.text_entry = true;
        x.prompt = Some("Who will you send it to?".into());
        let body = format_body(&x, Some(0), Some("Selh"));
        assert!(body.starts_with("Who will you send it to?"), "got: {body}");
        assert!(body.contains("Selh_"), "got: {body}");
        assert!(body.contains("Enter send · Esc cancel"));
        // Choice-cursor hint must not leak into the text-entry frame.
        assert!(!body.contains("→ choice"));
    }

    #[test]
    fn text_entry_body_defaults_prompt_and_empty_line() {
        let mut x = d();
        x.text_entry = true;
        let body = format_body(&x, None, None);
        assert!(body.starts_with("Enter a name."), "got: {body}");
        assert!(body.contains("\n\n_\n"), "got: {body}");
    }
}
