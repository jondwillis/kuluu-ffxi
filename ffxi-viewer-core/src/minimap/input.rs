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

#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct MinimapHoverGate {
    pub hovered: bool,
}

pub fn update_minimap_hover_gate(
    q: Query<&RelativeCursorPosition, With<MinimapRoot>>,
    mut gate: ResMut<MinimapHoverGate>,
) {
    let hovered = q.single().map(|r| r.cursor_over()).unwrap_or(false);
    if gate.hovered != hovered {
        gate.hovered = hovered;
    }
}

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
    let mut wheel_delta = 0.0;
    for ev in wheel.read() {
        wheel_delta += ev.y;
    }

    if !hover_gate.hovered {
        return;
    }

    let half_span = zone_half_span(state.active_aabb(*mode));

    if wheel_delta > 0.0 {
        zoom.zoom_by(1.0 / ZOOM_STEP_FACTOR, half_span);
        view.idle_frames = 0;
    } else if wheel_delta < 0.0 {
        zoom.zoom_by(ZOOM_STEP_FACTOR, half_span);
        view.idle_frames = 0;
    }

    if wheel_delta != 0.0 {
        pointer.wheel = 0.0;
    }

    if bindings.just_pressed(Action::CameraZoomIn, &keys) {
        zoom.zoom_by(1.0 / ZOOM_STEP_FACTOR, half_span);
        view.idle_frames = 0;
    }
    if bindings.just_pressed(Action::CameraZoomOut, &keys) {
        zoom.zoom_by(ZOOM_STEP_FACTOR, half_span);
        view.idle_frames = 0;
    }
}

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct MinimapDrag {
    pub active: bool,
}

pub fn handle_minimap_drag_input(
    mut pointer: ResMut<MousePointer>,
    hover_gate: Res<MinimapHoverGate>,
    zoom: Res<MinimapZoom>,
    mut drag: ResMut<MinimapDrag>,
    mut view: ResMut<MinimapView>,
) {
    if pointer.left && hover_gate.hovered && !drag.active {
        drag.active = true;
    }

    if !pointer.left {
        drag.active = false;
        return;
    }
    if !drag.active {
        return;
    }

    let delta = pointer.delta;
    pointer.delta = Vec2::ZERO;
    pointer.left_dragged = false;

    let Some(radius) = zoom.radius_yalms else {
        return;
    };
    if delta == Vec2::ZERO {
        return;
    }
    let yalms_per_pixel = (2.0 * radius) / MINIMAP_UI_SIZE_PX;

    view.pan_offset_xz -= delta * yalms_per_pixel;
    view.idle_frames = 0;
}

pub fn recenter_minimap_view(
    drag: Res<MinimapDrag>,
    zoom: Res<MinimapZoom>,
    mut view: ResMut<MinimapView>,
) {
    if drag.active || zoom.radius_yalms.is_none() {
        return;
    }

    view.idle_frames = view.idle_frames.saturating_add(1);
    if view.idle_frames < RECENTER_IDLE_FRAMES {
        return;
    }
    if view.pan_offset_xz == Vec2::ZERO {
        return;
    }

    let t = 1.0 / RECENTER_LERP_FRAMES as f32;
    view.pan_offset_xz *= 1.0 - t;
    if view.pan_offset_xz.length_squared() < 0.01 {
        view.pan_offset_xz = Vec2::ZERO;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{CameraMode, ChaseCamera};
    use crate::snapshot::SceneState;
    use bevy::input::mouse::MouseScrollUnit;
    use bevy::input::ButtonInput;

    use super::super::{MinimapState, MinimapView, MinimapZoom, ZOOM_DEFAULT_RADIUS};

    fn zoom_test_app(hovered: bool) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseWheel>()
            .init_resource::<MinimapState>()
            .init_resource::<MinimapMode>()
            .init_resource::<MinimapView>()
            .init_resource::<MinimapZoom>()
            .init_resource::<Bindings>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<SceneState>()
            .init_resource::<CameraMode>()
            .init_resource::<ChaseCamera>()
            .insert_resource(MinimapHoverGate { hovered })
            .insert_resource(MousePointer {
                wheel: 5.0,
                ..Default::default()
            });
        app
    }

    fn write_scroll_up(app: &mut App) {
        app.world_mut().write_message(MouseWheel {
            phase: bevy::input::touch::TouchPhase::Moved,
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: 5.0,
            window: Entity::PLACEHOLDER,
        });
    }

    #[test]
    fn hovered_scroll_zooms_minimap_and_consumes_wheel() {
        let mut app = zoom_test_app(true);
        app.add_systems(Update, handle_minimap_zoom_input);
        write_scroll_up(&mut app);
        app.update();

        assert_eq!(
            app.world().resource::<MinimapZoom>().radius_yalms,
            Some(ZOOM_DEFAULT_RADIUS / ZOOM_STEP_FACTOR),
            "scroll-up over the minimap should zoom it in"
        );
        assert_eq!(
            app.world().resource::<MousePointer>().wheel,
            0.0,
            "consuming the wheel must zero MousePointer::wheel so the \
             camera doesn't also zoom"
        );
    }

    #[test]
    fn unhovered_scroll_leaves_wheel_for_camera() {
        let mut app = zoom_test_app(false);
        app.add_systems(Update, handle_minimap_zoom_input);
        write_scroll_up(&mut app);
        app.update();

        assert_eq!(
            app.world().resource::<MinimapZoom>().radius_yalms,
            Some(ZOOM_DEFAULT_RADIUS),
            "minimap zoom must not change when the cursor is elsewhere"
        );
        assert_eq!(
            app.world().resource::<MousePointer>().wheel,
            5.0,
            "an un-hovered wheel must reach the camera untouched"
        );
    }

    #[test]
    fn hovered_scroll_does_not_move_camera_distance() {
        let mut app = zoom_test_app(true);
        app.insert_resource(CameraMode::Chase);
        let initial = app.world().resource::<ChaseCamera>().distance;
        app.add_systems(
            Update,
            (handle_minimap_zoom_input, crate::mouse::mouse_camera_system).chain(),
        );
        write_scroll_up(&mut app);
        app.update();

        assert_eq!(
            app.world().resource::<ChaseCamera>().distance,
            initial,
            "scrolling over the minimap must not zoom the chase camera"
        );
        assert_eq!(
            app.world().resource::<MinimapZoom>().radius_yalms,
            Some(ZOOM_DEFAULT_RADIUS / ZOOM_STEP_FACTOR),
            "the same scroll should have zoomed the minimap instead"
        );
    }
}
