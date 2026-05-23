//! Keyboard → `AgentCommand` for the native viewer.
//!
//! # Control model (camera-driven, FFXI default)
//!
//! ```text
//!   W/S       walk forward/back IN CAMERA DIRECTION (player heading
//!             snaps each tick to "away from camera" — ChaseCamera.yaw
//!             determines the move direction, not the player's prior heading).
//!   A/D       FFXI-classic "turn while walking" — rotates heading +
//!             camera yaw lock-step AND adds an implicit forward step
//!             in 3rd person, so holding either alone traces a circle
//!             (heading sweeps each tick, the forward vector sweeps
//!             with it). In first-person the walk implicit is dropped
//!             and A/D degenerates to pure rotate.
//!   Q/E       pure heading rotate (no walk contribution). Same camera
//!             yaw lock-step as A/D — the camera trails behind the
//!             rotated player. Useful for spinning in place to look
//!             around without orbiting. Unbound by default in Standard.
//!   ←/→       rotate camera yaw ONLY (free-look). Player heading
//!             unchanged until W/S press, which snaps it to camera-forward.
//!   ↑/↓       camera pitch (↑ raises camera/more overhead, ↓ lowers it).
//!   R         toggle autorun while forward is currently held.
//!   Tab       sweep targets left-to-right across the screen. First press
//!             picks the entity nearest the camera-frustum center; further
//!             presses step rightward through visible (plus slightly-out-
//!             of-view) entities. Mirrors FFXI retail.
//!   Esc       clear target selection (does NOT close the window).
//!   F8        toggle first-person camera. In FP, the cursor is locked
//!             (pointer-lock on web) and mouse-look (C3) drives heading 1:1.
//!             Keyboard A/D still rotates lock-step in either mode.
//!   ⌘Q ⌘W     close window (macOS quit / close-window shortcuts). Also
//!             responds to the OS window-close-requested event so the red
//!             traffic light works.
//! ```
//!
//! # Speed
//!
//! Per-tick step is derived from `state::Position::speed` via the wire
//! crate: `step = speed * 0.01` Bevy units (so the documented FFXI base
//! of speed=25 → 0.25/tick × 20 Hz = 5 u/s, matching `reactor.rs:50`'s
//! "FFXI base ~5 yalms/sec" comment). When the server populates speed
//! from `PosHead::speed`, modifiers (Bind/Quickening/etc.) flow through
//! automatically. Until then, `Position::default()` returns 25.
//!
//! # Autorun
//!
//! Phantom W. Toggled by R only when forward is currently held (FFXI:
//! tap-R from a standstill is a noop). Cancels: S press immediately, or
//! A/D held ≥ STRAFE_CANCEL_MS so a quick sidestep tap doesn't kill it.

use std::time::{Duration, Instant};

use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::window::WindowCloseRequested;
use ffxi_viewer_core::{
    heading_for_yaw, toggle_camera_mode, yaw_for_heading, Action, Bindings, CameraMode,
    ChaseCamera, ChatBuffer, CursorLockRequest, InputMode, IsSelf, LockOn, LockOnToggle,
    MenuStack, OperatorCamera, PassiveCursorState, SceneState, Target, WorldEntity,
};
use ffxi_viewer_wire::{Entity as WireEntity, Vec3 as WireVec3};
use tokio::sync::mpsc;

use crate::state::{ActionKind, AgentCommand};

/// Player A/D heading turn rate, **radians per second** of held key. Frame-
/// rate-independent so the dispatch tick rate (currently 60 Hz; see
/// `view_native::mod`) can change without retuning. 0.86 rad/s ≈ 49 °/s —
/// matches the retail FFXI feel of a visible but unhurried pivot (full
/// 180° in ~3.7 s) and keeps the older ROTATE_STEP_HELD=2-at-20-Hz target
/// of ~56°/s within ±13%. Camera yaw turns lock-step at the same rate so
/// the chase camera stays glued behind the player while turning in place.
pub const HEADING_TURN_RATE: f32 = 0.86;
/// Camera yaw delta per second when ←/→ held — same angular rate as
/// player rotation so free-look and steered turns feel comparable.
const CAMERA_YAW_RATE: f32 = HEADING_TURN_RATE;
/// Camera pitch delta per tick when ↑/↓ held. ~17 °/s @ 20 Hz — slow on
/// purpose so taps make small adjustments.
const PITCH_STEP_HELD: f32 = 0.015;
/// Sustained A/D hold required to cancel autorun. A brief tap (single
/// 50 ms tick) won't trip this; a held sidestep will.
const STRAFE_CANCEL_MS: u64 = 300;
/// Yalms-per-second contributed per unit of server-set speed. FFXI base
/// `speed = 25` gives 25 × 0.2 = 5 yalms/sec, matching the documented
/// "FFXI base ~5 yalms/sec" reactor comment. Step per tick is then
/// `speed * SPEED_TO_YPS * delta_secs` — frame-rate-independent so the
/// dispatch rate (currently 60 Hz; see `view_native::mod`) can change
/// without retuning movement speed.
const SPEED_TO_YPS: f32 = 0.2;

/// Retail movement-speed caps applied per direction (post reactor speed
/// scaling). Backing up is half-speed; pure strafe is three-quarters;
/// forward (with or without strafe) is full speed but the diagonal
/// forward+strafe vector is normalised so total magnitude stays ≤ 1×.
/// See https://ffxiclopedia.fandom.com/wiki/Category:Keyboard_Layout.
const BACKPEDAL_SCALE: f32 = 0.5;
const STRAFE_SCALE: f32 = 0.75;

/// Turn (A/D in 3rd person) — heading lerp rate toward direction of
/// motion, radians per real-time second.
///
/// Motion model: player strafes camera-perpendicular (A = camera-left,
/// D = camera-right). Heading lazily lerps toward the direction of
/// motion. Chase-camera yaw lazily lerps toward "behind player heading".
/// The two lerps create the orbit dynamics:
///
/// ```text
///   ω         = π/2 / (1/HLR + 1/CTR)   common angular velocity
///   lag_head  = ω / HLR                 radians player faces "back toward camera"
///   lag_chase = ω / CTR                 radians camera is off "directly behind"
///   orbit_r   = walk_speed / ω
/// ```
///
/// The two lags ALWAYS sum to π/2 (geometric constraint of the strafe
/// model). The split between them is what you tune:
///   - `HLR < CTR` (default): player faces partly back-toward-camera
///     (visible moonwalk-style turn); camera mostly behind. Matches the
///     "walk laterally, slowly rotating toward camera, camera chases
///     behind" intuition.
///   - `HLR > CTR`: player faces direction of motion (no moonwalk);
///     camera lags at flank.
///   - `HLR = CTR`: 50/50 split at 45° each.
///
/// At HLR=0.7, CTR=2.5 (shipped):
///   ω ≈ 0.86 rad/sec (heading rotates ~49°/sec, visible turn)
///   r ≈ 5.8 yalms
///   lag_head ≈ 70° (player faces mostly back toward camera)
///   lag_chase ≈ 20° (camera mostly behind, slightly off)
const HEADING_LERP_RATE_RAD_PER_SEC: f32 = 0.7;

/// Chase-camera yaw lerp rate toward "behind player heading", radians
/// per real-time second. See [`HEADING_LERP_RATE_RAD_PER_SEC`] for the
/// geometric trade-off and tuning notes.
const CHASE_TRACK_RATE_RAD_PER_SEC: f32 = 2.5;

/// Horizontal-distance threshold (yalms) above which `dispatch_movement_system`
/// abandons its local prediction and re-seeds from the snapshot. Sized for:
///   * normal per-frame divergence (≤ 1 frame's worth of movement at base speed,
///     so 0.083 yalm/frame at 60fps; even at 1fps that's 5 yalms — at the edge)
///   * legitimate server corrections (zone change, /warp, rubber-band): always
///     tens of yalms or more, so this catches them.
const PREDICTION_RESYNC_YALMS: f32 = 5.0;

#[derive(Resource, Clone)]
pub struct CommandTx(pub mpsc::Sender<AgentCommand>);

/// Autorun state. `phantom_forward` is the "is W virtually held?" flag
/// the dispatch system reads. `strafe_held_since` tracks how long A/D
/// have been continuously pressed, for the sustained-cancel rule.
#[derive(Resource, Default)]
pub struct AutoRun {
    pub phantom_forward: bool,
    pub strafe_held_since: Option<Instant>,
}

/// Fractional carry for the time-based A/D heading turn. Heading is a
/// u8 (256 units = 2π), but at 60 Hz dispatch the per-tick delta from a
/// finite turn rate (≈0.58 u8/tick at 0.86 rad/s) rounds to zero —
/// holding A/D would never accumulate enough to flip a unit. We keep a
/// signed f32 accumulator across ticks and only emit whole-unit
/// `wrapping_add` deltas when |accum| ≥ 1.0, draining the integer part
/// each time. Reset to 0 when no turn key is held so a paused press
/// doesn't replay leftover fraction.
#[derive(Resource, Default)]
pub struct HeadingTurnAccum {
    pub units: f32,
}

/// Advance the A/D heading-turn accumulator by one tick. Returns
/// `(integer_u8_delta_this_tick, float_u8_delta_this_tick)`.
pub fn advance_heading_turn(
    accum_units: &mut f32,
    dir: i32,
    dt_secs: f32,
) -> (i32, f32) {
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

/// Last position `dispatch_movement_system` emitted via `AgentCommand::Move`.
///
/// Why this exists: the system runs in `FixedUpdate` (60 Hz) but reads
/// `self_pos` from `state.snapshot`, which is refreshed at most once per
/// render frame. Without local prediction at /fps 30, both FixedUpdate
/// runs would see the same stale `self_pos`, halving walk speed.
#[derive(Resource, Default)]
pub struct LocalPlayerPrediction {
    pub pos: Vec3,
    pub initialized: bool,
}

/// Edge-trigger handler: window-close shortcuts, target/autorun/lock-on
/// toggles, and the Slash/Minus chat/menu opens (moved here from
/// `text_input.rs` so they're keyboard-layout-safe — see
/// `keybinds::mod` doc-comment).
///
/// Window-close runs *before* the [`InputMode`] gate so Cmd+Q / Cmd+W /
/// the red traffic light always work — even mid-chat-typing or with a
/// menu open. World-only actions only run when the user isn't focused in
/// some other UI (`text_input_system` owns most logical-key routing when
/// chat / menu / quick-action is active).
pub fn handle_input_system(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<Bindings>,
    mut window_close: MessageReader<WindowCloseRequested>,
    mut state: ResMut<SceneState>,
    cmd_tx: Res<CommandTx>,
    mut mode: ResMut<InputMode>,
    mut target: ResMut<Target>,
    mut autorun: ResMut<AutoRun>,
    mut camera_mode: ResMut<CameraMode>,
    mut chase: ResMut<ChaseCamera>,
    mut cursor_lock: ResMut<CursorLockRequest>,
    mut lock_on: ResMut<LockOn>,
    cam_q: Query<(&Camera, &Transform), With<OperatorCamera>>,
    mut exit: MessageWriter<AppExit>,
    mut rest_stance: ResMut<ffxi_viewer_core::combat_stance::RestStance>,
) {
    // Close-window: Cmd+Q, Cmd+W, or the OS-level WindowCloseRequested
    // event (red traffic light, App→Quit menu, etc.). Hard-wired (not
    // bindings-driven) — quitting must work in any input mode and isn't
    // an Action the user should ever rebind away.
    let cmd_held = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
    let close_shortcut =
        cmd_held && (keys.just_pressed(KeyCode::KeyQ) || keys.just_pressed(KeyCode::KeyW));
    let os_close = window_close.read().next().is_some();
    if close_shortcut || os_close {
        let _ = cmd_tx.0.try_send(AgentCommand::Disconnect);
        exit.write_default();
        return;
    }

    // First-person toggle. Default `V` (retail Compact 1), rebindable
    // via `Action::ToggleFirstPerson`. Runs in every InputMode *except*
    // `Chat` — the prior version was "unconditional" so the operator
    // could escape FP from any UI, but that meant typing `v` in chat
    // (e.g. inside `/endevent`, `/clearevt`, any word with a V) silently
    // toggled the camera mid-keystroke. Chat is the one mode where
    // keystrokes are text, not commands; every other UI (Menu, Dialog,
    // QuickAction, PassiveCursor) still passes V through to here, which
    // preserves the original "always escape FP" intent for non-text UIs.
    //
    // Cursor stays unlocked in FP: the OG client's FP didn't capture
    // the mouse, and our `mouse_camera_system` now gates FP look on
    // RMB-drag (with snap-back on release), so there's no need to
    // hide the cursor either.
    if !matches!(*mode, InputMode::Chat(_))
        && bindings.just_pressed(Action::ToggleFirstPerson, &keys)
    {
        toggle_camera_mode(&mut camera_mode, &mut chase);
        cursor_lock.locked = false;
    }

    // PassiveCursor toggle. Runs in BOTH directions and only from
    // World ↔ PassiveCursor — pressing the toggle key while in Chat /
    // Menu / Dialog / QuickAction is a no-op (the active UI takes
    // priority; user must Esc out first). The same key is the exit so
    // a single muscle-memory keypress always lands you back in World
    // from passive cursor.
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

    // Anything below is a world-mode action — let the text-input router
    // own these keys when a UI is focused.
    if !matches!(*mode, InputMode::World) {
        return;
    }

    // UI activation triggers — moved from `text_input.rs`'s logical-key
    // handler to keep them layout-safe. The triggering KeyboardInput
    // event still flows through `text_input_system` after the mode
    // change; for `/` it lands in handle_chat_key and appends `/` to the
    // (now Chat) buffer, reproducing the prior `with_prefix("/")` shape.
    // For `-` and Space, handle_chat_key/handle_menu_key either no-op or
    // produce the documented buffer state.
    if bindings.just_pressed(Action::OpenChatCommand, &keys) {
        *mode = InputMode::Chat(ChatBuffer::empty());
        return;
    }
    if bindings.just_pressed(Action::OpenMenu, &keys) {
        *mode = InputMode::Menu(MenuStack::root());
        return;
    }

    if bindings.just_pressed(Action::ClearTarget, &keys) {
        // FFXI: Esc deselects (target → none, menus close). No quit.
        target.id = None;
    }
    if bindings.just_pressed(Action::CycleTarget, &keys) {
        // Viewport-aware Tab: only entities currently inside the camera
        // frustum are candidates. First press picks the *nearest*; later
        // presses cycle left-to-right across the screen. Mirrors retail
        // FFXI better than "all targetable, sorted by distance" — Tab
        // shouldn't ever land on something behind the player or off-screen.
        if let Ok((camera, cam_t)) = cam_q.single() {
            let cam_global = GlobalTransform::from(*cam_t);
            target.id = cycle_target_viewport(
                &state.snapshot.entities,
                state.snapshot.self_pos.pos,
                target.id,
                state.snapshot.self_char_id,
                |world_pos| camera.world_to_ndc(&cam_global, world_pos),
            );
        }
    }

    // Retail F1–F6 party-slot targeting. Slot 1 = self (`self_char_id`);
    // slots 2..=6 index `snapshot.party[1..=5]` in insertion order — the
    // best the wire gives us, since the server's slot index isn't
    // broadcast separately. Empty slots silently no-op. Mirrors the
    // /targetparty<N> slash twins in `slash_commands.rs`.
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
            state
                .snapshot
                .party
                .get((slot - 1) as usize)
                .map(|p| p.id)
        };
        if let Some(id) = id {
            target.id = Some(id);
        }
    }
    if bindings.just_pressed(Action::ToggleAutorun, &keys) {
        // Tap-from-standstill is a no-op (FFXI parity). "Forward held"
        // means the rebound MoveForward key (KeyW under Compact 2,
        // Numpad8 under Standard) is currently down.
        if bindings.pressed(Action::MoveForward, &keys) {
            autorun.phantom_forward = !autorun.phantom_forward;
        }
    }
    if bindings.just_pressed(Action::ToggleEngage, &keys) {
        // Toggle: if engaged, cancel the reactor goal (server clears
        // combat via auto-attack-off semantics); otherwise engage the
        // current target. The reactor's first tick after Engage emits
        // ActionKind::Attack (0x01A subkind 0x02), then the server
        // drives auto-attack swings via 0x028 BATTLE2.
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
    // `Action::Sit` / `Action::Heal` press in World mode: toggle the
    // matching rest stance. Press to enter; press again (or any
    // movement-action press, handled in `dispatch_movement_system`)
    // to exit. Default unbound — operator binds via `/keybinds set`.
    //
    // Heal goes through the same `AgentCommand::Heal` channel as
    // the `/heal` slash so the server `EFFECT_HEALING` arms /
    // clears; `mirror_heal_stance` in `text_input` would normally
    // mirror that onto the resource, but the dispatcher doesn't run
    // for direct keypress paths, so we set `RestStance.kind`
    // ourselves here.
    if bindings.just_pressed(Action::Sit, &keys) {
        use ffxi_viewer_core::combat_stance::RestKind;
        let next = match rest_stance.kind {
            RestKind::Sit => RestKind::None,
            // Press Sit while healing → stand up *and* arm Heal::Off
            // so the server state catches up to the visual.
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
            // Toggling Heal from any non-Heal state arms it; the
            // server-side CAMP applies on the next outbound packet.
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

    // Auto-clear lock-on if the target despawned/zoned out so we don't
    // sit silently overriding heading toward a ghost id.
    if let Some(id) = lock_on.target_id {
        let still_visible = state.snapshot.entities.iter().any(|e| e.id == id);
        if !still_visible {
            lock_on.target_id = None;
        }
    }
}

/// Notify the server when the local `Target` changes. Centralizes what used
/// to be ad-hoc: Tab cycling, click-to-target, Esc deselect, and the
/// `/target Name` slash command all just mutate `Target.id`; this single
/// system catches every change and emits the corresponding 0x01A
/// `ChangeTarget` action (id 0x0F).
///
/// Without this, the server's notion of the player's target stays stale —
/// `/check`, /assist's "current target" semantics, action targeting
/// fallbacks, and any other target-aware verb would all misfire because
/// the server still thinks we're looking at whatever the last server-
/// initiated change was.
///
/// Deselect (Target.id → None) is sent as `target_id = 0, target_index =
/// 0`; Phoenix's `0x01a_action.cpp::process` treats id=0 as "no target",
/// matching retail behavior on Esc.
pub fn dispatch_target_change_system(
    target: Res<Target>,
    state: Res<SceneState>,
    cmd_tx: Res<CommandTx>,
    mode: Res<InputMode>,
) {
    if !target.is_changed() {
        return;
    }
    // Suppress the very first tick after world spawn — `Target::default()`
    // is `id: None`, and Bevy reports `is_changed()` for newly-inserted
    // resources. Sending a deselect on first frame would be a phantom
    // packet, and worse, would race with the lobby-handshake `InZone`
    // transition.
    if !matches!(
        *mode,
        InputMode::World
            | InputMode::Menu(_)
            | InputMode::QuickAction(_)
            | InputMode::PassiveCursor(_)
    ) {
        // Chat-mode target changes don't happen (the input router blocks
        // Tab/Esc), so this branch is mostly belt-and-suspenders.
        return;
    }

    let (target_id, target_index) = match target.id {
        Some(id) => match state.snapshot.entities.iter().find(|e| e.id == id) {
            Some(ent) => (id, ent.act_index),
            // Target points at an id we don't have in the snapshot (raced
            // with despawn). Skip — next snapshot tick will reconcile.
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

/// 20 Hz movement + camera-pitch/yaw dispatch. One Move command per tick
/// (or none if no inputs are active). Suspended while any non-`World`
/// [`InputMode`] is active so the player doesn't walk into a wall while
/// typing in chat or navigating a menu.
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
) {
    // Pause walking only when the operator's actively typing (Chat) or
    // making an event choice (Dialog). Menu and QuickAction overlays
    // navigate with arrow keys — those don't conflict with WASD movement,
    // and FFXI's quick-target picker historically stayed walkable. Esc
    // out of typing reliably resets autorun so a half-finished chat
    // session can't resume the player into a wall.
    if matches!(*mode, InputMode::Chat(_) | InputMode::Dialog(_)) {
        autorun.phantom_forward = false;
        autorun.strafe_held_since = None;
        return;
    }
    // Suppress arrow-key camera pitch/yaw while a Menu / QuickAction /
    // PassiveCursor is open so those keys steer the picker cursor (or
    // scroll the chat log, in PassiveCursor's case) instead of fighting
    // for the camera.
    let in_picker = matches!(
        *mode,
        InputMode::Menu(_) | InputMode::QuickAction(_) | InputMode::PassiveCursor(_)
    );

    // --- camera pitch: ↑ raises camera (more overhead), ↓ lowers it. ---
    // FP gets a wider clamp so the operator can mouse-/keyboard-look up
    // and down past horizontal; chase keeps the orbit-style range.
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

    // --- camera yaw: ←/→ orbit the camera (player unaffected). ---
    let mut yaw_d = 0.0;
    let yaw_step = CAMERA_YAW_RATE * time.delta_secs();
    if !in_picker && bindings.pressed(Action::CameraYawLeft, &keys) {
        yaw_d += yaw_step;
    }
    if !in_picker && bindings.pressed(Action::CameraYawRight, &keys) {
        yaw_d -= yaw_step;
    }
    if yaw_d != 0.0 {
        chase.yaw += yaw_d;
    }

    // --- camera zoom: `.` in, `,` out (defaults). Chase mode only —
    //     in FirstPerson there's no `distance` to step, and retail
    //     blocks the same keys. Held (`pressed`) and time-scaled so
    //     the rate is framerate-independent (`KEYBOARD_ZOOM_RATE`
    //     yalms/sec). Holding a key produces continuous smooth zoom;
    //     a quick tap produces ~1 fixed-tick step at 60 Hz.
    //
    //     Hover-gate: when the cursor is over the minimap, the same
    //     keys zoom the minimap instead (see
    //     `ffxi_viewer_core::minimap::input::handle_minimap_zoom_input`).
    //     Mirror-image of how `chat_wheel_scroll_system` consumes
    //     wheel events when hovering chat.
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

    // --- rest-stance gate (sit/heal) ---
    //
    // While the local self is in a rest stance, no movement Move
    // packets emit. Any *press* of a translation or rotation action
    // (W/S/A/D/Q/E and the bound Strafe* / Turn*) stands the player
    // up — that's the retail "stand on first input" behavior. The
    // press is observed via `Bindings::just_pressed` so the cancel
    // is edge-triggered; holding a key while transitioning into a
    // rest stance won't fight the new state.
    //
    // When Heal is cleared this way we also send `Heal::Off` on the
    // wire so the server's `EFFECT_HEALING` clears in the same tick
    // as the client visual. The session loop has its own
    // movement-detected auto-cancel (`session.rs:1936`), but that
    // only fires when the keepalive thread next runs *and* observes
    // a position delta — pre-empting it here keeps the visual /
    // wire state synced on the press, before any position actually
    // changes.
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
            // Fall through this tick to begin moving — the press
            // that stood us up is also a valid first frame of
            // locomotion, matching retail's behavior.
        } else {
            // Otherwise: full suppression. No Move emission this
            // tick, autorun stays off, prediction holds.
            autorun.phantom_forward = false;
            autorun.strafe_held_since = None;
            return;
        }
    }

    // --- forward / back ---
    let mut forward: i32 = 0;
    if bindings.pressed(Action::MoveForward, &keys) {
        forward += 1;
    }
    if bindings.pressed(Action::MoveBackward, &keys) {
        forward -= 1;
    }
    // Back-press immediately kills autorun (FFXI: backing out drops the lock).
    if bindings.just_pressed(Action::MoveBackward, &keys) {
        autorun.phantom_forward = false;
    }

    // --- rotate player (and chase camera lock-step). Time-based so the
    //     turn rate is framerate-independent: `HEADING_TURN_RATE` rad/s
    //     mapped onto the 256-unit FFXI heading scale. Holding A/D or
    //     Q/E sweeps the player heading at ~0.86 rad/s (~49°/s) — a
    //     visible pivot, not a snap. The fractional carry
    //     (`HeadingTurnAccum`) makes sub-unit per-tick deltas at 60 Hz
    //     accumulate into u8 ticks instead of rounding to zero every
    //     frame. Both `RotateLeft`/`Right` (Q/E) and `TurnLeft`/`Right`
    //     (A/D classic) feed the same accumulator; the A/D-implicit
    //     forward step is added separately below via `turn`. ---
    let mut player_rotate_dir: i32 = 0;
    if bindings.pressed(Action::RotateLeft, &keys) {
        player_rotate_dir -= 1;
    }
    if bindings.pressed(Action::RotateRight, &keys) {
        player_rotate_dir += 1;
    }
    if bindings.pressed(Action::TurnLeft, &keys) {
        player_rotate_dir -= 1;
    }
    if bindings.pressed(Action::TurnRight, &keys) {
        player_rotate_dir += 1;
    }
    let (player_rotate_u8, heading_delta_units) =
        advance_heading_turn(&mut turn_accum.units, player_rotate_dir, time.delta_secs());

    // --- strafe perpendicular to current heading. Unbound in every
    //     shipped preset after the A/D=turn / Q/E=rotate reshuffle —
    //     `pressed` returns false for unbound actions, so the strafe
    //     contribution is naturally zero. Kept as a rebindable verb so
    //     operators who want classic strafe can re-add it via
    //     `/keybinds set strafe-left ...`. ---
    let mut strafe: i32 = 0;
    if bindings.pressed(Action::StrafeLeft, &keys) {
        strafe -= 1;
    }
    if bindings.pressed(Action::StrafeRight, &keys) {
        strafe += 1;
    }

    // --- A/D-implicit forward step (classic FFXI orbit-while-walking).
    //     The heading rotation is already folded into `player_rotate_dir`
    //     above; `turn` here only contributes to forward motion. Suppressed
    //     when W/S already held or in first-person. ---
    let mut turn: i32 = 0;
    if bindings.pressed(Action::TurnLeft, &keys) {
        turn -= 1;
    }
    if bindings.pressed(Action::TurnRight, &keys) {
        turn += 1;
    }

    // Sustained pure-rotate (Q/E) or strafe hold cancels autorun. A/D
    // turn is intentionally excluded — orbit-while-autorun must keep autorun.
    let any_strafe_or_rotate =
        bindings.pressed(Action::RotateLeft, &keys)
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

    // Apply autorun: virtual W held.
    if autorun.phantom_forward {
        forward = forward.max(1);
    }

    // Fold `turn` based on camera mode. Must run AFTER the autorun-cancel
    // check (turn must not trigger cancel) and AFTER the autorun
    // phantom_forward expansion (so W+A doesn't double-count forward).
    //
    // 3rd person: motion is computed below in a dedicated strafe+lerp
    //   handler — player strafes camera-perpendicular (A=left, D=right)
    //   while heading lerps toward direction-of-motion and chase.yaw
    //   lerps toward "behind heading". The two lerps drive the orbit
    //   dynamics; see HEADING_LERP_RATE_RAD_PER_SEC.
    //
    // 1st person: no orbit visual to chase. Fold turn into player_rotate
    //   so A still rotates the player at the snappy spin-to-face rate
    //   via the standard per-tick u8 handler.
    let turn_in_chase = turn != 0 && matches!(*camera_mode, CameraMode::Chase);
    if turn != 0 && !turn_in_chase {
        player_rotate += turn;
    }

    // Lock-on heading override — computed before the no-input bail-out
    // so the camera pivots to follow the target even when the player
    // is standing still. Returns the new heading u8 if a usable target
    // is in the snapshot, else `None`.
    let self_pos = state.snapshot.self_pos;

    // Local prediction basis: when this system runs N times per render
    // frame (Bevy FixedUpdate catch-up at low /fps), every run after the
    // first must see the previously-emitted position — not the snapshot,
    // which only refreshes once per Update tick. Without this, multiple
    // FixedUpdate runs in one frame all compute from the same stale
    // `self_pos` and walk speed is pinned to one step per render frame.
    //
    // Snapshot wins on big divergence (zone change / /warp / server
    // snapback) — see [`PREDICTION_RESYNC_YALMS`]. Heading and speed are
    // never predicted locally; they're always snapshot-authoritative.
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
                    // LSB convention — mirror `heading_toward` in `reactor.rs`
                    // so lock-on and reactor-driven facing produce the same
                    // byte for the same geometry. `dy.atan2(dx)` matches LSB's
                    // `worldAngle(A, B) = atan2(B.z-A.z, B.x-A.x)`; the
                    // negative scale flips CCW to FFXI's CW.
                    let radians = dy.atan2(dx);
                    let raw = radians * -(128.0 / std::f32::consts::PI);
                    Some((raw.round() as i32).rem_euclid(256) as u8)
                }
            })
    });

    // Update camera yaw smoothly every tick while a rotate-direction key is
    // held (float domain — doesn't wait for the u8 heading-accumulator to
    // tick). Keeps the chase camera visibly glued behind the player
    // mid-pivot even on frames where the integer heading didn't advance.
    if player_rotate_dir != 0 {
        chase.yaw -= heading_delta_units * std::f32::consts::TAU / 256.0;
    }

    // Bail-out: nothing to send this tick. Fall through to the lock-on
    // heading-only branch if locked, or to `turn_in_chase` if A/D turn is
    // held in chase mode. Still emit when the u8 heading-accumulator
    // advanced so the server hears pure-turn motion.
    if forward == 0
        && strafe == 0
        && player_rotate_u8 == 0
        && !turn_in_chase
    {
        if let Some(h) = locked_heading {
            if h != self_pos.heading {
                chase.yaw = ffxi_viewer_core::yaw_for_heading(h);
                // Use basis_pos (prediction-authoritative) so a
                // heading-only Move doesn't yank a fresher local
                // position back to the lagging snapshot.
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

    // Compute heading. Two effects compose, in this order:
    //   1. If forward != 0: snap heading to camera-forward (the "reify
    //      inverse heading from camera" rule). This lets W/S walk in the
    //      direction the camera looks regardless of where the player was
    //      previously facing — FFXI third-person default.
    //   2. A/D rotation is applied AFTER the snap. Crucially, A/D rotates
    //      BOTH player heading AND camera yaw lock-step — so the camera
    //      stays fixed behind the player while turning in place, AND
    //      A/D actually steers the path during W-held movement (because
    //      yaw moved with heading, the next tick's snap is a no-op).
    //      ←/→ rotates ONLY camera yaw — that's the "free look" path,
    //      and W/S's snap is what makes the player follow camera direction.
    let mut heading = self_pos.heading;
    if forward != 0 {
        heading = heading_for_yaw(chase.yaw);
    }
    if player_rotate_u8 != 0 {
        let delta = player_rotate_u8.rem_euclid(256) as u8;
        heading = heading.wrapping_add(delta);
        // Note: camera yaw was already advanced smoothly above (float
        // domain, every tick). No second yaw delta here, otherwise the
        // camera would double-step on integer-tick frames and drift.
    }

    // Turn (3rd-person A/D) — strafe + heading-lerp + chase-lerp.
    //
    // Each tick:
    //   1. Compute strafe motion in camera-perpendicular direction
    //      (camera-left for A, camera-right for D). Composed with W/S
    //      and normalized so diagonals aren't faster than cardinals.
    //   2. Player heading lazily lerps toward direction-of-motion at
    //      HEADING_LERP_RATE_RAD_PER_SEC (the "slowly rotating" turn).
    //   3. chase.yaw lazily lerps toward "behind player heading" at
    //      CHASE_TRACK_RATE_RAD_PER_SEC (the "chase to be behind").
    //
    // The geometric constraint of the strafe model is `lag_h + lag_c = π/2`
    // — you can't have player face direction of motion AND camera behind
    // simultaneously. The default rates bias toward camera-behind, so the
    // player visibly turns "back toward the camera" as the chase pulls
    // around them. Motion stays lateral (camera-perpendicular) the
    // entire time.
    //
    // We stash the motion delta into `turn_dx`/`turn_dy` and apply it
    // after `x`/`y` are initialized from `basis_pos`. forward/strafe
    // are then gated off so the standard step handlers don't double-add.
    //
    // No-op in first-person: 1st-person folded turn into player_rotate
    // earlier; the standard rotate handler took care of it.
    let mut turn_dx: f32 = 0.0;
    let mut turn_dy: f32 = 0.0;
    if turn_in_chase {
        let camera_forward_h = heading_for_yaw(chase.yaw);
        let (cf_x, cf_y) = heading_to_forward(camera_forward_h);
        // LSB heading convention: +64 = +90° clockwise from above = "right".
        let (cr_x, cr_y) = heading_to_forward(camera_forward_h.wrapping_add(64));

        let fwd_signed = forward as f32;
        let lat_signed = turn as f32; // -1 = A (camera-left), +1 = D (camera-right).
        let mx = cf_x * fwd_signed + cr_x * lat_signed;
        let my = cf_y * fwd_signed + cr_y * lat_signed;
        let len = (mx * mx + my * my).sqrt();

        let step_magnitude = self_pos.speed as f32 * SPEED_TO_YPS * time.delta_secs();
        if len > 1e-3 && step_magnitude > 0.0 {
            let inv = 1.0 / len;
            turn_dx = mx * inv * step_magnitude;
            turn_dy = my * inv * step_magnitude;

            // Heading lerp toward direction of motion (NOT snapped — lag
            // is what lets chase.yaw catch up). atan2 → LSB worldAngle
            // formula matches `reactor.rs::heading_toward`.
            let motion_radians = my.atan2(mx);
            let motion_raw = motion_radians * -(128.0 / std::f32::consts::PI);
            let motion_h = (motion_raw.round() as i32).rem_euclid(256) as u8;

            let h_target = yaw_for_heading(motion_h);
            let h_current = yaw_for_heading(heading);
            let h_diff = wrap_signed_pi(h_target - h_current);
            let h_max_step = HEADING_LERP_RATE_RAD_PER_SEC * time.delta_secs();
            let h_step = h_diff.signum() * h_max_step.min(h_diff.abs());
            heading = heading_for_yaw(h_current + h_step);
        }

        // Chase-camera yaw lerps toward "behind player heading" (the
        // lerped value above). In steady state `lag_c = ω/CTR`.
        let chase_target = yaw_for_heading(heading);
        let c_diff = wrap_signed_pi(chase_target - chase.yaw);
        let c_max_step = CHASE_TRACK_RATE_RAD_PER_SEC * time.delta_secs();
        let c_step = c_diff.signum() * c_max_step.min(c_diff.abs());
        chase.yaw += c_step;

        // Suppress the standard forward/strafe step handlers — composite
        // motion above is the sole position update this tick.
        forward = 0;
        strafe = 0;
    }

    // Lock-on: heading already computed at the top of this function
    // (see `locked_heading` shadowed above). Apply it after WASD's
    // camera-forward snap and A/D rotation so movement intent still
    // composes — W walks toward the target, A/D shifts the
    // player→camera offset around the target axis. Yaw is also pinned
    // so the chase camera trails the locked player→target line.
    if let Some(h) = locked_heading {
        heading = h;
        chase.yaw = ffxi_viewer_core::yaw_for_heading(h);
    }

    // Step magnitude is time-based: `yalms/tick = speed * SPEED_TO_YPS *
    // dt`. `speed=0` (entity hasn't been populated yet) → 0 step, which
    // silently skips movement instead of teleporting somewhere weird.
    // Speed_base is the unmodified value.
    let raw_step = self_pos.speed as f32 * SPEED_TO_YPS * time.delta_secs();
    // Retail direction caps (applied after reactor speed scaling):
    //   * Backpedal (S only)            → 0.5×
    //   * Pure strafe (A/D only, no W/S) → 0.75×
    //   * Forward (W, with or without strafe) → 1.0×, but diagonal
    //     forward+strafe is normalised by 1/√2 so the combined vector
    //     magnitude doesn't exceed 1× forward speed.
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
    // Apply the turn handler's composite motion. Zero unless `turn_in_chase`
    // produced a step (forward/strafe were also gated off in that path).
    x += turn_dx;
    y += turn_dy;
    if forward != 0 && step > 0.0 {
        let (fwd_x, fwd_y) = heading_to_forward(heading);
        x += fwd_x * step * forward as f32;
        y += fwd_y * step * forward as f32;
    }
    if strafe != 0 && step > 0.0 {
        // Strafe-right = heading + 90° (clockwise viewed from above, FFXI
        // convention). Strafe-left is the negation.
        let right_heading = heading.wrapping_add(64);
        let (right_x, right_y) = heading_to_forward(right_heading);
        x += right_x * step * strafe as f32;
        y += right_y * step * strafe as f32;
    }

    // Wall-slide: if a navmesh is loaded for this zone, ask Detour
    // to clamp the proposed step. `slide_along` returns the input
    // unchanged when the start position isn't on any poly (player
    // off-mesh), and the move passes through. If anything fails the
    // raw target is used — wall-slide should never *break* movement.
    let (final_x, final_y, final_z) = if let Some(nav) = &navmesh.nav {
        let from = ffxi_nav::glam::Vec3::new(basis_pos.x, basis_pos.y, basis_pos.z);
        let to = ffxi_nav::glam::Vec3::new(x, y, basis_pos.z);
        let slid = nav
            .lock()
            .ok()
            .and_then(|guard| guard.slide_along(from, to));
        // TEMP: stuck-on-geometry probe. Log when WASD proposed a real
        // step (≥0.1 yalm) but the slide produced near-zero progress
        // (<0.1 yalm) — that's the "stuck" symptom. Cases distinguished:
        //   slide=None       → start off-mesh (would pass through, not stick)
        //   slide=Some(p≈from) → clamped to a single-poly cell (real stuck)
        // Remove once the wall-slide regression is diagnosed.
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

    let _ = cmd_tx.0.try_send(AgentCommand::Move {
        x: final_x,
        y: final_y,
        z: final_z,
        heading,
    });

    // Commit prediction so the next FixedUpdate run (possibly within
    // this same render frame) reads our just-emitted position rather
    // than the still-lagging snapshot.
    prediction.pos = Vec3::new(final_x, final_y, final_z);
}

/// FFXI heading 0..=255 → (forward.x, forward.y) unit vector in our
/// horizontal plane. LSB convention (matches `heading_toward` in
/// `reactor.rs`): heading 0 = +x (east), 64 = south, 128 = west, 192 =
/// north, CW from above. With `angle = h·τ/256`, the +x component is
/// `cos(angle)` and the +y component is `-sin(angle)` because FFXI
/// rotates clockwise while math `atan2` is CCW positive.
fn heading_to_forward(heading: u8) -> (f32, f32) {
    let angle = (heading as f32) * std::f32::consts::TAU / 256.0;
    (angle.cos(), -angle.sin())
}

/// Map an angle difference into `[-π, π]` so the turn handler's heading
/// and chase-yaw lerps always take the shortest arc around the modular
/// τ boundary.
#[inline]
fn wrap_signed_pi(x: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (x + PI).rem_euclid(TAU) - PI
}

/// Pure helper: viewport-aware Tab cycle, FFXI-retail style.
///
/// `project` maps an FFXI world position to NDC (`[-1, 1]` x/y; z `[0, 1]`
/// for in-front-of-camera, outside that range = behind / clipped).
/// Returns `None` when the math fails (camera at the same point as the
/// entity, etc.) — those entities are silently dropped.
///
/// Cycle behavior:
/// - First press (no current target, or current target is off-screen):
///   pick the entity **nearest the screen center** (smallest NDC magnitude)
///   among those *strictly* inside the camera frustum. World distance is
///   only used to break ties when two entities share the same angular
///   position. Falls back to the relaxed-frustum set if nothing is
///   strictly in-view.
/// - Subsequent presses: order candidates by hybrid score (same scoring
///   as first press — nearest in world with a small off-center penalty)
///   and step to the entry after the current target. Wraps at the end.
///   This matches FFXI retail's "Tab cycles nearest → next-nearest"
///   model rather than a screen-x sweep.
///
/// Frustum inclusion is **relaxed** past the strict `[-1, 1]` box (see
/// [`CYCLE_NDC_LIMIT`]) so that entities sitting just off the screen
/// edge — the common chase-cam "mob over my shoulder" case — stay in
/// the cycle. Retail FFXI exhibits the same forgiving behavior.
///
/// Self is excluded explicitly via `self_id` — the wire entity list
/// now includes the player's own entry (matched by `self_char_id`),
/// so Tab would otherwise auto-select self on the first press.
pub fn cycle_target_viewport<F>(
    entities: &[WireEntity],
    from: WireVec3,
    current: Option<u32>,
    self_id: Option<u32>,
    project: F,
) -> Option<u32>
where
    F: Fn(Vec3) -> Option<Vec3>,
{
    struct Cand {
        id: u32,
        ndc_x: f32,
        ndc_mag_sq: f32,
        dist_sq: f32,
        in_frustum: bool,
    }

    let mut candidates: Vec<Cand> = entities
        .iter()
        .filter(|e| Some(e.id) != self_id)
        .filter_map(|e| {
            // FFXI position → Bevy world: same mapping as `ffxi_to_bevy`.
            // Inlined here so we don't pull a Bevy dep into this fn for
            // unit tests; the conversion is one-line.
            let world_pos = Vec3::new(e.pos.x, e.pos.z, -e.pos.y);
            let ndc = project(world_pos)?;
            // `world_to_ndc` returns z>1 for points behind the camera in
            // Bevy's reverse-Z projection, and z<0 past the far plane.
            // Either way → not a valid cycle candidate.
            if ndc.z < 0.0 || ndc.z > 1.0 {
                return None;
            }
            // Relaxed lateral / vertical cull. Entities well past the
            // screen edge (more than ~30% beyond the frustum) are out;
            // anything closer than that may still be in the cycle.
            if ndc.x.abs() > CYCLE_NDC_LIMIT || ndc.y.abs() > CYCLE_NDC_LIMIT {
                return None;
            }
            // 3D distance: server range checks use all three axes, so
            // the cycle's "closeness" tiebreaker has to as well. On a
            // slope, a 2D-closer mob can be several yalms farther in 3D
            // and unreachable.
            let dx = e.pos.x - from.x;
            let dy = e.pos.y - from.y;
            let dz = e.pos.z - from.z;
            let in_frustum = ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0;
            Some(Cand {
                id: e.id,
                ndc_x: ndc.x,
                ndc_mag_sq: ndc.x * ndc.x + ndc.y * ndc.y,
                dist_sq: dx * dx + dy * dy + dz * dz,
                in_frustum,
            })
        })
        .collect();

    if candidates.is_empty() {
        return None;
    }

    let current_in_cycle =
        current.and_then(|id| candidates.iter().any(|c| c.id == id).then_some(id));

    match current_in_cycle {
        Some(curr) => {
            // Cycle by hybrid score (nearest first, with the same
            // off-center penalty used on first press). Retail FFXI's
            // Tab cycles nearest → next-nearest, not by screen-x —
            // walking through a mob crowd, the second Tab should land
            // on the second-closest mob, not the next one to the right.
            candidates.sort_by(|a, b| {
                let sa = a.dist_sq.sqrt() + NDC_PENALTY_YALMS * a.ndc_mag_sq.sqrt();
                let sb = b.dist_sq.sqrt() + NDC_PENALTY_YALMS * b.ndc_mag_sq.sqrt();
                sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal)
            });
            let pos = candidates.iter().position(|c| c.id == curr)?;
            Some(candidates[(pos + 1) % candidates.len()].id)
        }
        None => {
            // First press: prefer strictly-in-frustum entities; only fall
            // back to the relaxed set if nothing in-view qualifies. Among
            // the preferred pool, pick the lowest hybrid score —
            // world distance (yalms) plus a screen-offset penalty. At
            // NDC magnitude = 1 (entity at the frustum edge), the entity
            // is treated as `NDC_PENALTY_YALMS` further than its true
            // distance. This keeps Tab biased toward what's "in front
            // of you" while still letting a close off-center mob beat
            // a far centered one — the previous pure-NDC ranking would
            // skip the near mob entirely.
            let any_strict = candidates.iter().any(|c| c.in_frustum);
            candidates
                .iter()
                .filter(|c| !any_strict || c.in_frustum)
                .min_by(|a, b| {
                    let sa = a.dist_sq.sqrt() + NDC_PENALTY_YALMS * a.ndc_mag_sq.sqrt();
                    let sb = b.dist_sq.sqrt() + NDC_PENALTY_YALMS * b.ndc_mag_sq.sqrt();
                    sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|c| c.id)
        }
    }
}

/// State for the auto-recenter behavior: how long forward has been
/// continuously held with no operator yaw input. After
/// [`AUTO_RECENTER_HOLD_S`] the chase camera lerps toward "behind the
/// player at current heading" at [`AUTO_RECENTER_RATE`] rad/s. Any
/// `CameraYawLeft`/`Right` press cancels and resets the timer.
#[derive(Resource, Default)]
pub struct CameraAutoRecenter {
    /// `Some(instant)` while MoveForward has been held continuously with
    /// no yaw input since `instant`; `None` otherwise. Active recenter
    /// begins once `now - instant >= AUTO_RECENTER_HOLD_S`.
    pub forward_held_since: Option<Instant>,
}

/// Forward must be held this long with no yaw input before auto-recenter
/// engages. Matches retail's "walk a beat before the camera trails" feel
/// (long enough that brief forward taps don't twitch the camera).
const AUTO_RECENTER_HOLD_S: f32 = 0.5;
/// Angular rate of the auto-recenter lerp, radians/sec. ~0.6 rad/s ≈ 34°/s
/// — fast enough to obviously settle while walking, slow enough to read
/// as a smooth follow rather than a snap.
const AUTO_RECENTER_RATE: f32 = 0.6;
/// First-person look-at-lock pitch tracking rate, radians/sec. ~3 rad/s
/// is fast: when locked onto a tall mob the camera tips up to meet its
/// head within ~½ second from a level start.
const FP_LOCK_PITCH_RATE: f32 = 3.0;
/// Vertical offset (Bevy units) from a target entity's transform origin
/// to its head. Most NPC/PC capsules are ~1.9 yalms tall with origin at
/// the feet (see `scene::EntityMesh`); 1.5 lands roughly between the
/// neck and the crown, which is what the operator instinctively
/// "looks at" in 1st-person.
const TARGET_HEAD_OFFSET_Y: f32 = 1.5;

/// Camera-polish system: auto-recenter behind player when walking
/// forward with no yaw input, and (in first-person + lock-on) track the
/// locked target's head height by driving `chase.pitch`.
///
/// Runs in `Update`. Reads input + transforms; mutates `ChaseCamera`
/// only (no command dispatch). The two behaviors are independent and
/// composable — both can run in the same frame (e.g. running forward in
/// 1p with a lock-on: pitch tracks the target, yaw is not recentered
/// because lock-on is already pinning it via `dispatch_movement_system`).
pub fn camera_polish_system(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<Bindings>,
    time: Res<Time>,
    mode: Res<InputMode>,
    camera_mode: Res<CameraMode>,
    state: Res<SceneState>,
    lock_on: Res<LockOn>,
    mut chase: ResMut<ChaseCamera>,
    mut recenter: ResMut<CameraAutoRecenter>,
    self_q: Query<&Transform, (With<IsSelf>, Without<OperatorCamera>)>,
    target_q: Query<(&WorldEntity, &Transform), Without<OperatorCamera>>,
) {
    // Disabled outside World — recentering while typing in chat or
    // navigating a menu would be confusing (and movement is paused
    // anyway in those modes).
    if !matches!(*mode, InputMode::World) {
        recenter.forward_held_since = None;
        return;
    }

    // --- (a) Auto-recenter ---------------------------------------------
    // Track the "forward held with no yaw input" window. The yaw-input
    // test uses Action::CameraYawLeft/Right specifically (per the unit
    // spec) — A/D rotates the player AND the camera lock-step, so it's
    // intentionally NOT considered a "free-look yaw input" that should
    // cancel recenter.
    //
    // Chase-mode only: in first-person, "behind the player at current
    // heading" is not a meaningful concept (FP yaw drives the look
    // direction directly).
    let forward_held = bindings.pressed(Action::MoveForward, &keys);
    let yaw_input = bindings.pressed(Action::CameraYawLeft, &keys)
        || bindings.pressed(Action::CameraYawRight, &keys);

    if yaw_input || !forward_held || !matches!(*camera_mode, CameraMode::Chase) {
        recenter.forward_held_since = None;
    } else {
        let now = Instant::now();
        let started = *recenter.forward_held_since.get_or_insert(now);
        let elapsed = now.duration_since(started).as_secs_f32();
        if elapsed >= AUTO_RECENTER_HOLD_S {
            // Target yaw: camera directly behind the player at the
            // player's current heading. `yaw_for_heading` is the
            // already-used "camera-behind-player" mapping (see the
            // one-shot sync in `chase_camera_system`).
            let target_yaw = yaw_for_heading(state.snapshot.self_pos.heading);
            // Shortest-arc delta wrapped into [-π, π] so we don't take
            // the long way around when current yaw is already close to
            // target on the "wrong" side of ±π.
            let tau = std::f32::consts::TAU;
            let mut diff = (target_yaw - chase.yaw).rem_euclid(tau);
            if diff > std::f32::consts::PI {
                diff -= tau;
            }
            let max_step = AUTO_RECENTER_RATE * time.delta_secs();
            let step = diff.clamp(-max_step, max_step);
            chase.yaw += step;
        }
    }

    // --- (c) 1p auto-lock pitch tracking -------------------------------
    // Only when in first-person AND lock-on is active AND the locked
    // entity is in the scene. Drive `chase.pitch` toward the angle to
    // the target's head. Do NOT touch yaw — the existing lock-on path
    // in `dispatch_movement_system` already pins heading + chase.yaw
    // each tick.
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

    // Eye and head positions. Eye matches `firstperson_camera_system`'s
    // origin so the pitch we compute is the one that actually frames
    // the head on screen.
    let eye = self_t.translation + Vec3::Y * ChaseCamera::FP_EYE_HEIGHT;
    let head = target_pos + Vec3::Y * TARGET_HEAD_OFFSET_Y;
    let to_head = head - eye;
    // Degenerate (target on top of player) — leave pitch alone.
    let horiz = (to_head.x * to_head.x + to_head.z * to_head.z).sqrt();
    if horiz < 1e-4 && to_head.y.abs() < 1e-4 {
        return;
    }
    // FP's look_dir is `(-yaw.sin()*cos_p, sin_p, -yaw.cos()*cos_p)`, so
    // the +Y component is `sin(pitch)`. The pitch that points at `head`
    // therefore satisfies `sin(p) = to_head.y / |to_head|`, equivalent
    // to `atan2(to_head.y, horiz)`.
    let desired_pitch = to_head.y.atan2(horiz).clamp(
        ChaseCamera::FP_PITCH_MIN,
        ChaseCamera::FP_PITCH_MAX,
    );
    let max_step = FP_LOCK_PITCH_RATE * time.delta_secs();
    let diff = desired_pitch - chase.pitch;
    let step = diff.clamp(-max_step, max_step);
    chase.pitch += step;
}

/// How far past the strict `[-1, 1]` NDC frustum an entity may sit and
/// still appear in the Tab cycle. 1.3 ≈ 15% past each edge — enough to
/// pick up the "mob just barely off-screen behind your shoulder" case
/// that FFXI retail tabs to, without inviting entities that are clearly
/// behind the camera or far peripheral.
const CYCLE_NDC_LIMIT: f32 = 1.3;

/// First-press off-center penalty (yalms). The Tab cycle's first-press
/// score is `world_dist + NDC_PENALTY_YALMS * ndc_mag`; at the strict
/// frustum edge (NDC magnitude = 1) an entity is treated as if it were
/// 10 yalms further away than its true 3D distance. Picked to keep Tab
/// biased toward what the player is looking at while still letting a
/// close off-center mob beat a far centered one.
const NDC_PENALTY_YALMS: f32 = 10.0;

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_viewer_wire::{Entity as WireEntity, EntityKind, Vec3 as WireVec3};

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
        }
    }

    /// Project that places every entity at NDC (x = pos.x / 100, y = 0,
    /// z = 0.5) — i.e. all visible, in left-to-right order matching FFXI x.
    fn fake_proj(p: Vec3) -> Option<Vec3> {
        Some(Vec3::new(p.x / 100.0, 0.0, 0.5))
    }

    /// Project that culls everything behind x>50 (i.e. simulating a
    /// view frustum that only contains entities with FFXI x ≤ 50).
    fn culled_proj(p: Vec3) -> Option<Vec3> {
        if p.x > 50.0 {
            None
        } else {
            Some(Vec3::new(p.x / 100.0, 0.0, 0.5))
        }
    }

    /// Holding A/D for one second at 60 Hz should accumulate close to
    /// `HEADING_TURN_RATE * 256/τ` u8 units (~35 units = ~49°). This is
    /// the "finite turn rate" contract: visible, framerate-independent
    /// sweep at the documented rad/s.
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
        // Should match expected within 1 u8 of accumulator slack.
        assert!(
            (total_u8 - expected).abs() <= 1,
            "1s of held turn produced {total_u8} u8 (expected ~{expected})",
        );
        // Verify ≈ 49°/s — the finite, retail-feel rate.
        let degrees = total_u8 as f32 * 360.0 / 256.0;
        assert!(
            (degrees - 49.0).abs() < 3.0,
            "1s of held turn = {degrees:.1}°, expected ~49°",
        );
    }

    /// At 60 Hz the per-tick float delta (≈0.58 u8/tick) is below 1.0 —
    /// without an accumulator, every tick would round to zero and the
    /// player would never turn. This regression-guards the fractional-
    /// carry behavior.
    #[test]
    fn heading_turn_does_not_round_to_zero_per_tick() {
        let mut accum = 0.0_f32;
        let dt = 1.0 / 60.0;
        // First tick: no whole unit yet (sub-1 delta).
        let (whole_1, float_1) = advance_heading_turn(&mut accum, 1, dt);
        assert_eq!(whole_1, 0, "first 60Hz tick must not yet flip a u8");
        assert!(float_1 > 0.0 && float_1 < 1.0);
        assert!(accum > 0.0, "fractional units must carry over");
        // After enough ticks the carry must eventually flip.
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

    /// Releasing the turn key drops the fractional carry so a re-press
    /// doesn't start with a phantom partial unit (which would feel like
    /// a tiny snap on every tap).
    #[test]
    fn heading_turn_release_clears_fraction() {
        let mut accum = 0.0_f32;
        let dt = 1.0 / 60.0;
        // Build up some fractional carry.
        let _ = advance_heading_turn(&mut accum, 1, dt);
        assert!(accum > 0.0);
        // Release: accum must reset to exactly 0.
        let (whole, fdelta) = advance_heading_turn(&mut accum, 0, dt);
        assert_eq!(whole, 0);
        assert_eq!(fdelta, 0.0);
        assert_eq!(accum, 0.0);
    }

    /// Left and right turns are exact negatives of each other — holding
    /// A then D for the same duration must net zero net heading change.
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

    /// Project that maps FFXI x → NDC at a *wider* scale: ndc.x = pos.x / 50.
    fn wide_proj(p: Vec3) -> Option<Vec3> {
        Some(Vec3::new(p.x / 50.0, 0.0, 0.5))
    }

    /// Project that also varies NDC.y.
    fn xy_proj(p: Vec3) -> Option<Vec3> {
        Some(Vec3::new(p.x / 100.0, p.y / 100.0, 0.5))
    }

    #[test]
    fn first_press_picks_nearest_to_screen_center() {
        let from = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        // ndc.x = pos.x/100: entity 2 sits at 0.1, entity 3 at 0.2,
        // entity 1 at 0.3 — entity 2 is most centered.
        let entities = vec![ent(1, 30.0, 0.0), ent(2, 10.0, 0.0), ent(3, 20.0, 0.0)];
        let next = cycle_target_viewport(&entities, from, None, None, fake_proj);
        assert_eq!(next, Some(2));
    }

    #[test]
    fn cycle_excludes_self() {
        // Self is in the wire entity list (matched by `self_char_id`).
        // Tab must skip it on first press AND on subsequent cycle
        // steps, otherwise the operator will land on their own model.
        let from = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        // Entity 99 is "self" at the player position; entities 1/2 are
        // mobs slightly farther out.
        let entities = vec![
            ent(99, 0.0, 0.0),
            ent(1, 10.0, 0.0),
            ent(2, 20.0, 0.0),
        ];
        // First press: skips id=99 even though it's at ndc=0/dist=0;
        // picks id=1 (next-closest in the filtered set).
        assert_eq!(
            cycle_target_viewport(&entities, from, None, Some(99), fake_proj),
            Some(1)
        );
        // Cycling from id=1 wraps to id=2, never returning id=99.
        assert_eq!(
            cycle_target_viewport(&entities, from, Some(1), Some(99), fake_proj),
            Some(2)
        );
        assert_eq!(
            cycle_target_viewport(&entities, from, Some(2), Some(99), fake_proj),
            Some(1)
        );
    }

    #[test]
    fn first_press_3d_distance_includes_altitude() {
        // Two entities at identical (x, y) and identical NDC, distinguished
        // only by FFXI z (altitude). The closer-in-3D one must win.
        // Catches a 2D-distance regression in the first-press score.
        let from = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        // Both project to the same NDC under `fake_proj` (z is ignored
        // by that projector). Entity 1 is 5y above, entity 2 is 50y above.
        let entities = vec![ent_xyz(1, 0.0, 0.0, 5.0), ent_xyz(2, 0.0, 0.0, 50.0)];
        assert_eq!(
            cycle_target_viewport(&entities, from, None, None, fake_proj),
            Some(1)
        );
    }

    #[test]
    fn first_press_close_off_center_beats_far_centered() {
        // Regression: the user-reported "Tab fails to select the closest
        // mob" bug. Pure NDC-magnitude ranking would pick a far entity
        // that happens to sit dead-center on screen over a much closer
        // entity that's slightly off-axis. With the hybrid score
        // (world_dist + NDC_PENALTY_YALMS * ndc_mag), the closer entity
        // wins as long as the screen offset is reasonable.
        let from = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        // fake_proj: ndc.x = pos.x/100, ndc.y = 0.
        //   Entity 1 at (5, 30): ndc=(0.05, 0), |ndc|=0.05.
        //                         dist=√(25+900)=30.41, score=30.41+0.5=30.91
        //   Entity 2 at (20, 5): ndc=(0.20, 0), |ndc|=0.20.
        //                         dist=√(400+25)=20.62, score=20.62+2.0=22.62
        // Hybrid: entity 2 wins (lower score). Closer in world reaches.
        let entities = vec![ent(1, 5.0, 30.0), ent(2, 20.0, 5.0)];
        assert_eq!(
            cycle_target_viewport(&entities, from, None, None, fake_proj),
            Some(2)
        );
    }

    #[test]
    fn first_press_combined_ndc_and_world_distance() {
        // Entity high on the screen but horizontally centered should
        // lose to a slightly-off-center entity that's much closer in
        // world space (the hybrid score combines both signals).
        let from = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        // xy_proj reads bevy.x, bevy.y → after conversion that's
        // (FFXI.x, FFXI.z).
        //   Entity 1: FFXI (0, 0, 80)  → ndc=(0.00, 0.80), |ndc|=0.80,
        //                                 dist=80, score=80+8.0=88.
        //   Entity 2: FFXI (15, 0, 15) → ndc=(0.15, 0.15), |ndc|=0.212,
        //                                 dist=√(225+225)=21.2,
        //                                 score=21.2+2.12=23.3.
        // Entity 2 wins on combined score.
        let entities = vec![ent_xyz(1, 0.0, 0.0, 80.0), ent_xyz(2, 15.0, 0.0, 15.0)];
        assert_eq!(
            cycle_target_viewport(&entities, from, None, None, xy_proj),
            Some(2)
        );
    }

    #[test]
    fn subsequent_presses_cycle_by_distance() {
        let from = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        // Three entities at clearly distinct distances. Hybrid scores:
        //   id=2 at (5, 0)   → dist=5,  ndc=0.05 → score = 5.5
        //   id=3 at (15, 0)  → dist=15, ndc=0.15 → score = 16.5
        //   id=1 at (30, 0)  → dist=30, ndc=0.30 → score = 33.0
        // Sorted order: [2, 3, 1].
        let entities = vec![ent(1, 30.0, 0.0), ent(2, 5.0, 0.0), ent(3, 15.0, 0.0)];
        // From id=2 (nearest) → id=3 (next-nearest).
        assert_eq!(
            cycle_target_viewport(&entities, from, Some(2), None, fake_proj),
            Some(3)
        );
        // From id=3 → id=1 (third-nearest).
        assert_eq!(
            cycle_target_viewport(&entities, from, Some(3), None, fake_proj),
            Some(1)
        );
        // From id=1 → wraps back to id=2.
        assert_eq!(
            cycle_target_viewport(&entities, from, Some(1), None, fake_proj),
            Some(2)
        );
    }

    #[test]
    fn cycle_includes_slightly_out_of_view_entities() {
        // FFXI-retail parity: an entity sitting just past the strict
        // frustum edge (chase-cam "mob over my shoulder") is still part
        // of the cycle. With CYCLE_NDC_LIMIT=1.3, NDC.x up to ±1.3 is
        // eligible.
        let from = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        // wide_proj: ndc.x = pos.x / 50.
        //   Entity 1: x=-25 → ndc.x=-0.5 (strictly in frustum).
        //   Entity 2: x=60  → ndc.x=1.2  (slightly out — relaxed bound).
        //   Entity 3: x=80  → ndc.x=1.6  (well past — excluded).
        let entities = vec![ent(1, -25.0, 0.0), ent(2, 60.0, 0.0), ent(3, 80.0, 0.0)];
        // Currently targeting entity 1; pressing Tab cycles to entity 2
        // (the slightly-out one) and skips entity 3 entirely.
        assert_eq!(
            cycle_target_viewport(&entities, from, Some(1), None, wide_proj),
            Some(2)
        );
        // From entity 2, wrap back to entity 1 (entity 3 stays excluded).
        assert_eq!(
            cycle_target_viewport(&entities, from, Some(2), None, wide_proj),
            Some(1)
        );
    }

    #[test]
    fn first_press_prefers_strictly_in_frustum() {
        // When entities exist both strictly in-frustum and in the
        // relaxed band, the initial Tab pick comes from the in-frustum
        // pool even if a relaxed-band entity is closer to screen center.
        let from = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        // wide_proj: ndc.x = pos.x / 50.
        //   Entity 1: x=45 → ndc.x=0.90 (in frustum, off-center).
        //   Entity 2: x=60 → ndc.x=1.20 (slightly out — would be more
        //                                 centered if wrapped, but isn't).
        // Entity 1 wins because it's the only strict-in-frustum candidate.
        let entities = vec![ent(1, 45.0, 0.0), ent(2, 60.0, 0.0)];
        assert_eq!(
            cycle_target_viewport(&entities, from, None, None, wide_proj),
            Some(1)
        );
    }

    #[test]
    fn first_press_falls_back_to_relaxed_when_none_in_frustum() {
        // If literally nothing is strictly in-view, first-press still
        // picks the most-centered relaxed-band entity rather than
        // returning None.
        let from = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        // wide_proj: ndc.x = pos.x / 50.
        //   Entity 1: x=60 → ndc.x=1.20 (relaxed, slightly less centered).
        //   Entity 2: x=55 → ndc.x=1.10 (relaxed, more centered).
        let entities = vec![ent(1, 60.0, 0.0), ent(2, 55.0, 0.0)];
        assert_eq!(
            cycle_target_viewport(&entities, from, None, None, wide_proj),
            Some(2)
        );
    }

    #[test]
    fn off_screen_entities_are_skipped() {
        let from = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        // entity 4 at x=100 will be culled by `culled_proj`.
        let entities = vec![ent(1, 0.0, 0.0), ent(4, 100.0, 0.0)];
        let next = cycle_target_viewport(&entities, from, None, None, culled_proj);
        assert_eq!(next, Some(1));
        // From off-screen current → falls back to nearest visible.
        let next = cycle_target_viewport(&entities, from, Some(4), None, culled_proj);
        assert_eq!(next, Some(1));
    }

    #[test]
    fn empty_or_all_off_screen_returns_none() {
        let from = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let entities: Vec<WireEntity> = vec![];
        assert_eq!(
            cycle_target_viewport(&entities, from, None, None, fake_proj),
            None
        );
        // All off-screen.
        let entities = vec![ent(1, 100.0, 0.0), ent(2, 200.0, 0.0)];
        assert_eq!(
            cycle_target_viewport(&entities, from, None, None, culled_proj),
            None
        );
    }

    #[test]
    fn current_offscreen_falls_back_to_nearest() {
        let from = WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let entities = vec![ent(1, 30.0, 0.0), ent(99, 1000.0, 0.0)];
        // 99 not visible — should pick nearest visible (1).
        let next = cycle_target_viewport(&entities, from, Some(99), None, culled_proj);
        assert_eq!(next, Some(1));
    }
}
