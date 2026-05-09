//! Native windowed Bevy viewer — unified `App` covering launcher + viewer.
//!
//! Sibling to `view3d/` (which projects 3D into a terminal). This module
//! opens a real OS window via `bevy_winit`, runs the launcher state
//! machine, then transitions into the viewer-core scene + HUD.
//!
//! # Threading
//!
//! Bevy's winit-driven event loop must run on the **main thread on macOS**
//! (Cocoa requires it). The caller — `main.rs::run_native_main_thread` —
//! handles this: it builds an explicit tokio Runtime, then invokes
//! [`run`] synchronously on the main thread.
//!
//! # Why one App
//!
//! winit-0.30 enforces a process-wide singleton on `EventLoop`
//! (`winit-0.30.13/src/event_loop.rs:118` returns
//! `EventLoopError::RecreationAttempt` on the second `build()`). The
//! launcher and viewer therefore must share one `App`. State is
//! managed by [`AppPhase`] (top-level) and `launcher_ui::LauncherState`
//! (sub-state of `Launcher`).

pub mod bridge;
pub mod input;
pub mod launcher_ui;
pub mod navmesh_overlay;
pub mod slash_commands;
pub mod text_input;

use std::sync::Arc;

use anyhow::Result;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use ffxi_client::auth_client::AuthClient;
use ffxi_client::lobby_client::LobbyClient;
use ffxi_client::reactor::ReactorConfig;
use ffxi_client::{spawn_session_with_reactor, SessionHandle};
use ffxi_viewer_core::{
    add_hud_spawners, setup_world, setup_zone_line_assets, spawn_camera,
    hud::zone_flash::ZoneNameResolver, HudPlugin, MousePlugin, SceneState, ViewerCorePlugin,
    ZoneLineDescriptor, ZoneLineResolver,
};
use ffxi_viewer_wire::Stage as WireStage;
use tokio::runtime::Handle as RtHandle;

use crate::launcher::Defaults;

use self::bridge::NativeSource;
use self::input::{AutoRun, CommandTx};
use self::launcher_ui::{LoginErrorMsg, PendingConnect};

/// Top-level phase of the unified native `App`.
///
/// winit-0.30 makes `EventLoop` a process-singleton (see
/// `winit-0.30.13/src/event_loop.rs:118`), so the launcher and the
/// in-game viewer cannot be separate `App::run()` invocations on
/// macOS — they share one `App` and gate their systems on this phase.
///
/// `LauncherState` is a `SubStates` of `AppPhase::Launcher`: it only
/// exists while the launcher is active, and Bevy removes the
/// `State<LauncherState>` resource automatically when `AppPhase`
/// leaves `Launcher`.
#[derive(States, Default, Debug, Clone, Eq, PartialEq, Hash)]
pub enum AppPhase {
    /// Login form, char list, lobby select. Drives `LauncherState`.
    #[default]
    Launcher,
    /// Bridge: build `session::Config` from the launcher's `Selection`,
    /// call `spawn_session`, insert `NativeSource` + `CommandTx`,
    /// optionally start the relay, then advance to `InGame`.
    Connecting,
    /// World, camera, HUD, viewer input — same surface the old
    /// `view_native::run` produced.
    InGame,
}

/// Network ports + map override needed to build `session::Config` once
/// the launcher hands off a `Selection`. Lives across phases as a
/// resource because the bridge runs on `OnEnter(Connecting)` long after
/// `main.rs` returned.
#[derive(Resource, Clone)]
pub(crate) struct SessionPorts {
    pub auth_port: u16,
    pub data_port: u16,
    pub view_port: u16,
    pub map_host_override: Option<String>,
}

/// Optional WebSocket relay listen address. Read by the connecting
/// bridge and consumed there. Always present (as `Option`) so the
/// bridge system signature is feature-stable across `cfg(feature =
/// "relay")`.
#[derive(Resource, Default, Clone)]
pub(crate) struct RelayListen(
    #[allow(dead_code, reason = "read only when feature = \"relay\"")]
    pub Option<std::net::SocketAddr>,
);

/// Inputs for [`run`]. Bundled to keep `run_native_main_thread`
/// readable now that the App owns more state.
pub struct NativeRunArgs {
    pub server: String,
    pub ports: SessionPorts,
    pub auth: Arc<AuthClient>,
    pub lobby: Arc<LobbyClient>,
    pub defaults: Defaults,
    pub direct_mode_autostart: bool,
    pub runtime: RtHandle,
    pub relay_listen: Option<std::net::SocketAddr>,
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
    } = args;

    let mut app = App::new();

    // `LogPlugin` would install its own tracing subscriber and clobber the
    // one main.rs sets up. Remove it; let main.rs's stderr-routed logger
    // own logging.
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: format!("ffxi-client — {server}"),
                    resolution: (1280u32, 800u32).into(),
                    ..default()
                }),
                ..default()
            })
            .build()
            .disable::<LogPlugin>(),
    );

    // Top-level phase. Launcher is the default starting phase regardless
    // of direct mode — direct-mode auto-advance happens via the
    // `DirectModeAutostart` marker resource, see launcher_ui::register.
    app.init_state::<AppPhase>();

    // 60 Hz movement dispatch — matches the typical render cadence so each
    // rendered frame sees a fresh `self_pos` update. `AgentCommand::Move`
    // updates local state only (see `session::keepalive_loop` — Move
    // commands do NOT send a network packet; the actual `POS` goes out via
    // the 1 s keepalive tick); raising the dispatch rate doesn't increase
    // bandwidth, only local-prediction granularity. Step size scales with
    // `Time::<Fixed>::delta_secs()` in `input::dispatch_movement_system`,
    // so total speed (yalms/sec) is preserved.
    //
    // `Target` is initialized by `ViewerCorePlugin`, so we don't init it
    // here — the duplicate would shadow the one the scene system reads
    // for highlight materials, breaking Tab targeting visuals.
    app.insert_resource(Time::<Fixed>::from_hz(60.0))
        .init_resource::<AutoRun>()
        .insert_resource(ports)
        .insert_resource(RelayListen(relay_listen));

    if direct_mode_autostart {
        app.insert_resource(launcher_ui::DirectModeAutostart);
    }

    // Launcher: registers its sub-state, resources, and per-state systems.
    launcher_ui::register(&mut app, &server, auth, lobby, defaults, runtime);

    // Connecting: bridge from launcher's Selection → session + viewer
    // resources. Single OnEnter system, see [`bridge_connecting`].
    app.add_systems(OnEnter(AppPhase::Connecting), bridge_connecting);

    // InGame: spawn world + camera (3D), then the HUD. The launcher's
    // 2D camera is despawned by OnExit(AppPhase::Launcher).
    // setup_zone_line_assets primes the cached mesh/material so the
    // sync system can spawn markers without re-hitting the Assets each
    // zone change.
    app.add_systems(
        OnEnter(AppPhase::InGame),
        (setup_world, spawn_camera, setup_zone_line_assets),
    );
    add_hud_spawners(&mut app, OnEnter(AppPhase::InGame));

    // Viewer plugins. `ViewerCorePlugin` registers ingest_system gated
    // on `resource_exists::<NativeSource>` (added by the bridge), so
    // its presence on the schedule from app build time is harmless.
    app.add_plugins((
        ViewerCorePlugin::<NativeSource>::default(),
        HudPlugin,
        MousePlugin,
        navmesh_overlay::NavmeshOverlayPlugin,
    ))
    // Plug ffxi-nav's static zone-id → name table into the zone-flash
    // banner. Without this the banner falls back to `Zone #NNN`; with
    // it we get readable names like `West_Sarutabaruta` (rendered with
    // underscores swapped for spaces by the banner itself).
    .insert_resource(ZoneNameResolver::new(ffxi_nav::zone_name))
    // Plug ffxi-nav's compile-time zone-line table into the viewer.
    // The closure converts `ffxi_nav::ZoneLine` → viewer-core's slim
    // `ZoneLineDescriptor` so viewer-core stays decoupled from nav.
    .insert_resource(ZoneLineResolver::new(|zone_id| {
        ffxi_nav::zone_lines_for(zone_id)
            .iter()
            .map(|z| ZoneLineDescriptor {
                line_id: z.line_id,
                from_pos: z.from_pos,
                to_zone: z.to_zone,
            })
            .collect()
    }));

    // Viewer-only Update / FixedUpdate systems. Gate them on InGame so
    // they don't try to read `CommandTx` / `NativeSource` while we're
    // still in the launcher.
    app.add_systems(
        Update,
        (
            // Sync `InputMode::Dialog` with `snapshot.dialog` BEFORE the
            // text-input router reads the mode this frame — that way the
            // first keypress after a dialog opens already routes through
            // `handle_dialog_key` instead of the world handler.
            text_input::dialog_mode_sync_system,
            input::handle_input_system,
            text_input::text_input_system,
            // Watches `Target` for changes and emits a `ChangeTarget`
            // action so the server learns about Tab/click/Esc/slash
            // target changes. Chained after the input handlers so the
            // `is_changed()` flag reflects this frame's mutations.
            input::dispatch_target_change_system,
        )
            .chain()
            .run_if(in_state(AppPhase::InGame)),
    );
    app.add_systems(
        FixedUpdate,
        input::dispatch_movement_system.run_if(in_state(AppPhase::InGame)),
    );

    // Disconnect → return-to-launcher. Runs every frame in InGame and
    // bounces the phase back to Launcher when the session ends (clean
    // /logout from the server, /quit, kick, connection drop, etc.).
    // Without this, the operator sees `Stage::Disconnected` in the
    // stage-bar but the world stays on screen, frozen.
    app.add_systems(
        Update,
        return_to_launcher_on_disconnect.run_if(in_state(AppPhase::InGame)),
    );

    app.run();
    Ok(())
}

/// One-shot disconnect-watcher: when `SceneState.snapshot.stage` flips to
/// `Disconnected` while we're `InGame`, populate `LoginErrorMsg` with a
/// post-mortem and transition `AppPhase` back to `Launcher`. The
/// launcher's existing `restore_login_error_on_reentry` then routes us
/// to `LauncherState::LoginError` so the operator sees what happened
/// (and can press Esc to fall back to the Login screen with creds
/// remembered by `LoginForm`).
///
/// Why not auto-advance straight to `CharList`? The lobby's
/// `LobbyHandle` was consumed by `select` (see `OpenedLobbyInner` doc),
/// so reaching CharList requires a fresh `AuthInFlight`. For now the
/// operator hits Esc → Enter to re-handshake; an auto-advance using
/// the stored `Credentials` is a worthwhile follow-up.
fn return_to_launcher_on_disconnect(
    scene: Option<Res<SceneState>>,
    mut err: ResMut<LoginErrorMsg>,
    mut next_phase: ResMut<NextState<AppPhase>>,
) {
    // First few frames after `OnEnter(InGame)` may run before the
    // bridge inserts `NativeSource` and `SceneState` is empty —
    // skip cleanly.
    let Some(scene) = scene else { return };
    if scene.snapshot.stage != WireStage::Disconnected {
        return;
    }
    // Idempotent: don't overwrite an already-populated reason on
    // repeat ticks (the watcher fires every Update until phase
    // actually switches; the launcher reads `err.0` on
    // OnEnter(Launcher), so we only need to populate it once).
    if err.0.is_empty() {
        err.0 = "Disconnected from server. Press Esc to return to login.".into();
    }
    tracing::info!("disconnect-watcher: returning AppPhase to Launcher");
    next_phase.set(AppPhase::Launcher);
}

/// `OnEnter(AppPhase::Connecting)` bridge. Pulls the `Selection` the
/// launcher stashed in `PendingConnect`, builds a `session::Config`,
/// calls `spawn_session` inside the tokio runtime, inserts the viewer
/// resources (`NativeSource`, `CommandTx`), optionally starts the
/// relay, then transitions to `AppPhase::InGame`.
///
/// On any error (missing selection, future spawn_session failure path)
/// it writes the message into `LoginErrorMsg` and bounces back to
/// `AppPhase::Launcher`; the launcher's `restore_login_error_on_reentry`
/// system promotes us straight to `LoginError`.
fn bridge_connecting(
    mut commands: Commands,
    mut pending: ResMut<PendingConnect>,
    runtime: Res<launcher_ui::RuntimeHandle>,
    server: Res<launcher_ui::ServerInfo>,
    ports: Res<SessionPorts>,
    relay: Res<RelayListen>,
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
        // Native viewer is operator-attended — the dialog HUD needs the
        // event to stay alive long enough to read; the operator (or the
        // C5 phase 2 dialog input handler) will issue `EndEvent` to advance.
        user_driven_events: true,
    };

    // spawn_session_with_reactor calls tokio::spawn internally — we
    // need an active runtime context. `enter()` activates one for the
    // duration of the guard without forcing a future.
    //
    // `_with_reactor` (vs the bare `spawn_session`) installs the
    // reactor middleware in front of `session::run`, so goal-level
    // slash commands like /pathto, /follow, /engage are absorbed by
    // the reactor's per-tick state machine instead of falling through
    // to session and erroring with "reactor middleware not wired".
    // Non-goal commands (Move, Action, Chat) pass through with zero
    // added latency.
    let _guard = runtime.0.enter();
    let SessionHandle {
        state_rx,
        cmd_tx,
        event_tx,
        session_task: _,
        folder_task: _,
    } = spawn_session_with_reactor(cfg, ReactorConfig::default());
    let event_rx = event_tx.subscribe();

    // Optional WebSocket relay. We hold `event_tx` past this scope by
    // moving it into the resource below; the relay uses its own clones.
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

    commands.insert_resource(NativeSource::new(state_rx, event_rx));
    commands.insert_resource(CommandTx(cmd_tx));

    // Hold the broadcast Sender so subscribers (folder_task, relay)
    // don't observe channel-closed. The session/folder JoinHandles are
    // intentionally dropped: the App owns them for the lifetime of the
    // process now, and Bevy's process exit ends them.
    commands.insert_resource(SessionEventTx(event_tx));

    next_phase.set(AppPhase::InGame);
}

/// Holds the broadcast event sender for the lifetime of the App so
/// downstream subscribers (folder_task, optional relay) keep seeing
/// it as live. We never read it back.
#[derive(Resource)]
pub(crate) struct SessionEventTx(
    #[allow(dead_code)] pub tokio::sync::broadcast::Sender<crate::state::AgentEvent>,
);
