//! Standalone VFX test box — NOT part of the kuluu client. Fixed-size window with a carrion
//! worm on the left and a sworded Hume on the right, both loaded straight from the retail DATs. The
//! far-left panel fires the dam cascade (ROM/0/0.DAT `dam0`) through kuluu-render's production
//! routine/particle/audio systems with no session layer: every button press logs its path — what
//! dam0 picked and which stages ran — to this window AND stderr, so both sides see it.
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use kuluu_render::combat_stance::{EntityMotion, RestStance, SelfMoveIntent, WalkMode};
use kuluu_render::components::WorldEntity;
use kuluu_render::dat_root::SharedDatRoot;
use kuluu_render::ffxi_actor_render::{
    dispatch_action_overlay, tick_live_ffxi_actors, ActorDatRoot, ActorLoadInFlight, ActorSubject,
    FfxiRenderRoot, LoadActorRequest,
};
use kuluu_render::scene::{apply_invis_flag_system, EntityMesh, Target, TrackedEntities};
use kuluu_render::scheduler_runtime::{
    enqueue_routine, evaluate_switch, stage_summary, ActionDatRoot, ActionTarget, ActiveScheduler,
    GlobalEffectDir, HitContext, RoutineLookup, SchedulerRuntimePlugin, UnknownFieldPolicy,
};
use kuluu_render::skinned_ffxi_material::{FfxiSkinRegistry, FfxiSkinnedMaterialCache};
use kuluu_render::snapshot::{EventLog, SceneState};
use kuluu_snapshot::EntityKind;

/// Carrion Worm family - ROM/5/64.DAT (rabbit_tester's S12 model).
const WORM_FILE: u32 = 1724;
/// Pinned sword file — the race table's slot-6 row is a bare stub, so the panel pins this.
const HUME_MAIN_WEAPON: u32 = 8397;

/// LSB zone id for West Ronfaure (zone table maps it to mzb file 200, ROM/0/120.DAT).
const WEST_RONFAURE_ZONE_ID: u16 = 100;

/// MZB/DAT file id the zone table resolves West Ronfaure to.
const WEST_RONFAURE_MZB_FILE_ID: u32 = 200;

// Retail reference standing point (in-game debug readout x=-238.241 y=139.944 z=-49.754,
// wire order: z is height) — mid-zone open grass, tree line west, campfire at (-293, 137),
// the yama_2/5 mountains east behind the camera. Placements spawn at
// mzb_to_bevy(zone_pos) + world_pos, so world_pos is the negation of that conversion:
// -(-238.241, 49.754, -139.944).
const WR_ENTRY_OFFSET: Vec3 = Vec3::new(238.241, -49.754, 139.944);

const WORM_ID: u32 = 1;
const HUME_ID: u32 = 2;

/// The test box window size.
const WINDOW_SIZE: (u32, u32) = (800, 600);

/// Camera eye height; ANIMTEST_CAM_H raises it for zone inspection from above.
fn cam_height() -> f32 {
    std::env::var("ANIMTEST_CAM_H")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2.6)
}

#[derive(Resource, Default)]
struct TestLog {
    lines: Vec<String>,
}

/// Both sinks at once: the on-screen panel and stderr (the terminal next to the window).
fn log_line(log: &mut TestLog, msg: String) {
    eprintln!("[kuluu_noserver_tester] {msg}");
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
    Hi26,
    Sb00,
    LoadZone,
    LoadWeather,
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
            Self::Hi26 => "hi26 routine (g261 child)",
            Self::Sb00 => "sb00 routine (gs02 child)",
            Self::LoadZone => "load zone (West Ronfaure)",
            Self::LoadWeather => "load weather (clouds)",
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

    fn is_crit(self) -> bool {
        matches!(self, Self::PlayerChit | Self::MobChit)
    }
}

#[derive(Resource, Default)]
struct PendingCase(Option<Case>);

/// Worm death bookkeeping: dhit runs the `dead` fall-over, then hides the model; respawn brings it back.
#[derive(Resource, Default)]
struct WormState {
    dead_at: Option<Instant>,
}

const WORM_HIDE_AFTER_SECS: f32 = 2.5;

/// ANIMTEST_AUTO schedule: the first case fires after the actors have loaded, then one per
/// spacing — long enough for each effect's particles to live out their life between shots.
const AUTO_FIRST_DELAY_SECS: f32 = 4.0;
const AUTO_SPACING_SECS: f32 = 7.0;

#[derive(Resource, Default)]
struct AutoFire {
    queue: VecDeque<Case>,
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
        "hi26" => Case::Hi26,
        "sb00" => Case::Sb00,
        "zone" => Case::LoadZone,
        "weather" => Case::LoadWeather,
        _ => return None,
    })
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let Some(root) = ffxi_dat::archive::open_test_install() else {
        eprintln!("[kuluu_noserver_tester] no retail FFXI install found (set FFXI_DAT_PATH) — the test box needs the DATs");
        std::process::exit(1);
    };
    eprintln!("[kuluu_noserver_tester] retail install opened");

    // A bare App skips bevy's task pools, but SchedulerRuntimePlugin spawns its global-effect-dir
    // load on the async compute pool (same note as rabbit_tester's build_app).
    bevy::tasks::AsyncComputeTaskPool::get_or_init(Default::default);
    let mut app = App::new();

    let root = Arc::new(root);
    app.insert_resource(ActionDatRoot(Some(Arc::clone(&root))));
    app.insert_resource(SharedDatRoot(Some(Arc::clone(&root))));
    app.insert_resource(ActorDatRoot(Some(root)));

    app.add_plugins(DefaultPlugins.set(bevy::window::WindowPlugin {
        primary_window: Some(Window {
            title: "kuluu_noserver_tester".into(),
            resolution: WINDOW_SIZE.into(),
            ..default()
        }),
        ..default()
    }));

    // Particle lifetimes and the ANIMTEST_AUTO spacing are authored in 30 fps frames;
    // uncapped, a fast GPU runs them at half their intended duration. Same limiter as
    // the client's fps_cap (apply_fps_cap_system).
    app.add_plugins(bevy_framepace::FramepacePlugin);

    // The production VFX pipeline, minus the session layer.
    app.add_plugins(SchedulerRuntimePlugin);
    app.add_plugins(kuluu_render::skinned_ffxi_material::FfxiMaterialPlugin);
    app.add_plugins(kuluu_render::ffxi_particle_material::FfxiParticleMaterialPlugin);
    app.add_plugins(kuluu_render::ffxi_zone_material::FfxiZoneMaterialPlugin);
    app.add_plugins(kuluu_render::audio::AudioPlugin);
    // Zone geometry + weather: the LoadZone/LoadWeather buttons write snapshot fields and
    // these plugins do the rest (auto-load MZB off zone_id, load/sample weather sets,
    // spawn the weat/* particle generators). DatOverlayPlugin also owns the actor-load chain.
    app.add_plugins(kuluu_render::dat_mmb::DatOverlayPlugin);
    app.add_plugins(kuluu_render::weather::WeatherPlugin);
    app.add_plugins(kuluu_render::zone_particles::ZoneParticlesPlugin);
    app.add_plugins(kuluu_render::weather_particles::WeatherParticlesPlugin);
    // The weat/<type> set splits across modules: precipitation above, the cld1/cld2
    // camera canopies in zone_clouds, and the Sun/Moon-attached billboards in
    // celestial_particles (env-gated by FFXI_DAT_CELESTIALS). West Ronfaure's clod
    // ships only canopy + sun, so all three are needed for a visible sky.
    // Registers Assets<MoonMaterial> that sun_moon_system's SunMoonRenderCfg reads.
    app.add_plugins(kuluu_render::moon_material::MoonMaterialPlugin);
    app.add_plugins(kuluu_render::zone_clouds::ZoneCloudsPlugin);
    #[cfg(not(target_arch = "wasm32"))]
    app.add_plugins(kuluu_render::celestial_particles::CelestialParticlesPlugin);
    app.init_resource::<kuluu_render::weather_fx::CurrentWeather>()
        .init_resource::<kuluu_render::weather_fx::ActiveWeatherModifier>()
        .init_resource::<kuluu_render::hud::HudPanels>();
    app.add_systems(
        Update,
        kuluu_render::weather_fx::sync_current_weather_from_snapshot
            .before(kuluu_render::weather::sample_zone_weather),
    );
    // Writes VanaSky from the clock every frame; without it the Default sky (hour 0)
    // puts the sun below the horizon and samples cloud colour tracks at night.
    app.add_systems(Update, kuluu_render::sun_moon::sun_moon_system);

    // Resources the pipeline reads (rabbit_tester's rig + the actor-load task pair).
    // The production plugins below read these from the host app (kuluu wires them in its own
    // camera/input/debug-chat plugins); a bare App must provide them or their systems panic.
    app.add_message::<LoadActorRequest>();
    app.add_message::<kuluu_render::snapshot::ToastEvent>();
    app.init_resource::<kuluu_render::input_mode::InputMode>();
    app.init_resource::<kuluu_render::camera::CameraMode>();
    app.init_resource::<EventLog>()
        .init_resource::<TrackedEntities>()
        .init_resource::<kuluu_render::ffxi_actor_render::SpellSuffixCache>()
        .init_resource::<SceneState>()
        .init_resource::<EntityMotion>()
        .init_resource::<RestStance>()
        .init_resource::<WalkMode>()
        .init_resource::<SelfMoveIntent>()
        .init_resource::<Target>()
        .init_resource::<FfxiSkinRegistry>()
        .init_resource::<FfxiSkinnedMaterialCache>()
        .init_resource::<ActorLoadInFlight>()
        .init_resource::<TestLog>()
        .init_resource::<PendingCase>()
        .init_resource::<WormState>()
        .init_resource::<FloorHidden>();
    app.init_resource::<kuluu_render::graphics_settings::GraphicsSettings>()
        .init_resource::<kuluu_render::weather::ZoneDirectionalLighting>()
        .init_resource::<kuluu_render::weather::ZoneWeather>()
        .init_resource::<kuluu_render::dat_mzb::MzbCollisionGeometry>()
        .init_resource::<kuluu_render::vana_time::VanaClock>()
        .init_resource::<kuluu_render::sun_moon::VanaSky>()
        .init_resource::<kuluu_render::audio::BgmPlaybackState>();
    app.insert_resource(kuluu_render::scene::EntityMaterials {
        pc: Handle::default(),
        self_pc: Handle::default(),
        npc: Handle::default(),
        mob: Handle::default(),
        pet: Handle::default(),
        other: Handle::default(),
        aggro: Handle::default(),
        mob_claimed_self: Handle::default(),
        mob_claimed_other: Handle::default(),
        invis_orb: Handle::default(),
    });
    // ANIMTEST_AUTO=hi26,sb00 — fire the named cases on a fixed schedule with no input, so an
    // external capture can sync to the "AUTO fired" log lines.
    let auto_cases: Vec<Case> = std::env::var("ANIMTEST_AUTO")
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| case_from_name(s.trim()))
        .collect();
    if !auto_cases.is_empty() {
        eprintln!(
            "[kuluu_noserver_tester] ANIMTEST_AUTO: {}",
            auto_cases
                .iter()
                .copied()
                .map(Case::label)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    app.insert_resource(AutoFire {
        queue: VecDeque::from(auto_cases),
        next_at: Some(Instant::now() + Duration::from_secs_f32(AUTO_FIRST_DELAY_SECS)),
    });
    app.insert_resource(kuluu_render::EntityTable::default());
    app.insert_resource(EntityMesh {
        default: Handle::default(),
        pc: Handle::default(),
        mob: Handle::default(),
        pet: Handle::default(),
    });

    app.add_systems(Startup, setup_scene);
    app.add_systems(
        Update,
        (
            dispatch_action_overlay.before(tick_live_ffxi_actors),
            apply_invis_flag_system.before(tick_live_ffxi_actors),
            tick_live_ffxi_actors,
            kuluu_render::ffxi_actor_render::update_ffxi_render_actor_lighting,
            auto_fire_cases.before(handle_button_presses),
            handle_button_presses,
            run_pending_case,
            worm_death_watch,
            apply_floor_hidden,
            sync_log_text,
        ),
    );

    {
        let mut framepace = app
            .world_mut()
            .resource_mut::<bevy_framepace::FramepaceSettings>();
        framepace.limiter = bevy_framepace::Limiter::from_framerate(30.0);
    }

    app.run();
}

fn spawn_wire(
    commands: &mut Commands,
    tracked: &mut TrackedEntities,
    state: &mut SceneState,
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
    // Same formula as scene.rs heading_to_quat. Both skeletons face local +X at identity, so
    // worm 0 faces +X toward the Hume and Hume 128 faces -X back.
    let rot = Quat::from_rotation_y(-(heading as f32) * std::f32::consts::TAU / 256.0);
    let parent = commands
        .spawn((
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
    state.snapshot.entities.push(kuluu_snapshot::Entity {
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

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut load_tx: MessageWriter<LoadActorRequest>,
    mut tracked: ResMut<TrackedEntities>,
    mut state: ResMut<SceneState>,
    mut log: ResMut<TestLog>,
) {
    // Camera + light + ground. The marker routes camera-relative systems at
    // this eye: track_weather_particles rewrites camera-anchored origins and
    // select_zone_mmb_lod owns MMB chunk visibility; both early-return without it.
    let cam_h = cam_height();
    // Faces zone west (bevy -X/-Z): the jime tree line and campfire sit that way; the
    // yama_2_m mountain 18 units east stays behind the eye.
    commands.spawn((
        kuluu_render::camera::OperatorCamera,
        Camera3d::default(),
        // Aim above the horizon so the cloud canopy shares the frame with the ground.
        Transform::from_translation(Vec3::new(7.5, cam_h, 7.5))
            .looking_at(Vec3::new(0.0, 4.0, 0.0), Vec3::Y),
    ));
    commands.spawn(DirectionalLight {
        illuminance: 9000.0,
        shadow_maps_enabled: true,
        ..default()
    });
    app_ambient(&mut commands);
    // +Y normal: a +Z plane is a vertical wall at z=0 that hides everything behind it.
    let plane: Mesh = bevy::prelude::Plane3d::new(Vec3::Y, Vec2::splat(40.0)).into();
    commands.spawn((
        TestFloor,
        Mesh3d(meshes.add(plane)),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.22, 0.26, 0.22),
            ..default()
        })),
    ));

    // Worm left, sworded Hume right, facing each other in battle stance (weapons out).
    spawn_wire(
        &mut commands,
        &mut tracked,
        &mut state,
        WORM_ID,
        EntityKind::Mob,
        Vec3::new(-1.0, 0.0, 0.0),
        0,
        ffxi_proto::decode::animation::ATTACK,
        HUME_ID,
    );
    spawn_wire(
        &mut commands,
        &mut tracked,
        &mut state,
        HUME_ID,
        EntityKind::Pc,
        Vec3::new(1.0, 0.0, 0.0),
        128,
        ffxi_proto::decode::animation::ATTACK,
        WORM_ID,
    );

    // Production order: the face file first (head/hair), then slots 2..5 from the race table;
    // slot 6 is pinned to a real sword because its table row is a bare stub.
    let mut equipment = Vec::new();
    if let Some(face_file) = kuluu_render::look_resolver::resolve_face(0, 1) {
        equipment.push(face_file);
    } else {
        log_line(
            &mut log,
            "ERROR: face file unresolved — head will not render".into(),
        );
    }
    for slot in 2u16..=5 {
        if let Some(file_id) = kuluu_render::look_resolver::resolve_equipment_slot(slot << 12, 1) {
            equipment.push(file_id);
        }
    }
    equipment.push(HUME_MAIN_WEAPON);
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
            main_weapon: Some(HUME_MAIN_WEAPON),
            sub_weapon: None,
        },
    });

    spawn_ui(&mut commands);
    log_line(
        &mut log,
        format!(
            "scene up — worm (file {WORM_FILE}) left, HumeM + sword ({HUME_MAIN_WEAPON}) right; models loading"
        ),
    );
}

fn app_ambient(commands: &mut Commands) {
    // Bevy 0.19 ambient light resource.
    commands.insert_resource(bevy::light::GlobalAmbientLight {
        color: Color::srgb(1.0, 1.0, 1.0),
        brightness: 400.0,
        ..default()
    });
}

/// The flat test floor; hidden once a real zone loads under it.
#[derive(Component)]
struct TestFloor;

#[derive(Resource, Default)]
struct FloorHidden(bool);

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

#[derive(Component)]
struct LogText;

fn spawn_ui(commands: &mut Commands) {
    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Px(215.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(8.0)),
                column_gap: Val::Px(6.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.04, 0.05, 0.09, 0.92)),
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
        Case::Hi26,
        Case::Sb00,
        Case::LoadZone,
        Case::LoadWeather,
    ] {
        let button = commands
            .spawn((
                Node {
                    width: Val::Percent(100.0),
                    padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.16, 0.2, 0.3)),
                bevy::ui::widget::Button,
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

    let log_node = commands
        .spawn((Node {
            flex_grow: 1.0,
            width: Val::Percent(100.0),
            align_items: AlignItems::FlexStart,
            ..default()
        },))
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

fn handle_button_presses(
    q_buttons: Query<(Entity, &Interaction), With<bevy::ui::widget::Button>>,
    q_children: Query<&Children>,
    q_text: Query<&Text>,
    mut pending: ResMut<PendingCase>,
) {
    for (button, interaction) in q_buttons.iter() {
        if !matches!(interaction, Interaction::Pressed) {
            continue;
        }
        let label = q_children
            .get(button)
            .ok()
            .and_then(|c| c.first())
            .and_then(|&child| q_text.get(child).ok())
            .map(|t| t.to_string());
        let case = match label.as_deref() {
            Some("player normal hit") => Case::PlayerNhIt,
            Some("player crit hit") => Case::PlayerChit,
            Some("player death hit") => Case::PlayerDhit,
            Some("mob normal hit") => Case::MobNhIt,
            Some("mob crit hit") => Case::MobChit,
            Some("mob respawn") => Case::MobRespawn,
            Some("player level up") => Case::LevelUp,
            Some("hi26 routine (g261 child)") => Case::Hi26,
            Some("sb00 routine (gs02 child)") => Case::Sb00,
            Some("load zone (West Ronfaure)") => Case::LoadZone,
            Some("load weather (clouds)") => Case::LoadWeather,
            _ => continue,
        };
        pending.0 = Some(case);
    }
}

fn run_pending_case(
    mut pending: ResMut<PendingCase>,
    mut log: ResMut<TestLog>,
    mut load_tx: MessageWriter<kuluu_render::dat_mzb::LoadMzbRequest>,
    mut last_zone: ResMut<kuluu_render::dat_mzb::LastAutoLoadedZone>,
    mut floor_hidden: ResMut<FloorHidden>,
    tracked: Res<TrackedEntities>,
    q_children: Query<&Children>,
    q_render: Query<&kuluu_render::ffxi_actor_render::FfxiRenderActor>,
    global: Option<Res<GlobalEffectDir>>,
    mut events: ResMut<EventLog>,
    mut worm_state: ResMut<WormState>,
    mut state: ResMut<SceneState>,
    q_root: Query<&FfxiRenderRoot>,
    mut q_vis: Query<&mut Visibility>,
    mut commands: Commands,
) {
    let Some(case) = pending.0.take() else { return };
    log_line(&mut log, format!("case: {}", case.label()));

    match case {
        Case::MobRespawn => respawn_worm(
            &mut worm_state,
            &mut state,
            &mut log,
            &tracked,
            &q_root,
            &mut q_vis,
        ),
        Case::LevelUp => {
            events.push(kuluu_snapshot::ViewerEvent::LevelUp { player_id: HUME_ID });
        }
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
        // vendor/server/data/enums/weather.yaml: Clouds = 2; West Ronfaure's weat/clod set
        // carries the camera-distance-culled (sec3 0x2E) weather generators.
        Case::LoadZone => {
            load_tx.write(kuluu_render::dat_mzb::LoadMzbRequest {
                file_id: WEST_RONFAURE_MZB_FILE_ID,
                chunk_idx: None,
                world_pos: WR_ENTRY_OFFSET,
                auto_loaded: true,
                slot: kuluu_render::dat_mzb::ZONE_SLOT_MAIN,
                active_sub_area: None,
            });
            // Stamp the auto-load sentinel so it does not re-issue the block at ZERO offset.
            last_zone.file_id = Some(WEST_RONFAURE_MZB_FILE_ID);
            state.snapshot.zone_id = Some(WEST_RONFAURE_ZONE_ID);
            floor_hidden.0 = true;
            log_line(
                &mut log,
                format!("zone: West Ronfaure (mzb {WEST_RONFAURE_MZB_FILE_ID}) loading at entry offset {WR_ENTRY_OFFSET:?}"),
            );
        }
        Case::LoadWeather => {
            state.snapshot.weather = Some(kuluu_snapshot::Weather::Clouds);
            log_line(&mut log, "weather: clouds set (weat/clod)".into());
        }
        _ => fire_hit(
            case,
            &tracked,
            &q_children,
            &q_render,
            global.as_deref(),
            &mut worm_state,
            &mut log,
            &mut commands,
        ),
    }
}

fn actor_routines(
    entity: Entity,
    q_children: &Query<&Children>,
    q_render: &Query<&kuluu_render::ffxi_actor_render::FfxiRenderActor>,
) -> Option<std::collections::HashMap<ffxi_dat::datid::DatId, ffxi_dat::scheduler::Scheduler>> {
    q_children
        .get(entity)
        .ok()?
        .iter()
        .find_map(|child| q_render.get(child).ok())
        .map(|a| a.routines().clone())
}

fn fire_hit(
    case: Case,
    tracked: &TrackedEntities,
    q_children: &Query<&Children>,
    q_render: &Query<&kuluu_render::ffxi_actor_render::FfxiRenderActor>,
    global: Option<&GlobalEffectDir>,
    worm_state: &mut WormState,
    log: &mut TestLog,
    commands: &mut Commands,
) {
    let (attacker_id, victim_id) = match case {
        Case::PlayerNhIt | Case::PlayerChit | Case::PlayerDhit => (HUME_ID, WORM_ID),
        _ => (WORM_ID, HUME_ID),
    };
    let Some(attacker) = tracked.by_id.get(&attacker_id).copied() else {
        log_line(
            log,
            "attacker not loaded yet — try again in a second".into(),
        );
        return;
    };
    let Some(victim) = tracked.by_id.get(&victim_id).copied() else {
        log_line(log, "victim not loaded yet — try again in a second".into());
        return;
    };

    // The reaction runs on the VICTIM with target = attacker (production semantics: the victim's
    // own damg shadows the global one and links chit back onto the attacker).
    let Some(victim_routines) = actor_routines(victim, q_children, q_render) else {
        log_line(
            log,
            "victim has no routines yet — model still loading".into(),
        );
        return;
    };
    let mut lookup = RoutineLookup::new().with_actor(&victim_routines);
    if let Some(g) = global {
        lookup = lookup.with_dat(&g.schedulers);
    }

    let ctx = HitContext {
        resolution: 0,
        animation: 0,
        info: u32::from(case.info_bits()),
    };
    log_line(
        log,
        format!(
            "dam0 switch on victim (res=Hit, info={:#x}) — {}",
            case.info_bits(),
            if global.is_some() {
                "global effect dir present"
            } else {
                "NO global effect dir loaded!"
            }
        ),
    );

    for name in evaluate_switch(&lookup, b"dam0", &ctx, UnknownFieldPolicy::Random) {
        let fourcc = ffxi_dat::datid::DatId::from_name(&name)
            .as_str()
            .to_string();
        match ActiveScheduler::from_routine(&lookup, &name) {
            Some(active) => {
                log_line(log, format!("dam0 -> {fourcc}: {}", stage_summary(&active)));
                enqueue_routine(commands, victim, active);
                commands
                    .entity(victim)
                    .try_insert(ActionTarget(Some(attacker)));
            }
            None => log_line(
                log,
                format!("dam0 -> {fourcc}: UNRESOLVED (no such routine in victim+global)"),
            ),
        }
    }

    if case.is_crit() {
        for name in evaluate_switch(&lookup, b"crtl", &ctx, UnknownFieldPolicy::Match) {
            let fourcc = ffxi_dat::datid::DatId::from_name(&name)
                .as_str()
                .to_string();
            match ActiveScheduler::from_routine(&lookup, &name) {
                Some(active) => {
                    log_line(
                        log,
                        format!(
                            "crtl -> {fourcc} (spark on attacker): {}",
                            stage_summary(&active)
                        ),
                    );
                    enqueue_routine(commands, attacker, active);
                    commands
                        .entity(attacker)
                        .try_insert(ActionTarget(Some(victim)));
                }
                None => log_line(log, format!("crtl -> {fourcc}: UNRESOLVED")),
            }
        }
    }

    if case == Case::PlayerDhit {
        // Production's Defeated frame also runs the victim's `dead` fall-over; mirror it.
        match ActiveScheduler::from_routine(&lookup, b"dead") {
            Some(active) => {
                log_line(
                    log,
                    format!("defeated -> dead (fall-over): {}", stage_summary(&active)),
                );
                enqueue_routine(commands, victim, active);
            }
            None => log_line(log, "defeated: no `dead` routine on the victim".into()),
        }
        worm_state.dead_at = Some(Instant::now());
    }
}

fn respawn_worm(
    worm_state: &mut WormState,
    state: &mut SceneState,
    log: &mut TestLog,
    tracked: &TrackedEntities,
    q_root: &Query<&FfxiRenderRoot>,
    q_vis: &mut Query<&mut Visibility>,
) {
    let Some(worm) = tracked.by_id.get(&WORM_ID).copied() else {
        return;
    };
    if let Ok(root) = q_root.get(worm) {
        if let Ok(mut vis) = q_vis.get_mut(root.0) {
            *vis = Visibility::Visible;
        }
    }
    worm_state.dead_at = None;
    if let Some(e) = state.snapshot.entities.iter_mut().find(|e| e.id == WORM_ID) {
        e.hp_pct = Some(100);
    }
    log_line(log, "worm respawned (visible again, hp 100)".into());
}

// Play a named ROM/0/0.DAT routine on the worm (as target), no alpha override — the
// child-generator carriers (hi26/g261, sb00/gs02) spawn their children through this path.
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
    let Some(hume) = tracked.by_id.get(&HUME_ID).copied() else {
        log_line(log, "hume not loaded yet".into());
        return;
    };
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
                    "{} on worm (target hume): {}",
                    String::from_utf8_lossy(routine),
                    stage_summary(&active)
                ),
            );
            // Production semantics: the routine runs on the victim with ActionTarget = attacker,
            // so target-facing generators place off the Hume, not at the worm's feet.
            enqueue_routine(commands, worm, active);
            commands.entity(worm).try_insert(ActionTarget(Some(hume)));
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

fn worm_death_watch(
    mut worm_state: ResMut<WormState>,
    tracked: Res<TrackedEntities>,
    q_root: Query<&FfxiRenderRoot>,
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
