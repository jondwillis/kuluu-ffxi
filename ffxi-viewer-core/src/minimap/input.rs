//! Minimap input handlers — zoom (scroll wheel + `.`/`,` hotkeys)
//! and drag-pan.
//!
//! Both gated on cursor-hover via [`RelativeCursorPosition`] (the
//! Bevy 0.17 idiom the chat panel also uses at
//! `hud::chat_panel::chat_wheel_scroll_system`). When the cursor is
//! outside the minimap, this module is a no-op.
//!
//! # Coordination with camera zoom
//!
//! `.` (`Action::CameraZoomIn`) and `,` (`Action::CameraZoomOut`)
//! also drive the chase-camera distance in
//! `ffxi-client/src/view_native/input.rs`. When the cursor is over
//! the minimap, we want those keys to zoom the *minimap*, not the
//! camera. The coordination is one-directional:
//!
//!   * [`update_minimap_hover_gate`] sets [`MinimapHoverGate::hovered`]
//!     every frame from the cursor position.
//!   * The client's camera-zoom handler reads `MinimapHoverGate` and
//!     short-circuits when hovered.
//!
//! Scroll-wheel coordination is even simpler — we set
//! `MousePointer::wheel = 0.0` after consuming the event, same
//! pattern the chat panel uses.

#![cfg(not(target_arch = "wasm32"))]

use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;

use crate::keybinds::{Action, Bindings};
use crate::mouse::MousePointer;

use super::{
    zone_half_span, MinimapMode, MinimapRoot, MinimapState, MinimapView, MinimapZoom,
    ZOOM_STEP_FACTOR,
};

/// Cross-system flag: true iff the cursor is currently over the
/// minimap UI box. Set every frame by [`update_minimap_hover_gate`].
///
/// Read by the client's camera-zoom handler to suppress chase-camera
/// distance changes when the operator is trying to zoom the
/// *minimap* via the same `.` / `,` bindings.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct MinimapHoverGate {
    pub hovered: bool,
}

/// Refresh [`MinimapHoverGate`] from the minimap's
/// [`RelativeCursorPosition`]. Runs early in `Update` so downstream
/// systems (both this module's zoom handler and the client's camera
/// handler) see a fresh value.
pub fn update_minimap_hover_gate(
    q: Query<&RelativeCursorPosition, With<MinimapRoot>>,
    mut gate: ResMut<MinimapHoverGate>,
) {
    let hovered = q.single().map(|r| r.cursor_over()).unwrap_or(false);
    if gate.hovered != hovered {
        gate.hovered = hovered;
    }
}

/// Handle minimap zoom input — scroll wheel and `.`/`,` hotkeys.
///
/// Gated on [`MinimapHoverGate::hovered`] so a quick mouse-over
/// switches zoom control from camera to minimap without any explicit
/// mode-switch UI.
///
/// Scroll up / `.` → zoom in (smaller radius).
/// Scroll down / `,` → zoom out (larger radius, eventually fit-zone).
pub fn handle_minimap_zoom_input(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<Bindings>,
    state: Res<MinimapState>,
    mode: Res<MinimapMode>,
    hover_gate: Res<MinimapHoverGate>,
    mut wheel: MessageReader<MouseWheel>,
    mut pointer: ResMut<MousePointer>,
    mut zoom: ResMut<MinimapZoom>,
    mut view: ResMut<MinimapView>,
) {
    // Always drain wheel events so they don't leak into the next frame
    // when hover state transitions.
    let mut wheel_delta = 0.0;
    for ev in wheel.read() {
        wheel_delta += ev.y;
    }

    if !hover_gate.hovered {
        return;
    }

    let half_span = zone_half_span(state.active_aabb(*mode));

    if wheel_delta > 0.0 {
        // Scroll-up = zoom in (smaller world window).
        zoom.zoom_by(1.0 / ZOOM_STEP_FACTOR, half_span);
        view.idle_frames = 0;
    } else if wheel_delta < 0.0 {
        zoom.zoom_by(ZOOM_STEP_FACTOR, half_span);
        view.idle_frames = 0;
    }
    // Suppress camera wheel-zoom on the same physical event — same
    // pattern as `chat_wheel_scroll_system`.
    if wheel_delta != 0.0 {
        pointer.wheel = 0.0;
    }

    // Hotkey overload. `just_pressed` (not `pressed`) so the minimap
    // zoom moves one discrete tick per keypress — matches the wheel
    // semantics. Holding the key for continuous zoom would race
    // against the camera handler (which uses `pressed` for held-key
    // smoothness) and the steps would be too fine-grained anyway.
    if bindings.just_pressed(Action::CameraZoomIn, &keys) {
        zoom.zoom_by(1.0 / ZOOM_STEP_FACTOR, half_span);
        view.idle_frames = 0;
    }
    if bindings.just_pressed(Action::CameraZoomOut, &keys) {
        zoom.zoom_by(ZOOM_STEP_FACTOR, half_span);
        view.idle_frames = 0;
    }
}
