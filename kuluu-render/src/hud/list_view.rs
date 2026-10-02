//! The paged row list retail's item windows draw: a fixed page of fixed-height
//! rows, a page offset that is state rather than a function of the cursor, and
//! a scrollbar that reports where in the list the page sits.

use bevy::prelude::*;

use crate::hud::item_ui::theme;

/// Rows one page of a retail item list draws
/// (.agents/skills/retail-observe/references/2026-09-11-items-window.md).
pub const LIST_ROWS: usize = 10;

/// The icon plate on a list row.
pub const ROW_ICON_PX: f32 = 18.0;

/// A row is exactly as tall as its icon. Fixing the height is what stops a
/// label too long for its column from wrapping to a second line and growing the
/// list out from under the page.
pub const ROW_HEIGHT_PX: f32 = ROW_ICON_PX;

const ROW_GAP_PX: f32 = 5.0;

const SCROLLBAR_WIDTH_PX: f32 = 4.0;

/// First visible row of a list. Retail scrolls one row when the cursor leaves
/// the page and shifts the page along with the cursor on Left/Right, so the
/// offset is state, not a function of the cursor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListViewport {
    pub start: usize,
}

impl ListViewport {
    pub fn max_start(total: usize) -> usize {
        total.saturating_sub(LIST_ROWS)
    }

    /// Pull the page just far enough to keep `cursor` drawn.
    pub fn follow(&mut self, cursor: usize, total: usize) {
        if cursor < self.start {
            self.start = cursor;
        } else if cursor >= self.start + LIST_ROWS {
            self.start = cursor + 1 - LIST_ROWS;
        }
        self.start = self.start.min(Self::max_start(total));
    }

    /// Left/Right: the page shifts by a full page in step with the cursor (see
    /// [`page_cursor`]), clamped at both ends, no wrap.
    pub fn page(&mut self, forward: bool, total: usize) {
        self.start = if forward {
            (self.start + LIST_ROWS).min(Self::max_start(total))
        } else {
            self.start.saturating_sub(LIST_ROWS)
        };
    }
}

/// Up/Down in a list: one row, clamped at both ends (retail: Up on the first
/// row and Down on the last stay put).
pub fn step_cursor(cursor: usize, total: usize, down: bool) -> usize {
    if down {
        (cursor + 1).min(total.saturating_sub(1))
    } else {
        cursor.saturating_sub(1)
    }
}

/// Left/Right in a list: the cursor jumps a full page, clamped.
pub fn page_cursor(cursor: usize, total: usize, forward: bool) -> usize {
    if forward {
        (cursor + LIST_ROWS).min(total.saturating_sub(1))
    } else {
        cursor.saturating_sub(LIST_ROWS)
    }
}

/// A list row's plate.
pub fn row_node() -> Node {
    Node {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        column_gap: Val::Px(ROW_GAP_PX),
        height: Val::Px(ROW_HEIGHT_PX),
        flex_shrink: 0.0,
        overflow: Overflow::clip(),
        ..default()
    }
}

/// The label column of a list row: takes the slack the other columns leave and
/// clips whatever still does not fit, rather than wrapping. The label's text
/// goes *inside* this node, because a node's clip bounds its children and not
/// its own glyphs. Text reports its whole width as its basis, so the basis
/// starts at 0 or a long name would take the fixed columns' space instead of
/// being clipped.
pub fn row_label_clip() -> Node {
    Node {
        flex_grow: 1.0,
        flex_basis: Val::Px(0.0),
        min_width: Val::Px(0.0),
        overflow: Overflow::clip(),
        ..default()
    }
}

/// Single-line text layout for a row label.
pub fn row_label_layout() -> TextLayout {
    TextLayout {
        justify: Justify::Left,
        linebreak: LineBreak::NoWrap,
    }
}

/// Retail paints the cursor row gold and dims a row the player cannot act on
/// (a ware they cannot afford, an entry the window will not take).
pub fn row_color(cursor: bool, enabled: bool) -> Color {
    match (cursor, enabled) {
        (true, _) => theme::CURSOR,
        (false, true) => theme::TEXT,
        (false, false) => theme::FAINT,
    }
}

/// Rows one unit of wheel travel walks. A notch reports 1.0 and a trackpad
/// reports a stream of small fractions, so the carry below is what makes both
/// feel like the same list.
pub(crate) const WHEEL_ROWS_PER_UNIT: f32 = 0.12;

/// Walk `current` by the rows in `delta`, carrying the fraction that does not
/// reach a whole row. Result is clamped to `0..buffer_len`, and the carry is
/// dropped at either end so a long push does not bank travel to spend on the
/// way back.
pub fn apply_wheel_delta(
    current: usize,
    accum: f32,
    delta: f32,
    buffer_len: usize,
) -> (usize, f32) {
    if buffer_len == 0 {
        return (current, 0.0);
    }
    let mut frac = accum + delta * WHEEL_ROWS_PER_UNIT;

    let whole = frac.trunc() as i32;
    frac -= whole as f32;
    let max_rows = buffer_len.saturating_sub(1) as i32;
    let next = (current as i32 + whole).clamp(0, max_rows);

    let frac = if (next == 0 && whole < 0) || (next == max_rows && whole > 0) {
        0.0
    } else {
        frac
    };
    (next as usize, frac)
}

/// Track + thumb for a list that outgrows its page. Each caller tags its own
/// pair so two windows' scrollbars do not answer one query.
pub fn spawn_scrollbar(parent: &mut ChildSpawnerCommands, track: impl Bundle, thumb: impl Bundle) {
    parent
        .spawn((
            track,
            Node {
                width: Val::Px(SCROLLBAR_WIDTH_PX),
                align_self: AlignSelf::Stretch,
                flex_shrink: 0.0,
                display: Display::None,
                ..default()
            },
            BackgroundColor(theme::CELL_BG),
        ))
        .with_children(|track| {
            track.spawn((
                thumb,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    top: Val::Percent(0.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(theme::FRAME_EDGE),
            ));
        });
}

/// Where the thumb sits and how much of the list the page covers, as fractions
/// of the track. `None` for a list that fits on one page and so has no bar.
pub fn thumb_geometry(start: usize, total: usize) -> Option<(f32, f32)> {
    (total > LIST_ROWS).then(|| (start as f32 / total as f32, LIST_ROWS as f32 / total as f32))
}

/// Drive a scrollbar spawned by [`spawn_scrollbar`] from the page it reports.
pub fn apply_scrollbar(track: &mut Node, thumb: &mut Node, start: usize, total: usize) {
    let Some((top, height)) = thumb_geometry(start, total) else {
        if track.display != Display::None {
            track.display = Display::None;
        }
        return;
    };
    if track.display != Display::Flex {
        track.display = Display::Flex;
    }
    let top = Val::Percent(top * 100.0);
    let height = Val::Percent(height * 100.0);
    if thumb.top != top {
        thumb.top = top;
    }
    if thumb.height != height {
        thumb.height = height;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_follows_the_cursor_one_row_at_a_time() {
        const TOTAL: usize = 30;
        let mut v = ListViewport::default();
        v.follow(LIST_ROWS - 1, TOTAL);
        assert_eq!(v.start, 0, "the last drawn row needs no scroll");
        v.follow(LIST_ROWS, TOTAL);
        assert_eq!(v.start, 1, "one past the page scrolls by one row");
        v.follow(0, TOTAL);
        assert_eq!(v.start, 0, "moving above the page scrolls back up");
    }

    #[test]
    fn a_shrinking_list_pulls_the_page_back() {
        let mut v = ListViewport { start: 12 };
        v.follow(0, 4);
        assert_eq!(v.start, 0, "a list that fits has nothing to scroll");
    }

    #[test]
    fn a_list_that_fits_draws_no_scrollbar() {
        assert_eq!(thumb_geometry(0, LIST_ROWS), None);
        let (top, height) = thumb_geometry(0, LIST_ROWS * 2).expect("scrollable");
        assert_eq!((top, height), (0.0, 0.5));
        let (top, _) = thumb_geometry(LIST_ROWS, LIST_ROWS * 2).expect("scrollable");
        assert_eq!(top, 0.5, "the thumb sits where the page does");
    }

    /// The list's height must not depend on what is in it: a name too long for
    /// its column is clipped on its row instead of wrapping onto a second line
    /// and pushing the rows below it down.
    #[test]
    fn a_row_is_a_fixed_height_and_its_label_does_not_wrap() {
        assert_eq!(row_node().height, Val::Px(ROW_HEIGHT_PX));
        assert_eq!(row_node().overflow, Overflow::clip());
        assert_eq!(row_label_layout().linebreak, LineBreak::NoWrap);
        assert_eq!(row_label_clip().overflow, Overflow::clip());
        assert_eq!(
            row_label_clip().flex_basis,
            Val::Px(0.0),
            "a long label may not take the fixed columns' space"
        );
    }

    #[test]
    fn a_row_the_player_cannot_act_on_dims_and_the_cursor_row_stays_gold() {
        assert_eq!(row_color(false, true), theme::TEXT);
        assert_eq!(row_color(false, false), theme::FAINT);
        assert_eq!(row_color(true, false), theme::CURSOR);
    }

    #[test]
    fn cursor_steps_clamp_at_both_ends() {
        assert_eq!(step_cursor(0, 30, false), 0);
        assert_eq!(step_cursor(29, 30, true), 29);
        assert_eq!(page_cursor(0, 30, true), LIST_ROWS);
        assert_eq!(page_cursor(25, 30, true), 29);
        assert_eq!(page_cursor(5, 30, false), 0);
    }
}
