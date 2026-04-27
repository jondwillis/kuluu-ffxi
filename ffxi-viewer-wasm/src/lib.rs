#![forbid(unsafe_code)]
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

mod source;

use bevy::prelude::*;
use ffxi_viewer_core::{
    add_hud_spawners, setup_world, spawn_camera, HudPlugin, MousePlugin, ViewerCorePlugin,
};
use wasm_bindgen::prelude::*;
use web_sys::UrlSearchParams;

use crate::source::WasmSource;

const DEFAULT_WS_URL: &str = "ws://localhost:7000";

fn resolve_ws_url() -> String {
    let Some(window) = web_sys::window() else {
        return DEFAULT_WS_URL.to_string();
    };
    let Ok(search) = window.location().search() else {
        return DEFAULT_WS_URL.to_string();
    };

    let trimmed = search.trim_start_matches('?');
    let Ok(params) = UrlSearchParams::new_with_str(trimmed) else {
        return DEFAULT_WS_URL.to_string();
    };
    params
        .get("ws")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_WS_URL.to_string())
}

#[wasm_bindgen(start)]
pub fn run() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);

    let ws_url = resolve_ws_url();
    log::info!("ffxi-viewer-wasm: starting, ws_url={ws_url}");

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            canvas: Some("#bevy".to_string()),
            fit_canvas_to_parent: true,
            prevent_default_event_handling: false,
            title: "ffxi-viewer".to_string(),
            ..default()
        }),
        ..default()
    }))
    .insert_resource(WasmSource::connect(&ws_url))
    .add_systems(Startup, (setup_world, spawn_camera))
    .add_plugins((
        ViewerCorePlugin::<WasmSource>::default(),
        HudPlugin,
        MousePlugin,
    ));
    add_hud_spawners(&mut app, Startup);
    app.run();
}
