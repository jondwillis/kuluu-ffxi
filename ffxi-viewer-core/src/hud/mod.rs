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
pub mod logout_countdown;
pub mod menu;
pub mod mesh_debug;
pub mod quick_action;
pub mod roster;
pub mod self_hud;
pub mod shop;
pub mod stage_bar;
pub mod status_ribbon;
pub mod target_panel;
pub mod vana_clock;
pub mod weather_icon;
pub mod zone_flash;

use bevy::prelude::*;

/// Whether the dev-only HUD widgets (stage bar, agent goal panel, MMB
/// hover info, LLM badge, diagnostics strip, third `ChatKind::Debug`
/// chat pane) are visible. Default `false` — those panels are operator
/// telemetry that doesn't exist in vanilla FFXI / Ashita / Windower, so
/// the production view hides them. `/devhud on|off|toggle` flips this
/// at runtime; [`apply_dev_hud_visibility`] reacts on change and walks
/// every entity tagged [`DevHud`] to swap `Visibility::Hidden` and
/// `Visibility::Inherited`.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct HudVerbosity {
    pub dev_hud: bool,
}

/// Marker on every dev-only HUD root entity. [`apply_dev_hud_visibility`]
/// toggles `Visibility` on these in lockstep with [`HudVerbosity`].
#[derive(Component)]
pub struct DevHud;

/// Run when [`HudVerbosity`] flips: set every [`DevHud`]-tagged entity's
/// `Visibility` to match. Bevy UI propagates `Hidden` to descendants, so
/// only the root nodes need the marker.
///
/// `mesh_debug` has its own hover-driven visibility update; when
/// `dev_hud == false` we force it `Hidden` here and the hover system
/// short-circuits (see `mesh_debug::update_mesh_debug_hud`).
pub fn apply_dev_hud_visibility(
    verbosity: Res<HudVerbosity>,
    mut q: Query<&mut Visibility, With<DevHud>>,
) {
    if !verbosity.is_changed() {
        return;
    }
    let want = if verbosity.dev_hud {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut v in q.iter_mut() {
        if *v != want {
            *v = want;
        }
    }
}

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
        app.init_resource::<HudVerbosity>();
        app.init_resource::<chat_panel::ActiveChatTab>();
        app.init_resource::<llm_badge::BadgeClock>();
        app.init_resource::<mesh_debug::MeshHoverDebug>();
        // ZoneFlashState must exist before `update_zone_flash` runs the
        // first time. Registering at plugin build (not in the spawner)
        // sidesteps the `Commands` queue lag — without this, the first
        // Update tick after `OnEnter(InGame)` panics on missing resource.
        app.init_resource::<zone_flash::ZoneFlashState>();
        app.init_resource::<self_hud::SelfHealTracker>();
        app.init_resource::<target_panel::SwingPulse>();
        app.init_resource::<logout_countdown::LogoutCountdownAnchor>();
        app.init_resource::<logout_countdown::OptimisticLogoutCountdown>();
        app.add_message::<logout_countdown::LogoutRequested>();
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
                (
                    death_prompt::update_death_prompt_system,
                    weather_icon::update_weather_icon,
                ),
            ),
        );
        // Second `add_systems` call: Bevy's `IntoScheduleConfigs` tuple
        // impls cap out at 20 entries, and the block above hit that
        // ceiling — adding a 21st entry trips the trait-bound error.
        app.add_systems(Update, logout_countdown::update_logout_countdown);
        app.add_systems(Update, apply_dev_hud_visibility);
        app.add_systems(Update, chat_panel::chat_tab_click_system);
        app.add_systems(Update, chat_panel::update_chat_tab_visuals_system);
        // Runs AFTER update_chat_panel (which mutates panel height
        // each frame via auto-decay) so the tab bar + minimap see the
        // current frame's chat height, not last frame's. Bevy's
        // default Update ordering is parallel-when-possible; an
        // explicit `.after()` pins the dependency.
        app.add_systems(
            Update,
            chat_panel::position_bottom_left_stack_system
                .after(chat_panel::update_chat_panel),
        );
        app.add_systems(Update, weather_icon::update_weather_icon);
        // Combat pulse: detect-then-modulate, chained so the color
        // update sees the latched timestamp from the same frame. Both
        // must run every tick (no `is_changed` gate) — the flash decay
        // animates between server snapshots.
        app.add_systems(
            Update,
            (
                target_panel::detect_swing_pulse_system,
                target_panel::pulse_engaged_badge_color_system,
            )
                .chain(),
        );
        app.add_systems(
            Update,
            (
                mesh_debug::update_hover_state,
                mesh_debug::update_mesh_debug_hud,
            )
                .chain(),
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
pub fn add_hud_spawners<L: bevy::ecs::schedule::ScheduleLabel + Clone>(
    app: &mut App,
    schedule: L,
) {
    app.add_systems(
        schedule.clone(),
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
            logout_countdown::spawn_logout_countdown,
            mesh_debug::spawn_mesh_debug_hud,
        ),
    );
    // Second `add_systems` call — see the matching split in `Update`
    // registration above: Bevy's `IntoScheduleConfigs` tuple impls cap
    // at 20 entries.
    app.add_systems(schedule.clone(), weather_icon::spawn_weather_icon);
    // Minimap spawner. Native-only — the minimap module itself is
    // gated on `cfg(not(target_arch = "wasm32"))` because its
    // top-down backend reads MZB geometry. WASM front-ends skip this
    // entry without losing the rest of the HUD.
    #[cfg(not(target_arch = "wasm32"))]
    app.add_systems(schedule, crate::minimap::spawn_minimap);
}
