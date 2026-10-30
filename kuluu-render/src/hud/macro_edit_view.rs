//! One macro keytop, edited in its own window: a Prev/Next pair that steps the book's twenty keytops,
//! one row carrying that keytop's hotkey word beside its display name, and under them six command line
//! slots (owner ruling 2026-10-24, annotated on his capture of retail's window: `<Prev next>`,
//! `button - name`, `6 lines`). It stands in the band's corner while it is open and nothing else draws
//! with it (see [`crate::hud::menu::focus_excludes_ancestors`]) - which is what Prev/Next are for.
//!
//! No menu-window record carries this layout: `mcrledit`/`mcrbedit` hold only Copy/Paste/Delete blocks
//! and every `mcr*edit`/`mcr*long` grid is the keytop band itself (read them with
//! `cargo run -p ffxi-dat --example dat-menu-window-print -- mcr`), so the arrangement comes from the
//! owner's capture. Only the corner comes from the DATs, shared with the band whose keytops are stepped.

use bevy::prelude::*;

use crate::hud::client_user_files::{ENTRY_NAME_BYTES, MACRO_BOOK_SLOTS};
use crate::hud::macro_pallet_view::PALLET_WINDOW_NAME;
use crate::hud::menu::{
    macro_edit_control_of_field, macro_edit_hotkey_word, ConfigDatRows, MACRO_EDIT_CONTROL_COUNT,
    MACRO_EDIT_FIELD_COUNT, MACRO_EDIT_LINE_SLOTS, MACRO_EDIT_NEXT_CONTROL,
    MACRO_EDIT_PREV_CONTROL,
};
use crate::input_mode::{InputMode, MenuKind};
use crate::ui_font;

/// Pinned corner for a host whose DATs have not been read yet, transcribed from the band's record.
const PINNED_FRAME_X: f32 = 16.0;
const PINNED_FRAME_Y: f32 = 48.0;

/// Every field and button draws at kuluu's menu row type, so this window's words match the lists around
/// it when it is not open alone.
const FIELD_TEXT_PX: f32 = 14.0;
/// Air inside a box, between rows, and around them inside the frame.
const FIELD_PAD_X_PX: f32 = 5.0;
const FIELD_PAD_Y_PX: f32 = 3.0;
const ROW_GAP_PX: f32 = 6.0;
const PANE_PAD_PX: f32 = 8.0;
/// The gutter between a keytop's hotkey word and the name field it names.
const HOTKEY_GUTTER_PX: f32 = 6.0;
/// How many characters one line field is wide. Longer input keeps its tail, so the caret stays in sight
/// while he types (the law the bazaar comment field runs).
const LINE_VISIBLE_CHARS: usize = 25;

#[derive(Component)]
pub struct MacroEditPanel;

/// One box of the window, and which box it is. Prev/Next are buttons, `Hotkey` prints the key word,
/// `Name` and `Line(i)` are the editable fields.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MacroEditBox {
    Prev,
    Next,
    Hotkey,
    Name,
    Line(usize),
}

impl MacroEditBox {
    /// The control this box is, in the order the window's cursor walks them: `Prev`, `Next`, then every
    /// editable field top to bottom. The hotkey word is not a control at all - it labels the name field.
    fn control(self) -> Option<usize> {
        match self {
            Self::Prev => Some(MACRO_EDIT_PREV_CONTROL),
            Self::Next => Some(MACRO_EDIT_NEXT_CONTROL),
            // The display name is the first editable field; its line slots follow in order.
            Self::Name => Some(macro_edit_control_of_field(0)),
            Self::Line(line_no) => Some(macro_edit_control_of_field(line_no + 1)),
            Self::Hotkey => None,
        }
    }

    /// Whether this box is the one under the window's cursor.
    fn on_cursor(self, cursor: usize) -> bool {
        self.control() == Some(cursor.min(MACRO_EDIT_CONTROL_COUNT - 1))
    }

    /// What this box prints when it is not an editable field: the two step buttons and the hotkey word
    /// carry fixed words.
    fn fixed_label(self, hotkey_word: &'static str) -> &'static str {
        match self {
            Self::Prev => PREV_LABEL,
            Self::Next => NEXT_LABEL,
            Self::Hotkey => hotkey_word,
            _ => "",
        }
    }

    /// The room one field shows, in characters including its caret.
    fn room_chars(self) -> usize {
        match self {
            Self::Name => ENTRY_NAME_BYTES,
            Self::Line(_) => LINE_VISIBLE_CHARS + 1,
            _ => 0,
        }
    }

    /// Whether this box is drawn as a box at all: the hotkey word floats on the pane's own fill.
    fn framed(self) -> bool {
        !matches!(self, Self::Hotkey)
    }
}

const PREV_LABEL: &str = "\u{25c4} Prev";
const NEXT_LABEL: &str = "Next \u{25ba}";

fn chars_width_px(chars: usize) -> f32 {
    ui_font::text_width_px(&"0".repeat(chars), FIELD_TEXT_PX)
}

fn button_width(label: &str) -> f32 {
    ui_font::text_width_px(label, FIELD_TEXT_PX) + 2.0 * FIELD_PAD_X_PX
}

/// One row is one line of type plus its padding; every row of the pane is that tall.
fn row_height_px() -> f32 {
    ui_font::line_height_px(FIELD_TEXT_PX) + 2.0 * FIELD_PAD_Y_PX
}

/// The widest hotkey word this window can be asked to print, so switching keytops never reflows the
/// name field under the caret.
fn hotkey_word_width_px() -> f32 {
    (0..crate::hud::client_user_files::MACRO_BOOK_SLOTS)
        .map(|slot| ui_font::text_width_px(macro_edit_hotkey_word(slot), FIELD_TEXT_PX))
        .fold(0.0f32, f32::max)
}

/// Where one part of the pane sits: `x, y, w, h` in the pane's own box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PartRect {
    pub kind: MacroEditPart,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MacroEditPart {
    Prev,
    Next,
    Hotkey,
    Name,
    Line(usize),
}

/// The whole pane: its size and every part's place. Measured from this file's type metrics, which for a
/// monospace font is exact rather than estimated.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PaneLayout {
    pub width: f32,
    pub height: f32,
    pub rects: Vec<PartRect>,
}

/// Lay the pane out. Prev sits flush left and Next flush right on a line of their own; below them the
/// hotkey word stands beside the name field; the six line slots run edge to edge. The pane is as wide as
/// its widest row, so nothing crowds and nothing overlaps (owner 2026-10-24: "still to fucking wide" -
/// width comes from what a command line needs, not from the design column).
pub(crate) fn pane_layout() -> PaneLayout {
    let row_h = row_height_px();
    let prev_w = button_width(PREV_LABEL);
    let next_w = button_width(NEXT_LABEL);
    let hotkey_w = hotkey_word_width_px();
    // A name field holds everything a keytop name can be, plus the caret that marks it.
    let name_w = chars_width_px(ENTRY_NAME_BYTES) + 2.0 * FIELD_PAD_X_PX;
    let line_w = chars_width_px(LINE_VISIBLE_CHARS + 1) + 2.0 * FIELD_PAD_X_PX;

    let inner_width = (prev_w + ROW_GAP_PX + next_w)
        .max(hotkey_w + HOTKEY_GUTTER_PX + name_w)
        .max(line_w);

    let mut rects = Vec::with_capacity(MACRO_EDIT_FIELD_COUNT + 2);
    let button_y = PANE_PAD_PX;
    rects.push(PartRect {
        kind: MacroEditPart::Prev,
        x: PANE_PAD_PX,
        y: button_y,
        width: prev_w,
        height: row_h,
    });
    rects.push(PartRect {
        kind: MacroEditPart::Next,
        x: PANE_PAD_PX + inner_width - next_w,
        y: button_y,
        width: next_w,
        height: row_h,
    });

    let name_y = button_y + row_h + ROW_GAP_PX;
    rects.push(PartRect {
        kind: MacroEditPart::Hotkey,
        x: PANE_PAD_PX,
        y: name_y,
        width: hotkey_w,
        height: row_h,
    });
    rects.push(PartRect {
        kind: MacroEditPart::Name,
        x: PANE_PAD_PX + hotkey_w + HOTKEY_GUTTER_PX,
        y: name_y,
        width: name_w,
        height: row_h,
    });

    let first_line_y = name_y + row_h + ROW_GAP_PX;
    for line_no in 0..MACRO_EDIT_LINE_SLOTS {
        rects.push(PartRect {
            kind: MacroEditPart::Line(line_no),
            x: PANE_PAD_PX,
            y: first_line_y + (row_h + ROW_GAP_PX) * line_no as f32,
            width: inner_width,
            height: row_h,
        });
    }

    let last = rects.last().copied().unwrap_or(PartRect {
        kind: MacroEditPart::Name,
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: row_h,
    });
    PaneLayout {
        width: inner_width + 2.0 * PANE_PAD_PX,
        height: last.y + last.height + PANE_PAD_PX,
        rects,
    }
}

/// Stand the pane and its boxes up, hidden until a keytop editor holds focus. Boxes are pooled at spawn
/// and only re-filled afterwards, so an open editor allocates nothing per frame.
pub(crate) fn spawn_macro_edit_view(mut commands: Commands) {
    let layout = pane_layout();
    commands
        .spawn((
            crate::components::InGameEntity,
            MacroEditPanel,
            Node {
                position_type: PositionType::Absolute,
                display: Display::None,
                width: Val::Px(layout.width),
                height: Val::Px(layout.height),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(crate::hud::style::theme::FRAME_BG),
            BorderColor::all(crate::hud::style::theme::FRAME_EDGE),
            ZIndex(crate::hud::style::WINDOW_Z),
        ))
        .with_children(|pane| {
            for rect in layout.rects {
                let part = match rect.kind {
                    MacroEditPart::Prev => MacroEditBox::Prev,
                    MacroEditPart::Next => MacroEditBox::Next,
                    MacroEditPart::Hotkey => MacroEditBox::Hotkey,
                    MacroEditPart::Name => MacroEditBox::Name,
                    MacroEditPart::Line(line_no) => MacroEditBox::Line(line_no),
                };
                let label = match part {
                    MacroEditBox::Prev => PREV_LABEL.to_string(),
                    MacroEditBox::Next => NEXT_LABEL.to_string(),
                    _ => String::new(),
                };
                pane.spawn((
                    part,
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(rect.x),
                        top: Val::Px(rect.y),
                        width: Val::Px(rect.width),
                        height: Val::Px(rect.height),
                        border: UiRect::all(Val::Px(1.0)),
                        ..default()
                    },
                    BackgroundColor(if part.framed() {
                        crate::hud::style::theme::CELL_BG
                    } else {
                        Color::NONE
                    }),
                    BorderColor::all(if part.framed() {
                        crate::hud::style::theme::CELL_EDGE
                    } else {
                        Color::NONE
                    }),
                    Text::new(label),
                    crate::hud::style::text_font(FIELD_TEXT_PX),
                    TextColor(crate::hud::style::theme::TEXT),
                    TextLayout {
                        justify: Justify::Left,
                        linebreak: LineBreak::NoWrap,
                    },
                ));
            }
        });
}

/// A field's value as it shows on screen, cut to `room_chars` including its caret when the caret is
/// here. The head of the value goes first so the tail - whatever he is typing - stays visible.
pub(crate) fn field_text(value: &str, caret: bool, room_chars: usize) -> String {
    let keep = room_chars.saturating_sub(usize::from(caret));
    let skip = value.chars().count().saturating_sub(keep);
    let mut shown: String = value.chars().skip(skip).collect();
    if caret {
        shown.push('_');
    }
    shown
}

/// Which keytop is under edit, and where its caret sits.
#[derive(Debug, Clone, Copy)]
struct EditTarget {
    book: usize,
    slot: usize,
    cursor: usize,
}

impl EditTarget {
    fn of(mode: &InputMode) -> Option<Self> {
        let InputMode::Menu(stack) = mode else {
            return None;
        };
        let level = stack.levels.last()?;
        let MenuKind::MacroEdit { book, slot } = level.kind else {
            return None;
        };
        Some(Self {
            book: usize::from(book),
            slot: usize::from(slot),
            cursor: level.cursor.min(MACRO_EDIT_CONTROL_COUNT - 1),
        })
    }

    /// The window's draft, or the entry it opened on when nothing has been typed yet: either way every
    /// field wears one working copy (see [`crate::input_mode::MacroEditDraft`]).
    fn draft(self, mode: &InputMode) -> crate::input_mode::MacroEditDraft {
        let typed = match mode {
            InputMode::Menu(stack) => stack
                .levels
                .last()
                .and_then(|level| level.macro_edit_draft.clone()),
            _ => None,
        };
        typed.unwrap_or_else(|| {
            let files = crate::hud::client_user_files::client_user_files();
            let entry = files
                .macro_entry(self.book, self.slot.min(MACRO_BOOK_SLOTS - 1))
                .cloned()
                .unwrap_or_default();
            crate::input_mode::MacroEditDraft::new(&entry.name, &entry.body)
        })
    }
}

/// Draw the editor for the keytop its window names, and hide it whenever anything else holds focus. The
/// fields wear the level's draft, so an edit shows before any of it is stored; a window opened without
/// one reads his own macro file through [`crate::hud::client_user_files`].
pub(crate) fn update_macro_edit_view(
    mode: Res<InputMode>,
    config_rows: Res<ConfigDatRows>,
    mut panel_q: Query<&mut Node, With<MacroEditPanel>>,
    // The boxes never move (their places come from `pane_layout`, which no data changes), so this pass
    // only fills them: no `&mut Node` to conflict with the pane's own.
    #[allow(clippy::type_complexity)] mut box_q: Query<
        (
            &MacroEditBox,
            &mut Text,
            &mut TextColor,
            &mut BackgroundColor,
            &mut BorderColor,
        ),
        Without<MacroEditPanel>,
    >,
) {
    let Ok(mut panel) = panel_q.single_mut() else {
        return;
    };
    let Some(target) = EditTarget::of(&mode) else {
        if panel.display != Display::None {
            panel.display = Display::None;
        }
        return;
    };
    if panel.display != Display::Flex {
        panel.display = Display::Flex;
    }

    let layout = pane_layout();
    if panel.width != Val::Px(layout.width) {
        panel.width = Val::Px(layout.width);
    }
    if panel.height != Val::Px(layout.height) {
        panel.height = Val::Px(layout.height);
    }
    // The pane stands in the band's corner: that record seat once it has been read, pinned before then.
    let (frame_x, frame_y) = config_rows
        .record_frame(PALLET_WINDOW_NAME)
        .map(|frame| (frame.x, frame.y))
        .unwrap_or((PINNED_FRAME_X, PINNED_FRAME_Y));
    if panel.left != Val::Px(frame_x) {
        panel.left = Val::Px(frame_x);
    }
    if panel.top != Val::Px(frame_y) {
        panel.top = Val::Px(frame_y);
    }

    let draft = target.draft(&mode);

    for (part, mut text, mut color, mut fill, mut edge) in box_q.iter_mut() {
        let on_cursor = part.on_cursor(target.cursor);
        let value = match *part {
            MacroEditBox::Name => Some(draft.name.as_str()),
            MacroEditBox::Line(line_no) => draft.lines.get(line_no).map(String::as_str),
            _ => None,
        };
        let shown = if part.room_chars() > 0 {
            field_text(value.unwrap_or_default(), on_cursor, part.room_chars())
        } else {
            part.fixed_label(macro_edit_hotkey_word(target.slot))
                .to_string()
        };
        if **text != shown.as_str() {
            **text = shown;
        }
        let want_fill = if !part.framed() {
            Color::NONE
        } else if on_cursor {
            crate::hud::style::theme::CURSOR_BG
        } else {
            crate::hud::style::theme::CELL_BG
        };
        if fill.0 != want_fill {
            fill.0 = want_fill;
        }
        let want_edge = if !part.framed() {
            Color::NONE
        } else if on_cursor {
            crate::hud::style::theme::CURSOR
        } else {
            crate::hud::style::theme::CELL_EDGE
        };
        if edge.left != want_edge {
            *edge = BorderColor::all(want_edge);
        }
        let want_color = match *part {
            _ if on_cursor => crate::hud::style::theme::CURSOR,
            MacroEditBox::Hotkey => crate::hud::style::theme::TEXT,
            MacroEditBox::Name | MacroEditBox::Line(_) => {
                let empty = value.map(str::is_empty).unwrap_or(true);
                if empty {
                    crate::hud::style::theme::FAINT
                } else {
                    crate::hud::style::theme::TEXT
                }
            }
            MacroEditBox::Prev | MacroEditBox::Next => crate::hud::style::theme::TEXT,
        };
        if color.0 != want_color {
            color.0 = want_color;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bevy checks a system's parameter isolation the first time it runs, so this surface (a pane and its
    /// boxes, both taking `&mut Node`) would ship broken without a pass that schedules them.
    #[test]
    fn the_pane_survives_a_frame_of_its_own_systems() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<InputMode>()
            .init_resource::<ConfigDatRows>()
            .add_systems(Startup, spawn_macro_edit_view)
            .add_systems(Update, update_macro_edit_view);
        app.update();

        let mut panels = app
            .world_mut()
            .query_filtered::<&Node, With<MacroEditPanel>>();
        assert_eq!(
            panels.single(app.world()).expect("one pane").display,
            Display::None,
            "the pane waits for a focused keytop editor"
        );
    }

    #[test]
    fn every_part_is_placed_once_and_the_pane_holds_them() {
        // Prev, Next, the hotkey word, the name field and every line slot.
        let layout = pane_layout();
        assert_eq!(layout.rects.len(), MACRO_EDIT_FIELD_COUNT + 3);
        for rect in &layout.rects {
            assert!(rect.x >= PANE_PAD_PX && rect.y >= PANE_PAD_PX);
            assert!(rect.width > 0.0 && rect.height > 0.0);
            assert!(rect.x + rect.width <= layout.width + f32::EPSILON);
            assert!(rect.y + rect.height <= layout.height + f32::EPSILON);
        }
        // Every line slot runs the same width, one under the other.
        let lines: Vec<&PartRect> = layout
            .rects
            .iter()
            .filter(|r| matches!(r.kind, MacroEditPart::Line(_)))
            .collect();
        assert_eq!(lines.len(), MACRO_EDIT_LINE_SLOTS);
        for pair in lines.windows(2) {
            assert_eq!(pair[0].width, pair[1].width);
            assert!(pair[1].y > pair[0].y);
        }
        // Prev is the leftmost thing and Next the rightmost of their line.
        let prev = layout
            .rects
            .iter()
            .find(|r| r.kind == MacroEditPart::Prev)
            .copied()
            .expect("a Prev box");
        let next = layout
            .rects
            .iter()
            .find(|r| r.kind == MacroEditPart::Next)
            .copied()
            .expect("a Next box");
        assert!(next.x > prev.x + prev.width);
    }

    /// Prev and Next come first in the window's own order, so Left/Right can select them and Enter press
    /// them (owner ruling 2026-10-24), and every box after them is a field in drawing order.
    #[test]
    fn the_cursor_walks_the_buttons_first_then_every_field() {
        assert_eq!(MacroEditBox::Prev.control(), Some(0));
        assert_eq!(MacroEditBox::Next.control(), Some(1));
        assert_eq!(MacroEditBox::Hotkey.control(), None);
        assert_eq!(MacroEditBox::Name.control(), Some(2));
        assert_eq!(MacroEditBox::Line(0).control(), Some(3));
        let last_line = MACRO_EDIT_LINE_SLOTS - 1;
        assert_eq!(
            MacroEditBox::Line(last_line).control(),
            Some(MACRO_EDIT_CONTROL_COUNT - 1)
        );
        // One box per cursor position, and the hotkey word never takes it.
        for control in 0..MACRO_EDIT_CONTROL_COUNT {
            assert_eq!(MacroEditBox::Prev.on_cursor(control), control == 0);
            assert!(!MacroEditBox::Hotkey.on_cursor(control));
        }
        assert!(MacroEditBox::Line(last_line).on_cursor(MACRO_EDIT_CONTROL_COUNT + 7));
    }

    #[test]
    fn a_field_keeps_the_end_of_what_does_not_fit() {
        // Room for the caret either way, so it never walks out of its box.
        assert_eq!(field_text("/ma \"cure\" <t>", true, 7), "e\" <t>_");
        assert_eq!(field_text("short", false, 4), "hort");
        let long = field_text(
            "/pulverize_on_everything_i_see",
            true,
            LINE_VISIBLE_CHARS + 1,
        );
        assert_eq!(long.chars().count(), LINE_VISIBLE_CHARS + 1);
        assert!(long.ends_with('_'));
        assert!(long.ends_with("i_see_"), "the tail went: {long}");
    }
}
