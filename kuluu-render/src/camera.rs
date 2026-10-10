use bevy::camera::{Camera3dDepthTextureUsage, Hdr};
use bevy::light::{ShadowFilteringMethod, VolumetricFog};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::render::render_resource::TextureUsages;

#[cfg(not(target_arch = "wasm32"))]
use bevy::anti_alias::taa::TemporalAntiAliasing;

use crate::components::IsSelf;
#[cfg(not(target_arch = "wasm32"))]
use crate::graphics_settings::AaMode;
use crate::graphics_settings::GraphicsSettings;
use crate::scene::{BakedActor, NameplateLocator};

/// Kept for the camera systems and the client's collision clamp, which all
/// subtract `offset` from the anchor Y. Step smoothing now happens at the
/// source (the rendered self Transform Y is low-pass filtered in
/// `apply_self_prediction_system`), so this offset stays 0 — the field exists
/// so every camera-path anchor stays wired through one place if a camera-side
/// offset is ever needed again.
#[derive(Resource, Default)]
pub struct CameraStepSmoothing {
    pub offset: f32,
}

/// Rate-limited follow of the player position, used as the chase-camera anchor
/// instead of the raw player Transform. When the player starts moving the
/// anchor lags briefly (hesitates); each tick it moves toward the player at a
/// speed proportional to the gap (up to a cap). When the player stops, the
/// gap shrinks and the speed goes to zero — the anchor coasts in and stops
/// exactly on the player. No overshoot, no oscillation.
///
/// Not a spring: a spring's restoring force keeps momentum after target is
/// reached and produces bounce. Here velocity is DERIVED from the current
/// gap every tick, so hitting the target is a fixed point.
#[derive(Resource, Default)]
pub struct AnchorFollow {
    /// The smoothed anchor position (world space). None until first sample,
    /// then set to the player's position and updated each tick.
    pub pos: Option<Vec3>,
}

pub fn reset_camera_follow(
    mut follow: ResMut<AnchorFollow>,
    mut step: ResMut<CameraStepSmoothing>,
) {
    *follow = AnchorFollow::default();
    *step = CameraStepSmoothing::default();
}

/// Retail's chase-camera pivot-height law (`FFXiMain.dll retail-2026-09`, RVAs
/// `0x1F3A2..0x1F446`): the followed actor's skeleton box (vt+0x3CC, read as
/// `ffxi_dat::skel::Skeleton::height_span`) supplies a vertical span, capped at `2.5` (compare
/// against .rdata 0x329ea0, imm store at 0x1F3CC) when actor predicate vt+0x14C holds (ported as
/// always-on, its identity is [I]); the pivot sits span × `0.6` (.rdata 0x329a30) above the feet.
const ANCHOR_MAX_SPAN: f32 = 2.5;
const ANCHOR_SPAN_SCALE: f32 = 0.6;

#[inline]
pub fn anchor_bias_y(h_span: f32) -> f32 {
    h_span.min(ANCHOR_MAX_SPAN) * ANCHOR_SPAN_SCALE
}

const FIRST_PERSON_EYE_FRAC: f32 = 0.92;

pub const FALLBACK_ACTOR_HEIGHT: f32 = 2.3;

/// The pivot height over the actor's feet: from its skeleton's span, else from its measured mesh
/// extent when it was built without one.
#[inline]
pub fn third_person_anchor_y(baked: Option<&BakedActor>) -> f32 {
    anchor_bias_y(
        baked
            .map(|b| b.skeleton_span.unwrap_or(b.actor_height))
            .unwrap_or(FALLBACK_ACTOR_HEIGHT),
    )
}

#[inline]
pub fn first_person_eye_y(baked: Option<&BakedActor>) -> f32 {
    baked
        .map(|b| b.actor_height)
        .unwrap_or(FALLBACK_ACTOR_HEIGHT)
        * FIRST_PERSON_EYE_FRAC
}

// research/XIClient/src/XIClient/source/World/Actor/SkeletalMeshActor.cpp::GetElem,
// root-bone chocobo branch. Other mount/chair policies remain separate parity work.
const MOUNTED_ANCHOR_RISE: f32 = 1.3;

pub fn nameplate_anchor(
    model: &Transform,
    locator: Option<&NameplateLocator>,
    mounted: bool,
) -> Option<Vec3> {
    let locator = locator?;
    let mut offset = locator.offset?;
    if mounted && locator.root_attached {
        offset.y += MOUNTED_ANCHOR_RISE * locator.model_scale;
    }
    Some(model.translation + offset * model.scale)
}

#[derive(Component)]
pub struct OperatorCamera;

/// viewer-core owns no phase state machine, and the launcher backdrop drives the same
/// `SceneState` the in-game path does. The operator camera is spawned `OnEnter(InGame)` and
/// reaped with the rest of the `InGameEntity` set on exit, so its presence is the in-game
/// gate for systems that must not fire behind the character-select screen.
pub fn in_game(cameras: Query<(), With<OperatorCamera>>) -> bool {
    !cameras.is_empty()
}

pub const WORLD_GIZMO_LAYER: usize = 2;

pub fn configure_gizmo_render_layer(mut store: ResMut<bevy::gizmos::config::GizmoConfigStore>) {
    let (config, _) = store.config_mut::<bevy::gizmos::config::DefaultGizmoConfigGroup>();
    config.render_layers = bevy::camera::visibility::RenderLayers::layer(WORLD_GIZMO_LAYER);
}

#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CameraMode {
    #[default]
    Chase,
    FirstPerson,
}

#[derive(Resource)]
pub struct ChaseCamera {
    pub yaw: f32,

    pub pitch: f32,

    pub distance: f32,

    pub smoothing: f32,

    pub synced_initial: bool,

    /// Set on zone-in: the player teleported, so the next chase update places
    /// the eye directly behind them instead of smoothing across zones.
    pub snap_to_anchor: bool,

    /// Yaw a Q/E turn has given the camera that it has yet to swing through:
    /// the chase update pays it round the player at the lock-release catch,
    /// so the camera trails the turn a little and settles once the keys come
    /// up. A lock, a zone snap or a manual camera turn drops it.
    pub turn_owed: f32,
}

impl ChaseCamera {
    pub const PITCH_MIN: f32 = -0.30;

    /// Retail has no pitch clamp, because retail has no pitch: tilting adds to
    /// the eye's world Y (`CurrentEyePosition.y += offset`,
    /// research/XIClient/src/XIClient/source/World/Camera/CameraManager.cpp CameraManager::UpdatePlayerFollowingCamera) and leaves
    /// the horizontal offset alone. What bounds the tilt is
    /// [`Self::MIN_XZ_STANDOFF`] against [`Self::DIST_MAX`], and for a polar eye
    /// that is `acos(3/6)` — exactly 60°, against the 80° an uncited 1.40 used
    /// to allow.
    ///
    /// The 20° is load-bearing for camera collision, not feel. At 80° the eye
    /// sits 1.0 yalm horizontally from the anchor, so the anchor→eye segment is
    /// short and near-vertical — it threads the unauthored gap in the Lower
    /// Jeuno ceiling instead of hitting its underside (kuluu-64fh). At 60° the
    /// segment can never be nearer than 3 horizontally and stays oblique.
    pub const PITCH_MAX: f32 = std::f32::consts::FRAC_PI_3;

    pub const FP_PITCH_MIN: f32 = -std::f32::consts::FRAC_PI_2 + 0.05;

    pub const FP_PITCH_MAX: f32 = std::f32::consts::FRAC_PI_2 - 0.05;

    /// CameraManager.cpp CameraManager::UpdatePlayerFollowingCamera pushes an unobstructed eye back out whenever
    /// the 3D eye→target distance drops below 3.
    pub const DIST_MIN: f32 = 3.0;

    /// The same file:837-846 applies a second, independent floor to the
    /// *horizontal* separation — `(eye - target).MagnitudeXZ() < 3` is pushed
    /// straight back out in XZ, leaving the eye's Y untouched. It shares
    /// retail's literal with [`Self::DIST_MIN`] but is a different constraint:
    /// this one is what makes a tilted retail camera swing wide rather than
    /// climb over its target.
    pub const MIN_XZ_STANDOFF: f32 = 3.0;

    /// Retail's chase camera works in a much tighter band than a modern MMO's.
    /// Three independent references put the nominal radius at 6, and none of
    /// them admits anything like a 20-yalm pull-back:
    ///
    /// - research/XIClient/src/XIClient/source/World/Camera/CameraManager.cpp CameraManager::UpdatePlayerFollowingCamera normalises the
    ///   orbit rate against it — `angle = 6.0f / eyeToTargetDistance * angle`.
    /// - Same file:822, the camera-follow easing changes regime above 6.
    /// - research/xim/src/jsMain/kotlin/xim/poc/camera/PolarCamera.kt PolarCamera `maximumRadius = 6f`.
    ///
    /// The resting distance is nearer still: CameraManager.cpp CameraManager::CalculateDefaultCameraPosition v10 places the
    /// default eye at `{-3, 0, 0}` behind the actor, and :404 falls back to -4.
    ///
    /// This is load-bearing for camera collision, not just feel. Zone collision
    /// is authored coarsely — Lower Jeuno has ceiling over x=15.7 and none over
    /// x=17.7 — so a camera allowed 20 yalms out and ~19 up exits the building
    /// through gaps retail's camera never reaches (kuluu-64fh).
    pub const DIST_MAX: f32 = 6.0;

    pub const KEYBOARD_ZOOM_RATE: f32 = 10.0;

    /// Retail tilts by lifting the eye's Y, not by orbiting it, so its
    /// horizontal separation never shrinks as you look down — the eye→target
    /// distance grows instead, and CameraManager.cpp CameraManager::UpdatePlayerFollowingCamera eases it back toward
    /// [`Self::DIST_MAX`]. A polar eye reproduces that reachable envelope by
    /// growing its radius on demand rather than trading horizontal for
    /// vertical.
    pub fn orbit_radius(&self) -> f32 {
        let cos_p = self.pitch.cos().abs().max(f32::EPSILON);
        self.distance
            .max(Self::MIN_XZ_STANDOFF / cos_p)
            .clamp(Self::DIST_MIN, Self::DIST_MAX)
    }
}

impl Default for ChaseCamera {
    fn default() -> Self {
        Self {
            yaw: 0.0,

            pitch: 0.15,
            // Rests fully zoomed out, as XIM does (`previousRadius = radiusMax`,
            // PolarCamera.kt PolarCamera previousRadius). Was 18.0, which is now past DIST_MAX.
            distance: Self::DIST_MAX,
            smoothing: 0.18,
            synced_initial: false,
            snap_to_anchor: false,
            turn_owed: 0.0,
        }
    }
}

/// The operator camera's projection focal length — the live value the view zoom
/// (mouse wheel, PgUp/PgDn, `.`, `,`) integrates and [`apply_view_fov_system`] turns into the
/// vertical FOV it pushes each frame. Runtime-only: `GraphicsSettings::fov_deg` is the base it
/// re-seats from (in focal units), so a menu FOV change wins over a held zoom. Zooming the window (not the chase
/// distance) keeps world-space nameplate billboards constant on screen — their size derives from
/// `tan(fov/2)` (nameplate_billboard.rs), which is how retail's focal-driven projection behaves.
#[derive(Resource)]
pub struct ViewFov {
    /// Retail's camera-manager field of the same role (`cam+0x2F4`, read by its zoom integrator
    /// and by the projection build).
    pub focal_length: f32,
}

impl Default for ViewFov {
    fn default() -> Self {
        Self {
            focal_length: crate::graphics_settings::RETAIL_DEFAULT_FOCAL_LENGTH,
        }
    }
}

/// Which way [`ViewFov::step`] moves the focal length, named after the two retail zoom-key slots
/// it comes from (device `0x3F` actions `0x4F` / `0x50`, `FFXiMain.dll retail-2026-09`
/// RVA `0x1F812` / `0x1F86A`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomArm {
    /// Lengthens the focal toward [`ViewFov::MAX_FOCAL`] — narrower fov, closer view.
    In,
    /// Shortens it toward [`ViewFov::MIN_FOCAL`] — wider fov, further view.
    Out,
}

impl ViewFov {
    /// The one rate retail's zoom has: focal units per `1/60 s` frame tick (`FFXiMain.dll
    /// retail-2026-09` `.rdata` RVA `0x32A3E8` = `6.0f`). Keys (RVA `0x1F82F`, `0x1F87F`) and both
    /// mouse-wheel arms (RVA `0x1F8D5`, `0x1F91D`) all apply exactly this same product — there is
    /// no separate wheel sensitivity, only a longer or shorter *duration* it runs for (M34).
    /// Because the tick counts elapsed frames, multiplying by it keeps the focal moving at
    /// 6 × 60 = **360 focal per second** at any refresh rate (`tick` is `delta_secs × 60`, M29).
    pub const FOCAL_STEP_PER_TICK: f32 = 6.0;

    /// Frames the mouse-wheel accumulator owes per notch, i.e. how many frames of that one rate a
    /// notch buys (`FFXiMain.dll retail-2026-09` RVA `0x25E210`: `acc += notches * 2`, fed by the
    /// raw zDelta/120 at RVA `0x1DD8`; drained toward zero once per frame from RVA `0x25E240`).
    pub const WHEEL_FRAMES_PER_NOTCH: i32 = 2;

    /// Zoomed fully out (wide): focal clamp `242.0f` (`FFXiMain.dll retail-2026-09` `.rdata
    /// 0x32A3D4`, M17) ≈ 76.9°.
    pub const MIN_FOCAL: f32 = 242.0;

    /// Zoomed fully in (tight): focal clamp `900.0f` (`FFXiMain.dll retail-2026-09` `.rdata
    /// 0x32A3D8`, M17) ≈ 24.1°.
    pub const MAX_FOCAL: f32 = 900.0;

    /// The projection's vertical fov, in degrees, for a focal length over retail's fixed
    /// projection half-height: `FOV = 2·atan(192/focal)` (M17/Q7 pass of the disassembly docs; the
    /// formula itself is [web]-tier — XIClient `CMoElem::VirtOt1`, and it is what
    /// `graphics_settings`' derived default already ships on).
    pub fn deg_for_focal(focal_length: f32) -> f32 {
        (2.0 * (crate::graphics_settings::RETAIL_PROJECTION_HALF_HEIGHT / focal_length).atan())
            .to_degrees()
    }

    /// Inverse of [`Self::deg_for_focal`]: the focal length that gives `deg`, for re-seating the
    /// zoom on a menu FOV. Not clamped to the band — retail's clamps live in the key integration,
    /// and the menu row is product freedom beyond them.
    pub fn focal_for_deg(deg: f32) -> f32 {
        crate::graphics_settings::RETAIL_PROJECTION_HALF_HEIGHT / (deg.to_radians() * 0.5).tan()
    }

    /// The single zoom rate applied for one frame: `focal ± tick × 6.0` toward the arm's band end,
    /// returning the new focal and whether this frame hit that end (retail tests each bound with an
    /// x87 compare immediately after adding — `FFXiMain.dll retail-2026-09` RVA `0x1F83E` against
    /// `900.0`, `0x1F88E`/`0x1F92C` against `242.0` — and clamping the stored value, M34). Passing
    /// the elapsed frame count (M29: `delta_secs × 60`) reproduces retail's step exactly; one notch
    /// of the wheel is just this same call running for [`Self::WHEEL_FRAMES_PER_NOTCH`] frames.
    pub fn step(focal_length: f32, tick_frames: f32, arm: ZoomArm) -> (f32, bool) {
        let step = Self::FOCAL_STEP_PER_TICK * tick_frames;
        match arm {
            ZoomArm::In => {
                let next = focal_length + step;
                if next >= Self::MAX_FOCAL {
                    (Self::MAX_FOCAL, true)
                } else {
                    (next, false)
                }
            }
            ZoomArm::Out => {
                let next = focal_length - step;
                if next <= Self::MIN_FOCAL {
                    (Self::MIN_FOCAL, true)
                } else {
                    (next, false)
                }
            }
        }
    }

    /// One frame of the both-zoom-keys-held path. Retail flags the pair (`[0x10456D84] = 1`, RVA
    /// `0x1F806`) and from then on, while the flag stands, eases toward neutral by a quarter of the
    /// remaining difference — `ease = (350 − focal) × 0.25` (`.rdata` RVAs `0x32A3DC`, `0x329CE4`) —
    /// unless that step has fallen inside one focal unit (`−1.0 < ease < 1.0`, `.rdata` RVAs
    /// `0x32A3F0`, `0x32961C`), when it stores exactly `350.0` and clears the flag (RVA
    /// `0x1F76E..0x1F7C1`). Returns the new focal and whether it settled, i.e. whether retail
    /// cleared its flag (`FFXiMain.dll retail-2026-09`, M34; this replaces the "unreachable ease"
    /// reading in the earlier M17 pass — the ease is ordinary behaviour for any real gap).
    pub fn ease_to_neutral(focal_length: f32) -> (f32, bool) {
        let neutral = crate::graphics_settings::RETAIL_DEFAULT_FOCAL_LENGTH;
        let ease = (neutral - focal_length) * Self::NEUTRAL_EASE_FRACTION;
        if ease.abs() < Self::NEUTRAL_SNAP_BELOW_FOCAL {
            (neutral, true)
        } else {
            (focal_length + ease, false)
        }
    }

    /// Ease fraction of the remaining difference toward neutral, applied per frame on retail's
    /// both-keys path (`FFXiMain.dll retail-2026-09` `.rdata` RVA `0x329CE4`).
    const NEUTRAL_EASE_FRACTION: f32 = 0.25;

    /// The step below which retail stops easing and stores neutral exactly, in focal units — its two
    /// convergence compares bracket the ease at `.rdata` RVAs `0x32A3F0` and `0x32961C`
    /// (`FFXiMain.dll retail-2026-09`).
    const NEUTRAL_SNAP_BELOW_FOCAL: f32 = 1.0;

    /// The vertical fov in degrees that `focal_length` projects to.
    pub fn deg(&self) -> f32 {
        Self::deg_for_focal(self.focal_length)
    }
}

/// Retail's mouse-wheel zoom backlog: one signed frame count, `+= 2 × notches` as each
/// `WM_MOUSEWHEEL` arrives (`FFXiMain.dll retail-2026-09` RVA `0x1DD8` divides raw zDelta by 120 and
/// RVA `0x25E210` stores `acc += notches × 2` into `[0x1067A298]`), drained toward zero by one per
/// frame (RVA `0x25E240`, called once from the frame function at RVA `0x1295A` via thunk RVA
/// `0x25E0E0`), and cleared outright when a zoom arm hits a band end or a zoom key runs (RVA
/// `0x25E230`, called from RVAs `0x1F8A7`, `0x1F8FF`, `0x1F945`). Holding a wheel therefore buys
/// [`ViewFov::WHEEL_FRAMES_PER_NOTCH`] frames of the same focal rate the keys use — retail has one
/// rate and two durations (M34).
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WheelZoom {
    /// Retail's `[0x1067A298]`: frames of zoom still owed, sign = direction.
    frames_owed: i32,
}

impl WheelZoom {
    /// Notches in — retail's wheel adder (`FFXiMain.dll retail-2026-09` RVA `0x25E210`).
    pub fn add_notches(&mut self, notches: i32) {
        self.frames_owed += ViewFov::WHEEL_FRAMES_PER_NOTCH * notches;
    }

    /// The arm the owed frames drive — positive owes zoom-in (retail's `acc > 0` arm adds to the
    /// focal at RVA `0x1F8D5`), negative owes zoom-out (`acc < 0`, RVA `0x1F91D`).
    pub fn armed(&self) -> Option<ZoomArm> {
        match self.frames_owed {
            n if n > 0 => Some(ZoomArm::In),
            n if n < 0 => Some(ZoomArm::Out),
            _ => None,
        }
    }

    /// One frame's drain toward zero, exactly as retail's per-frame decrement does it (RVA
    /// `0x25E240`: `acc < 0 → acc + 1`, `acc > 0 → acc - 1`).
    pub fn drain_one(&mut self) {
        if self.frames_owed > 0 {
            self.frames_owed -= 1;
        } else if self.frames_owed < 0 {
            self.frames_owed += 1;
        }
    }

    /// Retail's clear (`acc = 0`, RVA `0x25E230`): a zoom key frame, or any arm reaching its band
    /// end, discards whatever the wheel still owed.
    pub fn clear(&mut self) {
        self.frames_owed = 0;
    }

    /// Frames still owed — for tests and for anything that wants to know a scroll is in flight.
    pub fn frames_owed(&self) -> i32 {
        self.frames_owed
    }
}

/// Pushes [`ViewFov`] into the operator camera's projection. Skipped while a running event
/// holds the camera: `cutscene_camera::apply_frame` owns that fov per frame (focal-driven) and
/// restores `settings.fov_deg` on release, so a zoom write would fight it mid-cutscene.
pub fn apply_view_fov_system(
    mut view_fov: ResMut<ViewFov>,
    settings: Res<GraphicsSettings>,
    cutscene: Res<crate::cutscene::CutsceneMode>,
    mut last_base: Local<Option<f32>>,
    mut cam_q: Query<&mut Projection, With<OperatorCamera>>,
) {
    // First frame and any menu change of the base FOV re-seat the zoom on it (degrees to focal).
    if *last_base != Some(settings.fov_deg) {
        view_fov.focal_length = ViewFov::focal_for_deg(settings.fov_deg);
        *last_base = Some(settings.fov_deg);
    }
    if cutscene.camera_locked {
        return;
    }
    let Ok(mut proj) = cam_q.single_mut() else {
        return;
    };
    if let Projection::Perspective(p) = &mut *proj {
        p.fov = ViewFov::deg_for_focal(view_fov.focal_length).to_radians();
    }
}

#[derive(Resource, Debug, Clone, Copy)]
pub struct CameraTransition {
    pub active: bool,

    pub t: f32,

    pub duration: f32,

    pub from_dist: f32,

    pub to_dist: f32,

    pub target_mode: CameraMode,

    pub saved_chase_dist: f32,
}

impl Default for CameraTransition {
    fn default() -> Self {
        Self {
            active: false,
            t: 0.0,
            duration: 0.35,
            from_dist: 0.0,
            to_dist: 0.0,
            target_mode: CameraMode::Chase,
            // What a first-person toggle restores to before the player has zoomed;
            // same value as the resting chase distance.
            saved_chase_dist: ChaseCamera::DIST_MAX,
        }
    }
}

impl CameraTransition {
    pub fn begin(&mut self, current_mode: CameraMode, current_dist: f32) {
        match current_mode {
            CameraMode::Chase => {
                self.saved_chase_dist = current_dist;
                self.from_dist = current_dist;
                self.to_dist = 0.0;
                self.target_mode = CameraMode::FirstPerson;
            }
            CameraMode::FirstPerson => {
                self.from_dist = 0.0;
                self.to_dist = self.saved_chase_dist;
                self.target_mode = CameraMode::Chase;
            }
        }
        self.active = true;
        self.t = 0.0;
    }
}

pub fn camera_transition_system(
    time: Res<Time>,
    mut transition: ResMut<CameraTransition>,
    mut mode: ResMut<CameraMode>,
    mut chase: ResMut<ChaseCamera>,
) {
    if !transition.active {
        return;
    }

    if matches!(transition.target_mode, CameraMode::Chase)
        && matches!(*mode, CameraMode::FirstPerson)
    {
        *mode = CameraMode::Chase;
    }

    transition.t = (transition.t + time.delta_secs() / transition.duration).min(1.0);

    let s = transition.t * transition.t * (3.0 - 2.0 * transition.t);
    chase.distance = transition.from_dist + (transition.to_dist - transition.from_dist) * s;

    if matches!(transition.target_mode, CameraMode::FirstPerson)
        && chase.distance < 1.0
        && matches!(*mode, CameraMode::Chase)
    {
        *mode = CameraMode::FirstPerson;
        chase.pitch = 0.0;
    }

    if transition.t >= 1.0 {
        chase.distance = transition.to_dist;
        *mode = transition.target_mode;
        if matches!(transition.target_mode, CameraMode::Chase) {
            chase.pitch = chase
                .pitch
                .clamp(ChaseCamera::PITCH_MIN, ChaseCamera::PITCH_MAX);
        }
        transition.active = false;
    }
}

pub fn spawn_camera(mut commands: Commands, settings: Res<GraphicsSettings>) {
    build_operator_camera(&mut commands, &settings, None);

    commands.insert_resource(ChaseCamera::default());
}

/// DLSS SR replaces both MSAA and TAA (settings.msaa() reports Off for
/// AaMode::Dlss, and dlss_active() is not true at the same time as wants_taa()).
/// The component's #[require] pulls in TemporalJitter, MipBias, DepthPrepass,
/// MotionVectorPrepass and Hdr automatically. The insert is gated on
/// dlss_active(), not the raw mode: with the runtime unsupported (or on a
/// default build, where this block does not compile at all) the camera comes
/// up plain and the menu shows DLSS (N/A).
pub fn build_operator_camera(
    commands: &mut Commands,
    settings: &GraphicsSettings,
    restore_transform: Option<Transform>,
) {
    // Depth texture is allocated per (target, msaa) with the OR of every view's usage on
    // that target (bevy core_3d prepare_core_3d_depth_textures), and re-created when MSAA
    // toggles — so this flag follows the current sample count for free.
    let camera_3d = Camera3d {
        depth_texture_usages: Camera3dDepthTextureUsage::from(
            TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
        ),
        ..Default::default()
    };

    let mut camera = commands.spawn((
        crate::components::InGameEntity,
        OperatorCamera,
        // Required for mesh picking under require_markers=true (kuluu-k929); the
        // render-scale path re-targets this same camera, so one marker covers
        // both native and off-screen scale.
        bevy::picking::mesh_picking::MeshPickingCamera,
        bevy::camera::visibility::RenderLayers::from_layers(&[0, WORLD_GIZMO_LAYER]),
        // The nameplate final pass reads this view's depth buffer to occlude plates
        // against walls. With MSAA on that read is a texture sample of the multi-sample
        // depth buffer (nameplate_final_pass.rs), which requires TEXTURE_BINDING — Bevy
        // only adds it for cameras carrying OcclusionCulling, so set it explicitly here.
        camera_3d,
        Hdr,
        settings.tonemapping(),
        ShadowFilteringMethod::Gaussian,
        settings.msaa(),
        Bloom {
            intensity: settings.bloom_intensity,

            prefilter: bevy::post_process::bloom::BloomPrefilter {
                threshold: 1.0,
                threshold_softness: 0.4,
            },
            ..Bloom::NATURAL
        },
        Projection::Perspective(PerspectiveProjection {
            far: crate::skybox::CAMERA_FAR,
            fov: settings.fov_deg.to_radians(),
            ..default()
        }),
        restore_transform.unwrap_or_else(|| {
            Transform::from_xyz(0.0, 12.0, 18.0).looking_at(Vec3::ZERO, Vec3::Y)
        }),
    ));

    if settings.volumetric_fog {
        camera.insert(VolumetricFog {
            step_count: settings.fog_step_count,

            ambient_intensity: 0.03,
            ambient_color: Color::srgb(0.85, 0.88, 1.0),
            jitter: 0.0,
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    if matches!(settings.anti_aliasing, AaMode::Taa) {
        camera.insert(TemporalAntiAliasing::default());
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "dlss"))]
    if settings.dlss_active() {
        camera.insert(bevy::anti_alias::dlss::Dlss::<
            bevy::anti_alias::dlss::DlssSuperResolutionFeature,
        > {
            perf_quality_mode: crate::graphics::dlss::to_bevy_quality(settings.dlss_quality),
            ..Default::default()
        });
    }
}

/// Native camera collision owns the transform; this retains its shared
/// scheduling anchor.
#[cfg(not(target_arch = "wasm32"))]
pub fn chase_camera_system() {}

#[cfg(any(target_arch = "wasm32", test))]
pub fn snapshot_chase_camera_system(
    mode: Res<CameraMode>,
    mut chase: ResMut<ChaseCamera>,
    state: Res<crate::snapshot::SceneState>,
    q_self: Query<(&Transform, Option<&BakedActor>), (With<IsSelf>, Without<OperatorCamera>)>,
    mut q_cam: Query<&mut Transform, (With<OperatorCamera>, Without<IsSelf>)>,
) {
    if !matches!(*mode, CameraMode::Chase) {
        return;
    }

    let Ok((self_t, baked)) = q_self.single() else {
        return;
    };
    let Ok(mut cam_t) = q_cam.single_mut() else {
        return;
    };

    if !chase.synced_initial {
        chase.yaw = yaw_for_heading(state.snapshot.self_pos.heading);
        chase.synced_initial = true;
    }

    let cos_p = chase.pitch.cos();
    let sin_p = chase.pitch.sin();
    let yaw_dir = Vec3::new(chase.yaw.sin(), 0.0, chase.yaw.cos());

    let anchor_y = third_person_anchor_y(baked);
    let anchor = self_t.translation + Vec3::Y * anchor_y;
    let radius = chase.orbit_radius();
    let desired = anchor + yaw_dir * (radius * cos_p) + Vec3::Y * (radius * sin_p);

    if chase.snap_to_anchor {
        cam_t.translation = desired;
        chase.snap_to_anchor = false;
    } else {
        cam_t.translation = cam_t.translation.lerp(desired, chase.smoothing);
    }
    cam_t.look_at(anchor, Vec3::Y);
}

#[cfg(target_arch = "wasm32")]
pub use snapshot_chase_camera_system as chase_camera_system;

pub fn firstperson_camera_system(
    mode: Res<CameraMode>,
    chase: Res<ChaseCamera>,
    step: Res<CameraStepSmoothing>,
    q_self: Query<(&Transform, Option<&BakedActor>), (With<IsSelf>, Without<OperatorCamera>)>,
    mut q_cam: Query<&mut Transform, (With<OperatorCamera>, Without<IsSelf>)>,
) {
    if !matches!(*mode, CameraMode::FirstPerson) {
        return;
    }
    let Ok((self_t, baked)) = q_self.single() else {
        return;
    };
    let Ok(mut cam_t) = q_cam.single_mut() else {
        return;
    };

    let eye = self_t.translation + Vec3::Y * (first_person_eye_y(baked) - step.offset);
    let cos_p = chase.pitch.cos();
    let look_dir = Vec3::new(
        -chase.yaw.sin() * cos_p,
        chase.pitch.sin(),
        -chase.yaw.cos() * cos_p,
    );
    cam_t.translation = eye;
    cam_t.look_at(eye + look_dir, Vec3::Y);
}

pub fn self_visibility_for_camera_mode_system(
    mode: Res<CameraMode>,
    mut q_self: Query<&mut Visibility, With<IsSelf>>,
) {
    let want = match *mode {
        CameraMode::FirstPerson => Visibility::Hidden,
        CameraMode::Chase => Visibility::Inherited,
    };
    for mut vis in q_self.iter_mut() {
        if *vis != want {
            *vis = want;
        }
    }
}

pub fn toggle_camera_mode(mode: &mut CameraMode, chase: &mut ChaseCamera) {
    *mode = match *mode {
        CameraMode::Chase => {
            chase.pitch = 0.0;
            CameraMode::FirstPerson
        }
        CameraMode::FirstPerson => {
            chase.pitch = chase
                .pitch
                .clamp(ChaseCamera::PITCH_MIN, ChaseCamera::PITCH_MAX);
            CameraMode::Chase
        }
    };
}

#[inline]
pub fn yaw_for_heading(heading: u8) -> f32 {
    let tau = std::f32::consts::TAU;
    -(heading as f32) * tau / 256.0 - std::f32::consts::FRAC_PI_2
}

#[inline]
pub fn heading_for_yaw(yaw: f32) -> u8 {
    let tau = std::f32::consts::TAU;
    let normalized = (-yaw - std::f32::consts::FRAC_PI_2).rem_euclid(tau);
    (normalized * 256.0 / tau).round() as u32 as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locator_skeleton(bone: usize) -> ffxi_dat::skel::Skeleton {
        use ffxi_dat::{
            datid::DatId,
            skel::{standard_position::ABOVE_HEAD, JointReference, Skeleton},
        };
        Skeleton {
            id: DatId::from_str("test"),
            joints: Vec::new(),
            references: vec![
                JointReference {
                    index: bone,
                    rotation: [0.0; 3],
                    position_offset: [1.0, -3.5, 2.0]
                };
                ABOVE_HEAD + 1
            ],
            bounding_boxes: Vec::new(),
            look_at_limits: Vec::new(),
        }
    }

    #[test]
    fn nameplate_anchor_converts_dat_axes_and_ignores_facing() {
        let locator = NameplateLocator::from_skeleton(&locator_skeleton(0), 2.0);
        let model = Transform {
            translation: Vec3::new(10.0, 20.0, 30.0),
            rotation: Quat::from_rotation_y(1.0),
            scale: Vec3::new(2.0, 3.0, 4.0),
        };
        assert_eq!(
            nameplate_anchor(&model, Some(&locator), false),
            Some(Vec3::new(14.0, 41.0, 14.0))
        );
        let mounted = nameplate_anchor(&model, Some(&locator), true).unwrap();
        assert!((mounted.y - 48.8).abs() < 1e-5);
        let nonroot = NameplateLocator::from_skeleton(&locator_skeleton(1), 2.0);
        assert_eq!(
            nameplate_anchor(&model, Some(&nonroot), true),
            nameplate_anchor(&model, Some(&nonroot), false)
        );
    }

    #[test]
    fn snapshot_viewer_camera_follows_player_movement() {
        let mut app = App::new();
        app.init_resource::<CameraMode>()
            .init_resource::<crate::snapshot::SceneState>()
            .insert_resource(ChaseCamera {
                smoothing: 1.0,
                ..default()
            })
            .add_systems(Update, snapshot_chase_camera_system);
        let player = app.world_mut().spawn((IsSelf, Transform::default())).id();
        let camera = app
            .world_mut()
            .spawn((OperatorCamera, Transform::default()))
            .id();
        app.update();
        let first = app.world().get::<Transform>(camera).unwrap().translation;
        let displacement = Vec3::new(1.0, 2.0, 3.0);
        app.world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation = displacement;
        app.update();
        let second = app.world().get::<Transform>(camera).unwrap().translation;
        assert!((second - first - displacement).length() < 1e-5);
    }

    #[test]
    fn yaw_heading_roundtrip_cardinals() {
        for &h in &[0u8, 64, 128, 192] {
            let y = yaw_for_heading(h);
            let back = heading_for_yaw(y);
            assert_eq!(back, h, "roundtrip for heading {h}");
        }
    }

    #[test]
    fn toggle_camera_mode_mediates_pitch_at_boundaries() {
        let mut mode = CameraMode::Chase;
        let mut chase = ChaseCamera {
            pitch: 0.55,
            ..Default::default()
        };
        toggle_camera_mode(&mut mode, &mut chase);
        assert_eq!(mode, CameraMode::FirstPerson);
        assert_eq!(chase.pitch, 0.0, "FP entry resets pitch to level");

        chase.pitch = -0.7;
        toggle_camera_mode(&mut mode, &mut chase);
        assert_eq!(mode, CameraMode::Chase);
        assert_eq!(
            chase.pitch,
            ChaseCamera::PITCH_MIN,
            "Chase re-entry clamps pitch up to the floor"
        );

        toggle_camera_mode(&mut mode, &mut chase);
        assert_eq!(chase.pitch, 0.0, "FP re-entry still resets pitch");
        chase.pitch = 1.5;
        toggle_camera_mode(&mut mode, &mut chase);
        assert_eq!(chase.pitch, ChaseCamera::PITCH_MAX);
    }

    #[test]
    fn pitch_max_is_the_standoff_expressed_as_an_angle() {
        let derived = (ChaseCamera::MIN_XZ_STANDOFF / ChaseCamera::DIST_MAX).acos();
        assert!(
            (ChaseCamera::PITCH_MAX - derived).abs() < 1e-6,
            "PITCH_MAX {} must stay the angle at which a DIST_MAX orbit still \
             clears MIN_XZ_STANDOFF horizontally ({derived})",
            ChaseCamera::PITCH_MAX
        );
    }

    #[test]
    fn orbit_never_trades_retails_horizontal_standoff_for_height() {
        let mut worst = f32::INFINITY;
        for d in 0..=30 {
            for p in 0..=30 {
                let chase = ChaseCamera {
                    distance: ChaseCamera::DIST_MIN
                        + (ChaseCamera::DIST_MAX - ChaseCamera::DIST_MIN) * d as f32 / 30.0,
                    pitch: ChaseCamera::PITCH_MIN
                        + (ChaseCamera::PITCH_MAX - ChaseCamera::PITCH_MIN) * p as f32 / 30.0,
                    ..Default::default()
                };
                let r = chase.orbit_radius();
                assert!(
                    r <= ChaseCamera::DIST_MAX + 1e-5,
                    "radius {r} escaped DIST_MAX at pitch {}",
                    chase.pitch
                );
                worst = worst.min(r * chase.pitch.cos());
            }
        }
        assert!(
            worst >= ChaseCamera::MIN_XZ_STANDOFF - 1e-5,
            "closest horizontal approach {worst} broke retail's {} standoff — \
             this is what let the camera thread the Lower Jeuno ceiling gap",
            ChaseCamera::MIN_XZ_STANDOFF
        );
    }

    #[test]
    fn firstperson_look_dir_matches_player_forward_at_default_yaw() {
        let yaw = 0.0_f32;
        let pitch = 0.0_f32;
        let cos_p = pitch.cos();
        let look = Vec3::new(-yaw.sin() * cos_p, pitch.sin(), -yaw.cos() * cos_p);

        let expected = Vec3::new(0.0, 0.0, -1.0);
        assert!(
            (look - expected).length() < 1e-6,
            "look {look:?} != expected {expected:?}"
        );
    }

    #[test]
    fn operator_camera_renders_world_and_gizmo_layers() {
        use bevy::camera::visibility::RenderLayers;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(GraphicsSettings::default());
        app.add_systems(
            Startup,
            |mut commands: Commands, settings: Res<GraphicsSettings>| {
                build_operator_camera(&mut commands, &settings, None);
            },
        );
        app.update();

        let mut q = app
            .world_mut()
            .query_filtered::<&RenderLayers, With<OperatorCamera>>();
        let layers = q.single(app.world()).expect("operator camera spawned");
        assert!(
            layers.intersects(&RenderLayers::layer(0)),
            "operator camera must still see world layer 0"
        );
        assert!(
            layers.intersects(&RenderLayers::layer(WORLD_GIZMO_LAYER)),
            "operator camera must see the gizmo overlay layer so debug \
             overlays still show in the live 3D view"
        );
    }

    #[test]
    fn view_fov_defaults_to_the_retail_derived_value() {
        assert_eq!(
            ViewFov::default().focal_length,
            crate::graphics_settings::RETAIL_DEFAULT_FOCAL_LENGTH
        );
        // The shipped default degree value is pinned by settings.rs's own guard test
        // (`default_fov_derives_from_retail_focal_length`); this one pins that the resource and
        // that constant agree through the focal conversion.
        assert!(
            (ViewFov::default().deg() - crate::graphics_settings::DEFAULT_FOV_DEG).abs() < 1e-4,
            "the live zoom's default must be the retail-derived FOV"
        );
    }

    #[test]
    fn zoom_step_is_the_one_rate_and_clamps_at_both_band_ends() {
        // Two frame ticks — retail's default cap — move the focal twice its per-tick step.
        let (near, hit) = ViewFov::step(500.0, 2.0, ZoomArm::In);
        assert_eq!((near, hit), (512.0, false));
        let (far, hit) = ViewFov::step(500.0, 2.0, ZoomArm::Out);
        assert_eq!((far, hit), (488.0, false));
        let (top, hit) = ViewFov::step(ViewFov::MAX_FOCAL - 1.0, 2.0, ZoomArm::In);
        assert_eq!((top, hit), (ViewFov::MAX_FOCAL, true));
        let (bottom, hit) = ViewFov::step(ViewFov::MIN_FOCAL + 1.0, 2.0, ZoomArm::Out);
        assert_eq!((bottom, hit), (ViewFov::MIN_FOCAL, true));
    }

    #[test]
    fn one_wheel_notch_is_two_frames_of_the_same_rate_as_a_held_key() {
        // The wheel buys duration, not a different step: `acc += 2` per notch, drained by 1/frame,
        // each frame running the same `tick × 6.0` (`FFXiMain.dll retail-2026-09` RVAs 0x25E210 /
        // 0x25E240 / 0x1F8D5). Three notches therefore spend six frames of that rate.
        let mut wheel = WheelZoom::default();
        wheel.add_notches(3);
        assert_eq!(wheel.frames_owed(), 6);

        let mut focal = 400.0;
        let mut frames_zooming = 0;
        for _ in 0..12 {
            if let Some(arm) = wheel.armed() {
                let (next, hit_clamp) = ViewFov::step(focal, 1.0, arm);
                focal = next;
                frames_zooming += 1;
                if hit_clamp {
                    wheel.clear();
                }
            }
            wheel.drain_one();
        }
        assert_eq!(frames_zooming, 6);
        assert_eq!(focal, 400.0 + 6.0 * ViewFov::FOCAL_STEP_PER_TICK);

        // A zoom-key frame discards whatever the wheel still owed, so the two never stack up.
        wheel.add_notches(2);
        wheel.clear();
        assert_eq!(wheel.armed(), None);
    }

    #[test]
    fn the_wheel_counter_drains_toward_zero_from_either_sign() {
        let mut wheel = WheelZoom::default();
        wheel.add_notches(-1);
        assert_eq!(wheel.armed(), Some(ZoomArm::Out));
        for _ in 0..5 {
            wheel.drain_one();
        }
        assert_eq!(wheel.frames_owed(), 0);
        assert_eq!(wheel.armed(), None);
    }

    #[test]
    fn both_zoom_keys_ease_to_neutral_and_then_settle_on_it() {
        // `FFXiMain.dll retail-2026-09` RVA 0x1F76E: the ease keeps running until its own step falls
        // inside one focal unit, and only then does it settle on neutral.
        assert_eq!(ViewFov::ease_to_neutral(500.0), (462.5, false));
        let mut focal = ViewFov::MIN_FOCAL;
        let mut settled = false;
        for _ in 0..40 {
            (focal, settled) = ViewFov::ease_to_neutral(focal);
            if settled {
                break;
            }
        }
        assert!(settled, "the ease must settle within a few dozen frames");
        assert_eq!(focal, crate::graphics_settings::RETAIL_DEFAULT_FOCAL_LENGTH);
    }

    #[test]
    fn focal_band_maps_to_the_documented_retail_fov_ends() {
        // 242 ≈ 76.9° wide, 350 ≈ 57.5°, 900 ≈ 24.1° tight (M17) — the web-tier formula's
        // published values, pinned so a change here cannot silently drift.
        for &(focal, deg) in &[(242.0, 76.9), (350.0, 57.5), (900.0, 24.1)] {
            let got = ViewFov::deg_for_focal(focal);
            assert!(
                (got - deg).abs() < 0.1,
                "{focal} focal should project to ≈{deg}°, got {got:.2}"
            );
        }
        let round_trip = ViewFov::focal_for_deg(ViewFov::deg_for_focal(437.0));
        assert!((round_trip - 437.0).abs() < 1e-3);
    }

    /// An unrelated `GraphicsSettings` change must not stomp a live view zoom
    /// (the blink seen while walking with the Commands UI open): only a real
    /// `fov_deg` change re-seats, so the menu FOV row still wins. The app runs
    /// the two systems unordered, so the zoom has to survive either order.
    #[test]
    fn unrelated_settings_changes_cannot_stomp_a_view_zoom() {
        use crate::graphics_settings::{apply_projection_system, GraphicsSettings};
        for projection_last in [false, true] {
            let mut app = App::new();
            app.init_resource::<GraphicsSettings>()
                .init_resource::<ViewFov>()
                .init_resource::<crate::cutscene::CutsceneMode>();
            let cam = app
                .world_mut()
                .spawn((
                    OperatorCamera,
                    Projection::from(PerspectiveProjection {
                        fov: crate::graphics_settings::DEFAULT_FOV_DEG.to_radians(),
                        ..default()
                    }),
                ))
                .id();
            if projection_last {
                app.add_systems(
                    Update,
                    (apply_view_fov_system, apply_projection_system).chain(),
                );
            } else {
                app.add_systems(
                    Update,
                    (apply_projection_system, apply_view_fov_system).chain(),
                );
            }
            let fov_now = |app: &App| {
                let Projection::Perspective(p) = app.world().get::<Projection>(cam).unwrap() else {
                    panic!("perspective projection expected")
                };
                p.fov.to_degrees()
            };
            // Frame 1 establishes the re-seat baseline.
            app.update();
            app.world_mut().resource_mut::<ViewFov>().focal_length = ViewFov::focal_for_deg(80.0);
            // A settings write that leaves fov_deg alone: what every menu key
            // does on its way through handle_menu_key's deref-mut handoffs.
            app.world_mut()
                .resource_mut::<GraphicsSettings>()
                .bloom_intensity += 0.01;
            app.update();
            assert!(
                (fov_now(&app) - 80.0).abs() < 1e-3,
                "a held zoom must survive unrelated settings changes (projection last: \
                 {projection_last}), got {}",
                fov_now(&app)
            );
            app.world_mut().resource_mut::<GraphicsSettings>().fov_deg = 45.0;
            app.update();
            assert!(
                (fov_now(&app) - 45.0).abs() < 1e-3,
                "a base fov_deg change must re-seat the zoom (projection last: {projection_last})"
            );
        }
    }

    #[test]
    fn anchor_bias_is_six_tenths_of_the_span_capped_at_retails_limit() {
        let bias = |span: f32| span * ANCHOR_SPAN_SCALE;
        for span in [0.1, FALLBACK_ACTOR_HEIGHT, ANCHOR_MAX_SPAN] {
            assert!(
                (anchor_bias_y(span) - bias(span)).abs() < 1e-6,
                "span {span}"
            );
        }
        for span in [ANCHOR_MAX_SPAN + 0.01, 2.0 * ANCHOR_MAX_SPAN] {
            assert!(
                (anchor_bias_y(span) - bias(ANCHOR_MAX_SPAN)).abs() < 1e-6,
                "span {span} sits at the cap"
            );
        }
        assert!(
            anchor_bias_y(FALLBACK_ACTOR_HEIGHT) < FALLBACK_ACTOR_HEIGHT,
            "a hume-sized pivot sits inside the body, not above the head"
        );
    }

    #[test]
    fn the_pivot_reads_the_skeleton_span_before_the_mesh_extent() {
        const MESH_EXTENT: f32 = 2.3;
        const SKELETON_SPAN: f32 = 1.9;
        let baked = |skeleton_span| BakedActor {
            min_mesh_y: 0.0,
            actor_height: MESH_EXTENT,
            skeleton_span,
        };
        assert_eq!(
            third_person_anchor_y(Some(&baked(Some(SKELETON_SPAN)))),
            anchor_bias_y(SKELETON_SPAN)
        );
        assert_eq!(
            third_person_anchor_y(Some(&baked(None))),
            anchor_bias_y(MESH_EXTENT)
        );
    }
}
