//! Operator chase camera with **decoupled yaw**: the camera sits at its own
//! `yaw` angle around the player Y-axis, independent of player heading. The
//! input layer mutates yaw/pitch directly; player heading is set elsewhere.
//!
//! When forward (W/S) is pressed, the input system snaps player heading to
//! [`heading_for_yaw`] so the player walks in the direction the camera looks
//! — FFXI's "third-person walk-toward-camera-forward" behavior.

use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::{ShadowFilteringMethod, VolumetricFog};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::render::view::Hdr;

#[cfg(not(target_arch = "wasm32"))]
use bevy::anti_alias::taa::TemporalAntiAliasing;

use crate::components::IsSelf;
#[cfg(not(target_arch = "wasm32"))]
use crate::graphics_settings::AaMode;
use crate::graphics_settings::GraphicsSettings;
use crate::scene::BakedActor;
use crate::snapshot::SceneState;

/// Fraction of an entity's full visual height (= `BakedActor.actor_height`)
/// to use as the chase-camera anchor / look-at point. 0.55 lands at
/// mid-chest — what retail FFXI frames in third-person. Race-invariant
/// in fractional terms: Galka chest is at ~55% of Galka height, Taru
/// chest at ~55% of Taru height. Race-dependent in absolute yalms,
/// which is what we want.
const THIRD_PERSON_ANCHOR_FRAC: f32 = 0.55;

/// Fraction of an entity's full visual height for the first-person eye.
/// 0.92 puts the eye-line just below the crown — matches retail FFXI's
/// first-person camera (look out from the head, not the brow).
const FIRST_PERSON_EYE_FRAC: f32 = 0.92;

/// Extra yalms above `actor_height` (≈ top-of-head) for the nameplate
/// floating-label anchor. Small enough that the label sits just clear
/// of the crown; large enough that it doesn't intersect tall hats.
const NAMEPLATE_OFFSET_ABOVE_CROWN: f32 = 0.1;

/// Fallback actor height (yalms) used when an entity lacks a
/// [`BakedActor`] — mobs without a skinned mesh, or the frames before
/// VOS2 dispatch attaches the component. Matched to the baked PC crown
/// (the CPU bake lands the head top at Y≈2.29) rather than the debug
/// capsule height (4.5): the capsule is hidden the moment skin loads,
/// so anchoring to it floated nameplates ~1 yalm high during the
/// pre-bake window. Matching the rendered body keeps the anchor stable
/// before and after the bake attaches the real `BakedActor`.
const FALLBACK_ACTOR_HEIGHT: f32 = 2.3;

/// Third-person camera anchor Y above an entity's feet, derived from
/// its [`BakedActor`] when present. The chase camera pivots both its
/// ray-collision origin AND its world placement around this point — so
/// race-tall actors get a higher anchor (and the camera is positioned
/// higher to match), race-short actors a lower one.
#[inline]
pub fn third_person_anchor_y(baked: Option<&BakedActor>) -> f32 {
    baked
        .map(|b| b.actor_height)
        .unwrap_or(FALLBACK_ACTOR_HEIGHT)
        * THIRD_PERSON_ANCHOR_FRAC
}

/// First-person eye-height Y above feet. See [`FIRST_PERSON_EYE_FRAC`].
#[inline]
pub fn first_person_eye_y(baked: Option<&BakedActor>) -> f32 {
    baked
        .map(|b| b.actor_height)
        .unwrap_or(FALLBACK_ACTOR_HEIGHT)
        * FIRST_PERSON_EYE_FRAC
}

/// Y above feet for a nameplate's world anchor. Sits just above the
/// top of the mesh; passed to `Camera::world_to_viewport` to compute
/// screen position.
#[inline]
pub fn nameplate_anchor_y(baked: Option<&BakedActor>) -> f32 {
    baked
        .map(|b| b.actor_height)
        .unwrap_or(FALLBACK_ACTOR_HEIGHT)
        + NAMEPLATE_OFFSET_ABOVE_CROWN
}

/// Marker on the operator camera entity.
#[derive(Component)]
pub struct OperatorCamera;

/// Render layer that all world-overlay gizmos (camera-collision debug,
/// navmesh overlay, target ring/arrow, aggro lines) are drawn on.
///
/// Gizmos default to render layer 0 — the same layer as the world
/// geometry — which means *every* camera renders them, including the
/// minimap's top-down "bake" camera in [`crate::minimap::topdown`].
/// That baked the blue collision wireframes (and any other overlay live
/// at zone-enter) straight into the static minimap texture.
///
/// The fix is a layer split: the default gizmo group is moved onto this
/// layer (front-ends call [`configure_gizmo_render_layer`] once at
/// startup), the live [`OperatorCamera`] opts back into it via
/// [`build_operator_camera`], and the bake camera is pinned to layer 0
/// only — so gizmos show in the live 3D view but never in the minimap.
///
/// Layers 0 (world / launcher backdrop) and 1 (launcher character
/// preview) are already taken, so this is 2.
pub const WORLD_GIZMO_LAYER: usize = 2;

/// Point the default gizmo config group at [`WORLD_GIZMO_LAYER`] so
/// gizmos render only to cameras that explicitly opt into that layer
/// (the live [`OperatorCamera`]) and never to the minimap bake camera.
///
/// Front-ends run this once at startup — it needs Bevy's `GizmoPlugin`
/// (part of `DefaultPlugins`) to have inserted `GizmoConfigStore`, which
/// a library crate can't assume in unit tests, so the wiring lives at
/// the app-assembly layer rather than in a viewer-core plugin.
pub fn configure_gizmo_render_layer(
    mut store: ResMut<bevy::gizmos::config::GizmoConfigStore>,
) {
    let (config, _) = store.config_mut::<bevy::gizmos::config::DefaultGizmoConfigGroup>();
    config.render_layers = bevy::camera::visibility::RenderLayers::layer(WORLD_GIZMO_LAYER);
}

/// Active camera projection. `Chase` is the FFXI-default third-person
/// orbit, `FirstPerson` snaps the camera to the player's eyes and lets the
/// look direction track [`ChaseCamera::yaw`]/`pitch` 1:1 (so mouse-look in
/// FP turns the avatar). Defaults to `Chase`; F8 toggles it from
/// `view_native/input.rs`.
///
/// Both modes share `ChaseCamera::yaw`/`pitch` storage to avoid cross-mode
/// jumps in the look direction. Pitch range differs (chase is clamped to
/// `[PITCH_MIN, PITCH_MAX]`; FP allows near-vertical), so the toggle
/// handler clamps pitch back into chase range when leaving FP.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CameraMode {
    #[default]
    Chase,
    FirstPerson,
}

/// Tunable chase-camera state. Yaw is the angle (Bevy radians, around +Y)
/// pointing from player toward camera. Default 0 = camera at player+Z. The
/// `synced_initial` flag lets the input layer one-shot align yaw to the
/// player's spawn heading so the camera starts behind, not "north" of a
/// south-facing avatar.
#[derive(Resource)]
pub struct ChaseCamera {
    /// Camera azimuth around the player, radians. yaw=0 → camera at +Z
    /// direction from player. Wraps via the input layer; not normalized
    /// here (`f32::sin`/`cos` are happy with any value).
    pub yaw: f32,
    /// Pitch from horizontal, radians. 0 = level (eye-line); π/2 = directly
    /// overhead. Clamp range: [`PITCH_MIN`]..[`PITCH_MAX`].
    pub pitch: f32,
    /// Total camera-to-player distance in Bevy units.
    pub distance: f32,
    /// Per-frame lerp factor for translation smoothing. 1.0 = snap.
    pub smoothing: f32,
    /// Set to true once we've snapped yaw to the spawn heading. Until then,
    /// the chase system performs the one-shot sync.
    pub synced_initial: bool,
}

impl ChaseCamera {
    /// Floor pitch ≈ -17°. Negative because the chase camera pivots
    /// around the chest anchor (`feet + height_target`), not the feet
    /// (see `chase_camera_system`). pitch=0 puts the camera level with
    /// chest looking horizontally at chest; pitch>0 lifts the camera
    /// above chest looking down; pitch<0 drops the camera below chest
    /// looking up at the avatar — the "from-below" / "below-horizon"
    /// shot that retail FFXI supports. Going below this floor pushes
    /// the camera underground at max zoom (cam.y = chest_y +
    /// distance·sin(pitch); at distance=30, pitch=-0.30 → cam.y≈0.25
    /// before the BVH ground-collision clamp catches it). The
    /// collision system *will* clamp ground hits, so this floor is
    /// primarily an aesthetic guard against the camera burrowing too
    /// far when the operator drags the mouse hard.
    pub const PITCH_MIN: f32 = -0.30;
    /// Ceiling pitch ≈ 80° — leaves a small angle so the camera doesn't go
    /// fully top-down (which would lose the chase aesthetic).
    pub const PITCH_MAX: f32 = 1.40;
    /// First-person look-down ceiling. Clamped just shy of -π/2 so the
    /// up-vector stays unambiguous (looking straight down would singularly
    /// flip yaw).
    pub const FP_PITCH_MIN: f32 = -std::f32::consts::FRAC_PI_2 + 0.05;
    /// First-person look-up ceiling. Symmetric with `FP_PITCH_MIN`.
    pub const FP_PITCH_MAX: f32 = std::f32::consts::FRAC_PI_2 - 0.05;
    /// Closest the chase camera can pull in. Below ~3.0 the player capsule
    /// clips through the near plane; for closer-than-3 use FirstPerson.
    pub const DIST_MIN: f32 = 2.0;
    /// Furthest the chase camera can pull out. Beyond 30 the avatar is too
    /// small to read on the operator's screen and HUD becomes the main signal.
    pub const DIST_MAX: f32 = 20.0;
    /// Keyboard zoom rate, **yalms per second of held key**. Used by
    /// the `.` / `,` bindings (`Action::CameraZoomIn`/`Out`). Time-based
    /// rather than per-press so holding the key produces smooth,
    /// framerate-independent zoom motion. 10 yalm/s traverses the full
    /// 3..30 range in ~2.7 seconds — fast enough to feel responsive,
    /// slow enough that an operator can stop on a chosen distance.
    pub const KEYBOARD_ZOOM_RATE: f32 = 10.0;
}

impl Default for ChaseCamera {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            // Retail's chase camera sits near shoulder-height of the
            // avatar with a faint downward tilt — eye-level, not
            // overhead. 0.55 rad (≈31°) was a noticeably more overhead
            // angle than retail; 0.15 rad (≈8.6°) puts the camera just
            // above horizontal so the player's head sits centered, with
            // the world framed past their shoulders. Stays above
            // `PITCH_MIN` so the initial clamp is a no-op.
            pitch: 0.15,
            distance: 18.0,
            smoothing: 0.18,
            synced_initial: false,
        }
    }
}

/// 1p↔3p zoom transition state. Replaces the instant
/// [`toggle_camera_mode`] behavior with a ~0.35s zoom interpolation —
/// retail FFXI dollies the camera between chase distance and the
/// eye-anchor rather than cutting. `target_mode` is the mode we land
/// in when `t` reaches 1.0; the actual `CameraMode` resource swaps
/// mid-transition based on the chase distance crossing
/// [`ChaseCamera::DIST_MIN`].
#[derive(Resource, Debug, Clone, Copy)]
pub struct CameraTransition {
    /// True while a transition is in progress.
    pub active: bool,
    /// Linear progress 0..=1.
    pub t: f32,
    /// Total transition time in seconds.
    pub duration: f32,
    /// Chase distance at transition start.
    pub from_dist: f32,
    /// Chase distance at transition end.
    pub to_dist: f32,
    /// Mode the system should end up in once `t` reaches 1.
    pub target_mode: CameraMode,
    /// Cached chase distance prior to entering FirstPerson, so the
    /// return trip restores the same orbit radius.
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
            saved_chase_dist: 18.0,
        }
    }
}

impl CameraTransition {
    /// Begin a transition. Caller passes the current mode + chase
    /// distance; the system flips `target_mode` and computes the
    /// from/to distances.
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

/// Tick system: drives the [`CameraTransition`] interpolation. Writes
/// `chase.distance` each frame; swaps `CameraMode` once the camera
/// crosses `DIST_MIN` (going in) or as soon as the transition starts
/// (coming out, so the chase camera renders the entire dolly).
pub fn camera_transition_system(
    time: Res<Time>,
    mut transition: ResMut<CameraTransition>,
    mut mode: ResMut<CameraMode>,
    mut chase: ResMut<ChaseCamera>,
) {
    if !transition.active {
        return;
    }
    // On the first tick of a "to Chase" transition, swap mode so the
    // chase system starts rendering immediately from the close-in
    // distance. For "to FP", stay in Chase until the camera is close
    // enough that swapping is visually indistinguishable.
    if matches!(transition.target_mode, CameraMode::Chase)
        && matches!(*mode, CameraMode::FirstPerson)
    {
        *mode = CameraMode::Chase;
    }

    transition.t = (transition.t + time.delta_secs() / transition.duration).min(1.0);
    // Smoothstep for an ease-in/ease-out feel.
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
    // Initial chase state; the respawn path (build_operator_camera
    // called from apply_anti_aliasing_system) deliberately leaves
    // ChaseCamera alone so the user's current distance/yaw/pitch
    // survives an MSAA flip.
    commands.insert_resource(ChaseCamera::default());
}

/// Spawn the OperatorCamera entity from current settings. Extracted so
/// `apply_anti_aliasing_system` can despawn + respawn the camera on
/// MSAA changes — patching `Msaa` in place hits a Bevy pipeline-cache
/// race where the view target's sample count flips one frame before
/// pipelines are re-specialized, panicking with "incompatible sample
/// count" inside `main_opaque_pass_3d`. Recreating the camera forces
/// Bevy to build fresh pipelines for the new sample count.
///
/// `restore_transform` reuses the camera's pose across the respawn so
/// the user doesn't get yanked back to the default `(0, 12, 18)`.
pub fn build_operator_camera(
    commands: &mut Commands,
    settings: &GraphicsSettings,
    restore_transform: Option<Transform>,
) {
    let mut camera = commands.spawn((
        crate::components::InGameEntity,
        OperatorCamera,
        // See world layer 0 *and* the gizmo overlay layer. World
        // geometry/lights default to layer 0; gizmos are moved to
        // WORLD_GIZMO_LAYER (see `configure_gizmo_render_layer`) so the
        // minimap bake camera — which stays on layer 0 only — never
        // captures them. The live view opts back into both here.
        bevy::camera::visibility::RenderLayers::from_layers(&[0, WORLD_GIZMO_LAYER]),
        Camera3d::default(),
        // HDR is a marker component in Bevy 0.17 (was `camera.hdr`
        // in older versions). Required for bloom and for TonyMcMapface
        // tonemapping to operate on its native HDR input.
        Hdr,
        // TonyMcMapface preserves saturated highlights better than the
        // default Reinhard — emissive target/aggro materials read as
        // glowing rather than blown-out white.
        Tonemapping::TonyMcMapface,
        // Soft PCF shadow filter. Without it the directional-light
        // shadows are jagged single-sample hard edges.
        ShadowFilteringMethod::Gaussian,
        settings.msaa(),
        Bloom {
            intensity: settings.bloom_intensity,
            // Raise the prefilter threshold so only HDR highlights
            // (sun glints, emissive targets, lit windows) bloom — not
            // every textured pixel above zero. With the default
            // threshold of 0.0 (`Bloom::NATURAL`'s default), diffuse
            // zone and character textures wash whitish at close range
            // as they fill more of the screen and bleed into adjacent
            // dark pixels through the bloom passes. 1.0 is "only above
            // LDR white"; softness gives a smooth rolloff so the
            // transition isn't a hard knee.
            prefilter: bevy::post_process::bloom::BloomPrefilter {
                threshold: 1.0,
                threshold_softness: 0.4,
            },
            ..Bloom::NATURAL
        },
        // Distance fog disabled for now. The `ZoneAtmosphere` seam
        // can still attach one per-zone later via
        // `apply_zone_atmosphere_system`; the helper
        // `crate::atmosphere::ffxi_distance_fog` remains as a
        // ready-to-use preset.
        //
        // Extend the far-clip past the celestial-disc sky radius
        // (`crate::sun_moon::SKY_RADIUS` = 4000m). Bevy's default
        // perspective far-clip is 1000m, which culled the sun/moon
        // discs entirely. The settings default (High = 6000m) gives
        // headroom and is still well within float-depth precision.
        Projection::Perspective(PerspectiveProjection {
            far: settings.view_distance,
            fov: settings.fov_deg.to_radians(),
            ..default()
        }),
        restore_transform.unwrap_or_else(|| {
            Transform::from_xyz(0.0, 12.0, 18.0).looking_at(Vec3::ZERO, Vec3::Y)
        }),
    ));

    // Raymarched volumetric fog is opt-in per the user's setting; pairs
    // with the `VolumetricLight` marker on the directional light
    // (`scene.rs::setup_world`) and a world-spanning `FogVolume`
    // entity. `step_count` is perf vs banding — 32 is fast and slightly
    // banded; 96 is the cinematic setting. 64 is the Bevy default.
    if settings.volumetric_fog {
        camera.insert(VolumetricFog {
            step_count: settings.fog_step_count,
            // Low ambient is critical for visible god rays — at 0.1
            // the unlit shafts get filled in by ambient and the
            // lit-vs-unlit contrast collapses to a uniform haze.
            // 0.03 keeps a hint of fill so deep shadow doesn't read
            // pure black through the fog.
            ambient_intensity: 0.03,
            ambient_color: Color::srgb(0.85, 0.88, 1.0),
            jitter: 0.0,
        });
    }

    // TAA on Ultra and any user-cycled Taa setting. WASM build doesn't
    // ship TAA (motion-vector prepass is heavy on WebGPU; `AaMode::Taa`
    // is clamped out of `AA_SLOTS` on wasm32), so the cfg-gate keeps
    // both the import + insert tidy.
    #[cfg(not(target_arch = "wasm32"))]
    if matches!(settings.anti_aliasing, AaMode::Taa) {
        camera.insert(TemporalAntiAliasing::default());
    }
}

/// Position camera using spherical coords (yaw, pitch, distance) anchored
/// on the [`IsSelf`] avatar.
///
/// Geometry: camera_offset = (sin(yaw)·cos(pitch), sin(pitch), cos(yaw)·cos(pitch)) · distance.
/// At yaw=0, pitch=0, the camera sits at player + (0, 0, distance) — straight
/// behind a player that faces -Z. Under LSB heading convention this is
/// `heading = heading_for_yaw(0) = 192` (FFXI +y), *not* heading 0.
///
/// Early-returns when [`CameraMode`] is `FirstPerson` — that mode is owned
/// by [`firstperson_camera_system`].
pub fn chase_camera_system(
    mode: Res<CameraMode>,
    mut chase: ResMut<ChaseCamera>,
    state: Res<SceneState>,
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

    // One-shot: align yaw to player's spawn heading so camera starts behind.
    if !chase.synced_initial {
        chase.yaw = yaw_for_heading(state.snapshot.self_pos.heading);
        chase.synced_initial = true;
    }

    let cos_p = chase.pitch.cos();
    let sin_p = chase.pitch.sin();
    let yaw_dir = Vec3::new(chase.yaw.sin(), 0.0, chase.yaw.cos());
    // Per-actor torso anchor (fraction of `BakedActor.actor_height`) —
    // Galka tall, Taru short, mob whatever the mesh measured. Both the
    // ray and the camera placement pivot here. Fallback constant kicks
    // in for capsule-only entities and the first frame before VOS2
    // dispatch lands.
    let anchor_y = third_person_anchor_y(baked);
    let anchor = self_t.translation + Vec3::Y * anchor_y;
    let desired = anchor + yaw_dir * (chase.distance * cos_p) + Vec3::Y * (chase.distance * sin_p);

    cam_t.translation = cam_t.translation.lerp(desired, chase.smoothing);
    cam_t.look_at(anchor, Vec3::Y);
}

/// First-person camera. Snaps the camera origin to the player's eye and
/// orients the look direction by `(yaw, pitch)` directly — opposite sign of
/// the chase parameterization, since chase yaw points *from* player *to*
/// camera (behind), while FP looks *forward* from the player.
///
/// Geometry: look_dir = (-sin(yaw)·cos(pitch), sin(pitch), -cos(yaw)·cos(pitch)).
/// Symmetry check: at yaw=0, pitch=0, `look_dir = (0, 0, -1)` — Bevy -z,
/// which under LSB heading is heading 192 (FFXI +y).
///
/// Early-returns when [`CameraMode`] is `Chase` — that mode is owned by
/// [`chase_camera_system`].
pub fn firstperson_camera_system(
    mode: Res<CameraMode>,
    chase: Res<ChaseCamera>,
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

    let eye = self_t.translation + Vec3::Y * first_person_eye_y(baked);
    let cos_p = chase.pitch.cos();
    let look_dir = Vec3::new(
        -chase.yaw.sin() * cos_p,
        chase.pitch.sin(),
        -chase.yaw.cos() * cos_p,
    );
    cam_t.translation = eye;
    cam_t.look_at(eye + look_dir, Vec3::Y);
}

/// Hide the player's own avatar in first-person, restore it in chase. Retail
/// FFXI does the same — without it the camera sits just behind the eyes and
/// renders the inside of the skull/equipment.
///
/// Writes `Visibility::Hidden` on the `IsSelf` entity for FP and
/// `Visibility::Inherited` for any other mode. Bevy propagates inherited
/// visibility, so every descendant (capsule, MMB submeshes, baked/skinned
/// actor meshes, equipment children) follows automatically — no per-child
/// query. Skips the write when the value is already correct so the
/// `Changed<Visibility>` filter on the propagation pass doesn't churn each
/// frame.
///
/// The self nameplate billboard is spawned standalone (no `ChildOf`), so it
/// stays visible regardless. That matches retail: there's no nameplate above
/// your own head in either mode, but in our viewer we render one anyway and
/// hiding it would be a separate change.
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

/// Toggle the camera between Chase and FirstPerson, applying mode-specific
/// invariants:
/// - Entering FP: **resets pitch to 0** (level look). Chase and FP share
///   `chase.pitch` storage but interpret it differently — chase reads it as
///   orbital elevation (default 0.55 rad ≈ 31° above horizontal so the
///   camera looks down at the avatar), FP reads it as forward look angle.
///   Carrying the orbital elevation directly into FP slammed the view 31°
///   above horizontal on entry; resetting to 0 starts the operator looking
///   straight ahead.
/// - Returning to Chase: clamps pitch back into `[PITCH_MIN, PITCH_MAX]` so
///   the chase camera doesn't end up inside the ground or fully overhead.
///
/// Lives on [`ChaseCamera`] (not `CameraMode`) because it touches both —
/// callers in input handlers grab `ResMut<ChaseCamera>` + `ResMut<CameraMode>`
/// and call this.
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

/// FFXI heading u8 → camera yaw radians (camera-behind-player).
///
/// LSB convention (matches `reactor::heading_toward` and
/// `view_native::input::heading_to_forward`): a player at heading `h` faces
/// Bevy forward = (cos(α), 0, sin(α)) where α = h·τ/256, so h=0 → +Bevy.x
/// (FFXI +x / "east"), h=64 → +Bevy.z (FFXI -y), h=128 → -Bevy.x, h=192
/// → -Bevy.z. Camera sits opposite, so player→camera = (-cos(α), 0, -sin(α));
/// with our parameterization `(sin(yaw), 0, cos(yaw))`, that gives
/// `yaw = -α - π/2`.
#[inline]
pub fn yaw_for_heading(heading: u8) -> f32 {
    let tau = std::f32::consts::TAU;
    -(heading as f32) * tau / 256.0 - std::f32::consts::FRAC_PI_2
}

/// Camera yaw radians → FFXI heading u8 (player facing away from camera).
///
/// Inverse of [`yaw_for_heading`]. Used by the input layer to snap player
/// heading to "look in the camera's forward direction" when W/S is pressed.
#[inline]
pub fn heading_for_yaw(yaw: f32) -> u8 {
    let tau = std::f32::consts::TAU;
    let normalized = (-yaw - std::f32::consts::FRAC_PI_2).rem_euclid(tau);
    (normalized * 256.0 / tau).round() as u32 as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaw_heading_roundtrip_cardinals() {
        // North, East, South, West (FFXI heading 0/64/128/192).
        for &h in &[0u8, 64, 128, 192] {
            let y = yaw_for_heading(h);
            let back = heading_for_yaw(y);
            assert_eq!(back, h, "roundtrip for heading {h}");
        }
    }

    /// `toggle_camera_mode` flips the mode and mediates the shared `pitch`
    /// storage: FP entry resets it to 0 (level look) so the orbital-pitch
    /// default doesn't slam the FP view upward; Chase re-entry clamps it
    /// back into `[PITCH_MIN, PITCH_MAX]` so the chase camera doesn't end
    /// up inside the ground or fully overhead.
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

        // Mouse-look down past the chase floor while in FP.
        chase.pitch = -0.7;
        toggle_camera_mode(&mut mode, &mut chase);
        assert_eq!(mode, CameraMode::Chase);
        assert_eq!(
            chase.pitch,
            ChaseCamera::PITCH_MIN,
            "Chase re-entry clamps pitch up to the floor"
        );

        // Looking far up in FP also clamps on chase re-entry.
        toggle_camera_mode(&mut mode, &mut chase); // -> FP (pitch reset to 0)
        assert_eq!(chase.pitch, 0.0, "FP re-entry still resets pitch");
        chase.pitch = 1.5;
        toggle_camera_mode(&mut mode, &mut chase); // -> Chase
        assert_eq!(chase.pitch, ChaseCamera::PITCH_MAX);
    }

    /// FP look direction is the negation of the chase camera-from-player
    /// vector: at chase.yaw = 0, pitch = 0, FP forward must be Bevy -Z (the
    /// direction a player at FFXI heading 0 / north faces).
    #[test]
    fn firstperson_look_dir_matches_player_forward_at_default_yaw() {
        let yaw = 0.0_f32;
        let pitch = 0.0_f32;
        let cos_p = pitch.cos();
        let look = Vec3::new(-yaw.sin() * cos_p, pitch.sin(), -yaw.cos() * cos_p);
        // At yaw=0 the FP look formula evaluates to Bevy -Z by construction.
        // (Under LSB heading convention this corresponds to heading 192,
        // not heading 0 — see `yaw_for_heading`.)
        let expected = Vec3::new(0.0, 0.0, -1.0);
        assert!(
            (look - expected).length() < 1e-6,
            "look {look:?} != expected {expected:?}"
        );
    }

    /// The operator camera must render BOTH the world layer (0) and the
    /// gizmo overlay layer ([`WORLD_GIZMO_LAYER`]): dropping layer 0 hides
    /// the world, dropping the gizmo layer hides the camera-collision /
    /// navmesh / target overlays from the live view. The minimap bake
    /// camera deliberately omits the gizmo layer — this test pins the
    /// live-view side of that split.
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
}
