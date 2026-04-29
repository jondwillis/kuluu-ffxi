//! Heads-up display: thin Bevy UI tree mirroring the chrome aesthetic of
//! `ffxi-client/src/chrome.rs`. We re-implement the look in Bevy UI rather
//! than importing chrome.rs because chrome.rs is owned by the parallel
//! session and is in the no-touch list.
//!
//! Layout (Stage 0c — only `stage_bar` is wired today; rest land in 0d):
//!
//! ```text
//! ┌────────────────────────────────────────────────────┐
//! │ ▌ ffxi-client ● in-zone ▪ Sylvie ▪ zone 230        │  ← stage_bar
//! ├────────────────────────────────────────────────────┤
//! │  diagnostics │  party        │  3D scene          │  ← (0d) split
//! ├────────────────────────────────────────────────────┤
//! │  chat                                              │  ← (0d)
//! └────────────────────────────────────────────────────┘
//! ```

pub mod agent_hud;
pub mod chat_input;
pub mod chat_panel;
pub mod diagnostics;
pub mod llm_badge;
pub mod menu;
pub mod quick_action;
pub mod roster;
pub mod stage_bar;

use bevy::prelude::*;

/// Color palette mirroring chrome.rs `Color::DarkGray` / `Color::Cyan` /
/// `Color::Green` / `Color::Yellow` / `Color::Red`. Defined once here so
/// every HUD module reads from the same constants.
pub mod palette {
    use bevy::prelude::Color;

    pub const BORDER: Color = Color::srgb(0.40, 0.40, 0.40);
    pub const BACKGROUND: Color = Color::srgb(0.04, 0.04, 0.04);
    pub const ACCENT: Color = Color::srgb(0.0, 1.0, 1.0); // cyan
    pub const TEXT: Color = Color::srgb(0.95, 0.95, 0.95);
    pub const MUTED: Color = Color::srgb(0.55, 0.55, 0.55);
    pub const DARK: Color = Color::srgb(0.40, 0.40, 0.40);

    pub const STAGE_IDLE: Color = DARK;
    pub const STAGE_TRANSITIONING: Color = Color::srgb(1.0, 0.85, 0.0); // yellow
    pub const STAGE_GOOD: Color = Color::srgb(0.0, 0.85, 0.0); // green
    pub const STAGE_BAD: Color = Color::srgb(0.95, 0.20, 0.20); // red
}

/// HUD aggregator plugin. Stage 0c registers `stage_bar` only. Stage 0d
/// will add `chat_panel`, `diagnostics`, `roster`.
pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<llm_badge::BadgeClock>();
        app.add_systems(
            Startup,
            (
                stage_bar::spawn_stage_bar,
                chat_panel::spawn_chat_panel,
                diagnostics::spawn_diagnostics,
                agent_hud::spawn_agent_hud,
                llm_badge::spawn_llm_badge,
                roster::spawn_roster_panel,
                chat_input::spawn_chat_input,
                menu::spawn_main_menu,
                quick_action::spawn_quick_action,
            ),
        );
        app.add_systems(
            Update,
            (
                stage_bar::update_stage_bar,
                chat_panel::update_chat_panel,
                diagnostics::update_diagnostics,
                agent_hud::update_agent_hud_system,
                llm_badge::refresh_badge_clock_system,
                llm_badge::update_llm_badge_system,
                roster::update_roster_panel_system,
                chat_input::update_chat_input,
                menu::update_main_menu,
                quick_action::update_quick_action,
            ),
        );
    }
}
