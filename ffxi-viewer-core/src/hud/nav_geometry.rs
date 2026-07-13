//! Pure navigation-geometry helpers shared by every drill-down/scrolling HUD
//! menu, so the list/grid/pane math backing each menu's "layout kind" is
//! implemented once rather than re-derived per menu.

use bevy::prelude::Resource;

/// Focus state for a dual-pane menu: a primary list plus a small secondary
/// box (e.g. the Items window's sort-options box) that can steal keyboard
/// focus. The primary pane owns focus by default.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct PaneFocus {
    pub secondary_focused: bool,
    pub secondary_cursor: usize,
}

/// First visible row index for a `visible_rows`-tall scrolling window over a
/// `total`-row list, centered on `cursor` and clamped so the window never
/// runs past either end.
pub fn scroll_window(cursor: usize, total: usize, visible_rows: usize) -> usize {
    if total <= visible_rows {
        return 0;
    }
    let half = visible_rows / 2;
    let max_start = total - visible_rows;
    cursor.saturating_sub(half).min(max_start)
}

/// Step a flat list cursor by `delta`, wrapping past either end (e.g. Up at
/// row 0 lands on the last row). `count == 0` always yields `0`.
pub fn list_step_wrapping(cursor: usize, count: usize, delta: i32) -> usize {
    if count == 0 {
        return 0;
    }
    (cursor as i32 + delta).rem_euclid(count as i32) as usize
}

/// Step a flat list cursor by `delta`, clamped at `0` and `count - 1` rather
/// than wrapping (dialog choice lists use this — Up at the top choice stays
/// put instead of jumping to the bottom). `count == 0` always yields `0`.
pub fn list_step_clamped(cursor: usize, count: usize, delta: i32) -> usize {
    if count == 0 {
        return 0;
    }
    (cursor as i32 + delta).clamp(0, count as i32 - 1) as usize
}

/// Move a 2D grid cursor by `(dx, dy)` cells, wrapping past each edge.
/// `table` is row-major (`table[row][col]`); `current` is searched for by
/// value each call rather than tracked as a separate `(row, col)`, so grid
/// menus can keep storing their cursor as the cell's own id type (e.g. an
/// equipment slot enum) instead of a row/col pair.
pub fn grid_step<const R: usize, const C: usize, T: Copy + PartialEq>(
    table: &[[T; C]; R],
    current: T,
    dx: i32,
    dy: i32,
) -> T {
    let mut cell = (0usize, 0usize);
    for (r, row) in table.iter().enumerate() {
        for (c, &t) in row.iter().enumerate() {
            if t == current {
                cell = (r, c);
            }
        }
    }
    let nr = (cell.0 as i32 + dy).rem_euclid(R as i32) as usize;
    let nc = (cell.1 as i32 + dx).rem_euclid(C as i32) as usize;
    table[nr][nc]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_window_keeps_short_lists_at_top() {
        assert_eq!(scroll_window(0, 5, 13), 0);
        assert_eq!(scroll_window(4, 5, 13), 0);
        assert_eq!(scroll_window(0, 13, 13), 0);
    }

    #[test]
    fn scroll_window_centers_and_clamps() {
        let total = 40;
        let rows = 13;
        assert_eq!(scroll_window(0, total, rows), 0);
        let mid = total / 2;
        assert_eq!(scroll_window(mid, total, rows), mid - rows / 2);
        assert_eq!(scroll_window(total - 1, total, rows), total - rows);
    }

    #[test]
    fn list_step_wrapping_wraps_at_both_ends() {
        assert_eq!(list_step_wrapping(0, 5, -1), 4);
        assert_eq!(list_step_wrapping(4, 5, 1), 0);
        assert_eq!(list_step_wrapping(2, 5, 1), 3);
        assert_eq!(list_step_wrapping(0, 0, 1), 0);
    }

    #[test]
    fn list_step_clamped_stops_at_both_ends() {
        assert_eq!(list_step_clamped(0, 5, -1), 0);
        assert_eq!(list_step_clamped(4, 5, 1), 4);
        assert_eq!(list_step_clamped(2, 5, 1), 3);
        assert_eq!(list_step_clamped(0, 0, 1), 0);
    }

    #[test]
    fn grid_step_wraps_and_steps() {
        let table = [[0, 1, 2], [3, 4, 5]];
        assert_eq!(grid_step(&table, 0, 1, 0), 1);
        assert_eq!(grid_step(&table, 2, 1, 0), 0);
        assert_eq!(grid_step(&table, 0, 0, 1), 3);
        assert_eq!(grid_step(&table, 3, 0, 1), 0);
        assert_eq!(grid_step(&table, 0, -1, 0), 2);
    }
}
