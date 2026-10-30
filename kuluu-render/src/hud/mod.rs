pub mod action_model;
pub mod auction;
pub mod bazaar_comment;
pub mod bazaar_view;
#[cfg(feature = "enhanced-cast-bar")]
pub mod cast_bar;
pub mod chat_input;
pub mod chat_panel;
pub mod check_view;
/// The player's own macro and equipment-set files in the install; see this module's header for what
/// law they establish.
pub mod client_user_files;
pub mod compass;
pub mod controls_law;
pub mod currency_law;
#[cfg(feature = "enhanced-death-countdown")]
pub mod death_countdown;
pub mod death_prompt;
pub mod delivery;
pub mod diagnostics;
pub mod dialog;
pub mod digit_spinner;
pub mod dock;
pub mod entity_hover_card;
pub mod equipment_screen;
pub mod gettext;
pub mod item_dat_root;
pub mod item_detail;
pub mod item_grid;
pub mod item_meta;
pub mod item_screen;
pub mod item_ui;
pub mod list_view;
#[cfg(feature = "enhanced-shutdown-counter")]
pub mod logout_countdown;
// Depends on `crate::minimap`, which is itself gated off wasm (lib.rs).
pub mod graphics_debug;
/// One macro keytop editor: Prev/Next, its hotkey word and name, six command line slots.
pub mod macro_edit_view;
/// The macro palette's two-rows-of-ten keytop band (`mcr1pall`/`mcr2pall`).
pub mod macro_pallet_view;
#[cfg(not(target_arch = "wasm32"))]
pub mod map_screen;
pub mod menu;
pub mod menu_anim;
pub mod menu_help_bar;
pub mod merit_windows;
pub mod mesh_debug;
pub mod network_status;
pub mod overlay;
pub mod panel_column;
pub mod quick_action;
// Reads the region map DATs through `crate::minimap` (wasm-gated), like `map_screen`.
#[cfg(not(target_arch = "wasm32"))]
pub mod region_map;
// (roster mockup deleted — real party/self HUD is party_frame.rs)
pub mod party_frame;
pub mod self_fishing;
pub mod shop;
pub mod stage_bar;
pub mod stair_debug;
pub mod status_panel;
pub mod status_ribbon;
pub mod style;
pub mod target_action_menu;
pub mod target_panel;
pub mod trade;
pub mod treasure_pool;
pub mod unity_fp;
pub mod unity_fp_view;
pub mod vana_clock;
pub mod weather_icon;
pub mod zone_flash;

use bevy::picking::{events::PointerPress, pointer::PointerButton};
use bevy::prelude::*;

pub(crate) fn primary_pressed_rows<'a, T: Component>(
    presses: &mut MessageReader<PointerPress>,
    rows: &'a Query<&T>,
    parents: &Query<&ChildOf>,
) -> Vec<&'a T> {
    let mut seen = bevy::platform::collections::HashSet::<Entity>::default();
    presses
        .read()
        .filter(|press| press.button == PointerButton::Primary)
        .filter_map(|press| {
            std::iter::once(press.entity)
                .chain(parents.iter_ancestors(press.entity))
                .find_map(|entity| rows.get(entity).ok().map(|row| (entity, row)))
        })
        .filter_map(|(entity, row)| seen.insert(entity).then_some(row))
        .collect()
}

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
    /// Enhanced build only: forces the retail weapon draw/sheathe movement
    /// hold back on (the gated build lifts it by default).
    #[cfg(feature = "enhanced-engage-move-lock-off")]
    pub engage_anim_lock: bool,
    /// Debug auto-enter (enternity-style): when on, event-dialog message frames
    /// advance themselves after their read time instead of waiting for Enter.
    /// Choice frames, item lines, text-entry frames, server custom menus, and
    /// the enternity blacklist (Paintbrush of Souls, Geomantic Reservoir) are
    /// not advanced. Toggled from the Debug menu "Auto-Enter CS" row.
    pub auto_enter_cs: bool,
    /// Graphics-debug panel (win/img/panel metrics). Runtime-only, no persist.
    pub graphics_debug: bool,
    /// Stair-debug status panel: last two ticks' walker decisions, the probe
    /// columns along the heading and the wall faces blocking them.
    /// Runtime-only, no persist.
    pub stair_debug: bool,
    /// Draws the in-world stair gizmos (probe-column markers, feet/h0 bars,
    /// move arrow, blocking-contact mark). Runtime-only, no persist.
    pub stair_draw: bool,
    /// Nameplate Debug panel: last-two-tick pass state from the final in-view
    /// nameplate pass (extract/bind/draw counters + far-plate clip position).
    /// Runtime-only, no persist.
    pub nameplate_debug: bool,
    /// Rolling panel-position capture to panelpositions.txt. Runtime-only,
    /// default off so the game does not spam a log unasked. No persist.
    pub position_log: bool,
    /// Party-frame UI Settings panel (Debug menu): layout overrides, bar
    /// toggles, distance readouts, scale. Runtime-only, no persist.
    pub ui_settings: bool,
    /// Debug weather gate (Debug menu Weather row): when true, suppresses the
    /// active-weather modifier (ambient tint/brightness, sun mul, lightning)
    /// and precipitation particles so scene-graphic errors can be isolated.
    /// The zone DAT environment (sky, base ambient, distance fog) is untouched —
    /// that is what the Fog row controls. Runtime-only, no persist.
    pub weather_off: bool,
    /// Debug fog gate (Debug menu Fog row): when true, strips every fog layer
    /// (DAT DistanceFog, volumetric ground haze + its volume entity) so
    /// scene-graphic errors can be isolated. Independent of `weather_off`.
    /// Runtime-only, no persist.
    pub fog_off: bool,
    /// Debug Entity List overlay (Debug menu "Entity List" row): when true,
    /// shows a scrollable dump of every live wire entity from the
    /// EntityTable — id, name, kind, position, status byte, hp%, invis/name-
    /// hidden/dead flags. Mouse wheel scrolls; default off.
    /// Runtime-only, no persist.
    pub entity_list: bool,
}

#[derive(Component)]
pub struct DevHud;

#[derive(Component)]
pub struct BottomLeftStack;

#[derive(Component)]
pub struct ChatTools;

pub const BOTTOM_LEFT_INSET_PX: f32 = 54.0;

pub fn spawn_bottom_left_stack(
    mut commands: Commands,
    mut images: ResMut<Assets<bevy::image::Image>>,
) {
    let icon_placeholder = item_ui::transparent_placeholder(&mut images);
    commands
        .spawn((
            crate::components::InGameEntity,
            BottomLeftStack,
            Node {
                position_type: PositionType::Absolute,

                bottom: Val::Px(BOTTOM_LEFT_INSET_PX),
                left: Val::Px(0.0),
                width: Val::Percent(50.0),

                height: Val::Auto,
                flex_direction: FlexDirection::ColumnReverse,

                align_items: AlignItems::FlexStart,
                row_gap: Val::Px(4.0),
                ..default()
            },
            ZIndex(style::CHAT_STACK_Z),
        ))
        .with_children(|p| {
            chat_panel::spawn_chat_panels_as_children(p);
            chat_panel::spawn_chat_tab_bar_as_child(p);

            p.spawn((
                ChatTools,
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::FlexStart,
                    column_gap: Val::Px(4.0),
                    ..default()
                },
            ))
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

                    item_screen::spawn_item_detail_card_as_child(col, icon_placeholder);

                    col.spawn((
                        item_screen::CompassClockCluster,
                        Node {
                            flex_direction: FlexDirection::Column,
                            align_items: AlignItems::FlexStart,
                            row_gap: Val::Px(4.0),
                            ..default()
                        },
                    ))
                    .with_children(|cluster| {
                        compass::spawn_compass_as_child(cluster);

                        cluster
                            .spawn(Node {
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

                    // Last in the column, so the field stands directly above the chat log it
                    // belongs to (owner 2026-10-24).
                    bazaar_comment::spawn_bazaar_comment_editor(col);
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
        app.init_resource::<menu::MagicSchoolTable>();
        app.init_resource::<menu::ConfigDatRows>();
        // ur/139: which level stands on which pooled surface, shared between the draw pass and
        // row hit routing (a shared seat relabels in place instead of opening a second box).
        app.init_resource::<menu::PooledSeats>();
        app.init_resource::<dock::DockBoard>();
        app.init_resource::<menu::CommandSelectorCaptions>();
        app.init_resource::<menu::EquipRecords>();
        app.init_resource::<unity_fp::UnityFpDat>();
        app.init_resource::<merit_windows::MeritDat>();

        app.init_resource::<overlay::ActiveOverlay>();
        app.init_resource::<chat_panel::ActiveChatTab>();
        app.init_resource::<chat_panel::ChatAutoSwitch>();
        app.init_resource::<chat_panel::ChatUnread>();
        app.init_resource::<chat_panel::ChatActivityTracker>();
        app.init_resource::<mesh_debug::MeshHoverDebug>();
        app.init_resource::<graphics_debug::GraphicsDebugState>();
        app.init_resource::<stair_debug::StairDebugSnapshot>();
        if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
            render_app.add_systems(
                bevy::render::Render,
                graphics_debug::record_surface_size
                    .in_set(bevy::render::RenderSystems::PrepareViews),
            );
        }
        app.init_resource::<zone_flash::ZoneFlashState>();

        app.init_resource::<status_ribbon::StatusIconCache>();
        app.init_resource::<status_ribbon::StatusIconDatRoot>();
        #[cfg(not(target_arch = "wasm32"))]
        app.init_resource::<region_map::ViewedRegionMap>();

        app.init_resource::<item_dat_root::ItemDatRoot>();
        app.init_resource::<item_dat_root::ItemIconCache>();
        app.init_resource::<item_detail::SortOptions>();
        app.init_resource::<item_detail::AutoSortedInventory>();
        app.init_resource::<item_detail::ItemMenuFocus>();
        app.init_resource::<item_screen::ItemScreenContainer>();
        app.init_resource::<item_screen::ItemListViewport>();
        #[cfg(not(target_arch = "wasm32"))]
        {
            app.init_resource::<map_screen::MapScreenDots>();
            app.init_resource::<map_screen::MapWidescanDots>();
            app.init_resource::<map_screen::MapScreenState>();
            app.init_resource::<map_screen::MapMarkers>();
            app.init_resource::<map_screen::ViewedMap>();
            app.init_resource::<map_screen::MapView>();
            app.init_resource::<map_screen::ChangeMapCatalog>();
        }

        app.init_resource::<check_view::CheckTarget>();
        app.init_resource::<bazaar_view::BazaarScreenState>();
        app.init_resource::<party_frame::PartyFrameSettings>();

        app.init_resource::<trade::TradeState>();

        app.init_resource::<delivery::DeliveryScreenState>();
        app.init_resource::<delivery::DeliveryInventory>();

        app.add_systems(
            PostUpdate,
            item_grid::sync_count_outlines.before(bevy::ui::UiSystems::Content),
        );

        app.init_resource::<shop::ShopScreenState>();
        app.add_message::<shop::ShopRowActivated>();
        app.init_resource::<auction::AuctionScreenState>();
        app.init_resource::<auction::AuctionSellInventory>();
        app.init_resource::<auction::AuctionEventCursor>();
        app.add_message::<auction::AuctionRowActivated>();

        app.add_message::<target_action_menu::TargetActionActivated>();
        app.add_message::<trade::TradeIntent>();
        app.init_resource::<target_panel::SwingPulse>();
        #[cfg(feature = "enhanced-shutdown-counter")]
        app.init_resource::<logout_countdown::LogoutCountdownAnchor>();
        #[cfg(feature = "enhanced-shutdown-counter")]
        app.init_resource::<logout_countdown::PendingLogoutRequest>();
        app.init_resource::<death_prompt::DeathPromptSelection>();
        #[cfg(feature = "enhanced-shutdown-counter")]
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
                (
                    shop::record_shop_appraisals,
                    shop::update_shop_panel_system,
                    shop::update_shop_scrollbar,
                    shop::shop_mouse_hover_system,
                    shop::shop_mouse_click_system,
                    shop::shop_wheel_scroll_system,
                )
                    .chain(),
                (
                    compass::update_compass,
                    compass::update_compass_art,
                    compass::update_compass_dots,
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
                    #[cfg(feature = "enhanced-buff-timers")]
                    status_ribbon::timers::update_status_timers,
                    status_ribbon::update_status_ribbon_selection,
                ),
                (
                    death_prompt::update_death_prompt_system,
                    weather_icon::update_weather_icon,
                ),
            ),
        );

        app.add_systems(
            Update,
            party_frame::update_party_dist_text_system
                .after(party_frame::update_party_frame_system),
        );

        // A window input opened owns its rows in the same frame: every stack mutator (keyboard,
        // mouse nav, arrows, cancel) runs inside the chain that ends at `dock_windows_system`, and
        // the row rebuild sits between that boundary and every draw pass. No drawn frame can then
        // see a new kind with unbuilt rows - holding the previous occupant's words onto a fresh
        // box for one frame was the right-side flash on any menu kind change.
        app.add_systems(
            Update,
            menu::refresh_dynamic_menu_rows
                .after(crate::hud::dock::dock_windows_system)
                .before(menu::update_main_menu),
        );
        // The window title bands: they need the word and the pixel width `update_main_menu` picks
        // for this frame, so they place immediately after it. They also write text faces, so they go
        // behind the face-stamping system to keep one writer per node in a frame.
        app.add_systems(
            Update,
            menu::place_menu_title_bands
                .after(menu::update_main_menu)
                .after(crate::ui_font::apply_ui_font),
        );
        app.add_systems(
            Update,
            (
                menu::settle_opened_windows
                    .after(menu::refresh_dynamic_menu_rows)
                    .before(menu::update_main_menu)
                    .before(menu::update_menu_stack_windows)
                    .before(unity_fp_view::update_unity_fp_windows),
                menu::persist_window_cursors,
            ),
        );
        app.add_systems(
            Update,
            (
                menu::update_menu_stack_windows,
                status_panel::update_status_level_box,
                status_panel::update_persona_frame,
                unity_fp_view::update_unity_fp_windows,
                merit_windows::merit_window_upkeep,
            )
                .after(menu::refresh_dynamic_menu_rows),
        );
        // The selector-column relabel squeeze: last writer of the pane's box, standing after
        // `update_main_menu` places it natural with words filled underneath.
        app.add_systems(
            Update,
            menu::squeeze_menu_column.after(menu::update_main_menu),
        );

        // The end-of-frame forensic sample (owner debug 2026-10-19): runs last among the
        // menu systems and names every visible UI text node while a Status flow is open,
        // so any stray drawer identifies itself in the log. The dedicated statcom2 copy
        // it once shadowed was removed same day (owner: "that menu should not exist here").
        app.add_systems(
            Update,
            (menu::ui_window_forensics.after(menu::update_menu_stack_windows),),
        );

        app.add_systems(
            Update,
            panel_column::layout_panel_column_system
                .after(target_panel::update_target_panel_system),
        );
        app.add_systems(
            Update,
            (
                dock::dock_windows_system
                    .after(target_panel::update_target_panel_system)
                    .after(party_frame::update_party_frame_system)
                    .before(menu::update_main_menu)
                    .before(menu::update_menu_stack_windows)
                    .before(unity_fp_view::update_unity_fp_windows),
                dock::place_target_window
                    .after(dock::dock_windows_system)
                    .before(panel_column::layout_panel_column_system),
            ),
        );

        // The bar's fade-in rider (owner law 2026-10-22): runs on the same frame as its fills so
        // the appearing frame already renders chrome-only at the first alpha step.
        app.add_systems(
            Update,
            (
                menu_help_bar::update_menu_help_bar,
                menu_help_bar::fade_menu_help_bar_in,
            )
                .chain()
                .after(menu::refresh_dynamic_menu_rows),
        );

        #[cfg(feature = "enhanced-shutdown-counter")]
        app.add_systems(Update, logout_countdown::update_logout_countdown);
        app.add_systems(Update, treasure_pool::update_treasure_pool);

        app.add_systems(
            Update,
            (
                target_action_menu::update_target_action_menu,
                target_action_menu::target_action_mouse_hover_system,
                target_action_menu::target_action_mouse_click_system,
                item_screen::update_item_screen_layout,
                menu::update_status_detail_window
                    .after(menu::refresh_dynamic_menu_rows)
                    .before(menu::update_main_menu),
                item_screen::update_item_screen.after(menu::refresh_dynamic_menu_rows),
                item_screen::update_item_scrollbar.after(item_screen::update_item_screen),
                item_screen::update_bag_tabs.after(item_screen::update_item_screen),
                trade::update_trade_window,
                check_view::update_check_view,
                bazaar_view::update_bazaar_view,
                bazaar_view::update_bazaar_scrollbar.after(bazaar_view::update_bazaar_view),
                // The Status column's squeeze legs (menu_anim law): this system writes persona
                // and `level` box geometry last, after the passes that place them.
                status_panel::update_status_panel
                    .after(status_panel::update_status_level_box)
                    .after(status_panel::update_persona_frame),
                equipment_screen::update_equipment_screen.after(menu::refresh_dynamic_menu_rows),
                delivery::rebuild_delivery_inventory,
                delivery::update_delivery_screen.after(delivery::rebuild_delivery_inventory),
                auction::rebuild_sell_inventory,
                auction::update_auction_screen.after(auction::rebuild_sell_inventory),
                auction::auction_chat_echo_system,
            ),
        );
        // Its own registration: the pass reads only `InputMode` and `DynamicMenu`, so it owes
        // nothing to the group above and that group has no room left in it.
        app.add_systems(Update, bazaar_comment::update_bazaar_comment_editor);
        // The palette band fills from the rows `refresh_dynamic_menu_rows` just built (their labels are
        // the display names of the book's keytops), so it runs behind that pass and never earlier.
        app.add_systems(
            Update,
            macro_pallet_view::update_macro_pallet_view
                .after(menu::refresh_dynamic_menu_rows)
                .after(bazaar_comment::update_bazaar_comment_editor),
        );
        // Same law for the keytop editor: its fields fill from the stack, and it draws over the band's
        // corner, which that band has just hidden.
        app.add_systems(
            Update,
            macro_edit_view::update_macro_edit_view
                .after(menu::refresh_dynamic_menu_rows)
                .after(macro_pallet_view::update_macro_pallet_view),
        );
        // Map screen + minimap markers depend on `crate::minimap` (wasm-gated).
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(
            Update,
            (
                map_screen::reset_map_screen_on_open,
                map_screen::refresh_change_map_catalog,
                map_screen::load_viewed_map,
                map_screen::update_map_view,
                map_screen::update_map_screen_image,
                map_screen::update_map_screen_markers,
                map_screen::update_map_widescan_dots.after(map_screen::update_map_view),
                map_screen::update_map_placed_markers,
                map_screen::update_map_panel.after(dock::dock_windows_system),
                region_map::update_region_map,
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
        #[cfg(feature = "enhanced-death-countdown")]
        app.add_systems(Update, death_countdown::update_death_countdown_system);

        app.add_systems(Update, chat_panel::chat_tab_click_system);
        app.add_systems(Update, chat_panel::chat_auto_switch_click_system);

        app.add_systems(
            Update,
            chat_panel::chat_auto_switch_and_unread_system
                .after(chat_panel::chat_tab_click_system)
                .before(chat_panel::update_chat_tab_visuals_system),
        );
        app.add_systems(Update, chat_panel::update_chat_tab_visuals_system);
        app.add_systems(
            Update,
            chat_panel::apply_chat_layout
                .after(panel_column::layout_panel_column_system)
                .before(chat_panel::update_chat_panel),
        );

        app.add_systems(Update, weather_icon::update_weather_icon);
        app.add_systems(Update, entity_hover_card::update_entity_hover_card_system);

        app.add_systems(
            Update,
            (
                party_frame::party_row_click_system,
                party_frame::ui_settings_click_system,
                // ur/139: routes clicks through the seat table, so it reads it current.
                menu::menu_mouse_hover_system.after(menu::update_menu_stack_windows),
                menu::menu_mouse_click_system.after(menu::update_menu_stack_windows),
                item_screen::item_row_mouse_hover_system,
                item_screen::item_row_mouse_click_system,
                item_screen::sort_option_mouse_system,
                (
                    item_detail::sync_inventory_sort_options_system,
                    item_detail::mirror_inventory_sort_options_system,
                )
                    .chain(),
                item_detail::auto_consolidate_inventory_system,
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
                graphics_debug::graphics_debug_metrics_system,
                graphics_debug::update_graphics_debug_hud,
                graphics_debug::update_nameplate_debug_hud,
                stair_debug::update_stair_debug_hud,
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

/// Bevy's add_systems tuple bound caps at 20 elements, and the main spawner
/// tuple above sits at that limit, so the remaining spawners go in separate
/// calls.
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
            zone_flash::spawn_zone_flash,
            self_fishing::spawn_fishing_hud,
            party_frame::spawn_party_frames,
            status_ribbon::spawn_status_ribbon,
            death_prompt::spawn_death_prompt,
            #[cfg(feature = "enhanced-shutdown-counter")]
            logout_countdown::spawn_logout_countdown,
            treasure_pool::spawn_treasure_pool,
            mesh_debug::spawn_mesh_debug_hud,
            entity_hover_card::spawn_entity_hover_card,
            network_status::spawn_network_status,
        ),
    );
    app.add_systems(schedule.clone(), graphics_debug::spawn_graphics_debug_hud);
    app.add_systems(schedule.clone(), graphics_debug::spawn_nameplate_debug_hud);
    app.add_systems(schedule.clone(), stair_debug::spawn_stair_debug_hud);

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
            macro_pallet_view::spawn_macro_pallet_view,
            macro_edit_view::spawn_macro_edit_view,
            status_panel::spawn_status_panel,
            menu::spawn_status_detail_window,
            equipment_screen::spawn_equipment_screen,
            delivery::spawn_delivery_screen,
            auction::spawn_auction_screen,
            shop::spawn_shop_panel,
            unity_fp_view::spawn_unity_fp_windows,
        ),
    );
    // Depends on `crate::minimap` (wasm-gated).
    #[cfg(not(target_arch = "wasm32"))]
    app.add_systems(
        schedule,
        (map_screen::spawn_map_screen, region_map::spawn_region_map),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn bottom_left_stack_spawns_one_compass_with_four_cardinals() {
        let mut world = World::new();
        #[cfg(not(target_arch = "wasm32"))]
        world.init_resource::<Assets<bevy::image::Image>>();
        world.run_system_once(spawn_bottom_left_stack).unwrap();

        assert_eq!(
            world.query::<&compass::CompassPanel>().iter(&world).count(),
            1
        );
        assert_eq!(
            world.query::<&compass::CompassLabel>().iter(&world).count(),
            4
        );
    }

    /// Bevy stacks UI roots by (GlobalZIndex, ZIndex) and leaves ties to archetype order, so only
    /// the chat stack's own lower layer keeps a menu that reaches the log drawn over it.
    #[test]
    fn menus_draw_over_the_chat_stack() {
        fn layer<F: bevy::ecs::query::QueryFilter>(world: &mut World) -> (i32, i32) {
            let (global, local) = world
                .query_filtered::<(Option<&GlobalZIndex>, Option<&ZIndex>), F>()
                .single(world)
                .expect("one root");
            (
                global.copied().unwrap_or_default().0,
                local.copied().unwrap_or_default().0,
            )
        }
        let mut world = World::new();
        #[cfg(not(target_arch = "wasm32"))]
        world.init_resource::<Assets<bevy::image::Image>>();
        world.run_system_once(spawn_bottom_left_stack).unwrap();
        world.run_system_once(menu::spawn_main_menu).unwrap();
        assert!(
            layer::<With<BottomLeftStack>>(&mut world) < layer::<With<menu::MainMenu>>(&mut world)
        );
    }
}
#[cfg(all(test, not(target_arch = "wasm32")))]
mod quantity_capture;
