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
pub struct MoveEnvParams<'w, 's> {
    // Player movement grounds height on the retail MZB zone collision (the real
    // .dat floor, which has the stairs). The coarse LSB Recast navmesh is a
    // mob-pathing mesh that flattens stairs, so it is NOT used here — only for
    // /pathto and minimap culling (kuluu-oe8y; see AGENTS.md).
    pub collision: Res<'w, kuluu_render::dat_mzb::MzbCollisionGeometry>,
    /// Debug noclip: when on, the wall clamp in dispatch_movement is bypassed
    /// (grounding stays on). Toggled from the Debug menu NoClip row or /noclip.
    pub hud_panels: Res<'w, kuluu_render::hud::HudPanels>,
    pub minimap_hover: Res<'w, kuluu_render::minimap::input::MinimapHoverGate>,
    pub pointer: Res<'w, kuluu_render::MousePointer>,
    pub pad: Res<'w, super::gamepad_input::PadStickIntent>,
    // Focus-less GUI driving (kuluu-0pof): remote movement injection.
    pub debug_ctrl: Option<Res<'w, super::DebugControlHandle>>,
    // Stair-capture drive channel (FFXI_STAIR_DRIVE): forward/strafe holds plus
    // a Q/E-style turn axis for the external driver. None unless wired at connect.
    pub stair_drive: Option<Res<'w, StairDriveHandle>>,
    /// Debug: the stair HUD's orchestration column reads the last two
    /// resolve_position verdicts from here (written each moving tick).
    pub orch_log: ResMut<'w, kuluu_render::hud::stair_debug::OrchDecisionLog>,
    /// The one stair detection per tick (word of god): resolve_position writes
    /// it, apply_self_prediction_system + HUD read it. No duplicate detect_stairs.
    pub last_stair: ResMut<'w, LastStairDetection>,
    /// For resolving a blocking door entity's mesh/texture name in debug.
    pub mmb_names: Query<
        'w,
        's,
        (
            &'static kuluu_render::components::MmbDebugInfo,
            &'static GlobalTransform,
            &'static ViewVisibility,
        ),
    >,
    /// Standalone avian door colliders are anonymous; this resolves them back
    /// to the occluder placement that carries MmbDebugInfo + ViewVisibility.
    pub door_source: Query<'w, 's, &'static super::avian_bridge::DoorColliderSource>,
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
/// Fields are the previous individual `Local`s verbatim; behaviour is
/// unchanged.
#[derive(Default)]
pub struct DispatchLocals {
    /// Latched world-space run heading for pure W/S: (forward sign, motion
    /// heading). Sampled from the camera frame when the key state changes,
    /// then held fixed so the camera's auto-recenter can swing behind
    /// without dragging the run direction with it.
    pub steer_latch: Option<(i32, u8)>,
    /// Rising-edge memory for pad stick just_pressed emulation.
    pub pad_edges: PadEdges,
    /// Bounce-settle countdown — nonzero for a few ticks after any
    /// landed / grace-held tick; see the stair-settle clamp before the
    /// Move send.
    pub step_settle: u8,
    /// Mob push-through accrual: which mob the player is shoving and for how
    /// long, so a sustained press releases that one mob from the sweep.
    pub push_through: super::avian_bridge::PushThrough,
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

use crate::state::{ActionKind, AgentCommand, FishingInput};

// Matches the retail first-person A/D view-rotate rate (HorizonXI video
// 2026-07-20: ~71 heading-units over a 2s hold ≈ 0.87 rad/s).
pub const HEADING_TURN_RATE: f32 = 0.86;

// Q/E rotate-in-place has no retail 3rd-person counterpart; 0.86 felt too
// sluggish in play-testing, so it gets its own snappier rate.
pub const ROTATE_KEY_RATE_RAD_PER_SEC: f32 = 2.0;

const CAMERA_YAW_RATE: f32 = HEADING_TURN_RATE * 4.0;

const PITCH_STEP_HELD: f32 = 0.015;

const STRAFE_CANCEL_MS: u64 = 300;

use kuluu_session::state::move_speed_yps;

const BACKPEDAL_SCALE: f32 = 0.5;
const STRAFE_SCALE: f32 = 0.75;

// A stick pulled this far toward the camera cancels autorun, like a tapped S;
// gentler deflections only carve (retail autorun is steerable).
const PAD_BACK_CANCEL_DEFLECTION: f32 = 0.5;

const PREDICTION_RESYNC_YALMS: f32 = 5.0;

/// Stair-detector lip cutoff (yalms): a ring sample within this height of the
/// player's foot is a LIP — curb, expansion joint, or broken decorative stair
/// piece. Lips are gray in the debug display and ignored by every stair calc
/// (banding, march guards). Must equal the band-1 lower bound (`B1_LO`) so no
/// sample falls into an unclassified gap: |dy| <= LIP_MAX is a lip, above it
/// up to 0.4 is a real step (purple/cyan), beyond that is red (wall / too
/// tall / unchainable).
const LIP_MAX: f32 = 0.18;

/// Debug data captured by `apply_self_prediction_system` for the gizmo drawer
/// to render in Update. FixedUpdate can't draw gizmos directly.
#[derive(bevy::prelude::Resource, Clone, Copy)]
pub struct FootprintDebug {
    pub enabled: bool,
    pub center_xz: bevy::math::Vec2,
    pub center_y: f32,
    pub radius: f32,
    pub sampled_points: [(bevy::math::Vec2, f32, bool, i8); 60], // (xz, y, green_kept, stair_band 0=none, +1..=+5 = up steps, -1..=-5 = down steps)
    pub avg_y: f32,
    pub slope_active: bool, // avg differs from center by > threshold
    // The 5 forward probe points along the detected up-stairs direction:
    // (world xz, ground y). NaN y means the probe didn't hit anything.
    pub fwd_probes: [(bevy::math::Vec2, f32); 11],
    // True when the line fit qualified as a staircase (slope in stair range)
    // and we're actively riding the ramp.
    pub ramp_locked: bool,
    // Fitted line at the two endpoints of the forward probe span, so the
    // drawer can just connect (near) to (far) as one purple segment.
    pub ramp_near_xz: bevy::math::Vec2,
    pub ramp_near_y: f32,
    pub ramp_far_xz: bevy::math::Vec2,
    pub ramp_far_y: f32,
    // Purple straight-down march: probe hits and detected risers. Fixed
    // arrays (not Vec) so FootprintDebug stays Copy. NaN y = unused slot.
    pub purple_probes: [(bevy::math::Vec2, f32); 60],
    pub purple_probe_count: usize,
    pub purple_risers: [(bevy::math::Vec2, f32); 5],
    pub purple_riser_count: usize,
    #[allow(dead_code)]
    pub purple_slope: f32, // NaN if the march didn't produce a slope
    #[allow(dead_code)]
    pub purple_slope_up: f32, // NaN if the ascent march didn't produce a slope
}

impl Default for FootprintDebug {
    fn default() -> Self {
        Self {
            enabled: false,
            center_xz: bevy::math::Vec2::ZERO,
            center_y: 0.0,
            radius: 0.0,
            sampled_points: [(bevy::math::Vec2::ZERO, 0.0, false, 0i8); 60],
            avg_y: 0.0,
            slope_active: false,
            fwd_probes: [(bevy::math::Vec2::ZERO, f32::NAN); 11],
            ramp_locked: false,
            ramp_near_xz: bevy::math::Vec2::ZERO,
            ramp_near_y: 0.0,
            ramp_far_xz: bevy::math::Vec2::ZERO,
            ramp_far_y: 0.0,
            purple_probes: [(bevy::math::Vec2::ZERO, f32::NAN); 60],
            purple_probe_count: 0,
            purple_risers: [(bevy::math::Vec2::ZERO, f32::NAN); 5],
            purple_riser_count: 0,
            purple_slope: f32::NAN,
            purple_slope_up: f32::NAN,
        }
    }
}

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
        // TEMP (exit-hunt): identify which close path fired.
        tracing::info!(
            close_shortcut,
            os_close,
            "TEMP input.rs: AppExit via close path"
        );
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
                    mode: crate::state::HealMode::Off,
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
        RestKind::Heal => (RestKind::None, crate::state::HealMode::Off),

        _ => (RestKind::Heal, crate::state::HealMode::On),
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
    avian: super::avian_bridge::AvianMoveParams,
    state: Res<SceneState>,
    cmd_tx: Res<CommandTx>,
    mode: Res<InputMode>,
    camera_mode: Res<CameraMode>,
    lock_on: Res<LockOn>,
    mut autorun: ResMut<AutoRun>,
    mut chase: ResMut<ChaseCamera>,
    mut turn_accum: ResMut<HeadingTurnAccum>,
    // Bundled per-tick locals (steer_latch + pad_edges + step_settle) so this
    // fn stays under bevy's 16-param SystemParam ceiling. See `DispatchLocals`
    // for the field-level docs the individual `Local`s used to carry.
    mut locals: Local<DispatchLocals>,
    mut prediction: ResMut<LocalPlayerPrediction>,
    mut env: MoveEnvParams,
    mut stance: StanceParams,
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
                    mode: crate::state::HealMode::Off,
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

    let raw_step = move_speed_yps(self_pos.speed, state.snapshot.self_mount.is_some())
        * time.delta_secs()
        * walk_mode.scale();

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

    // Ground height on the MZB zone collision — the retail `.dat` floor, which
    // has the stairs and ramps the coarse LSB pathing navmesh flattens away
    // (kuluu-oe8y). `ground_step` picks the up-facing floor closest to our feet
    // that we could climb to, so a stair step climbs (nearest floor is the next
    // step) and a stacked column (Bastok Markets' walkway over its canal)
    // resolves to the level we're on rather than teleporting to another layer.
    // MZB collision is in Bevy space (bevy.x = ffxi.x, bevy.z = -ffxi.y,
    // bevy.y = -ffxi.z).
    //
    // The step-up bound is what keeps a gap in the floor from launching us: with
    // it unbounded, one tick in Lower Jeuno snapped 5.5 units onto a roof and the
    // ratcheted reference height kept us there (kuluu-0nnl). No floor within
    // reach means hold our height for this tick. The old MZB wedge recovery
    // (kuluu-mo4q) is retired: the avian walker's move_and_slide depenetrates
    // overlap on every call, and the long-fall probe re-grounds on movement.
    // Note the server never corrects a bad z — it persists and echoes back
    // whatever c2s 0x015 sends.
    //
    // Horizontal movement: wall collision is client-side against the MZB wall
    // triangles (kuluu-q5sn). This tick's displacement (including forced
    // turns) is clamped, axis-separated, BEFORE it feeds prediction and c2s
    // 0x015, so the wire never carries a position inside a wall: the server
    // persists whatever we send (kuluu-mo4q). The navmesh still gates nothing
    // here (mob-pathing only). Suppressed interior shells are walked past
    // inside the query, so a shop doorway does not become a wall
    // (kuluu-vbpt follow-up).
    let wall_dx = x - basis_pos.x;
    let wall_dy = y - basis_pos.y;
    let mut det_out: Option<StairDetection> = None;
    let mut door_ent_out: Option<Entity> = None;
    let clip = if !env.hud_panels.noclip && (wall_dx != 0.0 || wall_dy != 0.0) {
        super::avian_bridge::resolve_position(
            &avian,
            &mut locals.push_through,
            basis_pos.x,
            basis_pos.y,
            basis_pos.z,
            wall_dx,
            wall_dy,
            time.delta_secs(),
            &mut det_out,
            &mut door_ent_out,
        )
    } else {
        kuluu_render::dat_mzb::WallClipResult::none(wall_dx, wall_dy)
    };
    // Store the one stair detection this tick (if resolve_position ran). asp +
    // HUD read it from here instead of calling detect_stairs again.
    if let Some(d) = det_out {
        env.last_stair.0 = d;
    }
    // If a door blocked us, resolve its mesh/texture name + drawn state
    // (drawn is informational only now: the undrawn-passthrough is gone, so
    // it feeds the debug string and nothing else).
    let door_name: String = match door_ent_out {
        Some(e) => {
            // The avian door collider is a STANDALONE entity (world-baked verts,
            // identity transform) and carries no identity components itself.
            // Follow its DoorColliderSource link back to the occluder placement
            // that has MmbDebugInfo + ViewVisibility; fall back to the entity
            // itself for any collider still attached directly (old path).
            let ident = env.door_source.get(e).map(|s| s.0).unwrap_or(e);
            match env.mmb_names.get(ident) {
                Ok((info, gt, vis)) => {
                    let drawn = vis.get() as u8;
                    let p = gt.translation();
                    format!(
                        "mesh={} tex={} worldpos=({:+.1},{:+.1},{:+.1}) drawn={}",
                        info.asset_name, info.variant_name, p.x, p.y, p.z, drawn
                    )
                }
                Err(_) => "door(entity, no MmbDebugInfo)".to_string(),
            }
        }
        None => String::new(),
    };
    x = basis_pos.x + clip.dx;
    y = basis_pos.y + clip.dy;
    // Debug: record the orchestration verdict for the stair HUD's right column.
    env.orch_log
        .push(kuluu_render::hud::stair_debug::OrchDecision {
            valid: true,
            is_a_stop: clip.dbg_is_a_stop,
            stop_slope: clip.dbg_stop_slope,
            slope_angle: clip.dbg_slope_angle,
            stop_steps: clip.dbg_stop_steps,
            tall_wall: clip.dbg_tall_wall,
            step_slope: clip.dbg_step_slope,
            step_height: clip.dbg_step_height,
            stop_wall: clip.dbg_stop_wall,
            wall_height: clip.dbg_wall_height,
            stop_door: clip.dbg_stop_door,
            stop_mob: clip.dbg_stop_mob,
            soft_timer: clip.dbg_soft_timer,
            block_nx: clip.dbg_block_nx,
            block_ny: clip.dbg_block_ny,
            block_nz: clip.dbg_block_nz,
            reason: clip.dbg_reason,
            hit_x: clip.dbg_hit_x,
            hit_y: clip.dbg_hit_y,
            hit_z: clip.dbg_hit_z,
            start_x: basis_pos.x,
            start_z: -basis_pos.y,
        });
    // Stash the door name (if any) in a resource for the HUD to show.
    env.orch_log.last_door_name = door_name;
    let final_x = x;
    let final_y = y;
    // A validated step-up this tick owns the vertical snap: its landing floor is
    // exactly where we stand. Re-running ground_step from our old height would pick
    // the surface nearest that OLD height in the new column — for a stair with
    // ground under it (Bastok Mines 2026-08-23) that is the low slab behind the
    // riser we just crossed, undoing every step.
    let mut stepped_this_tick = clip.landed_floor.is_some();
    let final_z = match clip.landed_floor {
        Some(floor_bevy_y) => -floor_bevy_y,
        None => {
            // Step grace: after a validated step-up we walk at normal speed while
            // the floor comes up under us; hold its height meanwhile (ground_step
            // would drop us onto the low slab under a buried stair). Grounding at
            // or above that floor clears it; the tick count bounds airtime.
            let pending_now = env.collision.pending_floor.lock().take();
            stepped_this_tick |= pending_now.is_some();
            let stepped_z = env
                .collision
                .ground_step(
                    bevy::math::Vec2::new(final_x, -final_y),
                    -basis_pos.z,
                    kuluu_render::dat_mzb::MAX_GROUND_STEP_UP,
                )
                .map(|floor_bevy_y| -floor_bevy_y);
            match pending_now {
                Some((f, ticks)) => {
                    let f_wire = -f;
                    match stepped_z {
                        // Grounded at or below the grace floor: still walking over to
                        // it. Consume one tick — and let the count EXPIRE at zero:
                        // writing (f, 0) back is a fixed point that would hold the
                        // phantom height forever if the floor never arrives.
                        Some(sz) if sz >= f_wire - 1e-3 => {
                            let next = ticks.saturating_sub(1);
                            *env.collision.pending_floor.lock() = (next > 0).then_some((f, next));
                            f_wire
                        }
                        // Grounded higher (on it or on the next level up): let go.
                        _ => stepped_z.unwrap_or(f_wire),
                    }
                }
                None => stepped_z.unwrap_or(basis_pos.z),
            }
        }
    };

    // Stair-settle clamp (bounce fix): the tick after a step-up lands,
    // ground_step re-samples the tread and can come back a hair below the
    // face_top the landing used — the body pops up then dips, reading as a
    // bounce on every riser. While settling (a few ticks after any landed /
    // grace tick), swallow only TINY wire-z drops; real descents — ramps
    // steeper than the threshold-per-tick, walk-offs, the grace-expiry slab
    // drop — exceed it and pass through untouched.
    const STEP_SETTLE_TICKS: u8 = 6;
    const STEP_SETTLE_MAX_DROP: f32 = 0.08;
    if stepped_this_tick {
        locals.step_settle = STEP_SETTLE_TICKS;
    }
    let final_z = if locals.step_settle > 0 {
        locals.step_settle -= 1;
        // wire z grows downward: a small positive delta is the dip we swallow
        let drop = final_z - basis_pos.z;
        if drop > 0.0 && drop < STEP_SETTLE_MAX_DROP {
            basis_pos.z
        } else {
            final_z
        }
    } else {
        final_z
    };

    let _ = cmd_tx.0.try_send(AgentCommand::Move {
        x: final_x,
        y: final_y,
        z: final_z,
        heading,
    });

    prediction.pos = Vec3::new(final_x, final_y, final_z);
}

/// Render-Y smoother state. The ONE authority for the self mesh's vertical
/// render position. The avian walker's wire Y steps tread-to-tread
/// (mathematically correct for collision); the render Y is a rate-limited
/// follow of it, so treads become a continuous ramp. No march, no fitted
/// plane, no engagement gates — the ring/purple march above still runs but
/// feeds ONLY the stair-debug HUD and gizmos now.
#[derive(Default)]
pub struct PlaneState {
    last_render_y: Option<f32>,
}

/// Apply the local walker's predicted position directly to the IsSelf
/// Transform. Runs in FixedUpdate right after `dispatch_movement_system` so
/// the rendered player follows the walker deterministically at 60 Hz. Without
/// this, self.y is only ever updated by the navmesh overlay's incidental
/// ground snap or a zone change, so climbing stairs visibly stutters even
/// though the walker itself is computing final_z correctly per tick.
pub fn apply_self_prediction_system(
    prediction: Res<LocalPlayerPrediction>,
    collision: Res<kuluu_render::dat_mzb::MzbCollisionGeometry>,
    time: Res<Time<Fixed>>,
    last_stair: Res<LastStairDetection>,
    mut dbg: ResMut<FootprintDebug>,
    mut plane_state: Local<PlaneState>,
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
    // Preserve rotation â self_visual_yaw_system owns it.
    let mut target = kuluu_render::ffxi_to_bevy(wire);

    // Slope-smoothed rendered Y across stair treads (Option B, "Better B").
    // The walker's authoritative Y snaps tread-to-tread — mathematically correct
    // for collision, but visually a jackhammer. The rendered mesh Y is instead
    // the average ground height over an 8-point ring around the character's
    // footprint. On flat ground all samples hit the same floor and it equals
    // the center height (no change). On stairs the ring straddles two treads
    // so the average smoothly ramps between them as the character walks across
    // the boundary — the continuous "red diagonal" retail look. Works in any
    // direction (radial sampling), critical for sidling up huge stairs.
    //
    // radius 0.3 : smaller than the smallest tread depth (0.4), so samples sit
    //              on one tread on flat/aligned parts; big enough that any
    //              tread transition immediately puts samples on both sides.
    // reject 0.4 : one stair rise, so samples on adjacent treads always pass;
    //              anything further (sample fell off the stair onto other
    //              terrain) is discarded before averaging.
    // ceiling +2 : raycast searches DOWN from a bit above the character, so
    //              the first floor hit is the tread we're on, not one below.
    // -------------------------------------------------------------------
    // Three-ring staircase detector (retail red-line slope).
    //
    // Three concentric rings of 8 rays each at radii 0.5, 1.0, 1.5 (24 rays).
    // For each of the 8 compass bearings the 4 heights (center + 3 ring
    // samples at 0.0/0.5/1.0/1.5 from character) are least-squares fit to a
    // line — that bearing's local slope + confidence (R^2). A weighted
    // vector-sum over all 8 bearings yields a CONTINUOUS up-stairs direction
    // (not snapped to one of 8 bearings), then 5 more forward probes at
    // 2.0..4.0 along that direction refit the line over 9 points so long
    // staircases hold a consistent slope. Character Y rides the fit's
    // intercept — continuous by construction, no tread snap ever.
    // -------------------------------------------------------------------
    // 5 concentric rings inside the original 1.5 outer bound — denser
    // sampling for better slope detection without extending scan range.
    // Orchestration calls the detector -- it does not contain it. detect_stairs
    // answers "step? slope? what ground height?" from position+geom. The render
    // smoother below reads this; resolve_position (wire height) reads it too.
    // One detector, multiple readers; the HUD reads the same StairDetection.
    // Read the ONE detection computed by resolve_position this tick (word of
    // god). No second detect_stairs call. `collision` is still used elsewhere in
    // this system, so it stays a param.
    let _ = &collision;
    let __det = last_stair.0;
    let center_xz = __det.center_xz;
    let center_y_raw = __det.center_y;
    let sample_data = __det.sample_data;
    let ramp_near = __det.ramp_near;
    let ramp_far = __det.ramp_far;
    let ramp_locked = __det.ramp_locked;
    let best_slope = __det.best_slope;
    let best_conf = __det.best_conf;
    let fwd_probes_dbg = __det.fwd_probes_dbg;
    let purple_probes_arr = __det.purple_probes_arr;
    let purple_probe_count = __det.purple_probe_count;
    let purple_risers_arr = __det.purple_risers_arr;
    let purple_riser_count = __det.purple_riser_count;
    let purple_slope = __det.purple_slope;
    let purple_slope_up = __det.purple_slope_up;
    let march_first_riser_rel = __det.march_first_riser_rel;
    // ---- Render-Y smoother (the merge) ----
    // ONE smoothing authority between the avian walker's wire Y and the
    // rendered mesh Y. The walker steps tread-to-tread; the render follows at
    // a capped vertical rate, turning treads into a continuous ramp in BOTH
    // directions (climb and descent). Everything upstream (ring, purple
    // march, orbs, ramp fit) is diagnostics for the stair HUD only — it no
    // longer writes render state, so nothing is left to blink, disengage, or
    // warp. The camera consumes this Y via the interpolated Transform, so
    // vertical smoothing lives here and ONLY here.
    //
    // RATE: max sustained vertical speed on FFXI stairs is about
    // slope(0.286/0.5) * sprint(~7 y/s) ~= 4 y/s; 6.0 tracks that with margin
    // so the render never falls cumulatively behind, while a fresh 0.286
    // riser still spreads across ~3 ticks instead of one.
    // SNAP: deltas past this are teleports (zone line, /goto, long falls) —
    // gliding those would smear the mesh across the world; snap instead.
    const RENDER_Y_RATE: f32 = 6.0; // yalms per second
    const RENDER_Y_SNAP: f32 = 2.0; // yalms
    let rendered_y = match plane_state.last_render_y {
        Some(last) => {
            let diff = target.y - last;
            if diff.abs() > RENDER_Y_SNAP {
                target.y
            } else {
                let max_step = RENDER_Y_RATE * time.delta_secs();
                last + diff.clamp(-max_step, max_step)
            }
        }
        None => target.y,
    };
    plane_state.last_render_y = Some(rendered_y);
    let _ = march_first_riser_rel;
    let chosen_y = rendered_y;
    let avg_for_dbg = chosen_y;
    let slope_active = (chosen_y - center_y_raw).abs() > 0.05;
    target.y = rendered_y;

    let _ = best_conf;
    let _ = best_slope;
    *dbg = FootprintDebug {
        enabled: true,
        center_xz,
        center_y: center_y_raw,
        radius: 1.0, // R_3 (detector ring radius), inlined post-extraction
        sampled_points: sample_data,
        avg_y: avg_for_dbg,
        slope_active,
        fwd_probes: fwd_probes_dbg,
        ramp_locked,
        ramp_near_xz: ramp_near.0,
        ramp_near_y: ramp_near.1,
        ramp_far_xz: ramp_far.0,
        ramp_far_y: ramp_far.1,
        purple_probes: purple_probes_arr,
        purple_probe_count,
        purple_risers: purple_risers_arr,
        purple_riser_count,
        purple_slope: purple_slope.unwrap_or(f32::NAN),
        purple_slope_up: purple_slope_up.unwrap_or(f32::NAN),
    };

    // Preserve rotation — self_visual_yaw_system owns it.
    // Publish the tick's authoritative render position to the interpolation
    // buffer. interpolate_self_transform_system (RunFixedMainLoop) lerps
    // Transform.translation between prev and curr every render frame so the
    // chase camera sees smooth motion instead of stair-stepped 60Hz updates.
    //
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

/// The staircase detector's answer. Computed by `detect_stairs`, read by the
/// render-Y smoother (apply_self_prediction_system) AND the wire height
/// (resolve_position). One detector, multiple readers; the HUD reads it too.
#[derive(Clone, Copy)]
pub struct StairDetection {
    pub center_xz: bevy::math::Vec2,
    pub center_y: f32,
    pub sample_data: [(bevy::math::Vec2, f32, bool, i8); 60],
    pub ramp_near: (bevy::math::Vec2, f32),
    pub ramp_far: (bevy::math::Vec2, f32),
    pub ramp_locked: bool,
    pub best_slope: f32,
    pub best_conf: f32,
    pub fwd_probes_dbg: [(bevy::math::Vec2, f32); 11],
    pub purple_probes_arr: [(bevy::math::Vec2, f32); 60],
    pub purple_probe_count: usize,
    pub purple_risers_arr: [(bevy::math::Vec2, f32); 5],
    pub purple_riser_count: usize,
    pub purple_slope: Option<f32>,
    pub purple_slope_up: Option<f32>,
    pub march_first_riser_rel: Option<f32>,
}

/// The single stored result of the ONE detect_stairs call per tick. The
/// orchestration (resolve_position) computes it; apply_self_prediction_system
/// and the HUD READ it from here. Word of god: one detector, many readers, no
/// duplicate raycasting.
#[derive(bevy::prelude::Resource, Clone, Copy)]
pub struct LastStairDetection(pub StairDetection);

impl Default for LastStairDetection {
    fn default() -> Self {
        Self(detect_stairs_empty())
    }
}

/// A zeroed StairDetection for the resource's initial value (before the first
/// real detection lands).
fn detect_stairs_empty() -> StairDetection {
    StairDetection {
        center_xz: bevy::math::Vec2::ZERO,
        center_y: 0.0,
        sample_data: [(bevy::math::Vec2::ZERO, 0.0, false, 0i8); 60],
        ramp_near: (bevy::math::Vec2::ZERO, 0.0),
        ramp_far: (bevy::math::Vec2::ZERO, 0.0),
        ramp_locked: false,
        best_slope: 0.0,
        best_conf: 0.0,
        fwd_probes_dbg: [(bevy::math::Vec2::ZERO, f32::NAN); 11],
        purple_probes_arr: [(bevy::math::Vec2::ZERO, f32::NAN); 60],
        purple_probe_count: 0,
        purple_risers_arr: [(bevy::math::Vec2::ZERO, f32::NAN); 5],
        purple_riser_count: 0,
        purple_slope: None,
        purple_slope_up: None,
        march_first_riser_rel: None,
    }
}

/// Staircase detector, lifted verbatim from apply_self_prediction_system. Pure:
/// reads position + collision, returns the slope/ground answer.
pub fn detect_stairs(
    target: bevy::math::Vec3,
    collision: &kuluu_render::dat_mzb::MzbCollisionGeometry,
) -> StairDetection {
    const R_1: f32 = 0.4;
    const R_2: f32 = 0.7;
    const R_3: f32 = 1.0;
    const R_4: f32 = 1.3;
    const R_5: f32 = 1.5;
    // Samples per ring — 12 = 30° angular resolution (was 8 = 45°). Denser
    // ring sampling makes per-bearing slope fits more stable.
    const RING_SAMPLES: usize = 12;
    const FWD_START: f32 = 2.0;
    const FWD_STEP: f32 = 0.5;
    const FWD_COUNT: usize = 5;
    const STAIR_SLOPE_MIN: f32 = 0.20;
    const STAIR_SLOPE_MAX: f32 = 0.80;
    const CONF_MIN: f32 = 0.40; // R^2 threshold — low because a staircase fit through a step-shaped point cloud is inherently noisy vs a true line

    let center_xz = bevy::math::Vec2::new(target.x, target.z);
    let center_y_raw = collision
        .ground_raycast(center_xz, target.y + 2.0)
        .unwrap_or(target.y);

    // 12 bearings at 30° spacing (was 8 at 45°). Finer angular resolution
    // for detecting stair direction; generated procedurally.
    let mut bearings: [bevy::math::Vec2; RING_SAMPLES] = [bevy::math::Vec2::ZERO; RING_SAMPLES];
    for (i, b) in bearings.iter_mut().enumerate() {
        let angle = (i as f32) * std::f32::consts::TAU / (RING_SAMPLES as f32);
        *b = bevy::math::Vec2::new(angle.cos(), angle.sin());
    }

    // Sample all three rings. Store as [ring][bearing] = (world xz, y).
    let radii = [R_1, R_2, R_3, R_4, R_5];
    let mut ring: [[(bevy::math::Vec2, f32); RING_SAMPLES]; 5] =
        [[(bevy::math::Vec2::ZERO, f32::NAN); RING_SAMPLES]; 5];
    for (ri, r) in radii.iter().enumerate() {
        for (bi, b) in bearings.iter().enumerate() {
            let world_xz = center_xz + *b * *r;
            let y = collision
                .ground_raycast(world_xz, target.y + 2.0)
                .unwrap_or(f32::NAN);
            ring[ri][bi] = (world_xz, y);
        }
    }

    // Debug: pack the middle ring's samples into the existing 8-slot debug array.
    let mut sample_data: [(bevy::math::Vec2, f32, bool, i8); 60] =
        [(bevy::math::Vec2::ZERO, 0.0, false, 0i8); 60];
    for ri in 0..5 {
        for bi in 0..RING_SAMPLES {
            sample_data[ri * RING_SAMPLES + bi] = (ring[ri][bi].0, ring[ri][bi].1, false, 0i8);
        }
    }

    // Classify every ring sample as: valid same-tread (green, sd.2 = true),
    // valid stair band (sd.3 != 0, up or down), or invalid (red outlier / gray
    // lip, sd.2 = false && sd.3 == 0). Downstream calcs must skip the invalid
    // ones: they don't represent the player's tread nor a real stair riser,
    // so feeding them into slope fits, descent direction weights, or descent
    // detection pollutes the result.
    //
    // Same-tread first: |y - center_y_raw| <= 0.1 → green. Then per-bearing
    // radial chaining for bands: walk each bearing from the player OUTWARD
    // and assign band N only if the samples inward on the same bearing form a
    // valid climbing/descending chain (same-tread → band 1 → band 2 …). An
    // isolated patch at band height with a gap/wall between it and the player
    // gets no band and stays red.
    // Two-pass dynamic band classification.
    //
    // Band 1 is the *first stair step* — the player's own tread is called
    // "same-tread green" and is not itself numbered (though conceptually
    // green IS band 0 / the shared base). A real tread height H is
    // typically ~0.4 in this world, so a legit first step lands somewhere
    // in (0.18, 0.45]. Anything at or below 0.18 is a lip (gray downstream),
    // not a stair.
    //
    // Pass A — same-tread (green): |dy| ≤ 0.06.
    // Pass B — band 1 candidates: 0.18 < |dy| ≤ 0.45. Static range so we
    //          have something to measure H from on the first frame.
    // Measure — H = median |dy| across all band 1 candidates. Falls back
    //          to 0.4 when no candidates exist yet.
    // Pass C — band N for N ≥ 2: |dy| ∈ ((N - 0.5) * H, (N + 0.5) * H].
    //          Ranges scale with the measured tread height so a shallow
    //          0.30 staircase doesn't get rounded up into a 0.40+ world.
    //
    // The per-bearing radial chaining pass then runs the same as before:
    // outward from the player, band N is only kept if the sample inward
    // on the same bearing is band N-1 (or the player's tread).
    const GREEN_TOL: f32 = 0.06;
    // Band-1 window: (LIP_MAX, 0.4] — above the lip cutoff up to a full
    // riser. B1_LO == LIP_MAX so no |dy| falls into an unclassified gap
    // between gray and purple; a sample at exactly LIP_MAX is still a lip.
    const B1_LO: f32 = LIP_MAX;
    // Upper bound stays 0.45 (not 0.4): real 0.4 risers measure up to ~0.43
    // in collision data, and capping at exactly 0.4 would push noisy 0.4
    // steps out of band 1 into red — the same bug as the old 0.2 lower bound.
    const B1_HI: f32 = 0.45;
    const H_FALLBACK: f32 = 0.4;

    // Same-tread pass.
    for ri in 0..5 {
        for bi in 0..RING_SAMPLES {
            let slot = ri * RING_SAMPLES + bi;
            let sd = &mut sample_data[slot];
            if sd.1.is_nan() {
                continue;
            }
            if (sd.1 - center_y_raw).abs() <= GREEN_TOL {
                sd.2 = true;
            }
        }
    }

    // Band 1 candidate pass — collect |dy| for every non-green sample
    // whose absolute drop/rise falls in the static band 1 window.
    let mut b1_dys: Vec<f32> = Vec::with_capacity(60);
    for ri in 0..5 {
        for bi in 0..RING_SAMPLES {
            let slot = ri * RING_SAMPLES + bi;
            let sd = &sample_data[slot];
            if sd.1.is_nan() || sd.2 {
                continue;
            }
            let ady = (sd.1 - center_y_raw).abs();
            // Strictly above the lip cutoff — a sample at exactly LIP_MAX is
            // still a lip, and lips must not feed the H measurement.
            if ady > B1_LO && ady <= B1_HI {
                b1_dys.push(ady);
            }
        }
    }
    // Median-of-candidates → H. Median rather than mean so one bad
    // sample can't drag the tread height around.
    let h_step: f32 = if b1_dys.is_empty() {
        H_FALLBACK
    } else {
        b1_dys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        b1_dys[b1_dys.len() / 2]
    };

    // Dynamic band classifier. Band 1 uses the static window (that's the
    // window we measured H in). Bands 2..=5 use half-step windows around
    // integer multiples of H, so a sample at |dy| ≈ N*H lands cleanly in
    // band N even for shallow or steep staircases.
    let band_of = |d: f32| -> i8 {
        let ad = d.abs();
        // Strictly above the lip cutoff — a sample at exactly LIP_MAX is
        // still a lip (gray), not a step.
        if ad > B1_LO && ad <= B1_HI {
            return if d > 0.0 { 1 } else { -1 };
        }
        for n in 2..=5i8 {
            let nf = n as f32;
            let lo = (nf - 0.5) * h_step;
            let hi = (nf + 0.5) * h_step;
            if ad > lo && ad <= hi {
                return if d > 0.0 { n } else { -n };
            }
        }
        0
    };

    // Per-bearing radial chaining pass.
    for bi in 0..RING_SAMPLES {
        let mut prev_level: i8 = 0; // 0 = player's tread level
        for ri in 0..5 {
            let slot = ri * RING_SAMPLES + bi;
            let sd = &mut sample_data[slot];
            if sd.1.is_nan() {
                prev_level = 0;
                continue;
            }
            if sd.2 {
                // Same-tread: re-anchors the chain at the player's tread.
                prev_level = 0;
                continue;
            }
            let d = sd.1 - center_y_raw;
            let this_band = band_of(d);
            if this_band == 0 {
                // In the "lip" dead zone (small |d| but not same-tread) or
                // above all band ranges. No band; break the chain so nothing
                // further out rides on it.
                prev_level = 0;
                continue;
            }
            let continues = if this_band > 0 {
                prev_level >= 0 && this_band <= prev_level + 1
            } else {
                prev_level <= 0 && this_band >= prev_level - 1
            };
            if continues {
                sd.3 = this_band;
                prev_level = this_band;
            } else {
                prev_level = 0;
            }
        }
    }

    // Per-bearing least-squares fit y = slope*x + intercept over the 4 points
    // (0, r_inner, r_mid, r_outer). Compute slope and R^2.
    fn fit4(xs: &[f32], ys: &[f32]) -> Option<(f32, f32, f32)> {
        let valid: Vec<(f32, f32)> = xs
            .iter()
            .zip(ys.iter())
            .filter(|(_, y)| !y.is_nan())
            .map(|(x, y)| (*x, *y))
            .collect();
        if valid.len() < 3 {
            return None;
        }
        let n = valid.len() as f32;
        let sx: f32 = valid.iter().map(|p| p.0).sum();
        let sy: f32 = valid.iter().map(|p| p.1).sum();
        let sxx: f32 = valid.iter().map(|p| p.0 * p.0).sum();
        let sxy: f32 = valid.iter().map(|p| p.0 * p.1).sum();
        let denom = n * sxx - sx * sx;
        if denom.abs() < 1e-6 {
            return None;
        }
        let slope = (n * sxy - sx * sy) / denom;
        let intercept = (sy - slope * sx) / n;
        // R^2 against horizontal mean.
        let mean_y = sy / n;
        let ss_tot: f32 = valid.iter().map(|p| (p.1 - mean_y).powi(2)).sum();
        let ss_res: f32 = valid
            .iter()
            .map(|p| (p.1 - (slope * p.0 + intercept)).powi(2))
            .sum();
        let r2 = if ss_tot > 1e-6 {
            1.0 - ss_res / ss_tot
        } else {
            1.0
        };
        Some((slope, intercept, r2))
    }

    // ==== VALIDATED DATASET ====
    // After classification, this is the ONLY dataset downstream calcs should
    // touch. If a sample is red (band == 0 && !same_tread) or gray (small
    // |dy| but not same-tread), it never lands here. Consumers below MUST NOT
    // reach back into the raw `ring` array.
    // Layout per sample: (ring_index, bearing_index, world_xz, y, band).
    //   band > 0  = up-stair band
    //   band < 0  = down-stair band
    //   band == 0 = same-tread (green); still valid, feeds slope fits.
    // Fixed-size backing array to avoid a Vec allocation each frame.
    let mut valid_samples: [(usize, usize, bevy::math::Vec2, f32, i8); 60] =
        [(0, 0, bevy::math::Vec2::ZERO, f32::NAN, 0); 60];
    let mut valid_count: usize = 0;
    for ri in 0..5 {
        for bi in 0..RING_SAMPLES {
            let slot = ri * RING_SAMPLES + bi;
            let sd = &sample_data[slot];
            if sd.1.is_nan() {
                continue;
            }
            let is_valid = sd.2 || sd.3 != 0;
            if !is_valid {
                continue;
            }
            valid_samples[valid_count] = (ri, bi, sd.0, sd.1, sd.3);
            valid_count += 1;
        }
    }
    // Bearing-indexed view: valid_by_bearing[bi] holds up to 5 (ri, y) pairs
    // for that bearing, in ring order. NaN = ring slot invalid on this
    // bearing. Used by the per-bearing slope fits below.
    let mut valid_by_bearing: [[f32; 5]; RING_SAMPLES] = [[f32::NAN; 5]; RING_SAMPLES];
    for &(ri, bi, _xz, y, _b) in valid_samples.iter().take(valid_count) {
        valid_by_bearing[bi][ri] = y;
    }

    // For each bearing, fit the 6-point profile [center, r1, r2, r3, r4, r5].
    let xs = [0.0f32, R_1, R_2, R_3, R_4, R_5];
    let mut bearing_slopes: [Option<(f32, f32)>; RING_SAMPLES] = [None; RING_SAMPLES];
    for bi in 0..RING_SAMPLES {
        let ys = [
            center_y_raw,
            valid_by_bearing[bi][0],
            valid_by_bearing[bi][1],
            valid_by_bearing[bi][2],
            valid_by_bearing[bi][3],
            valid_by_bearing[bi][4],
        ];
        // Require at least 4 non-NaN samples so a mostly-red bearing (which
        // has only 1-2 valid samples) can't fit a near-perfect line by
        // accident and pull the vector-sum sideways.
        let non_nan = ys.iter().filter(|y| !y.is_nan()).count();
        if non_nan < 4 {
            continue;
        }
        if let Some((slope, _int, r2)) = fit4(&xs, &ys) {
            bearing_slopes[bi] = Some((slope, r2));
        }
    }

    // Weighted vector-sum of bearings using |slope| * r2 as weight. The
    // accumulator should point TOWARD whichever direction has stair-shaped
    // geometry, whether the stairs go up (positive slope along that bearing)
    // or down (negative slope). Using signed slope makes acc point away from
    // a pure-down staircase and into the flat side, which then fails the fit.
    // Using |slope| collapses both cases correctly: acc points at the
    // detected staircase axis, and the refit determines up vs down from the
    // sign of the final slope.
    let mut acc = bevy::math::Vec2::ZERO;
    let mut best_conf: f32 = 0.0;
    let mut best_slope: f32 = 0.0;
    let mut any_qualifies = false;
    for bi in 0..RING_SAMPLES {
        if let Some((slope, r2)) = bearing_slopes[bi] {
            if r2 >= CONF_MIN && slope.abs() >= STAIR_SLOPE_MIN && slope.abs() <= STAIR_SLOPE_MAX {
                any_qualifies = true;
                acc += bearings[bi] * slope.abs() * r2;
                let conf = r2 * slope.abs();
                if conf > best_conf {
                    best_conf = conf;
                    best_slope = slope;
                }
            }
        }
    }

    let mut _ramp_y: Option<f32> = None;
    let mut ramp_locked = false;
    let mut fwd_probes_dbg: [(bevy::math::Vec2, f32); 11] =
        [(bevy::math::Vec2::ZERO, f32::NAN); 11];
    let mut ramp_near = (center_xz, center_y_raw);
    let mut ramp_far = (center_xz, center_y_raw);
    let mut up_dir = bevy::math::Vec2::ZERO;
    // (Dead now the lock is gone; kept only because the descent/ascent march
    // still writes into them. Prefixed with _ to silence unused warnings.)
    let mut _up_dir_slope_override: Option<f32> = None;
    let mut _purple_march_dir: bevy::math::Vec2 = bevy::math::Vec2::ZERO;
    // Along-distance of the first MEASURED riser -- used by the render step.
    let mut march_first_riser_rel: Option<f32> = None;
    let mut _march_last_riser_rel: Option<f32> = None;

    if any_qualifies && acc.length_squared() > 1e-6 {
        // Continuous up-stairs direction (not snapped to 8 bearings).
        up_dir = acc.normalize();
        // Fit forward and backward sides INDEPENDENTLY. A single line across
        // both sides can't describe a landing (stairs on one side, flat on
        // the other) because it isn't a line. Fitting each side separately
        // lets a landing lock on the stair side alone, and lets a continuous
        // staircase lock both sides with matching slopes.
        //
        // ONLY include samples that differ in height from the character by
        // more than half a riser (|y - center_y_raw| >= 0.15). Samples at the
        // character's own tread height contribute (x, 0) points that anchor
        // the regression at zero slope, pulling the fit shallow. On a landing
        // where the character is at the top of a staircase, half the "forward"
        // ring is still on the top tread — including those flat samples in
        // the fit drops the computed slope below STAIR_SLOPE_MIN even though
        // the descending samples are correctly stepped. Filter them out.
        // Center (0, center_y) still anchors the intercept, but as x=0 it
        // doesn't skew the slope.
        let same_tread = |y: f32| (y - center_y_raw).abs() < 0.15;
        let mut fwd_pts: Vec<(f32, f32)> = vec![(0.0, center_y_raw)];
        let mut back_pts: Vec<(f32, f32)> = vec![(0.0, center_y_raw)];
        // Read from the validated dataset only. Red samples never made it in.
        // Skip same-tread (band == 0 but green) samples too — they anchor at
        // 0-slope and pull the fit shallow on landings; only actual banded
        // stair samples feed the slope.
        for &(_ri, _bi, xz, y, band) in valid_samples.iter().take(valid_count) {
            if band == 0 {
                continue;
            } // same-tread, skip for slope
            if same_tread(y) {
                continue;
            } // belt-and-suspenders check
            let dx = xz - center_xz;
            let proj = dx.dot(up_dir);
            if proj > 0.0 {
                fwd_pts.push((proj, y));
            } else if proj < 0.0 {
                // Store backward samples with positive x (distance from
                // center) so each side's fit uses the same coordinate
                // convention: x = distance out from character, y = ground.
                back_pts.push((-proj, y));
            }
        }
        // Bidirectional forward-probe: 11 points at [-5..=5] along up_dir.
        // Forward probes go into fwd_pts; backward probes go into back_pts
        // (with sign flipped on their distance). Same-tread filter applies:
        // a probe that lands on the character's landing doesn't reveal
        // staircase shape.
        for (i, slot) in fwd_probes_dbg.iter_mut().enumerate() {
            let dist = -5.0 + i as f32;
            if dist.abs() < 0.5 {
                continue;
            }
            let probe_xz = center_xz + up_dir * dist;
            let y = collision.ground_raycast(probe_xz, center_y_raw + 2.0);
            *slot = (probe_xz, y.unwrap_or(f32::NAN));
            if let Some(y) = y {
                if same_tread(y) {
                    continue;
                }
                if dist > 0.0 {
                    fwd_pts.push((dist, y));
                } else {
                    back_pts.push((-dist, y));
                }
            }
        }
        // Fit each side.
        let fwd_fit = if fwd_pts.len() >= 4 {
            let xs: Vec<f32> = fwd_pts.iter().map(|p| p.0).collect();
            let ys: Vec<f32> = fwd_pts.iter().map(|p| p.1).collect();
            fit4(&xs, &ys)
        } else {
            None
        };
        let back_fit = if back_pts.len() >= 4 {
            let xs: Vec<f32> = back_pts.iter().map(|p| p.0).collect();
            let ys: Vec<f32> = back_pts.iter().map(|p| p.1).collect();
            fit4(&xs, &ys)
        } else {
            None
        };
        // A side qualifies if slope in stair range AND R² passes.
        let side_qualifies = |f: Option<(f32, f32, f32)>| -> Option<(f32, f32, f32)> {
            f.and_then(|(s, i, r)| {
                if r >= CONF_MIN && s.abs() >= STAIR_SLOPE_MIN && s.abs() <= STAIR_SLOPE_MAX {
                    Some((s, i, r))
                } else {
                    None
                }
            })
        };
        let fwd_q = side_qualifies(fwd_fit);
        let back_q = side_qualifies(back_fit);
        // Pick the higher-R² qualifying side. IMPORTANT: each side was fit
        // with x = distance out from character (positive on that side). So
        // back-side slope describes how Y changes as you walk BACKWARD along
        // up_dir. To draw the far endpoint we use the same-side's raw
        // (slope, intercept) with x = far_dist. The DIRECTION along up_dir
        // flips (forward = +up_dir, backward = -up_dir), but the SLOPE we
        // multiply by far_dist must stay in that side's own frame.
        // For best_slope (a "forward-relative" number downstream code reads),
        // we flip the back-side slope's sign so a "going down away from you"
        // reads negative regardless of which side.
        let (raw_slope, raw_intercept, _raw_r2, forward_side, any_side_qualified) =
            match (fwd_q, back_q) {
                (Some(f), Some(b)) => {
                    if f.2 >= b.2 {
                        (f.0, f.1, f.2, true, true)
                    } else {
                        (b.0, b.1, b.2, false, true)
                    }
                }
                (Some(f), None) => (f.0, f.1, f.2, true, true),
                (None, Some(b)) => (b.0, b.1, b.2, false, true),
                (None, None) => (0.0, 0.0, 0.0, true, false),
            };
        if any_side_qualified {
            _ramp_y = Some(raw_intercept);
            ramp_locked = true;
            let far_dist = FWD_START + FWD_STEP * (FWD_COUNT as f32 - 1.0);
            let dir_sign = if forward_side { 1.0 } else { -1.0 };
            ramp_near = (center_xz, raw_intercept);
            ramp_far = (
                center_xz + up_dir * far_dist * dir_sign,
                // Height at far_dist along whichever side we fit — raw_slope
                // is in that side's own x=distance-out frame, so no sign
                // flip on the slope here.
                raw_intercept + raw_slope * far_dist,
            );
            // best_slope is in the "forward-along-up_dir" frame for downstream
            // code: flip sign when we picked the backward side.
            best_slope = if forward_side { raw_slope } else { -raw_slope };
        }
    }

    // ---- Purple straight-down march: exact slope from vertical probes ----
    // The pink fan (above) gives a good DIRECTION (up_dir) but its slope math
    // is a least-squares fit through an angled sample fan, which produces a
    // wrong slope on descending stairs. This second pass measures the slope
    // EXACTLY: march straight-down raycasts every 0.1 along up_dir, record the
    // true ground height at each point, and find the risers (points where the
    // ground steps down by ~0.4). The horizontal distance between two risers
    // is the tread depth; the drop is the rise. rise / tread_depth = exact
    // slope. Only runs when a drop is already detected (a down-band exists),
    // so it's extra work only when there's actually a descent to measure.
    // Needs at least 2 risers: with only 1 step we skip everything and let the
    // player just drop off it (unnoticeable). Cap at 5 risers.
    //
    // Riser validity: a hard ~0.4 drop. Accept per-step drops in 0.2..=0.45.
    // A drop bigger than 0.45 between adjacent 0.1 samples is a cliff, not a
    // stair: stop the march there.
    let mut purple_probes_arr: [(bevy::math::Vec2, f32); 60] =
        [(bevy::math::Vec2::ZERO, f32::NAN); 60];
    let mut purple_probe_count: usize = 0;
    let mut purple_slope: Option<f32> = None;
    let mut purple_risers_arr: [(bevy::math::Vec2, f32); 5] =
        [(bevy::math::Vec2::ZERO, f32::NAN); 5];
    let mut purple_riser_count: usize = 0;
    // Measured tread Y at the first and last detected riser -- the real rise,
    // replacing the old hardcoded 0.4-per-riser assumption.
    let mut first_riser_y: f32 = 0.0;
    let mut last_riser_y: f32 = 0.0;
    // Detect a descent: any validated sample with a down-band. Reads only
    // from the validated dataset.
    let mut down_drop_detected = false;
    for s in valid_samples.iter().take(valid_count) {
        if s.4 < 0 {
            down_drop_detected = true;
            break;
        }
    }
    // Direction of the descent — weighted sum of DOWN-BAND sample bearings
    // from the validated dataset. Weight by band depth (|band| ∈ 1..=5).
    let mut descent_dir = bevy::math::Vec2::ZERO;
    for &(_ri, _bi, xz, _y, band) in valid_samples.iter().take(valid_count) {
        if band >= 0 {
            continue;
        }
        let dir = xz - center_xz;
        if dir.length_squared() > 1e-6 {
            descent_dir += dir.normalize() * (-band as f32);
        }
    }
    let march_dir = if descent_dir.length_squared() > 1e-6 {
        descent_dir.normalize()
    } else {
        up_dir
    };
    if march_dir.length_squared() > 0.5 && down_drop_detected {
        const STEP: f32 = 0.1;
        const MAX_MARCH: f32 = 6.0; // up to 6 units out (~5-6 treads)
        const RISER_MIN: f32 = 0.2;
        const RISER_MAX: f32 = 0.45;
        let n_steps = ((MAX_MARCH / STEP) as usize).min(60);
        // Record risers as we find them: (world_xz at the riser, along-distance).
        // A riser is a cumulative drop from the last tread level of >= RISER_MIN
        // that resolves within a short run. We track the "current tread height"
        // and watch for the ground dropping a full riser below it.
        let mut prev_y = center_y_raw;
        let mut current_tread_y = center_y_raw;
        // Pre-collect XZ positions of RED ring samples (band == 0, not
        // same-tread, not NaN). Red = the ring raycast returned data we
        // couldn't chain — often a wall-face hit rather than real ground. If
        // a purple probe lands near one of these, its raycast is likely
        // hitting the same bad geometry, so its height is unreliable and we
        // stop the march there rather than record a spurious riser.
        let mut red_xz: [bevy::math::Vec2; 60] = [bevy::math::Vec2::ZERO; 60];
        let mut red_count: usize = 0;
        for ri in 0..5 {
            for bi in 0..RING_SAMPLES {
                let slot = ri * RING_SAMPLES + bi;
                let sd = &sample_data[slot];
                if sd.1.is_nan() {
                    continue;
                }
                if sd.2 || sd.3 != 0 {
                    continue;
                }
                // Also skip gray "lip" samples (|d| <= LIP_MAX) — those
                // are real ground, not unreliable geometry. Only pure red counts.
                let dy = sd.1 - center_y_raw;
                if dy.abs() <= LIP_MAX {
                    continue;
                }
                if red_count < 60 {
                    red_xz[red_count] = sd.0;
                    red_count += 1;
                }
            }
        }
        const RED_PROX: f32 = 0.35; // reject probe if within this of a red sample
        for i in 1..=n_steps {
            let along = STEP * i as f32;
            let probe_xz = center_xz + march_dir * along;
            // Reject probe if it's near a red ring sample — its raycast is
            // going into the same unreliable geometry that flagged that
            // sample red. Break the march: we can't chain further out through
            // bad ground.
            let mut near_red = false;
            for r in red_xz.iter().take(red_count) {
                if probe_xz.distance_squared(*r) < RED_PROX * RED_PROX {
                    near_red = true;
                    break;
                }
            }
            if near_red {
                break;
            }
            let y = match collision.ground_raycast(probe_xz, center_y_raw + 2.0) {
                Some(y) => y,
                None => break, // ran off the geometry
            };
            if purple_probe_count < 60 {
                purple_probes_arr[purple_probe_count] = (probe_xz, y);
                purple_probe_count += 1;
            }
            // Drop between this sample and the previous 0.1 sample.
            let step_drop = prev_y - y; // positive = went down
            if step_drop > RISER_MAX {
                // Too steep for a single stair riser in one 0.1 step: cliff.
                break;
            }
            // Cumulative drop from the current tread level.
            let tread_drop = current_tread_y - y; // positive = below tread
            if (RISER_MIN..=RISER_MAX).contains(&tread_drop) {
                // Found a full riser: the ground settled one measured step
                // below the tread we were on. Record it AND its true Y and
                // treat this as the new tread.
                if purple_riser_count < 5 {
                    if purple_riser_count == 0 {
                        first_riser_y = y;
                    }
                    last_riser_y = y;
                    purple_risers_arr[purple_riser_count] = (probe_xz, along);
                    purple_riser_count += 1;
                }
                current_tread_y = y;
                if purple_riser_count >= 5 {
                    break;
                }
            } else if tread_drop > RISER_MAX {
                // Overshot a riser (dropped more than one step between tread
                // checks) — still count it as a riser and re-baseline, but
                // clamp the new tread to one riser down so a double-drop
                // doesn't desync the baseline. The recorded Y is the TRUE
                // measured ground either way.
                if purple_riser_count < 5 {
                    if purple_riser_count == 0 {
                        first_riser_y = y;
                    }
                    last_riser_y = y;
                    purple_risers_arr[purple_riser_count] = (probe_xz, along);
                    purple_riser_count += 1;
                }
                current_tread_y -= 0.4;
                if purple_riser_count >= 5 {
                    break;
                }
            }
            prev_y = y;
        }
        if purple_riser_count >= 2 {
            // Exact slope = MEASURED drop / measured run between the first and
            // last detected riser. The old code assumed every riser is 0.4
            // tall; real flights vary (Jeuno risers are ~0.286), which
            // inflated the slope ~40% and sank the plane through the stairs.
            let first = purple_risers_arr[0];
            let last = purple_risers_arr[purple_riser_count - 1];
            let run = last.1 - first.1; // along-distance
            let rise = first_riser_y - last_riser_y; // measured drop
            if run > 1e-3 && rise > 1e-3 {
                let s = rise / run; // magnitude; down = descending
                purple_slope = Some(s);
            }
            march_first_riser_rel = Some(first.1);
            _march_last_riser_rel = Some(last.1);
        }
    }
    // If the purple march measured a slope, it OVERRIDES the pink fit for the
    // lock: it's the exact rise/run, not a noisy regression. Force the ramp
    // lock on and set best_slope to the measured value (negative = down, since
    // this only runs when a down-drop was detected).
    if let Some(ps) = purple_slope {
        best_slope = -ps; // down
        ramp_locked = true;
        _up_dir_slope_override = Some(-ps);
        _purple_march_dir = march_dir;
        // Recompute the ramp gizmo line to follow the measured descent, or the
        // stale pink-fit endpoints (which can shoot off to the sky on a bad
        // down-fit) keep getting drawn. Near = player foot, far = along the
        // descent direction dropping at the measured slope.
        let far_dist = FWD_START + FWD_STEP * (FWD_COUNT as f32 - 1.0);
        ramp_near = (center_xz, center_y_raw);
        ramp_far = (
            center_xz + march_dir * far_dist,
            center_y_raw - ps * far_dist, // descending
        );
    }

    // ---------- Ascent march ----------
    // Mirror of the purple (descent) march for stairs the player is walking
    // UP into. Structure is deliberately parallel: detect the direction from
    // up-band samples, march outward at 0.1, watch for RISES of ~0.4 as
    // risers accumulate, compute rise/run.
    //
    // Deliberately does NOT touch best_slope, ramp_locked, up_dir_slope_override,
    // or the ramp gizmo — those are downhill lock machinery. The ascent slope
    // is a reporting-only measurement so the HUD stops showing `up=-` while
    // sitting on a bunch of up+1 samples.
    let mut purple_slope_up: Option<f32> = None;
    // Measured raycast Y at the first/last ascent riser, hoisted to fn scope
    // so the render step can anchor on it (mirrors first_riser_y for descent).
    let mut up_first_riser_y: f32 = 0.0;
    let mut up_last_riser_y: f32 = 0.0;
    let mut up_rise_detected = false;
    for s in valid_samples.iter().take(valid_count) {
        if s.4 > 0 {
            up_rise_detected = true;
            break;
        }
    }
    // Direction of the ascent — weighted sum of UP-BAND sample bearings from
    // the validated dataset. Weight by band height (|band| ∈ 1..=5).
    let mut ascent_dir = bevy::math::Vec2::ZERO;
    for &(_ri, _bi, xz, _y, band) in valid_samples.iter().take(valid_count) {
        if band <= 0 {
            continue;
        }
        let dir = xz - center_xz;
        if dir.length_squared() > 1e-6 {
            ascent_dir += dir.normalize() * (band as f32);
        }
    }
    let up_march_dir = if ascent_dir.length_squared() > 1e-6 {
        ascent_dir.normalize()
    } else {
        up_dir
    };
    if up_march_dir.length_squared() > 0.5 && up_rise_detected {
        const STEP: f32 = 0.1;
        const MAX_MARCH: f32 = 6.0;
        const RISER_MIN: f32 = 0.2;
        const RISER_MAX: f32 = 0.45;
        let n_steps = ((MAX_MARCH / STEP) as usize).min(60);
        let mut prev_y = center_y_raw;
        let mut current_tread_y = center_y_raw;
        // Same red-proximity guard as the descent march — if a probe lands
        // near a red ring sample, the raycast is going into the same
        // unreliable geometry, so we stop the march there.
        let mut red_xz: [bevy::math::Vec2; 60] = [bevy::math::Vec2::ZERO; 60];
        let mut red_count: usize = 0;
        for ri in 0..5 {
            for bi in 0..RING_SAMPLES {
                let slot = ri * RING_SAMPLES + bi;
                let sd = &sample_data[slot];
                if sd.1.is_nan() {
                    continue;
                }
                if sd.2 || sd.3 != 0 {
                    continue;
                }
                // Same gray-lip skip as the descent march: |d| <= LIP_MAX is
                // real ground (a lip), not unreliable geometry.
                let dy = sd.1 - center_y_raw;
                if dy.abs() <= LIP_MAX {
                    continue;
                }
                if red_count < 60 {
                    red_xz[red_count] = sd.0;
                    red_count += 1;
                }
            }
        }
        const RED_PROX: f32 = 0.35;
        let mut up_risers_arr: [(bevy::math::Vec2, f32); 5] =
            [(bevy::math::Vec2::ZERO, f32::NAN); 5];
        let mut up_riser_count: usize = 0;
        for i in 1..=n_steps {
            let along = STEP * i as f32;
            let probe_xz = center_xz + up_march_dir * along;
            let mut near_red = false;
            for r in red_xz.iter().take(red_count) {
                if probe_xz.distance_squared(*r) < RED_PROX * RED_PROX {
                    near_red = true;
                    break;
                }
            }
            if near_red {
                break;
            }
            // Look higher for the ascent raycast — a step ahead might be up
            // to 5 risers above us over 6 units of march, so start the ray
            // well above that.
            let y = match collision.ground_raycast(probe_xz, center_y_raw + 4.0) {
                Some(y) => y,
                None => break,
            };
            // Rise between this sample and the previous 0.1 sample.
            let step_rise = y - prev_y; // positive = went up
                                        // Ascent-specific: raycasts snap to tread tops, so a single 0.1
                                        // horizontal step can slightly overshoot a riser height when the
                                        // probe lands just past a tread edge. The descent uses RISER_MAX
                                        // (0.45) for its cliff gate; for ascent we allow a hair more so
                                        // a probe that lands ~0.47 above the previous doesn't kill the
                                        // march. Anything past 0.5 is a real wall.
            const ASCENT_CLIFF_MAX: f32 = 0.48;
            if step_rise > ASCENT_CLIFF_MAX {
                // Too tall for a single stair riser in one 0.1 step: wall.
                break;
            }
            // Cumulative rise from the current tread level.
            let tread_rise = y - current_tread_y; // positive = above tread
            if (RISER_MIN..=RISER_MAX).contains(&tread_rise) {
                if up_riser_count < 5 {
                    if up_riser_count == 0 {
                        up_first_riser_y = y;
                    }
                    up_last_riser_y = y;
                    up_risers_arr[up_riser_count] = (probe_xz, along);
                    up_riser_count += 1;
                }
                current_tread_y = y;
                if up_riser_count >= 5 {
                    break;
                }
            } else if tread_rise > RISER_MAX {
                if up_riser_count < 5 {
                    if up_riser_count == 0 {
                        up_first_riser_y = y;
                    }
                    up_last_riser_y = y;
                    up_risers_arr[up_riser_count] = (probe_xz, along);
                    up_riser_count += 1;
                }
                current_tread_y += 0.4;
                if up_riser_count >= 5 {
                    break;
                }
            }
            prev_y = y;
        }
        if up_riser_count >= 2 {
            let first = up_risers_arr[0];
            let last = up_risers_arr[up_riser_count - 1];
            let run = last.1 - first.1;
            // Measured climb (see the descent note — 0.4-per-riser is gone).
            let rise = up_last_riser_y - up_first_riser_y;
            if run > 1e-3 && rise > 1e-3 {
                purple_slope_up = Some(rise / run); // magnitude; up
            }
            // Only feed ramp bounds when the ascent march is driving the
            // lock this frame (descent takes precedence when both fire).
            if purple_slope.is_none() {
                march_first_riser_rel = Some(first.1);
                _march_last_riser_rel = Some(last.1);
            }
        }
    }

    // Wire the ascent march into the lock plumbing on frames where the
    // descent march did NOT produce a slope (i.e. the player is going up,
    // not down). Without this, `ramp_locked` never fires from ascent,
    // `detect_streak` never accumulates, the burst grid never runs, and
    // the render side never gets a smoothed plane on climbs. The ascent
    // slope is signed positive (going up), so up_dir_slope_override gets
    // +slope; up_dir stays the ascent march's measured direction.
    if purple_slope.is_none() {
        if let Some(us) = purple_slope_up {
            best_slope = us; // up
            ramp_locked = true;
            _up_dir_slope_override = Some(us);
            _purple_march_dir = up_march_dir;
            let far_dist = FWD_START + FWD_STEP * (FWD_COUNT as f32 - 1.0);
            ramp_near = (center_xz, center_y_raw);
            ramp_far = (
                center_xz + up_march_dir * far_dist,
                center_y_raw + us * far_dist, // ascending
            );
        }
    }

    StairDetection {
        center_xz,
        center_y: center_y_raw,
        sample_data,
        ramp_near,
        ramp_far,
        ramp_locked,
        best_slope,
        best_conf,
        fwd_probes_dbg,
        purple_probes_arr,
        purple_probe_count,
        purple_risers_arr,
        purple_riser_count,
        purple_slope,
        purple_slope_up,
        march_first_riser_rel,
    }
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
    fn heal_toggle_alternates_stance_and_wire_mode() {
        use crate::state::HealMode;
        use kuluu_render::combat_stance::{RestKind, RestStance};

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
            name_vis: 0,
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

/// Draws the footprint sampler debug: bright orange ring around the character
/// at radius `dbg.radius`, tiny spheres at each sample point (green kept, red
/// rejected), and a bright red disk at the averaged Y when slope smoothing is
/// active (i.e. we're crossing tread boundaries and the character Y is being
/// pulled off the raw ground). Runs per render frame (Update) since gizmos
/// aren't valid in FixedUpdate.
pub fn draw_footprint_debug_system(
    dbg: Res<FootprintDebug>,
    panels: Res<kuluu_render::hud::HudPanels>,
    mut gizmos: bevy::prelude::Gizmos,
) {
    use bevy::color::Color;
    use bevy::math::{Isometry3d, Quat, Vec3};
    if !dbg.enabled {
        return;
    }
    // User-facing menu toggle: Draw Stair Climber. When off, detection still
    // runs (so the character keeps climbing correctly) but no gizmos render.
    if !panels.stair_draw {
        return;
    }
    let center = Vec3::new(dbg.center_xz.x, dbg.center_y + 0.02, dbg.center_xz.y);
    // Five orange rings at 0.4/0.7/1.0/1.3/1.5 — denser sampling inside the
    // same 1.5 outer bound, better granularity for slope detection.
    let iso = Isometry3d::new(center, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2));
    gizmos.circle(iso, 0.4, Color::srgb(0.35, 0.18, 0.0));
    gizmos.circle(iso, 0.7, Color::srgb(0.55, 0.30, 0.0));
    gizmos.circle(iso, 1.0, Color::srgb(0.75, 0.42, 0.0));
    gizmos.circle(iso, 1.3, Color::srgb(0.90, 0.52, 0.0));
    gizmos.circle(iso, 1.5, Color::srgb(1.00, 0.60, 0.0));
    // Sample points: green kept, red rejected.
    for (xz, y, kept, band) in dbg.sampled_points.iter() {
        if y.is_nan() {
            continue;
        }
        let p = Vec3::new(xz.x, *y + 0.02, xz.y);
        // Up-bands (band > 0): purple ramp, brightening with step count.
        // Down-bands (band < 0): cyan ramp, brightening with step count.
        // Direction visible at a glance: purple stairs go UP away from you,
        // cyan stairs go DOWN away from you.
        let dy = *y - dbg.center_y;
        let color = if *band > 0 {
            let t = (*band as f32 - 1.0) / 4.0;
            let r = 0.55 + 0.35 * t;
            let g = 0.10 + 0.55 * t;
            let b = 1.00;
            Color::srgb(r, g, b)
        } else if *band < 0 {
            let t = ((-*band) as f32 - 1.0) / 4.0;
            let r = 0.05 + 0.20 * t;
            let g = 0.55 + 0.35 * t;
            let b = 0.85 + 0.15 * t;
            Color::srgb(r, g, b)
        } else if *kept {
            Color::srgb(0.2, 1.0, 0.2)
        } else if dy.abs() <= LIP_MAX {
            // Near-tread lip: no band, not same-tread, but at or below the
            // lip cutoff (LIP_MAX = 0.18). A curb, expansion joint, or a
            // broken decorative stair piece — not a real riser. Gray it out
            // so the display doesn't scream red for a non-issue.
            Color::srgb(0.55, 0.55, 0.55)
        } else {
            Color::srgb(1.0, 0.2, 0.2)
        };
        gizmos.sphere(Isometry3d::from_translation(p), 0.06, color);
    }
    // Bright red ring at the averaged Y when slope is active (retail-red).
    if dbg.slope_active {
        let avg_center = Vec3::new(dbg.center_xz.x, dbg.avg_y + 0.05, dbg.center_xz.y);
        let iso2 = Isometry3d::new(
            avg_center,
            Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
        );
        gizmos.circle(iso2, dbg.radius * 0.85, Color::srgb(1.0, 0.1, 0.1));
    }
    // Purple: the fitted ramp line + forward-probe hit points. Drawn whenever
    // we HAVE a fit (even when it wasn't locked), so we can see WHY a
    // detection didn't lock (line tilt out of range, misaligned, etc).
    if !dbg.ramp_near_xz.abs_diff_eq(dbg.ramp_far_xz, 1e-4) {
        let purple = if dbg.ramp_locked {
            Color::srgb(0.75, 0.15, 1.0) // bright locked
        } else {
            Color::srgb(0.35, 0.10, 0.55) // dim / speculative
        };
        let a = Vec3::new(
            dbg.ramp_near_xz.x,
            dbg.ramp_near_y + 0.08,
            dbg.ramp_near_xz.y,
        );
        let b = Vec3::new(dbg.ramp_far_xz.x, dbg.ramp_far_y + 0.08, dbg.ramp_far_xz.y);
        gizmos.line(a, b, purple);
        for (pxz, py) in dbg.fwd_probes.iter() {
            if py.is_nan() {
                continue;
            }
            let p = Vec3::new(pxz.x, *py + 0.05, pxz.y);
            gizmos.sphere(Isometry3d::from_translation(p), 0.05, purple);
        }
    }

    // (Removed: overhead diagnostic orbs — head lock_check status sphere,
    // down-bearing R²/slope pair, fwd-fit R²/slope pair, magnitude markers.
    // All of that info now shows numerically in the Stair Debug HUD panel,
    // so the overhead balls were pure visual noise.)

    // Purple straight-down march visualization. Each vertical probe hit is a
    // small purple dot at the true ground height. Detected risers are bright
    // magenta spheres. A polyline through the probe hits shows the measured
    // staircase profile — this is the EXACT geometry the slope is computed
    // from, so if the risers land on the real stair edges, the measurement
    // is correct.
    if dbg.purple_probe_count > 0 {
        let purple = Color::srgb(0.6, 0.1, 0.9);
        let mut prev: Option<Vec3> = None;
        for k in 0..dbg.purple_probe_count {
            let (xz, y) = dbg.purple_probes[k];
            if y.is_nan() {
                continue;
            }
            let p = Vec3::new(xz.x, y + 0.03, xz.y);
            gizmos.sphere(Isometry3d::from_translation(p), 0.03, purple);
            if let Some(pv) = prev {
                gizmos.line(pv, p, purple);
            }
            prev = Some(p);
        }
        let riser_col = Color::srgb(1.0, 0.2, 1.0);
        for k in 0..dbg.purple_riser_count {
            let (xz, _along) = dbg.purple_risers[k];
            // Draw the riser marker at the probe's ground height by finding the
            // matching probe; fall back to character height if not found.
            let mut y = dbg.center_y;
            for j in 0..dbg.purple_probe_count {
                let (pxz, py) = dbg.purple_probes[j];
                if pxz.abs_diff_eq(xz, 1e-3) {
                    y = py;
                    break;
                }
            }
            let p = Vec3::new(xz.x, y + 0.06, xz.y);
            gizmos.sphere(Isometry3d::from_translation(p), 0.10, riser_col);
        }
    }
}

/// Cache for the STAIR DEBUG zone/dat header lines: `DatRoot::from_env_or_default()`
/// touches the filesystem, so we only re-resolve when the effective zone key
/// (zone id + mog-house model) actually changes. Missing DAT root -> "?".
#[derive(Resource, Default)]
pub struct StairDebugZoneCache {
    /// Last effective key we resolved. `None` = never resolved yet.
    last_key: Option<(Option<u16>, Option<u16>)>,
    zone_name: String,
    zone_id: u16,
    dat_path: String,
}

/// Populates the shared StairDebugSnapshot from FootprintDebug each frame,
/// so the render crate's status panel (Show Stair Status) can display it
/// without pulling in a dependency on the input crate. Only runs when the
/// panel toggle is on — otherwise the snapshot stays as it was.
pub fn update_stair_debug_snapshot_system(
    dbg: Res<FootprintDebug>,
    panels: Res<kuluu_render::hud::HudPanels>,
    orch_log: Res<kuluu_render::hud::stair_debug::OrchDecisionLog>,
    scene: Res<kuluu_render::snapshot::SceneState>,
    mut zone_cache: ResMut<StairDebugZoneCache>,
    mut snap: ResMut<kuluu_render::hud::stair_debug::StairDebugSnapshot>,
) {
    use kuluu_render::hud::stair_debug::{OrbInfo, OrbTag};
    if !panels.stair_debug {
        return;
    }
    // Zone / DAT header: resolve on effective-zone-key change only. Effective
    // key = (zone_id, myroom model) so mog-house interiors show the interior's
    // DAT, matching effective_zone_file_id.
    let snap_ref = &scene.snapshot;
    let key = (snap_ref.zone_id, snap_ref.myroom.map(|m| m.model));
    if zone_cache.last_key != Some(key) {
        zone_cache.last_key = Some(key);
        zone_cache.zone_id = snap_ref.zone_id.unwrap_or(0);
        zone_cache.zone_name = snap_ref
            .zone_id
            .and_then(kuluu_nav::zone_name)
            .unwrap_or("")
            .to_string();
        zone_cache.dat_path = match (
            kuluu_render::snapshot::effective_zone_file_id(snap_ref),
            ffxi_dat::DatRoot::from_env_or_default().ok(),
        ) {
            (Some(fid), Some(root)) => root
                .resolve(fid)
                .ok()
                .map(|loc| {
                    format!(
                        "{}/{}/{}.DAT",
                        loc.rom_dir, loc.sub_path.dir, loc.sub_path.file
                    )
                })
                .unwrap_or_default(),
            _ => String::new(),
        };
    }
    snap.zone_id = zone_cache.zone_id;
    snap.zone_name = zone_cache.zone_name.clone();
    snap.dat_path = zone_cache.dat_path.clone();
    snap.drawing_enabled = panels.stair_draw;
    snap.player_xz = dbg.center_xz;
    snap.player_y = dbg.center_y;
    // Slopes: taken directly from the purple march measurements every frame.
    // (measured march is authoritative.)
    let (mut slope_up, mut slope_down) = (None, None);
    if !dbg.purple_slope.is_nan() && dbg.purple_slope > 0.0 {
        slope_down = Some(dbg.purple_slope);
    }
    if !dbg.purple_slope_up.is_nan() && dbg.purple_slope_up > 0.0 {
        slope_up = Some(dbg.purple_slope_up);
    }
    snap.slope_up = slope_up;
    snap.slope_down = slope_down;
    // Orchestration verdicts (last two ticks) for the right-hand column.
    snap.orch = orch_log.last_two;
    snap.door_name = orch_log.last_door_name.clone();
    // Per-orb classification. Same rules the renderer uses.
    let mut count_green = 0;
    let mut count_up = 0;
    let mut count_down = 0;
    let mut count_gray = 0;
    let mut count_red = 0;
    let mut orb_count = 0;
    for (xz, y, kept, band) in dbg.sampled_points.iter() {
        if y.is_nan() {
            continue;
        }
        let tag = if *band > 0 {
            count_up += 1;
            OrbTag::UpBand(*band)
        } else if *band < 0 {
            count_down += 1;
            OrbTag::DownBand(-*band)
        } else if *kept {
            count_green += 1;
            OrbTag::Green
        } else {
            let dy = *y - dbg.center_y;
            if dy.abs() <= LIP_MAX {
                count_gray += 1;
                OrbTag::Gray
            } else {
                count_red += 1;
                OrbTag::Red
            }
        };
        if orb_count < 60 {
            snap.orbs[orb_count] = OrbInfo {
                xz: *xz,
                y: *y,
                tag,
            };
            orb_count += 1;
        }
    }
    snap.count_green = count_green;
    snap.count_up = count_up;
    snap.count_down = count_down;
    snap.count_gray = count_gray;
    snap.count_red = count_red;
    snap.orb_count = orb_count;
}

// -----------------------------------------------------------------------------
// Stair-capture harness (FFXI_STAIR_DRIVE / FFXI_STAIR_CAPTURE) — rebuild #3.
// An external driver holds {-1,0,1} axes over a TCP JSON line; dispatch folds
// them into the real input pipeline, and `stair_capture_system` writes one JSON
// position sample per FixedUpdate tick while capturing. See docs/stair_capture.md
// for the protocol, run recipe and coordinate facts.
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
/// heading, purple-march slopes, derived up/down direction,
/// and gate diagnostics (active drive axes + dispatch early-return conditions)
/// so a frozen run can be diagnosed from the stream itself.
pub fn stair_capture_system(
    state: Res<SceneState>,
    prediction: Res<LocalPlayerPrediction>,
    dbg: Res<FootprintDebug>,
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
    // NaN means "the march produced no slope" — emit JSON null like run #2.
    let pslope_json = if dbg.purple_slope.is_nan() {
        String::from("null")
    } else {
        format!("{:.9e}", dbg.purple_slope)
    };
    let pslope_up_json = if dbg.purple_slope_up.is_nan() {
        String::from("null")
    } else {
        format!("{:.9e}", dbg.purple_slope_up)
    };

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
