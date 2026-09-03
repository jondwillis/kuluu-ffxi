pub mod action_model;
pub mod auction;
pub mod bazaar_view;
#[cfg(feature = "enhanced-cast-bar")]
pub mod cast_bar;
pub mod chat_input;
pub mod chat_panel;
pub mod check_view;
pub mod compass;
pub mod death_prompt;
pub mod delivery;
pub mod diagnostics;
pub mod dialog;
pub mod digit_spinner;
pub mod entity_hover_card;
pub mod equipment_screen;
pub mod item_dat_root;
pub mod item_detail;
pub mod item_grid;
pub mod item_meta;
pub mod item_screen;
pub mod item_ui;
pub mod logout_countdown;
// Depends on `crate::minimap`, which is itself gated off wasm (lib.rs).
pub mod graphics_debug;
#[cfg(not(target_arch = "wasm32"))]
pub mod map_screen;
pub mod menu;
pub mod menu_help_bar;
pub mod mesh_debug;
pub mod network_status;
pub mod overlay;
pub mod panel_column;
pub mod quick_action;
pub mod stair_debug;
// (roster mockup deleted — real party/self HUD is party_frame.rs)
pub mod party_frame;
pub mod self_fishing;
pub mod shop;
pub mod spinner;
pub mod stage_bar;
pub mod status_panel;
pub mod status_ribbon;
pub mod style;
pub mod target_action_menu;
pub mod target_panel;
pub mod trade;
pub mod treasure_pool;
pub mod vana_clock;
pub mod weather_icon;
pub mod zone_flash;

use bevy::prelude::*;

#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct HudVerbosity {
    pub dev_hud: bool,
}

#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct HudPanels {
    pub perf: bool,
    pub target_cycle: bool,
    pub mesh_debug: bool,
    /// Debug noclip (kuluu-q5sn follow-up): bypass the client-side wall clamp
    /// in movement. Grounding stays on. Toggled from the Debug menu NoClip row
    /// or /noclip; both flip this same flag.
    pub noclip: bool,
    /// Stair-climber debug: when true, shows the status panel populated by
    /// the input crate's stair detection state (orbs, slopes, classification
    /// counts). Independent from `stair_draw` (whether the in-world gizmos
    /// render); either can be on without the other.
    pub stair_debug: bool,
    /// Draws the in-world stair-climber gizmos (ring orbs, purple march,
    /// head sphere, ramp line). Off = detection still runs (the character
    /// still climbs), just no visuals.
    pub stair_draw: bool,
    /// Graphics-debug panel (win/img/panel metrics). Runtime-only, no persist.
    pub graphics_debug: bool,
    /// Nameplate Debug panel: last-two-tick pass state from the final in-view
    /// nameplate pass (extract/bind/draw counters + far-plate clip position).
    /// Runtime-only, no persist.
    pub nameplate_debug: bool,
    /// Rolling panel-position capture to panelpositions.txt. Runtime-only,
    /// default off so the game never spams a log unasked. No persist.
    pub position_log: bool,
    /// Party-frame UI Settings panel (Debug menu): layout overrides, bar
    /// toggles, distance readouts, scale. Runtime-only, no persist.
    pub ui_settings: bool,
}

#[derive(Component)]
pub struct DevHud;

#[derive(Component)]
pub struct BottomLeftStack;

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

                bottom: Val::Px(54.0),
                left: Val::Px(0.0),
                width: Val::Percent(50.0),

                height: Val::Auto,
                flex_direction: FlexDirection::ColumnReverse,

                align_items: AlignItems::FlexStart,
                row_gap: Val::Px(4.0),
                ..default()
            },
        ))
        .with_children(|p| {
            chat_panel::spawn_chat_panels_as_children(p);
            chat_panel::spawn_chat_tab_bar_as_child(p);

            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::FlexStart,
                column_gap: Val::Px(4.0),
                ..default()
            })
            .with_children(|row| {
                row.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::FlexStart,
                    row_gap: Val::Px(4.0),
                    ..default()
                })
                .with_children(|col| {
                    #[cfg(not(target_arch = "wasm32"))]
                    crate::minimap::spawn_minimap_as_child(col, &mut images);

                    #[cfg(target_arch = "wasm32")]
                    compass::spawn_compass_as_child(col);

                    col.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(4.0),
                        ..default()
                    })
                    .with_children(|under| {
                        vana_clock::spawn_vana_clock_as_child(under);
                        weather_icon::spawn_weather_icon_as_child(under);
                    });
                });

                target_action_menu::spawn_target_action_menu_as_child(row);
            });
        });
}

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

pub fn format_timer(secs: u32) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HudVerbosity>();
        app.init_resource::<HudPanels>();
        app.init_resource::<network_status::NetStatusVisible>();
        app.init_resource::<vana_clock::VanaClockVisible>();

        app.init_resource::<menu::DynamicMenu>();

        app.init_resource::<overlay::ActiveOverlay>();
        app.init_resource::<chat_panel::ActiveChatTab>();
        app.init_resource::<chat_panel::ChatAutoSwitch>();
        app.init_resource::<chat_panel::ChatUnread>();
        app.init_resource::<chat_panel::ChatActivityTracker>();
        app.init_resource::<mesh_debug::MeshHoverDebug>();
        app.init_resource::<stair_debug::StairDebugSnapshot>();
        app.init_resource::<graphics_debug::GraphicsDebugState>();
        // Render-world probe for the surface size (see graphics_debug).
        if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
            render_app.add_systems(
                bevy::render::Render,
                graphics_debug::record_surface_size
                    .in_set(bevy::render::RenderSystems::PrepareViews),
            );
        }
        app.init_resource::<stair_debug::OrchDecisionLog>();

        app.init_resource::<zone_flash::ZoneFlashState>();

        app.init_resource::<status_ribbon::StatusIconCache>();
        app.init_resource::<status_ribbon::StatusIconDatRoot>();

        app.init_resource::<item_dat_root::ItemDatRoot>();
        app.init_resource::<item_dat_root::ItemIconCache>();
        app.init_resource::<item_detail::SortOptions>();
        app.init_resource::<item_detail::ItemMenuFocus>();
        app.init_resource::<item_screen::ItemScreenContainer>();
        #[cfg(not(target_arch = "wasm32"))]
        {
            app.init_resource::<map_screen::MapScreenDots>();
            app.init_resource::<map_screen::MapWidescanDots>();
            app.init_resource::<map_screen::MapScreenState>();
            app.init_resource::<map_screen::MapMarkers>();
            app.init_resource::<map_screen::ViewedMap>();
            app.init_resource::<map_screen::MapView>();
        }

        app.init_resource::<check_view::CheckTarget>();
        app.init_resource::<bazaar_view::BazaarScreenState>();
        app.init_resource::<status_panel::StatusProfileOpen>();
        app.init_resource::<party_frame::PartyFrameSettings>();

        app.init_resource::<trade::TradeState>();

        app.init_resource::<delivery::DeliveryScreenState>();
        app.init_resource::<delivery::DeliveryInventory>();

        app.init_resource::<auction::AuctionScreenState>();
        app.init_resource::<auction::AuctionSellInventory>();
        app.init_resource::<auction::AuctionEventCursor>();
        app.add_message::<auction::AuctionRowActivated>();

        app.add_message::<target_action_menu::TargetActionActivated>();
        app.add_message::<trade::TradeIntent>();
        app.init_resource::<target_panel::SwingPulse>();
        app.init_resource::<logout_countdown::LogoutCountdownAnchor>();
        app.init_resource::<logout_countdown::OptimisticLogoutCountdown>();
        app.add_message::<logout_countdown::LogoutRequested>();

        app.add_message::<menu::MenuRowActivated>();
        app.add_message::<item_detail::InventorySortRequested>();
        app.add_message::<dialog::DialogChoiceActivated>();
        app.add_message::<quick_action::QuickActionActivated>();

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
                chat_input::update_chat_input,
                menu::update_main_menu,
                quick_action::update_quick_action,
                target_panel::update_target_panel_system,
                (
                    diagnostics::update_fps_system,
                    diagnostics::update_draws_system,
                ),
                dialog::update_dialog_panel_system,
                dialog::update_dialog_grid_system,
                dialog::update_dialog_options_system,
                shop::update_shop_panel_system,
                (
                    compass::update_compass,
                    #[cfg(not(target_arch = "wasm32"))]
                    compass::update_compass_track_pointer,
                ),
                vana_clock::update_vana_clock,
                zone_flash::update_zone_flash,
                party_frame::update_party_frame_system,
                party_frame::update_ui_settings_system,
                self_fishing::update_fishing_hud,
                (
                    status_ribbon::update_status_ribbon,
                    status_ribbon::update_status_timers,
                    status_ribbon::update_status_ribbon_selection,
                ),
                (
                    death_prompt::update_death_prompt_system,
                    weather_icon::update_weather_icon,
                ),
            ),
        );

        // Separate call: the main HUD tuple above is already at Bevy's 20-element
        // limit. In-place distance readouts — must follow the row rebuild so
        // freshly spawned distance texts get their value same-frame.
        app.add_systems(
            Update,
            party_frame::update_party_dist_text_system
                .after(party_frame::update_party_frame_system),
        );

        app.add_systems(
            Update,
            menu::refresh_dynamic_menu_rows.before(menu::update_main_menu),
        );

        app.add_systems(
            Update,
            panel_column::layout_panel_column_system
                .after(target_panel::update_target_panel_system),
        );

        app.add_systems(
            Update,
            menu_help_bar::update_menu_help_bar.after(menu::refresh_dynamic_menu_rows),
        );

        app.add_systems(Update, logout_countdown::update_logout_countdown);
        app.add_systems(Update, treasure_pool::update_treasure_pool);

        app.add_systems(
            Update,
            (
                target_action_menu::update_target_action_menu,
                target_action_menu::target_action_mouse_hover_system,
                target_action_menu::target_action_mouse_click_system,
                item_screen::update_item_screen.after(menu::refresh_dynamic_menu_rows),
                item_screen::update_bag_tabs.after(item_screen::update_item_screen),
                trade::update_trade_window,
                check_view::update_check_view,
                bazaar_view::update_bazaar_view,
                status_panel::update_status_panel,
                equipment_screen::update_equipment_screen.after(menu::refresh_dynamic_menu_rows),
                delivery::rebuild_delivery_inventory,
                delivery::update_delivery_screen.after(delivery::rebuild_delivery_inventory),
                auction::rebuild_sell_inventory,
                auction::update_auction_screen.after(auction::rebuild_sell_inventory),
                auction::auction_chat_echo_system,
            ),
        );
        // Map screen + minimap markers depend on `crate::minimap` (wasm-gated).
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(
            Update,
            (
                map_screen::reset_map_screen_on_open,
                map_screen::load_viewed_map,
                map_screen::update_map_view,
                map_screen::update_map_screen_image,
                map_screen::update_map_screen_markers,
                map_screen::update_map_widescan_dots.after(map_screen::update_map_view),
                map_screen::update_map_placed_markers,
                map_screen::update_map_panel,
            ),
        );
        app.add_systems(Update, apply_dev_hud_visibility);
        app.add_systems(
            Update,
            (
                network_status::update_network_status,
                network_status::apply_net_status_visibility,
                vana_clock::apply_vana_clock_visibility,
            ),
        );
        #[cfg(feature = "enhanced-buff-tooltips")]
        app.add_systems(Update, status_ribbon::tooltip::update_buff_tooltip);
        #[cfg(feature = "enhanced-cast-bar")]
        app.add_systems(Update, cast_bar::update_cast_bar);

        app.add_systems(Update, chat_panel::chat_tab_click_system);
        app.add_systems(Update, chat_panel::chat_auto_switch_click_system);

        app.add_systems(
            Update,
            chat_panel::chat_auto_switch_and_unread_system
                .after(chat_panel::chat_tab_click_system)
                .before(chat_panel::update_chat_tab_visuals_system),
        );
        app.add_systems(Update, chat_panel::update_chat_tab_visuals_system);

        app.add_systems(Update, weather_icon::update_weather_icon);
        app.add_systems(Update, entity_hover_card::update_entity_hover_card_system);

        app.add_systems(
            Update,
            (
                party_frame::party_row_click_system,
                party_frame::ui_settings_click_system,
                menu::menu_mouse_hover_system,
                menu::menu_mouse_click_system,
                item_screen::item_row_mouse_hover_system,
                item_screen::item_row_mouse_click_system,
                item_screen::sort_option_mouse_system,
                item_screen::bag_tab_mouse_system,
                dialog::dialog_mouse_hover_system,
                dialog::dialog_mouse_click_system,
                quick_action::quick_action_mouse_hover_system,
                quick_action::quick_action_mouse_click_system,
                auction::auction_mouse_hover_system,
                auction::auction_mouse_click_system,
            ),
        );

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
                stair_debug::update_stair_debug_hud,
                graphics_debug::graphics_debug_metrics_system,
                graphics_debug::update_graphics_debug_hud,
                graphics_debug::update_nameplate_debug_hud,
            )
                .chain(),
        );

        if !app.is_plugin_added::<bevy::diagnostic::FrameTimeDiagnosticsPlugin>() {
            app.add_plugins(bevy::diagnostic::FrameTimeDiagnosticsPlugin::default());
        }

        diagnostics::register_visible_meshes_diagnostic(app);
        app.add_systems(PostUpdate, diagnostics::count_visible_meshes_system);
    }
}

pub fn add_hud_spawners<L: bevy::ecs::schedule::ScheduleLabel + Clone>(app: &mut App, schedule: L) {
    app.add_systems(
        schedule.clone(),
        (
            spawn_bottom_left_stack,
            diagnostics::spawn_diagnostics,
            chat_input::spawn_chat_input,
            menu::spawn_main_menu,
            menu_help_bar::spawn_menu_help_bar,
            quick_action::spawn_quick_action,
            target_panel::spawn_target_panel,
            dialog::spawn_dialog_panel,
            shop::spawn_shop_panel,
            zone_flash::spawn_zone_flash,
            self_fishing::spawn_fishing_hud,
            party_frame::spawn_party_frames,
            status_ribbon::spawn_status_ribbon,
            death_prompt::spawn_death_prompt,
            logout_countdown::spawn_logout_countdown,
            treasure_pool::spawn_treasure_pool,
            mesh_debug::spawn_mesh_debug_hud,
            entity_hover_card::spawn_entity_hover_card,
            network_status::spawn_network_status,
        ),
    );
    // Separate call: bevy's add_systems tuple bound caps at 20, and we're
    // already at 20 above.
    app.add_systems(schedule.clone(), stair_debug::spawn_stair_debug_hud);
    app.add_systems(schedule.clone(), graphics_debug::spawn_graphics_debug_hud);
    app.add_systems(schedule.clone(), graphics_debug::spawn_nameplate_debug_hud);

    #[cfg(feature = "enhanced-buff-tooltips")]
    app.add_systems(schedule.clone(), status_ribbon::tooltip::spawn_buff_tooltip);

    #[cfg(feature = "enhanced-cast-bar")]
    app.add_systems(schedule.clone(), cast_bar::spawn_cast_bar);

    app.add_systems(
        schedule.clone(),
        (
            item_screen::spawn_item_screen,
            trade::spawn_trade_window,
            check_view::spawn_check_view,
            bazaar_view::spawn_bazaar_view,
            status_panel::spawn_status_panel,
            equipment_screen::spawn_equipment_screen,
            delivery::spawn_delivery_screen,
            auction::spawn_auction_screen,
        ),
    );
    // Depends on `crate::minimap` (wasm-gated).
    #[cfg(not(target_arch = "wasm32"))]
    app.add_systems(schedule, map_screen::spawn_map_screen);
}
