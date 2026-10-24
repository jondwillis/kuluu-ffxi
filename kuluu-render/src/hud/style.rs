//! The single style source for the entire HUD: in-game windows (menus, items,
//! shop, trade, dialog…) *and* dev/diagnostic overlays (diagnostics bar,
//! stage bar, network status, mesh debug). Everything takes colors and frames
//! from here so they cannot drift apart; the old `hud::palette` dev-overlay
//! module was folded into `theme` and removed.

use bevy::prelude::*;

/// Menus draw above the chat stack (which owns the lower-left quadrant), matching
/// the other pop-up windows (target-action / quick-action at 20, trade at 25).
pub const WINDOW_Z: i32 = 20;

/// The chat stack draws under every other HUD window, so a menu that reaches the log covers it
/// instead of moving it.
pub const CHAT_STACK_Z: i32 = -1;

/// Retail stacks the self, target, and party panels in one right-hand column, so
/// their width, inset, and gap are a shared contract, not independent tunings.
/// `hud::panel_column` owns the stacking; panels never position themselves
/// relative to each other.
pub const PANEL_WIDTH_PX: f32 = 220.0;
pub const PANEL_COLUMN_RIGHT_PX: f32 = 8.0;
/// Clearance under the column, leaving the chat stack's top edge visible.
pub const PANEL_COLUMN_BOTTOM_PX: f32 = 28.0;
pub const PANEL_COLUMN_GAP_PX: f32 = 8.0;

/// Our "blue" window theme: a translucent navy frame with a light steel-blue
/// edge, pale-blue title text, near-white body text, and a golden cursor
/// highlight on the focused row/slot. These are deliberate tunings (retail
/// ships no palette file to scrape), named here per no-magic-numbers.
pub mod theme {
    use bevy::prelude::Color;

    pub const FRAME_BG: Color = Color::srgba(0.05, 0.07, 0.16, 0.88);
    // Opaque FRAME_BG for map surfaces only: a translucent backing behind the
    // minimap/map image shimmers where the map texture is transparent, because
    // the alpha composites over the live moving 3D scene as the player walks
    // (kuluu-bi1s.1). Opaque backing = static fill, no edge flash.
    pub const MAP_BACKING: Color = Color::srgb(0.05, 0.07, 0.16);
    pub const FRAME_EDGE: Color = Color::srgb(0.60, 0.69, 0.85);
    pub const TITLE: Color = Color::srgb(0.80, 0.89, 1.0);
    pub const TEXT: Color = Color::srgb(0.91, 0.92, 0.95);
    pub const MUTED: Color = Color::srgb(0.58, 0.63, 0.74);
    pub const CURSOR: Color = Color::srgb(1.0, 0.84, 0.36);
    pub const CURSOR_BG: Color = Color::srgba(0.20, 0.28, 0.45, 0.65);
    pub const CELL_BG: Color = Color::srgba(0.10, 0.13, 0.24, 0.85);
    pub const CELL_EDGE: Color = Color::srgb(0.32, 0.38, 0.52);
    pub const DANGER: Color = Color::srgb(0.95, 0.35, 0.35);
    pub const WARN: Color = Color::srgb(1.0, 0.82, 0.30);
    pub const GOOD: Color = Color::srgb(0.45, 0.90, 0.55);
    pub const FAINT: Color = Color::srgb(0.44, 0.48, 0.58);
    /// A row the client will not answer. Retail keeps the same glyph colour and halves
    /// its intensity — ARGB 0x80808080 becomes 0x80404040 on a gated roomlist row
    /// (FFXiMain.dll retail-2026-09 `.text 0x1D6A1A`), and the same halving covers the
    /// `statcom2` rows disabled at open (`.text 0x148B50`). Half of TEXT.
    pub const TEXT_DISABLED: Color = Color::srgb(0.455, 0.46, 0.475);
    /// A persona attribute row's positive adjustment span: retail pushes the raw ARGB
    /// 0x80408040 when `bp_adj` is above zero (FFXiMain.dll retail-2026-09 `.text 0x147530`).
    pub const ATTR_ADJUST_UP: Color = Color::srgb(0.251, 0.502, 0.251);
    /// A persona attribute row's negative adjustment span — retail pushes ARGB
    /// 0x80804040 when `bp_adj` is below zero (FFXiMain.dll retail-2026-09 `.text 0x14754C`).
    pub const ATTR_ADJUST_DOWN: Color = Color::srgb(0.502, 0.251, 0.251);
    /// An item row wearable right now. Retail's per-row colour resolvers paint ARGB
    /// 0x80909040 when the equip legality check answers true (FFXiMain.dll retail-2026-09
    /// `.text 0x1DE18D`, repeated at `.text 0x20F788` and `.text 0x1EC8DA`).
    pub const TEXT_EQUIP_OK: Color = Color::srgb(0.565, 0.565, 0.251);
    /// An item row whose id already stands in one of the equip slots the client scans —
    /// ARGB 0x80509050 (FFXiMain.dll retail-2026-09 `.text 0x1DE173`, scan helper
    /// `.text 0x131FA0`), which takes precedence over the wearable-now lime.
    pub const TEXT_EQUIP_SAME_KIND: Color = Color::srgb(0.314, 0.565, 0.314);
}

/// The row prefix marking the cursor in every list menu — "> " on the selected
/// row, two spaces otherwise — so the items, main-menu, quick-action and
/// target-action lists all indent identically.
pub fn cursor_prefix(is_cursor: bool) -> &'static str {
    if is_cursor {
        "> "
    } else {
        "  "
    }
}

pub fn text_font(size: f32) -> TextFont {
    TextFont {
        font_size: size.into(),
        ..default()
    }
}

/// The inset a framed window leaves between its border and its content. A window that sizes itself
/// to its rows needs the same numbers, so they live here rather than only inside `window_frame`.
pub const WINDOW_PAD_X_PX: f32 = 10.0;
pub const WINDOW_PAD_Y_PX: f32 = 8.0;

/// A framed window panel: translucent navy fill, steel-blue 1px border,
/// column layout. Every game window's outer chrome goes through this.
pub fn window_frame() -> (Node, BackgroundColor, BorderColor) {
    (
        Node {
            padding: UiRect::axes(Val::Px(WINDOW_PAD_X_PX), Val::Px(WINDOW_PAD_Y_PX)),
            border: UiRect::all(Val::Px(1.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(2.0),
            ..default()
        },
        BackgroundColor(theme::FRAME_BG),
        BorderColor::all(theme::FRAME_EDGE),
    )
}
