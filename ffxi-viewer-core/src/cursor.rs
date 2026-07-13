use bevy::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use bevy::window::{CursorGrabMode, CursorOptions};
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};

#[cfg(not(target_arch = "wasm32"))]
use crate::input_method::InputMethod;
#[cfg(not(target_arch = "wasm32"))]
use crate::mouse::CursorLockRequest;
use crate::mouse::MousePointer;
use crate::picking::HoveredEntity;

#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CursorStyle {
    #[default]
    Arrow,
    Hand,
    Rotate,
}

pub fn system_cursor_icon(style: CursorStyle) -> SystemCursorIcon {
    match style {
        CursorStyle::Arrow => SystemCursorIcon::Default,
        CursorStyle::Hand => SystemCursorIcon::Pointer,
        CursorStyle::Rotate => SystemCursorIcon::Grabbing,
    }
}

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
                    #[cfg(not(target_arch = "wasm32"))]
                    apply_cursor_lock_system,
                )
                    .chain(),
            );
    }
}

fn rotate_writer_system(pointer: Res<MousePointer>, mut req: ResMut<CursorRequests>) {
    req.rotate = (pointer.left && pointer.left_dragged) || (pointer.right && pointer.right_dragged);
}

fn entity_hover_writer_system(hovered: Res<HoveredEntity>, mut req: ResMut<CursorRequests>) {
    if hovered.id.is_some() {
        req.hand = true;
    }
}

fn ui_hover_writer_system(interactions: Query<&Interaction>, mut req: ResMut<CursorRequests>) {
    for i in &interactions {
        if matches!(i, Interaction::Hovered | Interaction::Pressed) {
            req.hand = true;
            return;
        }
    }
}

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

#[cfg(not(target_arch = "wasm32"))]
fn apply_cursor_lock_system(
    style: Res<CursorStyle>,
    lock_request: Res<CursorLockRequest>,
    method: Res<InputMethod>,
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
    let want_visible = !rotating && !matches!(*method, InputMethod::Gamepad);
    if opts.visible != want_visible {
        opts.visible = want_visible;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_cursor_icon_maps_each_style() {
        assert_eq!(
            system_cursor_icon(CursorStyle::Arrow),
            SystemCursorIcon::Default
        );
        assert_eq!(
            system_cursor_icon(CursorStyle::Hand),
            SystemCursorIcon::Pointer
        );
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
