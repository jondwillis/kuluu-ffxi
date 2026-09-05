use std::collections::VecDeque;
use std::time::{Duration, Instant};

use bevy::ecs::system::SystemParam;
use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::window::WindowCloseRequested;

#[derive(SystemParam)]
pub struct StanceParams<'w> {
    pub rest_stance: ResMut<'w, kuluu_render::combat_stance::RestStance>,
    pub walk_mode: Res<'w, kuluu_render::combat_stance::WalkMode>,
    pub move_intent: ResMut<'w, kuluu_render::combat_stance::SelfMoveIntent>,
}

#[derive(SystemParam)]
pub struct MoveEnvParams<'w> {
    // Player movement grounds height on the retail MZB zone collision (the real
    // .dat floor, which has the stairs). The coarse LSB Recast navmesh is a
    // mob-pathing mesh that flattens stairs, so it is NOT used here — only for
    // /pathto and minimap culling (kuluu-oe8y; see AGENTS.md).
    pub collision: Res<'w, kuluu_render::dat_mzb::MzbCollisionGeometry>,
    /// Dynamic obstacles rebuilt every fixed tick before dispatch (plan §2.5):
    /// closed door leaves (walls + floors) and mob circles. Bundled here — this
    /// fn sits at bevy's 16-param SystemParam ceiling.
    pub obstacles: Res<'w, super::walker::obstacles::ObstacleSet>,
    /// Debug noclip: when on, the wall clamp in dispatch_movement is bypassed
    /// (grounding stays on). Toggled from the Debug menu NoClip row or /noclip.
    pub hud_panels: Res<'w, kuluu_render::hud::HudPanels>,
    pub minimap_hover: Res<'w, kuluu_render::minimap::input::MinimapHoverGate>,
    pub pointer: Res<'w, kuluu_render::MousePointer>,
    pub pad: Res<'w, super::gamepad_input::PadStickIntent>,
    // Focus-less GUI driving (kuluu-0pof): remote movement injection.
    pub(crate) debug_ctrl: Option<Res<'w, super::DebugControlHandle>>,
    // Stair-capture drive channel (FFXI_STAIR_DRIVE): forward/strafe holds plus
    // a Q/E-style turn axis for the external driver. None unless wired at connect.
    pub stair_drive: Option<Res<'w, StairDriveHandle>>,
}

/// Rising-edge memory for the pad stick, standing in for `just_pressed` where
/// held keys have one (rest-break and autorun-cancel).
#[derive(Default)]
pub struct PadEdges {
    move_active: bool,
    back_active: bool,
}

/// Bundled per-tick locals for [`dispatch_movement_system`]. Kept as a
/// single `Local<DispatchLocals>` because bevy's `SystemParam` derive tops
/// out at 16 params per system and this fn was already at the ceiling.
#[derive(Default)]
pub struct DispatchLocals {
    /// Latched world-space run heading for pure W/S: (forward sign, motion
    /// heading). Sampled from the camera frame when the key state changes,
    /// then held fixed so the camera's auto-recenter can swing behind
    /// without dragging the run direction with it.
    pub steer_latch: Option<(i32, u8)>,
    /// Rising-edge memory for pad stick just_pressed emulation.
    pub pad_edges: PadEdges,
    /// Cross-tick walker state (modes, push-through accrual, fall velocity);
    /// the stub is stateless until the real step lands.
    pub walker: super::walker::Walker,
}

#[derive(SystemParam)]
pub struct HudCaptureParams<'w> {
    pub hud_hidden: ResMut<'w, kuluu_render::hud_hide::HudHidden>,
    pub screenshot: MessageWriter<'w, super::screenshot::ScreenshotRequest>,
}

#[derive(SystemParam)]
pub struct KeyActionSources<'w> {
    pub keys: Res<'w, ButtonInput<KeyCode>>,
    pub bindings: Res<'w, Bindings>,
    pub pad: Res<'w, super::gamepad_input::PadPressed>,
}

#[derive(SystemParam)]
pub struct CameraInputParams<'w> {
    pub mode: ResMut<'w, CameraMode>,
    pub chase: ResMut<'w, ChaseCamera>,
    pub cursor_lock: ResMut<'w, CursorLockRequest>,
    pub lock_on: ResMut<'w, LockOn>,
    pub transition: ResMut<'w, CameraTransition>,
}
use kuluu_render::{
    heading_for_yaw, yaw_for_heading, Action, Bindings, CameraMode, CameraTransition, ChaseCamera,
    ChatBuffer, CursorLockRequest, InputMode, IsSelf, LockOn, LockOnToggle, MenuStack,
    OperatorCamera, PassiveCursorState, SceneState, Target, WorldEntity,
};
use kuluu_snapshot::{Entity as WireEntity, EntityKind, Vec3 as WireVec3};
use tokio::sync::mpsc;

use kuluu_session::state::{ActionKind, AgentCommand, FishingInput};

// Matches the retail first-person A/D view-rotate rate (HorizonXI video
// 2026-07-20: ~71 heading-units over a 2s hold ≈ 0.87 rad/s).
pub const HEADING_TURN_RATE: f32 = 0.86;

// Q/E rotate-in-place has no retail 3rd-person counterpart; 0.86 felt too
// sluggish in play-testing, so it gets its own snappier rate.
pub const ROTATE_KEY_RATE_RAD_PER_SEC: f32 = 2.0;

const CAMERA_YAW_RATE: f32 = HEADING_TURN_RATE * 4.0;

const PITCH_STEP_HELD: f32 = 0.015;

const STRAFE_CANCEL_MS: u64 = 300;

use kuluu_session::state::{
    ground_correction_matches, move_speed_yps, GROUND_CORRECTION_XY_EPSILON_YALMS,
};

const BACKPEDAL_SCALE: f32 = 0.5;
const STRAFE_SCALE: f32 = 0.75;

// A stick pulled this far toward the camera cancels autorun, like a tapped S;
// gentler deflections only carve (retail autorun is steerable).
const PAD_BACK_CANCEL_DEFLECTION: f32 = 0.5;

const PREDICTION_RESYNC_YALMS: f32 = 5.0;

// Retail body turn into a new camera-relative run direction takes ~0.5-0.7s
// for 90° (HorizonXI video 2026-07-20, D-press frames). The carve rate of a
// held A/D is then paced by the lazy camera follow (AUTO_RECENTER_RATE), not
// by this lerp.
const HEADING_LERP_RATE_RAD_PER_SEC: f32 = 2.5;

// S from a forward-facing stance is an instant about-face in retail
// (HorizonXI video 2026-07-20), not a carved arc; turns sharper than this
// snap instead of lerping.
const ABOUT_FACE_SNAP_RAD: f32 = 2.0;

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

pub fn reset_interaction_flags_on_zone_change(
    state: Res<SceneState>,
    mut prev_zone: Local<Option<Option<u16>>>,
    mut autorun: ResMut<AutoRun>,
    mut lock_on: ResMut<LockOn>,
    mut target: ResMut<Target>,
    mut rest: ResMut<kuluu_render::combat_stance::RestStance>,
    mut chase: ResMut<ChaseCamera>,
) {
    let zone = state.snapshot.zone_id;
    let changed = matches!(*prev_zone, Some(p) if p != zone);
    *prev_zone = Some(zone);
    if !changed {
        return;
    }
    *autorun = AutoRun::default();
    lock_on.target_id = None;
    target.id = None;
    *rest = kuluu_render::combat_stance::RestStance::default();
    // Swing the camera behind the character's new facing on every zone-in,
    // in both chase and first person (retail resets the view to look ahead).
    // Snap rather than smooth: the player teleported, so a lerp would smear
    // the eye across the two zones' coordinates.
    chase.yaw = kuluu_render::yaw_for_heading(state.snapshot.self_pos.heading);
    chase.snap_to_anchor = true;
}

pub fn advance_heading_turn(
    accum_units: &mut f32,
    rate_rad_per_sec: f32,
    dt_secs: f32,
) -> (i32, f32) {
    let float_delta = rate_rad_per_sec * (256.0 / std::f32::consts::TAU) * dt_secs;
    if rate_rad_per_sec == 0.0 {
        *accum_units = 0.0;
        return (0, 0.0);
    }
    *accum_units += float_delta;
    let whole = accum_units.trunc();
    *accum_units -= whole;
    (whole as i32, float_delta)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedMoveInputs {
    pub forward: i32,
    pub strafe: i32,
    pub steer: i32,
    pub rotate_dir: i32,
}

/// Retail 3rd-person movement is camera-relative (HorizonXI video, 2026-07-20):
/// W/A/S/D pick a run direction in the camera frame and the character turns
/// into it at full speed — S runs toward the camera, A/D never rotate in place
/// (that's Q/E, and A/D only in first person), and there is no unlocked
/// backpedal. Locked on, the character faces the target: A/D strafe and S
/// backpedals instead.
#[allow(clippy::too_many_arguments)]
pub fn resolve_move_inputs(
    forward_held: bool,
    backward_held: bool,
    turn_left: bool,
    turn_right: bool,
    strafe_left: bool,
    strafe_right: bool,
    rotate_left: bool,
    rotate_right: bool,
    autorun_forward: bool,
    locked: bool,
) -> ResolvedMoveInputs {
    let mut forward = i32::from(forward_held) - i32::from(backward_held);
    if autorun_forward {
        forward = forward.max(1);
    }
    let mut strafe = i32::from(strafe_right) - i32::from(strafe_left);
    let rotate_dir = i32::from(rotate_right) - i32::from(rotate_left);
    let mut steer = 0;
    let turn = i32::from(turn_right) - i32::from(turn_left);
    if locked {
        strafe = (strafe + turn).clamp(-1, 1);
    } else {
        steer = turn;
    }
    ResolvedMoveInputs {
        forward,
        strafe,
        steer,
        rotate_dir,
    }
}

/// World-space run heading for a camera-relative move: `forward` along the
/// camera's forward axis, `steer` along camera-right. Components are analog
/// (a stick preserves its direction ratio; keyboard passes -1/0/1). Callers
/// guarantee at least one component is non-zero (steer_in_chase requires it).
pub fn camera_relative_motion_heading(camera_forward_h: u8, forward: f32, steer: f32) -> u8 {
    let (cf_x, cf_y) = heading_to_forward(camera_forward_h);
    let (cr_x, cr_y) = heading_to_forward(camera_forward_h.wrapping_add(64));
    let mx = cf_x * forward + cr_x * steer;
    let my = cf_y * forward + cr_y * steer;
    let motion_radians = my.atan2(mx);
    let motion_raw = motion_radians * -(128.0 / std::f32::consts::PI);
    (motion_raw.round() as i32).rem_euclid(256) as u8
}

/// Retail resolves pad-vs-keyboard analog input by magnitude: whichever
/// source deflects further wins, ties to the keyboard
/// (research/XIClient InputManager::GetAnalogKey).
pub fn pick_mag(keyboard: f32, pad: f32) -> f32 {
    if pad.abs() > keyboard.abs() {
        pad
    } else {
        keyboard
    }
}

/// [`pick_mag`] quantized for the digital movement paths (locked-on strafe /
/// backpedal and first-person forward), which step at full speed per
/// direction like held keys.
pub fn merge_dir(keyboard: i32, pad: f32) -> i32 {
    if pad.abs() > keyboard.abs() as f32 {
        if pad > 0.0 {
            1
        } else {
            -1
        }
    } else {
        keyboard
    }
}

/// Modes that plant the character: an NPC dialog or a shop/box/counter screen
/// owns them until it closes. `Chat` is deliberately absent -- retail keeps
/// auto-run going while the input line is focused.
pub fn mode_cancels_autorun(mode: &InputMode) -> bool {
    matches!(
        mode,
        InputMode::Dialog(_)
            | InputMode::DeliveryBox
            | InputMode::Check
            | InputMode::Bazaar
            | InputMode::Auction
    )
}

/// Modes whose keystrokes belong to a text buffer, not to movement or camera.
/// Auto-run survives on `phantom_forward` alone.
pub fn mode_swallows_keys(mode: &InputMode) -> bool {
    matches!(mode, InputMode::Chat(_))
}

pub fn autorun_after_toggle(phantom_forward: bool, toggle_just_pressed: bool) -> bool {
    if toggle_just_pressed {
        !phantom_forward
    } else {
        phantom_forward
    }
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
    input_src: KeyActionSources,
    mut window_close: MessageReader<WindowCloseRequested>,
    mut state: ResMut<SceneState>,
    cmd_tx: Res<CommandTx>,
    mut mode: ResMut<InputMode>,
    mut target: ResMut<Target>,
    mut autorun: ResMut<AutoRun>,
    mut camera: CameraInputParams,
    cam_q: Query<(&Camera, &Transform), With<OperatorCamera>>,
    mut exit: MessageWriter<AppExit>,
    mut rest_stance: ResMut<kuluu_render::combat_stance::RestStance>,
    mut walk_mode: ResMut<kuluu_render::combat_stance::WalkMode>,
    mut tab_stack: ResMut<TabCycleStack>,
    select_target: Res<SelectTargetMode>,
    mut hud_capture: HudCaptureParams,
) {
    let camera_mode = &mut camera.mode;
    let chase = &mut camera.chase;
    let cursor_lock = &mut camera.cursor_lock;
    let lock_on = &mut camera.lock_on;
    let camera_transition = &mut camera.transition;

    let keys = &input_src.keys;
    let bindings = &input_src.bindings;
    // Keyboard and pad are merged per action: the pad dispatches `Action`s
    // directly (gamepad_input::PadPressed) rather than pulsing synthetic keys.
    let just = |a: Action| bindings.just_pressed(a, keys) || input_src.pad.just_pressed(a);

    let cmd_held = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
    let close_shortcut =
        cmd_held && (keys.just_pressed(KeyCode::KeyQ) || keys.just_pressed(KeyCode::KeyW));
    let os_close = window_close.read().next().is_some();
    if close_shortcut || os_close {
        let _ = cmd_tx.0.try_send(AgentCommand::Disconnect);
        exit.write_default();
        return;
    }

    if !matches!(*mode, InputMode::Chat(_)) && just(Action::ToggleFirstPerson) {
        chase.yaw = kuluu_render::yaw_for_heading(state.snapshot.self_pos.heading);
        camera_transition.begin(**camera_mode, chase.distance);
        cursor_lock.locked = false;
    }

    if !matches!(*mode, InputMode::Chat(_)) {
        if just(Action::ToggleHud) {
            hud_capture.hud_hidden.manual = !hud_capture.hud_hidden.manual;
        }
        if just(Action::Screenshot) {
            hud_capture
                .screenshot
                .write(super::screenshot::ScreenshotRequest {
                    path: super::screenshot::next_default_path(),
                });
        }
    }

    if just(Action::TogglePassiveCursor) {
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

    // Fishing inputs are modal: while a cast is live they take priority over
    // chat/menu/targeting so Enter sets the hook instead of acquiring a target
    // (retail consumes these keys for the mini-game while the rod is out).
    if state.snapshot.self_fishing.is_some() {
        let fishing_input = if just(Action::FishingHook) {
            Some(FishingInput::Hook)
        } else if just(Action::FishingReelLeft) {
            Some(FishingInput::Left)
        } else if just(Action::FishingReelRight) {
            Some(FishingInput::Right)
        } else if just(Action::FishingCancel) {
            Some(FishingInput::Cancel)
        } else {
            None
        };
        if let Some(input) = fishing_input {
            let _ = cmd_tx.0.try_send(AgentCommand::FishingInput { input });
            return;
        }
    }

    if bindings.just_pressed(Action::OpenChatCommand, keys) {
        *mode = InputMode::Chat(ChatBuffer::empty());
        return;
    }
    if just(Action::OpenMenu) {
        *mode = InputMode::Menu(MenuStack::root());
        return;
    }

    // The engaged "Switch Target" flow is retail's sanctioned mid-fight
    // re-target, so it plays the sub-target role here and reaches through the
    // lock; every other targeting key below is pinned by it.
    let target_pinned = kuluu_render::suppresses_retarget(lock_on, select_target.active);

    if !select_target.active && !target_pinned && just(Action::ClearTarget) {
        target.id = None;
    }

    let tab = just(Action::CycleTarget);

    let enter_acquire = just(Action::ConfirmAction)
        && target.id.is_none()
        && !kuluu_render::hud::death_prompt::is_dead(&state);
    if (tab || enter_acquire) && !target_pinned {
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

    let party_slot = if bindings.just_pressed(Action::TargetSelf, keys) {
        Some(1)
    } else if bindings.just_pressed(Action::TargetParty2, keys) {
        Some(2)
    } else if bindings.just_pressed(Action::TargetParty3, keys) {
        Some(3)
    } else if bindings.just_pressed(Action::TargetParty4, keys) {
        Some(4)
    } else if bindings.just_pressed(Action::TargetParty5, keys) {
        Some(5)
    } else if bindings.just_pressed(Action::TargetParty6, keys) {
        Some(6)
    } else {
        None
    };
    if let Some(slot) = party_slot.filter(|_| !target_pinned) {
        let id = if slot == 1 {
            state.snapshot.self_char_id
        } else {
            state.snapshot.party.get((slot - 1) as usize).map(|p| p.id)
        };
        if let Some(id) = id {
            target.id = Some(id);
        }
    }
    autorun.phantom_forward =
        autorun_after_toggle(autorun.phantom_forward, just(Action::ToggleAutorun));
    if bindings.just_pressed(Action::ToggleWalk, keys) {
        walk_mode.walking = !walk_mode.walking;
    }
    // Retail's "Select active window" action toggles lock-on / focuses the
    // active window; it never engages. The old engage/disengage toggle that
    // lived on this action pre-rename has been removed — engaging goes through
    // the Attack action menu entry.

    if bindings.just_pressed(Action::Sit, keys) {
        use kuluu_render::combat_stance::RestKind;
        let next = match rest_stance.kind {
            RestKind::Sit => RestKind::None,

            RestKind::Heal => {
                let _ = cmd_tx.0.try_send(AgentCommand::Heal {
                    mode: kuluu_session::state::HealMode::Off,
                });
                RestKind::None
            }
            RestKind::None => RestKind::Sit,
        };
        rest_stance.kind = next;
    }
    if bindings.just_pressed(Action::Heal, keys) {
        toggle_heal(&mut rest_stance, &cmd_tx);
    }

    if just(Action::ToggleLockOn) {
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
                Some(format!("lock-on: {name}"))
            }
            LockOnToggle::Cleared => Some("lock-on cleared".into()),
            // Retail's lock key is contextual: with nothing targeted (and no
            // lock to release) it toggles resting instead
            // (research/xim MainTool.kt::handleKeyEvents).
            LockOnToggle::NoTarget => {
                toggle_heal(&mut rest_stance, &cmd_tx);
                None
            }
        };
        if let Some(text) = toast {
            state.push_local_toast(kuluu_snapshot::ChatLine {
                spans: Vec::new(),
                channel: kuluu_snapshot::ChatChannel::Debug,
                sender: "client".into(),
                text,
                server_ts: 0,
                local_seq: 0,
            });
        }
    }

    if let Some(id) = lock_on.target_id {
        let still_visible = state.snapshot.entities.iter().any(|e| e.id == id);
        // A lock with no main target under it would pin targeting against a
        // target that isn't there, so a /target that drops the selection
        // releases the lock with it.
        if !still_visible || target.id.is_none() {
            lock_on.target_id = None;
        }
    }
}

fn toggle_heal(rest_stance: &mut kuluu_render::combat_stance::RestStance, cmd_tx: &CommandTx) {
    use kuluu_render::combat_stance::RestKind;
    let (next_kind, wire_mode) = match rest_stance.kind {
        RestKind::Heal => (RestKind::None, kuluu_session::state::HealMode::Off),

        _ => (RestKind::Heal, kuluu_session::state::HealMode::On),
    };
    let _ = cmd_tx.0.try_send(AgentCommand::Heal { mode: wire_mode });
    rest_stance.kind = next_kind;
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

/// Mirror the viewer's lock-on state into the reactor so it only squares the
/// engaged target up while locked. Without this the reactor's per-tick facing
/// snaps the player back toward the mob every 200ms even after the human
/// unlocks (kuluu-j03o).
pub fn sync_target_lock_system(
    lock_on: Res<LockOn>,
    cmd_tx: Res<CommandTx>,
    mut last_sent: Local<Option<bool>>,
) {
    let locked = lock_on.is_active();
    if *last_sent == Some(locked) {
        return;
    }
    if cmd_tx
        .0
        .try_send(AgentCommand::SetTargetLock { locked })
        .is_ok()
    {
        *last_sent = Some(locked);
    }
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
    // Bundled per-tick locals (steer_latch + pad_edges + walker state) so this
    // fn stays under bevy's 16-param SystemParam ceiling. See `DispatchLocals`
    // for the field-level docs the individual `Local`s used to carry.
    mut locals: Local<DispatchLocals>,
    mut prediction: ResMut<LocalPlayerPrediction>,
    env: MoveEnvParams,
    mut stance: StanceParams,
    // Ramp-field debug record (plan §4 step 2): the gizmo and snapshot systems
    // read what this tick's walker::step saw. At bevy's 16-param ceiling; a new
    // param here must bundle into an existing SystemParam struct.
    mut field_dbg: ResMut<super::walker::debug::FieldDebug>,
) {
    let rest_stance = &mut stance.rest_stance;
    let walk_mode = &stance.walk_mode;
    let move_intent = &mut stance.move_intent;
    // Default to stopped so every early return below reports no movement.
    **move_intent = kuluu_render::combat_stance::SelfMoveIntent::default();

    if mode_cancels_autorun(&mode) {
        autorun.phantom_forward = false;
        autorun.strafe_held_since = None;
        return;
    }

    let no_keys = ButtonInput::<KeyCode>::default();
    let keys: &ButtonInput<KeyCode> = if mode_swallows_keys(&mode) {
        &no_keys
    } else {
        &keys
    };

    let in_picker = matches!(
        *mode,
        InputMode::Menu(_)
            | InputMode::QuickAction(_)
            | InputMode::TargetAction(_)
            | InputMode::PassiveCursor(_)
    );

    // Pad sticks stay live where keyboard is muted or repurposed: retail keeps
    // the pad moving the character while the chat line has focus and while a
    // menu is open (menus are the d-pad's domain, not the sticks').
    let pad_move = env.pad.movement;
    let pad_cam = env.pad.camera;
    let pad_move_started = pad_move != Vec2::ZERO && !locals.pad_edges.move_active;
    locals.pad_edges.move_active = pad_move != Vec2::ZERO;
    let pad_back = pad_move.y < -PAD_BACK_CANCEL_DEFLECTION;
    let pad_back_started = pad_back && !locals.pad_edges.back_active;
    locals.pad_edges.back_active = pad_back;

    let mut pitch_d = 0.0;
    if !in_picker && bindings.pressed(Action::CameraPitchUp, keys) {
        pitch_d += PITCH_STEP_HELD;
    }
    if !in_picker && bindings.pressed(Action::CameraPitchDown, keys) {
        pitch_d -= PITCH_STEP_HELD;
    }
    pitch_d += pad_cam.y * PITCH_STEP_HELD;
    if pitch_d != 0.0 {
        let (lo, hi) = match *camera_mode {
            CameraMode::Chase => (ChaseCamera::PITCH_MIN, ChaseCamera::PITCH_MAX),
            CameraMode::FirstPerson => (ChaseCamera::FP_PITCH_MIN, ChaseCamera::FP_PITCH_MAX),
        };
        chase.pitch = (chase.pitch + pitch_d).clamp(lo, hi);
    }

    let mut yaw_d = 0.0;
    let yaw_step = CAMERA_YAW_RATE * time.delta_secs();
    if !in_picker && bindings.pressed(Action::CameraYawLeft, keys) {
        yaw_d -= yaw_step;
    }
    if !in_picker && bindings.pressed(Action::CameraYawRight, keys) {
        yaw_d += yaw_step;
    }
    yaw_d += pad_cam.x * yaw_step;
    if yaw_d != 0.0 {
        chase.yaw += yaw_d;
    }

    if matches!(*camera_mode, CameraMode::Chase) && !in_picker && !env.minimap_hover.hovered {
        let mut zoom_d = 0.0;
        let step = ChaseCamera::KEYBOARD_ZOOM_RATE * time.delta_secs();
        if bindings.pressed(Action::CameraZoomIn, keys) {
            zoom_d -= step;
        }
        if bindings.pressed(Action::CameraZoomOut, keys) {
            zoom_d += step;
        }
        // PgUp/PgDn drive the same chase zoom: Action::PageUp/PageDown are bound to those
        // keys in every preset and were previously unconsumed.
        if bindings.pressed(Action::PageUp, keys) {
            zoom_d -= step;
        }
        if bindings.pressed(Action::PageDown, keys) {
            zoom_d += step;
        }
        if zoom_d != 0.0 {
            chase.distance =
                (chase.distance + zoom_d).clamp(ChaseCamera::DIST_MIN, ChaseCamera::DIST_MAX);
        }
    }

    if kuluu_render::hud::death_prompt::is_dead(&state) {
        autorun.phantom_forward = false;
        autorun.strafe_held_since = None;
        return;
    }

    if rest_stance.is_resting() {
        use kuluu_render::combat_stance::RestKind;
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
        let pressed_move =
            move_actions.iter().any(|a| bindings.just_pressed(*a, keys)) || pad_move_started;
        if pressed_move {
            if matches!(rest_stance.kind, RestKind::Heal) {
                let _ = cmd_tx.0.try_send(AgentCommand::Heal {
                    mode: kuluu_session::state::HealMode::Off,
                });
            }
            rest_stance.begin_exit();
        }
        autorun.phantom_forward = false;
        autorun.strafe_held_since = None;
        return;
    }

    // The stand-up clip runs before the character moves (retail's cost for
    // breaking a rest); movement only starts if the keys are still held when it
    // ends, so this gate reads `pressed` state fresh on the frame it lifts.
    if rest_stance.exit_blocks_movement(time.delta_secs()) {
        autorun.phantom_forward = false;
        autorun.strafe_held_since = None;
        return;
    }

    let backward_just_pressed =
        bindings.just_pressed(Action::MoveBackward, keys) || pad_back_started;
    if backward_just_pressed {
        autorun.phantom_forward = false;
    }

    // Retail autorun is steerable: A/D carve the run without cancelling it.
    // Held strafe or Q/E rotate cancels after a short grace.
    let any_strafe = bindings.pressed(Action::StrafeLeft, keys)
        || bindings.pressed(Action::StrafeRight, keys)
        || bindings.pressed(Action::RotateLeft, keys)
        || bindings.pressed(Action::RotateRight, keys)
        || pad_move.x != 0.0;
    if any_strafe {
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

    let locked = lock_on.target_id.is_some();
    let first_person = matches!(*camera_mode, CameraMode::FirstPerson);

    let mut resolved = resolve_move_inputs(
        bindings.pressed(Action::MoveForward, keys),
        bindings.pressed(Action::MoveBackward, keys),
        bindings.pressed(Action::TurnLeft, keys),
        bindings.pressed(Action::TurnRight, keys),
        bindings.pressed(Action::StrafeLeft, keys),
        bindings.pressed(Action::StrafeRight, keys),
        bindings.pressed(Action::RotateLeft, keys),
        bindings.pressed(Action::RotateRight, keys),
        autorun.phantom_forward,
        locked,
    );
    let mut forward = resolved.forward;
    let mut strafe = resolved.strafe;
    // Focus-less GUI driving (kuluu-0pof): a socket `debug_drive` overrides the
    // key-derived axes, so a remote driver walks the real input path (heading,
    // wall-slide, re-ground) exactly as WASD would.
    if let Some(handle) = env.debug_ctrl.as_ref() {
        if let Ok(ctrl) = handle.0.lock() {
            if let Some((f, s)) = ctrl.active_drive(std::time::Instant::now()) {
                forward = f;
                strafe = s;
            }
        }
    }
    // Stair-capture drive channel (FFXI_STAIR_DRIVE): remote holds fold into the
    // real input pipeline exactly like held WASD/Q/E keys — steer-latch, heading
    // carve and re-ground all see them as normal movement. The one-shot `w` warp
    // is applied to chase.yaw in the yaw section below (exact aim, no timed pan).
    let drive_axes = env
        .stair_drive
        .as_ref()
        .and_then(|h| h.0.lock().ok())
        .and_then(|d| d.active());
    let drive_c = drive_axes.map(|a| a.3).unwrap_or(0);
    if let Some((df, ds, dt, _dc)) = drive_axes {
        forward = df;
        strafe = ds;
        resolved.rotate_dir += dt;
    }
    // Pad-vs-keyboard analog resolution is retail's larger-magnitude rule
    // (pick_mag). `pf`/`ps` keep the stick's direction ratio for the
    // camera-relative run; the locked and first-person paths step digitally,
    // so the stick quantizes to a direction there (merge_dir).
    let pf = pick_mag(forward as f32, pad_move.y);
    let ps = if first_person || locked {
        0.0
    } else {
        pick_mag(resolved.steer as f32, pad_move.x)
    };
    if locked {
        forward = merge_dir(forward, pad_move.y);
        strafe = merge_dir(strafe, pad_move.x);
    } else if first_person {
        forward = merge_dir(forward, pad_move.y);
    }
    // In chase mode steer is always a camera-relative run component (solo A/D
    // runs sideways); only first person keeps the arrow-turn pivot.
    // In first person A/D (and the stick's x axis) rotate the view like Q/E.
    let fp_rotate = if first_person {
        pick_mag(resolved.steer as f32, pad_move.x)
    } else {
        0.0
    };
    let turn_rate = ROTATE_KEY_RATE_RAD_PER_SEC * (resolved.rotate_dir as f32 + fp_rotate);
    let (player_rotate_u8, heading_delta_units) =
        advance_heading_turn(&mut turn_accum.units, turn_rate, time.delta_secs());
    let steer_in_chase = !first_person && !locked && (pf != 0.0 || ps != 0.0);
    // Deliberate camera pan (yaw keys / mouse drag) re-aims a pure W/S run;
    // the latch only holds the run direction against the passive
    // auto-recenter, not against the player actively steering the camera.
    let camera_panning = bindings.pressed(Action::CameraYawLeft, keys)
        || bindings.pressed(Action::CameraYawRight, keys)
        || pad_cam.x != 0.0
        || env.pointer.left
        || env.pointer.right
        || drive_c != 0;
    // A/D carve, Q/E rotate, and camera panning recompute the run direction
    // against the live camera every frame; anything else holds the latch.
    if !steer_in_chase || ps != 0.0 || resolved.rotate_dir != 0 || camera_panning {
        locals.steer_latch = None;
    }

    let self_pos = state.snapshot.self_pos;

    // The one speed variable (yalms/s): paces the horizontal step and every
    // vertical move inside the step band (walk mode merges slower than run).
    let speed_yps =
        move_speed_yps(self_pos.speed, state.snapshot.self_mount.is_some()) * walk_mode.scale();

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
                let stop = kuluu_session::state::MODEL_RADIUS_PC
                    + radius_for_wire_kind(ent.kind)
                    + kuluu_session::state::CONTACT_GAP;
                forward_allowance((basis_pos.x, basis_pos.y), (ent.pos.x, ent.pos.y), stop)
            })
    });

    // First person: the view IS the facing, so rotation (Q/E and A/D alike)
    // moves the camera rigidly and forward motion follows the view (mouse-look
    // included). In chase mode the camera instead trails via auto-recenter.
    if player_rotate_u8 != 0 && first_person {
        chase.yaw -= heading_delta_units * std::f32::consts::TAU / 256.0;
    }

    // Stair-capture drive camera axes: remote pan at the key yaw rate, plus a
    // one-shot exact warp (the "cheat": snap instead of timed presses fighting
    // latency). Runs before the idle early-return so aiming works while stopped.
    if drive_c != 0 {
        chase.yaw += drive_c as f32 * CAMERA_YAW_RATE * time.delta_secs();
    }
    if let Some(handle) = env.stair_drive.as_ref() {
        if let Ok(mut d) = handle.0.lock() {
            if let Some(target) = d.take_warp() {
                chase.yaw += wrap_signed_pi(target - chase.yaw);
            }
        }
    }

    if forward == 0 && strafe == 0 && player_rotate_u8 == 0 && !steer_in_chase {
        if let Some(h) = locked_heading {
            if h != self_pos.heading {
                chase.yaw = kuluu_render::yaw_for_heading(h);

                let _ = cmd_tx.0.try_send(AgentCommand::Move {
                    x: basis_pos.x,
                    y: basis_pos.y,
                    z: basis_pos.z,
                    heading: h,
                });
            }
        }
        // Idle tick: no horizontal move, but the walker still runs its vertical
        // step (settle onto the floor under the feet). Send a Move only when z
        // actually changed — session emits POS on its own 100 ms cadence anyway.
        let res = super::walker::step(
            &env.collision,
            &env.obstacles,
            &mut locals.walker,
            basis_pos.x,
            basis_pos.y,
            basis_pos.z,
            0.0,
            0.0,
            speed_yps,
            time.delta_secs(),
            env.hud_panels.noclip,
        );
        super::walker::debug::record_tick(
            &mut field_dbg,
            &env.collision,
            basis_pos.x,
            basis_pos.y,
            res.feet_z,
            self_pos.heading,
            speed_yps,
            &res,
        );
        if (res.feet_z - basis_pos.z).abs() > 1e-3 {
            let _ = cmd_tx.0.try_send(AgentCommand::Move {
                x: basis_pos.x,
                y: basis_pos.y,
                z: res.feet_z,
                heading: self_pos.heading,
            });
        }
        prediction.pos = Vec3::new(basis_pos.x, basis_pos.y, res.feet_z);
        return;
    }

    let was_moving = move_intent.moving;
    let moving = forward != 0 || strafe != 0 || steer_in_chase;
    let (intent_forward, intent_strafe) = if locked {
        (forward as f32, strafe as f32)
    } else if moving {
        (1.0, 0.0)
    } else {
        (0.0, 0.0)
    };
    **move_intent = kuluu_render::combat_stance::SelfMoveIntent {
        moving,
        forward: intent_forward,
        strafe: intent_strafe,
    };

    let mut heading = self_pos.heading;
    if player_rotate_u8 != 0 {
        let delta = player_rotate_u8.rem_euclid(256) as u8;
        heading = heading.wrapping_add(delta);
    }
    if forward != 0 && first_person {
        heading = heading_for_yaw(chase.yaw);
    }

    let raw_step = speed_yps * time.delta_secs();

    let mut turn_dx: f32 = 0.0;
    let mut turn_dy: f32 = 0.0;
    if steer_in_chase {
        let camera_forward_h = heading_for_yaw(chase.yaw);
        let continuous = ps != 0.0 || resolved.rotate_dir != 0 || camera_panning;
        let motion_h = if continuous {
            camera_relative_motion_heading(camera_forward_h, pf, ps)
        } else {
            let pf_sign = if pf > 0.0 { 1 } else { -1 };
            match locals.steer_latch {
                Some((f, h)) if f == pf_sign => h,
                _ => {
                    let h = camera_relative_motion_heading(camera_forward_h, pf_sign as f32, 0.0);
                    locals.steer_latch = Some((pf_sign, h));
                    h
                }
            }
        };

        if raw_step > 0.0 {
            let h_target = yaw_for_heading(motion_h);
            let h_current = yaw_for_heading(heading);
            let h_diff = wrap_signed_pi(h_target - h_current);

            // From standstill the model faces the run direction on the first
            // step (HorizonXI video 2026-07-20); the carve lerp only applies
            // to direction changes while already running.
            heading = if !was_moving || h_diff.abs() >= ABOUT_FACE_SNAP_RAD {
                motion_h
            } else {
                let h_alpha = 1.0 - (-HEADING_LERP_RATE_RAD_PER_SEC * time.delta_secs()).exp();
                heading_for_yaw(h_current + h_diff * h_alpha)
            };

            // Translate along the body's current (lerped) heading, not the
            // target run direction. Retail velocity is always body-aligned:
            // a direction change carves an arc as the model turns. Stepping
            // along motion_h while heading still lerps decouples facing from
            // travel and reads as ice-skating.
            let (mv_x, mv_y) = heading_to_forward(heading);
            turn_dx = mv_x * raw_step;
            turn_dy = mv_y * raw_step;
        }

        // Camera follow while carving is camera_polish_system's auto-recenter
        // (the single camera-follow authority); adding a second tug here would
        // tighten the carve circle below the retail-observed rate.
        forward = 0;
        strafe = 0;
    }

    if let Some(h) = locked_heading {
        heading = h;
        chase.yaw = kuluu_render::yaw_for_heading(h);
    }

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

    // The walker owns this tick's horizontal clamp and vertical authority
    // (plan §2.3/§2.4): wall sweep + slide, then the mode-driven vertical step
    // (MZB collision is in Bevy space, bevy.x = ffxi.x, bevy.z = -ffxi.y,
    // bevy.y = -ffxi.z). No floor within one step of reach means airborne;
    // a PERSISTENT wedge (a column with floors but none within
    // MAX_GROUND_STEP_UP) is broken by `recover_self_ground_system`, which runs
    // right after this one — the server never corrects a bad z, it persists and
    // echoes back whatever c2s 0x015 sends (kuluu-mo4q).
    let wall_dx = x - basis_pos.x;
    let wall_dy = y - basis_pos.y;
    let res = super::walker::step(
        &env.collision,
        &env.obstacles,
        &mut locals.walker,
        basis_pos.x,
        basis_pos.y,
        basis_pos.z,
        wall_dx,
        wall_dy,
        speed_yps,
        time.delta_secs(),
        env.hud_panels.noclip,
    );

    let final_x = basis_pos.x + res.dx;
    let final_y = basis_pos.y + res.dy;
    let final_z = res.feet_z;

    super::walker::debug::record_tick(
        &mut field_dbg,
        &env.collision,
        final_x,
        final_y,
        final_z,
        heading,
        speed_yps,
        &res,
    );

    let _ = cmd_tx.0.try_send(AgentCommand::Move {
        x: final_x,
        y: final_y,
        z: final_z,
        heading,
    });

    prediction.pos = Vec3::new(final_x, final_y, final_z);
}

/// How long the player must be under every floor in their column before the
/// wedge recovery fires. A stray floorless column is crossed in a tick or two at
/// run speed; a wedge lasts forever, so the delay separates them and keeps a
/// residual collision hole from launching the player onto a roof the way
/// kuluu-0nnl did.
const UNDER_FLOOR_RECOVERY_SECS: f32 = 0.5;

#[derive(Debug, Clone, Copy)]
struct GroundRecoveryCandidate {
    zone_id: u16,
    self_id: u32,
    reported_pos: Vec3,
    recovered_z: f32,
}

impl GroundRecoveryCandidate {
    fn matches(self, other: Self) -> bool {
        self.zone_id == other.zone_id
            && self.self_id == other.self_id
            && ground_correction_matches(
                self.reported_pos.x,
                self.reported_pos.y,
                other.reported_pos.x,
                other.reported_pos.y,
            )
            && (self.reported_pos.z - other.reported_pos.z).abs()
                <= GROUND_CORRECTION_XY_EPSILON_YALMS
            && (self.recovered_z - other.recovered_z).abs() <= GROUND_CORRECTION_XY_EPSILON_YALMS
    }
}

#[derive(Default)]
pub(crate) struct GroundRecoveryTracker {
    candidate: Option<GroundRecoveryCandidate>,
    stable_secs: f32,
    queued: bool,
}

impl GroundRecoveryTracker {
    fn observe(&mut self, candidate: Option<GroundRecoveryCandidate>, dt: f32) -> bool {
        let Some(candidate) = candidate else {
            *self = Self::default();
            return false;
        };
        if !self.candidate.is_some_and(|prior| prior.matches(candidate)) {
            self.candidate = Some(candidate);
            self.stable_secs = 0.0;
            self.queued = false;
        }
        self.stable_secs += dt;
        !self.queued && self.stable_secs >= UNDER_FLOOR_RECOVERY_SECS
    }

    fn mark_queued(&mut self) {
        self.queued = true;
    }
}

/// Breaks the wire-z wedge (kuluu-mo4q). `dispatch_movement_system` holds height
/// whenever `ground_step` finds no floor within reach. LSB ordinarily accepts
/// all three client coordinates without terrain validation; forced-position
/// and charm states are the exceptions
/// (`vendor/server/src/map/packets/c2s/0x015_pos.cpp`).
/// Being under every floor in the column is unreachable by walking, so it is
/// always a wedge and always safe to recover upward.
///
/// `pub(crate)` rather than `pub`: view_native is library-public now (the
/// walker's headless examples), so a bare `pub` would expose the crate-private
/// GroundRecoveryTracker through this signature.
pub(crate) fn recover_self_ground_system(
    time: Res<Time<Fixed>>,
    state: Res<SceneState>,
    cmd_tx: Res<CommandTx>,
    collision: Res<kuluu_render::dat_mzb::MzbCollisionGeometry>,
    mzb_in_flight: Res<kuluu_render::dat_mzb::LoadMzbInFlight>,
    mut tracker: Local<GroundRecoveryTracker>,
) {
    let self_pos = state.snapshot.self_pos;
    let self_id = state.snapshot.self_char_id.filter(|id| {
        state
            .snapshot
            .entities
            .iter()
            .any(|entity| entity.id == *id)
    });
    let reported_pos = Vec3::new(self_pos.pos.x, self_pos.pos.y, self_pos.pos.z);
    let candidate = ground_recovery_candidate(
        &collision,
        &mzb_in_flight,
        state.snapshot.zone_id,
        self_id,
        reported_pos,
    );
    if !tracker.observe(candidate, time.delta_secs()) {
        return;
    }
    let Some(candidate) = candidate else {
        return;
    };
    let cmd = ground_recovery_command(
        candidate.zone_id,
        candidate.self_id,
        candidate.reported_pos.x,
        candidate.reported_pos.y,
        candidate.recovered_z,
        self_pos.heading,
    );
    if cmd_tx.0.try_send(cmd).is_ok() {
        tracker.mark_queued();
    }
}

/// The corrective command [`recover_self_ground_system`] emits. It is
/// deliberately not an [`AgentCommand::Move`]: the reactor treats a Move as
/// player intent and would cancel a Following/Pathing goal for it, or drop it
/// outright under a forced-move override (kuluu-mo4q).
pub fn ground_recovery_command(
    zone_id: u16,
    self_id: u32,
    x: f32,
    y: f32,
    z: f32,
    heading: u8,
) -> AgentCommand {
    AgentCommand::GroundCorrection {
        zone_id,
        self_id,
        x,
        y,
        z,
        heading,
    }
}

/// Inert while a zone/interior load is in flight: the "under every floor is
/// unreachable by walking" argument holds only over a *complete* collision set,
/// and `sub_area_activation` swaps an interior in behind the exterior shell
/// asynchronously — mid-swap an indoor player's column holds only the shell
/// above them, and recovering onto it is the kuluu-0nnl roof snap. The load
/// outlasts [`UNDER_FLOOR_RECOVERY_SECS`], so the debounce alone cannot cover
/// this.
fn ground_recovery_candidate(
    collision: &kuluu_render::dat_mzb::MzbCollisionGeometry,
    mzb_in_flight: &kuluu_render::dat_mzb::LoadMzbInFlight,
    zone_id: Option<u16>,
    self_id: Option<u32>,
    pos: Vec3,
) -> Option<GroundRecoveryCandidate> {
    let zone_id = zone_id?;
    let self_id = self_id?;
    if mzb_in_flight.any_pending() {
        return None;
    }
    let column = bevy::math::Vec2::new(pos.x, -pos.y);
    let feet_y = -pos.z;

    if collision
        .ground_step(column, feet_y, kuluu_render::dat_mzb::MAX_GROUND_STEP_UP)
        .is_some()
    {
        return None;
    }
    let recovered_z = collision.ground_or_recover_wire_z(pos.x, pos.y, pos.z)?;
    if (recovered_z - pos.z).abs() <= f32::EPSILON {
        return None;
    }
    Some(GroundRecoveryCandidate {
        zone_id,
        self_id,
        reported_pos: pos,
        recovered_z,
    })
}

/// Publish the tick's authoritative render position to the interpolation
/// buffer. Runs in FixedUpdate right after `dispatch_movement_system` so the
/// rendered player follows the walker deterministically at 60 Hz;
/// interpolate_self_transform_system (RunFixedMainLoop) lerps
/// Transform.translation between prev and curr every render frame, so the
/// chase camera sees smooth motion instead of stair-stepped 60Hz updates.
/// Render Y == wire Y: no vertical smoothing here — the walker's stop settle
/// owns the "don't dip when we stop mid-step" job.
pub fn apply_self_prediction_system(
    prediction: Res<LocalPlayerPrediction>,
    mut q_self: Query<
        (
            &mut kuluu_render::PrevRenderPos,
            &mut kuluu_render::CurrRenderPos,
        ),
        (With<IsSelf>, Without<OperatorCamera>),
    >,
) {
    if !prediction.initialized {
        return;
    }
    let Ok((mut prev, mut curr)) = q_self.single_mut() else {
        return;
    };
    // prediction.pos is in wire (ffxi) space; convert to Bevy for the Transform.
    let wire = kuluu_snapshot::Vec3 {
        x: prediction.pos.x,
        y: prediction.pos.y,
        z: prediction.pos.z,
    };
    // Preserve rotation — self_visual_yaw_system owns it.
    let target = kuluu_render::ffxi_to_bevy(wire);

    // Uninitialized state: ensure_self_render_pos_system attaches PrevRenderPos
    // + CurrRenderPos seeded from the spawn Transform, but the spawn Transform
    // may still be the placeholder ZERO if this is the frame before scene sync.
    // Detect that (both exactly ZERO) and seed to this tick's target so the
    // first render doesn't warp from origin.
    if prev.0 == bevy::math::Vec3::ZERO && curr.0 == bevy::math::Vec3::ZERO {
        prev.0 = target;
        curr.0 = target;
    } else {
        prev.0 = curr.0;
        curr.0 = target;
    }
}

pub(super) fn heading_to_forward(heading: u8) -> (f32, f32) {
    let angle = (heading as f32) * std::f32::consts::TAU / 256.0;
    (angle.cos(), -angle.sin())
}

fn radius_for_wire_kind(kind: EntityKind) -> f32 {
    match kind {
        EntityKind::Pc => kuluu_session::state::MODEL_RADIUS_PC,
        EntityKind::Npc => kuluu_session::state::MODEL_RADIUS_NPC,
        EntityKind::Mob => kuluu_session::state::MODEL_RADIUS_MOB,
        EntityKind::Pet => kuluu_session::state::MODEL_RADIUS_PET,
        EntityKind::Other => kuluu_session::state::MODEL_RADIUS_OTHER,
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
            let ground = kuluu_render::ffxi_to_bevy(e.pos);
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

// Retail's camera swings behind a carving character at ~0.55 rad/s (HorizonXI
// video 2026-07-20: ~150-180° over a ~5s held D). This lazy follow is what
// makes a held A/D trace a wide circle — the camera-relative run direction
// only rotates as fast as the camera catches up. When no lateral steer is
// held (plain W/S), the camera snaps behind faster (play-testing feedback).
const CARVE_FOLLOW_RATE: f32 = 0.55;

const AUTO_RECENTER_RATE: f32 = 2.5;

/// Retail plants the chase camera when the character deliberately runs toward
/// it (unlocked S / about-face): the follow must not swing around to the
/// character's back mid-run. A/D carves sit near ±π/2 and must still follow,
/// so the hold only engages past this threshold.
const RECENTER_HOLD_RAD: f32 = 2.0;

pub fn recenter_follow_allowed(yaw_diff: f32) -> bool {
    yaw_diff.abs() < RECENTER_HOLD_RAD
}

const FP_LOCK_PITCH_RATE: f32 = 3.0;

const TARGET_HEAD_OFFSET_Y: f32 = 1.5;

pub fn camera_polish_system(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<Bindings>,
    pad: Res<super::gamepad_input::PadStickIntent>,
    time: Res<Time>,
    mode: Res<InputMode>,
    camera_mode: Res<CameraMode>,
    state: Res<SceneState>,
    lock_on: Res<LockOn>,
    pointer: Res<kuluu_render::MousePointer>,
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
        || bindings.pressed(Action::CameraYawRight, &keys)
        || pad.camera.x != 0.0;
    let drag_active = pointer.left || pointer.right;
    if yaw_input || drag_active {
        recenter.manual_override = true;
    }
    let movement_input = bindings.pressed(Action::MoveForward, &keys)
        || bindings.pressed(Action::MoveBackward, &keys)
        || bindings.pressed(Action::StrafeLeft, &keys)
        || bindings.pressed(Action::StrafeRight, &keys)
        || bindings.pressed(Action::TurnLeft, &keys)
        || bindings.pressed(Action::TurnRight, &keys)
        || bindings.pressed(Action::RotateLeft, &keys)
        || bindings.pressed(Action::RotateRight, &keys)
        || pad.movement != Vec2::ZERO;
    if movement_input {
        recenter.manual_override = false;
    }

    // Recenter only tracks the character while it is actually moving; idle,
    // the camera holds wherever the player left it (retail behavior).
    if movement_input
        && !yaw_input
        && !drag_active
        && !recenter.manual_override
        && matches!(*camera_mode, CameraMode::Chase)
    {
        let carving = bindings.pressed(Action::TurnLeft, &keys)
            || bindings.pressed(Action::TurnRight, &keys)
            || pad.movement.x != 0.0;
        let rate = if carving {
            CARVE_FOLLOW_RATE
        } else {
            AUTO_RECENTER_RATE
        };
        let target_yaw = yaw_for_heading(state.snapshot.self_pos.heading);
        let tau = std::f32::consts::TAU;
        let mut diff = (target_yaw - chase.yaw).rem_euclid(tau);
        if diff > std::f32::consts::PI {
            diff -= tau;
        }
        let alpha = 1.0 - (-rate * time.delta_secs()).exp();
        if recenter_follow_allowed(diff) {
            chase.yaw += diff * alpha;
        }
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

    let eye = self_t.translation + Vec3::Y * kuluu_render::first_person_eye_y(None);
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
    use kuluu_snapshot::{Entity as WireEntity, EntityKind, Vec3 as WireVec3};

    #[test]
    fn recovery_tracker_waits_then_latches_until_the_report_changes() {
        const TICK: f32 = 1.0 / 60.0;
        let candidate = GroundRecoveryCandidate {
            zone_id: 100,
            self_id: 7,
            reported_pos: Vec3::ZERO,
            recovered_z: -5.319,
        };
        let mut tracker = GroundRecoveryTracker::default();
        let mut ticks = 0;
        while !tracker.observe(Some(candidate), TICK) {
            ticks += 1;
            assert!(ticks < 1000, "debounce never fired while under the floor");
        }
        assert!(
            (ticks as f32 * TICK - UNDER_FLOOR_RECOVERY_SECS).abs() < TICK,
            "fired after {ticks} ticks, expected ~{UNDER_FLOOR_RECOVERY_SECS}s"
        );
        tracker.mark_queued();
        assert!(
            !tracker.observe(Some(candidate), UNDER_FLOOR_RECOVERY_SECS * 2.0),
            "one bad reported pose must enqueue at most one correction"
        );
        assert!(!tracker.observe(None, 0.0));
        assert!(!tracker.observe(Some(candidate), TICK));
    }

    #[test]
    fn recovery_tracker_does_not_accumulate_across_columns() {
        let first = GroundRecoveryCandidate {
            zone_id: 100,
            self_id: 7,
            reported_pos: Vec3::ZERO,
            recovered_z: -5.319,
        };
        let second = GroundRecoveryCandidate {
            reported_pos: Vec3::new(GROUND_CORRECTION_XY_EPSILON_YALMS * 2.0, 0.0, 0.0),
            ..first
        };
        let mut tracker = GroundRecoveryTracker::default();
        assert!(!tracker.observe(Some(first), UNDER_FLOOR_RECOVERY_SECS * 0.9));
        assert!(
            !tracker.observe(Some(second), UNDER_FLOOR_RECOVERY_SECS * 0.2),
            "time from another collision column must not satisfy the debounce"
        );
    }

    /// A single up-facing slab at bevy y = `floor_y` spanning the origin
    /// column, enough for `ground_step`/`ground_nearest` to resolve.
    fn slab_collision(floor_y: f32) -> kuluu_render::dat_mzb::MzbCollisionGeometry {
        use kuluu_render::dat_mzb::{
            build_collision_geometry, MzbCollisionGeometry, MzbInstance, MzbSubMesh,
        };
        const HALF: f32 = 10.0;
        let sub = MzbSubMesh {
            positions: vec![
                [-HALF, floor_y, -HALF],
                [HALF, floor_y, -HALF],
                [HALF, floor_y, HALF],
                [-HALF, floor_y, HALF],
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            tri_terrain: vec![0, 0],
            tri_normal: vec![[0.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
            tri_camera_transparent: vec![false, false],
            flags: 0,
        };
        let inst = MzbInstance {
            submesh_idx: 0,
            bevy_transform: bevy::prelude::Transform::IDENTITY,
            water_height_bevy: None,
            sub_area_link: 0,
        };
        MzbCollisionGeometry::from_block(build_collision_geometry(&[sub], &[inst], None))
    }

    /// A [`LoadMzbInFlight`] holding one outstanding zone-geometry load, as the
    /// window where `sub_area_activation` has retired a block and its
    /// replacement has not installed yet.
    fn one_load_in_flight() -> kuluu_render::dat_mzb::LoadMzbInFlight {
        use kuluu_render::dat_mzb::{LoadMzbInFlight, LoadedZoneGeom};
        bevy::tasks::AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        let task = bevy::tasks::AsyncComputeTaskPool::get().spawn(async {
            LoadedZoneGeom {
                submeshes: std::sync::Arc::new(Vec::new()),
                instances: std::sync::Arc::new(Vec::new()),
                mmb_spawns: Err(String::from("test stub")),
            }
        });
        let mut in_flight = LoadMzbInFlight::default();
        in_flight.tasks.insert((0, None), (Vec::new(), task));
        in_flight
    }

    /// The wedge repro: feet at wire z = 0 (bevy y = 0) with the only floor a
    /// full body above, past `MAX_GROUND_STEP_UP`.
    const WEDGE_FLOOR_BEVY_Y: f32 = 5.0;
    const WEDGE_POS: Vec3 = Vec3::new(0.0, 0.0, 0.0);

    #[test]
    fn wedged_candidate_repairs_the_reported_column() {
        const ASSERT_EPSILON: f32 = 1e-3;
        let collision = slab_collision(WEDGE_FLOOR_BEVY_Y);
        let candidate = ground_recovery_candidate(
            &collision,
            &kuluu_render::dat_mzb::LoadMzbInFlight::default(),
            Some(103),
            Some(7),
            WEDGE_POS,
        )
        .expect("the reported wire position is still wedged");
        let cmd = ground_recovery_command(
            candidate.zone_id,
            candidate.self_id,
            candidate.reported_pos.x,
            candidate.reported_pos.y,
            candidate.recovered_z,
            7,
        );

        assert!(matches!(
            cmd,
            AgentCommand::GroundCorrection { x, y, z, heading, .. }
                if x.abs() < ASSERT_EPSILON
                    && y.abs() < ASSERT_EPSILON
                    && (z + WEDGE_FLOOR_BEVY_Y).abs() < ASSERT_EPSILON
                    && heading == 7
        ));
        assert!(
            ground_recovery_candidate(
                &collision,
                &kuluu_render::dat_mzb::LoadMzbInFlight::default(),
                Some(103),
                Some(7),
                Vec3::new(20.0, 0.0, 0.0),
            )
            .is_none(),
            "a diagnosis from the origin column must not select another column's floor"
        );
    }

    #[test]
    fn recovery_is_gated_while_zone_collision_is_still_loading() {
        const TICK: f32 = 1.0 / 60.0;
        let collision = slab_collision(WEDGE_FLOOR_BEVY_Y);
        let loading = one_load_in_flight();
        let idle = kuluu_render::dat_mzb::LoadMzbInFlight::default();
        let mut tracker = GroundRecoveryTracker::default();

        // Well past the debounce: an interior swap outlasts
        // UNDER_FLOOR_RECOVERY_SECS, which is exactly why the debounce alone is
        // not the gate.
        let loading_ticks = (UNDER_FLOOR_RECOVERY_SECS * 4.0 / TICK) as u32;
        for _ in 0..loading_ticks {
            let candidate =
                ground_recovery_candidate(&collision, &loading, Some(103), Some(7), WEDGE_POS);
            assert!(
                candidate.is_none(),
                "recovered onto the shell while the collision set was incomplete"
            );
            assert!(!tracker.observe(candidate, TICK));
        }

        let mut fired = None;
        for _ in 0..loading_ticks {
            let candidate =
                ground_recovery_candidate(&collision, &idle, Some(103), Some(7), WEDGE_POS);
            if tracker.observe(candidate, TICK) {
                fired = candidate;
                break;
            }
        }
        match fired {
            Some(GroundRecoveryCandidate { recovered_z, .. }) => {
                assert!(
                    (recovered_z + WEDGE_FLOOR_BEVY_Y).abs() < 1e-3,
                    "recovered to wire z {recovered_z}, expected the slab"
                );
            }
            other => panic!("recovery must still fire on a real wedge once loaded, got {other:?}"),
        }
    }

    #[test]
    fn recovery_command_keeps_the_goal_and_only_rebases_its_own_override_column() {
        use kuluu_session::reactor::{Goal, Reactor, ReactorConfig};
        use kuluu_session::state::{AgentEvent, Position, Vec3 as WireVec3};

        let collision = slab_collision(WEDGE_FLOOR_BEVY_Y);
        let candidate = ground_recovery_candidate(
            &collision,
            &kuluu_render::dat_mzb::LoadMzbInFlight::default(),
            Some(103),
            Some(7),
            WEDGE_POS,
        )
        .expect("the wedge column must produce a recovery");
        let recovered_z = candidate.recovered_z;
        let cmd = ground_recovery_command(103, 7, 0.0, 0.0, recovered_z, 0);

        let mut r = Reactor::new(ReactorConfig::default());
        r.observe_event(&AgentEvent::Connected {
            account_id: 1,
            char_id: 7,
            character: "Tester".into(),
            zone_id: 103,
        });
        r.handle_command(AgentCommand::Follow {
            target_id: 7,
            distance: 3.0,
        });
        let routing = r.handle_command(cmd.clone());
        assert!(
            routing.forward.is_some(),
            "recovery never reached the wire: {routing:?}"
        );
        assert!(
            matches!(r.current_goal(), Goal::Following { target_id: 7, .. }),
            "recovery cancelled the player's follow: {:?}",
            r.current_goal()
        );

        const OVERRIDE_TTL_MS: u32 = 5_000;
        let forced_target = Position {
            pos: WireVec3 {
                x: 10.0,
                y: 0.0,
                z: 0.0,
            },
            ..Position::default()
        };
        r.observe_event(&AgentEvent::ForcedMove {
            mode: 0,
            target: forced_target,
            duration_ms: OVERRIDE_TTL_MS,
        });
        let routing = r.handle_command(cmd.clone());
        assert!(
            routing.forward.is_none(),
            "a correction diagnosed in another column must not affect forced movement"
        );
        assert!(
            matches!(r.current_override(), Some(ov) if ov.target.z.abs() < f32::EPSILON),
            "a different forced-move column must keep its own height"
        );

        let matching = ground_recovery_command(103, 7, 10.0, 0.0, recovered_z, 0);
        let routing = r.handle_command(matching);
        assert!(
            routing.forward.is_some(),
            "a correction at the forced target may repair that target"
        );
        assert!(
            matches!(r.current_override(), Some(ov) if (ov.target.z - recovered_z).abs() < 1e-6),
            "the override's own column should retain the corrected height"
        );
    }

    #[test]
    fn recovery_debounce_restarts_after_the_load_gate_clears() {
        let collision = slab_collision(WEDGE_FLOOR_BEVY_Y);
        let loading = one_load_in_flight();
        let idle = kuluu_render::dat_mzb::LoadMzbInFlight::default();
        let mut tracker = GroundRecoveryTracker::default();
        let candidate =
            ground_recovery_candidate(&collision, &loading, Some(103), Some(7), WEDGE_POS);
        assert!(candidate.is_none());
        assert!(!tracker.observe(candidate, UNDER_FLOOR_RECOVERY_SECS * 10.0));
        let candidate = ground_recovery_candidate(&collision, &idle, Some(103), Some(7), WEDGE_POS);
        assert!(
            !tracker.observe(candidate, UNDER_FLOOR_RECOVERY_SECS * 0.5),
            "gated time must not count toward the debounce"
        );
    }

    #[test]
    fn heal_toggle_alternates_stance_and_wire_mode() {
        use kuluu_render::combat_stance::{RestKind, RestStance};
        use kuluu_session::state::HealMode;

        let (tx, mut rx) = mpsc::channel(4);
        let cmd_tx = CommandTx(tx);
        let mut stance = RestStance::default();

        toggle_heal(&mut stance, &cmd_tx);
        assert_eq!(stance.kind, RestKind::Heal);
        assert!(matches!(
            rx.try_recv(),
            Ok(AgentCommand::Heal { mode: HealMode::On })
        ));

        toggle_heal(&mut stance, &cmd_tx);
        assert_eq!(stance.kind, RestKind::None);
        assert!(matches!(
            rx.try_recv(),
            Ok(AgentCommand::Heal {
                mode: HealMode::Off
            })
        ));

        stance.kind = RestKind::Sit;
        toggle_heal(&mut stance, &cmd_tx);
        assert_eq!(stance.kind, RestKind::Heal);
    }

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
            kuluu_session::state::MODEL_RADIUS_PC
        );
        assert_eq!(
            radius_for_wire_kind(EntityKind::Mob),
            kuluu_session::state::MODEL_RADIUS_MOB
        );
        assert_eq!(
            radius_for_wire_kind(EntityKind::Pet),
            kuluu_session::state::MODEL_RADIUS_PET
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
            name_vis: None,
            face_target: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            mount: None,
            status: 0,
            char_flags: Default::default(),
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

    #[derive(Default)]
    struct MoveKeys {
        forward: bool,
        backward: bool,
        turn_left: bool,
        turn_right: bool,
        rotate_left: bool,
        autorun: bool,
        locked: bool,
    }

    fn resolve(k: MoveKeys) -> ResolvedMoveInputs {
        resolve_move_inputs(
            k.forward,
            k.backward,
            k.turn_left,
            k.turn_right,
            false,
            false,
            k.rotate_left,
            false,
            k.autorun,
            k.locked,
        )
    }

    #[test]
    fn unlocked_turn_steers_not_strafes_not_rotates() {
        let r = resolve(MoveKeys {
            turn_left: true,
            ..Default::default()
        });
        assert_eq!(r.steer, -1);
        assert_eq!(r.strafe, 0);
        assert_eq!(r.rotate_dir, 0);
        assert_eq!(r.forward, 0);
    }

    #[test]
    fn mounted_speed_doubles_and_clamps_like_retail() {
        // LSB defaults: 50 on foot, 40 mounted. Without the client-side doubling
        // the mount would be the slower of the two.
        assert_eq!(move_speed_yps(50, false), 5.0);
        assert_eq!(move_speed_yps(40, true), 8.0);
        assert!(move_speed_yps(40, true) > move_speed_yps(50, false));

        // ControllableActor.cpp clamps after doubling, so the cap is only ever
        // reachable mounted (an unmounted u8 tops out at 25.5).
        assert_eq!(move_speed_yps(u8::MAX, false), 25.5);
        assert_eq!(
            move_speed_yps(u8::MAX, true),
            kuluu_session::state::MAX_MOVE_SPEED_YPS
        );

        // Bound sets speed 0; doubling must not conjure movement.
        assert_eq!(move_speed_yps(0, true), 0.0);
    }

    #[test]
    fn unlocked_forward_plus_turn_steers_at_full_speed() {
        let r = resolve(MoveKeys {
            forward: true,
            turn_left: true,
            ..Default::default()
        });
        assert_eq!(r.forward, 1);
        assert_eq!(r.steer, -1);
        assert_eq!(r.strafe, 0, "unlocked W+A must not strafe");
    }

    #[test]
    fn unlocked_backward_steers_toward_camera_full_speed() {
        let r = resolve(MoveKeys {
            backward: true,
            ..Default::default()
        });
        assert_eq!(r.forward, -1, "S feeds the camera-relative steer, no flip");
        assert_eq!(r.strafe, 0);
    }

    #[test]
    fn locked_backward_backpedals() {
        let r = resolve(MoveKeys {
            backward: true,
            locked: true,
            ..Default::default()
        });
        assert_eq!(r.forward, -1);
        assert_eq!(r.steer, 0);
    }

    #[test]
    fn locked_turn_strafes_not_steers() {
        let r = resolve(MoveKeys {
            turn_right: true,
            locked: true,
            ..Default::default()
        });
        assert_eq!(r.strafe, 1);
        assert_eq!(r.steer, 0);
    }

    #[test]
    fn rotate_key_is_independent_of_steer() {
        let r = resolve(MoveKeys {
            rotate_left: true,
            turn_right: true,
            ..Default::default()
        });
        assert_eq!(r.rotate_dir, -1);
        assert_eq!(r.steer, 1);
    }

    #[test]
    fn forward_and_backward_cancel() {
        let r = resolve(MoveKeys {
            forward: true,
            backward: true,
            ..Default::default()
        });
        assert_eq!(r.forward, 0);
    }

    #[test]
    fn autorun_keeps_running_while_steering() {
        let r = resolve(MoveKeys {
            autorun: true,
            turn_left: true,
            ..Default::default()
        });
        assert_eq!(r.forward, 1);
        assert_eq!(r.steer, -1);
    }

    #[test]
    fn backward_motion_heading_is_toward_camera() {
        // S runs at the camera: motion heading = camera forward + 180° (128 units).
        for cam in [0u8, 64, 128, 200] {
            assert_eq!(
                camera_relative_motion_heading(cam, -1.0, 0.0),
                cam.wrapping_add(128),
                "cam={cam}"
            );
        }
    }

    #[test]
    fn recenter_holds_camera_when_running_toward_it() {
        // S about-face: heading is a full π from the camera yaw — camera stays put.
        assert!(!recenter_follow_allowed(std::f32::consts::PI));
        assert!(!recenter_follow_allowed(-std::f32::consts::PI));
        assert!(!recenter_follow_allowed(2.5));
    }

    #[test]
    fn recenter_follows_carves_and_forward_travel() {
        // A/D carves sit near ±π/2; forward travel near 0. Both must follow.
        assert!(recenter_follow_allowed(0.0));
        assert!(recenter_follow_allowed(std::f32::consts::FRAC_PI_2));
        assert!(recenter_follow_allowed(-std::f32::consts::FRAC_PI_2));
    }

    #[test]
    fn forward_motion_heading_matches_camera_forward() {
        for cam in [0u8, 33, 100, 250] {
            assert_eq!(
                camera_relative_motion_heading(cam, 1.0, 0.0),
                cam,
                "cam={cam}"
            );
        }
    }

    #[test]
    fn steer_motion_heading_is_camera_right() {
        // D alone runs along camera-right (+64 heading units); A camera-left.
        assert_eq!(camera_relative_motion_heading(0, 0.0, 1.0), 64);
        assert_eq!(camera_relative_motion_heading(0, 0.0, -1.0), 192);
    }

    #[test]
    fn forward_steer_motion_heading_is_diagonal() {
        assert_eq!(camera_relative_motion_heading(0, 1.0, 1.0), 32);
        assert_eq!(camera_relative_motion_heading(0, -1.0, 1.0), 96);
    }

    #[test]
    fn analog_motion_heading_preserves_stick_direction_ratio() {
        // A stick at 30° off camera-forward must not collapse to the 45°
        // digital diagonal: atan(0.5/0.866) = 30° = ~21 heading units.
        let h = camera_relative_motion_heading(0, 0.866, 0.5);
        assert!((21i32 - i32::from(h)).abs() <= 1, "got {h}");
    }

    #[test]
    fn pick_mag_larger_deflection_wins_ties_to_keyboard() {
        assert_eq!(pick_mag(1.0, 0.4), 1.0);
        assert_eq!(pick_mag(0.0, -0.7), -0.7);
        assert_eq!(pick_mag(-1.0, 0.9), -1.0);
        assert_eq!(pick_mag(1.0, -1.0), 1.0);
        assert_eq!(pick_mag(0.0, 0.0), 0.0);
    }

    #[test]
    fn merge_dir_quantizes_a_winning_stick() {
        assert_eq!(merge_dir(0, 0.8), 1);
        assert_eq!(merge_dir(0, -0.3), -1);
        assert_eq!(merge_dir(1, -0.4), 1);
        assert_eq!(merge_dir(-1, 0.9), -1);
        assert_eq!(merge_dir(0, 0.0), 0);
    }

    #[test]
    fn autorun_toggle_engages_from_standstill() {
        assert!(autorun_after_toggle(false, true));
    }

    #[test]
    fn autorun_toggle_disengages_when_active() {
        assert!(!autorun_after_toggle(true, true));
    }

    #[test]
    fn autorun_unchanged_without_toggle_press() {
        assert!(!autorun_after_toggle(false, false));
        assert!(autorun_after_toggle(true, false));
    }

    #[test]
    fn focused_chat_input_keeps_autorun_running() {
        let chat = InputMode::Chat(ChatBuffer::empty());
        assert!(!mode_cancels_autorun(&chat));
        assert!(mode_swallows_keys(&chat));
    }

    #[test]
    fn npc_and_shop_screens_cancel_autorun() {
        for mode in [
            InputMode::Dialog(kuluu_render::DialogCursor::default()),
            InputMode::DeliveryBox,
            InputMode::Check,
            InputMode::Bazaar,
            InputMode::Auction,
        ] {
            assert!(mode_cancels_autorun(&mode), "{mode:?}");
            assert!(!mode_swallows_keys(&mode), "{mode:?}");
        }
    }

    #[test]
    fn world_mode_neither_cancels_autorun_nor_swallows_keys() {
        assert!(!mode_cancels_autorun(&InputMode::World));
        assert!(!mode_swallows_keys(&InputMode::World));
    }

    #[test]
    fn heading_turn_accumulates_to_finite_rate_over_one_second() {
        let mut accum = 0.0_f32;
        let dt = 1.0 / 60.0;
        let mut total_u8: i32 = 0;
        for _ in 0..60 {
            let (whole, _f) = advance_heading_turn(&mut accum, HEADING_TURN_RATE, dt);
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

        let (whole_1, float_1) = advance_heading_turn(&mut accum, HEADING_TURN_RATE, dt);
        assert_eq!(whole_1, 0, "first 60Hz tick must not yet flip a u8");
        assert!(float_1 > 0.0 && float_1 < 1.0);
        assert!(accum > 0.0, "fractional units must carry over");

        let mut flipped = false;
        for _ in 0..10 {
            let (w, _) = advance_heading_turn(&mut accum, HEADING_TURN_RATE, dt);
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

        let _ = advance_heading_turn(&mut accum, HEADING_TURN_RATE, dt);
        assert!(accum > 0.0);

        let (whole, fdelta) = advance_heading_turn(&mut accum, 0.0, dt);
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
            total_l += advance_heading_turn(&mut accum_l, -HEADING_TURN_RATE, dt).0;
            total_r += advance_heading_turn(&mut accum_r, HEADING_TURN_RATE, dt).0;
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

// -----------------------------------------------------------------------------
// Stair-capture harness (FFXI_STAIR_DRIVE / FFXI_STAIR_CAPTURE) — rebuild #3.
// An external driver holds {-1,0,1} axes over a TCP JSON line; dispatch folds
// them into the real input pipeline, and `stair_capture_system` writes one JSON
// position sample per FixedUpdate tick while capturing. See
// archive/docs/stair_capture.md (archived) for the protocol, run recipe and
// coordinate facts.
// -----------------------------------------------------------------------------

/// Remote drive state: axis holds from the external driver. Same {-1,0,1}
/// forward/strafe semantics as held keys, plus a Q/E-style turn axis (folded
/// into rotate_dir) and a chase-camera pan axis; `yaw_warp` is a one-shot exact
/// camera-aim target consumed on the next dispatch tick.
#[derive(Default)]
pub struct StairDrive {
    pub f: i32,
    pub s: i32,
    pub t: i32,
    /// Chase-camera yaw pan axis (W is camera-relative in chase mode; the body
    /// turn `t` does NOT re-aim forward).
    pub c: i32,
    /// Hold expiry; `None` means never armed (fresh handle has no live hold).
    until: Option<Instant>,
    /// One-shot exact chase.yaw target (radians); applied once, then cleared.
    yaw_warp: Option<f32>,
}

impl StairDrive {
    /// Live override axes (f, s, t, c), or `None` once the hold expired.
    pub fn active(&self) -> Option<(i32, i32, i32, i32)> {
        match self.until {
            Some(u) if Instant::now() < u => Some((self.f, self.s, self.t, self.c)),
            _ => None,
        }
    }

    /// Consume the pending one-shot camera warp, if any.
    pub fn take_warp(&mut self) -> Option<f32> {
        self.yaw_warp.take()
    }
}

/// Shared with the `FFXI_STAIR_DRIVE` TCP listener so driver holds reach the Bevy
/// input path without OS keystrokes. Always inserted; only listened on when the
/// env var names an address.
#[derive(Resource)]
pub struct StairDriveHandle(pub std::sync::Arc<std::sync::Mutex<StairDrive>>);

/// One TCP line per hold: `{"f":1,"s":0,"t":0,"c":0,"ms":8000}`. `f`/`s` are the
/// run axes (W/S, A/D), `t` is Q/E-style rotate-in-place, `c` pans the chase
/// camera at the key yaw rate; optional `"w"` sets a one-shot exact yaw target.
/// Replaces any prior hold; all-zero with `ms == 0` clears.
pub async fn serve_stair_drive(
    addr: std::net::SocketAddr,
    drive: std::sync::Arc<std::sync::Mutex<StairDrive>>,
) {
    let Ok(listener) = tokio::net::TcpListener::bind(addr).await else {
        tracing::warn!(%addr, "FFXI_STAIR_DRIVE bind failed");
        return;
    };
    tracing::info!(%addr, "FFXI_STAIR_DRIVE listening");
    loop {
        let Ok((sock, _)) = listener.accept().await else {
            break;
        };
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut lines = BufReader::new(tokio::io::BufWriter::new(sock)).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            // One hold per line; each line fully replaces the previous one.
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let f = v.get("f").and_then(|x| x.as_i64()).unwrap_or(0);
            let s = v.get("s").and_then(|x| x.as_i64()).unwrap_or(0);
            let t = v.get("t").and_then(|x| x.as_i64()).unwrap_or(0);
            let c = v.get("c").and_then(|x| x.as_i64()).unwrap_or(0);
            let ms = v.get("ms").and_then(|x| x.as_u64()).unwrap_or(0);
            let w = v.get("w").and_then(|x| x.as_f64());
            if (f, s, t, c) == (0, 0, 0, 0) && ms == 0 {
                // Clear: expire the hold immediately.
                if let Ok(mut d) = drive.lock() {
                    d.until = None;
                }
            } else {
                if let Ok(mut d) = drive.lock() {
                    d.f = f as i32;
                    d.s = s as i32;
                    d.t = t as i32;
                    d.c = c as i32;
                    d.until = Some(Instant::now() + Duration::from_millis(ms));
                }
            }
            if let Some(target) = w {
                if let Ok(mut d) = drive.lock() {
                    d.yaw_warp = Some(target as f32);
                }
            }
        }
    }
}

/// Per-tick capture state: tick counter + direction hysteresis memory.
#[derive(Default)]
pub struct CaptureState {
    tick: u64,
    last_z: Option<f32>,
    dir: &'static str,
}

/// One JSON position sample per FixedUpdate tick while FFXI_STAIR_CAPTURE names
/// an output file. Emits rendered transform, wire (FFXI-space) prediction +
/// heading, derived up/down direction,
/// and gate diagnostics (active drive axes + dispatch early-return conditions)
/// so a frozen run can be diagnosed from the stream itself.
pub fn stair_capture_system(
    state: Res<SceneState>,
    prediction: Res<LocalPlayerPrediction>,
    mode: Res<InputMode>,
    rest: Res<kuluu_render::combat_stance::RestStance>,
    camera: Res<ChaseCamera>,
    drive: Option<Res<'_, StairDriveHandle>>,
    q_self: Query<&kuluu_render::CurrRenderPos, (With<IsSelf>, Without<OperatorCamera>)>,
    mut cap: Local<CaptureState>,
) {
    static PATH: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let Some(path) = PATH.get_or_init(|| std::env::var("FFXI_STAIR_CAPTURE").ok()) else {
        return;
    };
    let Some(curr_pos) = q_self.single().ok() else {
        return; // no rendered self yet (zone transition / not logged in)
    };
    if state.snapshot.self_char_id.is_none() {
        return;
    }
    let wire = prediction.pos;
    cap.tick += 1;

    // Direction hysteresis: |dwz| > 0.015/tick flips dir, otherwise hold last.
    let dz = match cap.last_z {
        Some(last) => wire.z - last,
        None => 0.0,
    };
    if dz.abs() > 0.015 {
        cap.dir = if dz < 0.0 { "up" } else { "down" };
    }
    cap.last_z = Some(wire.z);

    // Active drive axes (diagnostics): what the driver is holding right now.
    let axes = drive
        .as_ref()
        .and_then(|h| h.0.lock().ok())
        .and_then(|d| d.active())
        .unwrap_or((0, 0, 0, 0));
    let rest_on = !matches!(rest.kind, kuluu_render::combat_stance::RestKind::None);
    // The purple-march slopes are gone with the old detector; emit JSON null
    // so the harness schema stays stable until the walker's field debug feeds
    // real values.
    let pslope_json = String::from("null");
    let pslope_up_json = String::from("null");

    let line = format!(
        "{{\"tick\":{},\"t_ms\":{},\"cyaw\":{:.9e},\"wx\":{:.9e},\"wy\":{:.9e},\"wz\":{:.9e},\
         \"rx\":{:.9e},\"ry\":{:.9e},\"rz\":{:.9e},\"heading\":{},\
         \"lock\":{},\"slope\":{},\"streak\":{},\
         \"pslope\":{},\"pslope_up\":{},\"dir\":\"{}\",\
         \"cancel\":{},\"swallow\":{},\"rest\":{},\
         \"df\":{},\"ds\":{},\"dt\":{},\"dc\":{}}}",
        cap.tick,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        camera.yaw,
        wire.x,
        wire.y,
        wire.z,
        // rx/ry/rz = per-tick authoritative render position (pre-interpolation).
        // Transform.translation is what the camera SEES (lerped between ticks);
        // CurrRenderPos.0 is what apply_self_prediction wrote THIS tick.
        curr_pos.0.x,
        curr_pos.0.y,
        curr_pos.0.z,
        state.snapshot.self_pos.heading,
        false, // lock (removed; harness JSON schema kept for tool compat)
        0.0,   // slope (removed)
        0u8,   // streak (removed)
        pslope_json,
        pslope_up_json,
        cap.dir,
        mode_cancels_autorun(&mode),
        mode_swallows_keys(&mode),
        rest_on,
        axes.0,
        axes.1,
        axes.2,
        axes.3,
    );

    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}
