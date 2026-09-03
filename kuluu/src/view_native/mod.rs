pub mod avian_bridge;
pub mod bridge;
pub mod camera_collision;
pub mod collision_bvh;
pub mod debug_heights;
pub mod exit_watchdog;
mod gamepad_input;
pub mod input;
pub mod key_drive;
pub mod key_items;
pub mod launcher_backdrop;
// 0.19 deprecated the feathers `*_bundle` spawn fns in favor of BSN scenes;
// the launcher screens migrate to BSN in kuluu-dnr5, so tolerate the shims
// until then.
#[allow(deprecated)]
pub mod launcher_ui;
#[allow(deprecated)]
pub mod model_viewer;
pub mod nameplate_occlude;
pub mod navmesh_overlay;
pub mod perf_hud;
pub mod qos;
pub mod screenshot;
pub mod slash_commands;
pub mod sub_area_report;
pub mod sun_occlusion;
pub mod target_list_hud;
pub mod text_input;
#[allow(deprecated)]
pub mod widgets;
pub mod zone_transition;

use std::sync::Arc;

use anyhow::Result;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use kuluu_render::{
    add_hud_spawners,
    atmosphere::LastAtmosphereZone,
    audio::BgmSlots,
    configure_gizmo_render_layer,
    dat_mzb::{LastAutoLoadedZone, MzbCollisionGeometry, ZoneAreaMap, ZoneChunkLightMap},
    hud::zone_flash::ZoneNameResolver,
    scene::TrackedEntities,
    setup_world, setup_zone_line_assets, spawn_camera, system_cursor_icon, CursorStyle, EventLog,
    HudPlugin, InGameEntity, MousePlugin, SceneState, ViewerCorePlugin, ZoneLineDescriptor,
    ZoneLineResolver,
};
use kuluu_session::auth_client::AuthClient;
use kuluu_session::lobby_client::LobbyClient;
use kuluu_session::reactor::ReactorConfig;
use kuluu_session::{spawn_session_with_reactor, SessionHandle};
use kuluu_snapshot::{Stage as WireStage, ViewerEvent};
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
    /// Rolling mark for the rprep sub-span fences (xtr/ast/vws/que/prp).
    last: Option<std::time::Instant>,
}

fn stamp_render_begin(mut s: ResMut<RenderSpanStamp>) {
    let now = std::time::Instant::now();
    s.begin = Some(now);
    s.prep_done = None;
    s.last = Some(now);
}

/// Fence between two RenderSystems sets: attributes time since the previous
/// mark to rprep sub-span `I` (labels in `perf_probe::RPREP_SPAN_LABELS`).
fn stamp_rprep_span<const I: usize>(mut s: ResMut<RenderSpanStamp>) {
    let now = std::time::Instant::now();
    if let Some(last) = s.last {
        kuluu_render::perf_probe::note_rprep_span(I, now - last);
    }
    s.last = Some(now);
}

fn stamp_render_prep_done(mut s: ResMut<RenderSpanStamp>) {
    if let Some(begin) = s.begin {
        let now = std::time::Instant::now();
        kuluu_render::perf_probe::note_render_prep(now - begin);
        if let Some(last) = s.last {
            // Final rprep sub-span: the Prepare set itself.
            kuluu_render::perf_probe::note_rprep_span(4, now - last);
        }
        s.prep_done = Some(now);
        s.last = Some(now);
    }
}

fn stamp_render_graph_done(s: Res<RenderSpanStamp>) {
    if let Some(prep_done) = s.prep_done {
        kuluu_render::perf_probe::note_render_graph(prep_done.elapsed());
    }
}

fn stamp_render_total(s: Res<RenderSpanStamp>) {
    if let Some(begin) = s.begin {
        kuluu_render::perf_probe::note_render_total(begin.elapsed());
    }
}

fn apply_fps_cap_system(
    settings: Res<kuluu_render::graphics_settings::GraphicsSettings>,
    mut framepace: ResMut<bevy_framepace::FramepaceSettings>,
) {
    use bevy_framepace::Limiter;
    framepace.limiter = match settings.fps_cap {
        0 => Limiter::Auto,
        n => Limiter::from_framerate(f64::from(n)),
    };
}

fn gpu_timing_render_plugin() -> bevy::render::RenderPlugin {
    use bevy::render::settings::{RenderCreation, WgpuFeatures, WgpuSettings};
    let mut settings = WgpuSettings::default();
    settings.features |=
        WgpuFeatures::TIMESTAMP_QUERY | WgpuFeatures::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    // Pass-level GPU spans (main_opaque_pass_3d etc.) require INSIDE_PASSES;
    // bevy_render's recorder silently skips them otherwise. Apple GPUs may not
    // expose this (no at-draw-boundary counter sampling) — requesting it on an
    // adapter that lacks it aborts device creation, so keep it opt-in.
    if std::env::var_os("FFXI_GPU_TIMING_PASSES").is_some() {
        settings.features |= WgpuFeatures::TIMESTAMP_QUERY_INSIDE_PASSES;
    }
    bevy::render::RenderPlugin {
        render_creation: RenderCreation::Automatic(Box::new(settings)),
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

/// Lets `insert_dat_roots` serve both the startup path (an `App` builder) and
/// the launcher reload path (`Commands`), so the DAT-root list exists once.
pub(crate) trait DatRootSink {
    fn put<R: Resource>(&mut self, resource: R);
}

impl DatRootSink for App {
    fn put<R: Resource>(&mut self, resource: R) {
        self.insert_resource(resource);
    }
}

impl DatRootSink for Commands<'_, '_> {
    fn put<R: Resource>(&mut self, resource: R) {
        self.insert_resource(resource);
    }
}

/// Every consumer of a `DatRoot` reads it through its own resource, and each one
/// must be re-inserted when the launcher changes the DAT path or that consumer
/// silently keeps rendering from the previous root (kuluu-1tr2, kuluu-051).
/// Adding a new `*DatRoot` means adding one line here and nowhere else.
pub(crate) fn insert_dat_roots(
    sink: &mut impl DatRootSink,
    dat_root: Option<std::sync::Arc<ffxi_dat::DatRoot>>,
) {
    sink.put(kuluu_render::minimap::retail::MinimapDatRoot(
        dat_root.clone(),
    ));
    sink.put(kuluu_render::hud::status_ribbon::StatusIconDatRoot(
        dat_root.clone(),
    ));
    sink.put(kuluu_render::hud::item_dat_root::ItemDatRoot(
        dat_root.clone(),
    ));
    sink.put(kuluu_render::moon_material::MoonDatRoot(dat_root.clone()));
    sink.put(kuluu_render::ui_element_atlas::UiElementDatRoot(
        dat_root.clone(),
    ));
    sink.put(kuluu_render::cutscene::CutsceneFadeDatRoot(
        dat_root.clone(),
    ));
    // Re-arm the latched spell-DAT load so a settings-screen DAT reload doesn't
    // serve suffixes from the previous install (kuluu-08rh).
    sink.put(kuluu_render::ffxi_actor_render::SpellSuffixCache::default());
    sink.put(DatRootRes(dat_root));
}

#[cfg(unix)]
#[derive(Resource, Clone)]
pub(crate) struct AgentPaused(pub std::sync::Arc<std::sync::atomic::AtomicBool>);

/// Focus-less GUI driving (kuluu-0pof): shared with the agent socket decoder so
/// remote `debug_drive`/`debug_heights` commands reach the Bevy input path.
#[derive(Resource, Clone)]
pub(crate) struct DebugControlHandle(pub kuluu_session::debug_control::SharedDebugControl);

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

    /// Open without taking focus (`play --unfocused`).
    pub unfocused: bool,

    /// Start muted (`play --mute`).
    pub mute: bool,
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
        unfocused,
        mute,
        #[cfg(unix)]
        agent_listen,
        dat_root,
    } = args;

    let mut app = App::new();

    // Load persisted graphics settings up here (rather than after DefaultPlugins)
    // so the initial window mode can honour `GraphicsSettings::fullscreen`. The
    // load is disk-only and doesn't touch app state; we hand the loaded value
    // straight to insert_resource further down. FFXI_FULLSCREEN env var still
    // wins — Direct-presentation diagnostics and remote-launch scripts rely on
    // it, and it shouldn't be overridden by whatever the last session saved.
    let (loaded_graphics, graphics_store_obj) = crate::graphics_store::load_or_default();

    // FFXI_FULLSCREEN forces exclusive fullscreen so macOS presents Direct instead of Composited
    // (native ⌃⌘F stays Composited); the Metal HUD's Composited/Direct flag then isolates whether
    // the periodic frame spikes are WindowServer compositor pacing.
    let force_exclusive = std::env::var_os("FFXI_FULLSCREEN").is_some();
    let want_fullscreen = force_exclusive || loaded_graphics.fullscreen;
    let window_mode = if !want_fullscreen {
        bevy::window::WindowMode::Windowed
    } else if loaded_graphics.windowed_fullscreen && !force_exclusive {
        // Saved preference is borderless windowed-fullscreen. The env-var
        // override always means exclusive, so it wins over this.
        bevy::window::WindowMode::BorderlessFullscreen(bevy::window::MonitorSelection::Primary)
    } else {
        bevy::window::WindowMode::Fullscreen(
            bevy::window::MonitorSelection::Primary,
            bevy::window::VideoModeSelection::Current,
        )
    };
    // FFXI_WINDOW_SIZE=WxH overrides the initial window size — lets scripted
    // verification exercise responsive layouts without OS-level window control.
    let resolution = std::env::var("FFXI_WINDOW_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))
        })
        .unwrap_or((1280, 800));
    // Before DefaultPlugins so these pools win get_or_init and TaskPoolPlugin's
    // create_default_pools no-ops (kuluu-3q8t).
    qos::init_task_pools_with_qos();
    // DLSS init must precede RenderPlugin (inside DefaultPlugins below): the
    // init plugin registers raw-Vulkan-instance callbacks that RenderPlugin's
    // build consumes, and it panics without the project id resource in place
    // first. DlssPlugin itself (the render-graph side) is auto-added by
    // AntiAliasPlugin under the same feature; whether DLSS actually works is
    // then reported at runtime via DlssSuperResolutionSupported, which
    // kuluu-render's availability probe folds into the graphics menu.
    #[cfg(feature = "dlss")]
    {
        app.insert_resource(kuluu_render::graphics::dlss::project_id());
        app.add_plugins(bevy::anti_alias::dlss::DlssInitPlugin);
    }
    let mut plugins = DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: format!("kuluu — {server}"),
            resolution: resolution.into(),
            mode: window_mode,
            // Lets gamescope surface the Steam Deck's on-screen keyboard when a
            // text field gains focus (off by default, so it never appeared).
            ime_enabled: true,
            // winit `with_active`: an agent-launched session opens behind
            // whatever the user is doing instead of yanking them out of a
            // full-screen app.
            focused: !unfocused,
            ..default()
        }),
        ..default()
    });
    if std::env::var_os("FFXI_GPU_TIMING").is_some() {
        plugins = plugins.set(gpu_timing_render_plugin());
    }
    let mut plugin_group = plugins.build().disable::<LogPlugin>();
    // Pipelined rendering (Bevy's macOS default) overlaps the render sub-app with the next
    // frame's main-world update. The render sub-app's rprep+rgraph is ~21ms here (rgraph alone
    // ~16ms of CPU command-encode/submit that is resolution- and vsync-independent — not
    // fill-bound); serially that caps the frame near 38fps. Measured 2026-07 in Bastok Mines:
    // enabling pipelining restores a stable 60fps (native res, vsync on), vs ~38fps serial.
    // Rendering is correct across zone-in/steady-state/exit. Kept default-on;
    // FFXI_NO_PIPELINED_RENDER disables it to bisect any main-thread-path issue that surfaces.
    if std::env::var_os("FFXI_NO_PIPELINED_RENDER").is_some() {
        plugin_group =
            plugin_group.disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>();
    }
    app.add_plugins(plugin_group);
    app.add_plugins(avian_bridge::AvianBridgePlugin);
    // Collider syncs run in FixedUpdate before dispatch_movement_system so
    // mob/door colliders are present + positioned before the walker sweeps
    // (avian's own pipeline update is in FixedPostUpdate; this ordering keeps
    // just-spawned colliders from being walk-through-able their first tick).
    avian_bridge::add_collider_sync_systems(&mut app);

    // Persisted audio settings: /debug Sound off (or /sound off) writes to
    // audio.json alongside graphics.json; restarts read it back here. CLI
    // `--mute` still wins — if the flag was passed, force both muted
    // regardless of what was on disk (a user asking for silence on launch
    // shouldn't get music because their last session left it on).
    let (loaded_audio_raw, audio_store_obj) = crate::audio_store::load_or_default();
    let loaded_audio = if mute {
        kuluu_render::audio::AudioMuteState {
            bgm: true,
            sfx: true,
            // Keep whatever master volume was persisted; --mute only forces the
            // category mutes, it isn't a volume reset.
            ..loaded_audio_raw
        }
    } else {
        loaded_audio_raw
    };
    app.insert_resource(loaded_audio);
    app.insert_resource(crate::audio_store::AudioStateRes {
        store: audio_store_obj,
    });

    // Bevy 0.19's GPU-driven mesh preprocessing is a large regression on Apple integrated GPUs
    // (measured 2026-07: 12.8fps GPU path vs 34.3fps CPU path in the same scene; the GPU-path
    // cost is resolution-independent — prp 53ms / rgraph 43ms stall on swapchain acquire and
    // submit). Default to the CPU mesh-batching path (Bevy 0.18-style) on macOS;
    // FFXI_GPU_PREPROCESS=1 opts back in. Two prongs are required to force the CPU path:
    // (1) insert before dependent plugins' finish() bake their CPU-vs-GPU choice
    //     (vendor/bevy_pbr render/mesh.rs reads it in finish), and
    // (2) re-assert in RenderStartup because bevy_render's BatchingPlugin re-creates the
    //     resource there via init_gpu_resource.
    if cfg!(target_os = "macos") && std::env::var_os("FFXI_GPU_PREPROCESS").is_none() {
        use bevy::render::batching::gpu_preprocessing::{
            GpuPreprocessingMode, GpuPreprocessingSupport,
        };
        if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
            render_app.insert_resource(GpuPreprocessingSupport {
                max_supported_mode: GpuPreprocessingMode::None,
            });
            render_app.add_systems(
                bevy::render::RenderStartup,
                (|mut support: bevy::prelude::ResMut<GpuPreprocessingSupport>| {
                    support.max_supported_mode = GpuPreprocessingMode::None;
                })
                .after(bevy::render::init_gpu_resource::<GpuPreprocessingSupport>),
            );
        }
    }

    if std::env::var_os("FFXI_GPU_TIMING").is_some() {
        if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
            render_app.add_systems(bevy::render::ExtractSchedule, log_pipeline_compiles);
        }
    }

    if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
        use bevy::render::{Render, RenderSystems};
        render_app.init_resource::<RenderSpanStamp>();
        render_app.add_systems(Render, qos::promote_render_thread);
        render_app.add_systems(bevy::render::ExtractSchedule, stamp_render_begin);
        render_app.add_systems(
            Render,
            (
                // rprep sub-span fences (order confirmed by bevy_render's
                // configure_sets chain: ExtractCommands → PrepareAssets/
                // PrepareMeshes → CreateViews → Specialize → PrepareViews →
                // Queue → PhaseSort → Prepare).
                stamp_rprep_span::<0>
                    .after(RenderSystems::ExtractCommands)
                    .before(RenderSystems::PrepareAssets)
                    .before(RenderSystems::PrepareMeshes),
                stamp_rprep_span::<1>
                    .after(RenderSystems::PrepareAssets)
                    .after(RenderSystems::PrepareMeshes)
                    .before(RenderSystems::CreateViews),
                stamp_rprep_span::<2>
                    .after(RenderSystems::PrepareViews)
                    .before(RenderSystems::Queue),
                stamp_rprep_span::<3>
                    .after(RenderSystems::PhaseSort)
                    .before(RenderSystems::Prepare),
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
        // Lives client-side (not the viewer-core apply_* set) because
        // bevy_framepace is a client dependency. /fps stays a session override
        // on top; this re-asserts the persisted cap on any settings change.
        app.add_systems(
            Update,
            apply_fps_cap_system.run_if(
                bevy::ecs::schedule::common_conditions::resource_changed::<
                    kuluu_render::graphics_settings::GraphicsSettings,
                >,
            ),
        );
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
        .insert_resource(RelayListen(relay_listen));
    insert_dat_roots(&mut app, dat_root);
    if let Some(store) = kuluu::overlay_store::default_store() {
        app.insert_resource(kuluu::overlay_store::OverlayStoreRes { store });
    }
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

    // World-click targeting must not run during the launcher / character-select
    // phases: a UI-button click there resolves to an empty world hit and would
    // open the no-target action menu, which then leaks into the game on zone-in.
    app.insert_resource(kuluu_render::WorldPickingEnabled(false));
    app.add_systems(
        OnEnter(AppPhase::InGame),
        |mut e: ResMut<kuluu_render::WorldPickingEnabled>| e.0 = true,
    );
    app.add_systems(
        OnExit(AppPhase::InGame),
        |mut e: ResMut<kuluu_render::WorldPickingEnabled>| e.0 = false,
    );

    app.add_systems(
        OnEnter(AppPhase::InGame),
        (setup_world, spawn_camera, setup_zone_line_assets),
    );
    add_hud_spawners(&mut app, OnEnter(AppPhase::InGame));
    app.init_resource::<perf_hud::PerfMonitor>();
    app.init_resource::<perf_hud::AssetChurn>();
    // Ungated: fires ~15s in regardless of app phase so the dump can't be
    // silently skipped if InGame is late (or never) entered.
    app.add_systems(Update, perf_hud::dump_render_diagnostics);
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
            drain_zone_sfx,
            drain_weather_particles,
            drain_cutscene_state,
            key_items::drain_key_items_viewed,
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

    // (graphics settings were loaded above so the initial window mode could honour `fullscreen`.)
    app.insert_resource(loaded_graphics);
    app.insert_resource(crate::graphics_store::GraphicsStateRes {
        store: graphics_store_obj,
    });

    app.insert_resource(crate::marker_store::load_or_default());

    app.add_plugins((
        ViewerCorePlugin::<NativeSource>::default(),
        HudPlugin,
        MousePlugin,
        navmesh_overlay::NavmeshOverlayPlugin,
        launcher_backdrop::LauncherBackdropPlugin,
        zone_transition::ZoneTransitionOverlayPlugin,
    ))
    .insert_resource(ZoneNameResolver::new(kuluu_nav::zone_name))
    .insert_resource(ZoneLineResolver::new(|zone_id| {
        kuluu_nav::zone_lines_for(zone_id)
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

    // When GPU timing is enabled we're in a perf-capture session: show the perf HUD by
    // default instead of requiring a manual toggle through the debug menu ("Perf" entry).
    if std::env::var_os("FFXI_GPU_TIMING").is_some() {
        app.insert_resource(kuluu_render::hud::HudPanels {
            perf: true,
            ..Default::default()
        });
    }

    app.init_resource::<input::TabCycleStack>();
    app.init_resource::<input::SelectTargetMode>();
    app.init_resource::<key_items::KeyItemsViewed>();

    app.insert_resource(crate::padbinds_store::load_or_default());
    app.init_resource::<gamepad_input::PrimaryGamepad>();
    app.init_resource::<gamepad_input::PadStickIntent>();
    app.init_resource::<gamepad_input::PadPressed>();
    app.add_message::<gamepad_input::PadKeyEvent>();
    app.add_systems(
        PreUpdate,
        gamepad_input::track_primary_gamepad_system.before(bevy::input::InputSystems),
    );
    app.add_systems(
        Update,
        gamepad_input::gamepad_launcher_nav_system.run_if(in_state(AppPhase::Launcher)),
    );
    // .after(InputSystems): the Gamepad components are refreshed there.
    app.add_systems(
        PreUpdate,
        (
            gamepad_input::gamepad_stick_system,
            gamepad_input::gamepad_action_system,
        )
            .after(bevy::input::InputSystems)
            .run_if(in_state(AppPhase::InGame)),
    );
    app.add_systems(OnExit(AppPhase::InGame), gamepad_input::drain_pad_state);

    app.add_systems(
        Update,
        (
            text_input::dialog_mode_sync_system,
            text_input::delivery_mode_sync_system,
            text_input::bazaar_mode_sync_system,
            text_input::auction_mode_sync_system,
            input::handle_input_system,
            text_input::text_input_system,
            text_input::mouse_nav_dispatch_system,
            input::dispatch_target_change_system,
            input::sync_target_lock_system,
            input::tab_cycle_invalidate_system,
            key_items::key_items_mark_seen_system,
            sub_area_report::report_sub_area_system,
        )
            .chain()
            .after(kuluu_render::chase_camera_system)
            .run_if(in_state(AppPhase::InGame)),
    );

    app.add_systems(
        Update,
        input::camera_polish_system
            .before(kuluu_render::chase_camera_system)
            .before(kuluu_render::firstperson_camera_system)
            .run_if(in_state(AppPhase::InGame))
            .run_if(kuluu_render::cutscene::player_camera_allowed),
    );
    app.init_resource::<input::FootprintDebug>();
    app.init_resource::<input::LastStairDetection>();
    app.init_resource::<input::StairDebugZoneCache>();
    app.add_systems(
        Update,
        (
            input::draw_footprint_debug_system,
            input::update_stair_debug_snapshot_system,
        )
            .run_if(in_state(AppPhase::InGame)),
    );
    app.add_systems(
        FixedUpdate,
        (
            input::dispatch_movement_system,
            input::apply_self_prediction_system,
            // FFXI_STAIR_CAPTURE: one JSON position line per tick (no-op unless set).
            input::stair_capture_system,
        )
            .chain()
            .run_if(in_state(AppPhase::InGame))
            .run_if(kuluu_render::cutscene::player_camera_allowed),
    );
    app.add_systems(
        Update,
        input::reset_interaction_flags_on_zone_change.run_if(in_state(AppPhase::InGame)),
    );

    app.add_systems(Update, crate::graphics_store::persist_graphics_on_change);
    app.add_systems(Update, crate::audio_store::persist_audio_on_change);
    app.add_systems(Update, crate::marker_store::sync_markers);

    app.add_systems(
        PostUpdate,
        collision_bvh::build_collision_bvh_system
            .after(bevy::transform::TransformSystems::Propagate)
            .run_if(in_state(AppPhase::InGame)),
    );

    app.add_systems(
        Update,
        collision_bvh::build_zone_collision_bvh_system
            .before(camera_collision::resolve_camera)
            .run_if(in_state(AppPhase::InGame).or_else(in_state(AppPhase::Launcher))),
    );
    app.add_systems(
        Update,
        sun_occlusion::update_sun_occlusion_system
            .after(collision_bvh::build_zone_collision_bvh_system)
            .before(kuluu_render::lens_flare::lens_flare_system),
    );
    app.add_systems(
        Update,
        camera_collision::resolve_camera
            .before(kuluu_render::nameplate_billboard::update_nameplate_billboards_system)
            .run_if(in_state(AppPhase::InGame)),
    );

    app.add_systems(
        Update,
        camera_collision::draw_camera_collision_debug
            .after(camera_collision::resolve_camera)
            .run_if(in_state(AppPhase::InGame)),
    );

    app.add_systems(
        PostUpdate,
        nameplate_occlude::occlude_nameplates_system
            .after(camera_collision::resolve_camera)
            .run_if(in_state(AppPhase::InGame)),
    );

    app.add_message::<debug_heights::DebugHeightsRequest>()
        .add_systems(
            Update,
            (
                debug_heights::trigger_debug_heights_from_socket,
                debug_heights::process_debug_heights,
            )
                .chain()
                .run_if(in_state(AppPhase::InGame)),
        );

    app.add_message::<screenshot::ScreenshotRequest>()
        .add_systems(
            Update,
            (
                screenshot::trigger_screenshot_from_socket,
                screenshot::process_screenshot_requests,
            )
                .chain(),
        );

    app.init_resource::<kuluu_render::hud_hide::HudHidden>()
        .init_resource::<kuluu_render::hud_hide::HudHideStash>()
        .add_systems(
            PostUpdate,
            kuluu_render::hud_hide::apply_hud_hidden
                .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate)
                .run_if(in_state(AppPhase::InGame)),
        );

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

/// Marker inserted on a clean `/logout` so the launcher re-auths and returns
/// to the character list instead of the login/server-select screen.
#[derive(Resource)]
pub(crate) struct ResumeCharListAfterLogout;

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
    // Tupled to stay inside Bevy's 16-param system limit.
    mut zone_geom: (
        ResMut<MzbCollisionGeometry>,
        ResMut<ZoneAreaMap>,
        ResMut<ZoneChunkLightMap>,
    ),
    mut last_zone: ResMut<LastAutoLoadedZone>,
    mut last_atmo: ResMut<LastAtmosphereZone>,
    mut bgm: ResMut<BgmSlots>,
    mut ambient_bed: ResMut<kuluu_render::audio::ZoneAmbientBed>,
    mut combat_sfx: ResMut<kuluu_render::audio::CombatSfxState>,
    mut system_sfx_cursor: ResMut<kuluu_render::audio::SystemSfxCursor>,
    mut engagement_chat_cursor: ResMut<kuluu_render::debug_chat::EngagementChatCursor>,
    mut speed_suppression_latch: ResMut<kuluu_render::debug_chat::SpeedSuppressionLatch>,
    mut entity_motion: ResMut<kuluu_render::combat_stance::EntityMotion>,
    mut animation_blends: ResMut<kuluu_render::combat_stance::AnimationBlends>,
) {
    let mut count = 0usize;
    for entity in q.iter() {
        // try_despawn: despawn() is recursive, so a parent earlier in the query may have
        // already freed this entity; bare despawn() would flood bevy_ecs error-handler
        // WARNs for every freed child during zone teardown (kuluu-6wv3).
        commands.entity(entity).try_despawn();
        count += 1;
    }

    tracked.by_id.clear();
    // Whole-resource reset, not a field-by-field clear: the parallel per-triangle
    // arrays and `cell_index` must go together, or a stale cell index will hand
    // `visit_tri` triangle ids that no longer exist. Matches the zone-change path.
    *zone_geom.0 = MzbCollisionGeometry::default();
    *zone_geom.1 = ZoneAreaMap::default();
    *zone_geom.2 = ZoneChunkLightMap::default();
    last_zone.file_id = None;
    last_atmo.file_id = None;

    bgm.active_entity = None;
    bgm.active = None;
    bgm.tracks = [None; kuluu_render::audio::SLOT_COUNT];
    bgm.event_cursor = 0;

    bgm.bgm_loop_counter = None;
    bgm.bgm_loops_reported = 0;

    ambient_bed.entity = None;
    ambient_bed.playing = None;

    *combat_sfx = kuluu_render::audio::CombatSfxState::default();

    *system_sfx_cursor = kuluu_render::audio::SystemSfxCursor::default();

    *engagement_chat_cursor = kuluu_render::debug_chat::EngagementChatCursor::default();
    *speed_suppression_latch = kuluu_render::debug_chat::SpeedSuppressionLatch::default();

    *scene = SceneState::default();
    events.recent.clear();

    entity_motion.by_id.clear();
    animation_blends.by_id.clear();

    tracing::info!(count, "OnExit(InGame): despawned scoped entities");
}

/// The cutscene fade latches by design, so its release has to be reachable from the session
/// boundary too — a black screen with no driver left is the one failure this feature must
/// not be able to reach.
fn drain_cutscene_state(
    mut mode: ResMut<kuluu_render::CutsceneMode>,
    mut fade: ResMut<kuluu_render::ScreenFade>,
    mut hud_hidden: ResMut<kuluu_render::hud_hide::HudHidden>,
) {
    *mode = kuluu_render::CutsceneMode::default();
    fade.clear();
    hud_hidden.cutscene = false;
}

fn drain_entity_prediction(mut prediction: ResMut<kuluu_render::combat_stance::EntityPrediction>) {
    prediction.by_id.clear();
}

fn drain_mzb_load_state(
    mut mzb_in_flight: ResMut<kuluu_render::dat_mzb::LoadMzbInFlight>,
    mut zone_geom_cache: ResMut<kuluu_render::dat_mzb::ZoneGeomCache>,
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
    mut queue: ResMut<kuluu_render::dat_mmb::MmbLoadQueue>,
    mut parse_cache: ResMut<kuluu_render::dat_mmb::MmbParseCache>,
    mut tex_pools: ResMut<kuluu_render::dat_mmb::MmbTexPools>,
    mut handle_cache: ResMut<kuluu_render::dat_mmb::MmbHandleCache>,
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
fn drain_particle_simulator(mut sim: ResMut<kuluu_render::particle_sim::ParticleSimulator>) {
    let dropped = sim.drain_entities().len();
    if dropped > 0 {
        tracing::info!(dropped, "OnExit(InGame): drained live particle generators");
    }
}

fn drain_zone_sfx(mut zone_sfx: ResMut<kuluu_render::zone_sfx::ZoneSfx>) {
    zone_sfx.clear();
}

fn drain_weather_particles(
    mut weather_particles: ResMut<kuluu_render::weather_particles::WeatherParticles>,
) {
    weather_particles.clear();
}

fn return_to_launcher_on_disconnect(
    mut commands: Commands,
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
    if matches!(kind, DisconnectKind::Clean) {
        commands.insert_resource(ResumeCharListAfterLogout);
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

#[cfg(test)]
mod zone_teardown_tests {
    use super::{despawn_ingame_entities, InGameEntity};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // bevy_ecs's `warn` command error handler (what bare despawn() routes
    // already-freed entities through) emits via the `log` facade under a
    // `bevy_ecs`-prefixed target; count those records to detect the flood.
    static BEVY_ECS_WARN_COUNT: AtomicUsize = AtomicUsize::new(0);

    struct BevyEcsWarnCounter;

    impl log::Log for BevyEcsWarnCounter {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            metadata.level() <= log::Level::Warn
        }

        fn log(&self, record: &log::Record) {
            if record.level() == log::Level::Warn && record.target().starts_with("bevy_ecs") {
                BEVY_ECS_WARN_COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }

        fn flush(&self) {}
    }

    fn world_with_teardown_resources() -> World {
        let mut world = World::new();
        world.init_resource::<super::SceneState>();
        world.init_resource::<super::EventLog>();
        world.init_resource::<super::TrackedEntities>();
        world.init_resource::<super::MzbCollisionGeometry>();
        world.init_resource::<super::ZoneAreaMap>();
        world.init_resource::<super::ZoneChunkLightMap>();
        world.init_resource::<super::LastAutoLoadedZone>();
        world.init_resource::<super::LastAtmosphereZone>();
        world.init_resource::<super::BgmSlots>();
        world.init_resource::<kuluu_render::audio::ZoneAmbientBed>();
        world.init_resource::<kuluu_render::audio::CombatSfxState>();
        world.init_resource::<kuluu_render::audio::SystemSfxCursor>();
        world.init_resource::<kuluu_render::debug_chat::EngagementChatCursor>();
        world.init_resource::<kuluu_render::debug_chat::SpeedSuppressionLatch>();
        world.init_resource::<kuluu_render::combat_stance::EntityMotion>();
        world.init_resource::<kuluu_render::combat_stance::AnimationBlends>();
        world
    }

    #[test]
    fn teardown_tolerates_recursively_freed_children_without_warns() {
        let _ = log::set_boxed_logger(Box::new(BevyEcsWarnCounter));
        log::set_max_level(log::LevelFilter::Warn);

        let mut world = world_with_teardown_resources();

        // InGameEntity is inserted on the parent before the child so the
        // parent's archetype — hence its query position — is created first:
        // the parent's recursive despawn frees the child before the teardown
        // loop reaches it, reproducing the zone-teardown double-despawn
        // (kuluu-6wv3).
        let parent = world.spawn_empty().id();
        let child = world.spawn_empty().id();
        world.entity_mut(parent).add_child(child);
        world.entity_mut(parent).insert(InGameEntity);
        world.entity_mut(child).insert(InGameEntity);

        world
            .run_system_once(despawn_ingame_entities)
            .expect("despawn_ingame_entities runs");

        let remaining = world
            .query_filtered::<Entity, With<InGameEntity>>()
            .iter(&world)
            .count();
        assert_eq!(remaining, 0, "teardown must despawn every InGameEntity");
        assert_eq!(
            BEVY_ECS_WARN_COUNT.load(Ordering::SeqCst),
            0,
            "already-freed children must not reach the bevy_ecs error handler"
        );
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

    let cfg = kuluu_session::session::Config {
        server: server.server.clone(),
        map_host_override: ports.map_host_override.clone(),
        auth_port: ports.auth_port,
        data_port: ports.data_port,
        view_port: ports.view_port,
        user: selection.user,
        password: selection.password,
        char_selection: kuluu_session::session::CharSelection::Id(selection.char_id),
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

    // Focus-less GUI driving (kuluu-0pof): the socket writes movement/heights
    // requests into this handle; GUI systems read it. Always present so input
    // systems can depend on it even when no socket is listening.
    let debug_ctrl = kuluu_session::debug_control::DebugControl::new_shared();
    commands.insert_resource(DebugControlHandle(debug_ctrl.clone()));

    // Stair-capture drive channel (FFXI_STAIR_DRIVE): always present so the input
    // path can depend on it; only listens when the env var names an address.
    let stair_drive = std::sync::Arc::new(std::sync::Mutex::new(
        crate::view_native::input::StairDrive::default(),
    ));
    commands.insert_resource(crate::view_native::input::StairDriveHandle(
        stair_drive.clone(),
    ));
    if let Ok(spec) = std::env::var("FFXI_STAIR_DRIVE") {
        let addr: std::net::SocketAddr = spec.parse().unwrap_or_else(|_| {
            let port: u16 = spec.trim_start_matches(':').parse().unwrap_or(9537);
            std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port))
        });
        let drive = stair_drive.clone();
        runtime.0.spawn(async move {
            crate::view_native::input::serve_stair_drive(addr, drive).await;
        });
    }

    #[cfg(unix)]
    if let Some(arg) = agent.0.clone() {
        let listen = kuluu_session::agent_socket::resolve_listen(&arg);
        let cmd_tx_agent = cmd_tx.clone();
        let event_tx_agent = event_tx.clone();
        let pause = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        commands.insert_resource(crate::view_native::AgentPaused(pause.clone()));
        let pause_for_socket = pause;
        let debug_ctrl_socket = debug_ctrl.clone();
        runtime.0.spawn(async move {
            if let Err(err) = kuluu_session::agent_socket::serve(
                listen,
                cmd_tx_agent,
                event_tx_agent,
                Some(pause_for_socket),
                Some(debug_ctrl_socket),
            )
            .await
            {
                tracing::warn!(error = %err, "agent socket listener exited");
            }
        });
    }

    commands.insert_resource(NativeSource::new(&runtime.0, state_rx, event_rx));
    commands.insert_resource(CommandTx(cmd_tx));

    commands.insert_resource(SessionEventTx(event_tx));

    next_phase.set(AppPhase::InGame);
}

#[derive(Resource)]
pub(crate) struct SessionEventTx(
    #[allow(dead_code)] pub tokio::sync::broadcast::Sender<crate::state::AgentEvent>,
);
