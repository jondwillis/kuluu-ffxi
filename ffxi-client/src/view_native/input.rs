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
//!   Tab       cycle on-screen targets. First press picks the nearest
//!             on-screen entity (party/own-pet sorted last); further
//!             presses step through a stable, frozen order one per press
//!             until every candidate is visited, then rebuild. Off-screen
//!             entities are never cycled. Mirrors FFXI retail.
//!   Enter     with no target, acquires the nearest on-screen entity (same
//!             pick as Tab); with a target, acts on it (see text_input.rs).
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

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use bevy::ecs::system::SystemParam;
use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::window::WindowCloseRequested;

/// Bundles camera-related ResMuts so `handle_input_system` stays under
/// Bevy's 16-param SystemParam tuple limit.
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

/// Player A/D heading turn rate, **radians per second** of held key. Frame-
/// rate-independent so the dispatch tick rate (currently 60 Hz; see
/// `view_native::mod`) can change without retuning. 0.86 rad/s ≈ 49 °/s —
/// matches the retail FFXI feel of a visible but unhurried pivot (full
/// 180° in ~3.7 s) and keeps the older ROTATE_STEP_HELD=2-at-20-Hz target
/// of ~56°/s within ±13%. Camera yaw turns lock-step at the same rate so
/// the chase camera stays glued behind the player while turning in place.
pub const HEADING_TURN_RATE: f32 = 0.86;
/// Camera yaw delta per second when ←/→ held. Free-look panning runs at
/// 2× the player A/D turn rate (~98 °/s, full 180° in ~1.8 s): the camera
/// has no momentum or animation to sell, so matching the player pivot rate
/// felt sluggish — retail's arrow-pan sweeps noticeably faster than the
/// body turns. Decoupled from `HEADING_TURN_RATE` so steered-turn feel is
/// unaffected.
const CAMERA_YAW_RATE: f32 = HEADING_TURN_RATE * 4.0;
/// Camera pitch delta per tick when ↑/↓ held. ~17 °/s @ 20 Hz — slow on
/// purpose so taps make small adjustments.
const PITCH_STEP_HELD: f32 = 0.015;
/// Sustained A/D hold required to cancel autorun. A brief tap (single
/// 50 ms tick) won't trip this; a held sidestep will.
const STRAFE_CANCEL_MS: u64 = 300;
/// Yalms-per-second per unit of *server-set* speed — the engine's fixed
/// speed-unit→yalms conversion, not a per-server tuning knob. The dynamic
/// term is `self_pos.speed`, which the server pushes in every CHAR_PC /
/// 0x0A `PosHead` (`UpdateSpeed()` → gear/buff/weight-modified). LSB's
/// `BASE_SPEED = 50` (settings/default/map.lua) renders as the documented
/// retail ~5 yalms/sec, so the conversion is 5 / 50 = 0.1. A classic
/// server sending a lower base byte therefore renders proportionally
/// slower (40 → 4 yps); haste gear pushing speed > base renders faster.
/// This matches the reactor's own "5 yalms/sec base" assumption
/// (`reactor.rs` `max_step_per_tick`); the previous 0.2 assumed a base of
/// 25 the server never sends, doubling local speed and desyncing the
/// avatar from server-confirmed position. Step per tick is
/// `speed * SPEED_TO_YPS * delta_secs` — frame-rate-independent so the
/// dispatch rate (currently 60 Hz; see `view_native::mod`) can change
/// without retuning movement speed.
const SPEED_TO_YPS: f32 = 0.1;

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
/// Heading spring constant (1/sec). Both heading-toward-motion and
/// chase-yaw-toward-behind-player are **exponential lerps**: each
/// tick advances by `residual * (1 - exp(-rate · dt))`, so the
/// angular velocity is proportional to the residual angle. This is
/// what produces the geometric 45°/45° steady-state lag — a
/// fixed-rate clamped lerp would just preserve the initial offset
/// (camera stays behind player at motion start) and never settle to
/// the constraint `lag_head + lag_chase = π/2`.
///
/// Shipped HLR = CTR = 5.0:
///   ω = HLR · π/4 ≈ 3.93 rad/sec (~225°/sec sustained turn)
///   r = walk_speed / ω ≈ 2.0 yalm (tight orbit circle)
///   lag_head = lag_chase = π/4 = 45° steady state
const HEADING_LERP_RATE_RAD_PER_SEC: f32 = 5.0;

/// Chase-camera yaw spring constant (1/sec). See
/// [`HEADING_LERP_RATE_RAD_PER_SEC`]. Equal to HLR for the 45°/45°
/// retail split.
const CHASE_TRACK_RATE_RAD_PER_SEC: f32 = 5.0;

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
    mut camera: CameraInputParams,
    cam_q: Query<(&Camera, &Transform), With<OperatorCamera>>,
    mut exit: MessageWriter<AppExit>,
    mut rest_stance: ResMut<ffxi_viewer_core::combat_stance::RestStance>,
    mut walk_mode: ResMut<ffxi_viewer_core::combat_stance::WalkMode>,
    mut tab_stack: ResMut<TabCycleStack>,
) {
    let camera_mode = &mut camera.mode;
    let chase = &mut camera.chase;
    let cursor_lock = &mut camera.cursor_lock;
    let lock_on = &mut camera.lock_on;
    let camera_transition = &mut camera.transition;
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
        // Retail dollies between 3p and 1p instead of cutting — start
        // a 0.35s zoom transition. `camera_transition_system` runs in
        // Update and handles mode-swap mid-dolly.
        camera_transition.begin(**camera_mode, chase.distance);
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

    // Tab cycle and Enter-with-no-target acquire share one on-screen,
    // *stable* stack (`TabCycleStack`): the candidate order is built once
    // and stepped through one pop per press, so Tab reliably advances
    // through a crowd instead of oscillating. Both need the camera
    // projection — which only this system has — so the Enter *acquire*
    // lives here, next to Tab. The Enter *act* on an existing target
    // (Talk for NPCs / action menu for combatants) stays in
    // `text_input.rs`'s logical-key path so that opening a menu doesn't
    // immediately re-dispatch the same Enter event into it.
    let tab = bindings.just_pressed(Action::CycleTarget, &keys);
    // Enter acquires here only when nothing is targeted yet; with a target
    // it's an "act" handled in `handle_world_key`. Suppressed while dead —
    // the death prompt routes Enter to `ReturnToHomePoint`.
    let enter_acquire = bindings.just_pressed(Action::ConfirmAction, &keys)
        && target.id.is_none()
        && !ffxi_viewer_core::hud::death_prompt::is_dead(&state);
    if tab || enter_acquire {
        if let Ok((camera, cam_t)) = cam_q.single() {
            let cam_global = GlobalTransform::from(*cam_t);
            // Party members + own pet sort to the end of the cycle. No
            // entity-level `owner` on the wire, so the owned-pet test
            // reuses the codebase's `Pet && claim_id == self` heuristic
            // (same as `/targetnpcparty`).
            let party_ids: Vec<u32> = state.snapshot.party.iter().map(|p| p.id).collect();
            let owner = state.snapshot.self_char_id.unwrap_or(0);
            let owned_pet_ids: Vec<u32> = state
                .snapshot
                .entities
                .iter()
                .filter(|e| matches!(e.kind, EntityKind::Pet) && e.claim_id == owner)
                .map(|e| e.id)
                .collect();
            // `None` = no on-screen candidate to advance to → keep the
            // current target (XIM `targetCycle` returns false / no change).
            // An empty Tab must never *clear* the target, and Enter-acquire
            // with nothing on-screen leaves `target.id` as `None` so
            // `handle_world_key` opens the no-target menu.
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
            state.snapshot.party.get((slot - 1) as usize).map(|p| p.id)
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
    if bindings.just_pressed(Action::ToggleWalk, &keys) {
        walk_mode.walking = !walk_mode.walking;
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
            | InputMode::TargetAction(_)
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
    walk_mode: Res<ffxi_viewer_core::combat_stance::WalkMode>,
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
        InputMode::Menu(_)
            | InputMode::QuickAction(_)
            | InputMode::TargetAction(_)
            | InputMode::PassiveCursor(_)
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
    // Left pans the view left, Right pans it right. The yaw→camera geometry
    // in `chase_camera_system` orbits the camera the same rotational sense
    // as the mouse drag below, so YawLeft subtracts and YawRight adds to
    // match. The previous name-matches-sign mapping read inverted (pushing
    // Right turned the view left); keep this in lock-step with the mouse
    // `chase.yaw += delta.x` sign so both input paths agree.
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

    // --- Heading rotate (accumulator-driven, framerate-independent).
    //     Q/E (`RotateLeft`/`Right`) is pure rotate in both camera
    //     modes. A/D (`TurnLeft`/`Right`) is folded in **only in
    //     first-person** — FP has no orbit visual, so A/D collapses
    //     to the same pivot as Q/E. In chase mode A/D is the
    //     camera-relative orbit verb handled below; folding it here
    //     would clobber the orbit's heading-lerp toward motion
    //     direction, defeating the retail 45°/45° steady-state. ---
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

    // Apply autorun: virtual W held.
    if autorun.phantom_forward {
        forward = forward.max(1);
    }

    // Fold `turn` based on camera mode. Must run AFTER the autorun-cancel
    // check (turn must not trigger cancel) and AFTER the autorun
    // phantom_forward expansion (so W+A doesn't double-count forward).
    //
    // A/D in Chase mode → orbit/strafe path below. Two coupled springs
    // (heading lerp toward motion direction, chase-yaw lerp toward
    // behind player) settle to lag_head + lag_chase = π/2 with the
    // shipped HLR=CTR=2.0 producing the retail 45°/45° split.
    //
    // A/D in FirstPerson is already folded into `player_rotate_dir`
    // above (FP has no orbit visual, so A/D collapses to pure rotate).
    let turn_in_chase = turn != 0 && matches!(*camera_mode, CameraMode::Chase);

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

    // Lock-on contact clamp: how far we may still advance *toward* the
    // locked target before the player's model touches the target's model.
    // `None` when nothing is locked (no clamp). Computed from `basis_pos`
    // (the prediction-authoritative position the step is applied to) so
    // multiple FixedUpdate runs per render frame stay stable. Used to cap
    // forward motion below so we hold at contact instead of overshooting and
    // re-facing — and chasing resumes naturally when the target walks away.
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
    if forward == 0 && strafe == 0 && player_rotate_u8 == 0 && !turn_in_chase {
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

        let step_magnitude =
            self_pos.speed as f32 * SPEED_TO_YPS * time.delta_secs() * walk_mode.scale();
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
            // Exponential lerp: residual * (1 - exp(-rate·dt)). Rate
            // proportional to residual is what produces the 45°
            // steady-state lag; a fixed-rate clamped lerp would just
            // preserve initial conditions.
            let h_alpha = 1.0 - (-HEADING_LERP_RATE_RAD_PER_SEC * time.delta_secs()).exp();
            heading = heading_for_yaw(h_current + h_diff * h_alpha);
        }

        // Chase-camera yaw exponential-lerps toward "behind player
        // heading" (the lerped value above). Equal rate to heading
        // lerp produces the 45°/45° split: camera trails permanently,
        // never catches up during sustained orbit.
        let chase_target = yaw_for_heading(heading);
        let c_diff = wrap_signed_pi(chase_target - chase.yaw);
        let c_alpha = 1.0 - (-CHASE_TRACK_RATE_RAD_PER_SEC * time.delta_secs()).exp();
        chase.yaw += c_diff * c_alpha;

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
    let raw_step = self_pos.speed as f32 * SPEED_TO_YPS * time.delta_secs() * walk_mode.scale();
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
        // Under lock-on, `heading` points straight at the target, so forward
        // motion is along the player→target axis. Cap it at the contact-ring
        // allowance so we stop on contact instead of overshooting and
        // flipping 180° to re-face. Only when advancing (forward > 0):
        // backpedal stays unclamped so you can always back away, and strafe
        // (below) is untouched so you can still orbit a contacted target.
        let fwd_step = match (forward > 0, lock_forward_allowance) {
            (true, Some(allowed)) => step.min(allowed),
            _ => step,
        };
        x += fwd_x * fwd_step * forward as f32;
        y += fwd_y * fwd_step * forward as f32;
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

/// Model collision radius for a wire entity kind, in yalms. Bridges the
/// `ffxi_viewer_wire::EntityKind` of snapshot entities to the single source
/// of truth (`crate::state::MODEL_RADIUS_*`), so the lock-on/auto-run clamp
/// and the reactor follow goal use identical numbers. (Inline match over the
/// wire enum, mirroring the precedent in `text_input.rs`.)
fn radius_for_wire_kind(kind: EntityKind) -> f32 {
    match kind {
        EntityKind::Pc => crate::state::MODEL_RADIUS_PC,
        EntityKind::Npc => crate::state::MODEL_RADIUS_NPC,
        EntityKind::Mob => crate::state::MODEL_RADIUS_MOB,
        EntityKind::Pet => crate::state::MODEL_RADIUS_PET,
        EntityKind::Other => crate::state::MODEL_RADIUS_OTHER,
    }
}

/// Remaining distance the player may advance toward a target before its
/// model touches the target's, given a center-to-center `stop` distance.
/// `0.0` once already at or inside contact (never negative), so a caller
/// clamping forward motion with `step.min(allowance)` holds at the ring and
/// never pushes through. Ground plane only (x, y).
fn forward_allowance(from_xy: (f32, f32), target_xy: (f32, f32), stop: f32) -> f32 {
    let dx = target_xy.0 - from_xy.0;
    let dy = target_xy.1 - from_xy.1;
    let dist = (dx * dx + dy * dy).sqrt();
    (dist - stop).max(0.0)
}

/// Map an angle difference into `[-π, π]` so the turn handler's heading
/// and chase-yaw lerps always take the shortest arc around the modular
/// τ boundary.
#[inline]
fn wrap_signed_pi(x: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (x + PI).rem_euclid(TAU) - PI
}

/// Persistent Tab/Enter target-cycle state. This is what makes the cycle
/// *stable*: instead of recomputing the candidate order on every press
/// (the old `cycle_target_viewport`, whose score shifted frame-to-frame as
/// entities/camera moved, so Tab oscillated), we build the ordered list
/// **once** into `ids`, pop one per press, and only rebuild when the stack
/// empties or goes idle. Mirrors XIM's `PlayerTargetSelector.TargetStack`
/// (research/xim/.../game/PlayerTargetSelector.kt:18-117).
#[derive(Resource, Default)]
pub struct TabCycleStack {
    /// Ordered candidate ids, front = next to pop. Built once per refresh.
    ids: VecDeque<u32>,
    /// Seconds since the last cycle advance (Tab / Enter-acquire). Ticked
    /// in `tab_cycle_invalidate_system`; past the reset threshold the stack
    /// is cleared so the next press rebuilds from the current nearest.
    idle_secs: f32,
    /// The id the cycle itself last wrote to `Target.id`. Lets
    /// `tab_cycle_invalidate_system` tell an internal advance (keep the
    /// stack) from an external target change — Esc / click / slash / party
    /// key (rebuild the stack) — without threading a reset through every
    /// `target.id =` site.
    last_emitted: Option<u32>,
}

/// Build the ordered Tab/Enter candidate list (front = best pick). Pure and
/// testable; the camera is injected as `project`.
///
/// `project` maps an FFXI world position to NDC (`[-1, 1]` x/y; z `[0, 1]`
/// for in-front-of-camera, outside that range = behind / clipped).
///
/// **On-screen only** (per retail): an entity must project inside the
/// (slightly relaxed, [`CYCLE_NDC_LIMIT`]) camera frustum to be eligible —
/// targets behind the player or well off-screen are never cycled.
///
/// Ordering keys, in priority order:
/// 1. **tier** — party members and the player's own pet sort to the *end*,
///    so plain Tab walks enemies/NPCs/other-PCs first (XIM
///    `getTargetableActors:168-169`).
/// 2. **strictly in-frustum** before the relaxed band, so the slightly-
///    off-screen "mob over the shoulder" entries come last.
/// 3. **hybrid score** `world_dist + NDC_PENALTY_YALMS·ndc_mag` — nearest
///    in world with a small screen-offset penalty, the "distance + facing"
///    proxy for an on-screen candidate set.
///
/// Self is excluded via `self_id` (the wire entity list includes the
/// player's own entry, matched by `self_char_id`).
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
        in_frustum: bool,
        score: f32,
    }

    let mut candidates: Vec<Cand> = entities
        .iter()
        .filter(|e| Some(e.id) != self_id)
        // `Other` is non-combat filler (doors, transports, unnamed
        // effects) — never a Tab/Enter target.
        .filter(|e| !matches!(e.kind, EntityKind::Other))
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
            // Lateral / vertical cull. Past the (slightly relaxed) screen
            // edge → off-screen → not eligible.
            if ndc.x.abs() > CYCLE_NDC_LIMIT || ndc.y.abs() > CYCLE_NDC_LIMIT {
                return None;
            }
            // 3D distance: server range checks use all three axes, so
            // the cycle's "closeness" key has to as well. On a slope, a
            // 2D-closer mob can be several yalms farther in 3D.
            let dx = e.pos.x - from.x;
            let dy = e.pos.y - from.y;
            let dz = e.pos.z - from.z;
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            let ndc_mag = (ndc.x * ndc.x + ndc.y * ndc.y).sqrt();
            let in_frustum = ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0;
            let tier = u8::from(party_ids.contains(&e.id) || owned_pet_ids.contains(&e.id));
            Some(Cand {
                id: e.id,
                tier,
                in_frustum,
                score: dist + NDC_PENALTY_YALMS * ndc_mag,
            })
        })
        .collect();

    candidates.sort_by(|a, b| {
        a.tier
            .cmp(&b.tier)
            // `true` (strictly in-frustum) sorts before `false`.
            .then_with(|| b.in_frustum.cmp(&a.in_frustum))
            .then_with(|| {
                a.score
                    .partial_cmp(&b.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    candidates.into_iter().map(|c| c.id).collect()
}

/// Advance the persistent stack one step and return the new target id.
///
/// The stack is what keeps the cycle stable: the ordered candidate list is
/// built once and consumed one pop per press; it rebuilds only when empty
/// (XIM `PlayerTargetSelector.kt:98-101`). Stale/vanished entries and the
/// current target are dropped from the front first (XIM `.kt:96`).
///
/// Passing `current = None` returns the first pick (nearest on-screen) and
/// seeds the stack with the rest — this is how Enter-with-no-target
/// acquires *and* leaves a cycle a following Tab can continue.
///
/// Returns `None` when there's nothing to advance to — either no on-screen
/// candidates at all, or the current target is the only one. The caller
/// treats `None` as "no change" and keeps the current target, never clears
/// it (XIM `targetCycle` returns `false` in the same cases).
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
    // Drop the current target + ids that have left the scene (XIM .kt:96).
    stack
        .ids
        .retain(|id| Some(*id) != current && entities.iter().any(|e| e.id == *id));
    // Refill only when exhausted — the order stays frozen until every
    // candidate this round has been visited.
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

/// Tick the cycle's idle timer and invalidate the stack on *external*
/// target changes. One change-detection system replaces a `stack.clear()`
/// at all eight `target.id =` sites.
///
/// The discriminator is [`TabCycleStack::last_emitted`]: `tab_cycle_next`
/// records every id it writes to `Target.id`, so an internal advance reads
/// back as `target.id == last_emitted` (keep the stack). Esc-clear, click,
/// slash `/target*`, party F-keys, and `auto_clear_target_system` all write
/// `Target.id` *without* touching `last_emitted`, so `id != last_emitted`
/// clears the stack and the next press rebuilds. On first insert both are
/// `None`, so `None != None` is false — no spurious clear.
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

/// State for the auto-recenter behavior. `manual_override` latches true
/// the moment the operator rotates the camera by hand (arrow keys or
/// mouse drag) and stays true until the operator moves the character.
/// While true, the post-motion chase-to-behind recenter is suppressed
/// so a manually-positioned camera holds its orbit. The
/// `forward_held_since` field is retained for diagnostic / future
/// timer-gated polish work.
#[derive(Resource, Default)]
pub struct CameraAutoRecenter {
    /// Reserved — recenter is no longer gated on a forward-hold timer.
    pub forward_held_since: Option<Instant>,
    /// True when the operator manually orbited the camera since the
    /// last character-movement input. Suppresses the chase recenter
    /// until a movement key clears it.
    pub manual_override: bool,
}

/// Spring constant for the post-motion chase recenter (1/sec).
/// Exponential lerp: each tick the residual angle shrinks by
/// `1 - exp(-rate · dt)`. At 3.0 the half-life is ~0.23s — the
/// camera settles fast and decisively after movement stops.
const AUTO_RECENTER_RATE: f32 = 3.0;
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
    pointer: Res<ffxi_viewer_core::MousePointer>,
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

    // Maintain the manual-orbit override flag.
    //   - Arrow-key yaw or mouse drag → operator is moving the camera by
    //     hand. Latch the override so we stop fighting them after they
    //     release.
    //   - Any character-movement input (W/S/A/D/strafe) → clear it. The
    //     player chose to move, so the camera should now follow.
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

    // --- (a) Persistent chase-to-behind-player ------------------------
    // Retail FFXI: the camera always wants to be behind the player.
    // Continuously runs (in chase mode only) as long as the operator
    // isn't actively rotating the camera with arrow keys. Subsumes the
    // old "MoveForward held >0.5s" recenter gate — the camera should
    // chase after A/D release too, until lag_c → 0.
    //
    // Exponential lerp at AUTO_RECENTER_RATE so the recenter is gentle
    // when the player is standing still (small residual → small step)
    // but doesn't fight the orbit-strafe's faster CHASE_TRACK_RATE
    // during sustained A/D (the orbit branch overwrites chase.yaw
    // every tick with a larger step before this runs).
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
    let eye = self_t.translation + Vec3::Y * ffxi_viewer_core::first_person_eye_y(None);
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
    let desired_pitch = to_head
        .y
        .atan2(horiz)
        .clamp(ChaseCamera::FP_PITCH_MIN, ChaseCamera::FP_PITCH_MAX);
    let max_step = FP_LOCK_PITCH_RATE * time.delta_secs();
    let diff = desired_pitch - chase.pitch;
    let step = diff.clamp(-max_step, max_step);
    chase.pitch += step;
}

/// How far past the strict `[-1, 1]` NDC frustum an entity may sit and
/// still appear in the Tab cycle. Retail is **on-screen only**, so this is
/// a small margin (1.1 ≈ 5% past each edge), not a generous one — it only
/// compensates for projecting against the entity *origin* (its feet) when
/// the body still straddles the screen edge. Strictly-in-frustum entities
/// (`|ndc| ≤ 1.0`) always sort ahead of this relaxed band, so it acts as a
/// fallback, not a co-equal pool. See [`build_tab_candidates`].
const CYCLE_NDC_LIMIT: f32 = 1.1;

/// Idle gap after which the Tab/Enter cycle stack resets, so the next press
/// rebuilds from the current nearest rather than continuing a stale order.
/// XIM uses 5s (`PlayerTargetSelector.kt:28`); retail feel per the user is
/// snappier.
const TAB_CYCLE_IDLE_RESET_SECS: f32 = 2.0;

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

    #[test]
    fn forward_allowance_caps_at_contact() {
        // 5 yalms apart, stop at 0.7 (two PC radii) → may advance 4.3.
        let a = forward_allowance((0.0, 0.0), (5.0, 0.0), 0.7);
        assert!((a - 4.3).abs() < 1e-3, "got {a}");
    }

    #[test]
    fn forward_allowance_zero_at_or_inside_contact() {
        // Exactly at the ring → no further advance.
        assert!(forward_allowance((0.0, 0.0), (0.7, 0.0), 0.7).abs() < 1e-6);
        // Already inside the ring → clamped to 0 (never negative → no push-out).
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

    fn from0() -> WireVec3 {
        WireVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }
    }

    /// Like [`ent`] but with an explicit kind (party/pet/npc tiering tests).
    fn ent_k(id: u32, x: f32, kind: EntityKind) -> WireEntity {
        let mut e = ent(id, x, 0.0);
        e.kind = kind;
        e
    }

    /// First Tab/Enter pick (nearest on-screen) with no party/pet present.
    fn first_pick<F: Fn(Vec3) -> Option<Vec3>>(
        entities: &[WireEntity],
        self_id: Option<u32>,
        project: F,
    ) -> Option<u32> {
        build_tab_candidates(entities, from0(), self_id, &[], &[], project)
            .first()
            .copied()
    }

    /// Drive the real stateful cycle `n` presses, feeding each result back
    /// as the next `current` — exactly how the Tab handler calls it.
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
        // ndc.x = pos.x/100; hybrid score = dist + 10·ndc_mag. Entity 2 is
        // both nearest and most centered, so it's the first pick.
        let entities = vec![ent(1, 30.0, 0.0), ent(2, 10.0, 0.0), ent(3, 20.0, 0.0)];
        assert_eq!(first_pick(&entities, None, fake_proj), Some(2));
    }

    #[test]
    fn cycle_excludes_self() {
        // Self (id=99) is in the wire entity list (matched by
        // `self_char_id`); it must never appear in the cycle.
        let entities = vec![ent(99, 0.0, 0.0), ent(1, 10.0, 0.0), ent(2, 20.0, 0.0)];
        assert_eq!(first_pick(&entities, Some(99), fake_proj), Some(1));
        // Driven cycle alternates 1 ↔ 2, never lands on self.
        assert_eq!(drive(&entities, Some(99), 4, fake_proj), vec![1, 2, 1, 2]);
    }

    #[test]
    fn first_press_3d_distance_includes_altitude() {
        // Identical (x, y) and identical NDC, distinguished only by FFXI z
        // (altitude). The closer-in-3D one wins — guards a 2D regression.
        let entities = vec![ent_xyz(1, 0.0, 0.0, 5.0), ent_xyz(2, 0.0, 0.0, 50.0)];
        assert_eq!(first_pick(&entities, None, fake_proj), Some(1));
    }

    #[test]
    fn first_press_close_off_center_beats_far_centered() {
        // Regression: the "Tab fails to select the closest mob" bug. A
        // close, slightly-off-axis entity must beat a far dead-centered one.
        //   Entity 1 (5, 30): ndc 0.05, dist 30.4, score 30.9
        //   Entity 2 (20, 5): ndc 0.20, dist 20.6, score 22.6  ← wins
        let entities = vec![ent(1, 5.0, 30.0), ent(2, 20.0, 5.0)];
        assert_eq!(first_pick(&entities, None, fake_proj), Some(2));
    }

    #[test]
    fn first_press_combined_ndc_and_world_distance() {
        // Centered-but-high entity loses to a slightly-off-center, much
        // closer one (the hybrid score combines both signals).
        //   Entity 1 (0,0,80): ndc (0, 0.8), dist 80, score 88
        //   Entity 2 (15,0,15): ndc (0.15, 0.15), dist 21.2, score 23.3 ← wins
        let entities = vec![ent_xyz(1, 0.0, 0.0, 80.0), ent_xyz(2, 15.0, 0.0, 15.0)];
        assert_eq!(first_pick(&entities, None, xy_proj), Some(2));
    }

    #[test]
    fn cycle_walks_nearest_to_farthest_then_wraps() {
        // Hybrid scores: id=2 (5.5), id=3 (16.5), id=1 (33) → order [2,3,1].
        // The driven cycle visits them in that order and wraps to the start.
        let entities = vec![ent(1, 30.0, 0.0), ent(2, 5.0, 0.0), ent(3, 15.0, 0.0)];
        assert_eq!(drive(&entities, None, 4, fake_proj), vec![2, 3, 1, 2]);
    }

    #[test]
    fn cycle_is_stable_under_position_jitter() {
        // The core regression: the OLD code recomputed the order every press
        // (the score shifts as entities/camera move), so Tab oscillated. The
        // stack freezes the order, so even with per-press position jitter a
        // single round visits every candidate exactly once before repeating.
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
            // Shuffle positions each press — a recompute WOULD reorder.
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
        // After a full round the stack rebuilds rather than returning None.
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
        // Plain Tab walks enemies/NPCs first; party members and the
        // player's own pet are tiered to the end (XIM parity).
        let entities = vec![
            ent(1, 10.0, 0.0),               // mob (tier 0)
            ent_k(2, 5.0, EntityKind::Pc),   // party PC (tier 1 via party_ids)
            ent_k(3, 15.0, EntityKind::Pet), // own pet (tier 1 via owned_pet_ids)
            ent_k(4, 20.0, EntityKind::Npc), // npc (tier 0)
        ];
        let order = build_tab_candidates(&entities, from0(), None, &[2], &[3], fake_proj);
        // tier 0 by score: 1 (11), 4 (22); tier 1 by score: 2 (5.5), 3 (16.5).
        assert_eq!(order, vec![1, 4, 2, 3]);
    }

    #[test]
    fn tab_keeps_current_when_it_is_the_only_candidate() {
        // Only the current target is on-screen → no advance. `tab_cycle_next`
        // returns None ("no change"); the caller keeps the current target
        // rather than clearing it. Regression guard for an empty Tab
        // deselecting the sole visible mob.
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

    #[test]
    fn other_kind_is_never_a_candidate() {
        // `Other` is non-combat filler (doors, transports) — excluded.
        let entities = vec![ent_k(1, 10.0, EntityKind::Other), ent(2, 20.0, 0.0)];
        assert_eq!(first_pick(&entities, None, fake_proj), Some(2));
    }

    #[test]
    fn advance_records_last_emitted_and_resets_idle() {
        // `last_emitted` is the discriminator `tab_cycle_invalidate_system`
        // uses to tell an internal advance from an external target change;
        // the idle timer resets on every advance.
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
        // On-screen-only, but with a small origin-vs-silhouette margin
        // (CYCLE_NDC_LIMIT = 1.1). wide_proj: ndc.x = x/50.
        //   Entity 1: x=-25 → -0.5 (strictly in frustum)
        //   Entity 2: x=52  →  1.04 (relaxed band, still eligible)
        //   Entity 3: x=70  →  1.4  (off-screen — excluded)
        // Strict sorts before relaxed; entity 3 never appears.
        let entities = vec![ent(1, -25.0, 0.0), ent(2, 52.0, 0.0), ent(3, 70.0, 0.0)];
        let order = build_tab_candidates(&entities, from0(), None, &[], &[], wide_proj);
        assert_eq!(order, vec![1, 2]);
    }

    #[test]
    fn first_press_prefers_strictly_in_frustum() {
        // wide_proj: ndc.x = x/50. Entity 1 (x=45 → 0.90) is strictly in
        // frustum; entity 2 (x=52 → 1.04) is in the relaxed band. The
        // strict one is the first pick.
        let entities = vec![ent(1, 45.0, 0.0), ent(2, 52.0, 0.0)];
        assert_eq!(first_pick(&entities, None, wide_proj), Some(1));
    }

    #[test]
    fn first_press_falls_back_to_relaxed_when_none_in_frustum() {
        // Nothing strictly in-view; first press still picks the closer
        // relaxed-band entity rather than returning None.
        //   Entity 1: x=55 → 1.10   Entity 2: x=52 → 1.04 (closer) ← wins
        let entities = vec![ent(1, 55.0, 0.0), ent(2, 52.0, 0.0)];
        assert_eq!(first_pick(&entities, None, wide_proj), Some(2));
    }

    #[test]
    fn off_screen_entities_are_skipped() {
        // entity 4 at x=100 is culled by `culled_proj` (x>50 → None).
        let entities = vec![ent(1, 0.0, 0.0), ent(4, 100.0, 0.0)];
        assert_eq!(first_pick(&entities, None, culled_proj), Some(1));
        // A current target that's off-screen falls back to nearest visible
        // rather than getting stuck.
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
        // All off-screen.
        let entities = vec![ent(1, 100.0, 0.0), ent(2, 200.0, 0.0)];
        assert_eq!(first_pick(&entities, None, culled_proj), None);
    }
}
