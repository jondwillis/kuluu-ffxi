//! Mouse cursor shape: swap the OS *system* cursor based on what's under the
//! pointer. No custom image assets, no UI overlay sprite — the compositor
//! draws the cursor at zero lag.
//!
//! Three states, priority `Rotate > Hand > Arrow`:
//! - `Arrow` (`SystemCursorIcon::Default`): resting, over empty world / chrome.
//! - `Hand` (`SystemCursorIcon::Pointer`): over a selectable target — a
//!   `WorldEntity` capsule, a `Button` UI node, etc.
//! - `Rotate`: while drag-rotating the camera (button held *and* dragged past
//!   the motion threshold — a bare click never triggers it).
//!
//! ## How the cursor is applied
//!
//! [`apply_cursor_icon_system`] writes the resolved [`CursorStyle`] to the
//! window as a [`CursorIcon::System`] (mapping via [`system_cursor_icon`]).
//!
//! On native there's a wrinkle: `bevy_feathers`' `CursorIconPlugin` (pulled in
//! by `FeathersPlugins`, which the launcher needs for its widgets) forces the
//! window cursor back to its `DefaultCursor` every frame whenever no feathers
//! widget is hovered — overwriting ours one frame after we set it. It leaves
//! `CursorOptions.visible` alone, which is why hiding the cursor always worked
//! while the icon reverted. The native front-end feeds our resolved shape into
//! feathers' `DefaultCursor` (`view_native`'s `drive_feathers_cursor`, via
//! [`system_cursor_icon`]) so feathers applies *our* cursor instead of
//! reverting. On web (no feathers) `apply_cursor_icon_system` is the sole
//! writer.
//!
//! `Rotate` is the exception: a camera drag locks the pointer
//! (`CursorGrabMode::Locked`) and hides it ([`apply_cursor_lock_system`],
//! native-only). Locking is what lets the camera orbit without bound — an
//! unlocked pointer would hit the screen edge and stall — and the raw
//! `MouseMotion` it still delivers drives [`crate::mouse::mouse_camera_system`].
//! Hiding matches retail FFXI (no cursor while orbiting) and avoids a frozen
//! cursor sitting mid-screen; the pointer reappears where the drag began on
//! release.

use bevy::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use bevy::window::{CursorGrabMode, CursorOptions};
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};

#[cfg(not(target_arch = "wasm32"))]
use crate::mouse::CursorLockRequest;
use crate::mouse::MousePointer;
use crate::picking::HoveredEntity;

/// Per-frame cursor look. Set by writers (entity-hover, UI-hover, camera-drag)
/// and consumed by the cursor applier. Priority is resolved in
/// [`resolve_cursor_style_system`]: highest-priority requester wins.
///
/// `Default` is `Arrow` — the resting state with nothing under the cursor.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CursorStyle {
    #[default]
    Arrow,
    Hand,
    Rotate,
}

/// The OS system cursor for each [`CursorStyle`]. Single source of truth shared
/// by [`apply_cursor_icon_system`] (the direct window writer, sole writer on
/// web) and the native front-end's `drive_feathers_cursor` (which feeds the
/// same value into feathers so it stops reverting our cursor).
///
/// `Rotate` maps to `Grabbing` for the brief moments it's visible — during an
/// actual drag the native lock hides the pointer entirely.
pub fn system_cursor_icon(style: CursorStyle) -> SystemCursorIcon {
    match style {
        CursorStyle::Arrow => SystemCursorIcon::Default,
        CursorStyle::Hand => SystemCursorIcon::Pointer,
        CursorStyle::Rotate => SystemCursorIcon::Grabbing,
    }
}

/// Per-frame requests from each writer system. The resolver picks the
/// highest-priority `true` and writes the final [`CursorStyle`]. `Rotate`
/// outranks `Hand` outranks `Arrow` (the resting default when no writer
/// requests anything).
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct CursorRequests {
    pub rotate: bool,
    pub hand: bool,
}

pub struct CursorPlugin;

impl Plugin for CursorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CursorStyle>()
            .init_resource::<CursorRequests>()
            .add_systems(
                Update,
                (
                    rotate_writer_system,
                    entity_hover_writer_system,
                    ui_hover_writer_system,
                    resolve_cursor_style_system,
                    apply_cursor_icon_system,
                    // Native only: while drag-rotating, lock + hide the OS
                    // pointer (infinite orbit, no drift, retail-faithful).
                    #[cfg(not(target_arch = "wasm32"))]
                    apply_cursor_lock_system,
                )
                    .chain(),
            );
    }
}

/// Camera-drag writer: request `Rotate` while either mouse button is held
/// *and* has dragged past the motion threshold (retail accepts LMB or RMB for
/// camera rotate; both Chase and FirstPerson use the same gate).
///
/// Gating on the `*_dragged` flags — not the bare button — keeps a click
/// (press + release, no motion) from briefly locking/hiding the pointer. The
/// flags persist through the drag (and survive a mid-drag pause) until the
/// button is next pressed, so the lock holds.
fn rotate_writer_system(pointer: Res<MousePointer>, mut req: ResMut<CursorRequests>) {
    req.rotate =
        (pointer.left && pointer.left_dragged) || (pointer.right && pointer.right_dragged);
}

/// Entity-hover writer: when an in-world entity is under the cursor, request
/// `Hand`. Reads [`HoveredEntity`] (updated by
/// `crate::picking::update_hovered_entity_system`).
fn entity_hover_writer_system(hovered: Res<HoveredEntity>, mut req: ResMut<CursorRequests>) {
    if hovered.id.is_some() {
        req.hand = true;
    }
}

/// UI-hover writer: when any UI node with an `Interaction` component is
/// currently `Hovered` or `Pressed`, request `Hand`. Bevy UI's `Interaction`
/// automatically tracks pointer state for any node that has the component, so
/// adding `Button` (or just `Interaction`) to a menu row is enough to get a
/// Hand cursor over it for free.
fn ui_hover_writer_system(interactions: Query<&Interaction>, mut req: ResMut<CursorRequests>) {
    for i in &interactions {
        if matches!(i, Interaction::Hovered | Interaction::Pressed) {
            req.hand = true;
            return;
        }
    }
}

/// Collapse per-frame requests into the final [`CursorStyle`] by priority, then
/// clear the request flags ready for the next frame's writers.
fn resolve_cursor_style_system(mut style: ResMut<CursorStyle>, mut req: ResMut<CursorRequests>) {
    let want = if req.rotate {
        CursorStyle::Rotate
    } else if req.hand {
        CursorStyle::Hand
    } else {
        CursorStyle::Arrow
    };
    if *style != want {
        *style = want;
    }
    req.rotate = false;
    req.hand = false;
}

/// Write the resolved [`CursorStyle`] to the window as an OS system cursor,
/// only when it changes. On web this is the sole cursor writer; on native it
/// also applies immediately on a style change — a frame ahead of feathers
/// re-asserting the same value via its `DefaultCursor` (see the module docs).
fn apply_cursor_icon_system(
    style: Res<CursorStyle>,
    window_q: Query<Entity, With<PrimaryWindow>>,
    mut commands: Commands,
) {
    if !style.is_changed() {
        return;
    }
    let Ok(window) = window_q.single() else {
        return;
    };
    commands
        .entity(window)
        .insert(CursorIcon::System(system_cursor_icon(*style)));
}

/// Native-only `Rotate` handling: lock + hide the OS pointer for the duration
/// of a camera drag.
///
/// Locking (`CursorGrabMode::Locked`) decouples the pointer from physical
/// motion, so the camera can orbit without bound (an unlocked pointer would
/// stall at the screen edge) while raw `MouseMotion` still drives
/// [`crate::mouse::mouse_camera_system`]. Hiding matches retail FFXI and avoids
/// a frozen cursor mid-screen; the pointer reappears where the drag began on
/// release.
///
/// Sole owner of the primary window's [`CursorOptions`] (grab mode +
/// visibility). The free-look lock request ([`CursorLockRequest`], F8) is
/// unioned into the grab decision so it keeps working, but only a drag hides
/// the cursor — free-look kept it visible.
///
/// The lock is gated on focus: a blur (Cmd-Tab) can swallow the button-release
/// event, leaving a phantom held button. Collapsing `rotating` on focus loss
/// keeps that from stranding the pointer locked + hidden.
#[cfg(not(target_arch = "wasm32"))]
fn apply_cursor_lock_system(
    style: Res<CursorStyle>,
    lock_request: Res<CursorLockRequest>,
    win_q: Query<&Window, With<PrimaryWindow>>,
    mut opts_q: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    let focused = win_q.single().map(|w| w.focused).unwrap_or(false);
    let rotating = matches!(*style, CursorStyle::Rotate) && focused;

    let Ok(mut opts) = opts_q.single_mut() else {
        return;
    };
    let want_grab = if lock_request.locked || rotating {
        CursorGrabMode::Locked
    } else {
        CursorGrabMode::None
    };
    if opts.grab_mode != want_grab {
        opts.grab_mode = want_grab;
    }
    let want_visible = !rotating;
    if opts.visible != want_visible {
        opts.visible = want_visible;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_cursor_icon_maps_each_style() {
        assert_eq!(system_cursor_icon(CursorStyle::Arrow), SystemCursorIcon::Default);
        assert_eq!(system_cursor_icon(CursorStyle::Hand), SystemCursorIcon::Pointer);
        assert_eq!(
            system_cursor_icon(CursorStyle::Rotate),
            SystemCursorIcon::Grabbing
        );
    }

    #[test]
    fn priority_rotate_beats_hand_beats_arrow() {
        let mut req = CursorRequests {
            rotate: true,
            hand: true,
        };
        let want = if req.rotate {
            CursorStyle::Rotate
        } else if req.hand {
            CursorStyle::Hand
        } else {
            CursorStyle::Arrow
        };
        assert_eq!(want, CursorStyle::Rotate);
        req.rotate = false;
        let want = if req.rotate {
            CursorStyle::Rotate
        } else if req.hand {
            CursorStyle::Hand
        } else {
            CursorStyle::Arrow
        };
        assert_eq!(want, CursorStyle::Hand);
        req.hand = false;
        let want = if req.rotate {
            CursorStyle::Rotate
        } else if req.hand {
            CursorStyle::Hand
        } else {
            CursorStyle::Arrow
        };
        assert_eq!(want, CursorStyle::Arrow);
    }
}
