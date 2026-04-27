pub mod action_model;
pub mod chat_input;
pub mod chat_panel;
pub mod check_view;
pub mod compass;
pub mod death_prompt;
pub mod diagnostics;
pub mod dialog;
pub mod entity_hover_card;
pub mod item_dat_root;
pub mod item_detail;
pub mod item_meta;
pub mod logout_countdown;
pub mod menu;
pub mod mesh_debug;
pub mod overlay;
pub mod quick_action;
pub mod roster;
pub mod self_hud;
pub mod shop;
pub mod stage_bar;
pub mod status_panel;
pub mod status_ribbon;
pub mod target_action_menu;
pub mod target_panel;
pub mod trade;
pub mod vana_clock;
pub mod weather_icon;
pub mod zone_flash;

use bevy::prelude::*;

#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct HudVerbosity {
    pub dev_hud: bool,
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
            target_action_menu::spawn_target_action_menu_as_child(p);

            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::FlexStart,
                column_gap: Val::Px(4.0),
                ..default()
            })
            .with_children(|row| {
                #[cfg(not(target_arch = "wasm32"))]
                crate::minimap::spawn_minimap_as_child(row, &mut images);

                row.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(4.0),
                    ..default()
                })
                .with_children(|col| {
                    compass::spawn_compass_as_child(col);
                    vana_clock::spawn_vana_clock_as_child(col);
                    weather_icon::spawn_weather_icon_as_child(col);
                });
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

pub mod palette {
    use bevy::prelude::Color;

    pub const BORDER: Color = Color::srgb(0.40, 0.40, 0.40);
    pub const BACKGROUND: Color = Color::srgb(0.04, 0.04, 0.04);
    pub const ACCENT: Color = Color::srgb(0.0, 1.0, 1.0);
    pub const TEXT: Color = Color::srgb(0.95, 0.95, 0.95);
    pub const MUTED: Color = Color::srgb(0.55, 0.55, 0.55);
    pub const DARK: Color = Color::srgb(0.40, 0.40, 0.40);

    pub const STAGE_IDLE: Color = DARK;
    pub const STAGE_TRANSITIONING: Color = Color::srgb(1.0, 0.85, 0.0);
    pub const STAGE_GOOD: Color = Color::srgb(0.0, 0.85, 0.0);
    pub const STAGE_BAD: Color = Color::srgb(0.95, 0.20, 0.20);
}

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HudVerbosity>();

        app.init_resource::<menu::DynamicMenu>();

        app.init_resource::<overlay::ActiveOverlay>();
        app.init_resource::<chat_panel::ActiveChatTab>();
        app.init_resource::<chat_panel::ChatAutoSwitch>();
        app.init_resource::<chat_panel::ChatUnread>();
        app.init_resource::<chat_panel::ChatActivityTracker>();
        app.init_resource::<mesh_debug::MeshHoverDebug>();

        app.init_resource::<zone_flash::ZoneFlashState>();
        app.init_resource::<self_hud::SelfHealTracker>();

        app.init_resource::<status_ribbon::StatusIconCache>();
        app.init_resource::<status_ribbon::StatusIconDatRoot>();

        app.init_resource::<item_dat_root::ItemDatRoot>();
        app.init_resource::<item_dat_root::ItemIconCache>();
        app.init_resource::<item_detail::SortOptions>();

        app.init_resource::<check_view::CheckTarget>();
        app.init_resource::<status_panel::StatusProfileOpen>();

        app.init_resource::<trade::TradeState>();

        app.add_message::<target_action_menu::TargetActionActivated>();
        app.add_message::<trade::TradeIntent>();
        app.init_resource::<target_panel::SwingPulse>();
        app.init_resource::<logout_countdown::LogoutCountdownAnchor>();
        app.init_resource::<logout_countdown::OptimisticLogoutCountdown>();
        app.add_message::<logout_countdown::LogoutRequested>();

        app.add_message::<menu::MenuRowActivated>();
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
                roster::update_roster_panel_system,
                chat_input::update_chat_input,
                menu::update_main_menu,
                quick_action::update_quick_action,
                target_panel::update_target_panel_system,
                (
                    diagnostics::update_fps_system,
                    diagnostics::update_draws_system,
                ),
                dialog::update_dialog_panel_system,
                dialog::update_dialog_options_system,
                shop::update_shop_panel_system,
                compass::update_compass,
                vana_clock::update_vana_clock,
                zone_flash::update_zone_flash,
                (
                    self_hud::update_self_hud,
                    self_hud::update_self_status,
                    self_hud::update_self_party_indicator,
                ),
                (
                    status_ribbon::update_status_ribbon,
                    status_ribbon::update_status_timers,
                ),
                (
                    death_prompt::update_death_prompt_system,
                    weather_icon::update_weather_icon,
                ),
            ),
        );

        app.add_systems(
            Update,
            menu::refresh_dynamic_menu_rows.before(menu::update_main_menu),
        );

        app.add_systems(Update, logout_countdown::update_logout_countdown);

        app.add_systems(
            Update,
            (
                target_action_menu::update_target_action_menu,
                target_action_menu::target_action_mouse_hover_system,
                target_action_menu::target_action_mouse_click_system,
                item_detail::update_item_detail,
                item_detail::update_sort_options,
                trade::update_trade_window,
                check_view::update_check_view,
                status_panel::update_status_panel,
            ),
        );
        app.add_systems(Update, apply_dev_hud_visibility);
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
                menu::menu_mouse_hover_system,
                menu::menu_mouse_click_system,
                dialog::dialog_mouse_hover_system,
                dialog::dialog_mouse_click_system,
                quick_action::quick_action_mouse_hover_system,
                quick_action::quick_action_mouse_click_system,
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
            stage_bar::spawn_stage_bar,
            spawn_bottom_left_stack,
            diagnostics::spawn_diagnostics,
            roster::spawn_roster_panel,
            chat_input::spawn_chat_input,
            menu::spawn_main_menu,
            quick_action::spawn_quick_action,
            target_panel::spawn_target_panel,
            dialog::spawn_dialog_panel,
            shop::spawn_shop_panel,
            zone_flash::spawn_zone_flash,
            self_hud::spawn_self_hud,
            status_ribbon::spawn_status_ribbon,
            death_prompt::spawn_death_prompt,
            logout_countdown::spawn_logout_countdown,
            mesh_debug::spawn_mesh_debug_hud,
            entity_hover_card::spawn_entity_hover_card,
        ),
    );

    app.add_systems(
        schedule,
        (
            item_detail::spawn_item_detail,
            trade::spawn_trade_window,
            check_view::spawn_check_view,
            status_panel::spawn_status_panel,
        ),
    );
}
