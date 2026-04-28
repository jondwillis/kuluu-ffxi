//! FFXI CLI/TUI client — entry point.

use ffxi_client::{
    SessionHandle, agent_io, auth_client, lobby_client, map_client, session, spawn_session, state,
};
mod launcher;
mod view3d;

use anyhow::{self, Context, Result, bail};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "ffxi-client", about = "Agent-drivable FFXI client (LSB/Phoenix).")]
struct Args {
    /// Server hostname or IP.
    #[arg(long, default_value = "127.0.0.1")]
    server: String,

    /// Login auth port (TLS-TCP).
    #[arg(long, default_value_t = ffxi_proto::login::LOGIN_AUTH_PORT)]
    auth_port: u16,

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

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Provision an account (or no-op if it already exists).
    Provision { user: String, password: String },
    /// Create a new character (requires account to exist first).
    CreateChar {
        user: String,
        password: String,
        name: String,
        race: u8,
        job: u8,
        body_type: u8,
        gender: u8,
        face: u8,
        tail: u8,
    },
    /// Authenticate and print the resulting session metadata.
    Login { user: String, password: String },
    /// End-to-end login + lobby handshake; print the map server handoff.
    Lobby {
        user: String,
        password: String,
        char_id: u32,
        char_name: String,
    },
    /// Full login → map UDP bootstrap. Sends the unencrypted 0x00A and
    /// dumps the first encrypted reply.
    MapBootstrap {
        user: String,
        password: String,
        char_id: u32,
        char_name: String,
    },
    /// End-to-end session: auth → lobby → map → zone-in → keepalive,
    /// emitting JSON-line `AgentEvent`s on stdout and reading `AgentCommand`s
    /// from stdin.
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
    },
    /// Same session as `Play`, but renders a Bevy-driven 3D operator
    /// dashboard into the terminal via `bevy_ratatui_camera`. Press `q`,
    /// `Esc`, or `Ctrl+C` to disconnect cleanly.
    ///
    /// All three positional args are optional; when any are missing, the
    /// command drops into an interactive launcher (see `launcher.rs`)
    /// that prompts for credentials, lists characters on the account,
    /// and lets you pick one before entering the TUI's alt-screen mode.
    /// The numeric charid is resolved by name against the lobby's
    /// character list — agents and humans don't need to know it.
    Tui {
        user: Option<String>,
        password: Option<String>,
        char_name: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse args *before* tracing init so we can route logs differently
    // for TUI mode — stderr writes corrupt ratatui's alternate-screen
    // buffer, so the TUI command routes logs to a file instead.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls default crypto provider");

    let args = Args::parse();

    // Logs go to stderr by default (so stdout stays clean for `play`'s
    // JSON-line event stream). `tui` enters alt-screen mode (Bevy +
    // RatatuiPlugins own the terminal) so its logs route to a file
    // instead.
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    if matches!(args.command, Command::Tui { .. }) {
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/ffxi-client.log")
            .context("opening /tmp/ffxi-client.log for tui logs")?;
        tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(log_file))
            .with_ansi(false)
            .with_env_filter(env_filter)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(env_filter)
            .init();
    }

    let auth = auth_client::AuthClient::new(args.server.clone(), args.auth_port);

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
            body_type,
            gender,
            face,
            tail,
        } => {
            auth.ensure_account(&user, &password).await.ok();
            let session = auth.login(&user, &password).await.context("login")?;
            let lobby = lobby_client::LobbyClient::new(
                args.server.clone(),
                args.data_port,
                args.view_port,
            );
            lobby
                .create_character(&session, &name, race, job, body_type, gender, face, tail)
                .await
                .context("character creation")?;
            tracing::info!(char_name = %name, race, job, "character created");
        }
        Command::Login { user, password } => {
            let session = auth
                .login(&user, &password)
                .await
                .context("login attempt")?;
            tracing::info!(
                account_id = session.account_id,
                session_hash = %hex(&session.session_hash),
                cert_sha256 = ?auth.verifier.fingerprint_hex(),
                "login OK"
            );
        }
        Command::MapBootstrap {
            user,
            password,
            char_id,
            char_name,
        } => {
            auth.ensure_account(&user, &password).await.ok();
            let session = auth.login(&user, &password).await.context("login")?;
            let lobby = lobby_client::LobbyClient::new(
                args.server.clone(),
                args.data_port,
                args.view_port,
            );
            let mut key3 = [0u8; 20];
            for (i, b) in key3.iter_mut().enumerate() {
                *b = ((i as u8).wrapping_mul(0x37)) ^ 0x42;
            }
            let handoff = lobby
                .handshake(&session, char_id, &char_name, 0, key3)
                .await
                .context("lobby")?;
            let ip = handoff.server_ip;
            let lobby_ip_str = format!(
                "{}.{}.{}.{}",
                ip & 0xFF,
                (ip >> 8) & 0xFF,
                (ip >> 16) & 0xFF,
                (ip >> 24) & 0xFF,
            );
            let server_addr: std::net::SocketAddr = match args.map_host_override.as_deref() {
                Some(host) => tokio::net::lookup_host((host, handoff.server_port))
                    .await
                    .context("resolving map_host_override")?
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("no addrs for {host}"))?,
                None => format!("{lobby_ip_str}:{}", handoff.server_port)
                    .parse()
                    .context("parsing handoff socket addr")?,
            };
            tracing::info!(
                lobby_said = %lobby_ip_str,
                using = %server_addr,
                "map server endpoint"
            );

            // Give the server a beat for the connect→map ZMQ CharZone message
            // to land and create our pending session. Without this the map
            // server sees a 0x00A with no pending session and silently drops.
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

            let map = map_client::MapClient::connect(server_addr, key3).await?;
            map.send_bootstrap(&map_client::BootstrapArgs {
                char_id,
                char_name: &char_name,
                account_name: &user,
                ticket: session.session_hash,
                version: 0,
                platform: *b"PC\0\0",
                cli_lang: 0,
            })
            .await?;
            tracing::info!(map_server = %server_addr, "0x00A bootstrap sent");

            // Server-side trick: `recv_parse` takes PSession by value, so the
            // caller's pointer stays null after the first bootstrap creates
            // the session — the early-return at map_networking.cpp:95
            // prevents send_parse from running. Retail clients always send a
            // second packet; on that receive `getSessionByIPP` returns the
            // freshly-created session, parse() dispatches our 0x00A
            // sub-packet, and the queued response flushes. We mimic that by
            // sending the same unencrypted bootstrap twice — simpler than
            // implementing FFXI's custom (non-zlib) bit-packed compression
            // for an encrypted second packet.
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            map.send_bootstrap(&map_client::BootstrapArgs {
                char_id,
                char_name: &char_name,
                account_name: &user,
                ticket: session.session_hash,
                version: 0,
                platform: *b"PC\0\0",
                cli_lang: 0,
            })
            .await?;
            tracing::info!("second bootstrap sent, listening for response");

            // Listen for multiple bundles over the zone-in flood window.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let mut total_subs = 0usize;
            loop {
                let now = std::time::Instant::now();
                if now >= deadline {
                    break;
                }
                let remaining = deadline - now;
                match tokio::time::timeout(remaining, map.recv_decrypted()).await {
                    Ok(Ok(buf)) => {
                        let walker = ffxi_proto::framing::walk_sub_packets(
                            &buf[ffxi_proto::framing::FFXI_HEADER_SIZE..],
                        );
                        let mut subs = Vec::new();
                        for r in walker {
                            match r {
                                Ok(sub) => subs.push((sub.opcode, sub.sequence, sub.data.len())),
                                Err(e) => {
                                    tracing::warn!(error = %e, "sub-packet walk error");
                                    break;
                                }
                            }
                        }
                        let opcodes: Vec<String> = subs
                            .iter()
                            .map(|(op, seq, len)| format!("0x{op:03x}@{seq}({len}b)"))
                            .collect();
                        tracing::info!(
                            bundle_bytes = buf.len(),
                            sub_count = subs.len(),
                            "bundle: {}",
                            opcodes.join(" ")
                        );
                        total_subs += subs.len();
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(error = %e, "recv error");
                    }
                    Err(_) => break,
                }
            }
            if total_subs == 0 {
                bail!("no reply from map server");
            }
            tracing::info!(total_subs, "zone-in flood complete");

            // Send encrypted 0x00D NETEND + 0x011 ZONE_TRANSITION to complete
            // the zone-in handshake. After this, the server expects regular
            // 0x015 POS packets to keep the session alive.
            let mut sub_seq: u16 = 2;
            let mut bundle_seq: u16 = 3; // we used 1, 1, 2 so far
            let netend = build_subpacket_netend(sub_seq);
            sub_seq += 1;
            let zone_transition = build_subpacket_zone_transition(sub_seq);
            sub_seq += 1;
            let mut payload = netend;
            payload.extend(zone_transition);
            map.send_encrypted(&payload, bundle_seq, /*ack=*/ 1).await?;
            tracing::info!(bundle_seq, "sent 0x00D + 0x011 (zone-in finalize)");
            bundle_seq += 1;

            // 1 Hz keepalive loop: send 0x015 POS with last-known position.
            // Server-side `cleanupSessions` expires sessions after ~60s of silence.
            // We also drain incoming bundles to keep the parse loop healthy.
            let self_x = 0.0f32;
            let self_y = 0.0f32;
            let self_z = 0.0f32;
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
            tick.tick().await; // first tick fires immediately
            let mut last_recv = std::time::Instant::now();
            for _ in 0..30 {
                tokio::select! {
                    _ = tick.tick() => {
                        let pos = build_subpacket_pos(sub_seq, self_x, self_y, self_z, 0);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map.send_encrypted(&pos, bundle_seq, sub_seq.wrapping_sub(1)).await {
                            tracing::warn!(error = %e, "send POS failed");
                            break;
                        }
                        bundle_seq = bundle_seq.wrapping_add(1);
                    }
                    res = tokio::time::timeout(std::time::Duration::from_millis(50), map.recv_decrypted()) => {
                        if let Ok(Ok(buf)) = res {
                            last_recv = std::time::Instant::now();
                            let walker = ffxi_proto::framing::walk_sub_packets(
                                &buf[ffxi_proto::framing::FFXI_HEADER_SIZE..]
                            );
                            let opcodes: Vec<String> = walker
                                .filter_map(|r| r.ok().map(|s| format!("0x{:03x}", s.opcode)))
                                .collect();
                            tracing::info!(bytes = buf.len(), "in-zone bundle: {}", opcodes.join(" "));
                        }
                    }
                }
                let age_ms = last_recv.elapsed().as_millis();
                if age_ms > 30_000 {
                    tracing::warn!(age_ms, "no server packets for 30s — exiting keepalive");
                    break;
                }
            }
            tracing::info!("keepalive loop ended");
        }
        Command::Play {
            user,
            password,
            char_name,
        } => {
            let lobby = lobby_client::LobbyClient::new(
                args.server.clone(),
                args.data_port,
                args.view_port,
            );

            let (user, password, char_id, char_name, initial_state) =
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
                char_id,
                char_name,
                initial_state: Some(initial_state),
            };
            let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(64);
            let (event_tx, event_rx) = tokio::sync::broadcast::channel(256);
            let session_task = tokio::spawn(session::run(cfg, cmd_rx, event_tx.clone()));
            let agent_task = tokio::spawn(agent_io::run(cmd_tx, event_rx));
            // Whichever finishes first ends the session.
            tokio::select! {
                r = session_task => r??,
                r = agent_task => r??,
            }
        }
        Command::Tui {
            user,
            password,
            char_name,
        } => {
            let lobby = lobby_client::LobbyClient::new(
                args.server.clone(),
                args.data_port,
                args.view_port,
            );

            // Direct mode: all three CLI args present → log in, open the
            // lobby (which fetches the char list), resolve name → charid,
            // run the lobby select on the same handle. Else hand off to
            // the interactive launcher with whatever we *do* have as
            // defaults. Both paths complete the lobby flow *before*
            // entering alt-screen so errors surface cleanly to stderr,
            // and both pass the resulting `InitialState` through to
            // `session::run` so it skips auth + lobby entirely.
            let (user, password, char_id, char_name, initial_state) =
                match (user, password, char_name) {
                    (Some(u), Some(p), Some(name)) => {
                        let session = auth
                            .login(&u, &p)
                            .await
                            .context("auth precheck (tui direct mode)")?;
                        let handle = lobby
                            .open(&session)
                            .await
                            .context("opening lobby (tui direct mode)")?;
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
                            .context("lobby select (tui direct mode)")?;
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
                char_id,
                char_name,
                initial_state: Some(initial_state),
            };
            let SessionHandle {
                state_rx,
                cmd_tx,
                event_tx,
                session_task,
                folder_task,
            } = spawn_session(cfg);

            // JSON event/command log: tokio task subscribes to the broadcast
            // and feeds an unbounded mpsc; Bevy drains it each frame. The
            // shared `show_all` flag is flipped by the `L` key in the input
            // handler — the feeder reads it before pushing each event so the
            // ring buffer doesn't fill with Position spam unless asked.
            let (log_tx, log_rx) = tokio::sync::mpsc::unbounded_channel();
            let show_all = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let log_feeder = tokio::spawn({
                let mut event_rx = event_tx.subscribe();
                let log_tx = log_tx.clone();
                let show_all = show_all.clone();
                async move {
                    while let Ok(ev) = event_rx.recv().await {
                        let suppress = !show_all.load(std::sync::atomic::Ordering::Relaxed)
                            && matches!(
                                &ev,
                                state::AgentEvent::PositionChanged { .. }
                                    | state::AgentEvent::Diagnostics { .. }
                            );
                        if suppress {
                            continue;
                        }
                        if let Ok(json) = serde_json::to_string(&ev) {
                            if log_tx.send(format!("→ {json}")).is_err() {
                                break;
                            }
                        }
                    }
                }
            });

            // Bevy owns its loop; park it on the blocking pool so the
            // tokio runtime keeps draining session/folder tasks.
            let mut view_task = tokio::task::spawn_blocking({
                let state_rx = state_rx.clone();
                let cmd_tx = cmd_tx.clone();
                let log_tx = log_tx.clone();
                let show_all = show_all.clone();
                move || view3d::run(state_rx, cmd_tx, log_rx, log_tx, show_all)
            });
            let mut session_task = session_task;

            // Whichever side finishes first triggers shutdown of the other.
            // - View exits first (q/Esc/Ctrl+C) → input handler already sent
            //   `AgentCommand::Disconnect`; give the session 2s to drain its
            //   shutdown sequence then force-abort if it's stuck.
            // - Session exits first (server disconnect) → no clean way to
            //   tell the Bevy app; abort the view task.
            let outcome: Result<()> = tokio::select! {
                r = &mut session_task => {
                    view_task.abort();
                    let _ = (&mut view_task).await;
                    r?.context("session task")
                }
                r = &mut view_task => {
                    let view_result = r?.context("view task");
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        &mut session_task,
                    ).await {
                        Ok(_) => {}
                        Err(_) => session_task.abort(),
                    }
                    view_result
                }
            };

            // Folder + log_feeder are blocked on `event_rx.recv()`, which
            // only returns `Closed` when *every* broadcast sender drops.
            // session::run's clone is gone by now, but `event_tx` here in
            // main outlives the select! above — drop it explicitly so the
            // receivers wake up. Without this, `q` hangs the process.
            drop(event_tx);
            let _ = folder_task.await;
            let _ = log_feeder.await;

            // Best-effort terminal restore — RatatuiPlugins owns teardown
            // but a panic mid-render could leave us in raw-mode hell.
            let _ = crossterm::terminal::disable_raw_mode();
            let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);

            outcome?;
        }
        Command::Lobby {
            user,
            password,
            char_id,
            char_name,
        } => {
            auth.ensure_account(&user, &password).await.ok(); // best-effort
            let session = auth.login(&user, &password).await.context("login")?;
            let lobby = lobby_client::LobbyClient::new(
                args.server.clone(),
                args.data_port,
                args.view_port,
            );
            // Random key3 per session — server uses it as the Blowfish seed.
            let mut key3 = [0u8; 20];
            for (i, b) in key3.iter_mut().enumerate() {
                *b = (i as u8).wrapping_mul(0x37);
            }
            // search_server_ip: 0 means "let server fill in default".
            let handoff = lobby
                .handshake(&session, char_id, &char_name, 0, key3)
                .await
                .context("lobby handshake")?;
            let ip = handoff.server_ip;
            let ip_str = format!(
                "{}.{}.{}.{}",
                ip & 0xFF,
                (ip >> 8) & 0xFF,
                (ip >> 16) & 0xFF,
                (ip >> 24) & 0xFF
            );
            tracing::info!(
                char_id = handoff.char_id,
                character = %handoff.character_name,
                map_server = %format!("{}:{}", ip_str, handoff.server_port),
                key3 = %hex(&handoff.session_key_seed),
                "lobby handoff OK"
            );
        }
    }

    Ok(())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Build a single GP_CLI_COMMAND_LOGIN sub-packet (4-byte header + 88-byte
/// body) suitable for inclusion as the only sub-packet in an encrypted
/// bundle. Mirrors `map_client::build_bootstrap_packet` body construction
/// without the FFXI bundle header or MD5 trailer.
#[allow(dead_code)]
fn build_encrypted_login_subpacket(
    char_id: u32,
    char_name: &str,
    account_name: &str,
    ticket: [u8; 16],
) -> Vec<u8> {
    let mut buf = vec![0u8; 92];

    let id: u16 = 0x00A;
    let size_words: u16 = 23;
    let header_word = id | (size_words << 9);
    buf[0..2].copy_from_slice(&header_word.to_le_bytes());
    buf[2..4].copy_from_slice(&2u16.to_le_bytes());

    buf[12..16].copy_from_slice(&char_id.to_le_bytes());
    let n = char_name.as_bytes().len().min(15);
    buf[34..34 + n].copy_from_slice(&char_name.as_bytes()[..n]);
    let n = account_name.as_bytes().len().min(15);
    buf[49..49 + n].copy_from_slice(&account_name.as_bytes()[..n]);
    buf[64..80].copy_from_slice(&ticket);
    buf[84..88].copy_from_slice(b"PC\0\0");

    let sum: u32 = buf[8..].iter().map(|&b| b as u32).sum();
    buf[4] = sum as u8;

    buf
}

fn build_subpacket_header(opcode: u16, size_words: u16, sync: u16) -> [u8; 4] {
    let id_and_size = opcode | (size_words << 9);
    let mut h = [0u8; 4];
    h[0..2].copy_from_slice(&id_and_size.to_le_bytes());
    h[2..4].copy_from_slice(&sync.to_le_bytes());
    h
}

/// `GP_CLI_COMMAND_NETEND` — 4-byte header + 4-byte body = 8 bytes (size_words=2).
fn build_subpacket_netend(sync: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x00D, 2, sync));
    // [State u16 = 0, padding00 u16 = 0]
    buf
}

/// `GP_CLI_COMMAND_ZONE_TRANSITION` — 4-byte header + 4-byte body = 8 bytes.
fn build_subpacket_zone_transition(sync: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x011, 2, sync));
    buf
}

/// `GP_CLI_COMMAND_POS` — 4-byte header + 28-byte body = 32 bytes (size_words=8).
/// Heading is a single-byte signed direction (0..=255 mapping to 0°..360°).
fn build_subpacket_pos(sync: u16, x: f32, y: f32, z: f32, heading: u8) -> Vec<u8> {
    let mut buf = vec![0u8; 32];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x015, 8, sync));
    buf[4..8].copy_from_slice(&x.to_le_bytes());
    // server's order is (x, z, y) — z and y swapped from screen-space mental model
    buf[8..12].copy_from_slice(&z.to_le_bytes());
    buf[12..16].copy_from_slice(&y.to_le_bytes());
    // MovTime (u16), MoveFlame (u16) — 0 for stationary
    // dir (i8), mode (bitfield u8)
    buf[20] = heading as u8;
    // facetarget (u16) — leave 0
    // TimeNow (u32) — client-side timestamp; server uses for jitter detection
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);
    buf[24..28].copy_from_slice(&now.to_le_bytes());
    buf
}
