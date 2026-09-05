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
use crate::scene::BakedActor;

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

const THIRD_PERSON_ANCHOR_FRAC: f32 = 0.55;

const FIRST_PERSON_EYE_FRAC: f32 = 0.92;

const NAMEPLATE_OFFSET_ABOVE_CROWN: f32 = 0.1;

const FALLBACK_ACTOR_HEIGHT: f32 = 2.3;

#[inline]
pub fn third_person_anchor_y(baked: Option<&BakedActor>) -> f32 {
    baked
        .map(|b| b.actor_height)
        .unwrap_or(FALLBACK_ACTOR_HEIGHT)
        * THIRD_PERSON_ANCHOR_FRAC
}

#[inline]
pub fn first_person_eye_y(baked: Option<&BakedActor>) -> f32 {
    baked
        .map(|b| b.actor_height)
        .unwrap_or(FALLBACK_ACTOR_HEIGHT)
        * FIRST_PERSON_EYE_FRAC
}

/// How much higher a mounted actor's overhead furniture rides. Retail anchors a
/// name on the AboveHead locator, which PC skeletons hang off the root joint —
/// so it does not follow a body the seat pose has lifted, and retail makes up
/// the difference with this while the actor is on a chocobo (research/XIClient
/// .../World/Actor/SkeletalMeshActor.cpp, `SkeletalMeshActor::GetElem` and
/// `VirtActor148`). The anchor below is root-relative in exactly the same way,
/// off a baked mesh height rather than that locator, so retail's rise carries
/// over even though the baseline sits a little lower.
const MOUNTED_ANCHOR_RISE: f32 = 1.3;

#[inline]
pub fn nameplate_anchor_y(baked: Option<&BakedActor>, mounted: bool) -> f32 {
    baked
        .map(|b| b.actor_height)
        .unwrap_or(FALLBACK_ACTOR_HEIGHT)
        + NAMEPLATE_OFFSET_ABOVE_CROWN
        + if mounted { MOUNTED_ANCHOR_RISE } else { 0.0 }
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
}

impl ChaseCamera {
    pub const PITCH_MIN: f32 = -0.30;

    /// Retail has no pitch clamp, because retail has no pitch: tilting adds to
    /// the eye's world Y (`CurrentEyePosition.y += offset`,
    /// research/XIClient/.../World/Camera/CameraManager.cpp:527-529) and leaves
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

    /// CameraManager.cpp:830-836 pushes an unobstructed eye back out whenever
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
    /// - research/XIClient/.../World/Camera/CameraManager.cpp:506 normalises the
    ///   orbit rate against it — `angle = 6.0f / eyeToTargetDistance * angle`.
    /// - Same file:822, the camera-follow easing changes regime above 6.
    /// - research/xim/.../camera/PolarCamera.kt:24 `maximumRadius = 6f`.
    ///
    /// The resting distance is nearer still: CameraManager.cpp:95 places the
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
    /// distance grows instead, and CameraManager.cpp:822 eases it back toward
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
            // PolarCamera.kt:46). Was 18.0, which is now past DIST_MAX.
            distance: Self::DIST_MAX,
            smoothing: 0.18,
            synced_initial: false,
            snap_to_anchor: false,
        }
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
            far: crate::skybox::camera_far(settings.view_distance),
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

    // DLSS SR replaces both MSAA and TAA (settings.msaa() reports Off for
    // AaMode::Dlss, and dlss_active() can't be true at the same time as
    // wants_taa()). The component's #[require] pulls in TemporalJitter,
    // MipBias, DepthPrepass, MotionVectorPrepass and Hdr automatically. Gated
    // on dlss_active(), not the raw mode: with the runtime unsupported (or on
    // a default build, where this block doesn't compile at all) the camera
    // comes up plain and the menu shows DLSS (N/A).
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

pub fn chase_camera_system() {
    // RETIRED. The chase camera is now owned entirely by the single authority
    // `resolve_camera` (kuluu/src/view_native/camera_collision.rs), which lives
    // in the crate that can reach the avian world for collision. This fn is kept
    // only as a scheduling anchor for the systems in mod.rs that order against
    // `chase_camera_system`; it takes no params and does nothing. Do not add
    // camera logic here — it belongs in resolve_camera.
}

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

    // snap_to_anchor_places_eye_behind_player_without_smoothing migrated to
    // kuluu::view_native::camera_collision after the WIP camera work retired this
    // crate's chase authority (resolve_camera is now the single eye owner).

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
