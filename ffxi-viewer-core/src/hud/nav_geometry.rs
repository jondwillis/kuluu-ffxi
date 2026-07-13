//! Pure navigation-geometry helpers shared by every drill-down/scrolling HUD
//! menu, so the list/grid/pane math backing each menu's "layout kind" is
//! implemented once rather than re-derived per menu.

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
}
