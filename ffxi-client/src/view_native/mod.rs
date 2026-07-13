pub mod bridge;
pub mod camera_collision;
pub mod collision_bvh;
pub mod debug_heights;
pub mod exit_watchdog;
pub mod input;
pub mod launcher_backdrop;
pub mod launcher_ui;
pub mod model_viewer;
pub mod nameplate_occlude;
pub mod navmesh_overlay;
pub mod perf_hud;
pub mod screenshot;
pub mod slash_commands;
pub mod target_list_hud;
pub mod text_input;
pub mod widgets;
pub mod zone_transition;

use std::sync::Arc;

use anyhow::Result;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use ffxi_client::auth_client::AuthClient;
use ffxi_client::lobby_client::LobbyClient;
use ffxi_client::reactor::ReactorConfig;
use ffxi_client::{spawn_session_with_reactor, SessionHandle};
use ffxi_viewer_core::{
    add_hud_spawners,
    atmosphere::LastAtmosphereZone,
    audio::BgmSlots,
    configure_gizmo_render_layer,
    dat_mzb::{LastAutoLoadedZone, MzbCollisionGeometry},
    hud::zone_flash::ZoneNameResolver,
    scene::TrackedEntities,
    setup_world, setup_zone_line_assets, spawn_camera, system_cursor_icon, CursorStyle, EventLog,
    HudPlugin, InGameEntity, MousePlugin, SceneState, ViewerCorePlugin, ZoneLineDescriptor,
    ZoneLineResolver,
};
use ffxi_viewer_wire::{Stage as WireStage, ViewerEvent};
use tokio::runtime::Handle as RtHandle;

use crate::launcher::Defaults;

use self::bridge::NativeSource;
use self::input::{
    AutoRun, CameraAutoRecenter, CommandTx, HeadingTurnAccum, LocalPlayerPrediction,
};
use self::launcher_ui::{LoginErrorMsg, PendingConnect};

fn drive_feathers_cursor(
    style: Res<CursorStyle>,
    mut default_cursor: ResMut<bevy::feathers::cursor::DefaultCursor>,
) {
    let want = bevy::feathers::cursor::EntityCursor::System(system_cursor_icon(*style));
    if default_cursor.0 != want {
        default_cursor.0 = want;
    }
}

// RenderDiagnosticsPlugin only records elapsed_gpu render-graph spans when the
// device has wgpu timestamp queries, so without this the perf HUD can only show
// CPU encode time and a frame spike's GPU cost is invisible. Gated by env var
// because requesting a feature the adapter lacks aborts device creation.
// Metal recompiles a shader on first draw of each pipeline variant, on the render thread, invisible
// to the perf HUD's pass timings. Logging when new pipelines reach Ok lets those timestamps be
// correlated against `perf: frame spike` lines to confirm/deny first-use compilation as the cause.
fn log_pipeline_compiles(
    cache: Res<bevy::render::render_resource::PipelineCache>,
    mut prev_ready: Local<usize>,
) {
    use bevy::render::render_resource::CachedPipelineState;
    let ready = cache
        .pipelines()
        .filter(|p| matches!(p.state, CachedPipelineState::Ok(_)))
        .count();
    if ready > *prev_ready {
        warn!(target: "perf", "pipeline: +{} compiled (total {ready})", ready - *prev_ready);
    }
    *prev_ready = ready;
}

// The perf HUD's cpu/late marks stop at the main app's Last schedule; these three fences split the
// remaining render-sub-app time into prep (extract→pre-graph, includes swapchain acquire), graph
// (encode+submit+present), and total (through PostCleanup; total−prep−graph ≈ framepace sleep,
// which bevy_framepace runs in RenderSystems::Cleanup).
#[derive(Resource, Default)]
struct RenderSpanStamp {
    begin: Option<std::time::Instant>,
    prep_done: Option<std::time::Instant>,
}

fn stamp_render_begin(mut s: ResMut<RenderSpanStamp>) {
    s.begin = Some(std::time::Instant::now());
    s.prep_done = None;
}

fn stamp_render_prep_done(mut s: ResMut<RenderSpanStamp>) {
    if let Some(begin) = s.begin {
        let now = std::time::Instant::now();
        ffxi_viewer_core::perf_probe::note_render_prep(now - begin);
        s.prep_done = Some(now);
    }
}

fn stamp_render_graph_done(s: Res<RenderSpanStamp>) {
    if let Some(prep_done) = s.prep_done {
        ffxi_viewer_core::perf_probe::note_render_graph(prep_done.elapsed());
    }
}

fn stamp_render_total(s: Res<RenderSpanStamp>) {
    if let Some(begin) = s.begin {
        ffxi_viewer_core::perf_probe::note_render_total(begin.elapsed());
    }
}

fn gpu_timing_render_plugin() -> bevy::render::RenderPlugin {
    use bevy::render::settings::{RenderCreation, WgpuFeatures, WgpuSettings};
    let mut settings = WgpuSettings::default();
    settings.features |=
        WgpuFeatures::TIMESTAMP_QUERY | WgpuFeatures::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    bevy::render::RenderPlugin {
        render_creation: RenderCreation::Automatic(settings),
        ..default()
    }
}

#[derive(States, Default, Debug, Clone, Eq, PartialEq, Hash)]
pub enum AppPhase {
    #[default]
    Launcher,

    Connecting,

    InGame,
}

#[derive(Resource, Clone)]
pub(crate) struct SessionPorts {
    pub auth_port: u16,
    pub data_port: u16,
    pub view_port: u16,
    pub map_host_override: Option<String>,
}

#[derive(Resource, Default, Clone)]
pub(crate) struct RelayListen(
    #[allow(dead_code, reason = "read only when feature = \"relay\"")]
    pub  Option<std::net::SocketAddr>,
);

#[cfg(unix)]
#[derive(Resource, Default, Clone)]
pub(crate) struct AgentListen(pub Option<String>);

#[derive(Resource, Default, Clone)]
pub(crate) struct DatRootRes(pub Option<std::sync::Arc<ffxi_dat::DatRoot>>);

#[cfg(unix)]
#[derive(Resource, Clone)]
pub(crate) struct AgentPaused(pub std::sync::Arc<std::sync::atomic::AtomicBool>);

pub struct NativeRunArgs {
    pub server: String,
    pub ports: SessionPorts,
    pub auth: Arc<AuthClient>,
    pub lobby: Arc<LobbyClient>,
    pub defaults: Defaults,
    pub direct_mode_autostart: bool,
    pub runtime: RtHandle,
    pub relay_listen: Option<std::net::SocketAddr>,

    #[cfg(unix)]
    pub agent_listen: Option<String>,

    pub dat_root: Option<std::sync::Arc<ffxi_dat::DatRoot>>,
}

pub fn run(args: NativeRunArgs) -> Result<()> {
    let NativeRunArgs {
        server,
        ports,
        auth,
        lobby,
        defaults,
        direct_mode_autostart,
        runtime,
        relay_listen,
        #[cfg(unix)]
        agent_listen,
        dat_root,
    } = args;

    let mut app = App::new();

    // FFXI_FULLSCREEN forces exclusive fullscreen so macOS presents Direct instead of Composited
    // (native ⌃⌘F stays Composited); the Metal HUD's Composited/Direct flag then isolates whether
    // the periodic frame spikes are WindowServer compositor pacing.
    let window_mode = if std::env::var_os("FFXI_FULLSCREEN").is_some() {
        bevy::window::WindowMode::Fullscreen(
            bevy::window::MonitorSelection::Primary,
            bevy::window::VideoModeSelection::Current,
        )
    } else {
        bevy::window::WindowMode::Windowed
    };
    let mut plugins = DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: format!("ffxi-client — {server}"),
            resolution: (1280u32, 800u32).into(),
            mode: window_mode,
            ..default()
        }),
        ..default()
    });
    if std::env::var_os("FFXI_GPU_TIMING").is_some() {
        plugins = plugins.set(gpu_timing_render_plugin());
    }
    let mut plugin_group = plugins.build().disable::<LogPlugin>();
    // Pipelined rendering (Bevy's macOS default) is disabled here by default; FFXI_PIPELINED_RENDER
    // opts back in so CPU/GPU overlap can hide the serial present/GPU stalls that surface as
    // render-bound frame spikes. Opt-in until proven safe against the native-window main-thread path.
    if std::env::var_os("FFXI_PIPELINED_RENDER").is_none() {
        plugin_group =
            plugin_group.disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>();
    }
    app.add_plugins(plugin_group);

    if std::env::var_os("FFXI_GPU_TIMING").is_some() {
        if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
            render_app.add_systems(bevy::render::ExtractSchedule, log_pipeline_compiles);
        }
    }

    if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
        use bevy::render::{Render, RenderSystems};
        render_app.init_resource::<RenderSpanStamp>();
        render_app.add_systems(bevy::render::ExtractSchedule, stamp_render_begin);
        render_app.add_systems(
            Render,
            (
                stamp_render_prep_done
                    .after(RenderSystems::Prepare)
                    .before(RenderSystems::Render),
                stamp_render_graph_done
                    .after(RenderSystems::Render)
                    .before(RenderSystems::Cleanup),
                stamp_render_total.in_set(RenderSystems::PostCleanup),
            ),
        );
    }

    app.add_systems(Startup, configure_gizmo_render_layer);

    app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin);

    // FFXI_NO_FRAMEPACE bisects pacing-induced stutter: if a periodic hitch vanishes without the
    // limiter, the cause is framepace's sleep interacting with vsync, not render work.
    if std::env::var_os("FFXI_NO_FRAMEPACE").is_none() {
        app.add_plugins(bevy_framepace::FramepacePlugin);
    }

    app.add_plugins(bevy::feathers::FeathersPlugins)
        .insert_resource(bevy::feathers::theme::UiTheme(
            bevy::feathers::dark_theme::create_dark_theme(),
        ))
        .add_plugins(widgets::WidgetsPlugin)
        .add_systems(Update, drive_feathers_cursor);

    if std::env::var_os("FFXI_WIDGET_DEMO").is_some() {
        app.add_systems(Startup, widgets::spawn_widget_demo);
    }

    app.init_state::<AppPhase>();

    app.insert_resource(Time::<Fixed>::from_hz(60.0))
        .init_resource::<AutoRun>()
        .init_resource::<CameraAutoRecenter>()
        .init_resource::<HeadingTurnAccum>()
        .init_resource::<LocalPlayerPrediction>()
        .init_resource::<text_input::CaptureMode>()
        .init_resource::<collision_bvh::ZoneCollisionBvh>()
        .insert_resource(ports)
        .insert_resource(RelayListen(relay_listen))
        .insert_resource(ffxi_viewer_core::minimap::retail::MinimapDatRoot(
            dat_root.clone(),
        ))
        .insert_resource(ffxi_viewer_core::hud::status_ribbon::StatusIconDatRoot(
            dat_root.clone(),
        ))
        .insert_resource(ffxi_viewer_core::hud::item_dat_root::ItemDatRoot(
            dat_root.clone(),
        ))
        .insert_resource(ffxi_viewer_core::moon_material::MoonDatRoot(
            dat_root.clone(),
        ))
        .insert_resource(ffxi_viewer_core::ui_element_atlas::UiElementDatRoot(
            dat_root.clone(),
        ))
        .insert_resource(DatRootRes(dat_root));
    #[cfg(unix)]
    app.insert_resource(AgentListen(agent_listen));

    if direct_mode_autostart {
        app.insert_resource(launcher_ui::DirectModeAutostart);
    }

    if defaults.user.is_some() {
        app.insert_resource(launcher_ui::CliOverridesPresent);
    }

    launcher_ui::register(&mut app, &server, auth, lobby, defaults, runtime);

    app.add_systems(OnEnter(AppPhase::Connecting), bridge_connecting);

    app.add_systems(
        OnEnter(AppPhase::InGame),
        (setup_world, spawn_camera, setup_zone_line_assets),
    );
    add_hud_spawners(&mut app, OnEnter(AppPhase::InGame));
    app.init_resource::<perf_hud::PerfMonitor>();
    app.init_resource::<perf_hud::AssetChurn>();
    app.add_systems(
        Update,
        perf_hud::track_asset_churn
            .before(perf_hud::update_perf_monitor)
            .run_if(in_state(AppPhase::InGame)),
    );
    app.add_systems(
        OnEnter(AppPhase::InGame),
        (
            target_list_hud::spawn_target_list_hud,
            perf_hud::spawn_perf_hud,
        ),
    );
    app.add_systems(
        First,
        perf_hud::mark_frame_start.run_if(in_state(AppPhase::InGame)),
    );
    app.add_systems(
        PostUpdate,
        perf_hud::mark_frame_end.run_if(in_state(AppPhase::InGame)),
    );
    app.add_systems(
        Last,
        perf_hud::mark_last_end.run_if(in_state(AppPhase::InGame)),
    );
    app.add_systems(
        Update,
        (
            perf_hud::update_perf_monitor,
            perf_hud::update_perf_graph,
            target_list_hud::update_target_list_hud,
        )
            .chain()
            .run_if(in_state(AppPhase::InGame)),
    );
    app.add_systems(
        Update,
        (
            perf_hud::apply_perf_visibility,
            target_list_hud::apply_target_list_visibility,
        )
            .run_if(in_state(AppPhase::InGame)),
    );

    app.add_systems(
        OnExit(AppPhase::InGame),
        (
            despawn_ingame_entities,
            drain_entity_prediction,
            drain_mzb_load_state,
            drain_mmb_load_state,
            drain_particle_simulator,
        ),
    );

    let (loaded_bindings, persisted) = crate::keybinds_store::load_or_default();
    let store = match crate::keybinds_store::KeybindsStore::default_path() {
        Ok(p) => crate::keybinds_store::KeybindsStore::new(p),

        Err(_) => crate::keybinds_store::KeybindsStore::new(
            std::env::temp_dir().join("ffxi-keybinds.json"),
        ),
    };
    app.insert_resource(loaded_bindings);
    app.insert_resource(crate::keybinds_store::KeybindsStateRes { store, persisted });

    let (loaded_graphics, graphics_store_obj) = crate::graphics_store::load_or_default();
    app.insert_resource(loaded_graphics);
    app.insert_resource(crate::graphics_store::GraphicsStateRes {
        store: graphics_store_obj,
    });

    app.add_plugins((
        ViewerCorePlugin::<NativeSource>::default(),
        HudPlugin,
        MousePlugin,
        navmesh_overlay::NavmeshOverlayPlugin,
        launcher_backdrop::LauncherBackdropPlugin,
        zone_transition::ZoneTransitionOverlayPlugin,
    ))
    .insert_resource(ZoneNameResolver::new(ffxi_nav::zone_name))
    .insert_resource(ZoneLineResolver::new(|zone_id| {
        ffxi_nav::zone_lines_for(zone_id)
            .iter()
            .map(|z| ZoneLineDescriptor {
                line_id: z.line_id,
                from_pos: z.from_pos,
                to_zone: z.to_zone,
                scale_x: z.scale_x,
                scale_z: z.scale_z,
                rotation: z.rotation,
            })
            .collect()
    }));

    app.init_resource::<input::TabCycleStack>();
    app.init_resource::<input::SelectTargetMode>();

    app.add_systems(
        Update,
        (
            text_input::dialog_mode_sync_system,
            input::handle_input_system,
            text_input::text_input_system,
            text_input::mouse_nav_dispatch_system,
            input::dispatch_target_change_system,
            input::tab_cycle_invalidate_system,
        )
            .chain()
            .after(ffxi_viewer_core::chase_camera_system)
            .run_if(in_state(AppPhase::InGame)),
    );

    app.add_systems(
        Update,
        input::camera_polish_system
            .before(ffxi_viewer_core::chase_camera_system)
            .before(ffxi_viewer_core::firstperson_camera_system)
            .run_if(in_state(AppPhase::InGame)),
    );
    app.add_systems(
        FixedUpdate,
        input::dispatch_movement_system.run_if(in_state(AppPhase::InGame)),
    );
    app.add_systems(
        Update,
        input::reset_interaction_flags_on_zone_change.run_if(in_state(AppPhase::InGame)),
    );

    app.add_systems(Update, crate::graphics_store::persist_graphics_on_change);

    app.add_systems(
        PostUpdate,
        collision_bvh::build_collision_bvh_system
            .after(bevy::transform::TransformSystems::Propagate)
            .run_if(in_state(AppPhase::InGame)),
    );

    app.add_systems(
        Update,
        collision_bvh::build_zone_collision_bvh_system
            .before(camera_collision::clamp_chase_camera_to_collision)
            .run_if(in_state(AppPhase::InGame)),
    );
    app.add_systems(
        Update,
        camera_collision::clamp_chase_camera_to_collision
            .after(ffxi_viewer_core::chase_camera_system)
            .before(ffxi_viewer_core::nameplate_billboard::update_nameplate_billboards_system)
            .run_if(in_state(AppPhase::InGame)),
    );

    app.add_systems(
        Update,
        camera_collision::draw_camera_collision_debug
            .after(camera_collision::clamp_chase_camera_to_collision)
            .run_if(in_state(AppPhase::InGame)),
    );

    app.add_systems(
        PostUpdate,
        nameplate_occlude::occlude_nameplates_system
            .after(camera_collision::clamp_chase_camera_to_collision)
            .run_if(in_state(AppPhase::InGame)),
    );

    app.add_message::<debug_heights::DebugHeightsRequest>()
        .add_systems(
            Update,
            debug_heights::process_debug_heights.run_if(in_state(AppPhase::InGame)),
        );

    app.add_message::<screenshot::ScreenshotRequest>()
        .add_systems(Update, screenshot::process_screenshot_requests);

    app.add_systems(
        Update,
        return_to_launcher_on_disconnect.run_if(in_state(AppPhase::InGame)),
    );

    app.add_systems(Update, arm_exit_watchdog_on_appexit);

    app.run();
    exit_watchdog::mark(exit_watchdog::Stage::AppRunReturned);
    Ok(())
}

fn arm_exit_watchdog_on_appexit(mut exits: MessageReader<AppExit>) {
    if exits.read().next().is_some() {
        exit_watchdog::arm();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisconnectKind {
    Clean,

    Forced,
}

fn classify_disconnect_reason(reason: &str) -> DisconnectKind {
    if reason.starts_with("server logout state=") {
        DisconnectKind::Clean
    } else {
        DisconnectKind::Forced
    }
}

fn despawn_ingame_entities(
    mut commands: Commands,
    q: Query<Entity, With<InGameEntity>>,
    mut scene: ResMut<SceneState>,
    mut events: ResMut<EventLog>,
    mut tracked: ResMut<TrackedEntities>,
    mut collision: ResMut<MzbCollisionGeometry>,
    mut last_zone: ResMut<LastAutoLoadedZone>,
    mut last_atmo: ResMut<LastAtmosphereZone>,
    mut bgm: ResMut<BgmSlots>,
    mut weather_ambient: ResMut<ffxi_viewer_core::audio::WeatherAmbient>,
    mut combat_sfx: ResMut<ffxi_viewer_core::audio::CombatSfxState>,
    mut system_sfx_cursor: ResMut<ffxi_viewer_core::audio::SystemSfxCursor>,
    mut engagement_chat_cursor: ResMut<ffxi_viewer_core::debug_chat::EngagementChatCursor>,
    mut speed_suppression_latch: ResMut<ffxi_viewer_core::debug_chat::SpeedSuppressionLatch>,
    mut entity_motion: ResMut<ffxi_viewer_core::combat_stance::EntityMotion>,
    mut animation_blends: ResMut<ffxi_viewer_core::combat_stance::AnimationBlends>,
) {
    let mut count = 0usize;
    for entity in q.iter() {
        commands.entity(entity).despawn();
        count += 1;
    }

    tracked.by_id.clear();
    collision.positions.clear();
    collision.indices.clear();
    last_zone.file_id = None;
    last_atmo.file_id = None;

    bgm.active_entity = None;
    bgm.active = None;
    bgm.tracks = [None; ffxi_viewer_core::audio::SLOT_COUNT];
    bgm.event_cursor = 0;

    bgm.bgm_loop_counter = None;
    bgm.bgm_loops_reported = 0;

    weather_ambient.active_entity = None;
    weather_ambient.active_weather = None;
    weather_ambient.prev_weather = None;

    *combat_sfx = ffxi_viewer_core::audio::CombatSfxState::default();

    *system_sfx_cursor = ffxi_viewer_core::audio::SystemSfxCursor::default();

    *engagement_chat_cursor = ffxi_viewer_core::debug_chat::EngagementChatCursor::default();
    *speed_suppression_latch = ffxi_viewer_core::debug_chat::SpeedSuppressionLatch::default();

    *scene = SceneState::default();
    events.recent.clear();

    entity_motion.by_id.clear();
    animation_blends.by_id.clear();

    tracing::info!(count, "OnExit(InGame): despawned scoped entities");
}

fn drain_entity_prediction(
    mut prediction: ResMut<ffxi_viewer_core::combat_stance::EntityPrediction>,
) {
    prediction.by_id.clear();
}

fn drain_mzb_load_state(
    mut mzb_in_flight: ResMut<ffxi_viewer_core::dat_mzb::LoadMzbInFlight>,
    mut zone_geom_cache: ResMut<ffxi_viewer_core::dat_mzb::ZoneGeomCache>,
    mut zone_collision_bvh: ResMut<collision_bvh::ZoneCollisionBvh>,
) {
    let dropped_tasks = mzb_in_flight.tasks.len();
    let dropped_cache = zone_geom_cache.entries.len();
    mzb_in_flight.tasks.clear();
    zone_geom_cache.entries.clear();

    zone_collision_bvh.0 = None;
    if dropped_tasks > 0 || dropped_cache > 0 {
        tracing::info!(
            dropped_tasks,
            dropped_cache,
            "OnExit(InGame): drained MZB-load state",
        );
    }
}

fn drain_mmb_load_state(
    mut queue: ResMut<ffxi_viewer_core::dat_mmb::MmbLoadQueue>,
    mut parse_cache: ResMut<ffxi_viewer_core::dat_mmb::MmbParseCache>,
    mut tex_pools: ResMut<ffxi_viewer_core::dat_mmb::MmbTexPools>,
    mut handle_cache: ResMut<ffxi_viewer_core::dat_mmb::MmbHandleCache>,
) {
    let dropped_queued = queue.pending.len();
    queue.pending.clear();
    parse_cache.by_asset.clear();
    tex_pools.by_file.clear();
    handle_cache.mesh.clear();
    handle_cache.material.clear();
    if dropped_queued > 0 {
        tracing::info!(
            dropped_queued,
            "OnExit(InGame): drained MMB-load backlog + caches",
        );
    }
}

// Particle generators hold mesh-entity handles in a resource Vec; the entities are despawned by
// despawn_ingame_entities (they carry InGameEntity), but the Vec itself must be cleared so it
// doesn't leak stale generators across a zone change.
fn drain_particle_simulator(mut sim: ResMut<ffxi_viewer_core::particle_sim::ParticleSimulator>) {
    let dropped = sim.drain_entities().len();
    if dropped > 0 {
        tracing::info!(dropped, "OnExit(InGame): drained live particle generators");
    }
}

fn return_to_launcher_on_disconnect(
    scene: Option<Res<SceneState>>,
    events: Option<Res<EventLog>>,
    mut err: ResMut<LoginErrorMsg>,
    mut next_phase: ResMut<NextState<AppPhase>>,
) {
    let Some(scene) = scene else { return };
    if scene.snapshot.stage != WireStage::Disconnected {
        return;
    }

    let kind = events
        .as_ref()
        .and_then(|log| {
            log.recent.iter().rev().find_map(|e| match e {
                ViewerEvent::Disconnected { reason } => Some(classify_disconnect_reason(reason)),
                _ => None,
            })
        })
        .unwrap_or(DisconnectKind::Forced);

    if matches!(kind, DisconnectKind::Forced) && err.0.is_empty() {
        err.0 = "Disconnected from server. Press Esc to return to login.".into();
    }
    tracing::info!(?kind, "disconnect-watcher: returning AppPhase to Launcher");
    next_phase.set(AppPhase::Launcher);
}

#[cfg(test)]
mod disconnect_tests {
    use super::{classify_disconnect_reason, DisconnectKind};

    #[test]
    fn server_logout_classified_clean() {
        assert_eq!(
            classify_disconnect_reason("server logout state=1"),
            DisconnectKind::Clean
        );
        assert_eq!(
            classify_disconnect_reason("server logout state=2"),
            DisconnectKind::Clean
        );
    }

    #[test]
    fn timeout_kick_agent_classified_forced() {
        assert_eq!(
            classify_disconnect_reason("no server packets for 60s"),
            DisconnectKind::Forced
        );
        assert_eq!(
            classify_disconnect_reason("agent requested disconnect"),
            DisconnectKind::Forced
        );
        assert_eq!(classify_disconnect_reason(""), DisconnectKind::Forced);
    }
}

fn bridge_connecting(
    mut commands: Commands,
    mut pending: ResMut<PendingConnect>,
    runtime: Res<launcher_ui::RuntimeHandle>,
    server: Res<launcher_ui::ServerInfo>,
    ports: Res<SessionPorts>,
    relay: Res<RelayListen>,
    #[cfg(unix)] agent: Res<AgentListen>,
    dat_root_res: Res<DatRootRes>,
    mut next_phase: ResMut<NextState<AppPhase>>,
    mut err: ResMut<LoginErrorMsg>,
) {
    let Some(selection) = pending.0.take() else {
        err.0 = "internal: AppPhase::Connecting entered without PendingConnect".into();
        next_phase.set(AppPhase::Launcher);
        return;
    };

    let cfg = ffxi_client::session::Config {
        server: server.server.clone(),
        map_host_override: ports.map_host_override.clone(),
        auth_port: ports.auth_port,
        data_port: ports.data_port,
        view_port: ports.view_port,
        user: selection.user,
        password: selection.password,
        char_selection: ffxi_client::session::CharSelection::Id(selection.char_id),
        initial_state: Some(selection.initial_state),

        user_driven_events: true,
        dat_root: dat_root_res.0.clone(),
    };

    let _guard = runtime.0.enter();
    let SessionHandle {
        state_rx,
        cmd_tx,
        event_tx,
        session_task: _,
        folder_task: _,
    } = spawn_session_with_reactor(cfg, ReactorConfig::default());
    let event_rx = event_tx.subscribe();

    #[cfg(feature = "relay")]
    if let Some(addr) = relay.0 {
        let state_rx_relay = state_rx.clone();
        let event_tx_relay = event_tx.clone();
        let cmd_tx_relay = cmd_tx.clone();
        runtime.0.spawn(async move {
            if let Err(err) =
                crate::relay::serve(addr, state_rx_relay, event_tx_relay, cmd_tx_relay).await
            {
                tracing::warn!(error = %err, "relay listener exited");
            }
        });
    }
    #[cfg(not(feature = "relay"))]
    let _ = relay;

    #[cfg(unix)]
    if let Some(arg) = agent.0.clone() {
        let listen = ffxi_client::agent_socket::resolve_listen(&arg);
        let cmd_tx_agent = cmd_tx.clone();
        let event_tx_agent = event_tx.clone();
        let pause = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        commands.insert_resource(crate::view_native::AgentPaused(pause.clone()));
        let pause_for_socket = pause;
        runtime.0.spawn(async move {
            if let Err(err) = ffxi_client::agent_socket::serve(
                listen,
                cmd_tx_agent,
                event_tx_agent,
                Some(pause_for_socket),
            )
            .await
            {
                tracing::warn!(error = %err, "agent socket listener exited");
            }
        });
    }

    commands.insert_resource(NativeSource::new(state_rx, event_rx));
    commands.insert_resource(CommandTx(cmd_tx));

    commands.insert_resource(SessionEventTx(event_tx));

    next_phase.set(AppPhase::InGame);
}

#[derive(Resource)]
pub(crate) struct SessionEventTx(
    #[allow(dead_code)] pub tokio::sync::broadcast::Sender<crate::state::AgentEvent>,
);
