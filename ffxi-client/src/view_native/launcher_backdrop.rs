use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use ffxi_client::lobby_client::CharSlot;
use ffxi_viewer_core::SceneState;

use super::collision_bvh::{CollisionBvh, ZoneCollisionBvh};
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

#[derive(Clone, Copy, PartialEq)]
enum FadeCut {
    Zone(u16),

    Segment,
}

#[derive(Resource, Default, Clone, Copy)]
enum BackdropFade {
    #[default]
    Idle,

    FadingOut {
        cut: FadeCut,
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

// Flythrough tuning, matched to the retail character-select backdrop observed
// on HorizonXI (artifacts/retail/20260719-1419*.png): a continuous slow dolly
// covering roughly a run-speed's distance per second, with no cut inside a 37s
// observation window.
const FLIGHT_SPEED: f32 = 5.0;
const FLIGHT_SEGMENT_SECS: f32 = 45.0;
// High enough to clear most MMB tree canopies, which the MZB collision probes
// cannot see.
const FLIGHT_CRUISE_HEIGHT: f32 = 14.0;
const FLIGHT_HEIGHT_SMOOTH_PER_SEC: f32 = 1.5;
const FLIGHT_TURN_RATE: f32 = 2.0_f32 * core::f32::consts::PI / 180.0;
const FLIGHT_PITCH: f32 = -10.0_f32 * core::f32::consts::PI / 180.0;
const FLIGHT_WALL_PROBE: f32 = 14.0;
const FLIGHT_PROBE_SKY: f32 = 40.0;
const FLIGHT_PROBE_RANGE: f32 = 400.0;
const VANTAGE_ATTEMPTS: u32 = 16;
const VANTAGE_EDGE_MARGIN: f32 = 0.15;

#[derive(Resource, Default)]
struct BackdropFlight {
    seated: bool,
    segment: u32,
    elapsed: f32,
    yaw: f32,
    turn_sign: f32,
}

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
            .init_resource::<BackdropFlight>()
            .insert_resource(ClearColor(Color::NONE))
            .add_systems(OnEnter(AppPhase::Launcher), spawn_backdrop_camera)
            .add_systems(OnExit(AppPhase::Launcher), despawn_backdrop_camera)
            .add_systems(
                Update,
                (
                    update_backdrop_from_selection,
                    drive_backdrop_fade,
                    drive_backdrop_flight,
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
        cut: FadeCut::Zone(target),
        elapsed: 0.0,
    };
}

fn drive_backdrop_fade(
    time: Res<Time>,
    mut fade: ResMut<BackdropFade>,
    mut zone: ResMut<LauncherBackdropZone>,
    mut flight: ResMut<BackdropFlight>,
    pending: Res<PendingBackdropSwap>,
) {
    let dt = time.delta_secs();
    *fade = match *fade {
        BackdropFade::Idle => return,
        BackdropFade::FadingOut { cut, elapsed } => {
            let next = elapsed + dt;
            if next >= FADE_OUT_SECS {
                match cut {
                    FadeCut::Zone(target) => zone.0 = target,
                    FadeCut::Segment => {}
                }
                flight.segment = flight.segment.wrapping_add(1);
                flight.seated = false;
                BackdropFade::Holding { elapsed: 0.0 }
            } else {
                BackdropFade::FadingOut { cut, elapsed: next }
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
                        cut: FadeCut::Zone(target),
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

fn drive_backdrop_flight(
    time: Res<Time>,
    zone_bvh: Res<ZoneCollisionBvh>,
    mut flight: ResMut<BackdropFlight>,
    mut fade: ResMut<BackdropFade>,
    mut cams: Query<&mut Transform, With<BackdropCamera>>,
) {
    let Some(bvh) = zone_bvh.0.as_ref() else {
        flight.seated = false;
        return;
    };
    let Ok(mut cam) = cams.single_mut() else {
        return;
    };

    // Geometry streams in chunk-by-chunk, rebuilding the BVH many times per
    // zone load; reseating on every rebuild would pin the camera to the same
    // deterministic vantage. Only reseat when unseated or when the rebuilt
    // BVH has no ground under the camera (a genuine zone swap).
    let lost_ground = zone_bvh.is_changed()
        && bvh
            .ray_cast(
                cam.translation + Vec3::Y * FLIGHT_PROBE_SKY,
                Vec3::NEG_Y,
                FLIGHT_PROBE_RANGE,
            )
            .is_none();
    if !flight.seated || lost_ground {
        seat_at_vantage(bvh, &mut flight, &mut cam);
        return;
    }

    let dt = time.delta_secs();
    flight.elapsed += dt;
    flight.yaw += flight.turn_sign * FLIGHT_TURN_RATE * dt;

    let forward = Quat::from_rotation_y(flight.yaw) * Vec3::NEG_Z;
    let look =
        Quat::from_rotation_y(flight.yaw) * Quat::from_rotation_x(FLIGHT_PITCH) * Vec3::NEG_Z;

    cam.translation += forward * FLIGHT_SPEED * dt;
    cam.look_to(look, Vec3::Y);

    let probe_origin = cam.translation + Vec3::Y * FLIGHT_PROBE_SKY;
    let ground = bvh
        .ray_cast(probe_origin, Vec3::NEG_Y, FLIGHT_PROBE_RANGE)
        .map(|t| probe_origin.y - t);
    match ground {
        Some(ground_y) => {
            let target_y = ground_y + FLIGHT_CRUISE_HEIGHT;
            let blend = (FLIGHT_HEIGHT_SMOOTH_PER_SEC * dt).min(1.0);
            cam.translation.y += (target_y - cam.translation.y) * blend;
        }
        None => {
            cut_segment(&mut fade);
            return;
        }
    }

    if bvh
        .ray_cast(cam.translation, look, FLIGHT_WALL_PROBE)
        .is_some()
        || flight.elapsed >= FLIGHT_SEGMENT_SECS
    {
        cut_segment(&mut fade);
    }
}

fn cut_segment(fade: &mut BackdropFade) {
    if fade.is_idle() {
        *fade = BackdropFade::FadingOut {
            cut: FadeCut::Segment,
            elapsed: 0.0,
        };
    }
}

fn seat_at_vantage(bvh: &CollisionBvh, flight: &mut BackdropFlight, cam: &mut Transform) {
    let Some((min, max)) = bvh.root_aabb() else {
        return;
    };
    for attempt in 0..VANTAGE_ATTEMPTS {
        let bits = vantage_hash(flight.segment, attempt);
        let fx = unit_float(bits);
        let fz = unit_float(bits >> 21);
        let fyaw = unit_float(bits >> 42);
        let span = (max - min) * (1.0 - 2.0 * VANTAGE_EDGE_MARGIN);
        let base = min + (max - min) * VANTAGE_EDGE_MARGIN;
        let x = base.x + span.x * fx;
        let z = base.z + span.z * fz;
        let yaw = fyaw * core::f32::consts::TAU;

        let probe_origin = Vec3::new(x, max.y + FLIGHT_PROBE_SKY, z);
        let Some(t) = bvh.ray_cast(probe_origin, Vec3::NEG_Y, FLIGHT_PROBE_RANGE) else {
            continue;
        };
        let pos = Vec3::new(x, probe_origin.y - t + FLIGHT_CRUISE_HEIGHT, z);
        let look = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(FLIGHT_PITCH) * Vec3::NEG_Z;
        if bvh.ray_cast(pos, look, FLIGHT_WALL_PROBE).is_some() {
            continue;
        }

        cam.translation = pos;
        cam.look_to(look, Vec3::Y);
        flight.yaw = yaw;
        flight.turn_sign = if bits & 1 == 0 { 1.0 } else { -1.0 };
        flight.elapsed = 0.0;
        flight.seated = true;
        return;
    }
    // No open vantage found (e.g. geometry still streaming in): stay put and
    // let the segment timer retry from a fresh hash sequence.
    flight.segment = flight.segment.wrapping_add(1);
    flight.elapsed = 0.0;
    flight.seated = true;
}

// splitmix64 finalizer (Steele et al., "Fast Splittable Pseudorandom Number
// Generators") — deterministic vantage sequence, no RNG state to carry.
fn vantage_hash(segment: u32, attempt: u32) -> u64 {
    let mut z = ((segment as u64) << 32 | attempt as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

const UNIT_FLOAT_BITS: u32 = 21;

fn unit_float(bits: u64) -> f32 {
    (bits & ((1 << UNIT_FLOAT_BITS) - 1)) as f32 / (1u64 << UNIT_FLOAT_BITS) as f32
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
