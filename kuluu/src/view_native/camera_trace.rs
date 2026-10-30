use bevy::prelude::*;
use kuluu_render::camera::{CameraMode, CameraTransition, OperatorCamera};
use kuluu_render::components::IsSelf;
use kuluu_render::cutscene::CutsceneMode;
use kuluu_render::cutscene_camera::CutsceneCameraTasks;

pub(super) fn enabled() -> bool {
    tracing::enabled!(target: "camera_trace", tracing::Level::TRACE)
}

pub(super) fn report(
    time: Res<Time>,
    cameras: Query<
        (&Transform, &GlobalTransform, &Projection, Option<&ChildOf>),
        With<OperatorCamera>,
    >,
    players: Query<&GlobalTransform, With<IsSelf>>,
    mode: Res<CameraMode>,
    transition: Res<CameraTransition>,
    cutscene: Res<CutsceneMode>,
    tasks: Res<CutsceneCameraTasks>,
) {
    for (local, world, projection, parent) in &cameras {
        let Projection::Perspective(perspective) = projection else {
            continue;
        };
        tracing::trace!(
            target: "camera_trace",
            elapsed_seconds = time.elapsed_secs_f64(),
            local_eye = ?local.translation,
            local_rotation = ?local.rotation,
            world_eye = ?world.translation(),
            world_forward = ?world.forward(),
            world_rotation = ?world.rotation(),
            parent = ?parent,
            mode = ?*mode,
            transition_active = transition.active,
            transition_t = transition.t,
            transition_target = ?transition.target_mode,
            cutscene_active = cutscene.active,
            camera_locked = cutscene.camera_locked,
            route_active = tasks.is_active(),
            applied_frame = ?tasks.held(),
            near = perspective.near,
            far = perspective.far,
            fov = perspective.fov,
            player_positions = ?players.iter().map(GlobalTransform::translation).collect::<Vec<_>>(),
            "final camera state"
        );
    }
}
