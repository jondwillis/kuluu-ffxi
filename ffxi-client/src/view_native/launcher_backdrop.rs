use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use ffxi_client::lobby_client::CharSlot;
use ffxi_viewer_core::SceneState;

use super::launcher_ui::{char_list::CharCursor, CharListData};
use super::AppPhase;

pub const BACKDROP_RENDER_LAYER: usize = 0;

pub const PREVIEW_RENDER_LAYER: usize = 1;

pub const DEFAULT_BACKDROP_ZONE: u16 = 102;

#[derive(Resource, Debug, Clone, Copy)]
pub struct LauncherBackdropZone(pub u16);

#[derive(Component)]
pub struct BackdropCamera;

#[derive(Component)]
struct BackdropScoped;

#[derive(Resource, Default)]
struct PendingBackdropSwap {
    target: Option<u16>,
}

#[derive(Resource, Default, Clone, Copy)]
enum BackdropFade {
    #[default]
    Idle,

    FadingOut {
        target: u16,
        elapsed: f32,
    },

    Holding {
        elapsed: f32,
    },

    FadingIn {
        elapsed: f32,
    },
}

impl BackdropFade {
    fn is_idle(&self) -> bool {
        matches!(self, BackdropFade::Idle)
    }

    fn alpha(&self) -> f32 {
        match *self {
            BackdropFade::Idle => 0.0,
            BackdropFade::FadingOut { elapsed, .. } => (elapsed / FADE_OUT_SECS).clamp(0.0, 1.0),
            BackdropFade::Holding { .. } => 1.0,
            BackdropFade::FadingIn { elapsed } => 1.0 - (elapsed / FADE_IN_SECS).clamp(0.0, 1.0),
        }
    }
}

const FADE_OUT_SECS: f32 = 0.25;

const FADE_HOLD_SECS: f32 = 0.40;
const FADE_IN_SECS: f32 = 0.35;

const FADE_COLOR: Color = Color::srgb(0.04, 0.04, 0.05);

#[derive(Component)]
struct BackdropFadeQuad;

#[derive(Resource)]
struct BackdropFadeMaterial(Handle<StandardMaterial>);

pub struct LauncherBackdropPlugin;

impl Plugin for LauncherBackdropPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(LauncherBackdropZone(DEFAULT_BACKDROP_ZONE))
            .init_resource::<PendingBackdropSwap>()
            .init_resource::<BackdropFade>()
            .insert_resource(ClearColor(Color::NONE))
            .add_systems(OnEnter(AppPhase::Launcher), spawn_backdrop_camera)
            .add_systems(OnExit(AppPhase::Launcher), despawn_backdrop_camera)
            .add_systems(
                Update,
                (
                    update_backdrop_from_selection,
                    drive_backdrop_fade,
                    apply_overlay_alpha,
                    mirror_backdrop_to_scene_state,
                )
                    .chain()
                    .run_if(in_state(AppPhase::Launcher)),
            );
    }
}

fn spawn_backdrop_camera(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let cam = commands
        .spawn((
            BackdropCamera,
            BackdropScoped,
            Camera3d::default(),
            Camera {
                order: -2,
                ..default()
            },
            RenderLayers::layer(BACKDROP_RENDER_LAYER),
            Transform::from_xyz(0.0, 6.0, 12.0).looking_at(Vec3::new(0.0, 2.0, 0.0), Vec3::Y),
        ))
        .id();

    let quad = meshes.add(Rectangle::new(40.0, 40.0));
    let mat = materials.add(StandardMaterial {
        base_color: FADE_COLOR.with_alpha(0.0),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        ..default()
    });
    commands.entity(cam).with_children(|c| {
        c.spawn((
            BackdropFadeQuad,
            BackdropScoped,
            Mesh3d(quad),
            MeshMaterial3d(mat.clone()),
            RenderLayers::layer(BACKDROP_RENDER_LAYER),
            Transform::from_xyz(0.0, 0.0, -0.2),
        ));
    });
    commands.insert_resource(BackdropFadeMaterial(mat));

    commands.spawn((
        BackdropScoped,
        DirectionalLight {
            illuminance: 8_000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

fn despawn_backdrop_camera(mut commands: Commands, q: Query<Entity, With<BackdropScoped>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }

    commands.remove_resource::<BackdropFadeMaterial>();
}

fn mirror_backdrop_to_scene_state(zone: Res<LauncherBackdropZone>, mut scene: ResMut<SceneState>) {
    let desired = Some(zone.0);
    if scene.snapshot.zone_id == desired {
        return;
    }
    scene.snapshot.zone_id = desired;
}

fn update_backdrop_from_selection(
    cursor: Option<Res<CharCursor>>,
    chars: Res<CharListData>,
    mut pending: ResMut<PendingBackdropSwap>,
    zone: Res<LauncherBackdropZone>,
    mut fade: ResMut<BackdropFade>,
) {
    let hovered: Option<u16> = cursor
        .as_deref()
        .and_then(|c| chars.0.get(c.0))
        .and_then(slot_zone_for_backdrop);
    pending.target = hovered;

    if !fade.is_idle() {
        return;
    }
    let Some(target) = hovered else {
        return;
    };
    if target == zone.0 {
        return;
    }
    *fade = BackdropFade::FadingOut {
        target,
        elapsed: 0.0,
    };
}

fn drive_backdrop_fade(
    time: Res<Time>,
    mut fade: ResMut<BackdropFade>,
    mut zone: ResMut<LauncherBackdropZone>,
    pending: Res<PendingBackdropSwap>,
) {
    let dt = time.delta_secs();
    *fade = match *fade {
        BackdropFade::Idle => return,
        BackdropFade::FadingOut { target, elapsed } => {
            let next = elapsed + dt;
            if next >= FADE_OUT_SECS {
                zone.0 = target;
                BackdropFade::Holding { elapsed: 0.0 }
            } else {
                BackdropFade::FadingOut {
                    target,
                    elapsed: next,
                }
            }
        }
        BackdropFade::Holding { elapsed } => {
            let next = elapsed + dt;
            if next >= FADE_HOLD_SECS {
                BackdropFade::FadingIn { elapsed: 0.0 }
            } else {
                BackdropFade::Holding { elapsed: next }
            }
        }
        BackdropFade::FadingIn { elapsed } => {
            let next = elapsed + dt;
            if next >= FADE_IN_SECS {
                match pending.target {
                    Some(target) if target != zone.0 => BackdropFade::FadingOut {
                        target,
                        elapsed: 0.0,
                    },
                    _ => BackdropFade::Idle,
                }
            } else {
                BackdropFade::FadingIn { elapsed: next }
            }
        }
    };
}

fn apply_overlay_alpha(
    fade: Res<BackdropFade>,
    mat_handle: Option<Res<BackdropFadeMaterial>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(mat_handle) = mat_handle else {
        return;
    };
    let Some(mut mat) = materials.get_mut(&mat_handle.0) else {
        return;
    };
    let want = fade.alpha();
    let current = mat.base_color.alpha();
    if (current - want).abs() < 0.001 {
        return;
    }
    mat.base_color = FADE_COLOR.with_alpha(want);
}

fn slot_zone_for_backdrop(slot: &CharSlot) -> Option<u16> {
    if slot.race == 0 || slot.zone_id == 0 {
        return None;
    }
    Some(slot.zone_id)
}
