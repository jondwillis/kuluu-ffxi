use bevy::camera::Hdr;
use bevy::light::{ShadowFilteringMethod, VolumetricFog};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;

#[cfg(not(target_arch = "wasm32"))]
use bevy::anti_alias::taa::TemporalAntiAliasing;

use crate::components::IsSelf;
#[cfg(not(target_arch = "wasm32"))]
use crate::graphics_settings::AaMode;
use crate::graphics_settings::GraphicsSettings;
use crate::scene::BakedActor;
use crate::snapshot::SceneState;

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

#[inline]
pub fn nameplate_anchor_y(baked: Option<&BakedActor>) -> f32 {
    baked
        .map(|b| b.actor_height)
        .unwrap_or(FALLBACK_ACTOR_HEIGHT)
        + NAMEPLATE_OFFSET_ABOVE_CROWN
}

#[derive(Component)]
pub struct OperatorCamera;

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
}

impl ChaseCamera {
    pub const PITCH_MIN: f32 = -0.30;

    pub const PITCH_MAX: f32 = 1.40;

    pub const FP_PITCH_MIN: f32 = -std::f32::consts::FRAC_PI_2 + 0.05;

    pub const FP_PITCH_MAX: f32 = std::f32::consts::FRAC_PI_2 - 0.05;

    pub const DIST_MIN: f32 = 2.0;

    pub const DIST_MAX: f32 = 20.0;

    pub const KEYBOARD_ZOOM_RATE: f32 = 10.0;
}

impl Default for ChaseCamera {
    fn default() -> Self {
        Self {
            yaw: 0.0,

            pitch: 0.15,
            distance: 18.0,
            smoothing: 0.18,
            synced_initial: false,
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
            saved_chase_dist: 18.0,
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
    let mut camera = commands.spawn((
        crate::components::InGameEntity,
        OperatorCamera,
        bevy::camera::visibility::RenderLayers::from_layers(&[0, WORLD_GIZMO_LAYER]),
        Camera3d::default(),
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
            far: settings.view_distance,
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
}

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

    if !chase.synced_initial {
        chase.yaw = yaw_for_heading(state.snapshot.self_pos.heading);
        chase.synced_initial = true;
    }

    let cos_p = chase.pitch.cos();
    let sin_p = chase.pitch.sin();
    let yaw_dir = Vec3::new(chase.yaw.sin(), 0.0, chase.yaw.cos());

    let anchor_y = third_person_anchor_y(baked);
    let anchor = self_t.translation + Vec3::Y * anchor_y;
    let desired = anchor + yaw_dir * (chase.distance * cos_p) + Vec3::Y * (chase.distance * sin_p);

    cam_t.translation = cam_t.translation.lerp(desired, chase.smoothing);
    cam_t.look_at(anchor, Vec3::Y);
}

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
}
