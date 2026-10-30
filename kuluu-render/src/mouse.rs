use bevy::input::mouse::{MouseButtonInput, MouseMotion, MouseWheel};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::camera::{
    yaw_for_heading, BodyFacingReference, CameraMode, CameraTransition, ChaseCamera, ViewFov,
    WheelZoom,
};
use crate::input_mode::InputMode;
use crate::snapshot::SceneState;

// The chase-camera aim laws, single-sourced here so keyboard, gamepad and mouse
// can never drift onto separate scales. Retail authors both per 1/60 s frame tick
// (`UpdatePlayerFollowingCamera`, FFXiMain.dll retail-2026-09 RVA 0x1EE60), so
// every coefficient below carries the x60 into seconds.

/// Azimuth a fully-deflected orbit axis sweeps, at retail's nominal rig radius
/// (FFXiMain.dll retail-2026-09 .rdata RVA 0x32A3EC per frame tick).
pub const CAMERA_ORBIT_RATE_RAD_PER_SEC: f32 = 0.027_924_445 * 60.0;

/// The radius the orbit normalises against, i.e. where `CAMERA_ORBIT_RATE_RAD_PER_SEC`
/// applies whole (FFXiMain.dll retail-2026-09 .rdata RVA 0x32A3E8).
pub const CAMERA_ORBIT_NOMINAL_RADIUS_YALMS: f32 = 6.0;

/// Divisor guard on that normalisation (FFXiMain.dll retail-2026-09 .rdata RVA
/// 0x329A18).
pub const CAMERA_AIM_MIN_DISTANCE_YALMS: f32 = 0.01;

/// Whether the followed actor currently lets the rig orbit freely, which decides whether the
/// orbit law normalises against radius at all. Addresses below are `FFXiMain.dll retail-2026-09`.
/// Retail reads one actor byte through a vtable slot: getter `vt+0x330` = RVA 0xA4670, and the two
/// non-skeleton classes sharing that slot return constant 1. The orbit site tests it as a boolean, so
/// the scaling block behind it is jumped over whenever the byte is non-zero — `call [eax + 0x330]`
/// (0x1F036), `test al,al` (0x1F03C), `jne 0x1f0a2` (0x1F03E) over a block that floors the
/// eye-to-lookat distance at `.rdata 0x329A18` and divides `.rdata 0x32A3E8` by it. The constructor
/// initialises the byte to 1 (0xA8A1C), so a plain retail actor free-runs and orbits at a flat angular
/// rate; only authored motion writing zero into that channel turns radius normalisation on. kuluu does
/// not parse that channel yet, so every caller passes this default; the settling read is the
/// joint-curve writer storing `round(curve x .rdata 0x329A20) & 0xff` into the byte (0x4BCE3/0x4BCF8).
pub const FOLLOW_ACTOR_FREE_RUN_DEFAULT: bool = true;

/// World-space eye rise a fully-deflected height axis produces; the law adds to the
/// eye's world Y and leaves the XZ offset alone (FFXiMain.dll retail-2026-09 .rdata
/// RVA 0x32A3E4 per frame tick).
pub const CAMERA_EYE_RISE_YALMS_PER_SEC: f32 = 0.106_666_67 * 60.0;

/// Azimuth rate of `axis` under the orbit law. A free-running actor (`free_run`, see
/// [`FOLLOW_ACTOR_FREE_RUN_DEFAULT`]) turns at a flat angular rate whatever the radius; only an
/// actor whose authored channel cleared that byte keeps constant tangential speed, so a closer rig
/// turns faster. Pass None as `eye_to_lookat_yalms` when the mode has no following-camera radius,
/// which leaves the normalisation inert regardless of `free_run`.
pub fn camera_orbit_yaw_rate_rad_per_sec(
    axis: f32,
    free_run: bool,
    eye_to_lookat_yalms: Option<f32>,
) -> f32 {
    let normalise = |distance: f32| {
        CAMERA_ORBIT_RATE_RAD_PER_SEC
            * (CAMERA_ORBIT_NOMINAL_RADIUS_YALMS / distance.max(CAMERA_AIM_MIN_DISTANCE_YALMS))
    };
    match (free_run, eye_to_lookat_yalms) {
        (false, Some(distance)) => axis * normalise(distance),
        _ => axis * CAMERA_ORBIT_RATE_RAD_PER_SEC,
    }
}

/// Pitch rate that reproduces retail's eye-Y rise inside a polar rig: at radius r
/// and elevation p the same world-space rise is `rise x cos(p) / r`. Pass None as
/// `eye_to_lookat_yalms` when the mode has no following-camera radius.
pub fn camera_height_pitch_rate_rad_per_sec(
    axis: f32,
    pitch_radians: f32,
    eye_to_lookat_yalms: Option<f32>,
) -> f32 {
    let rate = |distance: f32| {
        CAMERA_EYE_RISE_YALMS_PER_SEC * pitch_radians.cos()
            / distance.max(CAMERA_AIM_MIN_DISTANCE_YALMS)
    };
    axis * eye_to_lookat_yalms.map_or(
        CAMERA_EYE_RISE_YALMS_PER_SEC * pitch_radians.cos() / CAMERA_ORBIT_NOMINAL_RADIUS_YALMS,
        rate,
    )
}

/// Same screen-space convention as `mouse_aim_axis`; kuluu drives drag aim from this. The parked-
/// cursor path beside it comes from the block behind store 0x8F (gate FFXiMain.dll retail-2026-09
/// .text 0x124CC5, ±1 bands hugging the tracked screen rect at .text 0x124CEA..0x124D2A) and is
/// consumed through [`edge_pan_axis`].
pub fn mouse_drag_aim_axis(
    cursor: Vec2,
    anchor: Vec2,
    window_size: Vec2,
    saturation_fraction: f32,
) -> Vec2 {
    let span = window_size * saturation_fraction;
    if span.x <= 0.0 || span.y <= 0.0 {
        return Vec2::ZERO;
    }
    let offset = Vec2::new(cursor.x - anchor.x, anchor.y - cursor.y) / span;
    offset.clamp(Vec2::NEG_ONE, Vec2::ONE)
}

/// Retail aims from the cursor's absolute position, never its motion: the
/// normalized-offset accessors of the mouse input object take the cursor
/// against a screen rect and saturate toward ±1.0 at the edges (X `0x125FE0`,
/// Y `0x126100`; FFXiMain.dll retail-2026-09), and that handler emits virtual
/// arrow presses (dispatcher seed 0x125360, actions 0x16..0x19) which the same
/// integration as the real arrows consumes — no per-pixel radian sensitivity
/// exists on this path. Returns the saturated `(yaw+, pitch-up+)` aim axis from `cursor`, a
/// top-left-origin y-down window-relative position in logical pixels
/// like `CursorMoved::position`, against the same-unit `window_size`; degenerate
/// sizes return ZERO.
/// Which of those two accessors retail uses is decided by the mouse state byte of
/// `FFXiMain.dll retail-2026-09` (`[obj+0x4D]`, recorded as M36):
/// while it reads 5 or 4 the axis dispatcher takes the *anchor-relative* pair (RVA 0x126190 / 0x126200,
/// saturating at `.rdata` RVA 0x32A39C = 0.2 and 0x32DF9C = 1/21 of the extent), otherwise it returns
/// the stored absolute samples (`[obj+0x8C]` / `[obj+0x90]`, normalised by half the screen rect, which is
/// what this function computes; their writer is RVA 0x157720).
pub fn mouse_aim_axis(cursor: Vec2, window_size: Vec2) -> Vec2 {
    let half = 0.5 * window_size;
    if half.x <= 0.0 || half.y <= 0.0 {
        return Vec2::ZERO;
    }
    let offset = Vec2::new((cursor - half).x, (half - cursor).y) / half;
    offset.clamp(Vec2::NEG_ONE, Vec2::ONE)
}

#[derive(Resource, Debug, Default, Clone)]
pub struct MousePointer {
    pub cursor_pos: Option<Vec2>,

    pub left: bool,
    pub right: bool,
    pub middle: bool,

    pub delta: Vec2,

    pub wheel: f32,

    pub left_dragged: bool,

    pub right_dragged: bool,

    /// Per-button anchor and arm state behind those two flags.
    pub left_track: DragTrack,

    pub right_track: DragTrack,
}

/// Retail (`FFXiMain.dll retail-2026-09`) enters aim-drag through one of two arms, both measured per
/// button slot. The first is
/// displacement from the press anchor (`[obj + slot*8 + 0x1C/+0x20]`): squared cursor offset must
/// exceed `0x40`, i.e. more than eight pixels — `imul / imul / add; cmp edx, 0x40 / jg` at RVA
/// 0x125251 (right-button chain) and RVA 0x125111 (left-button chain).
const DRAG_ENGAGE_PX_SQ: f32 = 64.0;

/// The second arm (`FFXiMain.dll retail-2026-09`), for a button held down without travelling: each
/// slot carries a countdown armed to
/// `0x41c80000` = 25.0 (`[obj+0x78]` right at RVA 0x1252A6, `[obj+0x74]` left at RVA 0x1250CD and
/// 0x125063), the whole-tick clock is subtracted from it while the button is down (RVA
/// 0x1251FE..0x125216, 0x1250B0..0x1250C8) and entry is admitted once `fcomp 0.0` reports not-above
/// (`fld / fcomp dword [0x103295d8] / fnstsw ax / test ah, 0x41 / jp` at RVA 0x125256 and 0x125116).
/// That clock is the M29 tick — 1/60 s, two units per frame at the default cap — so 25 of them is
/// ~0.42 s of real time whatever the frame rate.
const DRAG_ENGAGE_HOLD_SECS: f32 = 25.0 / 60.0;

/// While dragging, retail's aim axes (`FFXiMain.dll retail-2026-09`) are not the cursor against the
/// screen centre but the cursor measured from the anchor it stores at `+0x38/+0x3C`, over a fifth of the extent and then
/// clamped to ±1 (`.rdata` RVA 0x32A39C = 0.2; accessors RVA 0x126190 / 0x126200, which the axis
/// dispatcher takes only while the mode byte reads 5 — see `mouse_drag_aim_axis`).
pub const DRAG_SATURATION_FRACTION: f32 = 0.2;

/// The left-button chain's saturation is a twenty-first of the extent instead (`FFXiMain.dll
/// retail-2026-09 .rdata` RVA 0x32DF9C
/// = `3d430c31` = 0.0476185), so it saturates almost three times sooner than the right one does.
pub const LEFT_DRAG_SATURATION_FRACTION: f32 = 1.0 / 21.0;

/// Where a held button measures from, and how long retail's two engagement arms have been running.
#[derive(Debug, Default, Clone, Copy)]
pub struct DragTrack {
    /// Press anchor (`[obj + slot*8 + 0x1C/+0x20]`). None while no cursor sample has been seen at
    /// all for this hold; the first sample observed while held becomes the anchor.
    pub press_pos: Option<Vec2>,
    /// Sum of `MouseMotion` deltas since the press, used only while there is no anchor to measure
    /// against — a platform that reports motion but never a cursor position still gets displacement.
    pub travel_since_press: Vec2,
    /// Seconds held and not yet engaged (retail's countdown slot `[obj+0x78]` / `[obj+0x74]`).
    pub held_secs: f32,
}

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct CursorLockRequest {
    pub locked: bool,
}

pub struct MousePlugin;

impl Plugin for MousePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MousePointer>()
            .init_resource::<CursorLockRequest>()
            .init_resource::<CameraMode>()
            .init_resource::<ChaseCamera>()
            .init_resource::<BodyFacingReference>()
            // A host adding only this plugin gets both resources its systems use.
            .init_resource::<ViewFov>()
            .init_resource::<WheelZoom>()
            .init_resource::<RetailCameraInversion>()
            .init_resource::<RetailEdgeRotate>()
            .add_systems(PreUpdate, collect_mouse_system)
            // Retail's wheel never moves the focal itself. The message handler only adds frames
            // owed (RVA 0x25E210); a frame function drains one per frame (RVA 0x25E240 through thunk
            // RVA 0x25E0E0); and the camera update spends whatever is owed, keys first. In kuluu
            // `dispatch_movement_system` is that consumer, so these two systems only keep the
            // counter honest (`FFXiMain.dll retail-2026-09`, M34).
            .add_systems(
                PreUpdate,
                wheel_zoom_accumulate_system.after(collect_mouse_system),
            )
            .add_systems(FixedPostUpdate, wheel_zoom_drain_system)
            .add_systems(Update, sync_camera_inversion_system)
            .add_systems(Update, sync_edge_rotate_system)
            .add_systems(
                Update,
                mouse_camera_system
                    .run_if(crate::cutscene::player_camera_allowed)
                    .after(sync_camera_inversion_system)
                    .after(sync_edge_rotate_system),
            );
    }
}

/// Retail's two engagement arms for one button slot, run every frame while that button is held. Both
/// arms come from the mouse state machine (M36): squared displacement from the press anchor, or expiry
/// of the hold countdown.
fn update_drag_engagement(
    held: bool,
    dragged: &mut bool,
    track: &mut DragTrack,
    cursor: Option<Vec2>,
    frame_motion: Vec2,
    delta_secs: f32,
) {
    if !held {
        // Retail's release path stores mode 0 and re-arms the countdown; neither survives a release.
        *dragged = false;
        *track = DragTrack::default();
        return;
    }
    let disp = if let (Some(anchor), Some(cursor)) = (track.press_pos, cursor) {
        cursor - anchor
    } else {
        // No anchor to measure against yet: integrate what this frame reported instead, and adopt the
        // first real sample as the anchor for later frames.
        track.travel_since_press += frame_motion;
        if track.press_pos.is_none() {
            track.press_pos = cursor;
        }
        track.travel_since_press
    };
    if disp.length_squared() > DRAG_ENGAGE_PX_SQ {
        *dragged = true;
        return;
    }
    if !*dragged {
        track.held_secs += delta_secs;
        if track.held_secs >= DRAG_ENGAGE_HOLD_SECS {
            *dragged = true;
        }
    }
}

pub fn collect_mouse_system(
    mode: Res<InputMode>,
    time: Res<Time>,
    mut motion: MessageReader<MouseMotion>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut wheel: MessageReader<MouseWheel>,
    mut cursor: MessageReader<CursorMoved>,
    windows: Query<(&Window, &CursorOptions), With<PrimaryWindow>>,
    mut state: ResMut<MousePointer>,
) {
    state.delta = Vec2::ZERO;
    state.wheel = 0.0;

    for ev in motion.read() {
        tracing::trace!(target: "input_trace", delta = ?ev.delta, "Raw mouse motion");
        state.delta += ev.delta;
    }
    let motion_delta = state.delta;
    let locked = windows.single().is_ok_and(|(window, options)| {
        window.focused && options.grab_mode == CursorGrabMode::Locked
    });
    if locked && (state.left || state.right) {
        for ev in cursor.read() {
            tracing::trace!(target: "input_trace", position = ?ev.position, locked, "Logical cursor position");
        }
        cursor.clear();
        if let Some(position) = state.cursor_pos.as_mut() {
            *position += motion_delta;
        }
    } else {
        for ev in cursor.read() {
            tracing::trace!(target: "input_trace", position = ?ev.position, locked, "Logical cursor position");
            state.cursor_pos = Some(ev.position);
        }
    }
    for ev in wheel.read() {
        tracing::trace!(target: "input_trace", unit = ?ev.unit, y = ev.y, "Mouse wheel input");
        state.wheel += ev.y;
    }
    if motion_delta != Vec2::ZERO || state.wheel != 0.0 {
        if let Ok((window, options)) = windows.single() {
            tracing::trace!(target: "input_trace", scale = window.scale_factor(), focused = window.focused, grab_mode = ?options.grab_mode, "Mouse window state");
        }
    }
    for ev in buttons.read() {
        let pressed = ev.state == ButtonState::Pressed;
        match ev.button {
            MouseButton::Left => {
                if pressed {
                    // The anchor is the cursor as it stands now: this frame's motion has already been
                    // folded into `cursor_pos`, which is retail's ordering too — its press handler
                    // stores the position from move messages that arrived earlier in the same frame.
                    state.left_dragged = false;
                    state.left_track = DragTrack {
                        press_pos: state.cursor_pos,
                        ..default()
                    };
                    // Winit coalesces the approach motion into the press's
                    // frame on long frames; that movement predates the hold
                    // and must not count as travel for this one.
                    state.delta = Vec2::ZERO;
                }
                state.left = pressed;
            }
            MouseButton::Right => {
                if pressed {
                    state.right_dragged = false;
                    state.right_track = DragTrack {
                        press_pos: state.cursor_pos,
                        ..default()
                    };
                    state.delta = Vec2::ZERO;
                }
                state.right = pressed;
            }
            MouseButton::Middle => state.middle = pressed,
            _ => {}
        }
    }

    let delta_secs = time.delta_secs();
    // Field-wise borrows: one `ResMut` deref, then the two slots are independent.
    let pointer = &mut *state;
    // Read after the button pass on purpose: a press zeroes this frame's motion, and travel since
    // that press must not include what happened before it.
    let cursor_pos = pointer.cursor_pos;
    let frame_motion = pointer.delta;
    update_drag_engagement(
        pointer.left,
        &mut pointer.left_dragged,
        &mut pointer.left_track,
        cursor_pos,
        frame_motion,
        delta_secs,
    );
    update_drag_engagement(
        pointer.right,
        &mut pointer.right_dragged,
        &mut pointer.right_track,
        cursor_pos,
        frame_motion,
        delta_secs,
    );

    if !matches!(*mode, InputMode::World) {
        state.delta = Vec2::ZERO;
        state.wheel = 0.0;
    }
}

/// The Mouse & Camera page's four axis-inversion flags. Each flag is one of its radio pairs, and the
/// store holds 0 for the "reset … to the default setting" row and 1 for the "invert … on the X/Y
/// axis" row (store ids [`crate::hud::menu::CONFIG_STORE_3P_INVERT_X`] and friends). Retail acts on
/// a set flag by negating that aim delta with −1.0f (.rdata RVA 0x32A3F0) — third-person from .text
/// FFXiMain.dll retail-2026-09 RVA 0x1EF7C/0x1EF9B, first person from RVA 0x20F8B/0x20FAA.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RetailCameraInversion {
    pub third_person_x: bool,
    pub third_person_y: bool,
    pub first_person_x: bool,
    pub first_person_y: bool,
}

fn inversion_flag(menu: &crate::hud::menu::DynamicMenu, store_id: u32) -> bool {
    menu.config_option_values
        .get(&store_id)
        .is_some_and(|value| *value != 0)
}

/// Copy the four Mouse & Camera inversion stores into [`RetailCameraInversion`]. A store nobody wrote
/// reads as not inverted, which is what retail's reset value means.
pub fn sync_camera_inversion_system(
    menu: Option<Res<crate::hud::menu::DynamicMenu>>,
    mut inversion: ResMut<RetailCameraInversion>,
) {
    let Some(menu) = menu else { return };
    if !menu.is_changed() && !inversion.is_added() {
        return;
    }
    let next = RetailCameraInversion {
        third_person_x: inversion_flag(&menu, crate::hud::menu::CONFIG_STORE_3P_INVERT_X),
        third_person_y: inversion_flag(&menu, crate::hud::menu::CONFIG_STORE_3P_INVERT_Y),
        first_person_x: inversion_flag(&menu, crate::hud::menu::CONFIG_STORE_FP_INVERT_X),
        first_person_y: inversion_flag(&menu, crate::hud::menu::CONFIG_STORE_FP_INVERT_Y),
    };
    if next != *inversion {
        *inversion = next;
    }
}

/// The screen-edge rotation band width: retail fires an axis when the cursor reaches `bound + 1`
/// from inside (guards at .text FFXiMain.dll retail-2026-09 RVA 0x124CE4/0x124CF4 and
/// 0x124D08..0x124D2A against the tracked screen rect's four word bounds).
pub const EDGE_PAN_BAND_PX: f32 = 1.0;

/// The discrete edge-park axis, in the same `(yaw+, pitch-up+)` convention as `mouse_aim_axis`: an
/// axis reads ±1 while the cursor sits within [`EDGE_PAN_BAND_PX`] of that screen border (the two
/// axes are independent, so a corner parks both), 0 elsewhere; degenerate sizes read ZERO. Retail's
/// own block stores −1/+1 per side in that sign order (`mov` bf800000 at .text FFXiMain.dll
/// retail-2026-09 RVA 0x124CEA etc.); the downstream pitch flip it applied is untraced image-wide,
/// so kuluu keeps its own up-positive convention and lets `apply_camera_inversion` gate the sign.
pub fn edge_pan_axis(cursor: Vec2, window_size: Vec2) -> Vec2 {
    if window_size.x <= 0.0 || window_size.y <= 0.0 {
        return Vec2::ZERO;
    }
    let mut axis = Vec2::ZERO;
    if cursor.x <= EDGE_PAN_BAND_PX {
        axis.x = -1.0;
    }
    if cursor.x >= window_size.x - 1.0 - EDGE_PAN_BAND_PX {
        // The +1 side is tested after the −1 side, exactly retail's instruction order; a band
        // overlap on a degenerate width resolves to the later test.
        axis.x = 1.0;
    }
    if cursor.y <= EDGE_PAN_BAND_PX {
        axis.y = 1.0;
    }
    if cursor.y >= window_size.y - 1.0 - EDGE_PAN_BAND_PX {
        axis.y = -1.0;
    }
    axis
}

/// Whether the Mouse & Camera page switched screen-edge rotation on. Only a written value of **0**
/// (row id 3 "The camera will rotate…") enables it: retail's gate skips the whole edge block for any
/// non-zero store (.text FFXiMain.dll retail-2026-09 RVA 0x124CC5), and its boot default is
/// table-driven/unfound, so an unwritten store keeps kuluu's current (off) behaviour.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RetailEdgeRotate {
    pub enabled: bool,
}

pub fn sync_edge_rotate_system(
    menu: Option<Res<crate::hud::menu::DynamicMenu>>,
    mut edge: ResMut<RetailEdgeRotate>,
) {
    let Some(menu) = menu else { return };
    if !menu.is_changed() && !edge.is_added() {
        return;
    }
    let enabled = menu
        .config_option_values
        .get(&crate::hud::menu::CONFIG_STORE_EDGE_ROTATE)
        .is_some_and(|value| *value == 0);
    if enabled != edge.enabled {
        edge.enabled = enabled;
    }
}

/// Negate the aim axis the page asked to invert. Only the sign changes, so saturation, the orbit-rate
/// law and the pitch clamps all stay in front of it.
pub fn apply_camera_inversion(
    axis: Vec2,
    mode: CameraMode,
    inversion: &RetailCameraInversion,
) -> Vec2 {
    let (flip_x, flip_y) = match mode {
        CameraMode::Chase => (inversion.third_person_x, inversion.third_person_y),
        CameraMode::FirstPerson => (inversion.first_person_x, inversion.first_person_y),
    };
    Vec2::new(
        if flip_x { -axis.x } else { axis.x },
        if flip_y { -axis.y } else { axis.y },
    )
}

/// Whether the control configuration that gates retail's body-facing reference consumer reads active.
/// The record ties the consumer to one value of a menu-side control setting but leaves which user-facing
/// setting that is among its material gaps (`.agents/skills/retail-observe/references/
/// 2026-10-07-camera-orbit-channels.md`, "Priority, applicability and missing laws"), so kuluu's one
/// control path reads active to carry the channel in the default input mode.
pub const BODY_REFERENCE_CONTROL_CONFIG_ACTIVE: bool = true;

/// The record's suppression of the direct manual angle (`.agents/skills/retail-observe/references/
/// 2026-10-07-camera-orbit-channels.md`, "Manual orbit input"): while the follow-mode flag is set, any
/// positive transition timer suppresses it, holding the orbit still across a camera cut.
pub fn manual_orbit_suppressed(transition_secs_remaining: f32, follow_mode: bool) -> bool {
    follow_mode && transition_secs_remaining > 0.0
}

/// Retail's followed-camera update does not let one-frame manual yaw reach the view as-is. Alongside
/// the direct angle it keeps a pending accumulator: after each polar-applier call the frame adds
/// `action6 × frame-tick × nought-point-one-oh-six-six-six` (float `[0x1032a3e4]`, build 2026-10-05, read from
/// the unpacked image) into `[obj+0x48]`, and while that pending angle stands against nought the update
/// latches the value **ten** (register `ebp` loaded at `0x1001f014`) into camera state dword `[0x10456d74]`
/// (`f_1001ee60`, sites `0x1001f0f7..0x1001f14d`; the pair `[obj+0x48]`/`[obj+0x54]` is then consumed against
/// threshold global `[0x10456d78]` at `0x1001fea4..0x1001fee5`). The apply semantics of that pair are not
/// resolved; what the playtest establishes (user, retail 2026-10-27) is the visible law: rapid alternating
/// keyboard ←/→ taps - each a fresh press cycle inside the latch window - move retail's camera not at all,
/// while kuluu applied every tap instantly and jackhammered.
///
/// Kuluu reproduces that observed law on its manual yaw press channel with a dwell of the same ten
/// frame ticks: angles from a press cycle apply only once it has been held continuously through the
/// window. The window length is the recorded latch value at kuluu's standing camera-tick measure
/// ([`CAMERA_FRAME_TICK_SECS`]); the internal pending/decay mechanism stays retail's.
pub const ORBIT_TAP_LATCH_FRAME_TICKS: f32 = 10.0;

/// The camera unit's frame tick, one sixtieth of a second - kuluu's standing measure for retail's
/// `f_10014cf0` multiplier; [`CAMERA_ORBIT_RATE_RAD_PER_SEC`] converts by the same factor.
pub const CAMERA_FRAME_TICK_SECS: f32 = 1.0 / 60.0;

/// How long a manual yaw press cycle must be held continuously before its angles reach the view:
/// [`ORBIT_TAP_LATCH_FRAME_TICKS`] camera frame ticks (a sixth of a second).
pub const ORBIT_TAP_DWELL_SECS: f32 = ORBIT_TAP_LATCH_FRAME_TICKS * CAMERA_FRAME_TICK_SECS;

/// One manual-yaw press cycle's dwell memory ([`ORBIT_TAP_DWELL_SECS`] above for the law and its
/// evidence). Keyboard and pad axes share it - retail feeds both devices into the one action6 channel,
/// so a stick flick-tap is gated exactly like a key tap. Mouse drag does not pass through here: that
/// chain gates earlier, on the button's engagement machine (`MousePointer::left_dragged`).
#[derive(Default, Clone)]
pub struct OrbitKeyDwell {
    held_secs: f32,
}

impl OrbitKeyDwell {
    /// Float slop on the press-clock comparison alone: `held_secs` accumulates frame deltas by addition,
    /// ten of which need not land bit-exactly on the window computed as a product. A frame short of the
    /// window sits a whole delta away (a sixtyth of a second), far beyond this epsilon.
    const COMPARE_EPS_SECS: f32 = 1e-6;

    /// Whether this frame's manual yaw may reach the view. Advances the press-cycle clock while any
    /// manual yaw input is held; release resets it. A direction flip inside one still-held cycle does
    /// not restart the dwell - retail's latch rides the action channel, not its sign.
    pub fn allow(&mut self, held_now: bool, delta_secs: f32) -> bool {
        if !held_now {
            self.held_secs = 0.0;
            return false;
        }
        self.held_secs += delta_secs;
        self.held_secs >= ORBIT_TAP_DWELL_SECS - Self::COMPARE_EPS_SECS
    }
}

/// Spend an aim axis on the rig through the orbit and eye-rise laws, inside the mode's pitch band.
/// The frame spends it in retail's order (camera record, "Body-facing reference channel"): the direct
/// manual angle first, under its transition-timer suppression, then the separately gated body-reference
/// correction through the same applier. Pitch is no part of either suppression.
pub fn orbit_chase_by_axis(
    axis: Vec2,
    mode: CameraMode,
    chase: &mut ChaseCamera,
    delta_secs: f32,
    body_facing: &BodyFacingReference,
    transition_secs_remaining: f32,
) {
    let rig = match mode {
        CameraMode::Chase => Some(chase.distance),
        CameraMode::FirstPerson => None,
    };
    if !manual_orbit_suppressed(transition_secs_remaining, FOLLOW_ACTOR_FREE_RUN_DEFAULT) {
        chase.yaw += camera_orbit_yaw_rate_rad_per_sec(axis.x, FOLLOW_ACTOR_FREE_RUN_DEFAULT, rig)
            * delta_secs;
    }
    chase.yaw += body_facing.post_orbit_angle(
        BODY_REFERENCE_CONTROL_CONFIG_ACTIVE,
        FOLLOW_ACTOR_FREE_RUN_DEFAULT,
        axis.x,
        transition_secs_remaining,
    );
    let (lo, hi) = match mode {
        CameraMode::Chase => (ChaseCamera::PITCH_MIN, ChaseCamera::PITCH_MAX),
        CameraMode::FirstPerson => (ChaseCamera::FP_PITCH_MIN, ChaseCamera::FP_PITCH_MAX),
    };
    chase.pitch = (chase.pitch
        + camera_height_pitch_rate_rad_per_sec(axis.y, chase.pitch, rig) * delta_secs)
        .clamp(lo, hi);
}

/// The remaining seconds of the frame's camera cut, which retail reads as its positive transition
/// timer (camera record, "Manual orbit input"): kuluu's one timed following-camera transition is the
/// mode-toggle [`CameraTransition`]; with none running the timer reads zero.
fn transition_secs_remaining(transition: Option<&CameraTransition>) -> f32 {
    transition
        .filter(|t| t.active)
        .map_or(0.0, |t| (1.0 - t.t).max(0.0) * t.duration)
}

pub fn mouse_camera_system(
    pointer: Res<MousePointer>,
    camera_mode: Res<CameraMode>,
    input_mode: Res<InputMode>,
    state: Res<SceneState>,
    move_intent: Option<Res<crate::combat_stance::SelfMoveIntent>>,
    inversion: Option<Res<RetailCameraInversion>>,
    edge_rotate: Option<Res<RetailEdgeRotate>>,
    body_facing: Res<BodyFacingReference>,
    transition: Option<Res<CameraTransition>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    time: Res<Time>,
    mut chase: ResMut<ChaseCamera>,

    mut prev_drag: Local<bool>,
) {
    let mode = *camera_mode;
    let transition_left = transition_secs_remaining(transition.as_deref());
    // A held button is not enough: retail aims only once the drag has engaged (its mode byte reads 4
    // or 5), and then measures from the press anchor, not the screen centre. Which chain wins when a
    // frame engages both follows retail's mouse handler, which runs the right-button chain first
    // (`call 0x125170` at RVA 0x1253A6) and the left-button chain last (RVA 0x1253AD), so the byte
    // ends up reading 4: kuluu checks left first. Addresses are `FFXiMain.dll retail-2026-09`. The
    // minimap pan claims the same cursor by clearing `cursor_pos` on the frames it owns.
    let drag = if pointer.left_dragged {
        Some((pointer.left_track.press_pos, LEFT_DRAG_SATURATION_FRACTION))
    } else if pointer.right_dragged {
        Some((pointer.right_track.press_pos, DRAG_SATURATION_FRACTION))
    } else {
        None
    };
    let drag_active = drag.is_some();
    if matches!(*input_mode, InputMode::World) {
        if let (Some((Some(anchor), saturation)), Some(cursor), Ok(window)) =
            (drag, pointer.cursor_pos, windows.single())
        {
            let inversion = inversion.as_deref().copied().unwrap_or_default();
            let axis = apply_camera_inversion(
                mouse_drag_aim_axis(cursor, anchor, window.size(), saturation),
                mode,
                &inversion,
            );
            let yaw_before = chase.yaw;
            orbit_chase_by_axis(
                axis,
                mode,
                &mut chase,
                time.delta_secs(),
                &body_facing,
                transition_left,
            );
            if axis != Vec2::ZERO {
                tracing::trace!(target: "input_trace", ?anchor, ?cursor, ?axis, yaw_delta = chase.yaw - yaw_before, delta_secs = time.delta_secs(), "Mouse camera drag");
            }
        }
    }
    // Parked-cursor rotation is the other aim feed and never runs beside a drag: retail's edge
    // block demands the mouse window mode byte read 0..2, while an engaged drag holds it at 4 or 5
    // (FFXiMain.dll retail-2026-09 .text 0x124B3C gate at 0x124CC5).
    if !drag_active
        && matches!(*input_mode, InputMode::World)
        && edge_rotate.is_some_and(|edge| edge.enabled)
    {
        if let (Some(cursor), Ok(window)) = (pointer.cursor_pos, windows.single()) {
            let axis = apply_camera_inversion(
                edge_pan_axis(cursor, window.size()),
                mode,
                &inversion.as_deref().copied().unwrap_or_default(),
            );
            if axis != Vec2::ZERO {
                orbit_chase_by_axis(
                    axis,
                    mode,
                    &mut chase,
                    time.delta_secs(),
                    &body_facing,
                    transition_left,
                );
            }
        }
    }

    // FP mouse-look is a temporary glance that snaps back to facing on
    // release — but only from a standstill. While moving, the pan steers the
    // heading (dispatch_movement_system commits chase.yaw as the run heading),
    // so snapping back to the lagged server heading would jerk the view.
    let moving = move_intent.is_some_and(|m| m.moving);
    if matches!(mode, CameraMode::FirstPerson) && *prev_drag && !drag_active && !moving {
        chase.yaw = yaw_for_heading(state.snapshot.self_pos.heading);
        chase.pitch = 0.0;
    }
    *prev_drag = drag_active;
}

/// Retail's `WM_MOUSEWHEEL` handler, and all it does (`FFXiMain.dll retail-2026-09`: zDelta/120 at
/// RVA `0x1DD8`, adder RVA `0x25E210`). It moves no focal and consults no camera mode — the backlog
/// simply grows, and only the camera update spends it. Keeping this apart from `mouse_camera_system`
/// also keeps a HUD that claims this frame's wheel (chat scroll, minimap pan) in front of the camera
/// zoom (M34).
pub fn wheel_zoom_accumulate_system(
    pointer: Res<MousePointer>,
    mode: Res<InputMode>,
    mut remainder: Local<f32>,
    mut owed: ResMut<WheelZoom>,
) {
    if !matches!(*mode, InputMode::World) {
        *remainder = 0.0;
        owed.clear();
        return;
    }
    *remainder += pointer.wheel;
    let notches = remainder.trunc() as i32;
    *remainder -= notches as f32;
    if notches != 0 {
        owed.add_notches(notches);
    }
}

// Drain after the fixed camera update so render-only frames cannot discard scroll input.
pub fn wheel_zoom_drain_system(mut owed: ResMut<WheelZoom>) {
    owed.drain_one();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_drag_uses_motion_when_cursor_position_stays_at_anchor() {
        const ANCHOR: Vec2 = Vec2::new(400.0, 300.0);
        const DELTA: Vec2 = Vec2::new(20.0, -10.0);
        let mut app = test_app();
        app.add_message::<MouseMotion>()
            .add_message::<MouseButtonInput>()
            .add_message::<MouseWheel>()
            .add_message::<CursorMoved>()
            .insert_resource(MousePointer {
                right: true,
                cursor_pos: Some(ANCHOR),
                right_track: DragTrack {
                    press_pos: Some(ANCHOR),
                    ..default()
                },
                ..default()
            })
            .add_systems(Update, collect_mouse_system);
        let window = app
            .world_mut()
            .spawn((
                Window {
                    focused: true,
                    ..default()
                },
                PrimaryWindow,
                CursorOptions {
                    grab_mode: CursorGrabMode::Locked,
                    ..default()
                },
            ))
            .id();
        for count in 1..=3 {
            app.world_mut().write_message(MouseMotion { delta: DELTA });
            app.world_mut().write_message(CursorMoved {
                window,
                position: ANCHOR,
                delta: Some(Vec2::ZERO),
            });
            app.update();
            assert_eq!(
                app.world().resource::<MousePointer>().cursor_pos,
                Some(ANCHOR + DELTA * count as f32)
            );
        }
        app.world_mut()
            .get_mut::<CursorOptions>(window)
            .unwrap()
            .grab_mode = CursorGrabMode::None;
        app.world_mut().write_message(CursorMoved {
            window,
            position: ANCHOR,
            delta: None,
        });
        app.update();
        assert_eq!(
            app.world().resource::<MousePointer>().cursor_pos,
            Some(ANCHOR)
        );
    }

    #[test]
    fn collect_mouse_resets_per_frame_signals() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseMotion>()
            .add_message::<MouseButtonInput>()
            .add_message::<MouseWheel>()
            .add_message::<CursorMoved>()
            .init_resource::<InputMode>()
            .init_resource::<SceneState>()
            .add_plugins(MousePlugin);

        app.world_mut().write_message(MouseMotion {
            delta: Vec2::new(3.0, 4.0),
        });
        app.world_mut().write_message(MouseWheel {
            phase: bevy::input::touch::TouchPhase::Moved,
            unit: bevy::input::mouse::MouseScrollUnit::Line,
            x: 0.0,
            y: 1.0,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        let p = app.world().resource::<MousePointer>();
        assert_eq!(p.delta, Vec2::new(3.0, 4.0));
        assert_eq!(p.wheel, 1.0);

        app.update();
        let p = app.world().resource::<MousePointer>();
        assert_eq!(p.delta, Vec2::ZERO);
        assert_eq!(p.wheel, 0.0);
    }

    #[test]
    fn motion_coalesced_into_the_press_frame_is_not_a_drag() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseMotion>()
            .add_message::<MouseButtonInput>()
            .add_message::<MouseWheel>()
            .add_message::<CursorMoved>()
            .init_resource::<InputMode>()
            .init_resource::<SceneState>()
            .add_plugins(MousePlugin);

        // The cursor's approach and the button press arrive in one frame (a
        // long frame under load); the click must not be read as a drag.
        app.world_mut().write_message(MouseMotion {
            delta: Vec2::new(300.0, 200.0),
        });
        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Left,
            state: ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert!(!app.world().resource::<MousePointer>().left_dragged);

        // Motion on later frames while held is a real drag.
        app.world_mut().write_message(MouseMotion {
            delta: Vec2::new(30.0, 0.0),
        });
        app.update();
        assert!(app.world().resource::<MousePointer>().left_dragged);
    }

    #[test]
    fn collect_mouse_button_state_is_sticky() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseMotion>()
            .add_message::<MouseButtonInput>()
            .add_message::<MouseWheel>()
            .add_message::<CursorMoved>()
            .init_resource::<InputMode>()
            .init_resource::<SceneState>()
            .add_plugins(MousePlugin);

        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Right,
            state: ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert!(app.world().resource::<MousePointer>().right);

        app.update();
        assert!(
            app.world().resource::<MousePointer>().right,
            "button stays pressed across frames until a release event"
        );

        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Right,
            state: ButtonState::Released,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert!(!app.world().resource::<MousePointer>().right);
    }

    #[test]
    fn mouse_aim_axis_saturates_at_the_window_edges() {
        let size = Vec2::new(800.0, 600.0);
        // Centre of the window is no aim at all.
        assert_eq!(mouse_aim_axis(Vec2::new(400.0, 300.0), size), Vec2::ZERO);

        // Half-way to the right edge reads half deflection, level pitch.
        let mid = mouse_aim_axis(Vec2::new(600.0, 300.0), size);
        assert!(
            (mid.x - 0.5).abs() < 1e-6,
            "half-way right → x=0.5, got {mid}"
        );
        assert!(mid.y.abs() < 1e-6, "level cursor must not pitch: {mid}");

        // Parked past the edge saturates at ±1 in both axes; above centre is +
        // (pitch up), and below centre is −.
        assert_eq!(mouse_aim_axis(Vec2::new(5000.0, -5000.0), size), Vec2::ONE);
        assert_eq!(
            mouse_aim_axis(Vec2::new(-5000.0, 5000.0), size),
            Vec2::new(-1.0, -1.0)
        );

        // A degenerate (unmeasurable) window must not divide by zero.
        assert_eq!(mouse_aim_axis(Vec2::ZERO, Vec2::ZERO), Vec2::ZERO);
    }

    #[test]
    fn mouse_camera_chase_drag_requires_a_held_button() {
        let cursor = Vec2::new(700.0, 300.0); // right of centre in an 800x600 window

        let mut app = aim_test_app(MousePointer {
            right: false,
            cursor_pos: Some(cursor),
            ..Default::default()
        });
        // Frame one carries a zero delta on the real-time clock, so every
        // assertion here rides after a second update.
        app.update();
        app.update();
        assert_eq!(
            app.world().resource::<ChaseCamera>().yaw,
            ChaseCamera::default().yaw,
            "a parked button must not turn the camera"
        );

        let mut app = aim_test_app(MousePointer {
            right: true,
            right_dragged: true,
            right_track: DragTrack {
                press_pos: Some(Vec2::new(400.0, 300.0)),
                ..Default::default()
            },
            cursor_pos: Some(cursor),
            ..Default::default()
        });
        app.update();
        app.update();
        assert!(
            app.world().resource::<ChaseCamera>().yaw > ChaseCamera::default().yaw,
            "cursor right of centre with a button held must turn yaw +"
        );
    }

    #[test]
    fn mouse_camera_aim_follows_the_cursor_quadrants() {
        // Right of centre, vertically centred: yaw only.
        let mut app = aim_test_app(MousePointer {
            right: true,
            right_dragged: true,
            right_track: DragTrack {
                press_pos: Some(Vec2::new(400.0, 300.0)),
                ..Default::default()
            },
            cursor_pos: Some(Vec2::new(700.0, 300.0)),
            ..Default::default()
        });
        let pitch_before = app.world().resource::<ChaseCamera>().pitch;
        app.update();
        app.update();
        let chase = app.world().resource::<ChaseCamera>();
        assert!(chase.yaw > 0.0);
        assert_eq!(chase.pitch, pitch_before, "a level cursor must not pitch");

        // Above centre, horizontally centred: pitch up only (clamped to the
        // mode's band).
        let mut app = aim_test_app(MousePointer {
            left: true,
            left_dragged: true,
            left_track: DragTrack {
                press_pos: Some(Vec2::new(400.0, 300.0)),
                ..Default::default()
            },
            cursor_pos: Some(Vec2::new(400.0, 100.0)),
            ..Default::default()
        });
        app.update();
        app.update();
        let chase = app.world().resource::<ChaseCamera>();
        assert_eq!(chase.yaw, ChaseCamera::default().yaw);
        assert!(
            chase.pitch > pitch_before && chase.pitch <= ChaseCamera::PITCH_MAX,
            "cursor above centre must raise pitch inside the band"
        );
    }

    #[test]
    fn mouse_camera_fp_aim_and_band() {
        let mut app = aim_test_app(MousePointer {
            right: true,
            right_dragged: true,
            right_track: DragTrack {
                press_pos: Some(Vec2::new(400.0, 300.0)),
                ..Default::default()
            },
            cursor_pos: Some(Vec2::new(700.0, 100.0)),
            ..Default::default()
        });
        app.insert_resource(CameraMode::FirstPerson);
        app.update();
        app.update();
        let chase = app.world().resource::<ChaseCamera>();
        assert!(chase.yaw > ChaseCamera::default().yaw);
        assert!(
            chase.pitch > ChaseCamera::default().pitch && chase.pitch <= ChaseCamera::FP_PITCH_MAX,
            "first-person aim raises pitch inside the FP band"
        );

        // Below centre pitches down toward FP_PITCH_MIN (never past it).
        let mut app = aim_test_app(MousePointer {
            right: true,
            right_dragged: true,
            right_track: DragTrack {
                press_pos: Some(Vec2::new(400.0, 300.0)),
                ..Default::default()
            },
            cursor_pos: Some(Vec2::new(400.0, 590.0)),
            ..Default::default()
        });
        app.insert_resource(CameraMode::FirstPerson);
        app.update();
        app.update();
        let chase = app.world().resource::<ChaseCamera>();
        assert!(chase.pitch < ChaseCamera::default().pitch);
    }

    #[test]
    fn mouse_camera_aim_needs_world_input_mode() {
        use crate::input_mode::ChatBuffer;
        let mut app = aim_test_app(MousePointer {
            right: true,
            right_dragged: true,
            right_track: DragTrack {
                press_pos: Some(Vec2::new(400.0, 300.0)),
                ..Default::default()
            },
            cursor_pos: Some(Vec2::new(790.0, 300.0)),
            ..Default::default()
        });
        app.insert_resource(InputMode::Chat(ChatBuffer::empty()));
        app.update();
        app.update();
        assert_eq!(
            app.world().resource::<ChaseCamera>().yaw,
            ChaseCamera::default().yaw,
            "a held button while chatting must not turn the camera"
        );
    }

    #[test]
    fn mouse_camera_aim_stops_when_the_cursor_is_claimed_elsewhere() {
        // The minimap pan suppresses camera aim by taking cursor_pos away (it
        // already consumed motion the same way); no position, no aim.
        let mut app = aim_test_app(MousePointer {
            right: true,
            right_dragged: true,
            right_track: DragTrack {
                press_pos: Some(Vec2::new(400.0, 300.0)),
                ..Default::default()
            },
            cursor_pos: None,
            ..Default::default()
        });
        app.update();
        app.update();
        assert_eq!(
            app.world().resource::<ChaseCamera>().yaw,
            ChaseCamera::default().yaw
        );
    }

    #[test]
    fn the_wheel_buys_frames_of_the_zoom_rate_instead_of_stepping_itself() {
        // Retail's wheel handler touches no focal and knows no camera mode: a scroll becomes
        // `notches × 2` frames owed (RVA 0x25E210), drained toward zero one per frame (RVA 0x25E240).
        // The step itself is the keys' `tick × 6.0`, run once per owed frame by the camera update
        // (`ViewFov::step`), which lives with the keys in `dispatch_movement_system`. So this side of
        // the split must leave the focal alone and owe exactly six frames for three notches up,
        // fourteen for seven down (`FFXiMain.dll retail-2026-09`, M34).
        for (notches, owed_after_scroll) in [(3.0_f32, 6_i32), (-7.0, -14)] {
            let mut app = test_app();
            app.add_message::<MouseMotion>()
                .add_message::<MouseButtonInput>()
                .add_message::<MouseWheel>()
                .add_message::<CursorMoved>();
            app.add_plugins(MousePlugin)
                .insert_resource(CameraMode::Chase)
                .insert_resource(ChaseCamera::default());

            app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_secs_f64(
                    1.0 / crate::scheduler_runtime::ROUTINE_FPS as f64,
                ),
            ));
            app.insert_resource(Time::<Fixed>::from_hz(
                crate::scheduler_runtime::ROUTINE_FPS as f64,
            ));
            app.update();
            let rest = app.world().resource::<ViewFov>().focal_length;
            // The scroll frame: the handler adds the frames owed, then the frame function's drain
            // takes one of them.
            app.world_mut().write_message(MouseWheel {
                phase: bevy::input::touch::TouchPhase::Moved,
                unit: bevy::input::mouse::MouseScrollUnit::Line,
                x: 0.0,
                y: notches,
                window: Entity::PLACEHOLDER,
            });
            app.update();
            assert_eq!(
                app.world().resource::<WheelZoom>().frames_owed(),
                owed_after_scroll - owed_after_scroll.signum()
            );

            // And it is a duration: exactly `|owed|` frames in total, each of them drain-only.
            let mut frames = 1;
            while app.world().resource::<WheelZoom>().frames_owed() != 0 && frames < 40 {
                app.update();
                frames += 1;
            }
            assert_eq!(frames, owed_after_scroll.abs());
            assert_eq!(
                app.world().resource::<ViewFov>().focal_length,
                rest,
                "the wheel side must not integrate the focal at all"
            );
        }
    }

    #[test]
    fn mouse_camera_never_moves_the_chase_distance() {
        // One zoom law only: pending wheel leaves the rig's distance alone in every camera mode.
        for (wheel, mode) in [
            (3.0, CameraMode::Chase),
            (-100.0, CameraMode::Chase),
            (5.0, CameraMode::FirstPerson),
        ] {
            let mut app = test_app();
            app.insert_resource(MousePointer {
                wheel,
                ..Default::default()
            })
            .insert_resource(mode)
            .insert_resource(ChaseCamera::default())
            .init_resource::<BodyFacingReference>()
            .add_systems(Update, mouse_camera_system);
            app.update();
            assert_eq!(
                app.world().resource::<ChaseCamera>().distance,
                ChaseCamera::default().distance,
                "wheel {wheel} moved the chase distance in {mode:?}"
            );
        }
    }

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        app.init_resource::<InputMode>();
        app.init_resource::<ViewFov>();
        app.insert_resource(SceneState::default());
        app
    }

    /// A chase-mode app with an explicit 800x600 primary window, for the
    /// position-based aim: `pointer` is inserted verbatim.
    fn aim_test_app(pointer: MousePointer) -> App {
        let mut app = test_app();
        let mut window = Window::default();
        window.resolution = bevy::window::WindowResolution::new(800, 600);
        app.world_mut().spawn((window, PrimaryWindow));
        app.insert_resource(pointer)
            .insert_resource(CameraMode::Chase)
            .insert_resource(ChaseCamera::default())
            .init_resource::<BodyFacingReference>()
            .add_systems(Update, mouse_camera_system);
        app
    }

    #[test]
    fn collect_mouse_zeros_signals_when_input_mode_is_not_world() {
        use crate::input_mode::ChatBuffer;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseMotion>()
            .add_message::<MouseButtonInput>()
            .add_message::<MouseWheel>()
            .add_message::<CursorMoved>()
            .insert_resource(InputMode::Chat(ChatBuffer::empty()))
            .init_resource::<SceneState>()
            .add_plugins(MousePlugin);

        app.world_mut().write_message(MouseMotion {
            delta: Vec2::new(10.0, 0.0),
        });
        app.update();
        assert_eq!(
            app.world().resource::<MousePointer>().delta,
            Vec2::ZERO,
            "non-World input mode suppresses motion delta"
        );
    }

    #[test]
    fn a_free_running_actor_orbits_at_the_same_angular_rate_at_every_radius() {
        // The byte is 1 from the constructor, so this is what retail does unless authored motion
        // clears it: rate per second is radius-independent.
        let near =
            camera_orbit_yaw_rate_rad_per_sec(1.0, true, Some(CAMERA_ORBIT_NOMINAL_RADIUS_YALMS));
        let far = camera_orbit_yaw_rate_rad_per_sec(1.0, true, Some(24.0));
        assert!(
            (near - far).abs() < 1e-6 && (near - CAMERA_ORBIT_RATE_RAD_PER_SEC).abs() < 1e-6,
            "free-run orbit must hold {CAMERA_ORBIT_RATE_RAD_PER_SEC} rad/s at any radius, near={near} far={far}"
        );
    }

    #[test]
    fn only_an_actor_byte_of_zero_makes_a_closer_rig_turn_faster() {
        let rate = |radius: f32| camera_orbit_yaw_rate_rad_per_sec(1.0, false, Some(radius));
        assert!(
            (rate(CAMERA_ORBIT_NOMINAL_RADIUS_YALMS) - CAMERA_ORBIT_RATE_RAD_PER_SEC).abs() < 1e-6
        );
        // Halving the rig radius must exactly double the angular rate: constant tangential speed.
        let half = rate(CAMERA_ORBIT_NOMINAL_RADIUS_YALMS / 2.0);
        let base = rate(CAMERA_ORBIT_NOMINAL_RADIUS_YALMS);
        assert!(
            (half - 2.0 * base).abs() <= 1e-6 * base,
            "the normalising arm must double the angular rate when the rig halves: half={half} base={base}"
        );
    }

    #[test]
    fn height_axis_rises_at_world_rate_not_angular_rate() {
        // At zero elevation the polar rig maps the world-space rise straight onto
        // an angle, so one second of a fully deflected axis must lift the eye by
        // `CAMERA_EYE_RISE_YALMS_PER_SEC` whatever the radius.
        let rise =
            |radius: f32| camera_height_pitch_rate_rad_per_sec(1.0, 0.0, Some(radius)) * radius;
        let near = rise(CAMERA_ORBIT_NOMINAL_RADIUS_YALMS);
        let far = rise(18.0);
        assert!((near - far).abs() < 1e-4 && (near - CAMERA_EYE_RISE_YALMS_PER_SEC).abs() < 1e-4);
    }

    #[test]
    fn drag_engagement_measures_displacement_from_the_press_anchor() {
        let anchor = Vec2::new(400.0, 300.0);
        let mut track = DragTrack {
            press_pos: Some(anchor),
            ..Default::default()
        };
        let mut dragged = false;
        // Six pixels from the press sits inside `DRAG_ENGAGE_PX_SQ`: what counts is where the cursor
        // ended up, not how far it moved in this frame.
        update_drag_engagement(
            true,
            &mut dragged,
            &mut track,
            Some(Vec2::new(406.0, 300.0)),
            Vec2::new(9.0, 0.0),
            1.0 / 60.0,
        );
        assert!(!dragged, "six pixels from the anchor must not engage");
        update_drag_engagement(
            true,
            &mut dragged,
            &mut track,
            Some(Vec2::new(409.0, 300.0)),
            Vec2::ZERO,
            1.0 / 60.0,
        );
        assert!(dragged, "nine pixels from the anchor must engage");

        // Wandering about inside the gate never engages either: once retail has an anchor it does not
        // accumulate travel.
        let mut track = DragTrack {
            press_pos: Some(anchor),
            ..Default::default()
        };
        let mut dragged = false;
        for (step, pos) in [(8.0, 407.0), (-8.0, 399.0), (8.0, 405.0)] {
            update_drag_engagement(
                true,
                &mut dragged,
                &mut track,
                Some(Vec2::new(pos, 300.0)),
                Vec2::new(step, 0.0),
                1.0 / 60.0,
            );
        }
        assert!(!dragged, "travel that ends inside the gate must not engage");

        // Release drops engagement and re-arms both arms.
        update_drag_engagement(
            false,
            &mut dragged,
            &mut track,
            Some(Vec2::new(700.0, 300.0)),
            Vec2::ZERO,
            1.0 / 60.0,
        );
        assert!(!dragged);
        assert_eq!(track.held_secs, 0.0);
        assert_eq!(track.press_pos, None);
    }

    #[test]
    fn drag_engagement_hold_arm_runs_out_at_twenty_five_ticks() {
        // Pressed with no cursor sample yet: the distance arm has nothing to measure, so only retail's
        // countdown can engage it - and it must take the whole 25 tick units.
        let mut track = DragTrack::default();
        let mut dragged = false;
        update_drag_engagement(
            true,
            &mut dragged,
            &mut track,
            None,
            Vec2::ZERO,
            DRAG_ENGAGE_HOLD_SECS - 0.01,
        );
        assert!(!dragged, "a hold short of the countdown must not engage");
        update_drag_engagement(true, &mut dragged, &mut track, None, Vec2::ZERO, 0.01);
        assert!(
            dragged,
            "the countdown arm must engage at 25 tick units (~0.42 s)"
        );
    }

    #[test]
    fn mouse_drag_aim_axis_saturates_at_a_fifth_of_the_extent() {
        let size = Vec2::new(800.0, 600.0);
        let anchor = Vec2::new(400.0, 300.0);
        assert_eq!(
            mouse_drag_aim_axis(anchor, anchor, size, DRAG_SATURATION_FRACTION),
            Vec2::ZERO
        );

        // A fifth of an 800-wide extent is 160 px, which `mouse_drag_aim_axis` clamps to +/-1, so half
        // that distance -- 80 px right of the press -- must read half deflection.
        let mid = mouse_drag_aim_axis(
            Vec2::new(480.0, 300.0),
            anchor,
            size,
            DRAG_SATURATION_FRACTION,
        );
        assert!((mid.x - 0.5).abs() < 1e-5 && mid.y.abs() < 1e-5, "{mid}");
        let edge = mouse_drag_aim_axis(
            Vec2::new(560.0, 300.0),
            anchor,
            size,
            DRAG_SATURATION_FRACTION,
        );
        assert!((edge.x - 1.0).abs() < 1e-5, "{edge}");

        // Screen-down stays negative pitch here as everywhere else: a fifth of the height (120 px)
        // down pins it to -1.
        let down = mouse_drag_aim_axis(
            Vec2::new(400.0, 420.0),
            anchor,
            size,
            DRAG_SATURATION_FRACTION,
        );
        assert!(down.x.abs() < 1e-5 && (down.y + 1.0).abs() < 1e-5, "{down}");

        // `LEFT_DRAG_SATURATION_FRACTION` is a twenty-first rather than a fifth of the extent, so it
        // pins nearly three times sooner for the same offset.
        let left_at_40 = mouse_drag_aim_axis(
            Vec2::new(440.0, 300.0),
            anchor,
            size,
            LEFT_DRAG_SATURATION_FRACTION,
        );
        assert!(
            (left_at_40.x - 1.0).abs() < 1e-5,
            "left must already be pinned at 40 px: {left_at_40}"
        );
        let left_half = mouse_drag_aim_axis(
            Vec2::new(400.0 + size.x / 42.0, 300.0),
            anchor,
            size,
            LEFT_DRAG_SATURATION_FRACTION,
        );
        assert!((left_half.x - 0.5).abs() < 1e-3, "{left_half}");

        // A degenerate extent must not divide by zero.
        assert_eq!(
            mouse_drag_aim_axis(Vec2::ZERO, Vec2::ZERO, Vec2::ZERO, DRAG_SATURATION_FRACTION),
            Vec2::ZERO
        );
    }

    #[test]
    fn an_inverted_page_entry_negates_only_that_rig_and_axis() {
        let inversion = RetailCameraInversion {
            third_person_x: true,
            first_person_y: true,
            ..RetailCameraInversion::default()
        };
        assert_eq!(
            apply_camera_inversion(Vec2::new(1.0, -2.0), CameraMode::Chase, &inversion),
            Vec2::new(-1.0, -2.0)
        );
        assert_eq!(
            apply_camera_inversion(Vec2::new(1.0, -2.0), CameraMode::FirstPerson, &inversion),
            Vec2::new(1.0, 2.0)
        );
    }

    /// The Mouse & Camera page writes the store; the live flag follows it — and the pair's reset row
    /// (value 0) takes the inversion away again.
    #[test]
    fn camera_inversion_follows_the_config_stores() {
        let mut menu = crate::hud::menu::DynamicMenu::default();
        menu.config_option_values
            .insert(crate::hud::menu::CONFIG_STORE_3P_INVERT_Y, 1);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(menu)
            .init_resource::<RetailCameraInversion>()
            .add_systems(Update, sync_camera_inversion_system);
        app.update();

        assert_eq!(
            *app.world().resource::<RetailCameraInversion>(),
            RetailCameraInversion {
                third_person_y: true,
                ..RetailCameraInversion::default()
            }
        );

        app.world_mut()
            .resource_mut::<crate::hud::menu::DynamicMenu>()
            .config_option_values
            .insert(crate::hud::menu::CONFIG_STORE_3P_INVERT_Y, 0);
        app.update();
        assert_eq!(
            *app.world().resource::<RetailCameraInversion>(),
            RetailCameraInversion::default(),
            "the reset row restores the default"
        );
    }

    #[test]
    fn edge_pan_axis_bands_hug_the_screen_borders() {
        let size = Vec2::new(800.0, 600.0);
        assert_eq!(edge_pan_axis(Vec2::new(400.0, 300.0), size), Vec2::ZERO);

        // The −1 side fires through bound+1 and the +1 side from (size−1)−1, two pixels deep each.
        assert_eq!(
            edge_pan_axis(Vec2::new(1.0, 300.0), size),
            Vec2::new(-1.0, 0.0)
        );
        assert_eq!(edge_pan_axis(Vec2::new(2.0, 300.0), size), Vec2::ZERO);
        assert_eq!(
            edge_pan_axis(Vec2::new(798.0, 300.0), size),
            Vec2::new(1.0, 0.0)
        );
        assert_eq!(edge_pan_axis(Vec2::new(797.0, 300.0), size), Vec2::ZERO);

        // Top band reads pitch-up, bottom pitch-down; a corner parks both axes at once.
        assert_eq!(
            edge_pan_axis(Vec2::new(400.0, 1.0), size),
            Vec2::new(0.0, 1.0)
        );
        assert_eq!(
            edge_pan_axis(Vec2::new(400.0, 598.0), size),
            Vec2::new(0.0, -1.0)
        );
        assert_eq!(
            edge_pan_axis(Vec2::new(799.0, 599.0), size),
            Vec2::new(1.0, -1.0)
        );

        // Degenerate sizes must not fire from a cursor that exists nowhere.
        assert_eq!(edge_pan_axis(Vec2::ZERO, Vec2::ZERO), Vec2::ZERO);
    }

    /// Only the rotate row (`conf7` id 3, store value 0) switches the feed on; the "will not rotate"
    /// row (value 1) and an unwritten store both keep it off.
    #[test]
    fn edge_rotate_follows_the_config_store() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(crate::hud::menu::DynamicMenu::default())
            .init_resource::<RetailEdgeRotate>()
            .add_systems(Update, sync_edge_rotate_system);
        app.update();
        assert!(
            !app.world().resource::<RetailEdgeRotate>().enabled,
            "an unwritten store keeps retail's edge rotation off"
        );

        app.world_mut()
            .resource_mut::<crate::hud::menu::DynamicMenu>()
            .config_option_values
            .insert(crate::hud::menu::CONFIG_STORE_EDGE_ROTATE, 0);
        app.update();
        assert!(app.world().resource::<RetailEdgeRotate>().enabled);

        app.world_mut()
            .resource_mut::<crate::hud::menu::DynamicMenu>()
            .config_option_values
            .insert(crate::hud::menu::CONFIG_STORE_EDGE_ROTATE, 1);
        app.update();
        assert!(!app.world().resource::<RetailEdgeRotate>().enabled);
    }

    #[test]
    fn a_parked_cursor_rotates_only_when_edge_rotate_is_on() {
        let parked = || MousePointer {
            cursor_pos: Some(Vec2::new(799.0, 300.0)),
            ..Default::default()
        };

        // Without the config row the same parked cursor turns nothing.
        let mut app = aim_test_app(parked());
        app.update();
        app.update();
        assert_eq!(
            app.world().resource::<ChaseCamera>().yaw,
            ChaseCamera::default().yaw,
            "edge rotation stays off until the Mouse & Camera page switches it on"
        );

        let mut app = aim_test_app(parked());
        app.insert_resource(RetailEdgeRotate { enabled: true });
        app.update();
        app.update();
        let chase = app.world().resource::<ChaseCamera>();
        assert!(
            chase.yaw > ChaseCamera::default().yaw,
            "parked right turns yaw +"
        );
        assert_eq!(chase.pitch, ChaseCamera::default().pitch);

        // A mid-screen cursor with the feed on is still no aim at all.
        let mut app = aim_test_app(MousePointer {
            cursor_pos: Some(Vec2::new(400.0, 300.0)),
            ..Default::default()
        });
        app.insert_resource(RetailEdgeRotate { enabled: true });
        app.update();
        app.update();
        assert_eq!(
            app.world().resource::<ChaseCamera>().yaw,
            ChaseCamera::default().yaw
        );

        // The page's X-inversion row flips the parked rotation like any other aim delta.
        let mut app = aim_test_app(MousePointer {
            cursor_pos: Some(Vec2::new(1.0, 300.0)),
            ..Default::default()
        });
        app.insert_resource(RetailEdgeRotate { enabled: true });
        app.insert_resource(RetailCameraInversion {
            third_person_x: true,
            ..RetailCameraInversion::default()
        });
        app.update();
        app.update();
        assert!(app.world().resource::<ChaseCamera>().yaw > ChaseCamera::default().yaw);

        // A drag owns the cursor: its absence of motion must not be re-added by the edge feed.
        let mut app = aim_test_app(MousePointer {
            right: true,
            right_dragged: true,
            right_track: DragTrack {
                press_pos: Some(Vec2::new(799.0, 300.0)),
                ..Default::default()
            },
            cursor_pos: Some(Vec2::new(799.0, 300.0)),
            ..Default::default()
        });
        app.insert_resource(RetailEdgeRotate { enabled: true });
        app.update();
        app.update();
        assert_eq!(
            app.world().resource::<ChaseCamera>().yaw,
            ChaseCamera::default().yaw,
            "the drag axis is zero at its own press point, and the edge feed never joins it"
        );
    }

    /// The suppression law over the direct manual angle: positive transition time answers only while
    /// follow mode is set (camera record, "Manual orbit input").
    #[test]
    fn a_positive_transition_timer_suppresses_the_direct_angle_in_follow_mode() {
        assert!(manual_orbit_suppressed(0.35, true));
        assert!(!manual_orbit_suppressed(0.35, false));
        assert!(!manual_orbit_suppressed(0.0, true));
    }

    #[test]
    fn the_latch_window_is_the_recorded_ten_camera_frame_ticks() {
        // Latch value ten at f_1001ee60 site 0x1001f141 (pending angle standing against nought);
        // kuluu's camera-tick measure is one sixtieth of a second.
        assert_eq!(ORBIT_TAP_LATCH_FRAME_TICKS, 10.0);
        assert!((ORBIT_TAP_DWELL_SECS - 10.0 / 60.0).abs() < 1e-7);
    }

    #[test]
    fn rapid_alternating_yaw_taps_below_the_latch_apply_nothing() {
        // The user's retail playtest of 2026-10-27: clicking ←/→ rapidly does not move the view. Eight
        // press cycles of two frames each - shorter than the ten-frame latch every time - must open the
        // gate never, so every tap contributes exactly zero angle.
        let mut dwell = OrbitKeyDwell::default();
        let dt = 1.0 / 60.0;
        for _ in 0..8 {
            for held in [true, true, false] {
                assert!(
                    !dwell.allow(held, dt),
                    "a tap cycle under the latch window opened the gate"
                );
            }
        }
    }

    #[test]
    fn a_press_held_through_the_latch_opens_on_the_tenth_frame_and_stays_open() {
        let mut dwell = OrbitKeyDwell::default();
        let dt = 1.0 / 60.0;
        let mut opened_at = None;
        for frame in 1..=25 {
            if dwell.allow(true, dt) && opened_at.is_none() {
                opened_at = Some(frame);
            }
        }
        assert_eq!(
            opened_at,
            Some(10),
            "the gate must open on the tenth held frame and not before"
        );
    }

    #[test]
    fn a_release_restarts_the_latch() {
        let mut dwell = OrbitKeyDwell::default();
        let dt = 1.0 / 60.0;
        // Warm one press cycle through its own dwell first: frames 1-9 of a fresh cycle are shut by
        // construction, so only require the gate to stand open once past the window.
        let mut opened = false;
        for _ in 0..12 {
            opened |= dwell.allow(true, dt);
        }
        assert!(opened, "a twelve-frame hold never opened the gate");
        assert!(
            dwell.allow(true, dt),
            "an open cycle must stay open frame to frame"
        );
        assert!(
            !dwell.allow(false, dt),
            "a released key must not hold the gate open"
        );
        for frame in 1..=9 {
            assert!(
                !dwell.allow(true, dt),
                "the fresh press cycle opened early at frame {frame}"
            );
        }
        assert!(
            dwell.allow(true, dt),
            "the fresh press cycle never opened on frame 10"
        );
    }

    /// One rig frame retail's way (`orbit_chase_by_axis`): the direct angle first and an active body
    /// reference contributing its negated term after it; with transition time remaining both horizontal
    /// contributions read zero, and a frame without manual action never spends the reference.
    #[test]
    fn an_active_body_reference_springs_back_after_the_direct_angle() {
        let dt = 1.0 / 60.0;
        let axis = Vec2::new(0.5, 0.0);
        let direct = || {
            camera_orbit_yaw_rate_rad_per_sec(
                0.5,
                FOLLOW_ACTOR_FREE_RUN_DEFAULT,
                Some(ChaseCamera::default().distance),
            ) * dt
        };

        let mut reference = BodyFacingReference::default();
        let mut chase = ChaseCamera::default();
        orbit_chase_by_axis(axis, CameraMode::Chase, &mut chase, dt, &reference, 0.0);
        assert!(
            (chase.yaw - direct()).abs() < 1e-6,
            "no reference: the direct angle alone"
        );

        reference.apply_heading_change(-0.3);
        let mut chase = ChaseCamera::default();
        orbit_chase_by_axis(axis, CameraMode::Chase, &mut chase, dt, &reference, 0.0);
        assert!(
            (chase.yaw - (direct() + 0.3)).abs() < 1e-6,
            "the negated term lands after the direct angle"
        );

        let mut chase = ChaseCamera::default();
        orbit_chase_by_axis(axis, CameraMode::Chase, &mut chase, dt, &reference, 0.2);
        assert_eq!(
            chase.yaw,
            ChaseCamera::default().yaw,
            "transition time holds both"
        );

        let mut chase = ChaseCamera::default();
        orbit_chase_by_axis(
            Vec2::ZERO,
            CameraMode::Chase,
            &mut chase,
            dt,
            &reference,
            0.0,
        );
        assert_eq!(
            chase.yaw,
            ChaseCamera::default().yaw,
            "zero action never spends it"
        );
    }
}
