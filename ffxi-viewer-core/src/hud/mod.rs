//! Heads-up display: thin Bevy UI tree carrying the visual language the
//! Bevy native viewer uses for session state. (Historical: the layout was
//! first prototyped in `ffxi-client/src/chrome.rs` for the now-removed
//! ratatui TUI subcommand; this is the canonical implementation now.)
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
pub mod compass;
pub mod death_prompt;
pub mod diagnostics;
pub mod dialog;
pub mod llm_badge;
pub mod menu;
pub mod quick_action;
pub mod roster;
pub mod self_hud;
pub mod shop;
pub mod stage_bar;
pub mod status_ribbon;
pub mod target_panel;
pub mod vana_clock;
pub mod zone_flash;

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

/// HUD aggregator plugin. Registers `Update` systems and `BadgeClock`.
/// The HUD's spawn-once UI nodes are NOT registered here; front-ends call
/// [`add_hud_spawners`] with the schedule of their choice (`Startup` for
/// single-state apps, `OnEnter(your_in_game_state)` for state-driven apps
/// that defer HUD construction until a session is live).
///
/// This split exists because winit-0.30 forbids creating a second
/// `EventLoop` per process, so the native client must run a single Bevy
/// `App` that overlays a launcher UI before the HUD; that front-end can't
/// have HUD nodes spawned at `Startup`.
pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<llm_badge::BadgeClock>();
        // ZoneFlashState must exist before `update_zone_flash` runs the
        // first time. Registering at plugin build (not in the spawner)
        // sidesteps the `Commands` queue lag — without this, the first
        // Update tick after `OnEnter(InGame)` panics on missing resource.
        app.init_resource::<zone_flash::ZoneFlashState>();
        app.init_resource::<self_hud::SelfHealTracker>();
        // Mouse-wheel scroll for the chat panel. Runs in `PreUpdate`
        // after `collect_mouse_system` so it can zero `MousePointer.wheel`
        // on consume — that's how camera-zoom is kept from double-firing
        // on the same physical wheel notch.
        app.add_systems(
            bevy::app::PreUpdate,
            chat_panel::chat_wheel_scroll_system.after(crate::mouse::collect_mouse_system),
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
                target_panel::update_target_panel_system,
                diagnostics::update_fps_system,
                dialog::update_dialog_panel_system,
                shop::update_shop_panel_system,
                compass::update_compass,
                vana_clock::update_vana_clock,
                zone_flash::update_zone_flash,
                (self_hud::update_self_hud, self_hud::update_self_status),
                status_ribbon::update_status_ribbon,
                death_prompt::update_death_prompt_system,
            ),
        );

        // FrameTimeDiagnosticsPlugin powers the `fps=` field in the
        // diagnostics strip. `add_plugins` is not idempotent, so guard
        // against double-registration if the front-end app already
        // installed it.
        if !app.is_plugin_added::<bevy::diagnostic::FrameTimeDiagnosticsPlugin>() {
            app.add_plugins(bevy::diagnostic::FrameTimeDiagnosticsPlugin::default());
        }
    }
}

/// Register the HUD's spawn-once systems on `schedule`. Pass `Startup`
/// for the wasm front-end (HUD lives for the whole app), or
/// `OnEnter(your_in_game_state)` for state-driven front-ends.
pub fn add_hud_spawners<L: bevy::ecs::schedule::ScheduleLabel>(app: &mut App, schedule: L) {
    app.add_systems(
        schedule,
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
            target_panel::spawn_target_panel,
            dialog::spawn_dialog_panel,
            shop::spawn_shop_panel,
            compass::spawn_compass,
            vana_clock::spawn_vana_clock,
            zone_flash::spawn_zone_flash,
            self_hud::spawn_self_hud,
            status_ribbon::spawn_status_ribbon,
            death_prompt::spawn_death_prompt,
        ),
    );
}
