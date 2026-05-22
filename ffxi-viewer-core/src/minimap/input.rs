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
    MINIMAP_UI_SIZE_PX, RECENTER_IDLE_FRAMES, RECENTER_LERP_FRAMES, ZOOM_STEP_FACTOR,
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

/// Drag-state tracker for click-and-drag panning. Separate from
/// `MousePointer::left` because we only count drags that *began*
/// inside the minimap — if the cursor swept in mid-drag from another
/// widget, we don't want it to pan retroactively.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct MinimapDrag {
    pub active: bool,
}

/// Handle click-and-drag panning of the minimap viewport.
///
/// Drag-pan is gated on `MinimapZoom::radius_yalms.is_some()` — at
/// fit-zone there's nothing to pan to. Pixel delta is converted to
/// world XZ via `yalms_per_pixel = 2r / MINIMAP_UI_SIZE_PX`, then
/// *subtracted* from `pan_offset_xz` so dragging right scrolls the
/// map right under the cursor (standard "grab and drag" UX).
///
/// Sets `idle_frames = 0` on every drag frame; the recenter system
/// counts back up and lerps to zero once idle.
pub fn handle_minimap_drag_input(
    pointer: Res<MousePointer>,
    hover_gate: Res<MinimapHoverGate>,
    zoom: Res<MinimapZoom>,
    mut drag: ResMut<MinimapDrag>,
    mut view: ResMut<MinimapView>,
) {
    // Begin drag only when both the press AND the cursor hit the
    // minimap. A drag that started elsewhere shouldn't get hijacked
    // mid-stroke if the cursor sweeps over the minimap.
    if pointer.left && hover_gate.hovered && !drag.active {
        drag.active = true;
    }
    // Release drag on button up regardless of cursor position — a
    // common pattern is to drag past the widget edge and release.
    if !pointer.left {
        drag.active = false;
        return;
    }
    if !drag.active {
        return;
    }
    let Some(radius) = zoom.radius_yalms else {
        // Fit-to-zone: pan is meaningless (the whole zone is visible).
        return;
    };
    if pointer.delta == Vec2::ZERO {
        return;
    }
    let yalms_per_pixel = (2.0 * radius) / MINIMAP_UI_SIZE_PX;
    // Subtract: dragging the cursor right pulls the world right under
    // it, so the visible window's center moves left → pan_offset.x
    // decreases.
    view.pan_offset_xz -= pointer.delta * yalms_per_pixel;
    view.idle_frames = 0;
}

/// Idle-counter + auto-recenter. When the user hasn't dragged for
/// [`RECENTER_IDLE_FRAMES`] frames, lerp the pan offset back toward
/// zero over [`RECENTER_LERP_FRAMES`] frames so the view re-locks on
/// the player smoothly rather than snapping.
///
/// Does nothing while a drag is active or when zoom is fit-to-zone.
pub fn recenter_minimap_view(
    drag: Res<MinimapDrag>,
    zoom: Res<MinimapZoom>,
    mut view: ResMut<MinimapView>,
) {
    if drag.active || zoom.radius_yalms.is_none() {
        return;
    }
    // Saturating add so we don't have to think about wraparound on
    // long idle stretches.
    view.idle_frames = view.idle_frames.saturating_add(1);
    if view.idle_frames < RECENTER_IDLE_FRAMES {
        return;
    }
    if view.pan_offset_xz == Vec2::ZERO {
        return;
    }
    // Critically-damped lerp: each frame, move (1/REMAINING_FRAMES)
    // of the remaining distance. After RECENTER_LERP_FRAMES the
    // residual is negligible; snap to zero to stop the residual
    // jitter that comes from accumulating tiny f32 multiplications.
    let t = 1.0 / RECENTER_LERP_FRAMES as f32;
    view.pan_offset_xz *= 1.0 - t;
    if view.pan_offset_xz.length_squared() < 0.01 {
        view.pan_offset_xz = Vec2::ZERO;
    }
}
