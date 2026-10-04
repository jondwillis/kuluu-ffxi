//! Pre-server animation test box (dev-only, no session layer). A chip in the launcher corner
//! loads a small scene — carrion worm left, sworded Hume right, both from retail DATs — plus a
//! panel that fires ROM/0/0.DAT's dam0 cascade through kuluu-render's production routine,
//! particle and audio systems. Every press logs its path (what dam0 picked, which stages ran)
//! to this window AND stderr so both sides see it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use kuluu_render::components::{InGameEntity, WorldEntity};
use kuluu_render::dat_mzb::{LastAutoLoadedZone, LoadMzbRequest, ZONE_SLOT_MAIN};
use kuluu_render::ffxi_actor_render::{
    ActorSubject, FfxiActorMeshChild, FfxiRenderActor, FfxiRenderRoot, LoadActorRequest,
};
use kuluu_render::scene::TrackedEntities;
use kuluu_render::scheduler_runtime::{
    enqueue_routine, parse_action_bytes_reporting, stage_summary, ActionDatRoot, ActionTarget,
    ActiveScheduler, GlobalEffectDir, ParticleSpawnTrace, RoutineLookup, VfxTrace,
    LEVEL_UP_EFFECT_DAT_ID,
};
use kuluu_render::snapshot::{EventLog, SceneState};
use kuluu_snapshot::EntityKind;

// Carrion Worm family model (kuluu-render/tests/rabbit_tester.rs S12 load).
const WORM_FILE: u32 = 1724;
// The equipment table's main-hand (8392) row is a bare-hand stub: its VertexOs2 chunk carries
// joint-mapped vertices but no polygon instructions, so nothing draws from it. Production fills
// the slot from the equipped item; the box pins a real sword file with geometry.
const TEST_SWORD_FILE: u32 = 8397;

// LSB zone id for West Ronfaure (zone table maps it to mzb file 200, ROM/0/120.DAT).
const WEST_RONFAURE_ZONE_ID: u16 = 100;

// MZB/DAT file id the zone table resolves West Ronfaure to.
const WEST_RONFAURE_MZB_FILE_ID: u32 = 200;

// LSB zone id for South Gustaberg (the Bastok tunnel entrance). DAT file 207 carries both the
// tunnel MZB and the `ligh` lamp dir — li00..li11 fixtures, lt00..lt11 halos, ghu1/ghu2 — all gated
// by the tkaa time-of-day track (dat-lamp-glow-probe: ROM/0/124.DAT).
const SOUTH_GUSTABERG_ZONE_ID: u16 = 107;
const SOUTH_GUSTABERG_MZB_FILE_ID: u32 = 207;

// Reference camera for lamp work: the in-game standing spot used by every lamp capture, read from
// the character's own entity stream (x=262.766 y=-205.876 z=2.326) through scene::ffxi_to_bevy
// (x,-z,-y); eye = that spot raised to head height. The halo cluster around it (DAT 207 lt* base_pos
// rows through mzb_to_bevy: x in 255.8..264.2, glass at +2.29) sits ~21 yalms ahead along -Z, so
// the look-at is that cluster's centroid — reproduces exactly where the character stands and looks.
const SG_LAMP_EYE: Vec3 = Vec3::new(262.77, -0.6, 205.88);
const SG_LAMP_LOOK_AT: Vec3 = Vec3::new(260.0, 1.5, 184.4);

// Game hour for the lamp room: just past dusk on DAT 207's tkaa gate (off between ~4.39h and
// ~17.46h, ≈0.96 after), held still by VanaClock::freeze_at_hour_minute so shots repeat.
const SG_LAMP_HOUR: u32 = 18;

// Lantern-alpha slider geometry (logical px): track fills the panel's inner width (215 - 2x8 pad).
const LAMP_SLIDER_TRACK_W: f32 = 195.0;
const LAMP_SLIDER_TRACK_H: f32 = 24.0;
const LAMP_SLIDER_KNOB_W: f32 = 10.0;

// Retail reference standing point (in-game debug readout x=-238.241 y=139.944 z=-49.754,
// wire order: z is height) — mid-zone open grass, tree line west, campfire at (-293, 137),
// the yama_2/5 mountains east behind the camera. Placements spawn at
// mzb_to_bevy(zone_pos) + world_pos, so world_pos is the negation of that conversion:
// -(-238.241, 49.754, -139.944).
const WR_ENTRY_OFFSET: Vec3 = Vec3::new(238.241, -49.754, 139.944);

// ANIMTEST_ZONE_ID / ANIMTEST_MZB_FILE_ID / ANIMTEST_WORLD_POS ("x,y,z") override the LoadZone
// case so external captures can frame any zone — South Gustaberg's tunnel lamps load at world_pos
// ZERO, where zone geometry lands at absolute mzb_to_bevy(native) coordinates.
fn env_zone_override() -> Option<(u16, u32, Vec3)> {
    let zone_id: u16 = std::env::var("ANIMTEST_ZONE_ID").ok()?.parse().ok()?;
    let mzb_file: u32 = std::env::var("ANIMTEST_MZB_FILE_ID").ok()?.parse().ok()?;
    let world_pos = match std::env::var("ANIMTEST_WORLD_POS") {
        Ok(s) => s
            .split(',')
            .map(|t| t.trim().parse::<f32>())
            .collect::<Result<Vec<_>, _>>()
            .ok()?
            .into_iter()
            .take(3)
            .chain(std::iter::repeat(0.0))
            .take(3)
            .collect::<Vec<_>>(),
        Err(_) => Vec3::ZERO.to_array().to_vec(),
    };
    if world_pos.len() != 3 {
        return None;
    }
    Some((zone_id, mzb_file, Vec3::from_slice(&world_pos)))
}

// ANIMTEST_HOUR pins VanaClock to a fixed game hour on LoadZone (night/midday captures).
fn env_hour_override() -> Option<f32> {
    std::env::var("ANIMTEST_HOUR").ok()?.trim().parse().ok()
}

// ANIMTEST_ACTOR_POS="x,y,z" re-bases the worm/hume pair (worm at base-1x, hume at base+1x)
// so an actor can stand under a zone lamp for lighting captures.
fn env_actor_pos() -> Option<Vec3> {
    let v: Vec<f32> = std::env::var("ANIMTEST_ACTOR_POS")
        .ok()?
        .split(',')
        .map(|t| t.trim().parse())
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if v.len() != 3 {
        return None;
    }
    Some(Vec3::from_slice(&v))
}

// ANIMTEST_CAM="px,py,pz,tx,ty,tz" overrides the box camera's position and look-at target.
fn env_camera_override() -> Option<(Vec3, Vec3)> {
    let v: Vec<f32> = std::env::var("ANIMTEST_CAM")
        .ok()?
        .split(',')
        .map(|t| t.trim().parse::<f32>())
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if v.len() != 6 {
        return None;
    }
    Some((Vec3::from_slice(&v[0..3]), Vec3::from_slice(&v[3..6])))
}

const WORM_ID: u32 = 1;
const HUME_ID: u32 = 2;

/// The worm's `dead` fall-over hides the model after this long; respawn brings it back.
const WORM_HIDE_AFTER_SECS: f32 = 2.5;

#[derive(Resource, Default)]
struct TestLog {
    lines: Vec<String>,
}

/// Both sinks at once: the on-screen panel and stderr (the terminal next to the window).
fn log_line(log: &mut TestLog, msg: String) {
    eprintln!("[animationtest] {msg}");
    log.lines.push(msg);
    if log.lines.len() > 400 {
        log.lines.drain(0..log.lines.len() - 400);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Case {
    PlayerNhIt,
    PlayerChit,
    PlayerDhit,
    MobNhIt,
    MobChit,
    MobRespawn,
    LevelUp,
    Gen141,
    Gen144,
    Hit1Full,
    Hi26,
    Sb00,
    I900,
    LoadZone,
    LoadWeather,
    SgLamps,
    Shot,
}

impl Case {
    fn label(self) -> &'static str {
        match self {
            Self::PlayerNhIt => "player normal hit",
            Self::PlayerChit => "player crit hit",
            Self::PlayerDhit => "player death hit",
            Self::MobNhIt => "mob normal hit",
            Self::MobChit => "mob crit hit",
            Self::MobRespawn => "mob respawn",
            Self::LevelUp => "player level up",
            Self::Gen141 => "g141 (alpha 1)",
            Self::Gen144 => "g144 (alpha 1)",
            Self::Hit1Full => "hit1 full (141/144 alpha 1)",
            Self::Hi26 => "hi26 routine (g261 child carrier)",
            Self::Sb00 => "sb00 routine (gs02 child carrier)",
            Self::I900 => "i900 zone gen (ai90 haze child)",
            Self::LoadZone => "load zone (West Ronfaure)",
            Self::LoadWeather => "load weather (clouds)",
            Self::SgLamps => "south gusta lamps @ 18:00",
            Self::Shot => "screenshot (GPU readback)",
        }
    }

    fn info_bits(self) -> u8 {
        match self {
            Self::PlayerNhIt | Self::MobNhIt => 0,
            Self::PlayerChit | Self::MobChit => ffxi_proto::melee::INFO_CRITICAL_HIT,
            Self::PlayerDhit => ffxi_proto::melee::INFO_DEFEATED,
            _ => 0,
        }
    }

    fn damage(self) -> u32 {
        match self {
            Self::PlayerNhIt | Self::MobNhIt => 5,
            Self::PlayerChit | Self::MobChit => 10,
            Self::PlayerDhit => TEST_MAX_HP * 2,
            _ => 0,
        }
    }
}

#[derive(Resource, Default)]
struct PendingCase(Option<Case>);

// run_pending_case sits at bevy's 16-parameter system ceiling (the ZoneLoadParams bundle is
// what keeps it there), so the shot rides a resource a dedicated two-param system consumes.
fn fire_pending_shot(
    mut shot: ResMut<PendingShot>,
    mut requests: MessageWriter<super::screenshot::ScreenshotRequest>,
) {
    let Some(path) = shot.0.take() else {
        return;
    };
    eprintln!("[animationtest] screenshot -> {}", path.display());
    requests.write(super::screenshot::ScreenshotRequest { path });
}

/// Worm death bookkeeping: dhit runs the `dead` fall-over, then hides the model.
#[derive(Resource, Default)]
struct WormState {
    dead_at: Option<Instant>,
}

const TEST_MAX_HP: u32 = 10_000;

/// Absolute-HP bookkeeping for the simulated combat flushes: the snapshot only carries a
/// percentage (the 0x0E wire shape), so damage is tracked here and converted on each hit.
#[derive(Resource, Default)]
struct TestHp {
    hume: u32,
    worm: u32,
}

/// One case at a time: while set, presses are rejected until the animation window elapses.
#[derive(Resource, Default)]
struct CaseLock {
    until: Option<Instant>,
}

// One-shot per activation: exp_* are the buffer counts from the load-time check; *_done mark
// the post-spawn drawn count already logged.
#[derive(Resource, Default)]
struct DrawnCheck {
    hume_done: bool,
    worm_done: bool,
    parts_ok: bool,
    exp_hume: usize,
    exp_worm: usize,
}

/// Everything the test scene spawns (3D + UI), so teardown takes it all down at once.
#[derive(Component)]
pub(crate) struct TestSceneScoped;

/// Set by the launcher's AnimationTest titlebar button (gated on the
/// `debug-animation_room` feature); consumed by handle_toggle.
#[derive(Resource, Default)]
pub(crate) struct PendingToggle(pub bool);

// ANIMTEST_AUTO schedule: the first case fires after the actors have loaded, then one per
// spacing — long enough for each effect's particles to live out their life between shots.
const AUTO_FIRST_DELAY_SECS: f32 = 4.0;
const AUTO_SPACING_SECS: f32 = 7.0;

#[derive(Resource, Default)]
struct AutoFire {
    queue: std::collections::VecDeque<Case>,
    next_at: Option<Instant>,
}

fn case_from_name(s: &str) -> Option<Case> {
    Some(match s {
        "nhit" => Case::PlayerNhIt,
        "chit" => Case::PlayerChit,
        "dhit" => Case::PlayerDhit,
        "mobnhit" => Case::MobNhIt,
        "mobchit" => Case::MobChit,
        "respawn" => Case::MobRespawn,
        "levelup" => Case::LevelUp,
        "g141" => Case::Gen141,
        "g144" => Case::Gen144,
        "hit1full" => Case::Hit1Full,
        "hi26" => Case::Hi26,
        "sb00" => Case::Sb00,
        "i900" => Case::I900,
        "zone" => Case::LoadZone,
        "weather" => Case::LoadWeather,
        "sglamps" => Case::SgLamps,
        // ANIMTEST_SHOT_PATH names the PNG; Bevy reads back the render target, so this works
        // with KULUU_WINDOW_HIDDEN=1 where no window capture can reach.
        "shot" => Case::Shot,
        _ => return None,
    })
}

// True while a real zone block is loaded into the box: zone_backdrop_visibility exists to
// hide the launcher backdrop's own geometry and would swallow the test zone too.
#[derive(Resource, Default)]
struct TestZoneActive(bool);

/// The flat test floor; hidden once a real zone loads under it (standalone tester parity).
#[derive(Component)]
struct TestFloor;

#[derive(Resource, Default)]
struct FloorHidden(bool);

/// One-shot screenshot request the box fires through the production Screenshot path (GPU
/// readback of the render target — valid while the window is buried).
#[derive(Resource, Default)]
struct PendingShot(Option<std::path::PathBuf>);

/// Set when the lamp-room case fires and cleared on teardown: drives the clock freeze and camera
/// framing edges in arm_lamp_room.
#[derive(Resource, Default)]
struct LampRoomActive(bool);

// Hand-rolled lantern-alpha slider — bevy_ui 0.19 ships no Slider widget. The track carries a
// Button so its Interaction tracks the whole mouse-hold (same mechanism as the case buttons), and
// drive_lamp_alpha_slider maps the held cursor through ComputedNode::normalize_point, which is the
// same physical-pixel space bevy_ui's own focus pass hit-tests in.
// While set, the occlusion raycast stands down so the lamp room can be lit without
// raycast-zeroed bindings. Seeded on (rays suppressed) by the lamp-room case; with no panel
// control here it stays as seeded until teardown resets it.
#[derive(Resource, Default)]
pub struct LampRaysOff(pub bool);

// The Enhanced half of the user's graphics settings (`dynamic_lights` = Lamps + Shadows makes each
// DAT lamp a real Bevy PointLight with cube shadow maps and flicker; volumetric fog scatters over
// the whole frame), neither of which exists in retail. Neither belongs under a lamp capture, so
// this checkbox (and the lamp-room seed) suppresses them and restores exactly what it replaced on
// un-suppress or teardown. `persist_graphics_on_change` rewrites graphics.json at both edges like
// any menu change would; the value ends up back where it started.
#[derive(Resource, Default)]
pub struct ShadowsOff(pub bool);

#[derive(Component)]
struct ShadowsCheckbox;

// The night-fx kill switches (panel rows after shadows): sun/moon/fog/stars, applied through the
// renderer's `SkyFxOverride` — sun and moon pin their celestial below the horizon (light, disc,
// landscape dir term), fog pulls the camera's DistanceFog, stars hide the dome. All default off;
// only the box ever flips them (see `kuluu_render::sun_moon::SkyFxOverride`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum SkyFxField {
    Sun,
    Moon,
    Fog,
    Stars,
}

impl SkyFxField {
    fn label(self) -> &'static str {
        match self {
            Self::Sun => "sun",
            Self::Moon => "moon",
            Self::Fog => "fog",
            Self::Stars => "stars",
        }
    }

    fn get(self, ov: &kuluu_render::sun_moon::SkyFxOverride) -> bool {
        match self {
            Self::Sun => ov.sun,
            Self::Moon => ov.moon,
            Self::Fog => ov.fog,
            Self::Stars => ov.stars,
        }
    }

    fn set(self, ov: &mut kuluu_render::sun_moon::SkyFxOverride) -> &mut bool {
        match self {
            Self::Sun => &mut ov.sun,
            Self::Moon => &mut ov.moon,
            Self::Fog => &mut ov.fog,
            Self::Stars => &mut ov.stars,
        }
    }
}

#[derive(Component)]
struct SkyCheckbox(SkyFxField);

// Lamps kill switch: checked hides the lig* halo billboards (the lantern glow sprites).
#[derive(Component)]
struct LampsCheckbox;

// Wall-glow kill switch: unchecked empties the faithful ActiveSceneLights feed, so the FFXI
// zone/actor materials lose the authored `ligh` palette that paints the stone around each lamp.
#[derive(Component)]
struct WallGlowCheckbox;

/// What suppression replaced: `None` while shadows run normally. The camera's VolumetricFog
/// component is stashed whole (it is only ever inserted at camera spawn, camera.rs), so restoring
/// re-inserts the exact same values rather than a second copy of its defaults.
#[derive(Resource, Default)]
struct ShadowOverrides(Option<ShadowSnapshot>);

/// The user's own Dynamic Lights value, stashed the first time the box flips enhance mode —
/// teardown hands it back so closing the box never rewrites a setting the box was never told
/// to change.
#[derive(Resource, Default)]
struct EnhanceRestore(Option<kuluu_render::graphics_settings::DynamicLights>);

#[derive(Clone)]
struct ShadowSnapshot {
    dynamic_lights: kuluu_render::graphics_settings::DynamicLights,
    light_flicker: bool,
    volumetric_fog: Option<bevy::light::VolumetricFog>,
}

#[derive(Component)]
struct LampSliderTrack;
#[derive(Component)]
struct LampSliderKnob;
#[derive(Component)]
struct LampSliderLabel;

// Wall-wash alpha slider (under the lantern one): same hand-rolled track/knob, range 0..2 over
// the full track width (1.0 = authored, knob at mid-travel).
#[derive(Component)]
struct WashSliderTrack;
#[derive(Component)]
struct WashSliderKnob;
#[derive(Component)]
struct WashSliderLabel;

// The LoadZone case's params as one SystemParam: Bevy 0.19 generates IntoSystem for fn pointers
// up to 16 params (bevy_ecs function_system all_tuples! impl_build_system 0..=16), and
// run_pending_case sits exactly at that cap with this bundle.
#[derive(SystemParam)]
struct ZoneLoadParams<'w> {
    load_tx: MessageWriter<'w, LoadMzbRequest>,
    last_zone: ResMut<'w, LastAutoLoadedZone>,
    backdrop_zone: ResMut<'w, super::launcher_backdrop::LauncherBackdropZone>,
    floor_hidden: ResMut<'w, FloorHidden>,
    lamp_rays: ResMut<'w, LampRaysOff>,
    sky_fx: ResMut<'w, kuluu_render::sun_moon::SkyFxOverride>,
    lamps_off: ResMut<'w, kuluu_render::particle_sim::LampHalosOff>,
    wall_glow_off: ResMut<'w, kuluu_render::particle_sim::WallWashOff>,
}

#[derive(Component)]
struct CloseBox;

#[derive(Component)]
struct CaseButton(Case);

#[derive(Component)]
struct LogText;

pub struct AnimationTestScenePlugin;

impl Plugin for AnimationTestScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TestLog>()
            .init_resource::<LampRaysOff>()
            .init_resource::<DrawnCheck>()
            .init_resource::<PendingCase>()
            .init_resource::<WormState>()
            .init_resource::<TestHp>()
            .init_resource::<CaseLock>()
            .init_resource::<PendingToggle>()
            .init_resource::<TestZoneActive>()
            .init_resource::<FloorHidden>()
            .init_resource::<PendingShot>()
            .init_resource::<ShadowsOff>()
            .init_resource::<ShadowOverrides>()
            .init_resource::<kuluu_render::sun_moon::SkyFxOverride>()
            .init_resource::<kuluu_render::particle_sim::LampHalosOff>()
            .init_resource::<kuluu_render::particle_sim::WallWashOff>()
            .init_resource::<kuluu_render::zone_point_lights::ZoneLampLightsOff>()
            .init_resource::<EnhanceRestore>()
            .init_resource::<LampRoomActive>();
        // ANIMTEST_AUTO=nhit,chit,... — fire the named cases on a fixed clock with no input
        // (standalone tester parity); opening the box too, so the whole run is hands-free.
        let auto_cases: Vec<Case> = std::env::var("ANIMTEST_AUTO")
            .unwrap_or_default()
            .split(',')
            .filter_map(|s| case_from_name(s.trim()))
            .collect();
        if !auto_cases.is_empty() {
            eprintln!(
                "[animationtest] ANIMTEST_AUTO: {}",
                auto_cases
                    .iter()
                    .map(|c| c.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            app.world_mut().resource_mut::<PendingToggle>().0 = true;
        }
        app.insert_resource(AutoFire {
            queue: std::collections::VecDeque::from(auto_cases),
            next_at: Some(Instant::now() + Duration::from_secs_f32(AUTO_FIRST_DELAY_SECS)),
        });
        app.add_systems(OnExit(super::AppPhase::Launcher), tear_down_test_scene)
            .add_systems(
                Update,
                sync_shadow_override.run_if(in_state(super::AppPhase::Launcher)),
            )
            .add_systems(
                Update,
                (
                    handle_toggle,
                    handle_close_press,
                    auto_fire_cases,
                    handle_case_presses,
                    run_pending_case,
                    fire_pending_shot,
                    apply_floor_hidden,
                    arm_lamp_room,
                    drive_lamp_alpha_slider,
                    verify_drawn,
                    worm_death_watch,
                    zone_backdrop_visibility,
                    watch_wires,
                    sync_case_buttons,
                    collect_spawn_traces,
                    sync_log_text,
                )
                    .chain()
                    .run_if(in_state(super::AppPhase::Launcher)),
            )
            // The checkbox/slider systems live outside the chained group — adding them there
            // would cross bevy's 17-element tuple ceiling for one add_systems call.
            .add_systems(
                Update,
                (
                    toggle_shadows_checkbox,
                    toggle_sky_checkboxes,
                    toggle_lamps_checkbox,
                    toggle_wall_glow_checkbox,
                    drive_wash_alpha_slider,
                )
                    .run_if(in_state(super::AppPhase::Launcher)),
            )
            // Last (post command-flush): the box's own spawn is deferred, so this is the earliest
            // point that sees it — no frame of launcher-camera coexistence.
            .add_systems(
                bevy::prelude::Last,
                apply_test_unload.run_if(in_state(super::AppPhase::Launcher)),
            );
    }
}

// The box owns the screen while it is up: unload (not just hide) the launcher render camera and
// the backdrop so nothing loaded sits behind the test scene; tear_down restores both. Level-
//triggered (runs in Last, after the frame's command flush) because the box's own spawn is
// deferred — Update-side systems can't see it on its opening frame.
fn apply_test_unload(
    q_box: Query<(), With<TestSceneScoped>>,
    q_launch_cam: Query<Entity, With<super::launcher_ui::LauncherCamera>>,
    q_backdrop: Query<Entity, With<super::launcher_backdrop::BackdropScoped>>,
    q_ui: Query<(Entity, Option<&ChildOf>), (With<Node>, Without<TestSceneScoped>)>,
    mut commands: Commands,
    mut log: ResMut<TestLog>,
    mut was_open: Local<bool>,
) {
    let open = q_box.iter().next().is_some();
    if !open {
        *was_open = false;
        return;
    }
    set_launcher_ui_visibility(&mut commands, &q_ui, false);
    for e in q_launch_cam.iter() {
        commands.entity(e).try_despawn();
    }
    // Idempotent once unloaded: empty scoped query, remove_resource is a silent no-op.
    super::launcher_backdrop::unload_for_test(&mut commands, &q_backdrop);
    if !*was_open {
        log_line(&mut log, "launcher + backdrop unloaded".into());
    }
    *was_open = true;
}

fn set_launcher_ui_visibility(
    commands: &mut Commands,
    q_ui: &Query<(Entity, Option<&ChildOf>), (With<Node>, Without<TestSceneScoped>)>,
    visible: bool,
) {
    for (e, parent) in q_ui.iter() {
        if parent.is_some() {
            continue;
        }
        let vis = if visible {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        commands.entity(e).insert(vis);
    }
}

fn handle_toggle(
    mut pending: ResMut<PendingToggle>,
    mut drawn_check: ResMut<DrawnCheck>,
    q_scoped: Query<Entity, With<TestSceneScoped>>,
    q_ui: Query<(Entity, Option<&ChildOf>), (With<Node>, Without<TestSceneScoped>)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    moon_materials: ResMut<Assets<kuluu_render::moon_material::MoonMaterial>>,
    images: ResMut<Assets<Image>>,
    settings: Res<kuluu_render::graphics_settings::GraphicsSettings>,
    mut load_tx: MessageWriter<LoadActorRequest>,
    mut tracked: ResMut<TrackedEntities>,
    mut scene: ResMut<SceneState>,
    actor_root: Res<ActionDatRoot>,
    mut log: ResMut<TestLog>,
    mut hp: ResMut<TestHp>,
) {
    if !pending.0 {
        return;
    }
    pending.0 = false;

    if q_scoped.iter().next().is_some() {
        tear_down(
            &mut commands,
            &q_scoped,
            &q_ui,
            &mut tracked,
            &mut scene,
            &mut meshes,
            &mut materials,
        );
        // The dispatch funnel's info! traces (routine resolution, particle defs/meshes) are
        // gated on this; the box is where they earn their keep.
        commands.insert_resource(VfxTrace(false));
        log_line(&mut log, "scene down".into());
        return;
    }

    drawn_check.hume_done = false;
    drawn_check.worm_done = false;
    drawn_check.parts_ok = true;
    hp.hume = TEST_MAX_HP;
    hp.worm = TEST_MAX_HP;
    commands.insert_resource(VfxTrace(true));
    activate_test_scene(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut load_tx,
        &mut tracked,
        &mut scene,
        &actor_root,
        &mut log,
        &mut drawn_check,
    );

    // Hide the launcher menu while the box is up; the X button (or leaving the Launcher phase)
    // restores it.
    set_launcher_ui_visibility(&mut commands, &q_ui, false);

    // Last: it consumes the owned params. poll_load_actor_tasks parks tasks without EntityMesh,
    // and the Update chain that resource unlocks (sync_entities_system & co.) reads the world
    // resources setup_world inserts on InGame entry — so run it here: real orb meshes/materials
    // for the wire placeholders, no defaults.
    kuluu_render::setup_world(
        commands,
        meshes,
        materials,
        moon_materials,
        images,
        settings,
    );
}

fn handle_close_press(
    q_close: Query<&Interaction, (With<CloseBox>, With<bevy::ui::widget::Button>)>,
    q_scoped: Query<Entity, With<TestSceneScoped>>,
    q_ui: Query<(Entity, Option<&ChildOf>), (With<Node>, Without<TestSceneScoped>)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut tracked: ResMut<TrackedEntities>,
    mut scene: ResMut<SceneState>,
    mut log: ResMut<TestLog>,
) {
    let Ok(interaction) = q_close.single() else {
        return;
    };
    if !matches!(interaction, Interaction::Pressed) {
        return;
    }
    tear_down(
        &mut commands,
        &q_scoped,
        &q_ui,
        &mut tracked,
        &mut scene,
        &mut meshes,
        &mut materials,
    );
    log_line(&mut log, "scene down".into());
}

fn activate_test_scene(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    load_tx: &mut MessageWriter<LoadActorRequest>,
    tracked: &mut TrackedEntities,
    scene: &mut SceneState,
    actor_root: &ActionDatRoot,
    log: &mut TestLog,
    check: &mut DrawnCheck,
) {
    let Some(dat_root) = actor_root.0.as_ref() else {
        log_line(
            log,
            "no retail install wired - pick one in Settings first".into(),
        );
        return;
    };

    // Camera + light + ground. The camera clears its own color so the launcher backdrop zone
    // does not show around the test floor.
    commands.spawn((
        TestSceneScoped,
        // Marker routes camera-relative systems at this eye: track_weather_particles rewrites
        // camera-anchored origins and select_zone_mmb_lod owns MMB chunk visibility; both
        // early-return without it. No other OperatorCamera exists pre-server (the in-game one
        // spawns OnEnter(InGame)), so the box is the sole marker while open.
        kuluu_render::camera::OperatorCamera,
        Camera3d::default(),
        // Order 3: above the launcher backdrop (-2) and any default-order (gizmo) camera;
        // 1-2 are the in-game nameplate overlay/composite slots.
        Camera {
            order: 3,
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        // Right angle to both actors (worm -X / Hume +X): the box's intended framing;
        // ANIMTEST_CAM re-frames it for zone captures.
        match env_camera_override() {
            Some((pos, target)) => Transform::from_translation(pos).looking_at(target, Vec3::Y),
            None => Transform::from_translation(Vec3::new(0.0, 2.6, 7.5))
                .looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
        },
    ));
    commands.spawn((
        TestSceneScoped,
        DirectionalLight {
            illuminance: 9000.0,
            shadow_maps_enabled: true,
            ..default()
        },
    ));
    commands.insert_resource(bevy::light::GlobalAmbientLight {
        color: Color::srgb(1.0, 1.0, 1.0),
        brightness: 400.0,
        ..default()
    });
    // +Y normal: a +Z plane is a vertical wall at z=0 that hides everything behind it.
    let plane: Mesh = Plane3d::new(Vec3::Y, Vec2::splat(40.0)).into();
    commands.insert_resource(FloorHidden(false));
    commands.spawn((
        TestSceneScoped,
        TestFloor,
        Mesh3d(meshes.add(plane)),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.22, 0.26, 0.22),
            ..default()
        })),
    ));

    // Worm left, sworded Hume right, facing each other. The snapshot entries keep the wires
    // alive: sync_entities_system despawns any tracked wire missing from the snapshot.
    // Heading is the system's orientation source of truth (sync re-derives the wire quat from
    // it on respawn), so both the spawn transform and the heading agree. Both skeletons face
    // local +X at identity (live-checked), so opposite headings square them onto each other:
    // worm 0 faces +X toward the Hume, Hume 128 faces -X back.
    // Both are engaged so they stand in battle stance with weapons out, not rest pose.
    let actor_base = env_actor_pos().unwrap_or(Vec3::ZERO);
    spawn_wire(
        commands,
        tracked,
        scene,
        WORM_ID,
        EntityKind::Mob,
        actor_base + Vec3::new(-1.0, 0.0, 0.0),
        0,
        ffxi_proto::decode::animation::ATTACK,
        HUME_ID,
    );
    spawn_wire(
        commands,
        tracked,
        scene,
        HUME_ID,
        EntityKind::Pc,
        actor_base + Vec3::new(1.0, 0.0, 0.0),
        128,
        ffxi_proto::decode::animation::ATTACK,
        WORM_ID,
    );

    // Production order: the face file first (head/hair), then slots 1..8 — load_pc reads
    // equipment[0] as the head. A weapon in that slot leaves the Hume headless.
    let mut equipment = Vec::new();
    let mut parts: Vec<(&str, u32)> = Vec::new();
    match kuluu_render::look_resolver::resolve_face(0, 1) {
        Some(face_file) => {
            equipment.push(face_file);
            parts.push(("face", face_file));
        }
        None => log_line(
            log,
            "ERROR: face file unresolved - head will not render".into(),
        ),
    }
    // Slots 2..5 come from the race's default equipment table (real geometry); slot 6 is pinned
    // to a real sword because its table row is a bare stub; slot 1 stays empty — the face file
    // carries hair and face, so no headgear.
    const SLOT_NAMES: [&str; 6] = ["head", "body", "hands", "legs", "feet", "sword"];
    for (slot, name) in (1u16..=6).zip(SLOT_NAMES) {
        if slot == 1 {
            continue;
        }
        let file_id = match slot {
            6 => Some(TEST_SWORD_FILE),
            _ => kuluu_render::look_resolver::resolve_equipment_slot(slot << 12, 1),
        };
        match file_id {
            Some(file_id) => {
                equipment.push(file_id);
                parts.push((name, file_id));
            }
            None => log_line(
                log,
                format!("ERROR: slot {slot} ({name}) unresolved - that body part will not render"),
            ),
        }
    }
    load_tx.write(LoadActorRequest {
        entity_id: WORM_ID,
        subject: ActorSubject::Npc {
            file_id: WORM_FILE,
            graph_size: 0,
        },
    });
    load_tx.write(LoadActorRequest {
        entity_id: HUME_ID,
        subject: ActorSubject::Pc {
            race: 1,
            mounted: false,
            equipment,
            body: None,
            main_weapon: Some(TEST_SWORD_FILE),
            sub_weapon: None,
        },
    });

    verify_parts(
        dat_root,
        &parts,
        kuluu_render::dat_vos2::skeleton_file_id_for_race(None, 1),
        WORM_FILE,
        log,
        check,
    );

    spawn_panel(commands);

    // X in the top-right corner closes the box back to the launcher menu.
    commands
        .spawn((
            TestSceneScoped,
            CloseBox,
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(12.0),
                top: Val::Px(8.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.35, 0.12, 0.12, 0.9)),
            bevy::ui::widget::Button,
        ))
        .with_child((
            Text::new("X"),
            TextFont {
                font_size: 14.0.into(),
                ..default()
            },
            TextColor(Color::WHITE),
        ));

    log_line(
        log,
        format!(
            "loaded: worm={WORM_FILE} hume=[{}]",
            parts
                .iter()
                .map(|(name, file_id)| format!("{name}={file_id}"))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    );
}

// Non-empty mesh buffers in a DAT: what load_pc turns into drawn mesh entities. None = unreadable.
fn part_mesh_count(dat_root: &ffxi_dat::DatRoot, file_id: u32) -> Option<usize> {
    let loc = dat_root.resolve(file_id).ok()?;
    let bytes = std::fs::read(loc.path_under(dat_root)).ok()?;
    Some(
        ffxi_dat::resource_dir::ResourceDir::from_bytes(bytes)
            .collect_skel_meshes()
            .iter()
            .flat_map(|m| m.meshes.iter())
            .filter(|b| !b.vertices.is_empty())
            .count(),
    )
}

// Every part file must be readable with mesh buffers, else load_pc drops it and the part
// never renders; also seeds DrawnCheck's expected counts for verify_drawn.
fn verify_parts(
    dat_root: &ffxi_dat::DatRoot,
    parts: &[(&str, u32)],
    skel_file: Option<u32>,
    worm_file: u32,
    log: &mut TestLog,
    check: &mut DrawnCheck,
) {
    let mut ok_bits = Vec::new();
    for (name, file_id) in parts {
        match part_mesh_count(dat_root, *file_id) {
            Some(n) if n > 0 => ok_bits.push(format!("{name}={file_id}({n})")),
            _ => {
                check.parts_ok = false;
                log_line(
                    log,
                    format!(
                        "ERROR: {name}={file_id} unreadable or 0 mesh buffers - will not render"
                    ),
                );
            }
        }
    }
    if !ok_bits.is_empty() {
        log_line(log, format!("check ok: {}", ok_bits.join(" ")));
    }

    check.exp_hume = skel_file
        .and_then(|f| part_mesh_count(dat_root, f))
        .unwrap_or(0)
        + parts
            .iter()
            .map(|(_, file_id)| part_mesh_count(dat_root, *file_id).unwrap_or(0))
            .sum::<usize>();
    check.exp_worm = part_mesh_count(dat_root, worm_file).unwrap_or(0);
    if check.exp_worm == 0 {
        log_line(
            log,
            format!("ERROR: worm={worm_file} unreadable or 0 mesh buffers - will not render"),
        );
    }
}

fn verify_drawn(
    mut check: ResMut<DrawnCheck>,
    tracked: Res<TrackedEntities>,
    q_root: Query<&FfxiRenderRoot>,
    q_children: Query<&Children>,
    q_mesh: Query<Entity, With<FfxiActorMeshChild>>,
    mut log: ResMut<TestLog>,
) {
    let count_drawn = |wire: Entity| -> Option<usize> {
        let root = q_root.get(wire).ok()?.0;
        let children = q_children.get(root).ok()?;
        Some(children.iter().filter(|&c| q_mesh.get(c).is_ok()).count())
    };

    if !check.hume_done {
        if let Some(hume) = tracked.by_id.get(&HUME_ID).copied() {
            if let Some(n) = count_drawn(hume) {
                check.hume_done = true;
                if n == 0 {
                    log_line(&mut log, "ERROR: hume drew no mesh parts".into());
                } else if n < check.exp_hume {
                    let exp = check.exp_hume;
                    log_line(
                        &mut log,
                        format!("ERROR: hume drew {n} of {exp} expected mesh parts"),
                    );
                } else if check.parts_ok {
                    log_line(
                        &mut log,
                        format!(
                            "drawn on player: {n} mesh parts (all checked parts, sword included)"
                        ),
                    );
                } else {
                    log_line(&mut log, format!("drawn on player: {n} mesh parts"));
                }
            }
        }
    }

    if !check.worm_done {
        if let Some(worm) = tracked.by_id.get(&WORM_ID).copied() {
            if let Some(n) = count_drawn(worm) {
                check.worm_done = true;
                if n == 0 {
                    log_line(&mut log, "ERROR: worm drew no mesh parts".into());
                } else {
                    log_line(&mut log, format!("drawn: worm {n} mesh parts"));
                }
            }
        }
    }
}

fn spawn_wire(
    commands: &mut Commands,
    tracked: &mut TrackedEntities,
    scene: &mut SceneState,
    id: u32,
    kind: EntityKind,
    pos: Vec3,
    heading: u8,
    animation: u8,
    bt_target_id: u32,
) {
    // The snapshot pos is FFXI space and the prediction tween pulls wires toward it, so it must
    // agree with the bevy-space wire transform (inverse of ffxi_to_bevy).
    let wire_pos = kuluu_snapshot::Vec3 {
        x: pos.x,
        y: -pos.z,
        z: -pos.y,
    };
    // Same formula as scene.rs heading_to_quat, so a sync respawn cannot re-orient the wire.
    let rot = Quat::from_rotation_y(-(heading as f32) * std::f32::consts::TAU / 256.0);
    // Visibility on the wire (as in sync_entities_system's spawn): Bevy then attaches
    // InheritedVisibility to the parent, so the model children don't trip B0004.
    let parent = commands
        .spawn((
            TestSceneScoped,
            WorldEntity {
                id,
                act_index: 0,
                kind,
            },
            Transform::from_translation(pos).with_rotation(rot),
            Visibility::default(),
        ))
        .id();
    tracked.by_id.insert(id, parent);
    scene.snapshot.entities.push(kuluu_snapshot::Entity {
        id,
        act_index: 0,
        kind,
        name: None,
        pos: wire_pos,
        heading,
        hp_pct: Some(100),
        bt_target_id,
        face_target: 0,
        claim_id: 0,
        speed: 0,
        speed_base: 0,
        look: None,
        animation,
        animationsub: 0,
        mount: None,
        status: 0,
        char_flags: Default::default(),
        monstrosity: false,
        name_vis: None,
    });
}

fn spawn_panel(commands: &mut Commands) {
    let panel = commands
        .spawn((
            TestSceneScoped,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Px(215.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(8.0)),
                column_gap: Val::Px(6.0),
                // Rows never crush under log growth; extra log lines clip at the panel floor.
                overflow: bevy::ui::Overflow::clip(),
                ..default()
            },
            BackgroundColor(Color::srgba(0.04, 0.05, 0.09, 0.92)),
            GlobalZIndex(6),
        ))
        .id();

    for case in [
        Case::PlayerNhIt,
        Case::PlayerChit,
        Case::PlayerDhit,
        Case::MobNhIt,
        Case::MobChit,
        Case::MobRespawn,
        Case::LevelUp,
        Case::Gen141,
        Case::Gen144,
        Case::Hit1Full,
        Case::Hi26,
        Case::Sb00,
        Case::I900,
        Case::LoadZone,
        Case::LoadWeather,
        Case::SgLamps,
    ] {
        let button = commands
            .spawn((
                TestSceneScoped,
                Node {
                    width: Val::Percent(100.0),
                    padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                    // Fixed-height panel + default flex_shrink=1 let a long log crush every
                    // row toward its content-min; the slider track's min is zero, so it vanished
                    // first (user repro: shadows-checkbox click). Controls never shrink.
                    flex_shrink: 0.0,
                    ..default()
                },
                BackgroundColor(Color::srgb(0.16, 0.2, 0.3)),
                bevy::ui::widget::Button,
                CaseButton(case),
            ))
            .with_child((
                Text::new(case.label()),
                TextFont {
                    font_size: 13.0.into(),
                    ..default()
                },
                TextColor(Color::WHITE),
            ))
            .id();
        commands.entity(panel).add_child(button);
    }

    // Lantern-alpha slider row: label + track + knob. Seed both drawn pieces from the renderer's
    // default lift so panel and shader agree before anyone touches it.
    let seed_lift = kuluu_render::particle_sim::LAMP_ALPHAMAP_LIFT_DEFAULT;
    let label = commands
        .spawn((
            TestSceneScoped,
            LampSliderLabel,
            Node {
                flex_shrink: 0.0,
                ..default()
            },
            Text::new(format!("lantern alpha {seed_lift:.2}")),
            TextFont {
                font_size: 12.0.into(),
                ..default()
            },
            TextColor(Color::srgb(0.85, 0.8, 0.65)),
        ))
        .id();
    commands.entity(panel).add_child(label);
    let knob = commands
        .spawn((
            TestSceneScoped,
            LampSliderKnob,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(seed_lift * (LAMP_SLIDER_TRACK_W - LAMP_SLIDER_KNOB_W)),
                top: Val::Px(2.0),
                width: Val::Px(LAMP_SLIDER_KNOB_W),
                height: Val::Px(LAMP_SLIDER_TRACK_H - 4.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.85, 0.72, 0.35)),
        ))
        .id();
    let track = commands
        .spawn((
            TestSceneScoped,
            LampSliderTrack,
            Node {
                width: Val::Px(LAMP_SLIDER_TRACK_W),
                height: Val::Px(LAMP_SLIDER_TRACK_H),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(Color::srgb(0.13, 0.15, 0.2)),
            bevy::ui::widget::Button,
        ))
        .id();
    commands.entity(track).add_child(knob);
    commands.entity(panel).add_child(track);

    // Wall-wash alpha slider row (under the lantern one): label + track + knob, seeded at
    // WASH_ALPHA_LIFT_DEFAULT.
    let seed_wash = kuluu_render::particle_sim::WASH_ALPHA_LIFT_DEFAULT;
    let wash_label = commands
        .spawn((
            TestSceneScoped,
            WashSliderLabel,
            Node {
                flex_shrink: 0.0,
                ..default()
            },
            Text::new(format!("wash alpha {seed_wash:.2}")),
            TextFont {
                font_size: 12.0.into(),
                ..default()
            },
            TextColor(Color::srgb(0.85, 0.8, 0.65)),
        ))
        .id();
    commands.entity(panel).add_child(wash_label);
    let wash_knob = commands
        .spawn((
            TestSceneScoped,
            WashSliderKnob,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(
                    (seed_wash / kuluu_render::particle_sim::WASH_ALPHA_LIFT_MAX)
                        * (LAMP_SLIDER_TRACK_W - LAMP_SLIDER_KNOB_W),
                ),
                top: Val::Px(2.0),
                width: Val::Px(LAMP_SLIDER_KNOB_W),
                height: Val::Px(LAMP_SLIDER_TRACK_H - 4.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.85, 0.72, 0.35)),
        ))
        .id();
    let wash_track = commands
        .spawn((
            TestSceneScoped,
            WashSliderTrack,
            Node {
                width: Val::Px(LAMP_SLIDER_TRACK_W),
                height: Val::Px(LAMP_SLIDER_TRACK_H),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(Color::srgb(0.13, 0.15, 0.2)),
            bevy::ui::widget::Button,
        ))
        .id();
    commands.entity(wash_track).add_child(wash_knob);
    commands.entity(panel).add_child(wash_track);

    // Shadows row: filled = on; unchecked suppresses the Enhanced dynamic-lights half of the
    // user's graphics settings (Bevy PointLight lamp shadows + flicker, volumetric fog density).
    let shadows_checkbox = commands
        .spawn((
            TestSceneScoped,
            ShadowsCheckbox,
            Node {
                width: Val::Percent(100.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(Color::srgb(0.16, 0.2, 0.3)),
            bevy::ui::widget::Button,
            Text::new("[x] shadows"),
            TextFont {
                font_size: 13.0.into(),
                ..default()
            },
            TextColor(Color::WHITE),
        ))
        .id();
    commands.entity(panel).add_child(shadows_checkbox);

    // Night-fx rows (moon/fog first — the ones the lamp-room walls needed): filled = on,
    // unchecked suppresses.
    for field in [
        SkyFxField::Moon,
        SkyFxField::Fog,
        SkyFxField::Sun,
        SkyFxField::Stars,
    ] {
        let row = commands
            .spawn((
                TestSceneScoped,
                Node {
                    width: Val::Percent(100.0),
                    padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                    flex_shrink: 0.0,
                    ..default()
                },
                BackgroundColor(Color::srgb(0.16, 0.2, 0.3)),
                bevy::ui::widget::Button,
                Text::new(format!("[x] {}", field.label())),
                TextFont {
                    font_size: 13.0.into(),
                    ..default()
                },
                TextColor(Color::WHITE),
                SkyCheckbox(field),
            ))
            .id();
        commands.entity(panel).add_child(row);
    }

    // Lamps row: filled = halos drawn; unchecked hides them live.
    let lamps_checkbox = commands
        .spawn((
            TestSceneScoped,
            Node {
                width: Val::Percent(100.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(Color::srgb(0.16, 0.2, 0.3)),
            bevy::ui::widget::Button,
            Text::new("[x] lamps"),
            TextFont {
                font_size: 13.0.into(),
                ..default()
            },
            TextColor(Color::WHITE),
            LampsCheckbox,
        ))
        .id();
    commands.entity(panel).add_child(lamps_checkbox);

    // Wall-glow row: filled = the ghu*/li* wash volumes draw (authored additive look); unchecked
    // hides them. The wash-alpha slider scales their brightness.
    let wall_glow_checkbox = commands
        .spawn((
            TestSceneScoped,
            Node {
                width: Val::Percent(100.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(Color::srgb(0.16, 0.2, 0.3)),
            bevy::ui::widget::Button,
            Text::new("[x] wall glow"),
            TextFont {
                font_size: 13.0.into(),
                ..default()
            },
            TextColor(Color::WHITE),
            WallGlowCheckbox,
        ))
        .id();
    commands.entity(panel).add_child(wall_glow_checkbox);

    let log_node = commands
        .spawn((
            TestSceneScoped,
            Node {
                // Absorbs the panel's leftover space (and any overflow) so no log line ever
                // squeezes the control rows above it.
                flex_grow: 1.0,
                min_height: Val::Px(0.0),
                width: Val::Percent(100.0),
                align_items: AlignItems::FlexStart,
                ..default()
            },
        ))
        .with_child((
            Text::new("(log)"),
            TextFont {
                font_size: 10.5.into(),
                ..default()
            },
            TextColor(Color::srgb(0.75, 0.85, 0.75)),
            LogText,
        ))
        .id();
    commands.entity(panel).add_child(log_node);
}

const NORMAL_HIT_WATCH_SECS: f64 = 1.2;
const CRITICAL_HIT_WATCH_SECS: f64 = 1.5;
const DEATH_HIT_WATCH_SECS: f64 = 2.0;
const LEVEL_UP_WATCH_SECS: f64 = 3.5;
const RESPAWN_WATCH_SECS: f64 = 0.3;
const GENERATOR_WATCH_SECS: f64 = 2.0;
const CHILD_EFFECT_WATCH_SECS: f64 = 2.5;
const ZONE_LOAD_WATCH_SECS: f64 = 2.0;
const WEATHER_LOAD_WATCH_SECS: f64 = 0.5;
const LAMP_ROOM_WATCH_SECS: f64 = 4.0;
const SCREENSHOT_WATCH_SECS: f64 = 1.5;

fn case_duration(case: Case) -> Duration {
    let seconds = match case {
        Case::PlayerNhIt | Case::MobNhIt => NORMAL_HIT_WATCH_SECS,
        Case::PlayerChit | Case::MobChit => CRITICAL_HIT_WATCH_SECS,
        Case::PlayerDhit => DEATH_HIT_WATCH_SECS,
        Case::LevelUp => LEVEL_UP_WATCH_SECS,
        Case::MobRespawn => RESPAWN_WATCH_SECS,
        Case::Gen141 | Case::Gen144 => GENERATOR_WATCH_SECS,
        Case::Hit1Full | Case::Hi26 | Case::Sb00 | Case::I900 => CHILD_EFFECT_WATCH_SECS,
        Case::LoadZone => ZONE_LOAD_WATCH_SECS,
        Case::LoadWeather => WEATHER_LOAD_WATCH_SECS,
        Case::SgLamps => LAMP_ROOM_WATCH_SECS,
        Case::Shot => SCREENSHOT_WATCH_SECS,
    };
    Duration::from_secs_f64(seconds)
}

// Pop the ANIMTEST_AUTO queue on schedule; run_pending_case consumes it the same frame.
fn auto_fire_cases(
    mut auto: ResMut<AutoFire>,
    mut pending: ResMut<PendingCase>,
    mut log: ResMut<TestLog>,
) {
    let now = Instant::now();
    if now < auto.next_at.unwrap_or_else(Instant::now) {
        return;
    }
    let Some(case) = auto.queue.pop_front() else {
        return;
    };
    auto.next_at = Some(now + Duration::from_secs_f32(AUTO_SPACING_SECS));
    log_line(&mut log, format!("AUTO fired {}", case.label()));
    pending.0 = Some(case);
}

// The box's actors must survive every case; a silent despawn (zone load, sync sweep) is the
// class of bug that leaves an empty scene with no log line to explain it.
fn watch_wires(
    tracked: Res<TrackedEntities>,
    mut last: Local<Option<(bool, bool)>>,
    mut log: ResMut<TestLog>,
) {
    let now = (
        tracked.by_id.contains_key(&WORM_ID),
        tracked.by_id.contains_key(&HUME_ID),
    );
    if let Some(prev) = *last {
        if prev.0 && !now.0 {
            log_line(&mut log, "wire lost: worm".into());
        }
        if prev.1 && !now.1 {
            log_line(&mut log, "wire lost: hume".into());
        }
    }
    *last = Some(now);
}

fn handle_case_presses(
    q_buttons: Query<(&CaseButton, &Interaction)>,
    mut pending: ResMut<PendingCase>,
    lock: Res<CaseLock>,
) {
    for (case_button, interaction) in q_buttons.iter() {
        if !matches!(interaction, Interaction::Pressed) {
            continue;
        }
        // Bevy holds Pressed for the whole mouse-down, so a locked press is ignored silently
        // (the buttons grey out while the case window runs) instead of logging per frame.
        if lock.until.is_some_and(|until| Instant::now() < until) {
            continue;
        }
        pending.0 = Some(case_button.0);
    }
}

fn collect_spawn_traces(
    mut traces: MessageReader<ParticleSpawnTrace>,
    q_scoped: Query<Entity, With<TestSceneScoped>>,
    mut log: ResMut<TestLog>,
) {
    if q_scoped.iter().next().is_none() {
        return;
    }
    for t in traces.read() {
        log_line(&mut log, t.0.clone());
    }
}

// Grey the case buttons out while a case window runs; restore them when it elapses.
fn sync_case_buttons(
    lock: Res<CaseLock>,
    mut q_btns: Query<(&mut BackgroundColor, &Children), With<CaseButton>>,
    mut q_text: Query<&mut TextColor>,
) {
    let locked = lock.until.is_some_and(|until| Instant::now() < until);
    for (mut bg, children) in &mut q_btns {
        *bg = if locked {
            BackgroundColor(Color::srgb(0.10, 0.12, 0.16))
        } else {
            BackgroundColor(Color::srgb(0.16, 0.2, 0.3))
        };
        for child in children {
            if let Ok(mut tc) = q_text.get_mut(*child) {
                *tc = TextColor(if locked {
                    Color::srgb(0.45, 0.48, 0.55)
                } else {
                    Color::WHITE
                });
            }
        }
    }
}

fn run_pending_case(
    mut pending: ResMut<PendingCase>,
    mut log: ResMut<TestLog>,
    tracked: Res<TrackedEntities>,
    q_children: Query<&Children>,
    q_render: Query<&FfxiRenderActor>,
    global: Option<Res<GlobalEffectDir>>,
    root: Res<ActionDatRoot>,
    mut worm_state: ResMut<WormState>,
    mut scene: ResMut<SceneState>,
    mut zone: ZoneLoadParams,
    q_root: Query<&kuluu_render::ffxi_actor_render::FfxiRenderRoot>,
    mut q_vis: Query<&mut Visibility>,
    mut commands: Commands,
    mut events: ResMut<EventLog>,
    mut hp: ResMut<TestHp>,
    mut lock: ResMut<CaseLock>,
) {
    let Some(case) = pending.0.take() else {
        return;
    };
    log_line(&mut log, format!("case: {}", case.label()));

    // Reset the tester alpha override each case so a prior g141/g144 inspection doesn't leak.
    commands.insert_resource(kuluu_render::particle_sim::TestAlphaOverride(
        Default::default(),
    ));

    // A dead worm can neither swing nor react: bring it back before the hit so the impact
    // lands on a live model.
    if matches!(
        case,
        Case::PlayerNhIt | Case::PlayerChit | Case::PlayerDhit | Case::MobNhIt | Case::MobChit
    ) && hp.worm == 0
    {
        log_line(&mut log, "worm dead - respawning before hit".into());
        respawn_worm(
            &mut worm_state,
            &mut scene,
            &mut log,
            &tracked,
            &q_root,
            &mut q_vis,
            &q_children,
            &mut commands,
            &mut hp,
        );
    }

    match case {
        Case::MobRespawn => respawn_worm(
            &mut worm_state,
            &mut scene,
            &mut log,
            &tracked,
            &q_root,
            &mut q_vis,
            &q_children,
            &mut commands,
            &mut hp,
        ),
        Case::LevelUp => fire_level_up(
            &root,
            &tracked,
            &q_children,
            &q_render,
            global.as_deref(),
            &mut log,
            &mut commands,
        ),
        Case::Gen141 => fire_single_gen(
            &tracked,
            global.as_deref(),
            *b"g141",
            &mut log,
            &mut commands,
        ),
        Case::Gen144 => fire_single_gen(
            &tracked,
            global.as_deref(),
            *b"g144",
            &mut log,
            &mut commands,
        ),
        Case::Hit1Full => fire_hit1_full(&tracked, global.as_deref(), &mut log, &mut commands),
        Case::Hi26 => fire_named_routine(
            &tracked,
            global.as_deref(),
            b"hi26",
            &mut log,
            &mut commands,
        ),
        Case::Sb00 => fire_named_routine(
            &tracked,
            global.as_deref(),
            b"sb00",
            &mut log,
            &mut commands,
        ),
        Case::I900 => fire_zone_i900(&tracked, root.0.clone(), &mut log, &mut commands),
        Case::SgLamps => {
            // Item one of the lamp-room setup: lantern rays off (the panel checkbox shows checked).
            *zone.lamp_rays = LampRaysOff(true);
            load_sg_lamp_room(&mut zone, &mut scene, &mut log, &mut commands);
        }
        Case::LoadZone => {
            let (zone_id, mzb_file, world_pos) = env_zone_override().unwrap_or((
                WEST_RONFAURE_ZONE_ID,
                WEST_RONFAURE_MZB_FILE_ID,
                WR_ENTRY_OFFSET,
            ));
            if zone.last_zone.file_id == Some(mzb_file) {
                log_line(&mut log, "zone: already loaded".into());
            } else {
                // Drive the zone through the backdrop resource so
                // mirror_backdrop_to_scene_state keeps snapshot.zone_id in agreement. The
                // snapshot write is atomic with the pre-stamp: auto_load_zone_geometry_system
                // compares effective_zone_file_id(snapshot) against LastAutoLoadedZone, and a
                // frame gap between the two re-issues the block at a ZERO offset (which would
                // stand us 50+ units off the terrain).
                *zone.backdrop_zone = super::launcher_backdrop::LauncherBackdropZone(zone_id);
                scene.snapshot.zone_id = Some(zone_id);
                zone.last_zone.file_id = Some(mzb_file);
                zone.load_tx.write(LoadMzbRequest {
                    file_id: mzb_file,
                    chunk_idx: None,
                    world_pos,
                    auto_loaded: true,
                    slot: ZONE_SLOT_MAIN,
                    active_sub_area: None,
                });
                log_line(
                    &mut log,
                    format!("zone: id {zone_id} (mzb {mzb_file}) loading at offset {world_pos:?}"),
                );
            }
            if let Some(hour) = env_hour_override() {
                commands
                    .insert_resource(kuluu_render::vana_time::VanaClock::anchored_at_hour(hour));
                log_line(&mut log, format!("clock: pinned to hour {hour}"));
            }
            zone.floor_hidden.0 = true;
            commands.insert_resource(TestZoneActive(true));
        }
        Case::LoadWeather => {
            // sync_current_weather_from_snapshot copies this into CurrentWeather every frame
            // (myroom is None pre-server); sample_zone_weather then selects weat/clod for 200.
            scene.snapshot.weather = Some(kuluu_snapshot::Weather::Clouds);
            log_line(&mut log, "weather: clouds set (weat/clod)".into());
        }
        Case::Shot => {
            let path = std::env::var("ANIMTEST_SHOT_PATH")
                .ok()
                .map(std::path::PathBuf::from);
            log_line(
                &mut log,
                format!(
                    "shot: {}",
                    path.clone()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "default screenshot-<n>.png".into())
                ),
            );
            commands.insert_resource(PendingShot(path));
        }
        _ => fire_hit(case, &tracked, &mut log, &mut events, &mut scene, &mut hp),
    }

    lock.until = Some(Instant::now() + case_duration(case));
}

fn actor_routines(
    entity: Entity,
    q_children: &Query<&Children>,
    q_render: &Query<&FfxiRenderActor>,
) -> Option<std::collections::HashMap<ffxi_dat::datid::DatId, ffxi_dat::scheduler::Scheduler>> {
    q_children
        .get(entity)
        .ok()?
        .iter()
        .find_map(|child| q_render.get(child).ok())
        .map(|a| a.routines().clone())
}

// The button is the server: one BATTLE2-shaped ActionStarted into the EventLog. Production
// reacts from there — dispatch_action_overlay plays the attacker's swing clip (ati0), and
// dispatch_melee_action_started enqueues the effects and arms the victim reaction that the
// swing's DamageCallback fires at its impact frame.
fn fire_hit(
    case: Case,
    tracked: &TrackedEntities,
    log: &mut TestLog,
    events: &mut EventLog,
    scene: &mut SceneState,
    hp: &mut TestHp,
) {
    let (attacker_id, victim_id) = match case {
        Case::PlayerNhIt | Case::PlayerChit | Case::PlayerDhit => (HUME_ID, WORM_ID),
        _ => (WORM_ID, HUME_ID),
    };
    if !tracked.by_id.contains_key(&attacker_id) || !tracked.by_id.contains_key(&victim_id) {
        log_line(log, "actors not loaded yet - try again in a second".into());
        return;
    }
    events.push(kuluu_snapshot::ViewerEvent::ActionStarted {
        actor_id: attacker_id,
        action_id: u32::from_le_bytes(*b"atk0"),
        action_kind: ffxi_proto::melee::CATEGORY_BASIC_ATTACK,
        target_id: Some(victim_id),
        result: Some((0, 0)), // resolution Hit, animation RightAttack
        animation: Some(0),
        outcome: Some((case.info_bits(), 0, 0)),
    });
    // The 0x0E ships in the same flush as BATTLE2: apply the damage to the tracked HP and
    // publish the percentage, so death rides on wire state like real combat.
    let victim_hp = if victim_id == WORM_ID {
        &mut hp.worm
    } else {
        &mut hp.hume
    };
    *victim_hp = victim_hp.saturating_sub(case.damage());
    let pct = ((*victim_hp as u128) * 100 / TEST_MAX_HP as u128).min(100) as u8;
    if let Some(e) = scene
        .snapshot
        .entities
        .iter_mut()
        .find(|e| e.id == victim_id)
    {
        e.hp_pct = Some(pct);
    }
    log_line(
        log,
        format!(
            "action started: {} -> {} ({}): hp {}",
            if attacker_id == HUME_ID {
                "hume"
            } else {
                "worm"
            },
            if victim_id == WORM_ID { "worm" } else { "hume" },
            case.label(),
            *victim_hp
        ),
    );
}

fn respawn_worm(
    worm_state: &mut WormState,
    scene: &mut SceneState,
    log: &mut TestLog,
    tracked: &TrackedEntities,
    q_root: &Query<&kuluu_render::ffxi_actor_render::FfxiRenderRoot>,
    q_vis: &mut Query<&mut Visibility>,
    q_children: &Query<&Children>,
    commands: &mut Commands,
    hp: &mut TestHp,
) {
    let Some(worm) = tracked.by_id.get(&WORM_ID).copied() else {
        return;
    };
    if let Ok(root) = q_root.get(worm) {
        if let Ok(mut vis) = q_vis.get_mut(root.0) {
            *vis = Visibility::Visible;
        }
    }
    // Cancel the death path so the pose falls back to idle: drop the Defeated latch and any
    // running `dead` scheduler (its cor0 hold is what keeps a respawned worm on the ground).
    if let Ok(children) = q_children.get(worm) {
        for child in children {
            commands
                .entity(*child)
                .remove::<kuluu_render::scheduler_runtime::DeadFromAction>();
            commands
                .entity(*child)
                .remove::<kuluu_render::scheduler_runtime::ActiveSchedulers>();
        }
    }
    worm_state.dead_at = None;
    hp.worm = TEST_MAX_HP;
    if let Some(e) = scene.snapshot.entities.iter_mut().find(|e| e.id == WORM_ID) {
        e.hp_pct = Some(100);
    }
    log_line(log, "worm respawned (visible again, hp 100)".into());
}

fn fire_level_up(
    root: &ActionDatRoot,
    tracked: &TrackedEntities,
    q_children: &Query<&Children>,
    q_render: &Query<&FfxiRenderActor>,
    global: Option<&GlobalEffectDir>,
    log: &mut TestLog,
    commands: &mut Commands,
) {
    let Some(hume) = tracked.by_id.get(&HUME_ID).copied() else {
        log_line(log, "hume not loaded yet".into());
        return;
    };
    let Some(dat_root) = root.0.as_ref() else {
        log_line(log, "no install wired".into());
        return;
    };
    let Ok(loc) = dat_root.resolve(LEVEL_UP_EFFECT_DAT_ID) else {
        log_line(
            log,
            format!("level-up effect DAT {LEVEL_UP_EFFECT_DAT_ID} not found in the install"),
        );
        return;
    };
    let Ok(bytes) = std::fs::read(loc.path_under(dat_root)) else {
        log_line(log, "failed to read the level-up effect DAT".into());
        return;
    };
    let (schedulers, _assets, _cameras) =
        kuluu_render::scheduler_runtime::parse_action_bytes(&bytes);
    log_line(
        log,
        format!(
            "level-up effect DAT file {LEVEL_UP_EFFECT_DAT_ID}: {} routines",
            schedulers.len()
        ),
    );
    let hume_routines = actor_routines(hume, q_children, q_render);
    let mut lookup = RoutineLookup::new().with_dat(&schedulers);
    if let Some(r) = &hume_routines {
        lookup = lookup.with_actor(r);
    }
    if let Some(g) = global {
        lookup = lookup.with_dat(&g.schedulers);
    }
    match ActiveScheduler::from_routine(&lookup, b"main") {
        Some(active) => {
            log_line(
                log,
                format!("lvup main on hume: {}", stage_summary(&active)),
            );
            enqueue_routine(commands, hume, active);
        }
        None => log_line(log, "lvup `main` UNRESOLVED".into()),
    }
}

// Tester-only: spawn a single named generator on the worm (as target) with its alpha forced to 1,
// so an a=0 additive flash can be seen in isolation. The def is resolved from the global effect
// dir at spawn time (spawn_particle_generators), independent of this synthetic one-stage routine.
fn fire_single_gen(
    tracked: &TrackedEntities,
    _global: Option<&GlobalEffectDir>,
    gen: [u8; 4],
    log: &mut TestLog,
    commands: &mut Commands,
) {
    let Some(worm) = tracked.by_id.get(&WORM_ID).copied() else {
        log_line(log, "worm not loaded yet".into());
        return;
    };
    // Same production semantics as fire_named_routine: the generator sits where it does in
    // hi14/hit1 — on the victim with target = attacker. insert (not try_insert): a stale
    // target from an earlier case must not win first-writer.
    if let Some(hume) = tracked.by_id.get(&HUME_ID).copied() {
        commands.entity(worm).insert(ActionTarget(Some(hume)));
    }
    let stage = ffxi_dat::scheduler::SchedulerStage {
        kind: ffxi_dat::scheduler::StageKind::Particle,
        raw_type: 0x02, // SpawnGenerator
        stage_words: ffxi_dat::scheduler::SYNTHESIZED_STAGE_WORDS,
        delay_frames: 0,
        duration_frames: 0, // one burst; the particle lives out its own max_life
        id: gen,
        max_loops: 0,
        transition_in: 0,
        transition_out: 0,
        model_transform: None,
        follow_points: None,
        screen_color: None,
        actor_fade: None,
        idle_transition_time: None,
        flinch_duration: None,
        model_visibility: None,
        spell_effect: None,
        random_group: None,
        sound_range: None,
        control_flow: None,
        local_dir: ffxi_dat::scheduler::NO_LOCAL_DIR,
    };
    let sched = ffxi_dat::scheduler::Scheduler {
        name: *b"tst1",
        stages: vec![ffxi_dat::scheduler::TimedStage { frame: 0, stage }],
    };
    let scheds = [sched];
    let lookup = RoutineLookup::new().with_dat(&scheds);
    match ActiveScheduler::from_routine(&lookup, b"tst1") {
        Some(active) => {
            commands.insert_resource(kuluu_render::particle_sim::TestAlphaOverride(
                std::collections::HashSet::from([gen]),
            ));
            log_line(
                log,
                format!(
                    "{} on worm (alpha 1): {}",
                    String::from_utf8_lossy(&gen),
                    stage_summary(&active)
                ),
            );
            enqueue_routine(commands, worm, active);
        }
        None => log_line(log, "single-gen routine UNRESOLVED".into()),
    }
}

// Tester-only: play the full ROM/0/0.DAT hit1 routine on the worm (as target) with g141/g144's
// alpha forced to 1 so their a=0 additive flashes are visible alongside the rest of the chain.
fn fire_hit1_full(
    tracked: &TrackedEntities,
    global: Option<&GlobalEffectDir>,
    log: &mut TestLog,
    commands: &mut Commands,
) {
    let Some(worm) = tracked.by_id.get(&WORM_ID).copied() else {
        log_line(log, "worm not loaded yet".into());
        return;
    };
    // Production semantics: hit1 runs on the victim with target = attacker (see
    // fire_named_routine); insert so a stale target from an earlier case cannot win.
    if let Some(hume) = tracked.by_id.get(&HUME_ID).copied() {
        commands.entity(worm).insert(ActionTarget(Some(hume)));
    }
    let Some(g) = global else {
        log_line(log, "no global effect dir wired".into());
        return;
    };
    let lookup = RoutineLookup::new().with_dat(&g.schedulers);
    match ActiveScheduler::from_routine(&lookup, b"hit1") {
        Some(active) => {
            commands.insert_resource(kuluu_render::particle_sim::TestAlphaOverride(
                std::collections::HashSet::from([*b"g141", *b"g144"]),
            ));
            log_line(
                log,
                format!(
                    "hit1 full on worm (g141/g144 alpha 1): {}",
                    stage_summary(&active)
                ),
            );
            enqueue_routine(commands, worm, active);
        }
        None => log_line(log, "hit1 UNRESOLVED in the global effect dir".into()),
    }
}

// Tester-only: play a named ROM/0/0.DAT routine on the worm (as target), no alpha override.
fn fire_named_routine(
    tracked: &TrackedEntities,
    global: Option<&GlobalEffectDir>,
    routine: &[u8; 4],
    log: &mut TestLog,
    commands: &mut Commands,
) {
    let Some(worm) = tracked.by_id.get(&WORM_ID).copied() else {
        log_line(log, "worm not loaded yet".into());
        return;
    };
    // Production semantics: the routine runs on the victim with ActionTarget = attacker, so
    // target-facing generators place off the Hume like a real hit instead of at the worm's feet.
    // insert (not try_insert): a stale target from an earlier case must not win first-writer.
    if let Some(hume) = tracked.by_id.get(&HUME_ID).copied() {
        commands.entity(worm).insert(ActionTarget(Some(hume)));
    }
    let Some(g) = global else {
        log_line(log, "no global effect dir wired".into());
        return;
    };
    let lookup = RoutineLookup::new().with_dat(&g.schedulers);
    match ActiveScheduler::from_routine(&lookup, routine) {
        Some(active) => {
            log_line(
                log,
                format!(
                    "{} on worm: {}",
                    String::from_utf8_lossy(routine),
                    stage_summary(&active)
                ),
            );
            enqueue_routine(commands, worm, active);
        }
        None => log_line(
            log,
            format!(
                "{} UNRESOLVED in the global effect dir",
                String::from_utf8_lossy(routine)
            ),
        ),
    }
}

// i900 (West Ronfaure fefr/fefs) binds ai90 — a Distortion generator — as its sec2 0x44 child,
// and retail triggers it from server events; no zone routine names it. The test synthesizes the
// one Particle stage that would name i900 and runs it on a proxy actor carrying the zone file's
// ActionAssets: production dispatch resolves ai90 from the local tier and arms the haze child
// through the same entry point as any other 0x02 stage.
fn fire_zone_i900(
    tracked: &TrackedEntities,
    dat_root: Option<Arc<ffxi_dat::DatRoot>>,
    log: &mut TestLog,
    commands: &mut Commands,
) {
    let Some(worm) = tracked.by_id.get(&WORM_ID).copied() else {
        log_line(log, "worm not loaded yet".into());
        return;
    };
    let hume = tracked.by_id.get(&HUME_ID).copied();
    commands.entity(worm).insert(ActionTarget(hume));
    let Some(root) = dat_root else {
        log_line(log, "no DAT root wired".into());
        return;
    };
    let Ok(loc) = root.resolve(WEST_RONFAURE_MZB_FILE_ID) else {
        log_line(
            log,
            format!("zone mzb {} not found", WEST_RONFAURE_MZB_FILE_ID),
        );
        return;
    };
    let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
        log_line(
            log,
            format!("zone mzb {} unreadable", WEST_RONFAURE_MZB_FILE_ID),
        );
        return;
    };
    let (_schedulers, assets, _report, _cameras) = parse_action_bytes_reporting(&bytes);
    // i900's authored life (90 frames in the DAT) is the stage window.
    const I900_LIFE_FRAMES: u16 = 90;
    let routine = ffxi_dat::scheduler::Scheduler {
        name: *b"tst1",
        stages: vec![ffxi_dat::scheduler::TimedStage {
            frame: 0,
            stage: ffxi_dat::scheduler::SchedulerStage {
                kind: ffxi_dat::scheduler::StageKind::Particle,
                raw_type: 0x02,
                stage_words: ffxi_dat::scheduler::SYNTHESIZED_STAGE_WORDS,
                delay_frames: 0,
                duration_frames: I900_LIFE_FRAMES,
                id: *b"i900",
                max_loops: 0,
                transition_in: 0,
                transition_out: 0,
                model_transform: None,
                follow_points: None,
                screen_color: None,
                actor_fade: None,
                idle_transition_time: None,
                flinch_duration: None,
                model_visibility: None,
                spell_effect: None,
                random_group: None,
                sound_range: None,
                control_flow: None,
                local_dir: *b"fefs",
            },
        }],
    };
    let active = ActiveScheduler::from_scheduler(&routine);
    // The worm stands at its spawn point; the proxy shares that spot so the effect lands on it.
    const WORM_SPAWN_POS: Vec3 = Vec3::new(-1.0, 0.0, 0.0);
    let proxy = commands
        .spawn((
            TestSceneScoped,
            Transform::from_translation(WORM_SPAWN_POS),
            GlobalTransform::from_translation(WORM_SPAWN_POS),
            ActionTarget(hume),
            assets,
        ))
        .id();
    enqueue_routine(commands, proxy, active);
    log_line(
        log,
        "i900 on zone-asset proxy at the worm: Particle id='i900' dir=fefs (ai90 haze child)"
            .into(),
    );
}

fn worm_death_watch(
    mut worm_state: ResMut<WormState>,
    tracked: Res<TrackedEntities>,
    q_root: Query<&kuluu_render::ffxi_actor_render::FfxiRenderRoot>,
    mut q_vis: Query<&mut Visibility>,
) {
    let Some(dead_at) = worm_state.dead_at else {
        return;
    };
    if dead_at.elapsed() < Duration::from_secs_f32(WORM_HIDE_AFTER_SECS) {
        return;
    }
    let Some(worm) = tracked.by_id.get(&WORM_ID).copied() else {
        return;
    };
    if let Ok(root) = q_root.get(worm) {
        if let Ok(mut vis) = q_vis.get_mut(root.0) {
            *vis = Visibility::Hidden;
        }
    }
    worm_state.dead_at = None;
}

fn apply_floor_hidden(flag: Res<FloorHidden>, mut q: Query<&mut Visibility, With<TestFloor>>) {
    if !flag.is_changed() {
        return;
    }
    for mut vis in &mut q {
        *vis = if flag.0 {
            Visibility::Hidden
        } else {
            Visibility::default()
        };
    }
}

// Loads the South Gustaberg tunnel through the same atomic write set as LoadZone (backdrop,
// snapshot and LastAutoLoadedZone pre-stamp in one frame, or auto-load re-issues the block at a
// wrong offset) but always at world_pos ZERO: SG's zone-static lamp placements are authored in
// absolute native coordinates, and every lamp capture is framed against that absolute space. The
// ZoneParticlesPlugin then spawns ligh/'s li*/lt* generators itself from effective_zone_file_id.
fn load_sg_lamp_room(
    zone: &mut ZoneLoadParams,
    scene: &mut SceneState,
    log: &mut TestLog,
    commands: &mut Commands,
) {
    commands.insert_resource(LampRoomActive(true));
    if zone.last_zone.file_id == Some(SOUTH_GUSTABERG_MZB_FILE_ID) {
        log_line(
            &mut *log,
            format!(
                "lamp room: South Gustaberg already loaded (zone {} / DAT {})",
                SOUTH_GUSTABERG_ZONE_ID, SOUTH_GUSTABERG_MZB_FILE_ID
            ),
        );
        return;
    }
    *zone.backdrop_zone = super::launcher_backdrop::LauncherBackdropZone(SOUTH_GUSTABERG_ZONE_ID);
    scene.snapshot.zone_id = Some(SOUTH_GUSTABERG_ZONE_ID);
    zone.last_zone.file_id = Some(SOUTH_GUSTABERG_MZB_FILE_ID);
    zone.load_tx.write(LoadMzbRequest {
        file_id: SOUTH_GUSTABERG_MZB_FILE_ID,
        chunk_idx: None,
        world_pos: Vec3::ZERO,
        auto_loaded: true,
        slot: ZONE_SLOT_MAIN,
        active_sub_area: None,
    });
    zone.floor_hidden.0 = true;
    commands.insert_resource(TestZoneActive(true));
    // Lamp work wants the authored look: suppress the occlusion raycast unless a capture asks
    // for it (ANIMTEST_LAMP_RAYS_ON=1 keeps the BVH zeroing running for before/after shots).
    if std::env::var("ANIMTEST_LAMP_RAYS_ON").is_ok() {
        zone.lamp_rays.0 = false;
        log_line(&mut *log, "lamp rays: ON (ANIMTEST_LAMP_RAYS_ON)".into());
    } else {
        zone.lamp_rays.0 = true;
        log_line(
            &mut *log,
            "lamp rays: seeded off (authored bindings; checkbox re-enables the raycast)".into(),
        );
    }
    // Same reasoning for the user's own Enhanced dynamic-lights/VF settings: lamp captures want
    // authored lighting only. ANIMTEST_LAMP_SHADOWS_ON=1 skips the suppression.
    if std::env::var("ANIMTEST_LAMP_SHADOWS_ON").is_ok() {
        log_line(&mut *log, "shadows: ON (ANIMTEST_LAMP_SHADOWS_ON)".into());
    } else {
        commands.insert_resource(ShadowsOff(true));
        log_line(
            &mut *log,
            "shadows: seeded off (Bevy lamp shadows/flicker/volumetric fog suppressed; checkbox re-enables)"
                .into(),
        );
    }
    // ANIMTEST_SKY_OFF="moon,fog,sun,stars" pre-checks the night-fx kill switches for headless
    // A/B captures (the panel labels mirror from the override, same as a click would).
    if let Ok(list) = std::env::var("ANIMTEST_SKY_OFF") {
        let mut hit: Vec<&'static str> = Vec::new();
        for name in list.split(',').map(|s| s.trim().to_ascii_lowercase()) {
            let field = match name.as_str() {
                "sun" => Some(SkyFxField::Sun),
                "moon" => Some(SkyFxField::Moon),
                "fog" => Some(SkyFxField::Fog),
                "stars" => Some(SkyFxField::Stars),
                _ => None,
            };
            if let Some(field) = field {
                *field.set(&mut zone.sky_fx) = true;
                hit.push(field.label());
            }
        }
        log_line(&mut *log, format!("sky fx seeded off: {}", hit.join(", ")));
    }
    // ANIMTEST_LAMP_HALOS_OFF=1 pre-checks the lamps kill switch for headless A/B captures.
    if std::env::var_os("ANIMTEST_LAMP_HALOS_OFF").is_some() {
        zone.lamps_off.0 = true;
        log_line(
            &mut *log,
            "lamps: seeded OFF (halo billboards hidden)".into(),
        );
    }
    // ANIMTEST_WALL_GLOW_OFF=1 pre-checks the wall-glow kill switch for headless A/B captures.
    if std::env::var_os("ANIMTEST_WALL_GLOW_OFF").is_some() {
        zone.wall_glow_off.0 = true;
        log_line(&mut *log, "wall glow: seeded OFF (washes hidden)".into());
    }
    log_line(
        &mut *log,
        format!(
            "lamp room: South Gustaberg loading at absolute origin (zone {} / DAT {})",
            SOUTH_GUSTABERG_ZONE_ID, SOUTH_GUSTABERG_MZB_FILE_ID
        ),
    );
}

// Edges of the lamp-room case. On activation: hold the game clock at SG_LAMP_HOUR (the tkaa gate,
// the halos' ToD curves and the sky all read this one clock, so "time doesn't move" freezes them
// together) and re-frame the box camera on the standing spot — unless ANIMTEST_CAM overrides it.
// ANIMTEST_LAMP_ALPHA=0..1 scripts the lantern-alpha value for headless A/B shots. On teardown the
// clock is released again.
fn arm_lamp_room(
    lamp: Res<LampRoomActive>,
    mut clock: ResMut<kuluu_render::vana_time::VanaClock>,
    mut q_cam: Query<&mut Transform, With<kuluu_render::camera::OperatorCamera>>,
    mut sim: ResMut<kuluu_render::particle_sim::ParticleSimulator>,
    mut log: ResMut<TestLog>,
) {
    if !lamp.is_changed() {
        return;
    }
    if !lamp.0 {
        // Release only the room's own hold; an authored or debug hold stays.
        if clock.hold_origin() == Some(kuluu_render::vana_time::ClockHoldOrigin::AnimationRoom) {
            clock.thaw();
        }
        return;
    }
    clock.freeze_at_hour_minute(
        SG_LAMP_HOUR,
        0,
        kuluu_render::vana_time::ClockHoldOrigin::AnimationRoom,
    );
    log_line(
        &mut log,
        format!("clock: frozen at {:02}:00 (tkaa gate on)", SG_LAMP_HOUR),
    );
    if env_camera_override().is_none() {
        for mut cam in q_cam.iter_mut() {
            *cam = Transform::from_translation(SG_LAMP_EYE).looking_at(SG_LAMP_LOOK_AT, Vec3::Y);
        }
        log_line(
            &mut log,
            format!(
                "camera: framed eye {:?} look-at {:?}",
                SG_LAMP_EYE.to_array(),
                SG_LAMP_LOOK_AT.to_array()
            ),
        );
    }
    if let Some(v) = std::env::var("ANIMTEST_LAMP_ALPHA")
        .ok()
        .and_then(|s| s.trim().parse::<f32>().ok())
    {
        sim.set_lamp_halos_lift(v);
        log_line(&mut log, format!("lantern alpha scripted to {v:.3}"));
    }
}

// One flip per mouse-down (Button holds Interaction for the whole hold, same reason the case
// buttons edge-detect through CaseLock); filled box = shadows on.
fn toggle_shadows_checkbox(
    mut q_check: Query<(&Interaction, &mut BackgroundColor, &mut Text), With<ShadowsCheckbox>>,
    mut shadows: ResMut<ShadowsOff>,
    mut log: ResMut<TestLog>,
    mut was_pressed: Local<bool>,
) {
    let Ok((interaction, mut bg, mut text)) = q_check.single_mut() else {
        return;
    };
    let pressed = matches!(interaction, Interaction::Pressed);
    if pressed && !*was_pressed {
        shadows.0 = !shadows.0;
        log_line(
            &mut log,
            if shadows.0 {
                "shadows: off (Bevy lamp point-lights + flicker + volumetric fog suppressed)".into()
            } else {
                "shadows: on (user graphics settings restored)".into()
            },
        );
    }
    *was_pressed = pressed;
    let on = !shadows.0;
    if shadows.is_changed() || !text.starts_with(if on { "[x]" } else { "[ ]" }) {
        *text = Text::new(format!("{} shadows", if on { "[x]" } else { "[ ]" }));
    }
    *bg = BackgroundColor(Color::srgb(0.16, 0.2, 0.3));
}

// One flip per mouse-down for all four sky-effect rows (edge-detect is per row: a hold over one
// row must not repeat); filled box = on. Labels mirror the override every frame cheaply.
fn toggle_sky_checkboxes(
    mut q_rows: Query<(
        Entity,
        &Interaction,
        &mut BackgroundColor,
        &mut Text,
        &SkyCheckbox,
    )>,
    mut ov: ResMut<kuluu_render::sun_moon::SkyFxOverride>,
    mut log: ResMut<TestLog>,
    mut was_pressed: Local<std::collections::HashMap<bevy::ecs::entity::Entity, bool>>,
) {
    let changed = ov.is_changed();
    for (row_entity, interaction, mut bg, mut text, cx) in q_rows.iter_mut() {
        let pressed = matches!(interaction, Interaction::Pressed);
        if pressed && !was_pressed.get(&row_entity).copied().unwrap_or(false) {
            let field = cx.0;
            let flag = field.set(&mut ov);
            *flag = !*flag;
            log_line(
                &mut log,
                format!(
                    "{}: {}",
                    field.label(),
                    if field.get(&ov) { "off" } else { "on" }
                ),
            );
        }
        was_pressed.insert(row_entity, pressed);
        let on = !cx.0.get(&ov);
        if changed || !text.starts_with(if on { "[x]" } else { "[ ]" }) {
            *text = Text::new(format!(
                "{} {}",
                if on { "[x]" } else { "[ ]" },
                cx.0.label()
            ));
        }
        *bg = BackgroundColor(Color::srgb(0.16, 0.2, 0.3));
    }
}

// Same one-flip-per-mouse-down pattern; flips the halo kill switch (the tick purges live halos).
fn toggle_lamps_checkbox(
    mut q_check: Query<(&Interaction, &mut BackgroundColor, &mut Text), With<LampsCheckbox>>,
    mut off: ResMut<kuluu_render::particle_sim::LampHalosOff>,
    mut log: ResMut<TestLog>,
    mut was_pressed: Local<bool>,
) {
    let Ok((interaction, mut bg, mut text)) = q_check.single_mut() else {
        return;
    };
    let pressed = matches!(interaction, Interaction::Pressed);
    if pressed && !*was_pressed {
        off.0 = !off.0;
        log_line(
            &mut log,
            if off.0 {
                "lamps: OFF (halo billboards hidden)".into()
            } else {
                "lamps: on (halos restored live)".into()
            },
        );
    }
    *was_pressed = pressed;
    let on = !off.0;
    if off.is_changed() || !text.starts_with(if on { "[x]" } else { "[ ]" }) {
        *text = Text::new(format!("{} lamps", if on { "[x]" } else { "[ ]" }));
    }
    *bg = BackgroundColor(Color::srgb(0.16, 0.2, 0.3));
}

// Same one-flip-per-mouse-down pattern; shows/hides the wash volumes live.
fn toggle_wall_glow_checkbox(
    mut q_check: Query<(&Interaction, &mut BackgroundColor, &mut Text), With<WallGlowCheckbox>>,
    mut off: ResMut<kuluu_render::particle_sim::WallWashOff>,
    mut log: ResMut<TestLog>,
    mut was_pressed: Local<bool>,
) {
    let Ok((interaction, mut bg, mut text)) = q_check.single_mut() else {
        return;
    };
    let pressed = matches!(interaction, Interaction::Pressed);
    if pressed && !*was_pressed {
        off.0 = !off.0;
        log_line(
            &mut log,
            if off.0 {
                "wall glow: OFF (washes hidden)".into()
            } else {
                "wall glow: on (washes drawing)".into()
            },
        );
    }
    *was_pressed = pressed;
    let on = !off.0;
    if off.is_changed() || !text.starts_with(if on { "[x]" } else { "[ ]" }) {
        *text = Text::new(format!("{} wall glow", if on { "[x]" } else { "[ ]" }));
    }
    *bg = BackgroundColor(Color::srgb(0.16, 0.2, 0.3));
}

// Apply/restore the suppression itself, decoupled from the checkbox so the lamp-room seed and a
// launcher teardown take the same path. `settings.volumetric_fog` only ever takes effect at camera
// spawn (camera.rs), so live suppression removes the stashed component and restore re-inserts it.
fn apply_shadow_override(
    shadows: bool,
    settings: &mut kuluu_render::graphics_settings::GraphicsSettings,
    saved: &mut Option<ShadowSnapshot>,
    commands: &mut Commands,
    camera: Option<Entity>,
    live_fog: Option<&bevy::light::VolumetricFog>,
) {
    use kuluu_render::graphics_settings::DynamicLights;
    match (shadows, saved.as_mut()) {
        (true, None) => {
            let snapshot = ShadowSnapshot {
                dynamic_lights: settings.dynamic_lights,
                light_flicker: settings.light_flicker,
                volumetric_fog: live_fog.cloned(),
            };
            if settings.dynamic_lights == DynamicLights::Enhanced {
                // Vanilla keeps feeding the FFXI zone/actor materials exactly as retail does; only
                // the Bevy PointLight + cube-shadow half disappears.
                settings.dynamic_lights = DynamicLights::Vanilla;
                settings.light_flicker = false;
            }
            if snapshot.volumetric_fog.is_some() {
                if let Some(cam) = camera {
                    commands.entity(cam).remove::<bevy::light::VolumetricFog>();
                }
            }
            *saved = Some(snapshot);
        }
        (false, Some(_)) => {
            // Take the snapshot first: re-inserting runs through deferred commands, so it must
            // not still be borrowed while we clear the stash.
            let restore = saved.take().expect("matched Some");
            settings.dynamic_lights = restore.dynamic_lights;
            settings.light_flicker = restore.light_flicker;
            if let (Some(cam), Some(fog)) = (camera, restore.volumetric_fog) {
                commands.entity(cam).insert(fog);
            }
        }
        _ => {}
    }
}

fn sync_shadow_override(
    shadows: Res<ShadowsOff>,
    mut settings: ResMut<kuluu_render::graphics_settings::GraphicsSettings>,
    mut saved: ResMut<ShadowOverrides>,
    restore: Res<EnhanceRestore>,
    mut commands: Commands,
    mut persist_gate: ResMut<crate::graphics_store::GraphicsPersistSuspended>,
    cam_q: Query<
        (Entity, Option<&bevy::light::VolumetricFog>),
        With<kuluu_render::camera::OperatorCamera>,
    >,
) {
    if !shadows.is_changed() {
        return;
    }
    let (camera, live_fog) = cam_q
        .iter()
        .next()
        .map(|(e, f)| (Some(e), f))
        .unwrap_or((None, None));
    apply_shadow_override(
        shadows.0,
        &mut settings,
        &mut saved.0,
        &mut commands,
        camera,
        live_fog,
    );
    // The override (and the restore's write-back of the user's own values) stay off-disk; same
    // for a live enhance-mode flip, which teardown hands back the same way.
    persist_gate.0 = saved.0.is_some() || restore.0.is_some();
}

fn drive_lamp_alpha_slider(
    windows: Query<&Window, With<PrimaryWindow>>,
    q_track: Query<(&ComputedNode, &UiGlobalTransform, &Interaction), With<LampSliderTrack>>,
    mut sim: ResMut<kuluu_render::particle_sim::ParticleSimulator>,
    mut q_knob: Query<&mut Node, With<LampSliderKnob>>,
    mut q_label: Query<&mut Text, With<LampSliderLabel>>,
) {
    let Ok((node, xform, interaction)) = q_track.single() else {
        return;
    };
    if !matches!(interaction, Interaction::Pressed) {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Some(rel) = node.normalize_point(*xform, cursor) else {
        return;
    };
    let v = (rel.x + 0.5).clamp(0.0, 1.0);
    sim.set_lamp_halos_lift(v);
    if let Ok(mut knob) = q_knob.single_mut() {
        knob.left = Val::Px((LAMP_SLIDER_TRACK_W - LAMP_SLIDER_KNOB_W) * v);
    }
    if let Ok(mut label) = q_label.single_mut() {
        *label = Text::new(format!("lantern alpha {v:.2}"));
    }
}

// Wall-wash slider: full track width spans 0..WASH_ALPHA_LIFT_MAX (unlike the lamp lift's 0..1).
fn drive_wash_alpha_slider(
    windows: Query<&Window, With<PrimaryWindow>>,
    q_track: Query<(&ComputedNode, &UiGlobalTransform, &Interaction), With<WashSliderTrack>>,
    mut sim: ResMut<kuluu_render::particle_sim::ParticleSimulator>,
    mut q_knob: Query<&mut Node, With<WashSliderKnob>>,
    mut q_label: Query<&mut Text, With<WashSliderLabel>>,
) {
    let Ok((node, xform, interaction)) = q_track.single() else {
        return;
    };
    if !matches!(interaction, Interaction::Pressed) {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Some(rel) = node.normalize_point(*xform, cursor) else {
        return;
    };
    let v = (rel.x + 0.5).clamp(0.0, 1.0) * kuluu_render::particle_sim::WASH_ALPHA_LIFT_MAX;
    sim.set_wash_alpha_lift(v);
    if let Ok(mut knob) = q_knob.single_mut() {
        knob.left = Val::Px(
            (LAMP_SLIDER_TRACK_W - LAMP_SLIDER_KNOB_W)
                * (v / kuluu_render::particle_sim::WASH_ALPHA_LIFT_MAX),
        );
    }
    if let Ok(mut label) = q_label.single_mut() {
        *label = Text::new(format!("wash alpha {v:.2}"));
    }
}

// The launcher backdrop mirrors a live zone into the same world space; its meshes carry
// InGameEntity and would show through around the test floor. While the box is up, hide every
// InGameEntity not under it — effect particles are children of the test actors, so they stay.
// Restores visibility on teardown.
fn zone_backdrop_visibility(
    q_scoped: Query<Entity, With<TestSceneScoped>>,
    test_zone: Res<TestZoneActive>,
    mut q_vis: Query<
        (&mut Visibility, Option<&ChildOf>),
        (With<InGameEntity>, Without<TestSceneScoped>),
    >,
    q_anc: Query<(Option<&ChildOf>, Has<TestSceneScoped>)>,
) {
    // A user-loaded zone block is InGameEntity too; the sweep only exists for the backdrop's
    // own geometry, so it stands down while one is active.
    let active = q_scoped.iter().next().is_some() && !test_zone.0;
    for (mut vis, parent) in &mut q_vis {
        if active {
            if matches!(*vis, Visibility::Hidden) {
                continue;
            }
            let mut under_test = false;
            let mut cur = parent.map(|p| p.parent());
            while let Some(p) = cur {
                let (next, scoped) = match q_anc.get(p) {
                    Ok(v) => v,
                    Err(_) => break,
                };
                if scoped {
                    under_test = true;
                    break;
                }
                cur = next.map(|n| n.parent());
            }
            if !under_test {
                *vis = Visibility::Hidden;
            }
        } else if matches!(*vis, Visibility::Hidden) {
            *vis = Visibility::default();
        }
    }
}

fn sync_log_text(log: Res<TestLog>, mut node: Query<&mut Text, With<LogText>>) {
    if !log.is_changed() {
        return;
    }
    let recent: Vec<String> = log.lines.iter().rev().take(18).cloned().collect();
    let text = recent.into_iter().rev().collect::<Vec<_>>().join("\n");
    if let Ok(mut t) = node.single_mut() {
        *t = Text::new(text);
    }
}

fn tear_down(
    commands: &mut Commands,
    q_scoped: &Query<Entity, With<TestSceneScoped>>,
    q_ui: &Query<(Entity, Option<&ChildOf>), (With<Node>, Without<TestSceneScoped>)>,
    tracked: &mut TrackedEntities,
    scene: &mut SceneState,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
) {
    // Restore-gate: phase exit runs this with no box ever up; only a real open unloads things.
    let was_open = q_scoped.iter().next().is_some();
    // Drop the test zone and let mirror_backdrop_to_scene_state + auto-load bring the
    // default backdrop block back at its own offset.
    commands.insert_resource(TestZoneActive(false));
    // The lamp-room clock hold is normally released by `arm_lamp_room` reacting to this flag, but a
    // phase exit stops Update before that edge is observed, so hand the clock and the slider values
    // back here rather than depending on one more Launcher frame.
    commands.queue(|world: &mut World| {
        if world
            .get_resource::<LampRoomActive>()
            .is_some_and(|lamp| lamp.0)
        {
            if let Some(mut clock) = world.get_resource_mut::<kuluu_render::vana_time::VanaClock>()
            {
                if clock.hold_origin()
                    == Some(kuluu_render::vana_time::ClockHoldOrigin::AnimationRoom)
                {
                    clock.thaw();
                }
            }
        }
        if let Some(mut sim) =
            world.get_resource_mut::<kuluu_render::particle_sim::ParticleSimulator>()
        {
            sim.reset_test_lighting();
        }
    });
    commands.insert_resource(LampRoomActive(false));
    // Generator alpha forced for an a=0 additive flash is tester state: once the box is gone every
    // generator reads authored alpha again.
    commands.remove_resource::<kuluu_render::particle_sim::TestAlphaOverride>();

    // Resetting shadows to "on" makes `sync_shadow_override` restore whatever the suppression
    // replaced on its next pass (still in Launcher either way); leaving the Launcher phase itself
    // is covered by `tear_down_test_scene` applying it directly, as Update stops running there.
    commands.insert_resource(ShadowsOff(false));
    // Night-fx kill switches too: leaving the box means authored sky behaviour again (the fog
    // suppressor re-inserts what it stashed when this flips back to all-off).
    commands.insert_resource(kuluu_render::sun_moon::SkyFxOverride::default());
    commands.insert_resource(kuluu_render::particle_sim::LampHalosOff(false));
    commands.insert_resource(kuluu_render::particle_sim::WallWashOff(false));
    commands.insert_resource(kuluu_render::zone_point_lights::ZoneLampLightsOff(false));
    // A stale rays-off toggle must not follow the app into a real session.
    commands.insert_resource(LampRaysOff(false));
    commands.insert_resource(super::launcher_backdrop::LauncherBackdropZone(
        super::launcher_backdrop::DEFAULT_BACKDROP_ZONE,
    ));
    for e in q_scoped.iter() {
        // try_despawn: despawn() is recursive, so a parent earlier in the query may have
        // already freed this entity (same fix as launcher_backdrop's teardown).
        commands.entity(e).try_despawn();
    }
    set_launcher_ui_visibility(commands, q_ui, true);
    // Restore what the open unloaded: launcher render camera + backdrop entities. Zone state
    // was left standing, so the standing zone block is already in place for the mirror.
    if was_open {
        super::launcher_ui::spawn_launcher_camera_core(&mut *commands);
        super::launcher_backdrop::restore_for_test(commands, &mut *meshes, &mut *materials);
    }
    tracked.by_id.remove(&WORM_ID);
    tracked.by_id.remove(&HUME_ID);
    scene
        .snapshot
        .entities
        .retain(|e| e.id != WORM_ID && e.id != HUME_ID);
}

fn tear_down_test_scene(
    mut commands: Commands,
    q_scoped: Query<Entity, With<TestSceneScoped>>,
    q_ui: Query<(Entity, Option<&ChildOf>), (With<Node>, Without<TestSceneScoped>)>,
    mut tracked: ResMut<TrackedEntities>,
    mut scene: ResMut<SceneState>,
    mut graphics_settings: ResMut<kuluu_render::graphics_settings::GraphicsSettings>,
    mut overrides: ResMut<ShadowOverrides>,
    mut restore: ResMut<EnhanceRestore>,
    mut persist_gate: ResMut<crate::graphics_store::GraphicsPersistSuspended>,
    cam_q: Query<Entity, With<kuluu_render::camera::OperatorCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Update (and so `sync_shadow_override`) stops running once the Launcher phase ends: hand
    // any live suppression back before dropping the scene. Re-inserting needs no live-fog read:
    // the snapshot already holds the component.
    let camera = cam_q.iter().next();
    apply_shadow_override(
        false,
        &mut graphics_settings,
        &mut overrides.0,
        &mut commands,
        camera,
        None,
    );
    // Hand the user's own Dynamic Lights value back if the box flipped it.
    if let Some(prev) = restore.0.take() {
        graphics_settings.dynamic_lights = prev;
    }
    persist_gate.0 = false;
    tear_down(
        &mut commands,
        &q_scoped,
        &q_ui,
        &mut tracked,
        &mut scene,
        &mut meshes,
        &mut materials,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuluu_render::particle_sim::{
        ParticleSimulator, TestAlphaOverride, LAMP_ALPHAMAP_LIFT_DEFAULT, WASH_ALPHA_LIFT_DEFAULT,
    };

    #[test]
    fn room_keeps_rebuilt_launcher_ui_hidden() {
        let mut app = App::new();
        app.init_resource::<TestLog>()
            .add_systems(Last, apply_test_unload);
        let room = app
            .world_mut()
            .spawn((TestSceneScoped, Node::default(), Visibility::Inherited))
            .id();
        let original = app
            .world_mut()
            .spawn((Node::default(), Visibility::Hidden))
            .id();
        app.update();
        app.world_mut().despawn(original);
        let rebuilt = app
            .world_mut()
            .spawn((Node::default(), Visibility::Inherited))
            .id();
        app.update();
        assert_eq!(
            app.world().entity(rebuilt).get::<Visibility>(),
            Some(&Visibility::Hidden)
        );
        assert_eq!(
            app.world().entity(room).get::<Visibility>(),
            Some(&Visibility::Inherited)
        );
        app.world_mut().despawn(room);
        app.world_mut()
            .entity_mut(rebuilt)
            .insert(Visibility::Inherited);
        app.update();
        assert_eq!(
            app.world().entity(rebuilt).get::<Visibility>(),
            Some(&Visibility::Inherited)
        );
    }

    // Everything the box borrows from production has to come back on launcher exit: the alpha kill
    // switch removed, the game clock thawed even though `arm_lamp_room` never sees the flag flip, and
    // both lighting sliders at their authored values.
    #[test]
    fn launcher_exit_returns_everything_the_box_borrowed() {
        let mut sim = ParticleSimulator::default();
        sim.set_lamp_halos_lift(0.0);
        sim.set_wash_alpha_lift(0.0);
        let mut clock = kuluu_render::vana_time::VanaClock::default();
        clock.freeze_at_hour_minute(
            SG_LAMP_HOUR,
            0,
            kuluu_render::vana_time::ClockHoldOrigin::AnimationRoom,
        );

        let mut app = App::new();
        app.init_resource::<TrackedEntities>()
            .init_resource::<SceneState>()
            .init_resource::<kuluu_render::graphics_settings::GraphicsSettings>()
            .init_resource::<ShadowOverrides>()
            .init_resource::<EnhanceRestore>()
            .init_resource::<crate::graphics_store::GraphicsPersistSuspended>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .insert_resource(sim)
            .insert_resource(clock)
            .insert_resource(LampRoomActive(true))
            .insert_resource(TestAlphaOverride([*b"g141", *b"g144"].into()))
            .add_systems(Update, tear_down_test_scene);

        app.update();

        assert!(!app.world().contains_resource::<TestAlphaOverride>());
        assert!(
            !app.world()
                .resource::<kuluu_render::vana_time::VanaClock>()
                .is_frozen(),
            "the lamp-room clock hold outlived the box"
        );
        let sim = app.world().resource::<ParticleSimulator>();
        assert_eq!(sim.lamp_halos_lift(), LAMP_ALPHAMAP_LIFT_DEFAULT);
        assert_eq!(sim.wash_alpha_lift(), WASH_ALPHA_LIFT_DEFAULT);
    }
}
