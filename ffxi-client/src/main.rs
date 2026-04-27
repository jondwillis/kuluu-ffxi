#![allow(clippy::type_complexity, clippy::too_many_arguments)]

#[cfg(any(feature = "native-window", feature = "relay"))]
use ffxi_client::state;
use ffxi_client::{agent_io, auth_client, lobby_client, session};

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
    #[arg(long, default_value = "127.0.0.1")]
    server: String,

    #[arg(long, default_value_t = ffxi_proto::login::LOGIN_AUTH_PORT)]
    auth_port: u16,

    #[arg(long, default_value = "json")]
    auth_flavor: auth_client::AuthFlavor,

    #[arg(long)]
    xiloader_version: Option<String>,

    #[arg(long, default_value_t = ffxi_proto::login::LOGIN_DATA_PORT)]
    data_port: u16,

    #[arg(long, default_value_t = ffxi_proto::login::LOGIN_VIEW_PORT)]
    view_port: u16,

    #[arg(long)]
    map_host_override: Option<String>,

    #[cfg(feature = "relay")]
    #[arg(long, value_parser = ffxi_client::relay::parse_relay_listen)]
    relay_listen: Option<std::net::SocketAddr>,

    #[cfg(unix)]
    #[arg(long)]
    agent_listen: Option<String>,

    #[arg(long)]
    require_dat: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Provision {
        user: String,
        password: String,
    },

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

    Play {
        user: Option<String>,
        password: Option<String>,
        char_name: Option<String>,

        #[arg(long)]
        headless: bool,
    },

    #[cfg(feature = "native-window")]
    ModelViewer {
        #[arg(long)]
        race: Option<u8>,

        #[arg(long)]
        face: Option<u8>,

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

        #[arg(long)]
        model_id: Option<String>,

        #[arg(long)]
        clip: Option<String>,
    },
}

fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls default crypto provider");

    let args = Args::parse();

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    // Held for the lifetime of main so the chrome layer flushes its
    // trace-<n>.json when the app exits cleanly (close the window / Esc out —
    // a hard kill skips the flush).
    #[cfg(feature = "trace")]
    let _trace_guard = {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        let (chrome_layer, guard) = tracing_chrome::ChromeLayerBuilder::new().build();
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .with(chrome_layer)
            .init();
        guard
    };
    #[cfg(not(feature = "trace"))]
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(env_filter)
        .init();

    ffxi_client::launcher_store::load().settings.apply_to_env();

    let auth = auth_client::AuthClient::with_flavor_and_version(
        args.server.clone(),
        args.auth_port,
        args.auth_flavor,
        args.xiloader_version.as_deref(),
    );

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;

    #[cfg(feature = "native-window")]
    if matches!(
        args.command,
        Command::Play {
            headless: false,
            ..
        }
    ) {
        let result = run_gui_main_thread(&rt, args, auth);
        drop(rt);
        view_native::exit_watchdog::mark(view_native::exit_watchdog::Stage::RuntimeDropped);
        view_native::exit_watchdog::mark(view_native::exit_watchdog::Stage::MainReturning);
        view_native::exit_watchdog::note_complete();
        return result;
    }
    #[cfg(feature = "native-window")]
    if matches!(args.command, Command::ModelViewer { .. }) {
        return run_model_viewer_main_thread(args);
    }

    rt.block_on(async move { run_command_async(args, auth).await })
}

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
                 Put an install at vendor/game-files/ or set FFXI_DAT_PATH (see README \
                 'Getting the game files'); pass --require-dat to fail fast."
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

                user_driven_events: false,
                dat_root: dat_root.clone(),
            };
            let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(64);
            let (event_tx, event_rx) = tokio::sync::broadcast::channel(1024);
            let session_task = tokio::spawn(session::run(cfg, cmd_rx, event_tx.clone()));
            let agent_task = tokio::spawn(agent_io::run(cmd_tx.clone(), event_rx));

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
                    if let Err(err) =
                        ffxi_client::agent_socket::serve(listen, sock_cmd_tx, sock_event_tx, None)
                            .await
                    {
                        tracing::warn!(error = %err, "agent socket listener exited");
                    }
                });
            }

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
