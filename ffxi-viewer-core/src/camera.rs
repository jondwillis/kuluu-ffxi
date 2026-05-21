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
use crate::snapshot::SceneState;

/// Marker on the operator camera entity.
#[derive(Component)]
pub struct OperatorCamera;

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
    /// Raise the look-at point above char origin so the camera doesn't aim
    /// at the ground (the capsule's center is ~1.0 above the floor).
    pub height_target: f32,
    /// Per-frame lerp factor for translation smoothing. 1.0 = snap.
    pub smoothing: f32,
    /// Set to true once we've snapped yaw to the spawn heading. Until then,
    /// the chase system performs the one-shot sync.
    pub synced_initial: bool,
}

impl ChaseCamera {
    /// Floor pitch ≈ 6° — keeps camera off the ground plane.
    pub const PITCH_MIN: f32 = 0.10;
    /// Ceiling pitch ≈ 80° — leaves a small angle so the camera doesn't go
    /// fully top-down (which would lose the chase aesthetic).
    pub const PITCH_MAX: f32 = 1.40;
    /// First-person look-down ceiling. Clamped just shy of -π/2 so the
    /// up-vector stays unambiguous (looking straight down would singularly
    /// flip yaw).
    pub const FP_PITCH_MIN: f32 = -std::f32::consts::FRAC_PI_2 + 0.05;
    /// First-person look-up ceiling. Symmetric with `FP_PITCH_MIN`.
    pub const FP_PITCH_MAX: f32 = std::f32::consts::FRAC_PI_2 - 0.05;
    /// Bevy units from feet to eye for first-person. Capsule mesh has
    /// `height = 1.9` per `scene::EntityMesh`; eye sits below the top so
    /// the cap doesn't intersect the near-clip.
    /// First-person eye height **above the entity's feet** (i.e.
    /// above `transform.y`, which is now feet-on-ground for every
    /// entity after the feet-at-origin refactor in `setup_world` /
    /// `dat_vos2`). Tuned for a typical adult silhouette (~4.5-yalm
    /// total height); Galka/Taru drift is negligible at first-person
    /// distance.
    pub const FP_EYE_HEIGHT: f32 = 3.9;
    /// Closest the chase camera can pull in. Below ~3.0 the player capsule
    /// clips through the near plane; for closer-than-3 use FirstPerson.
    pub const DIST_MIN: f32 = 3.0;
    /// Furthest the chase camera can pull out. Beyond 30 the avatar is too
    /// small to read on the operator's screen and HUD becomes the main signal.
    pub const DIST_MAX: f32 = 30.0;
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
            pitch: 0.55,
            distance: 18.0,
            // Look-at target ≈ chest height above feet. The feet-at-
            // origin refactor moved `transform.y` from body-center to
            // feet, so this constant absorbs the old capsule half-height
            // (≈ 2.25 yalms for PC) plus a small chest lift on top.
            height_target: 3.25,
            smoothing: 0.18,
            synced_initial: false,
        }
    }
}

pub fn spawn_camera(mut commands: Commands, settings: Res<GraphicsSettings>) {
    // Read AA, bloom, fog, view distance, and FOV from the user's
    // persisted settings (defaults to High preset) so the first frame
    // matches the loaded config — the reactor systems in
    // `graphics_settings` re-apply on every change, but spawning at the
    // right initial values avoids a one-frame visual pop.
    let mut camera = commands.spawn((
        crate::components::InGameEntity,
        OperatorCamera,
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
        Transform::from_xyz(0.0, 12.0, 18.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Raymarched volumetric fog is opt-in per the user's setting; pairs
    // with the `VolumetricLight` marker on the directional light
    // (`scene.rs::setup_world`) and a world-spanning `FogVolume`
    // entity. `step_count` is perf vs banding — 32 is fast and slightly
    // banded; 96 is the cinematic setting. 64 is the Bevy default.
    if settings.volumetric_fog {
        camera.insert(VolumetricFog {
            step_count: settings.fog_step_count,
            ambient_intensity: 0.1,
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

    commands.insert_resource(ChaseCamera::default());
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
    q_self: Query<&Transform, (With<IsSelf>, Without<OperatorCamera>)>,
    mut q_cam: Query<&mut Transform, (With<OperatorCamera>, Without<IsSelf>)>,
) {
    if !matches!(*mode, CameraMode::Chase) {
        return;
    }

    let Ok(self_t) = q_self.single() else {
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
    let desired = self_t.translation
        + yaw_dir * (chase.distance * cos_p)
        + Vec3::Y * (chase.distance * sin_p);

    cam_t.translation = cam_t.translation.lerp(desired, chase.smoothing);
    cam_t.look_at(self_t.translation + Vec3::Y * chase.height_target, Vec3::Y);
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
    q_self: Query<&Transform, (With<IsSelf>, Without<OperatorCamera>)>,
    mut q_cam: Query<&mut Transform, (With<OperatorCamera>, Without<IsSelf>)>,
) {
    if !matches!(*mode, CameraMode::FirstPerson) {
        return;
    }
    let Ok(self_t) = q_self.single() else {
        return;
    };
    let Ok(mut cam_t) = q_cam.single_mut() else {
        return;
    };

    let eye = self_t.translation + Vec3::Y * ChaseCamera::FP_EYE_HEIGHT;
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
/// - Entering FP: leaves yaw/pitch alone (the FP system can use the wider
///   range immediately).
/// - Returning to Chase: clamps pitch back into `[PITCH_MIN, PITCH_MAX]` so
///   the chase camera doesn't end up inside the ground or fully overhead.
///
/// Lives on [`ChaseCamera`] (not `CameraMode`) because it touches both —
/// callers in input handlers grab `ResMut<ChaseCamera>` + `ResMut<CameraMode>`
/// and call this.
pub fn toggle_camera_mode(mode: &mut CameraMode, chase: &mut ChaseCamera) {
    *mode = match *mode {
        CameraMode::Chase => CameraMode::FirstPerson,
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

    /// `toggle_camera_mode` flips the mode; pitch is preserved when going
    /// into FP (FP allows the wider range) and clamped back into chase
    /// range when leaving FP.
    #[test]
    fn toggle_camera_mode_clamps_pitch_only_on_chase_entry() {
        let mut mode = CameraMode::Chase;
        let mut chase = ChaseCamera {
            pitch: 0.55,
            ..Default::default()
        };
        toggle_camera_mode(&mut mode, &mut chase);
        assert_eq!(mode, CameraMode::FirstPerson);
        assert_eq!(chase.pitch, 0.55, "FP entry preserves pitch");

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
        toggle_camera_mode(&mut mode, &mut chase); // -> FP
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
}
