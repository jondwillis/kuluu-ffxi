use bevy::prelude::*;
use kuluu_snapshot::DialogState;

use crate::hud::item_dat_root::{ItemDatRoot, ItemIconCache};
use crate::hud::item_grid::{spawn_item_cell, CellOverlay, CELL_GAP_PX, CELL_PX};
use crate::hud::item_ui::transparent_placeholder;
use crate::hud::style::{self, theme};
use crate::hud_hide::HudHideExempt;
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

/// Container for the delivery-box style item grid shown inside the dialog
/// panel when the server exposes a `DialogGrid` in the dialog state.
#[derive(Component)]
pub struct DialogGridBox;

#[derive(Component)]
pub struct DialogGridCellFrame {
    pub index: usize,
}

#[derive(Component)]
pub struct DialogGridIcon {
    pub index: usize,
}

#[derive(Component)]
pub struct DialogGridLabel {
    pub index: usize,
}

/// Retail delivery box is a 2x4 grid.
pub const MAX_GRID_CELLS: usize = 8;

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

pub fn spawn_dialog_panel(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = transparent_placeholder(&mut images);
    commands
        .spawn((
            crate::components::InGameEntity,
            // Retail's HIDE_HUD (0x67) presets the event-message box to its own
            // mode instead of hiding it with the HUD: CTkMsgBase.cpp DisableDraw
            // only disables primitive drawing when field_48 == 0, so the cutscene
            // text box stays visible through HIDE_HUD (research/XiEvents/OpCodes/
            // 0x0067.md). The panel must survive apply_hud_hidden with it.
            HudHideExempt,
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

            p.spawn((
                DialogGridBox,
                Node {
                    display: Display::None,
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: Val::Px(CELL_GAP_PX),
                    row_gap: Val::Px(CELL_GAP_PX),
                    margin: UiRect {
                        top: Val::Px(6.0),
                        ..default()
                    },
                    ..default()
                },
            ))
            .with_children(|g| {
                for index in 0..MAX_GRID_CELLS {
                    spawn_item_cell(
                        g,
                        DialogGridCellFrame { index },
                        DialogGridIcon { index },
                        DialogGridLabel { index },
                        CellOverlay::StackCount,
                        placeholder.clone(),
                    );
                }
            });

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
    // The dedicated delivery screen owns the UI while a box is open; suppress
    // the generic dialog panel so a settling Mog-menu frame can't double-render.
    let dialog = snap.dialog.as_ref().filter(|_| snap.delivery_box.is_none());
    let Some(dialog) = dialog else {
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

#[allow(clippy::too_many_arguments)]
pub fn update_dialog_grid_system(
    state: Res<SceneState>,
    mode: Res<InputMode>,
    dat_root: Res<ItemDatRoot>,
    mut icon_cache: ResMut<ItemIconCache>,
    mut images: ResMut<Assets<Image>>,
    mut box_q: Query<&mut Node, With<DialogGridBox>>,
    mut cell_q: Query<
        (&DialogGridCellFrame, &mut Node, &mut BorderColor),
        (Without<DialogGridBox>, Without<DialogGridIcon>),
    >,
    mut icon_q: Query<
        (&DialogGridIcon, &mut Node, &mut ImageNode),
        (Without<DialogGridBox>, Without<DialogGridCellFrame>),
    >,
    mut label_q: Query<
        (&DialogGridLabel, &mut Text, &mut Node),
        (
            Without<DialogGridBox>,
            Without<DialogGridCellFrame>,
            Without<DialogGridIcon>,
        ),
    >,
) {
    if !state.is_changed() && !mode.is_changed() {
        return;
    }
    let Ok(mut box_node) = box_q.single_mut() else {
        return;
    };
    let grid = state.snapshot.dialog.as_ref().and_then(|d| d.grid.as_ref());
    let Some(grid) = grid else {
        if box_node.display != Display::None {
            box_node.display = Display::None;
        }
        return;
    };

    let cols = grid.cols.max(1) as f32;
    let want_width = Val::Px(cols * CELL_PX + (cols - 1.0) * CELL_GAP_PX);
    if box_node.width != want_width {
        box_node.width = want_width;
    }
    if box_node.display != Display::Flex {
        box_node.display = Display::Flex;
    }

    let cursor = match &*mode {
        InputMode::Dialog(c) => Some(c.cursor),
        _ => None,
    };

    for (frame, mut node, mut border) in &mut cell_q {
        let cell = grid.cells.get(frame.index);
        let display = if cell.is_some() {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
        let selected = cell
            .and_then(|c| c.choice)
            .is_some_and(|choice| Some(choice) == cursor);
        let want_edge = if selected {
            theme::CURSOR
        } else {
            theme::CELL_EDGE
        };
        let want_border = BorderColor::all(want_edge);
        if *border != want_border {
            *border = want_border;
        }
    }

    for (icon, mut node, mut image) in &mut icon_q {
        let cell = grid.cells.get(icon.index);
        let item = cell.and_then(|c| c.item_no);
        let handle = item.and_then(|n| icon_cache.ensure(n, &dat_root, &mut images));
        match handle {
            Some(h) => {
                if image.image != h {
                    image.image = h;
                }
                let want_color = if cell.is_some_and(|c| c.sent) {
                    Color::srgba(1.0, 1.0, 1.0, 0.35)
                } else {
                    Color::WHITE
                };
                if image.color != want_color {
                    image.color = want_color;
                }
                if node.display != Display::Flex {
                    node.display = Display::Flex;
                }
            }
            None => {
                if node.display != Display::None {
                    node.display = Display::None;
                }
            }
        }
    }

    for (label, mut text, mut node) in &mut label_q {
        let cell = grid.cells.get(label.index);
        let want = match cell {
            Some(c) if c.item_no.is_some() && c.quantity > 1 => c.quantity.to_string(),
            _ => String::new(),
        };
        let display = if want.is_empty() {
            Display::None
        } else {
            Display::Flex
        };
        if **text != want {
            **text = want;
        }
        if node.display != display {
            node.display = display;
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
    let dialog = state.snapshot.dialog.as_ref();
    let choices: &[String] = dialog.map(|d| d.choices.as_slice()).unwrap_or(&[]);
    // Choices represented by grid cells are navigated (and highlighted) on the
    // icon grid itself; suppress their duplicate flat rows so only the
    // non-grid rows (recipient, Cancel) render as a list.
    let in_grid = |choice: u32| -> bool {
        dialog
            .and_then(|d| d.grid.as_ref())
            .is_some_and(|g| g.cells.iter().any(|c| c.choice == Some(choice)))
    };
    let cursor = match &*mode {
        InputMode::Dialog(c) => c.cursor,
        _ => 0,
    };

    for (btn, mut node) in &mut rows {
        let want = if (btn.choice as usize) < choices.len() && !in_grid(btn.choice) {
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
            _ if in_grid(lbl.choice) => (String::new(), theme::TEXT),
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
        // alone; plain speech gets an advance hint — except frames retail
        // dismisses on its own clock (the `7F 34/35/36 NN` auto-prompt code,
        // research/cexi-docs/dialog/format.md): those show no ▼ prompt.
        if d.choices.is_empty() {
            if d.auto_advance.is_some() {
                return prompt.clone();
            }
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
            grid: None,
            custom_menu: false,
            cancel_armed: true,
            speaker_index: None,
            contains_item: false,
            auto_advance: None,
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

    /// A retail auto-prompt frame (the `7F 34/35/36 NN` code) dismisses itself
    /// on the session's clock, so the box shows no ▼ prompt of its own.
    #[test]
    fn auto_advance_body_shows_no_advance_hint() {
        let mut x = d();
        x.prompt = Some("The year is 1156.".into());
        x.auto_advance = Some(5);
        let body = format_body(&x, None, None);
        assert_eq!(body, "The year is 1156.");
        assert!(!body.contains("Enter to continue"));
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

    // Headless draw-level proof for event 503 (Southern San d'Oria new-character
    // CS): feed the render systems the session's REAL recorded frames and assert
    // the panel nodes actually flip visible with the right text — a drawn box,
    // not just an emitted event. Frames are the post-PT-fix decodes of
    // ROM/25/39.DAT entries 7668+, recorded against local LSB.
    mod event503_draw {
        use super::*;
        use crate::input_mode::{DialogCursor, InputMode};
        use bevy::ecs::system::RunSystemOnce;

        /// Event 503's first narration frame (ROM/25/39.DAT entry 7668), as the
        /// session emits it: speakerless line carries a blank header, no choices.
        fn narration_frame() -> DialogState {
            let mut x = d();
            x.event_id = (6u32 << 16) | 503;
            x.npc_id = 6;
            x.npc_name = Some(String::new());
            x.act_index = 1024;
            x.event_para = 503;
            x.prompt = Some(
                "The fortress city of San d'Oria lies to the\nnorth on the great continent of Quon. The\nbeating heart of an ancient kingdom, it is\nhome to a thousand legends past."
                    .to_string(),
            );
            x
        }

        /// Event 503's second choice menu (G4a), exact recorded strings.
        fn g4a_menu_frame() -> DialogState {
            let mut x = narration_frame();
            x.prompt = Some("What would you like to hear about?".into());
            x.choices = vec![
                "Helping people.".into(),
                "Hunting monsters.".into(),
                "Making easy money.".into(),
                "Working for my country.".into(),
            ];
            x
        }

        /// Bare world with the panel spawned hidden, as `add_hud_spawners` leaves it.
        fn app_with_panel() -> App {
            let mut app = App::new();
            app.init_resource::<Assets<Image>>()
                .init_resource::<SceneState>()
                .insert_resource(InputMode::default());
            app.world_mut().run_system_once(spawn_dialog_panel).unwrap();
            app
        }

        /// The two systems that own the panel's visible state; in the live app
        /// they run after `dialog_mode_sync_system` has already flipped World→Dialog.
        fn run_draw(app: &mut App) {
            app.world_mut()
                .run_system_once(update_dialog_panel_system)
                .unwrap();
            app.world_mut()
                .run_system_once(update_dialog_options_system)
                .unwrap();
        }

        fn panel_display(world: &mut World) -> Display {
            world
                .query_filtered::<&Node, With<DialogPanel>>()
                .iter(world)
                .next()
                .expect("panel root spawned")
                .display
        }

        fn header_text(world: &mut World) -> String {
            world
                .query_filtered::<&Text, (With<DialogHeader>, Without<DialogBody>)>()
                .iter(world)
                .next()
                .expect("header spawned")
                .0
                .clone()
        }

        fn body_text(world: &mut World) -> String {
            world
                .query_filtered::<&Text, (With<DialogBody>, Without<DialogHeader>)>()
                .iter(world)
                .next()
                .expect("body spawned")
                .0
                .clone()
        }

        fn row_displays(world: &mut World) -> Vec<Display> {
            let mut rows: Vec<(u32, Display)> = world
                .query::<(&DialogChoiceButton, &Node)>()
                .iter(world)
                .map(|(b, n)| (b.choice, n.display))
                .collect();
            rows.sort_by_key(|(choice, _)| *choice);
            rows.into_iter().map(|(_, d)| d).collect()
        }

        fn row_labels(world: &mut World) -> Vec<String> {
            let mut labels: Vec<(u32, String)> = world
                .query::<(&DialogOptionText, &Text)>()
                .iter(world)
                .map(|(l, t)| (l.choice, t.0.clone()))
                .collect();
            labels.sort_by_key(|(choice, _)| *choice);
            labels.into_iter().map(|(_, s)| s).collect()
        }

        fn row_colors(world: &mut World) -> Vec<Color> {
            let mut colors: Vec<(u32, Color)> = world
                .query::<(&DialogOptionText, &TextColor)>()
                .iter(world)
                .map(|(l, c)| (l.choice, c.0))
                .collect();
            colors.sort_by_key(|(choice, _)| *choice);
            colors.into_iter().map(|(_, c)| c).collect()
        }

        #[test]
        fn panel_spawns_hidden_and_stays_there_without_a_frame() {
            let mut app = app_with_panel();
            run_draw(&mut app);
            assert_eq!(panel_display(app.world_mut()), Display::None);
            assert!(row_displays(app.world_mut())
                .iter()
                .all(|d| *d == Display::None));
        }

        #[test]
        fn event503_narration_frame_draws_the_panel() {
            let mut app = app_with_panel();
            {
                let mut state = app.world_mut().resource_mut::<SceneState>();
                state.snapshot.dialog = Some(narration_frame());
            }
            app.world_mut()
                .insert_resource(InputMode::Dialog(DialogCursor {
                    cursor: 0,
                    entry: None,
                }));
            run_draw(&mut app);

            assert_eq!(
                panel_display(app.world_mut()),
                Display::Flex,
                "panel root must flip visible (same-frame: dialog_mode_sync_system already flipped the mode)"
            );
            assert_eq!(
                header_text(app.world_mut()),
                "",
                "narration header must be blank (speakerless, not the player's name)"
            );
            let want_body = format!(
                "The fortress city of San d'Oria lies to the\nnorth on the great continent of Quon. The\nbeating heart of an ancient kingdom, it is\nhome to a thousand legends past.\n\n{CONTINUE_MARKER} Enter to continue"
            );
            assert_eq!(body_text(app.world_mut()), want_body);
            let displays = row_displays(app.world_mut());
            assert_eq!(
                displays.len(),
                MAX_OPTION_ROWS as usize,
                "no choices: every pooled row stays hidden with empty labels"
            );
            assert!(displays.iter().all(|d| *d == Display::None));
            assert!(row_labels(app.world_mut()).iter().all(|s| s.is_empty()));
        }

        #[test]
        fn event503_g4a_menu_draws_four_rows_with_cursor_on_first() {
            let mut app = app_with_panel();
            {
                let mut state = app.world_mut().resource_mut::<SceneState>();
                state.snapshot.dialog = Some(g4a_menu_frame());
            }
            app.world_mut()
                .insert_resource(InputMode::Dialog(DialogCursor {
                    cursor: 0,
                    entry: None,
                }));
            run_draw(&mut app);

            assert_eq!(panel_display(app.world_mut()), Display::Flex);
            assert_eq!(
                body_text(app.world_mut()),
                "What would you like to hear about?",
                "a menu's body is the prompt alone; options render as rows"
            );

            let displays = row_displays(app.world_mut());
            for (i, d) in displays.iter().enumerate() {
                let want = if i < 4 { Display::Flex } else { Display::None };
                assert_eq!(*d, want, "row {i} display");
            }
            let mut want_labels = vec![
                format!("{CURSOR_MARKER} Helping people."),
                "  Hunting monsters.".into(),
                "  Making easy money.".into(),
                "  Working for my country.".into(),
            ];
            want_labels.extend(std::iter::repeat_n(String::new(), 12));
            assert_eq!(row_labels(app.world_mut()), want_labels);
            let colors = row_colors(app.world_mut());
            assert_eq!(colors[0], theme::CURSOR, "cursor row gets the cursor color");
            for c in &colors[1..4] {
                assert_eq!(*c, theme::TEXT);
            }
        }

        #[test]
        fn event503_menu_cursor_move_relabels_rows() {
            let mut app = app_with_panel();
            {
                let mut state = app.world_mut().resource_mut::<SceneState>();
                state.snapshot.dialog = Some(g4a_menu_frame());
            }
            app.world_mut()
                .insert_resource(InputMode::Dialog(DialogCursor {
                    cursor: 0,
                    entry: None,
                }));
            run_draw(&mut app);

            let mut mode = app.world_mut().resource_mut::<InputMode>();
            if let InputMode::Dialog(c) = &mut *mode {
                c.cursor = 2;
            } else {
                panic!("expected Dialog mode");
            }
            run_draw(&mut app);

            let labels = row_labels(app.world_mut());
            assert_eq!(labels[0], "  Helping people.");
            assert_eq!(labels[2], format!("{CURSOR_MARKER} Making easy money."));
        }

        /// Event 503's A6 HIDE_HUD is live through every narration beat (B1–B8)
        /// and the gate approach; retail keeps the event-message box drawable
        /// through it, so the panel root must survive `apply_hud_hidden` while a
        /// plain HUD root hides.
        #[test]
        fn cutscene_hud_hide_keeps_the_dialog_panel_visible() {
            use crate::hud_hide::{apply_hud_hidden, HudHidden, HudHideStash};
            let mut app = app_with_panel();
            app.init_resource::<HudHidden>()
                .init_resource::<HudHideStash>();

            // Node requires Visibility (bevy_ui ui_node.rs #[require]), so the
            // panel root carries a default Inherited in any world — same as live.
            let panel = app
                .world_mut()
                .query_filtered::<Entity, With<DialogPanel>>()
                .iter(app.world())
                .next()
                .expect("panel root");
            assert!(app.world().get::<Visibility>(panel).is_some());
            let plain_root = app
                .world_mut()
                .spawn((Node::default(), Visibility::Inherited))
                .id();

            app.world_mut().resource_mut::<HudHidden>().cutscene = true;
            app.world_mut().run_system_once(apply_hud_hidden).unwrap();

            let panel_vis = *app
                .world()
                .get::<Visibility>(panel)
                .expect("panel root visibility");
            assert_eq!(
                panel_vis,
                Visibility::Inherited,
                "retail keeps the event-message box visible through HIDE_HUD"
            );
            let plain_vis = *app
                .world()
                .get::<Visibility>(plain_root)
                .expect("plain root visibility");
            assert_eq!(plain_vis, Visibility::Hidden, "plain HUD roots still hide");
        }

        #[test]
        fn closing_the_dialog_hides_the_panel_again() {
            let mut app = app_with_panel();
            {
                let mut state = app.world_mut().resource_mut::<SceneState>();
                state.snapshot.dialog = Some(g4a_menu_frame());
            }
            app.world_mut()
                .insert_resource(InputMode::Dialog(DialogCursor {
                    cursor: 0,
                    entry: None,
                }));
            run_draw(&mut app);
            assert_eq!(panel_display(app.world_mut()), Display::Flex);

            // EventEnded clears the dialog (state.rs AgentEvent::EventEnded arm).
            let mut state = app.world_mut().resource_mut::<SceneState>();
            state.snapshot.dialog = None;
            run_draw(&mut app);

            assert_eq!(
                panel_display(app.world_mut()),
                Display::None,
                "panel must hide on close"
            );
            assert!(row_displays(app.world_mut())
                .iter()
                .all(|d| *d == Display::None));
        }

        #[test]
        fn grid_draw_runs_with_disjoint_node_queries() {
            use kuluu_snapshot::{DialogGrid, DialogGridCell};

            let mut app = app_with_panel();
            app.world_mut().init_resource::<ItemDatRoot>();
            app.world_mut().init_resource::<ItemIconCache>();
            {
                let mut state = app.world_mut().resource_mut::<SceneState>();
                let mut frame = d();
                frame.grid = Some(DialogGrid {
                    cols: 2,
                    rows: 1,
                    cells: vec![
                        DialogGridCell {
                            choice: Some(0),
                            item_no: Some(4096),
                            quantity: 3,
                            sent: false,
                        },
                        DialogGridCell::default(),
                    ],
                });
                state.snapshot.dialog = Some(frame);
            }
            // B0001: the box/frame/icon/label queries all take &mut Node, so the
            // system only initializes if their filters keep them disjoint.
            app.world_mut()
                .run_system_once(update_dialog_grid_system)
                .unwrap();

            let box_node = app
                .world_mut()
                .query_filtered::<&Node, With<DialogGridBox>>()
                .iter(app.world())
                .next()
                .expect("grid box spawned")
                .clone();
            assert_eq!(box_node.display, Display::Flex);
            let want_cols = 2.0;
            assert_eq!(
                box_node.width,
                Val::Px(want_cols * CELL_PX + (want_cols - 1.0) * CELL_GAP_PX)
            );

            let mut counts: Vec<String> = app
                .world_mut()
                .query_filtered::<(&DialogGridLabel, &Text), ()>()
                .iter(app.world())
                .filter(|(l, _)| l.index < 2)
                .map(|(l, t)| format!("{}:{}", l.index, t.as_str()))
                .collect();
            counts.sort();
            assert_eq!(counts, vec!["0:3".to_string(), "1:".to_string()]);
        }
    }
}
