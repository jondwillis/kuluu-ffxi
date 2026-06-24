use std::collections::VecDeque;
use std::time::{Duration, Instant};

use bevy::ecs::system::SystemParam;
use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::window::WindowCloseRequested;

#[derive(SystemParam)]
pub struct CameraInputParams<'w> {
    pub mode: ResMut<'w, CameraMode>,
    pub chase: ResMut<'w, ChaseCamera>,
    pub cursor_lock: ResMut<'w, CursorLockRequest>,
    pub lock_on: ResMut<'w, LockOn>,
    pub transition: ResMut<'w, CameraTransition>,
}
use ffxi_viewer_core::{
    heading_for_yaw, yaw_for_heading, Action, Bindings, CameraMode, CameraTransition, ChaseCamera,
    ChatBuffer, CursorLockRequest, InputMode, IsSelf, LockOn, LockOnToggle, MenuStack,
    OperatorCamera, PassiveCursorState, SceneState, Target, WorldEntity,
};
use ffxi_viewer_wire::{Entity as WireEntity, EntityKind, Vec3 as WireVec3};
use tokio::sync::mpsc;

use crate::state::{ActionKind, AgentCommand};

pub const HEADING_TURN_RATE: f32 = 0.86;

const CAMERA_YAW_RATE: f32 = HEADING_TURN_RATE * 4.0;

const PITCH_STEP_HELD: f32 = 0.015;

const STRAFE_CANCEL_MS: u64 = 300;

const SPEED_TO_YPS: f32 = 0.1;

const BACKPEDAL_SCALE: f32 = 0.5;
const STRAFE_SCALE: f32 = 0.75;

const HEADING_LERP_RATE_RAD_PER_SEC: f32 = 5.0;

const CHASE_TRACK_RATE_RAD_PER_SEC: f32 = 5.0;

const PREDICTION_RESYNC_YALMS: f32 = 5.0;

#[derive(Resource, Clone)]
pub struct CommandTx(pub mpsc::Sender<AgentCommand>);

#[derive(Resource, Default)]
pub struct AutoRun {
    pub phantom_forward: bool,
    pub strafe_held_since: Option<Instant>,
}

#[derive(Resource, Default)]
pub struct HeadingTurnAccum {
    pub units: f32,
}

pub fn advance_heading_turn(accum_units: &mut f32, dir: i32, dt_secs: f32) -> (i32, f32) {
    let turn_units_per_sec = HEADING_TURN_RATE * 256.0 / std::f32::consts::TAU;
    let float_delta = dir as f32 * turn_units_per_sec * dt_secs;
    if dir == 0 {
        *accum_units = 0.0;
        return (0, 0.0);
    }
    *accum_units += float_delta;
    let whole = accum_units.trunc();
    *accum_units -= whole;
    (whole as i32, float_delta)
}

#[derive(Resource, Default)]
pub struct LocalPlayerPrediction {
    pub pos: Vec3,
    pub initialized: bool,
}

#[derive(Resource, Default)]
pub struct SelectTargetMode {
    pub active: bool,
    pub prev: Option<u32>,
}

pub fn handle_input_system(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<Bindings>,
    mut window_close: MessageReader<WindowCloseRequested>,
    mut state: ResMut<SceneState>,
    cmd_tx: Res<CommandTx>,
    mut mode: ResMut<InputMode>,
    mut target: ResMut<Target>,
    mut autorun: ResMut<AutoRun>,
    mut camera: CameraInputParams,
    cam_q: Query<(&Camera, &Transform), With<OperatorCamera>>,
    mut exit: MessageWriter<AppExit>,
    mut rest_stance: ResMut<ffxi_viewer_core::combat_stance::RestStance>,
    mut walk_mode: ResMut<ffxi_viewer_core::combat_stance::WalkMode>,
    mut tab_stack: ResMut<TabCycleStack>,
    select_target: Res<SelectTargetMode>,
) {
    let camera_mode = &mut camera.mode;
    let chase = &mut camera.chase;
    let cursor_lock = &mut camera.cursor_lock;
    let lock_on = &mut camera.lock_on;
    let camera_transition = &mut camera.transition;

    let cmd_held = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
    let close_shortcut =
        cmd_held && (keys.just_pressed(KeyCode::KeyQ) || keys.just_pressed(KeyCode::KeyW));
    let os_close = window_close.read().next().is_some();
    if close_shortcut || os_close {
        let _ = cmd_tx.0.try_send(AgentCommand::Disconnect);
        exit.write_default();
        return;
    }

    if !matches!(*mode, InputMode::Chat(_))
        && bindings.just_pressed(Action::ToggleFirstPerson, &keys)
    {
        camera_transition.begin(**camera_mode, chase.distance);
        cursor_lock.locked = false;
    }

    if bindings.just_pressed(Action::TogglePassiveCursor, &keys) {
        match *mode {
            InputMode::World => {
                *mode = InputMode::PassiveCursor(PassiveCursorState::fresh_chat());
                return;
            }
            InputMode::PassiveCursor(_) => {
                *mode = InputMode::World;
                return;
            }
            _ => {}
        }
    }

    if !matches!(*mode, InputMode::World) {
        return;
    }

    if bindings.just_pressed(Action::OpenChatCommand, &keys) {
        *mode = InputMode::Chat(ChatBuffer::empty());
        return;
    }
    if bindings.just_pressed(Action::OpenMenu, &keys) {
        *mode = InputMode::Menu(MenuStack::root());
        return;
    }

    if !select_target.active && bindings.just_pressed(Action::ClearTarget, &keys) {
        target.id = None;
    }

    let tab = bindings.just_pressed(Action::CycleTarget, &keys);

    let enter_acquire = bindings.just_pressed(Action::ConfirmAction, &keys)
        && target.id.is_none()
        && !ffxi_viewer_core::hud::death_prompt::is_dead(&state);
    if tab || enter_acquire {
        if let Ok((camera, cam_t)) = cam_q.single() {
            let cam_global = GlobalTransform::from(*cam_t);

            let party_ids: Vec<u32> = state.snapshot.party.iter().map(|p| p.id).collect();
            let owner = state.snapshot.self_char_id.unwrap_or(0);
            let owned_pet_ids: Vec<u32> = state
                .snapshot
                .entities
                .iter()
                .filter(|e| matches!(e.kind, EntityKind::Pet) && e.claim_id == owner)
                .map(|e| e.id)
                .collect();

            if let Some(next) = tab_cycle_next(
                &mut tab_stack,
                &state.snapshot.entities,
                state.snapshot.self_pos.pos,
                target.id,
                state.snapshot.self_char_id,
                &party_ids,
                &owned_pet_ids,
                |world_pos| camera.world_to_ndc(&cam_global, world_pos),
            ) {
                target.id = Some(next);
            }
        }
    }

    let party_slot = if bindings.just_pressed(Action::TargetSelf, &keys) {
        Some(1)
    } else if bindings.just_pressed(Action::TargetParty2, &keys) {
        Some(2)
    } else if bindings.just_pressed(Action::TargetParty3, &keys) {
        Some(3)
    } else if bindings.just_pressed(Action::TargetParty4, &keys) {
        Some(4)
    } else if bindings.just_pressed(Action::TargetParty5, &keys) {
        Some(5)
    } else if bindings.just_pressed(Action::TargetParty6, &keys) {
        Some(6)
    } else {
        None
    };
    if let Some(slot) = party_slot {
        let id = if slot == 1 {
            state.snapshot.self_char_id
        } else {
            state.snapshot.party.get((slot - 1) as usize).map(|p| p.id)
        };
        if let Some(id) = id {
            target.id = Some(id);
        }
    }
    if bindings.just_pressed(Action::ToggleAutorun, &keys)
        && bindings.pressed(Action::MoveForward, &keys)
    {
        autorun.phantom_forward = !autorun.phantom_forward;
    }
    if bindings.just_pressed(Action::ToggleWalk, &keys) {
        walk_mode.walking = !walk_mode.walking;
    }
    if bindings.just_pressed(Action::ToggleEngage, &keys) {
        let currently_engaged = matches!(
            state.snapshot.current_goal,
            Some(ffxi_viewer_wire::ReactorGoal::Engaged { .. })
        );
        if currently_engaged {
            let _ = cmd_tx.0.try_send(AgentCommand::Cancel);
            state.push_local_toast(ffxi_viewer_wire::ChatLine {
                channel: ffxi_viewer_wire::ChatChannel::Debug,
                sender: "client".into(),
                text: "disengage".into(),
                server_ts: 0,
                local_seq: 0,
            });
        } else {
            match target.id {
                Some(id) => {
                    let name = state
                        .snapshot
                        .entities
                        .iter()
                        .find(|e| e.id == id)
                        .and_then(|e| e.name.clone())
                        .unwrap_or_else(|| format!("#{id:08X}"));
                    let _ = cmd_tx.0.try_send(AgentCommand::Engage { target_id: id });
                    state.push_local_toast(ffxi_viewer_wire::ChatLine {
                        channel: ffxi_viewer_wire::ChatChannel::Debug,
                        sender: "client".into(),
                        text: format!("engaging {name}"),
                        server_ts: 0,
                        local_seq: 0,
                    });
                }
                None => {
                    state.push_local_toast(ffxi_viewer_wire::ChatLine {
                        channel: ffxi_viewer_wire::ChatChannel::Debug,
                        sender: "client".into(),
                        text: "engage: no target (Tab to cycle)".into(),
                        server_ts: 0,
                        local_seq: 0,
                    });
                }
            }
        }
    }

    if bindings.just_pressed(Action::Sit, &keys) {
        use ffxi_viewer_core::combat_stance::RestKind;
        let next = match rest_stance.kind {
            RestKind::Sit => RestKind::None,

            RestKind::Heal => {
                let _ = cmd_tx.0.try_send(AgentCommand::Heal {
                    mode: crate::state::HealMode::Off,
                });
                RestKind::None
            }
            RestKind::None => RestKind::Sit,
        };
        rest_stance.kind = next;
    }
    if bindings.just_pressed(Action::Heal, &keys) {
        use ffxi_viewer_core::combat_stance::RestKind;
        let (next_kind, wire_mode) = match rest_stance.kind {
            RestKind::Heal => (RestKind::None, crate::state::HealMode::Off),

            _ => (RestKind::Heal, crate::state::HealMode::On),
        };
        let _ = cmd_tx.0.try_send(AgentCommand::Heal { mode: wire_mode });
        rest_stance.kind = next_kind;
    }

    if bindings.just_pressed(Action::ToggleLockOn, &keys) {
        let result = lock_on.toggle(target.id);
        let toast = match result {
            LockOnToggle::Locked(id) => {
                let name = state
                    .snapshot
                    .entities
                    .iter()
                    .find(|e| e.id == id)
                    .and_then(|e| e.name.clone())
                    .unwrap_or_else(|| format!("#{id:08X}"));
                format!("lock-on: {name}")
            }
            LockOnToggle::Cleared => "lock-on cleared".into(),
            LockOnToggle::NoTarget => "lock-on: no target".into(),
        };
        state.push_local_toast(ffxi_viewer_wire::ChatLine {
            channel: ffxi_viewer_wire::ChatChannel::Debug,
            sender: "client".into(),
            text: toast,
            server_ts: 0,
            local_seq: 0,
        });
    }

    if let Some(id) = lock_on.target_id {
        let still_visible = state.snapshot.entities.iter().any(|e| e.id == id);
        if !still_visible {
            lock_on.target_id = None;
        }
    }
}

pub fn dispatch_target_change_system(
    target: Res<Target>,
    state: Res<SceneState>,
    cmd_tx: Res<CommandTx>,
    mode: Res<InputMode>,
) {
    if !target.is_changed() {
        return;
    }

    if !matches!(
        *mode,
        InputMode::World
            | InputMode::Menu(_)
            | InputMode::QuickAction(_)
            | InputMode::TargetAction(_)
            | InputMode::PassiveCursor(_)
    ) {
        return;
    }

    let (target_id, target_index) = match target.id {
        Some(id) => match state.snapshot.entities.iter().find(|e| e.id == id) {
            Some(ent) => (id, ent.act_index),

            None => return,
        },
        None => (0, 0),
    };

    let _ = cmd_tx.0.try_send(AgentCommand::Action {
        target_id,
        target_index,
        kind: ActionKind::ChangeTarget,
    });
}

pub fn dispatch_movement_system(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<Bindings>,
    time: Res<Time<Fixed>>,
    state: Res<SceneState>,
    cmd_tx: Res<CommandTx>,
    mode: Res<InputMode>,
    camera_mode: Res<CameraMode>,
    lock_on: Res<LockOn>,
    mut autorun: ResMut<AutoRun>,
    mut chase: ResMut<ChaseCamera>,
    mut turn_accum: ResMut<HeadingTurnAccum>,
    mut prediction: ResMut<LocalPlayerPrediction>,
    navmesh: Res<super::navmesh_overlay::NavmeshState>,
    minimap_hover: Res<ffxi_viewer_core::minimap::input::MinimapHoverGate>,
    mut rest_stance: ResMut<ffxi_viewer_core::combat_stance::RestStance>,
    walk_mode: Res<ffxi_viewer_core::combat_stance::WalkMode>,
) {
    if matches!(*mode, InputMode::Chat(_) | InputMode::Dialog(_)) {
        autorun.phantom_forward = false;
        autorun.strafe_held_since = None;
        return;
    }

    let in_picker = matches!(
        *mode,
        InputMode::Menu(_)
            | InputMode::QuickAction(_)
            | InputMode::TargetAction(_)
            | InputMode::PassiveCursor(_)
    );

    let mut pitch_d = 0.0;
    if !in_picker && bindings.pressed(Action::CameraPitchUp, &keys) {
        pitch_d += PITCH_STEP_HELD;
    }
    if !in_picker && bindings.pressed(Action::CameraPitchDown, &keys) {
        pitch_d -= PITCH_STEP_HELD;
    }
    if pitch_d != 0.0 {
        let (lo, hi) = match *camera_mode {
            CameraMode::Chase => (ChaseCamera::PITCH_MIN, ChaseCamera::PITCH_MAX),
            CameraMode::FirstPerson => (ChaseCamera::FP_PITCH_MIN, ChaseCamera::FP_PITCH_MAX),
        };
        chase.pitch = (chase.pitch + pitch_d).clamp(lo, hi);
    }

    let mut yaw_d = 0.0;
    let yaw_step = CAMERA_YAW_RATE * time.delta_secs();
    if !in_picker && bindings.pressed(Action::CameraYawLeft, &keys) {
        yaw_d -= yaw_step;
    }
    if !in_picker && bindings.pressed(Action::CameraYawRight, &keys) {
        yaw_d += yaw_step;
    }
    if yaw_d != 0.0 {
        chase.yaw += yaw_d;
    }

    if matches!(*camera_mode, CameraMode::Chase) && !in_picker && !minimap_hover.hovered {
        let mut zoom_d = 0.0;
        let step = ChaseCamera::KEYBOARD_ZOOM_RATE * time.delta_secs();
        if bindings.pressed(Action::CameraZoomIn, &keys) {
            zoom_d -= step;
        }
        if bindings.pressed(Action::CameraZoomOut, &keys) {
            zoom_d += step;
        }
        if zoom_d != 0.0 {
            chase.distance =
                (chase.distance + zoom_d).clamp(ChaseCamera::DIST_MIN, ChaseCamera::DIST_MAX);
        }
    }

    if rest_stance.is_resting() {
        use ffxi_viewer_core::combat_stance::RestKind;
        let move_actions = [
            Action::MoveForward,
            Action::MoveBackward,
            Action::StrafeLeft,
            Action::StrafeRight,
            Action::TurnLeft,
            Action::TurnRight,
            Action::RotateLeft,
            Action::RotateRight,
        ];
        let pressed_move = move_actions
            .iter()
            .any(|a| bindings.just_pressed(*a, &keys));
        if pressed_move {
            if matches!(rest_stance.kind, RestKind::Heal) {
                let _ = cmd_tx.0.try_send(AgentCommand::Heal {
                    mode: crate::state::HealMode::Off,
                });
            }
            rest_stance.kind = RestKind::None;
        } else {
            autorun.phantom_forward = false;
            autorun.strafe_held_since = None;
            return;
        }
    }

    let mut forward: i32 = 0;
    if bindings.pressed(Action::MoveForward, &keys) {
        forward += 1;
    }
    if bindings.pressed(Action::MoveBackward, &keys) {
        forward -= 1;
    }

    if bindings.just_pressed(Action::MoveBackward, &keys) {
        autorun.phantom_forward = false;
    }

    let mut player_rotate_dir: i32 = 0;
    if bindings.pressed(Action::RotateLeft, &keys) {
        player_rotate_dir -= 1;
    }
    if bindings.pressed(Action::RotateRight, &keys) {
        player_rotate_dir += 1;
    }
    if matches!(*camera_mode, CameraMode::FirstPerson) {
        if bindings.pressed(Action::TurnLeft, &keys) {
            player_rotate_dir -= 1;
        }
        if bindings.pressed(Action::TurnRight, &keys) {
            player_rotate_dir += 1;
        }
    }
    let (player_rotate_u8, heading_delta_units) =
        advance_heading_turn(&mut turn_accum.units, player_rotate_dir, time.delta_secs());

    let mut strafe: i32 = 0;
    if bindings.pressed(Action::StrafeLeft, &keys) {
        strafe -= 1;
    }
    if bindings.pressed(Action::StrafeRight, &keys) {
        strafe += 1;
    }

    let mut turn: i32 = 0;
    if bindings.pressed(Action::TurnLeft, &keys) {
        turn -= 1;
    }
    if bindings.pressed(Action::TurnRight, &keys) {
        turn += 1;
    }

    let any_strafe_or_rotate = bindings.pressed(Action::RotateLeft, &keys)
        || bindings.pressed(Action::RotateRight, &keys)
        || strafe != 0;
    if any_strafe_or_rotate {
        let now = Instant::now();
        let started = *autorun.strafe_held_since.get_or_insert(now);
        if autorun.phantom_forward
            && now.duration_since(started) >= Duration::from_millis(STRAFE_CANCEL_MS)
        {
            autorun.phantom_forward = false;
        }
    } else {
        autorun.strafe_held_since = None;
    }

    if autorun.phantom_forward {
        forward = forward.max(1);
    }

    let turn_in_chase = turn != 0 && matches!(*camera_mode, CameraMode::Chase);

    let self_pos = state.snapshot.self_pos;

    let self_present = state
        .snapshot
        .self_char_id
        .is_some_and(|id| state.snapshot.entities.iter().any(|e| e.id == id));
    if !self_present {
        prediction.initialized = false;
        return;
    }

    let snap_pos = Vec3::new(self_pos.pos.x, self_pos.pos.y, self_pos.pos.z);
    let basis_pos = if !prediction.initialized
        || (snap_pos - prediction.pos).length() > PREDICTION_RESYNC_YALMS
    {
        prediction.pos = snap_pos;
        prediction.initialized = true;
        snap_pos
    } else {
        prediction.pos
    };

    let locked_heading: Option<u8> = lock_on.target_id.and_then(|id| {
        state
            .snapshot
            .entities
            .iter()
            .find(|e| e.id == id)
            .and_then(|ent| {
                let dx = ent.pos.x - self_pos.pos.x;
                let dy = ent.pos.y - self_pos.pos.y;
                if dx.abs() <= 0.001 && dy.abs() <= 0.001 {
                    None
                } else {
                    let radians = dy.atan2(dx);
                    let raw = radians * -(128.0 / std::f32::consts::PI);
                    Some((raw.round() as i32).rem_euclid(256) as u8)
                }
            })
    });

    let lock_forward_allowance: Option<f32> = lock_on.target_id.and_then(|id| {
        state
            .snapshot
            .entities
            .iter()
            .find(|e| e.id == id)
            .map(|ent| {
                let stop = crate::state::MODEL_RADIUS_PC
                    + radius_for_wire_kind(ent.kind)
                    + crate::state::CONTACT_GAP;
                forward_allowance((basis_pos.x, basis_pos.y), (ent.pos.x, ent.pos.y), stop)
            })
    });

    if player_rotate_dir != 0 {
        chase.yaw -= heading_delta_units * std::f32::consts::TAU / 256.0;
    }

    if forward == 0 && strafe == 0 && player_rotate_u8 == 0 && !turn_in_chase {
        if let Some(h) = locked_heading {
            if h != self_pos.heading {
                chase.yaw = ffxi_viewer_core::yaw_for_heading(h);

                let _ = cmd_tx.0.try_send(AgentCommand::Move {
                    x: basis_pos.x,
                    y: basis_pos.y,
                    z: basis_pos.z,
                    heading: h,
                });
            }
        }
        return;
    }

    let mut heading = self_pos.heading;
    if forward != 0 {
        heading = heading_for_yaw(chase.yaw);
    }
    if player_rotate_u8 != 0 {
        let delta = player_rotate_u8.rem_euclid(256) as u8;
        heading = heading.wrapping_add(delta);
    }

    let mut turn_dx: f32 = 0.0;
    let mut turn_dy: f32 = 0.0;
    if turn_in_chase {
        let camera_forward_h = heading_for_yaw(chase.yaw);
        let (cf_x, cf_y) = heading_to_forward(camera_forward_h);

        let (cr_x, cr_y) = heading_to_forward(camera_forward_h.wrapping_add(64));

        let fwd_signed = forward as f32;
        let lat_signed = turn as f32;
        let mx = cf_x * fwd_signed + cr_x * lat_signed;
        let my = cf_y * fwd_signed + cr_y * lat_signed;
        let len = (mx * mx + my * my).sqrt();

        let step_magnitude =
            self_pos.speed as f32 * SPEED_TO_YPS * time.delta_secs() * walk_mode.scale();
        if len > 1e-3 && step_magnitude > 0.0 {
            let inv = 1.0 / len;
            turn_dx = mx * inv * step_magnitude;
            turn_dy = my * inv * step_magnitude;

            let motion_radians = my.atan2(mx);
            let motion_raw = motion_radians * -(128.0 / std::f32::consts::PI);
            let motion_h = (motion_raw.round() as i32).rem_euclid(256) as u8;

            let h_target = yaw_for_heading(motion_h);
            let h_current = yaw_for_heading(heading);
            let h_diff = wrap_signed_pi(h_target - h_current);

            let h_alpha = 1.0 - (-HEADING_LERP_RATE_RAD_PER_SEC * time.delta_secs()).exp();
            heading = heading_for_yaw(h_current + h_diff * h_alpha);
        }

        let chase_target = yaw_for_heading(heading);
        let c_diff = wrap_signed_pi(chase_target - chase.yaw);
        let c_alpha = 1.0 - (-CHASE_TRACK_RATE_RAD_PER_SEC * time.delta_secs()).exp();
        chase.yaw += c_diff * c_alpha;

        forward = 0;
        strafe = 0;
    }

    if let Some(h) = locked_heading {
        heading = h;
        chase.yaw = ffxi_viewer_core::yaw_for_heading(h);
    }

    let raw_step = self_pos.speed as f32 * SPEED_TO_YPS * time.delta_secs() * walk_mode.scale();

    let dir_scale = if forward > 0 && strafe != 0 {
        std::f32::consts::FRAC_1_SQRT_2
    } else if forward < 0 {
        BACKPEDAL_SCALE
    } else if forward == 0 && strafe != 0 {
        STRAFE_SCALE
    } else {
        1.0
    };
    let step = raw_step * dir_scale;
    let mut x = basis_pos.x;
    let mut y = basis_pos.y;

    x += turn_dx;
    y += turn_dy;
    if forward != 0 && step > 0.0 {
        let (fwd_x, fwd_y) = heading_to_forward(heading);

        let fwd_step = match (forward > 0, lock_forward_allowance) {
            (true, Some(allowed)) => step.min(allowed),
            _ => step,
        };
        x += fwd_x * fwd_step * forward as f32;
        y += fwd_y * fwd_step * forward as f32;
    }
    if strafe != 0 && step > 0.0 {
        let right_heading = heading.wrapping_add(64);
        let (right_x, right_y) = heading_to_forward(right_heading);
        x += right_x * step * strafe as f32;
        y += right_y * step * strafe as f32;
    }

    let (final_x, final_y, final_z) = if let Some(nav) = &navmesh.nav {
        let from = ffxi_nav::glam::Vec3::new(basis_pos.x, basis_pos.y, basis_pos.z);
        let to = ffxi_nav::glam::Vec3::new(x, y, basis_pos.z);
        let slid = nav
            .lock()
            .ok()
            .and_then(|guard| guard.slide_along(from, to));

        let proposed = ((x - basis_pos.x).powi(2) + (y - basis_pos.y).powi(2)).sqrt();
        if proposed > 0.1 {
            let (resulting, branch) = match &slid {
                Some(p) => {
                    let r = ((p.x - basis_pos.x).powi(2) + (p.y - basis_pos.y).powi(2)).sqrt();
                    (r, "slide_some")
                }
                None => (proposed, "slide_none_passthrough"),
            };
            if resulting < 0.1 {
                tracing::warn!(
                    branch,
                    from_xy = format!("({:.2},{:.2},{:.2})", from.x, from.y, from.z),
                    to_xy = format!("({:.2},{:.2})", to.x, to.y),
                    slid_xy = ?slid.as_ref().map(|p| (p.x, p.y, p.z)),
                    proposed_step = proposed,
                    resulting_step = resulting,
                    "wall-slide probe: proposed move but stuck (resulting <0.1)"
                );
            }
        }
        match slid {
            Some(p) => (p.x, p.y, p.z),
            None => (x, y, basis_pos.z),
        }
    } else {
        (x, y, basis_pos.z)
    };

    // slide_along passes Z through; the server's engage range is 3D, so an
    // off-ground reported Y inflates distance to grounded mobs. Re-ground here.
    let final_z = navmesh
        .nav
        .as_ref()
        .and_then(|nav| {
            nav.lock()
                .ok()?
                .nearest_height_at(final_x, final_y, basis_pos.z)
        })
        .unwrap_or(final_z);

    let _ = cmd_tx.0.try_send(AgentCommand::Move {
        x: final_x,
        y: final_y,
        z: final_z,
        heading,
    });

    prediction.pos = Vec3::new(final_x, final_y, final_z);
}

fn heading_to_forward(heading: u8) -> (f32, f32) {
    let angle = (heading as f32) * std::f32::consts::TAU / 256.0;
    (angle.cos(), -angle.sin())
}

fn radius_for_wire_kind(kind: EntityKind) -> f32 {
    match kind {
        EntityKind::Pc => crate::state::MODEL_RADIUS_PC,
        EntityKind::Npc => crate::state::MODEL_RADIUS_NPC,
        EntityKind::Mob => crate::state::MODEL_RADIUS_MOB,
        EntityKind::Pet => crate::state::MODEL_RADIUS_PET,
        EntityKind::Other => crate::state::MODEL_RADIUS_OTHER,
    }
}

fn forward_allowance(from_xy: (f32, f32), target_xy: (f32, f32), stop: f32) -> f32 {
    let dx = target_xy.0 - from_xy.0;
    let dy = target_xy.1 - from_xy.1;
    let dist = (dx * dx + dy * dy).sqrt();
    (dist - stop).max(0.0)
}

#[inline]
fn wrap_signed_pi(x: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (x + PI).rem_euclid(TAU) - PI
}

#[derive(Resource, Default)]
pub struct TabCycleStack {
    ids: VecDeque<u32>,

    idle_secs: f32,

    last_emitted: Option<u32>,
}

impl TabCycleStack {
    pub fn pending_len(&self) -> usize {
        self.ids.len()
    }

    pub fn idle_secs(&self) -> f32 {
        self.idle_secs
    }
}

pub fn build_tab_candidates<F>(
    entities: &[WireEntity],
    from: WireVec3,
    self_id: Option<u32>,
    party_ids: &[u32],
    owned_pet_ids: &[u32],
    project: F,
) -> Vec<u32>
where
    F: Fn(Vec3) -> Option<Vec3>,
{
    struct Cand {
        id: u32,
        tier: u8,
        score: f32,
    }

    let mut candidates: Vec<Cand> = entities
        .iter()
        .filter(|e| Some(e.id) != self_id)
        .filter(|e| e.is_cycle_candidate())
        .filter_map(|e| {
            let ground = ffxi_viewer_core::ffxi_to_bevy(e.pos);
            let mut center_off: Option<f32> = None;
            for h in TAB_SAMPLE_HEIGHTS {
                let Some(ndc) = project(ground + Vec3::Y * h) else {
                    continue;
                };
                if ndc.z < 0.0 || ndc.z > 1.0 {
                    continue;
                }
                if ndc.x.abs() > CYCLE_NDC_X_LIMIT || ndc.y.abs() > CYCLE_NDC_Y_LIMIT {
                    continue;
                }
                let off = ndc.x.abs();
                if center_off.is_none_or(|m| off < m) {
                    center_off = Some(off);
                }
            }
            let center_off = center_off?;

            let dx = e.pos.x - from.x;
            let dy = e.pos.y - from.y;
            let dz = e.pos.z - from.z;
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            let tier = u8::from(party_ids.contains(&e.id) || owned_pet_ids.contains(&e.id));
            Some(Cand {
                id: e.id,
                tier,
                score: dist + NDC_PENALTY_YALMS * center_off,
            })
        })
        .collect();

    candidates.sort_by(|a, b| {
        a.tier.cmp(&b.tier).then_with(|| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    candidates.into_iter().map(|c| c.id).collect()
}

#[allow(clippy::too_many_arguments)]
pub fn tab_cycle_next<F>(
    stack: &mut TabCycleStack,
    entities: &[WireEntity],
    from: WireVec3,
    current: Option<u32>,
    self_id: Option<u32>,
    party_ids: &[u32],
    owned_pet_ids: &[u32],
    project: F,
) -> Option<u32>
where
    F: Fn(Vec3) -> Option<Vec3>,
{
    stack
        .ids
        .retain(|id| Some(*id) != current && entities.iter().any(|e| e.id == *id));

    if stack.ids.is_empty() {
        let order =
            build_tab_candidates(entities, from, self_id, party_ids, owned_pet_ids, &project);
        stack.ids = order
            .into_iter()
            .filter(|id| Some(*id) != current)
            .collect();
    }
    let next = stack.ids.pop_front()?;
    stack.idle_secs = 0.0;
    stack.last_emitted = Some(next);
    Some(next)
}

pub fn tab_cycle_invalidate_system(
    target: Res<Target>,
    time: Res<Time>,
    mut stack: ResMut<TabCycleStack>,
) {
    stack.idle_secs += time.delta_secs();
    if stack.idle_secs > TAB_CYCLE_IDLE_RESET_SECS {
        stack.ids.clear();
    }
    if target.is_changed() && target.id != stack.last_emitted {
        stack.ids.clear();
        stack.last_emitted = target.id;
    }
}

#[derive(Resource, Default)]
pub struct CameraAutoRecenter {
    pub forward_held_since: Option<Instant>,

    pub manual_override: bool,
}

const AUTO_RECENTER_RATE: f32 = 3.0;

const FP_LOCK_PITCH_RATE: f32 = 3.0;

const TARGET_HEAD_OFFSET_Y: f32 = 1.5;

pub fn camera_polish_system(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<Bindings>,
    time: Res<Time>,
    mode: Res<InputMode>,
    camera_mode: Res<CameraMode>,
    state: Res<SceneState>,
    lock_on: Res<LockOn>,
    pointer: Res<ffxi_viewer_core::MousePointer>,
    mut chase: ResMut<ChaseCamera>,
    mut recenter: ResMut<CameraAutoRecenter>,
    self_q: Query<&Transform, (With<IsSelf>, Without<OperatorCamera>)>,
    target_q: Query<(&WorldEntity, &Transform), Without<OperatorCamera>>,
) {
    if !matches!(*mode, InputMode::World) {
        recenter.forward_held_since = None;
        return;
    }

    let yaw_input = bindings.pressed(Action::CameraYawLeft, &keys)
        || bindings.pressed(Action::CameraYawRight, &keys);
    let drag_active = pointer.left || pointer.right;
    if yaw_input || drag_active {
        recenter.manual_override = true;
    }
    let movement_input = bindings.pressed(Action::MoveForward, &keys)
        || bindings.pressed(Action::MoveBackward, &keys)
        || bindings.pressed(Action::StrafeLeft, &keys)
        || bindings.pressed(Action::StrafeRight, &keys)
        || bindings.pressed(Action::TurnLeft, &keys)
        || bindings.pressed(Action::TurnRight, &keys);
    if movement_input {
        recenter.manual_override = false;
    }

    if !yaw_input
        && !drag_active
        && !recenter.manual_override
        && matches!(*camera_mode, CameraMode::Chase)
    {
        let target_yaw = yaw_for_heading(state.snapshot.self_pos.heading);
        let tau = std::f32::consts::TAU;
        let mut diff = (target_yaw - chase.yaw).rem_euclid(tau);
        if diff > std::f32::consts::PI {
            diff -= tau;
        }
        let alpha = 1.0 - (-AUTO_RECENTER_RATE * time.delta_secs()).exp();
        chase.yaw += diff * alpha;
    }

    if !matches!(*camera_mode, CameraMode::FirstPerson) {
        return;
    }
    let Some(target_id) = lock_on.target_id else {
        return;
    };
    let Ok(self_t) = self_q.single() else {
        return;
    };
    let mut target_pos: Option<Vec3> = None;
    for (we, t) in target_q.iter() {
        if we.id == target_id {
            target_pos = Some(t.translation);
            break;
        }
    }
    let Some(target_pos) = target_pos else {
        return;
    };

    let eye = self_t.translation + Vec3::Y * ffxi_viewer_core::first_person_eye_y(None);
    let head = target_pos + Vec3::Y * TARGET_HEAD_OFFSET_Y;
    let to_head = head - eye;

    let horiz = (to_head.x * to_head.x + to_head.z * to_head.z).sqrt();
    if horiz < 1e-4 && to_head.y.abs() < 1e-4 {
        return;
    }

    let desired_pitch = to_head
        .y
        .atan2(horiz)
        .clamp(ChaseCamera::FP_PITCH_MIN, ChaseCamera::FP_PITCH_MAX);
    let max_step = FP_LOCK_PITCH_RATE * time.delta_secs();
    let diff = desired_pitch - chase.pitch;
    let step = diff.clamp(-max_step, max_step);
    chase.pitch += step;
}

const CYCLE_NDC_X_LIMIT: f32 = 1.1;

const CYCLE_NDC_Y_LIMIT: f32 = 1.6;

const TAB_CYCLE_IDLE_RESET_SECS: f32 = 2.0;

const NDC_PENALTY_YALMS: f32 = 10.0;

const TAB_SAMPLE_HEIGHTS: [f32; 5] = [0.0, 0.5, 1.0, 1.5, 2.0];

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_viewer_wire::{Entity as WireEntity, EntityKind, Vec3 as WireVec3};

    #[test]
    fn forward_allowance_caps_at_contact() {
        let a = forward_allowance((0.0, 0.0), (5.0, 0.0), 0.7);
        assert!((a - 4.3).abs() < 1e-3, "got {a}");
    }

    #[test]
    fn forward_allowance_zero_at_or_inside_contact() {
        assert!(forward_allowance((0.0, 0.0), (0.7, 0.0), 0.7).abs() < 1e-6);

        assert_eq!(forward_allowance((0.0, 0.0), (0.4, 0.0), 0.7), 0.0);
    }

    #[test]
    fn radius_for_wire_kind_matches_state_source() {
        assert_eq!(
            radius_for_wire_kind(EntityKind::Pc),
            crate::state::MODEL_RADIUS_PC
        );
        assert_eq!(
            radius_for_wire_kind(EntityKind::Mob),
            crate::state::MODEL_RADIUS_MOB
        );
        assert_eq!(
            radius_for_wire_kind(EntityKind::Pet),
            crate::state::MODEL_RADIUS_PET
        );
    }

    fn ent(id: u32, x: f32, y: f32) -> WireEntity {
        ent_xyz(id, x, y, 0.0)
    }

    fn ent_xyz(id: u32, x: f32, y: f32, z: f32) -> WireEntity {
        WireEntity {
            id,
            act_index: 0,
            kind: EntityKind::Mob,
            name: None,
            pos: WireVec3 { x, y, z },
            heading: 0,
            hp_pct: None,
            bt_target_id: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            status: 0,
        }
    }

    fn fake_proj(p: Vec3) -> Option<Vec3> {
        Some(Vec3::new(p.x / 100.0, 0.0, 0.5))
    }

    fn culled_proj(p: Vec3) -> Option<Vec3> {
        if p.x > 50.0 {
            None
        } else {
            Some(Vec3::new(p.x / 100.0, 0.0, 0.5))
        }
    }

    #[test]
    fn heading_turn_accumulates_to_finite_rate_over_one_second() {
        let mut accum = 0.0_f32;
        let dt = 1.0 / 60.0;
        let mut total_u8: i32 = 0;
        for _ in 0..60 {
            let (whole, _f) = advance_heading_turn(&mut accum, 1, dt);
            total_u8 += whole;
        }
        let expected = (HEADING_TURN_RATE * 256.0 / std::f32::consts::TAU).round() as i32;

        assert!(
            (total_u8 - expected).abs() <= 1,
            "1s of held turn produced {total_u8} u8 (expected ~{expected})",
        );

        let degrees = total_u8 as f32 * 360.0 / 256.0;
        assert!(
            (degrees - 49.0).abs() < 3.0,
            "1s of held turn = {degrees:.1}°, expected ~49°",
        );
    }

    #[test]
    fn heading_turn_does_not_round_to_zero_per_tick() {
        let mut accum = 0.0_f32;
        let dt = 1.0 / 60.0;

        let (whole_1, float_1) = advance_heading_turn(&mut accum, 1, dt);
        assert_eq!(whole_1, 0, "first 60Hz tick must not yet flip a u8");
        assert!(float_1 > 0.0 && float_1 < 1.0);
        assert!(accum > 0.0, "fractional units must carry over");

        let mut flipped = false;
        for _ in 0..10 {
            let (w, _) = advance_heading_turn(&mut accum, 1, dt);
            if w != 0 {
                flipped = true;
                break;
            }
        }
        assert!(flipped, "accumulator never produced a whole-unit step");
    }

    #[test]
    fn heading_turn_release_clears_fraction() {
        let mut accum = 0.0_f32;
        let dt = 1.0 / 60.0;

        let _ = advance_heading_turn(&mut accum, 1, dt);
        assert!(accum > 0.0);

        let (whole, fdelta) = advance_heading_turn(&mut accum, 0, dt);
        assert_eq!(whole, 0);
        assert_eq!(fdelta, 0.0);
        assert_eq!(accum, 0.0);
    }

    #[test]
    fn heading_turn_is_symmetric() {
        let dt = 1.0 / 60.0;
        let mut accum_l = 0.0_f32;
        let mut accum_r = 0.0_f32;
        let mut total_l: i32 = 0;
        let mut total_r: i32 = 0;
        for _ in 0..30 {
            total_l += advance_heading_turn(&mut accum_l, -1, dt).0;
            total_r += advance_heading_turn(&mut accum_r, 1, dt).0;
        }
        assert_eq!(total_l, -total_r);
    }

    fn wide_proj(p: Vec3) -> Option<Vec3> {
        Some(Vec3::new(p.x / 50.0, 0.0, 0.5))
    }

    fn xy_proj(p: Vec3) -> Option<Vec3> {
        Some(Vec3::new(p.x / 100.0, p.y / 100.0, 0.5))
    }

    fn grounded_only_proj(p: Vec3) -> Option<Vec3> {
        if (-11.0..=-7.0).contains(&p.y) {
            Some(Vec3::new(p.x / 100.0, 0.0, 0.5))
        } else {
            None
        }
    }

    fn from0() -> WireVec3 {
        WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }
    }

    fn ent_k(id: u32, x: f32, kind: EntityKind) -> WireEntity {
        let mut e = ent(id, x, 0.0);
        e.kind = kind;
        e
    }

    fn first_pick<F: Fn(Vec3) -> Option<Vec3>>(
        entities: &[WireEntity],
        self_id: Option<u32>,
        project: F,
    ) -> Option<u32> {
        build_tab_candidates(entities, from0(), self_id, &[], &[], project)
            .first()
            .copied()
    }

    fn drive<F: Fn(Vec3) -> Option<Vec3> + Copy>(
        entities: &[WireEntity],
        self_id: Option<u32>,
        n: usize,
        project: F,
    ) -> Vec<u32> {
        let mut stack = TabCycleStack::default();
        let mut current = None;
        let mut out = Vec::new();
        for _ in 0..n {
            current = tab_cycle_next(
                &mut stack,
                entities,
                from0(),
                current,
                self_id,
                &[],
                &[],
                project,
            );
            out.push(current.expect("cycle should yield a target"));
        }
        out
    }

    #[test]
    fn first_press_picks_nearest_on_screen() {
        let entities = vec![ent(1, 30.0, 0.0), ent(2, 10.0, 0.0), ent(3, 20.0, 0.0)];
        assert_eq!(first_pick(&entities, None, fake_proj), Some(2));
    }

    #[test]
    fn cycle_excludes_self() {
        let entities = vec![ent(99, 0.0, 0.0), ent(1, 10.0, 0.0), ent(2, 20.0, 0.0)];
        assert_eq!(first_pick(&entities, Some(99), fake_proj), Some(1));

        assert_eq!(drive(&entities, Some(99), 4, fake_proj), vec![1, 2, 1, 2]);
    }

    #[test]
    fn cycle_excludes_dead() {
        let mut dead_mob = ent(2, 10.0, 0.0);
        dead_mob.hp_pct = Some(0);

        let mut dead_pc = ent(4, 5.0, 0.0);
        dead_pc.kind = EntityKind::Pc;
        dead_pc.hp_pct = Some(0);

        let entities = vec![ent(1, 30.0, 0.0), dead_mob, ent(3, 20.0, 0.0), dead_pc];

        assert_eq!(first_pick(&entities, None, fake_proj), Some(3));
        assert_eq!(drive(&entities, None, 4, fake_proj), vec![3, 1, 3, 1]);
    }

    #[test]
    fn first_press_3d_distance_includes_altitude() {
        let entities = vec![ent_xyz(1, 0.0, 0.0, 5.0), ent_xyz(2, 0.0, 0.0, 50.0)];
        assert_eq!(first_pick(&entities, None, fake_proj), Some(1));
    }

    #[test]
    fn first_press_close_off_center_beats_far_centered() {
        let entities = vec![ent(1, 5.0, 30.0), ent(2, 20.0, 5.0)];
        assert_eq!(first_pick(&entities, None, fake_proj), Some(2));
    }

    #[test]
    fn first_press_combined_ndc_and_world_distance() {
        let entities = vec![ent_xyz(1, 0.0, 0.0, 80.0), ent_xyz(2, 15.0, 0.0, 15.0)];
        assert_eq!(first_pick(&entities, None, xy_proj), Some(2));
    }

    #[test]
    fn candidate_projects_at_canonical_grounded_height() {
        let entities = vec![ent_xyz(1, 5.0, 0.0, 10.0)];
        let order = build_tab_candidates(&entities, from0(), None, &[], &[], grounded_only_proj);
        assert_eq!(
            order,
            vec![1],
            "elevated entity must project at scene::ffxi_to_bevy height (-z), not the mirror (+z)"
        );
    }

    #[test]
    fn cycle_walks_nearest_to_farthest_then_wraps() {
        let entities = vec![ent(1, 30.0, 0.0), ent(2, 5.0, 0.0), ent(3, 15.0, 0.0)];
        assert_eq!(drive(&entities, None, 4, fake_proj), vec![2, 3, 1, 2]);
    }

    #[test]
    fn cycle_is_stable_under_position_jitter() {
        let mut entities = vec![
            ent(1, 5.0, 0.0),
            ent(2, 10.0, 0.0),
            ent(3, 15.0, 0.0),
            ent(4, 20.0, 0.0),
            ent(5, 25.0, 0.0),
        ];
        let mut stack = TabCycleStack::default();
        let mut current = None;
        let mut visited = Vec::new();
        for i in 0..5 {
            for e in entities.iter_mut() {
                e.pos.x += if i % 2 == 0 { 3.0 } else { -2.0 };
            }
            current = tab_cycle_next(
                &mut stack,
                &entities,
                from0(),
                current,
                None,
                &[],
                &[],
                fake_proj,
            );
            visited.push(current.unwrap());
        }
        let mut sorted = visited.clone();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            vec![1, 2, 3, 4, 5],
            "no repeats in a round: {visited:?}"
        );
    }

    #[test]
    fn cycle_refills_after_exhaustion() {
        let entities = vec![ent(1, 5.0, 0.0), ent(2, 10.0, 0.0), ent(3, 15.0, 0.0)];
        let seq = drive(&entities, None, 6, fake_proj);
        assert_eq!(seq.len(), 6);
        assert!(seq.iter().all(|&x| (1..=3).contains(&x)));
        let mut round1 = seq[0..3].to_vec();
        round1.sort_unstable();
        assert_eq!(round1, vec![1, 2, 3], "first round visits every candidate");
    }

    #[test]
    fn party_and_own_pet_sort_last() {
        let entities = vec![
            ent(1, 10.0, 0.0),
            ent_k(2, 5.0, EntityKind::Pc),
            ent_k(3, 15.0, EntityKind::Pet),
            ent_k(4, 20.0, EntityKind::Npc),
        ];
        let order = build_tab_candidates(&entities, from0(), None, &[2], &[3], fake_proj);

        assert_eq!(order, vec![1, 4, 2, 3]);
    }

    #[test]
    fn tab_keeps_current_when_it_is_the_only_candidate() {
        let entities = vec![ent(1, 10.0, 0.0)];
        let mut stack = TabCycleStack::default();
        assert_eq!(
            tab_cycle_next(
                &mut stack,
                &entities,
                from0(),
                Some(1),
                None,
                &[],
                &[],
                fake_proj
            ),
            None
        );
    }

    fn feet_below_screen_proj(p: Vec3) -> Option<Vec3> {
        Some(Vec3::new(p.x / 100.0, p.y - 1.5, 0.5))
    }

    #[test]
    fn near_mob_with_feet_off_bottom_is_still_cyclable() {
        let entities = vec![ent_xyz(1, 0.0, 0.0, 0.0)];
        assert_eq!(
            first_pick(&entities, None, feet_below_screen_proj),
            Some(1),
            "near mob with off-screen feet but on-screen body must be cyclable",
        );
    }

    #[test]
    fn fully_off_screen_mob_is_still_excluded() {
        fn all_below_proj(p: Vec3) -> Option<Vec3> {
            Some(Vec3::new(p.x / 100.0, p.y - 10.0, 0.5))
        }
        let entities = vec![ent_xyz(1, 0.0, 0.0, 0.0)];
        assert_eq!(first_pick(&entities, None, all_below_proj), None);
    }

    #[test]
    fn other_kind_is_never_a_candidate() {
        let entities = vec![ent_k(1, 10.0, EntityKind::Other), ent(2, 20.0, 0.0)];
        assert_eq!(first_pick(&entities, None, fake_proj), Some(2));
    }

    #[test]
    fn advance_records_last_emitted_and_resets_idle() {
        let entities = vec![ent(1, 10.0, 0.0), ent(2, 20.0, 0.0)];
        let mut stack = TabCycleStack {
            idle_secs: 99.0,
            ..Default::default()
        };
        let next = tab_cycle_next(
            &mut stack,
            &entities,
            from0(),
            None,
            None,
            &[],
            &[],
            fake_proj,
        );
        assert_eq!(next, Some(1));
        assert_eq!(stack.last_emitted, Some(1));
        assert_eq!(stack.idle_secs, 0.0);
    }

    #[test]
    fn cycle_includes_slightly_out_of_view_entities() {
        let entities = vec![ent(1, -25.0, 0.0), ent(2, 52.0, 0.0), ent(3, 70.0, 0.0)];
        let order = build_tab_candidates(&entities, from0(), None, &[], &[], wide_proj);
        assert_eq!(order, vec![1, 2]);
    }

    #[test]
    fn first_press_prefers_strictly_in_frustum() {
        let entities = vec![ent(1, 45.0, 0.0), ent(2, 52.0, 0.0)];
        assert_eq!(first_pick(&entities, None, wide_proj), Some(1));
    }

    fn first_person_proj(p: Vec3) -> Option<Vec3> {
        let eye = Vec3::new(0.0, 1.5, 0.0);
        let r = p - eye;
        let depth = -r.z;
        if depth <= 0.05 {
            return None;
        }
        let span = depth * 0.4;
        Some(Vec3::new(r.x / span, r.y / span, 0.5))
    }

    #[test]
    fn near_centered_mob_beats_far_mob_at_close_range() {
        let near = ent_xyz(1, 0.0, 1.2, 0.0);
        let far = ent_xyz(2, 0.0, 4.0, 0.0);
        assert_eq!(
            first_pick(&[near, far], None, first_person_proj),
            Some(1),
            "the closest horizontally-centered mob must win even when its body \
             spans the screen vertically",
        );
    }

    #[test]
    fn first_press_falls_back_to_relaxed_when_none_in_frustum() {
        let entities = vec![ent(1, 55.0, 0.0), ent(2, 52.0, 0.0)];
        assert_eq!(first_pick(&entities, None, wide_proj), Some(2));
    }

    #[test]
    fn off_screen_entities_are_skipped() {
        let entities = vec![ent(1, 0.0, 0.0), ent(4, 100.0, 0.0)];
        assert_eq!(first_pick(&entities, None, culled_proj), Some(1));

        let mut stack = TabCycleStack::default();
        assert_eq!(
            tab_cycle_next(
                &mut stack,
                &entities,
                from0(),
                Some(4),
                None,
                &[],
                &[],
                culled_proj
            ),
            Some(1)
        );
    }

    #[test]
    fn empty_or_all_off_screen_returns_none() {
        let none: Vec<WireEntity> = vec![];
        assert_eq!(first_pick(&none, None, fake_proj), None);
        let mut stack = TabCycleStack::default();
        assert_eq!(
            tab_cycle_next(&mut stack, &none, from0(), None, None, &[], &[], fake_proj),
            None
        );

        let entities = vec![ent(1, 100.0, 0.0), ent(2, 200.0, 0.0)];
        assert_eq!(first_pick(&entities, None, culled_proj), None);
    }
}
