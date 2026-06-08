//! Browser front-end for the operator viewer. Wraps `ffxi-viewer-core`'s
//! `ViewerCorePlugin` with a `WasmSource` that pulls frames off a WebSocket
//! relay (Stage 2 in `ffxi-client/src/relay.rs`).
//!
//! Build harness: trunk. `index.html` references this crate as a `data-trunk
//! rel="rust"` target; trunk runs `cargo build --target wasm32-unknown-unknown`,
//! `wasm-bindgen` post-processing, and serves the resulting JS+wasm alongside
//! the HTML shell.

#![forbid(unsafe_code)]
// See ffxi-viewer-core: Bevy ECS makes these lints noise.
#![allow(
    clippy::type_complexity,
    clippy::too_many_arguments,
    clippy::doc_lazy_continuation,
    clippy::doc_overindented_list_items
)]

mod source;

use bevy::prelude::*;
use ffxi_viewer_core::{
    add_hud_spawners, setup_world, spawn_camera, HudPlugin, MousePlugin, ViewerCorePlugin,
};
use wasm_bindgen::prelude::*;
use web_sys::UrlSearchParams;

use crate::source::WasmSource;

/// Default relay address used when no `?ws=` query param is supplied.
const DEFAULT_WS_URL: &str = "ws://localhost:7000";

/// Pull `?ws=...` out of `window.location.search`, falling back to the
/// localhost default. Defensive against missing window / malformed URL —
/// the worst case is "use the default", never a panic.
fn resolve_ws_url() -> String {
    let Some(window) = web_sys::window() else {
        return DEFAULT_WS_URL.to_string();
    };
    let Ok(search) = window.location().search() else {
        return DEFAULT_WS_URL.to_string();
    };
    // `.search()` returns "?foo=bar" including the leading '?', or "" when
    // empty. `UrlSearchParams::new_with_str` accepts either form.
    let trimmed = search.trim_start_matches('?');
    let Ok(params) = UrlSearchParams::new_with_str(trimmed) else {
        return DEFAULT_WS_URL.to_string();
    };
    params
        .get("ws")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_WS_URL.to_string())
}

/// Entry point. `#[wasm_bindgen(start)]` makes the wasm module call this
/// automatically on load — trunk's generated JS runs `init()` then triggers
/// the start function, so we don't need to expose anything else.
#[wasm_bindgen(start)]
pub fn run() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);

    let ws_url = resolve_ws_url();
    log::info!("ffxi-viewer-wasm: starting, ws_url={ws_url}");

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            // Trunk targets the `<canvas id="bevy">` in index.html.
            canvas: Some("#bevy".to_string()),
            fit_canvas_to_parent: true,
            prevent_default_event_handling: false,
            title: "ffxi-viewer".to_string(),
            ..default()
        }),
        ..default()
    }))
    .insert_resource(WasmSource::connect(&ws_url))
    // ViewerCorePlugin no longer registers world/camera/HUD itself
    // (the native client must defer those until a session exists).
    // Wasm wants the old behavior — register them on Startup explicitly.
    .add_systems(Startup, (setup_world, spawn_camera))
    .add_plugins((
        ViewerCorePlugin::<WasmSource>::default(),
        HudPlugin,
        MousePlugin,
    ));
    add_hud_spawners(&mut app, Startup);
    app.run();
}
