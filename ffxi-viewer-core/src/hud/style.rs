//! The single style source for in-game windows (menus, items, shop, trade,
//! dialog…). Dev/diagnostic overlays keep their own dev colors (`hud/mod.rs`);
//! game windows must take colors and frames from here so they cannot drift
//! apart.

use bevy::prelude::*;

/// Menus draw above the chat stack (which owns the lower-left quadrant), matching
/// the other pop-up windows (target-action / quick-action at 20, trade at 25).
pub const WINDOW_Z: i32 = 20;

/// Our "blue" window theme: a translucent navy frame with a light steel-blue
/// edge, pale-blue title text, near-white body text, and a golden cursor
/// highlight on the focused row/slot. These are deliberate tunings (retail
/// ships no palette file to scrape), named here per no-magic-numbers.
pub mod theme {
    use bevy::prelude::Color;

    pub const FRAME_BG: Color = Color::srgba(0.05, 0.07, 0.16, 0.88);
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
}

pub fn text_font(size: f32) -> TextFont {
    TextFont {
        font_size: size.into(),
        ..default()
    }
}

/// A framed window panel: translucent navy fill, steel-blue 1px border,
/// column layout. Every game window's outer chrome goes through this.
pub fn window_frame() -> (Node, BackgroundColor, BorderColor) {
    (
        Node {
            padding: UiRect::axes(Val::Px(10.0), Val::Px(8.0)),
            border: UiRect::all(Val::Px(1.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(2.0),
            ..default()
        },
        BackgroundColor(theme::FRAME_BG),
        BorderColor::all(theme::FRAME_EDGE),
    )
}
