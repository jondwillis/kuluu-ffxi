use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use kuluu_render::SceneState;
use kuluu_session::lobby_client::CharSlot;

use super::backdrop_preload::{self, BackdropPreloads};
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
pub(crate) struct BackdropScoped;

/// How long the character list's cursor has to rest on one entry before its zone
/// replaces the backdrop. Sweeping a list otherwise queues a full retire and
/// reload per keypress.
const HOVER_DWELL_SECS: f32 = 0.4;

#[derive(Resource, Default)]
struct BackdropHoverDwell {
    zone: Option<u16>,
    held: f32,
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
/// The Hold waits for the cut-to block to finish loading; this only stops a
/// stream that wedges from holding black forever.
const HOLD_MAX_SECS: f32 = 45.0;

const FADE_COLOR: Color = Color::srgb(0.04, 0.04, 0.05);

/// Flythrough tuning, matched to the retail character-select backdrop
/// observed on HorizonXI (screenshots taken 2026-07-19): a continuous slow
/// dolly covering roughly a run-speed's distance per second, with no cut
/// inside a 37s observation window.
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

/// Startup order for the launcher screens: black plus a corner loader while the
/// zone streams in behind it, then one fade onto a scene that can already answer
/// input. Streaming in the screen's own frame leaves it no frames to take input
/// in; lifting the veil mid-stream instead reveals half a background.
#[derive(Resource)]
struct StartupStage {
    phase: StartupPhase,
    elapsed: f32,
}

impl Default for StartupStage {
    fn default() -> Self {
        Self {
            phase: StartupPhase::HoldingScreens,
            elapsed: 0.0,
        }
    }
}

#[derive(Clone, Copy)]
enum StartupPhase {
    /// Black veil while the screens behind it are built and given their frames.
    HoldingScreens,
    /// Background streaming. The veil is now behind the screens, so they are read
    /// and clicked against black while the 3D loads behind them.
    LoadingBackground,
    /// Veil fading out to put the loaded scene behind the screens.
    Revealing,
    Done,
}

impl StartupStage {
    fn backdrop_allowed(&self) -> bool {
        !matches!(self.phase, StartupPhase::HoldingScreens)
    }
}

/// Black hold long enough for the screens to be laid out and drawn once before
/// anything is streamed.
const SCREEN_HOLD_SECS: f32 = 0.25;
const VEIL_FADE_SECS: f32 = 0.45;
const LOADER_DOTS_PER_SEC: f32 = 2.0;

/// The launcher's backdrop stream is a full load: distance-gated streaming would
/// hold every far placement `pending` forever with a static attract camera, and the
/// startup veil gate (`has_pending_zone_placements`) could never open - the sign-in
/// screen spun "Loading background" endlessly (owner 2026-10-22: the login "just
/// loads forever... the preload is AFTER the veil is dropped"). From the frame the
/// zone mirrors into the scene every placement streams, still bounded per frame by
/// the background budget so the screens keep their input.
fn sync_backdrop_stream_mode(
    background: Res<LauncherBackgroundEnabled>,
    stage: Res<StartupStage>,
    mut everything: ResMut<kuluu_render::dat_mmb::StreamEverything>,
) {
    let full = background.0 && !matches!(stage.phase, StartupPhase::HoldingScreens);
    if everything.0 != full {
        everything.0 = full;
    }
}

/// The veil covers the screens for its opening beat, then drops under every
/// launcher screen (`launcher_ui::footer` sits at 5) so only the 3D scene stays
/// behind it.
const STARTUP_VEIL_COVER_Z: i32 = 8;
const STARTUP_VEIL_BEHIND_UI_Z: i32 = -1;
const STARTUP_LOADER_Z: i32 = 9;
const LOADER_FONT_PX: f32 = 12.0;
/// Clears the launcher footer strip (`launcher_ui::footer::FOOTER_HEIGHT_PX`).
const LOADER_BOTTOM_INSET_PX: f32 = 44.0;
const LOADER_SIDE_INSET_PX: f32 = 16.0;

/// Three dot entities that exist the whole time and only change colour, so the
/// loader's box is exactly as wide in every frame and the word cannot shift.
const LOADER_DOT_COUNT: usize = 3;

const VEIL_COLOR: Color = Color::BLACK;
const LOADER_TEXT: &str = "Loading background";
const LOADER_TEXT_COLOR: Color = Color::srgb(0.62, 0.66, 0.70);

#[derive(Component)]
struct StartupVeil;

/// The corner loader's root node: the word and [`LOADER_DOT_COUNT`] dot glyphs.
#[derive(Component)]
struct StartupLoaderRoot;

/// One of the three dots. Lit while its `order` is below the current phase.
#[derive(Component)]
struct StartupLoaderDot {
    order: usize,
}

/// `-cpu-mode` registers the backdrop with `background: false`: the screens stay
/// on their own and no zone is streamed for them.
#[derive(Resource, Clone, Copy)]
pub struct LauncherBackgroundEnabled(pub bool);

pub struct LauncherBackdropPlugin {
    pub background: bool,
}

impl Plugin for LauncherBackdropPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(LauncherBackgroundEnabled(self.background))
            .insert_resource(LauncherBackdropZone(DEFAULT_BACKDROP_ZONE))
            .init_resource::<BackdropHoverDwell>()
            .init_resource::<BackdropFade>()
            .init_resource::<BackdropFlight>()
            .init_resource::<StartupStage>()
            .init_resource::<BackdropPreloads>()
            .init_resource::<kuluu_render::dat_mmb::StreamEverything>()
            .insert_resource(ClearColor(Color::NONE))
            .add_systems(
                OnEnter(AppPhase::Launcher),
                (spawn_startup_stage, spawn_backdrop_camera).chain(),
            )
            .add_systems(
                OnExit(AppPhase::Launcher),
                (
                    despawn_startup_stage,
                    forget_backdrop_preloads,
                    despawn_backdrop_camera,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    refresh_backdrop_preload_targets,
                    backdrop_preload::preload_backdrop_zones,
                )
                    .chain()
                    // Whatever the cursor will want next is warmed only with what
                    // the visible stream leaves over.
                    .after(kuluu_render::dat_mmb::process_load_mmb_requests)
                    .run_if(in_state(AppPhase::Launcher)),
            )
            .add_systems(
                Update,
                (
                    sync_backdrop_stream_mode,
                    advance_startup_stage,
                    update_startup_loader,
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

fn spawn_startup_stage(mut commands: Commands, background: Res<LauncherBackgroundEnabled>) {
    // Nothing is streamed under `-cpu-mode`, so there is nothing to veil for.
    if !background.0 {
        commands.insert_resource(StartupStage {
            phase: StartupPhase::Done,
            elapsed: 0.0,
        });
        return;
    }
    commands.insert_resource(StartupStage::default());
    commands.spawn((
        StartupVeil,
        GlobalZIndex(STARTUP_VEIL_COVER_Z),
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(VEIL_COLOR),
    ));
    let loader = commands
        .spawn((
            StartupLoaderRoot,
            GlobalZIndex(STARTUP_LOADER_Z),
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(LOADER_BOTTOM_INSET_PX),
                right: Val::Px(LOADER_SIDE_INSET_PX),
                ..default()
            },
            TextColor(LOADER_TEXT_COLOR),
        ))
        .id();
    commands.entity(loader).with_children(|loader| {
        loader.spawn((
            Text::new(LOADER_TEXT),
            TextFont::from_font_size(LOADER_FONT_PX),
            TextColor(LOADER_TEXT_COLOR),
        ));
        for order in 0..LOADER_DOT_COUNT {
            loader.spawn((
                StartupLoaderDot { order },
                Text::new("."),
                TextFont::from_font_size(LOADER_FONT_PX),
                // Off dots keep their glyph (and so their width) and merely go
                // clear; that is what stops the row from resizing.
                TextColor(LOADER_TEXT_COLOR.with_alpha(0.0)),
            ));
        }
    });
}

fn despawn_startup_stage(
    mut commands: Commands,
    q_veil: Query<Entity, With<StartupVeil>>,
    q_loader: Query<Entity, With<StartupLoaderRoot>>,
) {
    for e in q_veil.iter() {
        commands.entity(e).try_despawn();
    }
    // The loader's dots are its children, so this one takes the subtree.
    for e in q_loader.iter() {
        commands.entity(e).despawn();
    }
    commands.remove_resource::<StartupStage>();
}

/// The veil drops when the zone FLOOR has landed (the same `main_zone_floor_ready`
/// signal the in-game loading overlay reads) - that scene is then fully answerable
/// and stays so. Model placements keep streaming behind it afterwards, at the
/// background budget: they are preload, and per owner 2026-10-22 all preloading is
/// AFTER the veil drop, never a condition for it (waiting on every placement spun
/// the sign-in loader forever when far placements stayed pending).
fn advance_startup_stage(
    mut commands: Commands,
    time: Res<Time>,
    mut stage: ResMut<StartupStage>,
    mut q_veil: Query<(Entity, &mut BackgroundColor), With<StartupVeil>>,
    mut q_veil_z: Query<&mut GlobalZIndex, With<StartupVeil>>,
    q_loader: Query<Entity, With<StartupLoaderRoot>>,
    scene: Res<SceneState>,
    last_auto_loaded: Res<kuluu_render::dat_mzb::LastAutoLoadedZone>,
    mzb_in_flight: Res<kuluu_render::dat_mzb::LoadMzbInFlight>,
) {
    let dt = time.delta_secs();
    match stage.phase {
        StartupPhase::HoldingScreens => {
            stage.elapsed += dt;
            if stage.elapsed < SCREEN_HOLD_SECS {
                return;
            }
            stage.phase = StartupPhase::LoadingBackground;
            // From here the screens are foreground: only the 3D scene stays
            // covered, so input and rendering of the UI keep running normally.
            if let Ok(mut z) = q_veil_z.single_mut() {
                z.0 = STARTUP_VEIL_BEHIND_UI_Z;
            }
        }
        StartupPhase::LoadingBackground => {
            // The mirror writes the zone when this phase unlocks it, and that
            // write is what makes a floor something to wait for. Floor only:
            // placements stream on behind the screens after the veil drops.
            let floored = scene.snapshot.zone_id.is_some()
                && kuluu_render::dat_mzb::main_zone_floor_ready(
                    &scene.snapshot,
                    &last_auto_loaded,
                    &mzb_in_flight,
                );
            if floored {
                stage.phase = StartupPhase::Revealing;
                stage.elapsed = 0.0;
            }
        }
        StartupPhase::Revealing => {
            stage.elapsed += dt;
            let progress = (stage.elapsed / VEIL_FADE_SECS).clamp(0.0, 1.0);
            let Ok((veil, mut bg)) = q_veil.single_mut() else {
                stage.phase = StartupPhase::Done;
                return;
            };
            if progress < 1.0 {
                bg.0 = VEIL_COLOR.with_alpha(1.0 - progress);
                return;
            }
            commands.entity(veil).try_despawn();
            for e in q_loader.iter() {
                commands.entity(e).despawn();
            }
            stage.phase = StartupPhase::Done;
        }
        StartupPhase::Done => {}
    }
}

/// Lights dots one at a time and then drops them all, without ever changing
/// what is laid out: an unlit dot still occupies its glyph.
fn update_startup_loader(
    stage: Res<StartupStage>,
    time: Res<Time>,
    mut q_dots: Query<(&StartupLoaderDot, &mut TextColor)>,
) {
    if matches!(stage.phase, StartupPhase::Done) {
        return;
    }
    let lit = (time.elapsed_secs() * LOADER_DOTS_PER_SEC) as usize % (LOADER_DOT_COUNT + 1);
    for (dot, mut color) in q_dots.iter_mut() {
        let want = if dot.order < lit {
            LOADER_TEXT_COLOR
        } else {
            LOADER_TEXT_COLOR.with_alpha(0.0)
        };
        if **color != want {
            **color = want;
        }
    }
}

fn spawn_backdrop_camera(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    spawn_backdrop_core(&mut commands, &mut meshes, &mut materials);
}

pub(crate) fn spawn_backdrop_core(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let cam = commands
        .spawn((
            BackdropCamera,
            BackdropScoped,
            kuluu_render::weather::ZoneViewCamera,
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
    despawn_backdrop_core(&mut commands, &q);
}

fn mirror_backdrop_to_scene_state(
    background: Res<LauncherBackgroundEnabled>,
    stage: Res<StartupStage>,
    zone: Res<LauncherBackdropZone>,
    mut scene: ResMut<SceneState>,
) {
    // Writing the zone here is what starts the DAT stream behind it: never under
    // `-cpu-mode`, and not until the screens have had their own frames.
    if !background.0 || !stage.backdrop_allowed() {
        return;
    }
    let desired = Some(zone.0);
    if scene.snapshot.zone_id == desired {
        return;
    }
    scene.snapshot.zone_id = desired;
}

pub(crate) fn despawn_backdrop_core(
    commands: &mut Commands,
    q_scoped: &Query<Entity, With<BackdropScoped>>,
) {
    for e in q_scoped.iter() {
        // try_despawn: despawn() is recursive, so a parent earlier in the query may have
        // already freed this entity; bare despawn() floods the bevy_ecs error handler with
        // "Entity despawned" WARNs at login (same fix as despawn_ingame_entities).
        commands.entity(e).try_despawn();
    }
    commands.remove_resource::<BackdropFadeMaterial>();
}

fn update_backdrop_from_selection(
    cursor: Option<Res<CharCursor>>,
    chars: Res<CharListData>,
    zone: Res<LauncherBackdropZone>,
    mut fade: ResMut<BackdropFade>,
    time: Res<Time>,
    mut dwell: ResMut<BackdropHoverDwell>,
) {
    let hovered: Option<u16> = cursor
        .as_deref()
        .and_then(|c| chars.0.get(c.0))
        .and_then(slot_zone_for_backdrop);
    let wanted = hovered.filter(|hovered| *hovered != zone.0);

    match wanted {
        Some(target) if dwell.zone == Some(target) => dwell.held += time.delta_secs(),
        target => {
            dwell.zone = target;
            dwell.held = 0.0;
        }
    }

    if !fade.is_idle() || wanted.is_none() || dwell.held < HOVER_DWELL_SECS {
        return;
    }
    let Some(target) = wanted else {
        return;
    };
    dwell.zone = None;
    dwell.held = 0.0;

    // Always through black: a flip that skipped the veil would spawn its whole
    // prop set inside one frame, which is exactly the hitch this avoids. A
    // warmed zone just clears the hold in frames instead of seconds.
    *fade = BackdropFade::FadingOut {
        cut: FadeCut::Zone(target),
        elapsed: 0.0,
    };
}

/// After the first backdrop has landed, every other zone on the character list
/// is warmed complete in the background, so its flip clears the black hold in
/// frames instead of streaming from cold.
fn refresh_backdrop_preload_targets(
    stage: Res<StartupStage>,
    chars: Option<Res<CharListData>>,
    background: Res<LauncherBackgroundEnabled>,
    zone: Res<LauncherBackdropZone>,
    mut preloads: ResMut<BackdropPreloads>,
) {
    if !background.0 || !matches!(stage.phase, StartupPhase::Done) {
        return;
    }
    let Some(chars) = chars else {
        return;
    };
    let current = ffxi_dat::zone_dat::zone_id_to_mzb_file_id(zone.0);
    preloads.set_targets(
        chars
            .0
            .iter()
            .filter_map(slot_zone_for_backdrop)
            .filter(|slot_zone| *slot_zone != zone.0)
            .filter_map(ffxi_dat::zone_dat::zone_id_to_mzb_file_id)
            .filter(|file_id| Some(*file_id) != current)
            .collect(),
    );
}

fn forget_backdrop_preloads(mut preloads: ResMut<BackdropPreloads>) {
    preloads.forget_all();
}

fn drive_backdrop_fade(
    time: Res<Time>,
    mut fade: ResMut<BackdropFade>,
    mut zone: ResMut<LauncherBackdropZone>,
    mut flight: ResMut<BackdropFlight>,
    mmb_queue: Res<kuluu_render::dat_mmb::MmbLoadQueue>,
    mmb_parses: Res<kuluu_render::dat_mmb::MmbLoadInFlight>,
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
        // Black until the cut-to block has nothing left to load; a timer alone
        // would lift onto half a zone again.
        BackdropFade::Holding { elapsed } => {
            let next = elapsed + dt;
            let work_left = !mmb_parses.tasks.is_empty() || mmb_queue.has_pending_zone_placements();
            if (!work_left && next >= FADE_HOLD_SECS) || next >= HOLD_MAX_SECS {
                BackdropFade::FadingIn { elapsed: 0.0 }
            } else {
                BackdropFade::Holding { elapsed: next }
            }
        }
        // Always lands Idle; a hover still standing elsewhere is re-decided by
        // `update_backdrop_from_selection` from the latest cursor, so no live
        // re-cut here to fight it.
        BackdropFade::FadingIn { elapsed } => {
            let next = elapsed + dt;
            if next >= FADE_IN_SECS {
                BackdropFade::Idle
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
    let Some(bvh) = zone_bvh.tree() else {
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
    let mut z = ((segment as u64) << 32 | attempt as u64)
        .wrapping_add(kuluu_render::scheduler_runtime::SPLITMIX64_GOLDEN_RATIO);
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
