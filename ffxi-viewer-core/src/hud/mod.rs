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
pub mod entity_hover_card;
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

/// Marker on the bottom-left flex container that owns the chat panels,
/// tab bar, and minimap. The container uses `FlexDirection::ColumnReverse`
/// so the first child (the chat panel pool) sits at the bottom and
/// subsequent children stack upward — when chat auto-decay shrinks the
/// panel height, the tab bar + minimap slide down with it automatically
/// (Taffy handles the math, no per-frame positioning system needed).
///
/// Native-only because the minimap child is native-only (`#[cfg]` gated
/// in `crate::minimap`). The wasm front-end registers chat panels and
/// tab bar via [`add_hud_spawners`] without the minimap.
#[derive(Component)]
pub struct BottomLeftStack;

/// Spawn the [`BottomLeftStack`] flex container + its children: the
/// three chat panels (Social / Battle / Debug), the tab bar, and (on
/// native) the minimap. Children stack via Bevy UI's Taffy flex flow
/// — no manual `position_bottom_left_stack_system` any more.
///
/// Order in the DOM (`with_children` insertion order) matters because
/// `FlexDirection::ColumnReverse` places the first child at the bottom
/// and subsequent children upward:
///
///   1. chat panels (bottom of stack — the active one is visible,
///                   inactive Display::None and skipped by flex)
///   2. tab bar     (above the active panel)
///   3. minimap     (above the tab bar) — native only
pub fn spawn_bottom_left_stack(
    mut commands: Commands,
    #[cfg(not(target_arch = "wasm32"))] mut images: ResMut<Assets<bevy::image::Image>>,
) {
    commands
        .spawn((
            crate::components::InGameEntity,
            BottomLeftStack,
            Node {
                position_type: PositionType::Absolute,
                // Sit above the chat-input strip (bottom 28..52) with
                // a 2px gap. `bottom: 54` is the same anchor the
                // freestanding chat panel used to use; the difference
                // now is that everything above it flows up via flex.
                bottom: Val::Px(54.0),
                left: Val::Px(0.0),
                width: Val::Percent(50.0),
                // `height: Auto` so the container resizes to fit its
                // children. As the chat panel auto-decays its height
                // between PANEL_MIN_HEIGHT_PX and PANEL_MAX_HEIGHT_PX,
                // the whole stack grows/shrinks while staying anchored
                // to the viewport bottom.
                height: Val::Auto,
                flex_direction: FlexDirection::ColumnReverse,
                // Left-align children of different widths (the minimap
                // is 192px square, the tab bar is auto-sized to its
                // buttons, the chat panel is 100% of the stack
                // width). Without `FlexStart`, Bevy's default
                // `Stretch` would force the minimap and tab bar to
                // full stack width.
                align_items: AlignItems::FlexStart,
                row_gap: Val::Px(4.0),
                ..default()
            },
        ))
        .with_children(|p| {
            chat_panel::spawn_chat_panels_as_children(p);
            chat_panel::spawn_chat_tab_bar_as_child(p);
            #[cfg(not(target_arch = "wasm32"))]
            crate::minimap::spawn_minimap_as_child(p, &mut images);
            // Cluster the Vana clock + weather chip immediately above
            // the minimap, replacing retail's "minimap compass" slot.
            // Column-reverse means later children stack visually
            // higher, so these sit at the top of the bottom-left
            // stack — closest to the camera viewport edge.
            vana_clock::spawn_vana_clock_as_child(p);
            weather_icon::spawn_weather_icon_as_child(p);
        });
}

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
        // Stage-2 Magic / Abilities menus pull from this resource;
        // `refresh_dynamic_menu_rows` (registered below) rebuilds it
        // every frame from the active SceneSnapshot mirrors.
        app.init_resource::<menu::DynamicMenu>();
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
        // Mouse-driven menu/dialog/quick-action activation. The consumer
        // (in `ffxi-client/src/view_native/text_input.rs`) reads these
        // alongside the keyboard `KeyboardInput` stream so both paths
        // share the same dispatch helpers.
        app.add_message::<menu::MenuRowActivated>();
        app.add_message::<dialog::DialogChoiceActivated>();
        app.add_message::<quick_action::QuickActionActivated>();
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
        // refresh_dynamic_menu_rows must run BEFORE update_main_menu
        // so the renderer sees a fresh Magic / Abilities list instead
        // of last frame's stale rows. The tuple above is at the
        // 20-system cap so this hook goes in its own add_systems
        // with an explicit .before() ordering.
        app.add_systems(
            Update,
            menu::refresh_dynamic_menu_rows.before(menu::update_main_menu),
        );
        // Second `add_systems` call: Bevy's `IntoScheduleConfigs` tuple
        // impls cap out at 20 entries, and the block above hit that
        // ceiling — adding a 21st entry trips the trait-bound error.
        app.add_systems(Update, logout_countdown::update_logout_countdown);
        app.add_systems(Update, apply_dev_hud_visibility);
        app.add_systems(Update, chat_panel::chat_tab_click_system);
        app.add_systems(Update, chat_panel::update_chat_tab_visuals_system);
        // The chat panel + tab bar + minimap auto-flow via the
        // `BottomLeftStack` flex container (Taffy-driven). No
        // per-frame positioning system needed — Bevy UI recomputes
        // the layout whenever a child's size changes (e.g. chat
        // auto-decay shrinks/grows `Node::height`).
        app.add_systems(Update, weather_icon::update_weather_icon);
        app.add_systems(Update, entity_hover_card::update_entity_hover_card_system);
        // Mouse-driven menu / dialog / quick-action systems. Each pair is
        // (hover, click); hover follows mouse to sync cursor state with
        // the visual highlight, click emits the activation message.
        app.add_systems(
            Update,
            (
                menu::menu_mouse_hover_system,
                menu::menu_mouse_click_system,
                dialog::update_dialog_choice_highlight_system,
                dialog::dialog_mouse_hover_system,
                dialog::dialog_mouse_click_system,
                quick_action::quick_action_mouse_hover_system,
                quick_action::quick_action_mouse_click_system,
            ),
        );
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
pub fn add_hud_spawners<L: bevy::ecs::schedule::ScheduleLabel + Clone>(app: &mut App, schedule: L) {
    app.add_systems(
        schedule.clone(),
        (
            stage_bar::spawn_stage_bar,
            // Bottom-left flex container owns the chat panels, tab
            // bar, and (on native) the minimap. Replaces the old
            // freestanding chat_panel::spawn_chat_panel +
            // minimap::spawn_minimap pair.
            spawn_bottom_left_stack,
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
            zone_flash::spawn_zone_flash,
            self_hud::spawn_self_hud,
            status_ribbon::spawn_status_ribbon,
            death_prompt::spawn_death_prompt,
            logout_countdown::spawn_logout_countdown,
            mesh_debug::spawn_mesh_debug_hud,
            entity_hover_card::spawn_entity_hover_card,
        ),
    );
    // Minimap, Vana clock, and weather icon all spawn as children of
    // `BottomLeftStack` (see `spawn_bottom_left_stack`); no separate
    // top-level registration is needed for them any more.
}
