#![cfg(test)]

use bevy::asset::AssetPlugin;
use bevy::image::ImagePlugin;
use bevy::input::InputPlugin;
use bevy::picking::{InteractionPlugin, PickingPlugin};
use bevy::prelude::*;
use bevy::text::TextPlugin;
use bevy::ui::UiPlugin;

/// A headless `App` with the real `HudPlugin` wired up exactly like
/// production (`add_hud_spawners` + `HudPlugin`), so tests exercise actual
/// spawn/update systems rather than a re-implemented subset. No window, no
/// GPU — `bevy_ui` layout and `bevy_text` shaping both run purely on CPU.
pub(crate) fn headless_hud_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        ImagePlugin::default(),
        InputPlugin,
        TextPlugin,
        // UiPlugin's viewport_picking system reads the HoverMap resource,
        // owned by bevy_picking's InteractionPlugin — needed even though
        // nothing in a headless test actually hovers anything. Skips
        // DefaultPickingPlugins' PointerInputPlugin, which needs a real
        // WindowPlugin-registered WindowEvent message this harness has none of.
        PickingPlugin,
        InteractionPlugin,
        UiPlugin,
    ));
    // CursorMoved is normally registered by bevy_window's WindowPlugin, which
    // this harness deliberately omits (no real window) — mouse::MousePlugin's
    // collect_mouse_system still expects it to exist, even unused.
    app.add_message::<bevy::window::CursorMoved>();
    app.add_message::<crate::snapshot::ToastEvent>();
    // UiPlugin's ImageNode content-sizing system expects this bevy_sprite
    // asset type, normally registered by SpritePlugin (part of DefaultPlugins).
    app.init_asset::<bevy::image::TextureAtlasLayout>();

    // Resources HudPlugin's ~30 panel systems need but don't own themselves —
    // normally supplied by ViewerCorePlugin in the real app. HudPlugin
    // registers every panel into one Update schedule, so even though this
    // harness only asserts on menu/target_action_menu, every other panel's
    // systems still run each app.update() and need their resources present
    // or the whole schedule panics (Bevy resource params are all-or-nothing
    // per run, not just for the system under test).
    app.init_resource::<crate::input_mode::InputMode>();
    app.init_resource::<crate::snapshot::SceneState>();
    app.init_resource::<crate::vana_time::VanaClock>();
    app.init_resource::<crate::EventLog>();
    app.init_resource::<crate::scene::TrackedEntities>();
    app.init_resource::<crate::scene::Target>();
    app.init_resource::<crate::keybinds::Bindings>();
    app.init_resource::<crate::camera::CameraMode>();
    app.init_resource::<crate::lock_on::LockOn>();
    app.init_resource::<crate::zone_lines::ZoneLineState>();
    app.init_resource::<crate::graphics_settings::GraphicsSettings>();
    app.init_resource::<crate::hud::chat_panel::ChatScroll>();
    app.init_resource::<crate::hud::chat_panel::BattleScroll>();
    app.init_resource::<crate::hud::chat_panel::DebugScroll>();
    app.init_resource::<crate::hud::chat_panel::ChatScrollAccum>();
    app.init_resource::<crate::hud::chat_panel::BattleScrollAccum>();
    app.init_resource::<crate::hud::chat_panel::DebugScrollAccum>();
    app.init_resource::<crate::ui_font::UiFont>();
    app.add_systems(Startup, crate::ui_font::load_ui_font);

    app.add_plugins(crate::MousePlugin);
    // entity_hover_card.rs reads these two (crate::picking::PickingPlugin's
    // own resources) but crate::PickingPlugin itself pulls in
    // bevy::picking::mesh_picking::MeshPickingPlugin, which needs
    // Assets<Mesh> from bevy_render's MeshPlugin — real 3D-world click
    // targeting this headless UI harness has no use for. Supply just the
    // two Default resources directly instead of the whole plugin.
    app.init_resource::<crate::picking::HoveredEntity>();
    app.init_resource::<crate::picking::PickBridgePointer>();
    app.add_plugins(crate::InputMethodPlugin);
    app.add_plugins(crate::hud::HudPlugin);
    crate::add_hud_spawners(&mut app, Startup);

    app.update();
    app
}

/// Set `InputMode` and run enough `Update` passes for it to propagate
/// through the HUD's update systems (two covers dynamic-menu-row refresh ->
/// render, which are ordered but split across two systems in one schedule).
pub(crate) fn set_mode_and_settle(app: &mut App, mode: crate::input_mode::InputMode) {
    *app.world_mut()
        .resource_mut::<crate::input_mode::InputMode>() = mode;
    app.update();
    app.update();
}
