//! FFXI agent-driven client — entry point.

// Bevy ECS dictates system signatures (see ffxi-viewer-core; insurmountable).
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

#[cfg(any(feature = "native-window", feature = "relay"))]
use ffxi_client::state;
use ffxi_client::{agent_io, auth_client, lobby_client, session};
// Re-import the lib-level relay/wire_translate at the binary crate root
// so that submodules under main.rs can keep referring to them via
// `crate::wire_translate` / `crate::relay`. `wire_translate` is only
// reached via `view_native::bridge`, hence the narrower gate; `relay` is
// reached directly from main.rs's Play / native paths. Same re-import
// trick `state` uses above.
#[cfg(feature = "native-window")]
use ffxi_client::graphics_store;
#[cfg(feature = "native-window")]
use ffxi_client::keybinds_store;
#[cfg(feature = "relay")]
use ffxi_client::relay;
#[cfg(feature = "native-window")]
use ffxi_client::wire_translate;
mod launcher;
#[cfg(feature = "native-window")]
mod view_native;

use anyhow::{self, bail, Context, Result};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "ffxi-client",
    about = "Agent-drivable FFXI client (LSB/Phoenix)."
)]
struct Args {
    /// Server hostname or IP.
    #[arg(long, default_value = "127.0.0.1")]
    server: String,

    /// Login auth port (TLS-TCP).
    #[arg(long, default_value_t = ffxi_proto::login::LOGIN_AUTH_PORT)]
    auth_port: u16,

    /// Auth wire encoding: `json` for LSB's post-rewrite JSON handshake
    /// (default — works against the local docker stack), or `binary`
    /// for hxiloader-style 102-byte payload + MAC + ASCII version
    /// (HorizonXI's `play.horizonxi.com` runs this).
    #[arg(long, default_value = "json")]
    auth_flavor: auth_client::AuthFlavor,

    /// Xiloader version triple sent in JSON auth (`major.minor.patch`).
    /// Beats the `FFXI_XILOADER_VERSION` env var. Default `2.1.0` matches
    /// LSB upstream; HorizonXI requires `2.0.0` (their server is one minor
    /// behind). Server compares major+minor only — patch is free.
    #[arg(long)]
    xiloader_version: Option<String>,

    /// Login data port (TLS-TCP).
    #[arg(long, default_value_t = ffxi_proto::login::LOGIN_DATA_PORT)]
    data_port: u16,

    /// Login view port (TLS-TCP).
    #[arg(long, default_value_t = ffxi_proto::login::LOGIN_VIEW_PORT)]
    view_port: u16,

    /// Override the map server host returned by the lobby (`zone_settings.zoneip`
    /// is `127.0.0.1` in the dev stack, which doesn't route from inside the
    /// docker network — supply the container hostname `map` here).
    #[arg(long)]
    map_host_override: Option<String>,

    /// Bind a WebSocket relay on `<addr>` that publishes
    /// `ffxi-viewer-wire` frames (same shape the GUI viewer reads).
    /// Works in both `play` modes (GUI and `--headless`). Accepts either
    /// `host:port` (e.g. `127.0.0.1:7777`) or the literal `auto` (alias
    /// for `127.0.0.1:0` — the OS picks a free port and the chosen
    /// address is printed to stderr at startup). Defaults to off;
    /// clients can use `?format=json` for human-readable JSON instead
    /// of postcard.
    #[cfg(feature = "relay")]
    #[arg(long, value_parser = ffxi_client::relay::parse_relay_listen)]
    relay_listen: Option<std::net::SocketAddr>,

    /// Bind a Unix-domain socket at `<path>` that speaks the same
    /// JSON-line `AgentCommand` / `AgentEvent` protocol as `play
    /// --headless`'s stdio. Used by `ffxi-mcp` in attach mode
    /// (`FFXI_ATTACH=…`) to drive a long-lived GUI `play` client without
    /// spawning its own headless subprocess. Accepts an absolute path or
    /// `auto`
    /// (writes `${TMPDIR}/ffxi-agent-{pid}.sock` plus a discovery
    /// pidfile at `${TMPDIR}/ffxi-agent.pid`). If unset, falls back
    /// to the `FFXI_AGENT_LISTEN` env var.
    #[cfg(unix)]
    #[arg(long)]
    agent_listen: Option<String>,

    /// Hard-fail at startup if no FFXI client DAT install is reachable
    /// (env var `FFXI_DAT_PATH` unset *and* the workspace-relative
    /// fallback `./vendor/Game/SquareEnix/FINAL FANTASY XI` doesn't
    /// exist). Default behavior is soft-degrade: log a warning, run
    /// without static-NPC name resolution. Agents that depend on
    /// non-`?` NPC names (the farming / scout playbooks) should set
    /// this so misconfiguration surfaces at boot rather than as
    /// degraded mid-session output.
    #[arg(long)]
    require_dat: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Provision an account (or no-op if it already exists).
    Provision { user: String, password: String },
    /// Create a new character (requires account to exist first).
    /// Create a new character.
    /// Args: user pass name race(1..=8) job(1..=6) nation(0..=2) size(0..=2) face(0..=15)
    CreateChar {
        user: String,
        password: String,
        name: String,
        race: u8,
        job: u8,
        nation: u8,
        size: u8,
        face: u8,
    },
    /// End-to-end agent-drivable session: auth → lobby → map → zone-in →
    /// keepalive.
    ///
    /// Default opens a real native OS window via `bevy_winit` (Esc to
    /// disconnect). Pass `--headless` to run without a window, emitting
    /// JSON-line `AgentEvent`s on stdout and reading `AgentCommand`s from
    /// stdin — the transport `ffxi-mcp` and other harnesses drive. Both
    /// modes also accept `--agent-listen` (Unix socket) and
    /// `--relay-listen` (WebSocket).
    ///
    /// All three positional args are optional; when any are missing, the
    /// command drops into an interactive launcher (see `launcher.rs`)
    /// that prompts for credentials, lists characters on the account,
    /// and lets you pick one. The numeric charid is resolved by name
    /// against the lobby's character list — agents and humans don't
    /// need to know it.
    Play {
        user: Option<String>,
        password: Option<String>,
        char_name: Option<String>,
        /// Run without a GUI window: drive the session purely over the
        /// stdio JSON agent protocol (and any `--agent-listen` /
        /// `--relay-listen` transports). Required on builds compiled
        /// without the `native-window` feature.
        #[arg(long)]
        headless: bool,
    },
    /// Standalone model viewer. Opens a Bevy window with a 3D preview
    /// and form controls for inspecting arbitrary PC race/face/equipment
    /// combos and NPC model_ids with animation playback. Bypasses auth /
    /// lobby / map entirely — only the local DAT install is read.
    ///
    /// All form fields can be pre-populated via flags so a
    /// `/look <name>` output line drops straight into a command:
    ///
    ///   ffxi-client model-viewer --race 3 --face 11 \
    ///       --head 0x1012 --body 0x2018 --hands 0x300F \
    ///       --legs 0x4007 --feet 0x5011 --main 0x6000 --sub 0x7000
    ///
    /// `--model-id` switches the form to NPC mode (mutually exclusive
    /// with the PC fields). `--clip` sets the initial animation
    /// override (e.g. `idl`, `btl`, `run0`).
    #[cfg(feature = "native-window")]
    ModelViewer {
        /// PC race byte (1..=8). Hume M=1, Hume F=2, Elv M=3, Elv F=4,
        /// Taru M=5, Taru F=6, Mithra=7, Galka=8.
        #[arg(long)]
        race: Option<u8>,
        /// PC face index (1..=16ish; 0 falls back to 1).
        #[arg(long)]
        face: Option<u8>,
        /// Equipment slot ids. Each accepts hex (`0x1006`) or decimal.
        /// 0 = empty slot.
        #[arg(long)]
        head: Option<String>,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        hands: Option<String>,
        #[arg(long)]
        legs: Option<String>,
        #[arg(long)]
        feet: Option<String>,
        #[arg(long)]
        main: Option<String>,
        #[arg(long)]
        sub: Option<String>,
        #[arg(long)]
        ranged: Option<String>,
        /// NPC mode: single u16 model_id (hex or decimal). When set,
        /// the form starts in NPC mode and PC fields are ignored.
        #[arg(long)]
        model_id: Option<String>,
        /// Initial animation clip name (3-char prefix, e.g. `idl`,
        /// `btl`, `run0`, `sit`). Defaults to `idl`.
        #[arg(long)]
        clip: Option<String>,
    },
}

fn main() -> Result<()> {
    // Parse args *before* tracing init so we can route logs differently
    // for TUI mode — stderr writes corrupt ratatui's alternate-screen
    // buffer, so the TUI command routes logs to a file instead.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls default crypto provider");

    let args = Args::parse();

    // Logs go to stderr by default (so stdout stays clean for `play
    // --headless`'s JSON-line event stream). The GUI `play` opens its own
    // OS window and leaves the launching terminal alone, so stderr is fine
    // there too.
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(env_filter)
        .init();

    // Apply persisted launcher Settings (FFXI_DAT_PATH / FFXI_NAVMESH_DIR /
    // FFXI_MAC) to the process environment NOW — single-threaded, before the
    // tokio runtime spawns workers and before any `from_env_or_default`
    // resolution downstream. Real env vars still win unless the launcher's
    // per-field "override" box is ticked (see launcher_store::Settings).
    ffxi_client::launcher_store::load().settings.apply_to_env();

    let auth = auth_client::AuthClient::with_flavor_and_version(
        args.server.clone(),
        args.auth_port,
        args.auth_flavor,
        args.xiloader_version.as_deref(),
    );

    // Bevy/winit on macOS strictly requires its event loop on the OS main
    // thread (Cocoa restriction). Build the tokio runtime explicitly so the
    // GUI `play` path can run preflight async work via `block_on`, then run
    // Bevy synchronously on this (main) thread. All other commands (and
    // `play --headless`) run entirely inside the runtime.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;

    // GUI is the default `play` mode; `--headless` falls through to the
    // tokio runtime body below. When compiled without `native-window`,
    // this branch vanishes and the headless guard in `run_command_async`
    // handles a stray non-`--headless` invocation.
    #[cfg(feature = "native-window")]
    if matches!(
        args.command,
        Command::Play {
            headless: false,
            ..
        }
    ) {
        return run_gui_main_thread(&rt, args, auth);
    }
    #[cfg(feature = "native-window")]
    if matches!(args.command, Command::ModelViewer { .. }) {
        return run_model_viewer_main_thread(args);
    }

    rt.block_on(async move { run_command_async(args, auth).await })
}

/// Resolve the FFXI client DAT install once per process. The same
/// `Arc<DatRoot>` is then cloned into every `session::Config` so the
/// 10× VTABLE/FTABLE files only get read once. When no install is
/// reachable, returns `Ok(None)` and the session falls back to "?"
/// for static NPC names (dynamic entities still resolve via wire
/// packets). `require_dat` flips the soft-degrade into a hard error.
fn resolve_dat_root(require_dat: bool) -> Result<Option<std::sync::Arc<ffxi_dat::DatRoot>>> {
    match ffxi_dat::DatRoot::from_env_or_default() {
        Ok(root) => {
            tracing::info!(
                source = %root.root().display(),
                "loaded FFXI DAT install for NPC name lookup"
            );
            Ok(Some(std::sync::Arc::new(root)))
        }
        Err(err) if require_dat => Err(anyhow::anyhow!(
            "--require-dat is set but no DAT install was found: {err}"
        )),
        Err(err) => {
            tracing::warn!(
                error = %err,
                "no FFXI DAT install reachable; static NPC names will render as '?'. \
                 Set FFXI_DAT_PATH or pass --require-dat to fail fast."
            );
            Ok(None)
        }
    }
}

async fn run_command_async(args: Args, auth: auth_client::AuthClient) -> Result<()> {
    match args.command {
        Command::Provision { user, password } => {
            auth.ensure_account(&user, &password)
                .await
                .context("account provisioning")?;
            tracing::info!(
                cert_sha256 = ?auth.verifier.fingerprint_hex(),
                "account ensured"
            );
        }
        Command::CreateChar {
            user,
            password,
            name,
            race,
            job,
            nation,
            size,
            face,
        } => {
            auth.ensure_account(&user, &password).await.ok();
            let session = auth.login(&user, &password).await.context("login")?;
            let lobby =
                lobby_client::LobbyClient::new(args.server.clone(), args.data_port, args.view_port);
            let spec = lobby_client::CharCreateSpec {
                name: name.clone(),
                race,
                job,
                nation,
                size,
                face,
            };
            lobby
                .create_character(&session, &spec)
                .await
                .context("character creation")?;
            tracing::info!(char_name = %name, race, job, nation, "character created");
        }
        Command::Play {
            user,
            password,
            char_name,
            headless,
        } => {
            // Reached for `play --headless`, or — on builds without the
            // `native-window` feature — any `play`. The GUI default is
            // intercepted on the main thread in `main`, so a non-headless
            // invocation here means the GUI simply isn't compiled in.
            if !headless {
                bail!(
                    "this build has no GUI window (compiled without --features native-window); \
                     pass --headless to run the stdio agent session, or rebuild with the feature"
                );
            }
            let dat_root = resolve_dat_root(args.require_dat)?;
            let lobby =
                lobby_client::LobbyClient::new(args.server.clone(), args.data_port, args.view_port);

            let (user, password, char_id, _char_name, initial_state) =
                match (user, password, char_name) {
                    (Some(u), Some(p), Some(name)) => {
                        let session = auth
                            .login(&u, &p)
                            .await
                            .context("auth precheck (play direct mode)")?;
                        let handle = lobby
                            .open(&session)
                            .await
                            .context("opening lobby (play direct mode)")?;
                        let slot = handle
                            .chars()
                            .iter()
                            .find(|c| c.name == name)
                            .cloned()
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "no character named '{name}' on account '{u}' (have: {:?})",
                                    handle
                                        .chars()
                                        .iter()
                                        .map(|c| c.name.as_str())
                                        .collect::<Vec<_>>()
                                )
                            })?;
                        let mut key3 = [0u8; 20];
                        for (i, b) in key3.iter_mut().enumerate() {
                            *b = ((i as u8).wrapping_mul(0x37)) ^ 0x5a;
                        }
                        let handoff = handle
                            .select(slot.char_id, &slot.name, key3)
                            .await
                            .context("lobby select (play direct mode)")?;
                        let initial_state = session::InitialState {
                            auth: session,
                            handoff,
                            key3,
                        };
                        (u, p, slot.char_id, slot.name, initial_state)
                    }
                    (u, p, n) => {
                        let defaults = launcher::Defaults {
                            user: u,
                            password: p,
                            char_name: n,
                        };
                        let sel = launcher::run(&args.server, &auth, &lobby, defaults)
                            .await
                            .context("interactive launcher")?;
                        (
                            sel.user,
                            sel.password,
                            sel.char_id,
                            sel.char_name,
                            sel.initial_state,
                        )
                    }
                };

            let cfg = session::Config {
                server: args.server.clone(),
                map_host_override: args.map_host_override.clone(),
                auth_port: args.auth_port,
                data_port: args.data_port,
                view_port: args.view_port,
                user,
                password,
                char_selection: session::CharSelection::Id(char_id),
                initial_state: Some(initial_state),
                // Headless agent path: keep auto-dismiss on so unattended
                // sessions don't stall when an event packet arrives.
                user_driven_events: false,
                dat_root: dat_root.clone(),
            };
            let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(64);
            let (event_tx, event_rx) = tokio::sync::broadcast::channel(1024);
            let session_task = tokio::spawn(session::run(cfg, cmd_rx, event_tx.clone()));
            let agent_task = tokio::spawn(agent_io::run(cmd_tx.clone(), event_rx));

            // Optional agent socket — parallel to the stdio agent_io
            // above, for harness configurations that prefer connecting
            // over a Unix socket (e.g. `ffxi-mcp` attach mode). The
            // socket and stdio paths share the same `cmd_tx` /
            // `event_tx`, so commands from either source merge into the
            // single session inbox.
            #[cfg(unix)]
            if let Some(arg) = args
                .agent_listen
                .clone()
                .or_else(|| std::env::var("FFXI_AGENT_LISTEN").ok())
            {
                let listen = ffxi_client::agent_socket::resolve_listen(&arg);
                let sock_cmd_tx = cmd_tx.clone();
                let sock_event_tx = event_tx.clone();
                tokio::spawn(async move {
                    if let Err(err) = ffxi_client::agent_socket::serve(
                        listen,
                        sock_cmd_tx,
                        sock_event_tx,
                        None, // headless `play` has no GUI pause toggle
                    )
                    .await
                    {
                        tracing::warn!(error = %err, "agent socket listener exited");
                    }
                });
            }

            // Optional WebSocket relay. Needs a `watch::Receiver<SessionState>`,
            // so we run a folder task that converts the broadcast event stream
            // into the same canonical state the native viewer reads.
            //
            // Pre-flight is a synchronous TcpListener::bind/drop — the
            // session task wouldn't start in time to surface a port
            // collision before agent_io blocks on stdin, so we'd appear
            // hung. Failing fast here makes the misconfig obvious.
            #[cfg(feature = "relay")]
            if let Some(addr) = args.relay_listen {
                if let Err(err) = relay::preflight_bind(addr) {
                    eprintln!("error: {err:#}");
                    eprintln!("hint: pass `--relay-listen auto` to let the OS assign a free port,",);
                    eprintln!("      or pick a different `host:port`.");
                    std::process::exit(2);
                }
            }

            #[cfg(feature = "relay")]
            let _relay_keepalive = if let Some(addr) = args.relay_listen {
                let (state_tx, state_rx) =
                    tokio::sync::watch::channel(state::SessionState::default());
                let folder_rx = event_tx.subscribe();
                let _folder = tokio::spawn(session::run_event_folder(folder_rx, state_tx));
                let relay_event_tx = event_tx.clone();
                let relay_cmd_tx = cmd_tx.clone();
                tokio::spawn(async move {
                    if let Err(err) =
                        relay::serve(addr, state_rx, relay_event_tx, relay_cmd_tx).await
                    {
                        tracing::warn!(error = %err, "relay listener exited");
                    }
                });
                Some(_folder)
            } else {
                None
            };

            // Whichever finishes first ends the session.
            tokio::select! {
                r = session_task => r??,
                r = agent_task => r??,
            }
        }
        #[cfg(feature = "native-window")]
        Command::ModelViewer { .. } => {
            unreachable!(
                "ModelViewer is dispatched on the main thread by \
                 run_model_viewer_main_thread; it must not reach the tokio runtime body"
            );
        }
    }

    Ok(())
}

#[cfg(feature = "native-window")]
fn run_gui_main_thread(
    rt: &tokio::runtime::Runtime,
    args: Args,
    auth: auth_client::AuthClient,
) -> Result<()> {
    #[cfg(feature = "relay")]
    let relay_listen = args.relay_listen;
    #[cfg(not(feature = "relay"))]
    let relay_listen: Option<std::net::SocketAddr> = None;
    #[cfg(unix)]
    let agent_listen = args
        .agent_listen
        .clone()
        .or_else(|| std::env::var("FFXI_AGENT_LISTEN").ok());
    // Fail fast on EADDRINUSE before we open the launcher window. A
    // post-bind error from the relay task would only emit a buried
    // `tracing::warn!` and the user would silently end up with no
    // browser-visible relay.
    #[cfg(feature = "relay")]
    if let Some(addr) = relay_listen {
        if let Err(err) = relay::preflight_bind(addr) {
            eprintln!("error: {err:#}");
            eprintln!("hint: pass `--relay-listen auto` to let the OS assign a free port,");
            eprintln!("      or pick a different `host:port`.");
            std::process::exit(2);
        }
    }
    let args_require_dat = args.require_dat;
    let Args {
        server,
        auth_port,
        data_port,
        view_port,
        map_host_override,
        command,
        ..
    } = args;
    let Command::Play {
        user,
        password,
        char_name,
        ..
    } = command
    else {
        unreachable!("dispatched only when args.command is Command::Play (GUI mode)");
    };

    // Direct mode = all three positional args provided. The launcher
    // pre-fills its form with `defaults` and auto-advances past Login
    // and CharList when this marker is present (see launcher_ui's
    // direct_mode_login_autostart / direct_mode_charlist_autoselect).
    // No more synchronous block_on preflight — the launcher's async
    // tasks handle both modes uniformly.
    let direct_mode_autostart = user.is_some() && password.is_some() && char_name.is_some();

    let defaults = launcher::Defaults {
        user,
        password,
        char_name,
    };

    let lobby = lobby_client::LobbyClient::new(server.clone(), data_port, view_port);

    let dat_root = resolve_dat_root(args_require_dat)?;

    view_native::run(view_native::NativeRunArgs {
        server,
        ports: view_native::SessionPorts {
            auth_port,
            data_port,
            view_port,
            map_host_override,
        },
        auth: std::sync::Arc::new(auth),
        lobby: std::sync::Arc::new(lobby),
        defaults,
        direct_mode_autostart,
        runtime: rt.handle().clone(),
        relay_listen,
        #[cfg(unix)]
        agent_listen,
        dat_root,
    })
    .context("native viewer")
}

#[cfg(feature = "native-window")]
fn run_model_viewer_main_thread(args: Args) -> Result<()> {
    let dat_root = resolve_dat_root(args.require_dat)?;
    let Command::ModelViewer {
        race,
        face,
        head,
        body,
        hands,
        legs,
        feet,
        main,
        sub,
        ranged,
        model_id,
        clip,
    } = args.command
    else {
        unreachable!("dispatched only when args.command is Command::ModelViewer");
    };
    view_native::model_viewer::run(view_native::model_viewer::ModelViewerArgs {
        dat_root,
        race,
        face,
        head,
        body,
        hands,
        legs,
        feet,
        main,
        sub,
        ranged,
        model_id,
        clip,
    })
    .context("model viewer")
}
